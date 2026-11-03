// The camera's arithmetic, with nothing of the game in it (mod-camera/test
// holds it): where to stand to frame a plot, how to orbit it while idle, how
// to glide from one pose to the next, how a driver's input moves the
// character in its own frame, and where the character may not go.
//
// The game's conventions: x east, y up, z south; yaw 0 faces south (+z), 90
// west (-x), 180 north (-z), -90 east (+x); pitch is positive looking down.
package dev.redfab.camera;

public final class CameraMath {
    /// A position and where it looks, in the game's conventions.
    public record Pose(double x, double y, double z, double yaw, double pitch) {
        @Override
        public String toString() {
            return String.format("(%.1f, %.1f, %.1f) yaw %.1f pitch %.1f", x, y, z, yaw, pitch);
        }
    }

    /// The game's default vertical field of view, in degrees.
    public static final double VERTICAL_FOV = 70;
    /// The framing pose looks down at this angle, so the circuit reads as a plan with some relief.
    public static final double FRAMING_PITCH = 65;
    /// The framing pose stands at least this far above the plot's top.
    public static final double MIN_HEIGHT = 6;
    /// The part of the view the plot may fill, leaving an edge.
    private static final double FILL = 0.88;
    /// How far past the plots' row the character may wander, and how high above them.
    private static final double MARGIN = 64;
    private static final double BELOW = 8;

    private CameraMath() {}

    private static double rad(double degrees) {
        return Math.toRadians(degrees);
    }

    /// The direction a pose looks along.
    private static double[] forward(Pose p) {
        double yaw = rad(p.yaw()), pitch = rad(p.pitch());
        return new double[] { -Math.sin(yaw) * Math.cos(pitch), -Math.sin(pitch), Math.cos(yaw) * Math.cos(pitch) };
    }

    /// The pose's right, on the horizontal.
    private static double[] right(double yawDegrees) {
        double yaw = rad(yawDegrees);
        return new double[] { -Math.cos(yaw), 0, -Math.sin(yaw) };
    }

    private static double[] cross(double[] a, double[] b) {
        return new double[] { a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0] };
    }

    private static double dot(double[] a, double[] b) {
        return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    }

    /// Whether every corner of the plot's box is inside the view from `pose`,
    /// with the vertical field of view `vfov` (degrees) and the screen's aspect.
    public static boolean fits(Pose pose, int[] origin, int[] size, double vfov, double aspect) {
        double[] f = forward(pose);
        double[] r = right(pose.yaw());
        double[] u = cross(r, f);
        double tanV = Math.tan(rad(vfov / 2)) * FILL;
        double tanH = tanV * aspect;
        for (int corner = 0; corner < 8; corner++) {
            double[] v = {
                origin[0] + ((corner & 1) == 0 ? 0 : size[0]) - pose.x(),
                origin[1] + ((corner & 2) == 0 ? 0 : size[1]) - pose.y(),
                origin[2] + ((corner & 4) == 0 ? 0 : size[2]) - pose.z(),
            };
            double depth = dot(v, f);
            if (depth <= 0) return false;
            if (Math.abs(dot(v, u)) > tanV * depth) return false;
            if (Math.abs(dot(v, r)) > tanH * depth) return false;
        }
        return true;
    }

    /// Where to stand to see the whole plot: above and south of its centre,
    /// facing north, looking down at FRAMING_PITCH, as near as the view allows.
    public static Pose frame(int[] origin, int[] size, double aspect) {
        double cx = origin[0] + size[0] / 2.0;
        double cy = origin[1] + size[1] / 2.0;
        double cz = origin[2] + size[2] / 2.0;
        double top = origin[1] + size[1];
        double far = Math.max(size[0], size[2]) * 4 + 64;
        Pose candidate = null;
        // The nearest distance along the viewing line at which the plot fits: doubled until it does, then halved in.
        double lo = 1, hi = far;
        if (!fits(at(cx, cy, cz, hi), origin, size, VERTICAL_FOV, aspect)) hi = far * 4;
        for (int i = 0; i < 40; i++) {
            double mid = (lo + hi) / 2;
            if (fits(at(cx, cy, cz, mid), origin, size, VERTICAL_FOV, aspect)) hi = mid;
            else lo = mid;
        }
        candidate = at(cx, cy, cz, hi);
        if (candidate.y() < top + MIN_HEIGHT) {
            double d = (top + MIN_HEIGHT - cy) / Math.sin(rad(FRAMING_PITCH));
            candidate = at(cx, cy, cz, d);
        }
        return candidate;
    }

    /// The pose `distance` from the centre, up and south of it, looking down at the centre.
    private static Pose at(double cx, double cy, double cz, double distance) {
        double pitch = rad(FRAMING_PITCH);
        return new Pose(cx, cy + distance * Math.sin(pitch), cz + distance * Math.cos(pitch), 180, FRAMING_PITCH);
    }

    public static double distance(Pose pose, double[] centre) {
        double dx = pose.x() - centre[0], dy = pose.y() - centre[1], dz = pose.z() - centre[2];
        return Math.sqrt(dx * dx + dy * dy + dz * dz);
    }

    /// The yaw that faces `centre` from the pose, on the horizontal.
    public static double yawToward(Pose pose, double[] centre) {
        double dx = centre[0] - pose.x(), dz = centre[2] - pose.z();
        return Math.toDegrees(Math.atan2(-dx, dz));
    }

    /// The signed difference from `from` to `to`, the short way round, in (-180, 180].
    public static double yawDifference(double from, double to) {
        double d = (to - from) % 360;
        if (d <= -180) d += 360;
        if (d > 180) d -= 360;
        return d;
    }

    private static double wrapYaw(double yaw) {
        double w = (yaw + 180) % 360;
        if (w < 0) w += 360;
        return w - 180;
    }

    /// The framing pose swung around the centre by `angle` degrees, still facing it.
    public static Pose orbit(Pose framing, double[] centre, double angle) {
        double a = rad(angle);
        double ox = framing.x() - centre[0], oy = framing.y() - centre[1], oz = framing.z() - centre[2];
        double x = centre[0] + ox * Math.cos(a) - oz * Math.sin(a);
        double z = centre[2] + ox * Math.sin(a) + oz * Math.cos(a);
        Pose moved = new Pose(x, centre[1] + oy, z, framing.yaw(), framing.pitch());
        return new Pose(x, moved.y(), z, yawToward(moved, centre), framing.pitch());
    }

    /// One tick of gliding from `from` toward `to`: the fraction `k` of what is
    /// left, the yaw the short way round.
    public static Pose approach(Pose from, Pose to, double k) {
        return new Pose(
            from.x() + (to.x() - from.x()) * k,
            from.y() + (to.y() - from.y()) * k,
            from.z() + (to.z() - from.z()) * k,
            wrapYaw(from.yaw() + yawDifference(from.yaw(), to.yaw()) * k),
            from.pitch() + (to.pitch() - from.pitch()) * k);
    }

    /// The velocity for a drive input `move` (right, up, forward, each -1..1)
    /// at `yaw`, at most `speed` on the horizontal and `speed` vertically.
    public static double[] velocity(double yaw, double[] move, double speed) {
        double[] f = { -Math.sin(rad(yaw)), 0, Math.cos(rad(yaw)) };
        double[] r = right(yaw);
        double vx = r[0] * move[0] + f[0] * move[2];
        double vz = r[2] * move[0] + f[2] * move[2];
        double horizontal = Math.hypot(vx, vz);
        if (horizontal > 1) {
            vx /= horizontal;
            vz /= horizontal;
        }
        double vy = Math.max(-1, Math.min(1, move[1]));
        return new double[] { vx * speed, vy * speed, vz * speed };
    }

    /// The pose turned by a look input, the yaw wrapping and the pitch stopping at straight up and down.
    public static Pose look(Pose pose, double dyaw, double dpitch) {
        return new Pose(pose.x(), pose.y(), pose.z(), wrapYaw(pose.yaw() + dyaw), Math.max(-90, Math.min(90, pose.pitch() + dpitch)));
    }

    /// The position kept over the plots' row (`area`: x0, y0, z0, x1, y1, z1)
    /// with a margin, no lower than a little under the plots and no higher than
    /// a margin above them.
    public static double[] clamp(double[] p, double[] area, double floorY) {
        return new double[] {
            Math.max(area[0] - MARGIN, Math.min(area[3] + MARGIN, p[0])),
            Math.max(floorY - BELOW, Math.min(area[4] + MARGIN, p[1])),
            Math.max(area[2] - MARGIN, Math.min(area[5] + MARGIN, p[2])),
        };
    }
}
