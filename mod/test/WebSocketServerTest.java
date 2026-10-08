// The stream server alone, without Minecraft: every client gets a writer
// thread, and both the writer and the client thread must end when the client
// goes, whether by a close frame or by dropping the socket. Run by
// tools/build_mod.sh after the build; fails with a non-zero status.
import dev.redfab.observer.WebSocketServer;

import java.io.InputStream;
import java.io.OutputStream;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;

public final class WebSocketServerTest {
    private static final int CLIENTS = 20;

    private static long threadsNamed(String name) {
        return Thread.getAllStackTraces().keySet().stream().filter(t -> t.getName().equals(name)).count();
    }

    private static int freePort() throws Exception {
        try (ServerSocket probe = new ServerSocket(0)) {
            return probe.getLocalPort();
        }
    }

    /// Opens a handshaken client and reads the hello frame.
    private static Socket client(int port) throws Exception {
        Socket socket = new Socket("127.0.0.1", port);
        OutputStream out = socket.getOutputStream();
        out.write(("GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
            + "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
        out.flush();
        InputStream in = socket.getInputStream();
        StringBuilder response = new StringBuilder();
        int b;
        while ((b = in.read()) >= 0) {
            response.append((char) b);
            if (response.toString().endsWith("\r\n\r\n")) break;
        }
        if (!response.toString().startsWith("HTTP/1.1 101")) throw new AssertionError("no upgrade: " + response);
        if (in.read() != 0x81) throw new AssertionError("the hello is not a text frame");
        int length = in.read() & 0x7F;
        if (in.readNBytes(length).length != length) throw new AssertionError("a short hello");
        return socket;
    }

    private static void waitUntil(java.util.function.BooleanSupplier done, long ms) throws InterruptedException {
        long deadline = System.currentTimeMillis() + ms;
        while (!done.getAsBoolean() && System.currentTimeMillis() < deadline) Thread.sleep(20);
    }

    public static void main(String[] args) throws Exception {
        int port = freePort();
        WebSocketServer server = new WebSocketServer(port, () -> "{\"hello\":\"test\",\"tick\":0}", (request, reply) -> reply.accept("{\"ok\":true}"));
        server.start();
        Socket[] sockets = new Socket[CLIENTS];
        for (int i = 0; i < CLIENTS; i++) {
            for (int attempt = 0; ; attempt++) {
                try {
                    sockets[i] = client(port);
                    break;
                } catch (java.net.ConnectException e) {
                    if (attempt == 50) throw e; // the acceptor is not listening yet
                    Thread.sleep(20);
                }
            }
        }
        waitUntil(() -> server.clientCount() == CLIENTS && threadsNamed("redfab-observer-writer") == CLIENTS, 3000);
        long writers = threadsNamed("redfab-observer-writer");
        System.out.println("connected: clients=" + server.clientCount() + " writer threads=" + writers);
        if (server.clientCount() != CLIENTS || writers != CLIENTS) {
            System.out.println("FAIL: every client should have one writer thread");
            System.exit(1);
        }
        server.broadcast("{\"tick\":1,\"changes\":[]}");

        // Half the clients say goodbye with a close frame; the other half just drop the socket.
        for (int i = 0; i < CLIENTS; i++) {
            if (i % 2 == 0) {
                OutputStream out = sockets[i].getOutputStream();
                out.write(new byte[] {(byte) 0x88, (byte) 0x80, 1, 2, 3, 4}); // a masked, empty close frame
                out.flush();
            }
            sockets[i].close();
        }
        long started = System.currentTimeMillis();
        waitUntil(() -> threadsNamed("redfab-observer-writer") == 0 && threadsNamed("redfab-observer-client") == 0 && server.clientCount() == 0, 3000);
        long elapsed = System.currentTimeMillis() - started;
        long writersLeft = threadsNamed("redfab-observer-writer");
        long clientsLeft = threadsNamed("redfab-observer-client");
        System.out.println("after " + elapsed + " ms: clients=" + server.clientCount() + " writer threads=" + writersLeft + " client threads=" + clientsLeft);
        if (writersLeft != 0 || clientsLeft != 0 || server.clientCount() != 0) {
            System.out.println("FAIL: a thread outlived its client (the writer must end within a second of the client going)");
            System.exit(1);
        }
        System.out.println("PASS: " + CLIENTS + " clients went, every thread ended");
        System.exit(0);
    }
}
