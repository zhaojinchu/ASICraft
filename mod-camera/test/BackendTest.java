// The mod's end of the backend's socket, against a fake backend (the observer
// mod's WebSocket server, which the tests may use as any server): it connects,
// says hello with the player's name, takes a frame and a drive as they come,
// shrugs off a message it does not understand, and leaves cleanly.
import java.net.ServerSocket;
import java.net.URI;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;

import dev.redfab.camera.Backend;
import dev.redfab.camera.Json;
import dev.redfab.observer.WebSocketServer;

public final class BackendTest {
    private static int failures = 0;

    private static void check(boolean ok, String what) {
        if (!ok) {
            failures++;
            System.out.println("FAIL: " + what);
        }
    }

    private static boolean waitFor(java.util.function.BooleanSupplier condition, long ms) throws InterruptedException {
        long deadline = System.currentTimeMillis() + ms;
        while (System.currentTimeMillis() < deadline) {
            if (condition.getAsBoolean()) return true;
            Thread.sleep(20);
        }
        return condition.getAsBoolean();
    }

    @SuppressWarnings("unchecked")
    public static void main(String[] args) throws Exception {
        int port;
        try (ServerSocket probe = new ServerSocket(0)) {
            port = probe.getLocalPort();
        }
        List<String> received = new CopyOnWriteArrayList<>();
        WebSocketServer server = new WebSocketServer(port, () -> "{\"hello\":\"redfab-live\",\"frame\":{\"plot\":1,\"origin\":[512,64,0],\"size\":[17,4,3]}}", (text, reply) -> received.add(text));
        server.start();

        List<Object[]> frames = new CopyOnWriteArrayList<>();
        List<Object[]> drives = new CopyOnWriteArrayList<>();
        List<Boolean> turns = new CopyOnWriteArrayList<>();
        List<Boolean> ups = new CopyOnWriteArrayList<>();
        List<String> logs = new CopyOnWriteArrayList<>();
        Backend.Listener listener = new Backend.Listener() {
            @Override
            public void frame(int plot, int[] origin, int[] size) {
                frames.add(new Object[] { plot, origin, size });
            }

            @Override
            public void drive(double[] move, double[] look, int plot, boolean owner) {
                drives.add(new Object[] { move, look, plot, owner });
            }

            @Override
            public void turn(boolean on) {
                turns.add(on);
            }

            @Override
            public void connected(boolean up) {
                ups.add(up);
            }
        };
        Backend backend = new Backend(URI.create("ws://127.0.0.1:" + port + "/camera?key=secret"), "Steve_7", listener, logs::add);
        backend.start();

        // The hello goes out with the player's name; the backend's hello (with a frame) is taken.
        check(waitFor(() -> received.size() == 1, 5000), "no hello reached the server: " + received);
        Map<String, Object> hello = (Map<String, Object>) Json.parse(received.get(0));
        check("redfab-camera".equals(hello.get("hello")) && "Steve_7".equals(hello.get("player")), "the hello reads " + received.get(0));
        check(waitFor(() -> ups.contains(true), 5000), "the mod did not report itself connected");
        check(waitFor(() -> frames.size() == 1, 5000) && (int) frames.get(0)[0] == 1 && ((int[]) frames.get(0)[1])[0] == 512 && ((int[]) frames.get(0)[2])[2] == 3, "the hello's frame was not taken");

        // A frame, a drive, and something that is not a message in between.
        server.broadcast("{\"frame\":{\"plot\":3,\"origin\":[1024,64,0],\"size\":[94,4,68]}}");
        check(waitFor(() -> frames.size() == 2, 5000), "the frame did not arrive");
        check((int) frames.get(1)[0] == 3 && ((int[]) frames.get(1)[1])[0] == 1024 && ((int[]) frames.get(1)[2])[0] == 94, "the frame reads wrong");
        server.broadcast("not json at all");
        server.broadcast("{\"something\":\"else\"}");
        server.broadcast("{\"drive\":{\"move\":[1,0,-0.5],\"look\":[12.5,-3],\"plot\":3,\"owner\":true}}");
        check(waitFor(() -> drives.size() == 1, 5000), "the drive did not arrive after the noise");
        double[] move = (double[]) drives.get(0)[0];
        double[] look = (double[]) drives.get(0)[1];
        check(move[0] == 1 && move[2] == -0.5 && look[0] == 12.5 && look[1] == -3 && (int) drives.get(0)[2] == 3 && (boolean) drives.get(0)[3], "the drive reads wrong");
        check(logs.stream().anyMatch((l) -> l.contains("not json")), "the noise was not logged: " + logs);
        check(backend.isConnected(), "the noise disconnected the mod");

        // A turn begins and ends: the camera holds while someone drives, even when they stop to look
        // (found on the deployed camera, 2026-11-11: it glided off two seconds after the last key).
        long unknownBefore = logs.stream().filter((l) -> l.contains("unknown message")).count(); // the noise above is one
        server.broadcast("{\"turn\":true}");
        check(waitFor(() -> turns.size() == 1, 5000) && turns.get(0), "the turn's start did not arrive: " + turns);
        server.broadcast("{\"turn\":false}");
        check(waitFor(() -> turns.size() == 2, 5000) && !turns.get(1), "the turn's end did not arrive: " + turns);
        check(logs.stream().filter((l) -> l.contains("unknown message")).count() == unknownBefore, "a turn message was taken for an unknown one: " + logs);

        // Leaving: the server sees the client go within a second.
        backend.close();
        check(waitFor(() -> server.clientCount() == 0, 2000), "the client did not leave: " + server.clientCount());
        check(waitFor(() -> ups.contains(false), 2000), "the mod did not report itself disconnected");

        if (failures > 0) {
            System.out.println(failures + " backend client checks failed");
            System.exit(1);
        }
        System.out.println("backend client: hello with the player, frames, drives and turns taken, noise ignored, leaves cleanly");
        System.exit(0); // the observer server's accept thread is not a daemon
    }
}
