// The redfab observer: a server mod that watches. Every block state change
// in the overworld is collected, and at the end of each server tick the batch
// goes out, with the game time, to every client of a WebSocket on localhost.
// Changes made between ticks go out as a tick starts, under the number of the
// tick just completed: that is when commands run, and what they do counts as
// the last thing in that tick.
//
// Nothing here changes a block or a tick. The one thing a client may ask for
// is `{"forget": "redfab:NAME"}`: the structure template of that name is
// dropped from the server's template cache, which otherwise keeps every
// placed template until a reload. That touches no block; it is the one
// departure from observing only, and it is limited to the redfab namespace.
package dev.redfab.observer;

import java.util.ArrayList;
import java.util.List;
import java.util.function.Consumer;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import net.fabricmc.api.ModInitializer;
import net.minecraft.resources.Identifier;
import net.minecraft.server.MinecraftServer;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

public final class Observer implements ModInitializer {
    public static final Logger LOGGER = LoggerFactory.getLogger("redfab-observer");

    /// `-Dredfab.observer.port=N`; 0 means no stream (the mod still loads).
    public static final int PORT = Integer.getInteger("redfab.observer.port", 25570);

    private static final Pattern FORGET = Pattern.compile("^\\s*\\{\\s*\"forget\"\\s*:\\s*\"(redfab:[a-z0-9_./-]+)\"\\s*\\}\\s*$");

    private static final List<String> changes = new ArrayList<>();
    private static long lastTick = -1;
    private static WebSocketServer server;
    private static volatile MinecraftServer minecraft;

    @Override
    public void onInitialize() {
        if (PORT > 0) {
            server = new WebSocketServer(PORT, () -> "{\"hello\":\"redfab-observer\",\"tick\":" + lastTick + "}", Observer::request);
            server.start();
        }
        LOGGER.info("watching block changes{}", PORT > 0 ? ", streaming on 127.0.0.1:" + PORT : "");
    }

    /// The server, once it ticks; the forget request needs its template manager.
    public static void attach(MinecraftServer server) {
        if (minecraft == null) minecraft = server;
    }

    /// A block changed: `pos` as "x,y,z", `state` in command syntax.
    public static void changed(String pos, String state) {
        synchronized (changes) {
            changes.add("[\"" + pos + "\",\"" + state + "\"]");
        }
    }

    /// The server finished tick `tick` (the overworld's game time).
    public static void tickDone(long tick) {
        flush(tick, true);
    }

    /// A tick is about to start; `tick` is still the last completed one. What
    /// changed since its end happened between ticks and belongs to it.
    public static void betweenTicks(long tick) {
        flush(tick, false);
    }

    private static void flush(long tick, boolean heartbeat) {
        String batch;
        synchronized (changes) {
            lastTick = tick;
            if (changes.isEmpty() && !heartbeat) {
                return; // the end of every tick goes out even when nothing changed
            }
            batch = "{\"tick\":" + tick + ",\"changes\":[" + String.join(",", changes) + "]}";
            changes.clear();
        }
        if (server != null) {
            server.broadcast(batch);
        }
    }

    /// A text frame from a client. Only the forget request is understood; it is
    /// answered with {"forgot": NAME} once the server thread has dropped the template.
    static void request(String text, Consumer<String> reply) {
        Matcher m = FORGET.matcher(text);
        MinecraftServer current = minecraft;
        if (!m.matches() || current == null) {
            reply.accept("{\"error\":\"the only request is {\\\"forget\\\": \\\"redfab:NAME\\\"}\"}");
            return;
        }
        String name = m.group(1);
        Identifier id = Identifier.tryParse(name);
        if (id == null) {
            reply.accept("{\"error\":\"not a template name\"}");
            return;
        }
        current.execute(() -> {
            current.getStructureTemplateManager().remove(id);
            reply.accept("{\"forgot\":\"" + name + "\"}");
        });
    }
}
