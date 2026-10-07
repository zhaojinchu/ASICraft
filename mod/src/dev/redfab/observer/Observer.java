// The redfab observer: a server mod that only watches. Every block state change
// in the overworld is collected, and at the end of each server tick the batch
// goes out, with the game time, to every client of a WebSocket on localhost.
// Nothing here changes a block or a tick.
package dev.redfab.observer;

import java.util.ArrayList;
import java.util.List;

import net.fabricmc.api.ModInitializer;

public final class Observer implements ModInitializer {
    /// `-Dredfab.observer.port=N`; 0 means no stream (the mod still loads).
    public static final int PORT = Integer.getInteger("redfab.observer.port", 25570);

    private static final List<String> changes = new ArrayList<>();
    private static long lastTick = -1;
    private static WebSocketServer server;

    @Override
    public void onInitialize() {
        if (PORT > 0) {
            server = new WebSocketServer(PORT, () -> "{\"hello\":\"redfab-observer\",\"tick\":" + lastTick + "}");
            server.start();
        }
        System.out.println("[redfab-observer] watching block changes" + (PORT > 0 ? ", streaming on 127.0.0.1:" + PORT : ""));
    }

    /// A block changed: `pos` as "x,y,z", `state` in command syntax.
    public static void changed(String pos, String state) {
        synchronized (changes) {
            changes.add("[\"" + pos + "\",\"" + state + "\"]");
        }
    }

    /// The server finished tick `tick` (the overworld's game time).
    public static void tickDone(long tick) {
        String batch;
        synchronized (changes) {
            lastTick = tick;
            if (changes.isEmpty() && tick % 20 != 0) {
                return; // a heartbeat every second keeps clients sure of the tick
            }
            batch = "{\"tick\":" + tick + ",\"changes\":[" + String.join(",", changes) + "]}";
            changes.clear();
        }
        if (server != null) {
            server.broadcast(batch);
        }
    }
}
