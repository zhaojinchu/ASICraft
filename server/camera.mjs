// The camera: the user's own game client, logged in on their PC, joined to
// the server as a spectator and streamed to the page. Its client-side mod
// (mod-camera/) connects here, outbound, on /camera with a shared key; this
// module tells it where to look and relays a visitor's driving to it, and
// decides who drives: the plot's owner at once and for as long as the plot is
// theirs, any other viewer in a queue of timed turns, idle hands giving it
// back, and the automatic camera (the plot last placed or touched) when nobody
// holds it. The relay's API on loopback says whether the video is up.
//
// Mod -> backend:  {"hello": "redfab-camera", "player": NAME}
// Backend -> mod:  {"hello": "redfab-live", "frame": ...}
//                  {"frame": {"plot": N, "origin": [x, y, z], "size": [sx, sy, sz]}}
//                  {"drive": {"move": [x, y, z], "look": [yaw, pitch], "plot": N, "owner": bool}}
// Viewer -> backend (on /live):  {"drive": "take"} | {"drive": "release"} | {"drive": {"move", "look"}}
// Backend -> viewer:  {"drive": "yours", "seconds": S | null} | {"drive": "queued", "position": P}
//                     {"drive": "over", "why": ...} | {"ok": false, "error": ...}

export const CAMERA_DEFAULTS = {
    turnSeconds: 60, // a visitor's turn; the plot's owner has no limit while the plot is theirs
    idleSeconds: 10, // a turn with no input for this long ends
    pollSeconds: 5, // how often the relay's API is asked whether the stream is up
    tickMs: 250, // how often the turn's clocks are looked at
};

const PLAYER_NAME = /^[A-Za-z0-9_]{3,16}$/;

const number = (v, bound) => typeof v === 'number' && Number.isFinite(v) && Math.abs(v) <= bound;

/// A drive input as the page sends it: a move vector of three numbers from -1
/// to 1 and a look of two numbers (degrees of yaw and pitch to add), or null.
function parseInput(drive) {
    if (!drive || typeof drive !== 'object' || Array.isArray(drive)) return null;
    const { move, look } = drive;
    if (!Array.isArray(move) || move.length !== 3 || !move.every((v) => number(v, 1))) return null;
    if (!Array.isArray(look) || look.length !== 2 || !look.every((v) => number(v, 360))) return null;
    return { move, look };
}

export function createCamera({ plots, world, log = () => {}, relayApi = null, ...options }) {
    const opts = { ...CAMERA_DEFAULTS, ...options };
    let mod = null; // { connection, player }
    let driver = null; // { viewer, plot, owner, until, last }
    const queue = []; // viewers waiting for a turn, with the plot they watch
    let framed = null; // the plot the automatic camera is on
    let streaming = relayApi ? false : null;

    const send = (to, message) => {
        try {
            to.send(JSON.stringify(message));
        } catch {
            // a socket on its way out
        }
    };

    const frameOf = (plot) => ({ plot: plot.index, origin: plot.origin, size: plot.design.size });

    /// The automatic camera: the plot last placed or touched, when nobody drives.
    const frame = () => {
        if (!mod || driver || !framed || !framed.design) return;
        send(mod.connection, { frame: frameOf(framed) });
    };

    const startTurn = (viewer, plot, owner) => {
        const now = Date.now();
        driver = { viewer, plot: plot.index, held: plot, owner, until: owner ? null : now + opts.turnSeconds * 1000, last: now };
        send(viewer, { drive: 'yours', seconds: owner ? null : opts.turnSeconds });
    };

    /// Ends the turn in hand, passes the camera on, or lets the automatic camera resume.
    const endTurn = (why) => {
        if (!driver) return;
        const { viewer } = driver;
        driver = null;
        send(viewer, { drive: 'over', why });
        if (mod && queue.length) {
            const next = queue.shift();
            startTurn(next.viewer, next.plot, false);
            return;
        }
        frame();
    };

    const position = (viewer) => queue.findIndex((q) => q.viewer === viewer) + 1;

    const clock = setInterval(() => {
        if (!driver) return;
        const now = Date.now();
        if (driver.until !== null && now >= driver.until) endTurn('time: the turn is over');
        else if (now - driver.last >= opts.idleSeconds * 1000) endTurn('idle: no input for a while');
        else if (driver.owner && !driver.held.viewers.has(driver.viewer)) endTurn('the plot was released');
    }, opts.tickMs);

    let poll = null;
    const askRelay = async () => {
        try {
            const reply = await fetch(`${relayApi}/v3/paths/get/camera`, { signal: AbortSignal.timeout(2000) });
            streaming = reply.ok && (await reply.json()).ready === true;
        } catch {
            streaming = false;
        }
    };
    if (relayApi) {
        askRelay();
        poll = setInterval(askRelay, opts.pollSeconds * 1000);
    }

    return {
        get connected() {
            return mod !== null;
        },

        /// Takes the mod's socket: a hello with the player's name first, which
        /// puts that player in spectator mode; then the frames and inputs flow to it.
        attach(connection) {
            let player = null;
            connection.onMessage = async (text) => {
                let message;
                try {
                    message = JSON.parse(text);
                } catch {
                    return send(connection, { error: 'the message must be JSON' });
                }
                if (player === null) {
                    if (!message || message.hello !== 'redfab-camera' || typeof message.player !== 'string' || !PLAYER_NAME.test(message.player)) {
                        send(connection, { error: 'the hello must name the player name: {"hello": "redfab-camera", "player": NAME}' });
                        return connection.close(1008, 'no hello');
                    }
                    player = message.player;
                    mod = { connection, player };
                    log(`camera: ${player} connected`);
                    if (typeof world.command === 'function') {
                        try {
                            await world.command(`gamemode spectator ${player}`);
                        } catch (err) {
                            log(`camera: gamemode spectator ${player}: ${err.message}`);
                        }
                    }
                    send(connection, { hello: 'redfab-live', frame: framed && framed.design && !driver ? frameOf(framed) : null });
                    return undefined;
                }
                return send(connection, { error: 'the camera sends nothing after its hello' });
            };
            connection.onClose = () => {
                if (mod?.connection !== connection) return;
                log(`camera: ${player} gone`);
                mod = null;
                endTurn('the camera went away');
                queue.length = 0;
            };
        },

        /// A plot was placed: the automatic camera goes there.
        placed(plot) {
            framed = plot;
            frame();
        },

        /// A lever moved on a plot: the automatic camera goes there.
        touched(plot) {
            framed = plot;
            frame();
        },

        /// A viewer's drive message. Returns the reply to send it, or null when
        /// nothing is owed (an input forwarded, a release answered by `over`).
        request(viewer, plot, drive) {
            if (!mod) return { ok: false, error: 'no camera: the camera is not connected' };
            if (drive === 'take') {
                if (driver?.viewer === viewer) return { drive: 'yours', seconds: driver.owner ? null : Math.ceil((driver.until - Date.now()) / 1000) };
                if (viewer.controls) {
                    if (driver) {
                        const bumped = driver;
                        driver = null;
                        send(bumped.viewer, { drive: 'over', why: "the plot's owner took the camera" });
                        queue.unshift({ viewer: bumped.viewer, plot: bumped.held });
                    }
                    startTurn(viewer, plot, true);
                    return null;
                }
                if (!driver) {
                    startTurn(viewer, plot, false);
                    return null;
                }
                if (!queue.some((q) => q.viewer === viewer)) queue.push({ viewer, plot });
                return { drive: 'queued', position: position(viewer) };
            }
            if (drive === 'release') {
                if (driver?.viewer === viewer) {
                    endTurn('released');
                    return null;
                }
                const at = position(viewer);
                if (at) {
                    queue.splice(at - 1, 1);
                    return { drive: 'over', why: 'left the queue' };
                }
                return { ok: false, error: 'not driving: take the camera first' };
            }
            const input = parseInput(drive);
            if (!input) return { ok: false, error: 'a drive message is "take", "release", or {move: [x, y, z] from -1 to 1, look: [yaw, pitch]}' };
            if (driver?.viewer !== viewer) return { ok: false, error: 'not driving: take the camera first' };
            driver.last = Date.now();
            send(mod.connection, { drive: { move: input.move, look: input.look, plot: driver.plot, owner: driver.owner } });
            return null;
        },

        /// A viewer's socket closed: its turn or its place in the queue goes.
        viewerLeft(viewer) {
            if (driver?.viewer === viewer) endTurn('left');
            const at = position(viewer);
            if (at) queue.splice(at - 1, 1);
        },

        status() {
            return {
                connected: mod !== null,
                player: mod?.player ?? null,
                driver: driver ? { plot: driver.plot, owner: driver.owner } : null,
                queue: queue.length,
                plot: framed?.index ?? null,
                streaming,
            };
        },

        close() {
            clearInterval(clock);
            if (poll) clearInterval(poll);
            if (mod) mod.connection.close(1001, 'closing');
        },
    };
}
