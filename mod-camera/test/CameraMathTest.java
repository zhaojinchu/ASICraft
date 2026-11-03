// The camera's arithmetic, held without the game: a plot is framed inside the
// view with the camera above and south of it looking north and down; the idle
// orbit keeps its distance and keeps facing the centre; the approach converges
// and never overshoots; a drive input moves in the character's own frame, with
// the game's yaw convention (0 south, 90 west, 180 north, -90 east); the
// position stays inside the plots' area.
import dev.redfab.camera.CameraMath;
import dev.redfab.camera.CameraMath.Pose;

public final class CameraMathTest {
    private static int failures = 0;

    private static void check(boolean ok, String what) {
        if (!ok) {
            failures++;
            System.out.println("FAIL: " + what);
        }
    }

    private static boolean near(double a, double b, double eps) {
        return Math.abs(a - b) <= eps;
    }

    public static void main(String[] args) {
        // Framing: the counter's plot, 94 x 4 x 68 at (1024, 64, 0).
        Pose pose = CameraMath.frame(new int[] { 1024, 64, 0 }, new int[] { 94, 4, 68 }, 16.0 / 9.0);
        double cx = 1024 + 94 / 2.0, cz = 0 + 68 / 2.0;
        check(near(pose.x(), cx, 1e-9), "the framing pose is centred east-west: " + pose);
        check(pose.z() > cz, "the framing pose is south of the centre: " + pose);
        check(pose.y() > 64 + 4, "the framing pose is above the plot: " + pose);
        check(near(pose.yaw(), 180, 1e-9), "the framing pose faces north: " + pose);
        check(pose.pitch() > 45 && pose.pitch() < 90, "the framing pose looks down at an angle: " + pose);
        // Every corner of the plot is inside the vertical and horizontal field of view.
        check(CameraMath.fits(pose, new int[] { 1024, 64, 0 }, new int[] { 94, 4, 68 }, CameraMath.VERTICAL_FOV, 16.0 / 9.0), "the plot does not fit the view: " + pose);
        // A larger plot is seen from further away, a smaller one from nearer.
        Pose big = CameraMath.frame(new int[] { 0, 64, 0 }, new int[] { 300, 4, 300 }, 16.0 / 9.0);
        Pose small = CameraMath.frame(new int[] { 0, 64, 0 }, new int[] { 17, 4, 3 }, 16.0 / 9.0);
        check(big.y() > pose.y() && pose.y() > small.y(), "the distance does not follow the size: " + big + " " + pose + " " + small);
        check(CameraMath.fits(big, new int[] { 0, 64, 0 }, new int[] { 300, 4, 300 }, CameraMath.VERTICAL_FOV, 16.0 / 9.0), "the large plot does not fit: " + big);
        check(small.y() >= 64 + 4 + CameraMath.MIN_HEIGHT, "the small plot is framed from too close: " + small);

        // The orbit: at any angle the camera is the same distance from the centre and faces it.
        double[] centre = { cx, 64 + 2, cz };
        double distance = CameraMath.distance(pose, centre);
        for (int step = 0; step < 8; step++) {
            double angle = step * 45;
            Pose orbit = CameraMath.orbit(pose, centre, angle);
            check(near(CameraMath.distance(orbit, centre), distance, 1e-6), "the orbit changed the distance at " + angle + ": " + orbit);
            check(near(orbit.pitch(), pose.pitch(), 1e-9), "the orbit changed the pitch at " + angle);
            double facing = CameraMath.yawToward(orbit, centre);
            check(near(CameraMath.yawDifference(orbit.yaw(), facing), 0, 1e-6), "the orbit does not face the centre at " + angle + ": yaw " + orbit.yaw() + " toward " + facing);
        }
        check(near(CameraMath.orbit(pose, centre, 0).x(), pose.x(), 1e-9) && near(CameraMath.orbit(pose, centre, 0).z(), pose.z(), 1e-9), "the orbit at angle 0 is not the framing pose");

        // The approach: monotone, converging, no overshoot, and the yaw takes the short way round.
        Pose from = new Pose(0, 70, 0, 170, 30);
        Pose to = new Pose(100, 120, -50, -170, 60);
        Pose at = from;
        double lastGap = CameraMath.distance(from, new double[] { to.x(), to.y(), to.z() });
        for (int tick = 0; tick < 200; tick++) {
            at = CameraMath.approach(at, to, 0.1);
            double gap = CameraMath.distance(at, new double[] { to.x(), to.y(), to.z() });
            check(gap <= lastGap + 1e-9, "the approach moved away at tick " + tick);
            lastGap = gap;
            check(at.yaw() >= 170 || at.yaw() <= -170, "the yaw went the long way round at tick " + tick + ": " + at.yaw());
        }
        check(lastGap < 0.01 && near(CameraMath.yawDifference(at.yaw(), to.yaw()), 0, 0.01) && near(at.pitch(), to.pitch(), 0.01), "the approach did not converge: " + at);

        // Driving: forward follows the yaw (0 south is +z, 90 west is -x, 180 north is -z, -90 east is +x), right is to the right, up is up.
        double speed = 0.5;
        double[] south = CameraMath.velocity(0, new double[] { 0, 0, 1 }, speed);
        check(near(south[0], 0, 1e-9) && near(south[2], speed, 1e-9), "forward at yaw 0 is not south: " + south[0] + "," + south[2]);
        double[] west = CameraMath.velocity(90, new double[] { 0, 0, 1 }, speed);
        check(near(west[0], -speed, 1e-9) && near(west[2], 0, 1e-9), "forward at yaw 90 is not west: " + west[0] + "," + west[2]);
        double[] north = CameraMath.velocity(180, new double[] { 0, 0, 1 }, speed);
        check(near(north[2], -speed, 1e-9), "forward at yaw 180 is not north");
        double[] rightOfSouth = CameraMath.velocity(0, new double[] { 1, 0, 0 }, speed);
        check(near(rightOfSouth[0], -speed, 1e-9), "right when facing south is not west: " + rightOfSouth[0]);
        double[] up = CameraMath.velocity(37, new double[] { 0, 1, 0 }, speed);
        check(near(up[1], speed, 1e-9) && near(up[0], 0, 1e-9) && near(up[2], 0, 1e-9), "up is not straight up");
        double[] diagonal = CameraMath.velocity(0, new double[] { 1, 0, 1 }, speed);
        check(near(Math.hypot(diagonal[0], diagonal[2]), speed, 1e-9), "a diagonal is faster than straight: " + Math.hypot(diagonal[0], diagonal[2]));

        // The look: yaw wraps, pitch is clamped to straight up and straight down.
        Pose looked = CameraMath.look(new Pose(0, 0, 0, 170, 80), 20, 20);
        check(near(looked.yaw(), -170, 1e-9), "the yaw did not wrap: " + looked.yaw());
        check(near(looked.pitch(), 90, 1e-9), "the pitch went past straight down: " + looked.pitch());

        // The bounds: the position stays over the plots' row, with a margin, and between the floor and the ceiling.
        double[] area = { 0, 64, 0, 8 * 512, 64 + 4, 512 }; // x0, y0, z0, x1, y1, z1 of the plots
        double[] inside = CameraMath.clamp(new double[] { 100, 80, 100 }, area, 64);
        check(inside[0] == 100 && inside[1] == 80 && inside[2] == 100, "a position inside moved");
        double[] far = CameraMath.clamp(new double[] { -1000, 10, 5000 }, area, 64);
        check(far[0] >= -64 && far[2] <= 512 + 64 && far[1] >= 64 - 8, "a position outside was not brought back: " + far[0] + "," + far[1] + "," + far[2]);
        double[] high = CameraMath.clamp(new double[] { 100, 9999, 100 }, area, 64);
        check(high[1] <= 64 + 4 + 64, "a position too high was not brought down: " + high[1]);

        if (failures > 0) {
            System.out.println(failures + " camera math checks failed");
            System.exit(1);
        }
        System.out.println("camera math: framing, orbit, approach, driving, look and bounds hold");
    }
}
