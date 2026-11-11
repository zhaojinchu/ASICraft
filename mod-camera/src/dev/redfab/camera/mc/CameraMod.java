// The camera mod's game-facing part: a client mod that flies the user's own
// spectator where the backend says. Started with
// -Dredfab.camera.url=wss://NAME/camera?key=KEY it connects to the backend
// (dev.redfab.camera.Backend) with the launcher's player name, and from then
// on, every client tick (MinecraftMixin): while a turn is on, a driver's input
// moves and turns the character in its own frame and the camera holds between
// inputs; otherwise the automatic camera glides to the framing pose above the
// plot the backend last named and orbits it slowly. The position is set
// outright each tick, so the keyboard at the PC does nothing, and the
// character never leaves the plots' area. Out of the world (the server
// restarted under it) it joins again on its own. Without the property the mod
// loads and does nothing.
package dev.redfab.camera.mc;

import java.net.URI;
import java.util.ArrayDeque;
import java.util.Arrays;

import dev.redfab.camera.Backend;
import dev.redfab.camera.CameraMath;
import dev.redfab.camera.CameraMath.Pose;
import net.fabricmc.api.ClientModInitializer;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.ConnectScreen;
import net.minecraft.client.gui.screens.TitleScreen;
import net.minecraft.client.multiplayer.ServerData;
import net.minecraft.client.multiplayer.resolver.ServerAddress;
import net.minecraft.client.player.LocalPlayer;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

public final class CameraMod implements ClientModInitializer {
    public static final Logger LOGGER = LoggerFactory.getLogger("redfab-camera");
    /// The backend's /camera socket with the key; null means the mod stays idle.
    public static final String URL = System.getProperty("redfab.camera.url");

    /// Blocks per tick when driven (12 blocks a second), and up or down.
    static final double SPEED = 0.6;
    /// The glide's fraction of what is left, per tick.
    static final double APPROACH = 0.08;
    /// The idle orbit, degrees per tick (a turn in about 40 s).
    static final double ORBIT = 0.15;
    /// Ticks without a driver's input before the automatic camera takes over again, for a backend
    /// that says nothing of turns. One that does (`turn`) keeps the camera still for the whole turn:
    /// a driver who stops to look must not see it glide off two seconds later.
    static final int HANDS_OFF_TICKS = 40;
    /// The server the game joins through the tunnel, and how long it stays out of the world (and
    /// waits between tries) before it joins again: a restart of the backend, weekly or after a
    /// deploy, left the game on "Connection Lost" and the stream showing it until the PC's 04:00
    /// restart.
    static final String SERVER = System.getProperty("redfab.camera.server", "localhost:25565");
    static final int REJOIN_TICKS = 200;

    record Frame(int plot, int[] origin, int[] size) {}

    record Drive(double[] move, double[] look, int plot, boolean owner) {}

    private static Backend backend;
    private static volatile boolean connected;
    private static volatile Frame pendingFrame;
    private static volatile Boolean pendingTurn;
    private static final ArrayDeque<Drive> drives = new ArrayDeque<>();

    // The client thread's own.
    private static Pose pose;
    private static Frame framed;
    private static Pose framing;
    private static double[] centre;
    private static double angle;
    private static double[] area;
    private static double floorY;
    private static long tick;
    private static long lastDriveTick = Long.MIN_VALUE;
    private static boolean turnKnown; // the backend speaks of turns
    private static boolean manual; // a turn is on: the camera stays where the driver leaves it
    private static long outSince = -1; // the tick the game left the world, or -1 while in it
    private static long lastJoin = Long.MIN_VALUE / 2;

    @Override
    public void onInitializeClient() {
        LOGGER.info(URL == null ? "no redfab.camera.url: the camera mod stays idle" : "the camera mod will connect to the backend once the game ticks");
    }

    /// One client tick, from the end of Minecraft.tick.
    public static void tick(Minecraft mc) {
        if (URL == null) return;
        tick += 1;
        if (backend == null) {
            String player = mc.getUser().getName();
            backend = new Backend(URI.create(URL), player, new Backend.Listener() {
                @Override
                public void frame(int plot, int[] origin, int[] size) {
                    pendingFrame = new Frame(plot, origin, size);
                }

                @Override
                public void drive(double[] move, double[] look, int plot, boolean owner) {
                    synchronized (drives) {
                        drives.add(new Drive(move, look, plot, owner));
                    }
                }

                @Override
                public void turn(boolean on) {
                    pendingTurn = on;
                }

                @Override
                public void connected(boolean up) {
                    connected = up;
                    if (!up) pendingTurn = Boolean.FALSE; // no backend, no driver: back to the automatic camera
                    LOGGER.info(up ? "connected to the backend" : "the backend went; connecting again");
                }
            }, LOGGER::info);
            LOGGER.info("connecting to the backend as {}", player);
            backend.start();
        }
        // Out of the world while the backend answers (so the game is up behind it), and not already
        // connecting or loading: join again, every REJOIN_TICKS until it is in.
        if (mc.level == null) {
            if (outSince < 0) outSince = tick;
            boolean busy = mc.gui.screen() instanceof ConnectScreen || mc.gui.overlay() != null;
            if (connected && !busy && tick - outSince >= REJOIN_TICKS && tick - lastJoin >= REJOIN_TICKS) {
                lastJoin = tick;
                LOGGER.info("out of the world: joining {} again", SERVER);
                ConnectScreen.startConnecting(new TitleScreen(), mc, ServerAddress.parseString(SERVER), new ServerData("redfab", SERVER, ServerData.Type.OTHER), false, null);
            }
        } else {
            outSince = -1;
        }

        LocalPlayer player = mc.player;
        if (player == null || mc.level == null || !player.isSpectator()) {
            pose = null; // the next pose starts from wherever the character is
            return;
        }
        if (pose == null) pose = new Pose(player.getX(), player.getY(), player.getZ(), player.getYRot(), player.getXRot());

        Frame fresh = pendingFrame;
        if (fresh != null) {
            pendingFrame = null;
            double aspect = mc.getWindow().getGuiScaledHeight() == 0 ? 16.0 / 9.0 : mc.getWindow().getGuiScaledWidth() / (double) mc.getWindow().getGuiScaledHeight();
            // The same plot framed again (a lever on it moved) keeps the orbit where it is.
            boolean same = framed != null && framed.plot() == fresh.plot() && Arrays.equals(framed.origin(), fresh.origin()) && Arrays.equals(framed.size(), fresh.size());
            framed = fresh;
            framing = CameraMath.frame(fresh.origin(), fresh.size(), aspect);
            centre = new double[] { fresh.origin()[0] + fresh.size()[0] / 2.0, fresh.origin()[1] + fresh.size()[1] / 2.0, fresh.origin()[2] + fresh.size()[2] / 2.0 };
            if (!same) angle = 0;
            floorY = fresh.origin()[1];
            double[] box = { fresh.origin()[0], fresh.origin()[1], fresh.origin()[2], fresh.origin()[0] + fresh.size()[0], fresh.origin()[1] + fresh.size()[1], fresh.origin()[2] + fresh.size()[2] };
            area = area == null ? box : new double[] { Math.min(area[0], box[0]), Math.min(area[1], box[1]), Math.min(area[2], box[2]), Math.max(area[3], box[3]), Math.max(area[4], box[4]), Math.max(area[5], box[5]) };
            LOGGER.info("framing plot {} at {}", fresh.plot(), framing);
        }

        Boolean turn = pendingTurn;
        if (turn != null) {
            pendingTurn = null;
            turnKnown = true;
            manual = turn;
        }

        Drive drive = null;
        synchronized (drives) {
            while (!drives.isEmpty()) drive = drives.poll(); // the latest input wins; they come ten a second
        }
        boolean held = turnKnown ? manual : tick - lastDriveTick <= HANDS_OFF_TICKS;
        if (drive != null) {
            pose = CameraMath.look(pose, drive.look()[0], drive.look()[1]);
            double[] v = CameraMath.velocity(pose.yaw(), drive.move(), SPEED);
            pose = new Pose(pose.x() + v[0], pose.y() + v[1], pose.z() + v[2], pose.yaw(), pose.pitch());
            lastDriveTick = tick;
        } else if (framing != null && !held) {
            angle += ORBIT;
            pose = CameraMath.approach(pose, CameraMath.orbit(framing, centre, angle), APPROACH);
        }
        if (area != null) {
            double[] p = CameraMath.clamp(new double[] { pose.x(), pose.y(), pose.z() }, area, floorY);
            pose = new Pose(p[0], p[1], p[2], pose.yaw(), pose.pitch());
        }
        player.snapTo(pose.x(), pose.y(), pose.z(), (float) pose.yaw(), (float) pose.pitch());
        player.setDeltaMovement(0, 0, 0);
    }

    /// Whether the HUD is hidden: always while this is the camera (the property is set), so the
    /// stream is the game's picture alone; the page says which plot is on camera and who drives.
    public static boolean hidesHud() {
        return URL != null;
    }
}
