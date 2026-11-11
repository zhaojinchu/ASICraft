// The Fabric server with the observer mod, launched and prepared the way the
// harness does it (harness/server.py, harness/session.py), and the world the
// backend talks to: placing structure files, setting probe blocks, clearing
// plots, keeping chunks loaded, reading the tick time, and the mod's stream.
//
// Two RCON connections: levers, placements and clears share one; the clock
// edges of every plot go over the other, so a backlog of lever commands never
// delays an edge. Both re-dial when they drop. The stream reconnects on its
// own with a backoff, and says so, because what the server did meanwhile was
// missed. `health()` reports all of that for /api/status.

import { spawn } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, openSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { mkdir, rm, writeFile } from 'node:fs/promises';
import { createServer as createNetServer, createConnection } from 'node:net';
import { createHash, randomBytes } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Rcon } from './rcon.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

export const EULA_URL = 'https://aka.ms/MinecraftEULA';
// Written by `tools/setup.sh --accept-eula`; the harness looks for the same file.
export const EULA_FILE = join(root, '.tools/eula-accepted');

// The server configuration shared with the harness: gamerules, server.properties
// defaults and JVM arguments. The backend's own differences go on top, below.
export const PROFILE_FILE = join(root, 'harness/data/server_profile.json');
export const PROFILE = JSON.parse(readFileSync(PROFILE_FILE, 'utf8'));
export const GAMERULES = PROFILE.gamerules;
export const MOTD = 'redfab live playground';
// The live world's flat layers, as server.properties writes them (colons escaped): air up to
// y = 59, white concrete at y = 60, the void's biome. LIVE_FLOOR_Y is that floor's height.
export const LIVE_FLOOR_Y = 60;
// The camera glides between plots 512 blocks apart faster than the game lets a player move
// (about 10 blocks a tick), and the server refused every such move ("moved too quickly"),
// keeping the player where it was and the chunks it sends there, so the camera could show
// no plot but the first, and each refusal snapped the picture back for a frame. The only
// player is the camera, through the tunnel and the whitelist, so the check is off here.
export const LIVE_GAMERULES = { player_movement_check: 'false' };
export const LIVE_GROUND = '{"layers"\\:[{"block"\\:"minecraft\\:air","height"\\:124},{"block"\\:"minecraft\\:white_concrete","height"\\:1}],"biome"\\:"minecraft\\:the_void"}';
const FILL_LIMIT = 32_768; // blocks per fill command, the game's default limit

/// What the backend writes to server.properties: the profile's defaults, this
/// run's ports and password, its own motd, online-mode off (the game port stays
/// on loopback, and the camera's tunnel is the one way in), and every player a
/// spectator. The camera is the only player; the server puts it in spectator mode
/// as it joins, which a command cannot, since the camera's mod says hello from the
/// title screen, before its player is in the world. The harness never has players.
/// The camera sees 16 chunks (256 blocks) where the profile's 4 cut every design
/// short of its far edge; the flat world makes the extra chunks cheap, and the
/// simulation distance stays the profile's, since the plots are forceloaded.
/// Under the plots, which are placed at y = 64, is a floor of white concrete at
/// y = 60 (124 layers of air, then the floor), plain for the camera and four
/// blocks clear of every circuit, which a solid block beside dust would change.
export function serverProperties({ gamePort, rconPort, password }) {
    return {
        ...PROFILE.properties,
        'server-port': String(gamePort),
        'rcon.port': String(rconPort),
        'rcon.password': password,
        'online-mode': 'false',
        'gamemode': 'spectator',
        'force-gamemode': 'true',
        'view-distance': '16',
        'generator-settings': LIVE_GROUND,
        'motd': MOTD,
    };
}

/// The UUID an offline-mode server gives a player: version 3, from the MD5 of
/// "OfflinePlayer:" and the name. `whitelist add NAME` asks Mojang for the
/// account's own UUID instead, which an offline server never sees, so the
/// player it lists is turned away.
export function offlineUuid(name) {
    const bytes = createHash('md5').update(`OfflinePlayer:${name}`, 'utf8').digest();
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    const hex = bytes.toString('hex');
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/// Puts `name` on the run's whitelist by its offline UUID, keeping the entries
/// already there; the server reads it again on `whitelist reload`.
export function writeWhitelist(runDir, name) {
    const file = join(runDir, 'whitelist.json');
    const entries = existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : [];
    const uuid = offlineUuid(name);
    if (!entries.some((e) => e.uuid === uuid)) entries.push({ uuid, name });
    writeFileSync(file, `${JSON.stringify(entries, null, 2)}\n`);
    return entries;
}

/// The server's command line after the Java binary: the profile's JVM arguments, then this run's markers.
export function javaArgs({ runDir, observerPort }) {
    return [
        ...PROFILE.jvm_args,
        `-DbundlerRepoDir=${join(root, '.tools/mc/bundle')}`,
        `-Dredfab.run=${runDir}`,
        `-Dredfab.observer.port=${observerPort}`,
        '-jar', join(runDir, 'fabric-server-launch.jar'), '--nogui',
    ];
}

/// Kills the server a backend that was killed left behind in `runDir`, by its
/// pid file, before the directory is pulled from under it. Returns the pid
/// killed, or null when there was nothing to kill.
export function killLeftover(runDir, log = () => {}) {
    const pidFile = join(runDir, 'server.pid');
    if (!existsSync(pidFile)) return null;
    const pid = Number(readFileSync(pidFile, 'utf8').trim());
    if (!Number.isInteger(pid) || pid <= 1) return null;
    try {
        process.kill(pid, 'SIGKILL');
    } catch {
        return null; // already gone
    }
    log(`killed the server left behind in ${runDir} (pid ${pid})`);
    return pid;
}

export function readVersions() {
    const pins = {};
    for (const line of readFileSync(join(root, 'versions.env'), 'utf8').split('\n')) {
        const m = line.match(/^\s*([A-Z_0-9]+)=(.*)$/);
        if (m) pins[m[1]] = m[2].trim();
    }
    return pins;
}

/// The Java binary: `.tools/java`, which tools/setup.sh links to the JDK's
/// java wherever the tarball put it, or the macOS JDK layout from before the link.
export function javaPath() {
    const linked = join(root, '.tools/java');
    return existsSync(linked) ? linked : join(root, '.tools/jdk/Contents/Home/bin/java');
}

/// Throws unless whoever runs this machine has accepted the server's EULA.
export function requireEula(file = EULA_FILE) {
    if (existsSync(file)) return;
    throw new Error(`the Minecraft server's EULA (${EULA_URL}) has not been accepted on this machine; read it, then run tools/setup.sh --accept-eula, which records the acceptance in ${file}`);
}

function freePort() {
    return new Promise((ok) => {
        const probe = createNetServer();
        probe.listen(0, '127.0.0.1', () => {
            const { port } = probe.address();
            probe.close(() => ok(port));
        });
    });
}

function portOpen(port) {
    return new Promise((ok) => {
        const socket = createConnection({ host: '127.0.0.1', port });
        socket.setTimeout(500);
        socket.once('connect', () => {
            socket.destroy();
            ok(true);
        });
        socket.once('error', () => ok(false));
        socket.once('timeout', () => {
            socket.destroy();
            ok(false);
        });
    });
}

const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms));

/// Connects to the mod's stream and hands every message to `onMessage`. One
/// reconnect loop at a time: a close only starts it when none is running, and
/// a refused attempt waits longer each time, up to `maxDelay`. `onReconnect`
/// runs after every reconnection but the first connection.
export function observe(port, onMessage, { log = () => {}, onReconnect = () => {}, maxDelay = 5000, openTimeout = 60_000 } = {}) {
    let socket = null;
    let stopped = false;
    let loop = null;
    let connections = 0;
    let lastMessageAt = 0;
    let attempts = 0;

    const attempt = () => new Promise((ok, bad) => {
        const ws = new WebSocket(`ws://127.0.0.1:${port}/`);
        const refused = () => bad(new Error('refused'));
        ws.onerror = () => {};
        ws.onclose = refused;
        ws.onmessage = (e) => {
            lastMessageAt = Date.now();
            onMessage(JSON.parse(e.data));
        };
        ws.onopen = () => {
            if (stopped) {
                ws.close();
                refused();
                return;
            }
            socket = ws;
            ws.onclose = () => {
                if (socket === ws) socket = null;
                if (stopped) return;
                log('the observer stream closed; reconnecting');
                reconnect();
            };
            ok();
        };
    });

    /// Tries until connected or `deadline`; concurrent callers share the one loop.
    function connect(deadline) {
        if (loop) return loop;
        loop = (async () => {
            let delay = 250;
            for (;;) {
                if (stopped) throw new Error('stopped');
                attempts += 1;
                try {
                    await attempt();
                    connections += 1;
                    return;
                } catch {
                    if (deadline !== null && Date.now() > deadline) throw new Error(`no observer stream on port ${port}`);
                    await sleep(delay);
                    delay = Math.min(delay * 2, maxDelay);
                }
            }
        })().finally(() => {
            loop = null;
        });
        return loop;
    }

    function reconnect() {
        connect(null).then(() => {
            log(`the observer stream is back (connection ${connections})`);
            onReconnect();
        }, (err) => log(`the observer stream: ${err.message}`));
    }

    return {
        open: () => connect(Date.now() + openTimeout),
        close() {
            stopped = true;
            socket?.close();
        },
        send(text) {
            if (!socket) throw new Error('the observer stream is down');
            socket.send(text);
        },
        get up() {
            return socket !== null;
        },
        get age() {
            return lastMessageAt ? Date.now() - lastMessageAt : null;
        },
        get reconnects() {
            return Math.max(0, connections - 1);
        },
        get attempts() {
            return attempts;
        },
    };
}

/// `gamePort` fixes the game's port (on loopback, as always) so that the camera's
/// tunnel can name it; null takes a free one at every start.
export async function launchMinecraft({ runDir, log = () => {}, timeout = 240_000, gamePort: fixedGamePort = null }) {
    const pins = readVersions();
    requireEula();
    const java = javaPath();
    const jar = join(root, '.tools/mc', `server-${pins.MC_VERSION}.jar`);
    const fabric = join(root, '.tools/fabric');
    const mod = join(root, 'build/mod/redfab-observer.jar');
    for (const [what, path] of [['the JDK', java], ['the server jar', jar], ['Fabric', join(fabric, 'fabric-server-launch.jar')], ['the observer mod', mod]]) {
        if (!existsSync(path)) throw new Error(`${what} is missing at ${path}; run tools/setup.sh and tools/build_mod.sh`);
    }
    const [gamePort, rconPort, observerPort] = [fixedGamePort ?? await freePort(), await freePort(), await freePort()];
    const password = randomBytes(16).toString('hex');

    // A server left behind by a backend that was killed would have the run directory pulled from under it.
    killLeftover(runDir, log);
    rmSync(runDir, { recursive: true, force: true });
    mkdirSync(runDir, { recursive: true });
    // Accepted by whoever ran `tools/setup.sh --accept-eula` (see requireEula).
    writeFileSync(join(runDir, 'eula.txt'), 'eula=true\n');
    const properties = serverProperties({ gamePort, rconPort, password });
    writeFileSync(join(runDir, 'server.properties'), Object.entries(properties).map(([k, v]) => `${k}=${v}\n`).join(''));
    for (const item of ['libraries', '.fabric', 'versions']) cpSync(join(fabric, item), join(runDir, item), { recursive: true, verbatimSymlinks: true });
    cpSync(join(fabric, 'fabric-server-launch.jar'), join(runDir, 'fabric-server-launch.jar'));
    writeFileSync(join(runDir, 'fabric-server-launcher.properties'), `serverJar=${jar}\n`);
    mkdirSync(join(runDir, 'mods'));
    cpSync(mod, join(runDir, 'mods/redfab-observer.jar'));

    const logFile = openSync(join(runDir, 'server.log'), 'w');
    const proc = spawn(java, javaArgs({ runDir, observerPort }), { cwd: runDir, stdio: ['ignore', logFile, logFile], detached: true });
    writeFileSync(join(runDir, 'server.pid'), `${proc.pid}\n`);
    const onExit = () => {
        if (proc.exitCode === null) proc.kill('SIGKILL');
    };
    process.on('exit', onExit);
    // Whoever runs the backend hears when the game dies, expectedly or not.
    let exited = null;
    let stopping = false;
    const exitListeners = [];
    proc.on('exit', (code, signal) => {
        exited = { code, signal, expected: stopping };
        if (!stopping) log(`Minecraft exited unexpectedly (${signal || code}); see ${join(runDir, 'server.log')}`);
        for (const cb of exitListeners) cb(exited);
    });
    const started = Date.now();
    log(`starting Minecraft ${pins.MC_VERSION} with the observer mod in ${runDir}`);
    while (!(await portOpen(rconPort))) {
        if (proc.exitCode !== null) throw new Error(`the server exited during startup; see ${join(runDir, 'server.log')}`);
        if (Date.now() - started > timeout) {
            proc.kill('SIGKILL');
            throw new Error(`the server did not open RCON within ${timeout} ms; see ${join(runDir, 'server.log')}`);
        }
        await sleep(250);
    }
    // One RCON connection: the server answers concurrent connections through
    // one shared buffer, so a second one gets the first one's replies (see
    // rcon.mjs). The clock lane jumps the queue instead of having its own socket.
    const rcon = await Rcon.connect('127.0.0.1', rconPort, password, { log });
    log(`server up in ${((Date.now() - started) / 1000).toFixed(1)} s`);

    // The stream.
    const listeners = [];
    const reconnectListeners = [];
    let tick = null;
    let forgotten = 0;
    const stream = observe(observerPort, (message) => {
        if (message.hello !== undefined) {
            tick = message.tick;
            return;
        }
        if (message.forgot !== undefined) {
            forgotten += 1;
            return;
        }
        tick = message.tick;
        for (const cb of listeners) cb(message);
    }, { log, onReconnect: () => reconnectListeners.forEach((cb) => cb()) });
    await stream.open();

    // The world's rules: nothing moves but redstone, and commands may change big boxes.
    for (const [rule, value] of Object.entries(GAMERULES)) {
        const reply = await rcon.command(`gamerule ${rule} ${value}`);
        if (!reply.includes('is now set to')) throw new Error(`gamerule ${rule}: ${reply}`);
    }
    // Time stands still (advance_time is off), but a new world stands still at sunrise: noon,
    // for the camera's even light. The harness never looks, so this is the backend's alone.
    const noon = await rcon.command('time set noon');
    // 26.3 answers "Set minecraft:overworld to time marker minecraft:noon"; anything else is logged.
    if (!/set the time|time marker minecraft:noon/i.test(noon)) log(`time set noon: ${noon.trim()}`);
    // The backend's own rules (LIVE_GAMERULES), which the harness has no players to need.
    for (const [rule, value] of Object.entries(LIVE_GAMERULES)) {
        const reply = await rcon.command(`gamerule ${rule} ${value}`);
        if (!reply.includes('is now set to')) log(`gamerule ${rule}: ${reply.trim()}`);
    }
    // Plot clears are big fills; the harness never fills past the game's default, so this is the backend's alone.
    const limitReply = await rcon.command(`gamerule max_block_modifications ${PROFILE.max_block_modifications}`);
    const fillLimit = limitReply.includes('is now set to') ? PROFILE.max_block_modifications : FILL_LIMIT;
    if (fillLimit === FILL_LIMIT) log(`fills stay under ${FILL_LIMIT} blocks (${limitReply.trim()})`);
    const query = await rcon.command('tick query');
    if (!/running normally/i.test(query)) log(`tick query: ${query.trim()}`);

    const structures = join(runDir, 'world/generated/redfab/structure');
    await mkdir(structures, { recursive: true });

    const world = {
        get tick() {
            return tick;
        },
        onBatch(cb) {
            listeners.push(cb);
        },
        onStreamReconnect(cb) {
            reconnectListeners.push(cb);
        },
        command: (text) => rcon.command(text),
        /// Lets the camera's player join: its offline UUID on the whitelist, read again.
        async allow(name) {
            writeWhitelist(runDir, name);
            const reply = await rcon.command('whitelist reload');
            if (!/reloaded/i.test(reply)) throw new Error(`whitelist reload: ${reply.trim()}`);
        },
        async forceload([[x0, , z0], [x1, , z1]]) {
            const chunksX = Math.floor(x1 / 16) - Math.floor(x0 / 16) + 1;
            const rowsPerStrip = Math.max(1, Math.floor(256 / chunksX));
            for (let z = z0; z <= z1;) {
                const zEnd = Math.min(z1, (Math.floor(z / 16) + rowsPerStrip) * 16 - 1);
                const reply = await rcon.command(`forceload add ${x0} ${z} ${x1} ${zEnd}`);
                if (!reply.includes('force loaded') && !reply.startsWith('No chunks were marked')) throw new Error(`forceload: ${reply}`);
                z = zEnd + 1;
            }
        },
        async place({ name, structure, origin }) {
            await writeFile(join(structures, `${name}.nbt`), structure);
            const [x, y, z] = origin;
            const reply = await rcon.command(`place template redfab:${name} ${x} ${y} ${z} none none 1.0 0 strict`);
            if (!reply.startsWith('Loaded template')) throw new Error(`place template: ${reply}`);
        },
        /// Drops a placed template from the game's cache (the mod does it) and from disk.
        async forget(name) {
            await rm(join(structures, `${name}.nbt`), { force: true });
            stream.send(JSON.stringify({ forget: `redfab:${name}` }));
        },
        get forgotten() {
            return forgotten;
        },
        async setblock([x, y, z], state, { lane = 'levers' } = {}) {
            const reply = await rcon.command(`setblock ${x} ${y} ${z} ${state}`, { urgent: lane === 'clock' });
            if (!reply.startsWith('Changed the block') && !reply.startsWith('Could not set the block')) throw new Error(`setblock: ${reply}`);
        },
        async clear([[x0, y0, z0], [x1, y1, z1]]) {
            // In strips of rows that stay under the fill limit: a full 512 by 512 plot is more than one fill even with the raised limit.
            const layer = (x1 - x0 + 1) * (y1 - y0 + 1);
            const rows = Math.max(1, Math.floor(fillLimit / layer));
            for (let z = z0; z <= z1; z += rows) {
                const zEnd = Math.min(z1, z + rows - 1);
                const reply = await rcon.command(`fill ${x0} ${y0} ${z} ${x1} ${y1} ${zEnd} minecraft:air`);
                if (!/block/i.test(reply)) throw new Error(`fill: ${reply}`);
            }
        },
        async tickTime() {
            const reply = await rcon.command('tick query');
            const avg = reply.match(/Average time per tick:\s*([\d.]+)\s*ms/i);
            const p95 = reply.match(/P95:\s*([\d.]+)\s*ms/i);
            if (!avg) return { avg_ms: null, p95_ms: null, raw: reply.trim() };
            return { avg_ms: Number(avg[1]), p95_ms: p95 ? Number(p95[1]) : null };
        },
        /// Whether the game, its RCON and the stream are there right now.
        health() {
            return {
                running: proc.exitCode === null && exited === null,
                rcon_up: rcon.up,
                rcon_reconnects: Math.max(0, rcon.reconnects),
                stream_up: stream.up,
                stream_age_ms: stream.age,
                stream_reconnects: stream.reconnects,
                forgotten,
            };
        },
    };

    return {
        world,
        runDir,
        proc,
        source: { kind: 'live', mc_version: pins.MC_VERSION, jar_sha1: pins.MC_SERVER_SHA1, redstone_experiments: false },
        /// `cb` runs when the Java process exits, with {code, signal, expected}.
        onExit(cb) {
            if (exited) cb(exited);
            else exitListeners.push(cb);
        },
        get exited() {
            return exited;
        },
        async close() {
            stopping = true;
            stream.close();
            try {
                await rcon.command('stop');
            } catch {
                // the connection drops as the server stops, or the server is already gone
            }
            rcon.close();
            const deadline = Date.now() + 60_000;
            while (proc.exitCode === null && Date.now() < deadline) await sleep(200);
            if (proc.exitCode === null) proc.kill('SIGKILL');
            process.off('exit', onExit);
            log('server stopped');
        },
    };
}
