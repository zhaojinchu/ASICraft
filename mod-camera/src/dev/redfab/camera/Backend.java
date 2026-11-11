// The mod's end of the backend's /camera socket (server/camera.mjs): connects
// outbound with the JDK's own WebSocket client, says hello with the player's
// name, takes the frames and the drives as they come, and comes back on its
// own when the socket drops, waiting a little longer each time. Nothing of
// the game in here; mod-camera/test/BackendTest.java holds it against a fake
// backend. The listener is called on the client's threads: the game-facing
// part queues what it hears for the client tick.
package dev.redfab.camera;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.WebSocket;
import java.time.Duration;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;

public final class Backend implements AutoCloseable {
    public interface Listener {
        /// Where to look: the plot's number, its origin and the design's size, in world blocks.
        void frame(int plot, int[] origin, int[] size);

        /// A driver's input: a move (right, up, forward, each -1..1), a look (yaw and pitch to add, degrees).
        void drive(double[] move, double[] look, int plot, boolean owner);

        /// A turn began (true: someone drives, so the camera holds where they leave it) or ended
        /// (false: the automatic camera takes over again).
        void turn(boolean on);

        /// The backend said hello (true), or the socket went (false).
        void connected(boolean up);
    }

    private static final long FIRST_RETRY_MS = 1000;
    private static final long LONGEST_RETRY_MS = 30_000;

    private final URI url;
    private final String player;
    private final Listener listener;
    private final Consumer<String> log;
    private final HttpClient http = HttpClient.newHttpClient();
    private final ScheduledExecutorService timer = Executors.newSingleThreadScheduledExecutor((r) -> {
        Thread t = new Thread(r, "redfab-camera-backend");
        t.setDaemon(true);
        return t;
    });
    private volatile WebSocket socket;
    private volatile boolean connected;
    private volatile boolean closed;
    private long retryMs = FIRST_RETRY_MS;

    public Backend(URI url, String player, Listener listener, Consumer<String> log) {
        this.url = url;
        this.player = player;
        this.listener = listener;
        this.log = log;
    }

    public void start() {
        timer.execute(this::connect);
    }

    public boolean isConnected() {
        return connected;
    }

    private void connect() {
        if (closed) return;
        http.newWebSocketBuilder().connectTimeout(Duration.ofSeconds(10)).buildAsync(url, new WebSocket.Listener() {
            private final StringBuilder partial = new StringBuilder();

            @Override
            public void onOpen(WebSocket ws) {
                socket = ws;
                retryMs = FIRST_RETRY_MS;
                ws.sendText(Json.write(Map.of("hello", "redfab-camera", "player", player)), true);
                ws.request(1);
            }

            @Override
            public CompletionStage<?> onText(WebSocket ws, CharSequence data, boolean last) {
                partial.append(data);
                if (last) {
                    handle(partial.toString());
                    partial.setLength(0);
                }
                ws.request(1);
                return null;
            }

            @Override
            public CompletionStage<?> onClose(WebSocket ws, int code, String reason) {
                dropped("closed (" + code + (reason.isEmpty() ? "" : " " + reason) + ")");
                return null;
            }

            @Override
            public void onError(WebSocket ws, Throwable error) {
                dropped(String.valueOf(error.getMessage()));
            }
        }).whenComplete((ws, error) -> {
            if (error != null) dropped(String.valueOf(error.getCause() != null ? error.getCause().getMessage() : error.getMessage()));
        });
    }

    private synchronized void dropped(String why) {
        boolean was = connected;
        connected = false;
        socket = null;
        if (was) listener.connected(false);
        if (closed) return;
        log.accept("backend: " + why + "; connecting again in " + retryMs / 1000 + " s");
        timer.schedule(this::connect, retryMs, TimeUnit.MILLISECONDS);
        retryMs = Math.min(retryMs * 2, LONGEST_RETRY_MS);
    }

    private void handle(String text) {
        Object parsed;
        try {
            parsed = Json.parse(text);
        } catch (IllegalArgumentException e) {
            log.accept("backend: not json: " + abbreviate(text));
            return;
        }
        if (!(parsed instanceof Map<?, ?> m)) {
            log.accept("backend: not a message: " + abbreviate(text));
            return;
        }
        if ("redfab-live".equals(m.get("hello"))) {
            connected = true;
            listener.connected(true);
            if (m.get("frame") instanceof Map<?, ?> f) frame(f);
            if (m.get("turn") instanceof Boolean on) listener.turn(on);
            return;
        }
        if (m.get("frame") instanceof Map<?, ?> f) {
            frame(f);
            return;
        }
        if (m.get("turn") instanceof Boolean on) {
            listener.turn(on);
            return;
        }
        if (m.get("drive") instanceof Map<?, ?> d) {
            drive(d);
            return;
        }
        if (m.containsKey("error")) {
            log.accept("backend: " + m.get("error"));
            return;
        }
        log.accept("backend: unknown message: " + abbreviate(text));
    }

    private void frame(Map<?, ?> f) {
        try {
            listener.frame(((Number) f.get("plot")).intValue(), ints(f.get("origin"), 3), ints(f.get("size"), 3));
        } catch (RuntimeException e) {
            log.accept("backend: a frame that does not read: " + e.getMessage());
        }
    }

    private void drive(Map<?, ?> d) {
        try {
            listener.drive(doubles(d.get("move"), 3), doubles(d.get("look"), 2), ((Number) d.get("plot")).intValue(), Boolean.TRUE.equals(d.get("owner")));
        } catch (RuntimeException e) {
            log.accept("backend: a drive that does not read: " + e.getMessage());
        }
    }

    private static int[] ints(Object value, int n) {
        double[] d = doubles(value, n);
        int[] out = new int[n];
        for (int i = 0; i < n; i++) out[i] = (int) Math.round(d[i]);
        return out;
    }

    private static double[] doubles(Object value, int n) {
        if (!(value instanceof List<?> list) || list.size() != n) throw new IllegalArgumentException("expected " + n + " numbers");
        double[] out = new double[n];
        for (int i = 0; i < n; i++) out[i] = ((Number) list.get(i)).doubleValue();
        return out;
    }

    private static String abbreviate(String text) {
        return text.length() > 80 ? text.substring(0, 80) + "..." : text;
    }

    @Override
    public void close() {
        closed = true;
        WebSocket ws = socket;
        socket = null;
        if (ws != null) ws.sendClose(WebSocket.NORMAL_CLOSURE, "closing");
        boolean was = connected;
        connected = false;
        if (was) listener.connected(false);
        timer.shutdownNow();
    }
}
