// The Fabric server with the observer mod, launched and prepared the way the
// harness does it (harness/server.py, harness/session.py), and the world the
// backend talks to: placing structure files, setting probe blocks, clearing
// plots, keeping chunks loaded, reading the tick time, and the mod's stream.

import { spawn } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, openSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { mkdir, writeFile } from 'node:fs/promises';
import { createServer as createNetServer, createConnection } from 'node:net';
import { randomBytes } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Rcon } from './rcon.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const GAMERULES = {
    random_tick_speed: '0',
    advance_time: 'false',
    advance_weather: 'false',
    spawn_mobs: 'false',
    spawn_monsters: 'false',
    spawn_phantoms: 'false',
    spawn_patrols: 'false',
    spawn_wandering_traders: 'false',
    spawn_wardens: 'false',
    mob_griefing: 'false',
    block_drops: 'false',
    entity_drops: 'false',
    log_admin_commands: 'false',
    fire_spread_radius_around_player: '0',
};
const FILL_LIMIT = 32_768; // blocks per fill command, the game's default limit

export function readVersions() {
    const pins = {};
    for (const line of readFileSync(join(root, 'versions.env'), 'utf8').split('\n')) {
        const m = line.match(/^\s*([A-Z_0-9]+)=(.*)$/);
        if (m) pins[m[1]] = m[2].trim();
    }
    return pins;
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

/// Connects to the mod's stream and hands every message to `onMessage`; reconnects if it drops.
function observe(port, onMessage, log) {
    let socket = null;
    let stopped = false;
    const open = async () => {
        for (let attempt = 0; !stopped; attempt++) {
            try {
                await new Promise((ok, bad) => {
                    socket = new WebSocket(`ws://127.0.0.1:${port}/`);
                    socket.onopen = ok;
                    socket.onerror = () => bad(new Error('refused'));
                    socket.onmessage = (e) => onMessage(JSON.parse(e.data));
                    socket.onclose = () => {
                        if (!stopped) {
                            log('the observer stream closed; reconnecting');
                            setTimeout(open, 500);
                        }
                    };
                });
                return;
            } catch {
                if (attempt > 60) throw new Error(`no observer stream on port ${port}`);
                await sleep(500);
            }
        }
    };
    return { open, close: () => { stopped = true; socket?.close(); } };
}

export async function launchMinecraft({ runDir, log = () => {}, timeout = 240_000 }) {
    const pins = readVersions();
    const java = join(root, '.tools/jdk/Contents/Home/bin/java');
    const jar = join(root, '.tools/mc', `server-${pins.MC_VERSION}.jar`);
    const fabric = join(root, '.tools/fabric');
    const mod = join(root, 'build/mod/redfab-observer.jar');
    for (const [what, path] of [['the JDK', java], ['the server jar', jar], ['Fabric', join(fabric, 'fabric-server-launch.jar')], ['the observer mod', mod]]) {
        if (!existsSync(path)) throw new Error(`${what} is missing at ${path}; run tools/setup.sh and tools/build_mod.sh`);
    }
    const [gamePort, rconPort, observerPort] = [await freePort(), await freePort(), await freePort()];
    const password = randomBytes(16).toString('hex');

    rmSync(runDir, { recursive: true, force: true });
    mkdirSync(runDir, { recursive: true });
    // The project owner accepted the Minecraft EULA for local testing on 2026-10-01.
    writeFileSync(join(runDir, 'eula.txt'), 'eula=true\n');
    const preset = JSON.stringify({ layers: [], biome: 'minecraft:the_void' }).replaceAll(':', '\\:');
    const properties = {
        'server-ip': '127.0.0.1',
        'server-port': String(gamePort),
        'enable-rcon': 'true',
        'rcon.port': String(rconPort),
        'rcon.password': password,
        'broadcast-rcon-to-ops': 'false',
        'level-type': 'minecraft\\:flat',
        'generator-settings': preset,
        'generate-structures': 'false',
        'pause-when-empty-seconds': '-1',
        'spawn-protection': '0',
        'view-distance': '4',
        'simulation-distance': '4',
        'max-tick-time': '-1',
        'difficulty': 'peaceful',
        'online-mode': 'false',
        'motd': 'redfab live playground',
    };
    writeFileSync(join(runDir, 'server.properties'), Object.entries(properties).map(([k, v]) => `${k}=${v}\n`).join(''));
    for (const item of ['libraries', '.fabric', 'versions']) cpSync(join(fabric, item), join(runDir, item), { recursive: true, verbatimSymlinks: true });
    cpSync(join(fabric, 'fabric-server-launch.jar'), join(runDir, 'fabric-server-launch.jar'));
    writeFileSync(join(runDir, 'fabric-server-launcher.properties'), `serverJar=${jar}\n`);
    mkdirSync(join(runDir, 'mods'));
    cpSync(mod, join(runDir, 'mods/redfab-observer.jar'));

    const logFile = openSync(join(runDir, 'server.log'), 'w');
    const proc = spawn(java, [
        '-Xms512M', '-Xmx2G',
        `-DbundlerRepoDir=${join(root, '.tools/mc/bundle')}`,
        `-Dredfab.run=${runDir}`,
        `-Dredfab.observer.port=${observerPort}`,
        '-jar', join(runDir, 'fabric-server-launch.jar'), '--nogui',
    ], { cwd: runDir, stdio: ['ignore', logFile, logFile], detached: true });
    const onExit = () => {
        if (proc.exitCode === null) proc.kill('SIGKILL');
    };
    process.on('exit', onExit);
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
    const rcon = await Rcon.connect('127.0.0.1', rconPort, password);
    log(`server up in ${((Date.now() - started) / 1000).toFixed(1)} s`);

    // The stream.
    const listeners = [];
    let tick = null;
    const stream = observe(observerPort, (message) => {
        if (message.hello !== undefined) {
            tick = message.tick;
            return;
        }
        tick = message.tick;
        for (const cb of listeners) cb(message);
    }, log);
    await stream.open();

    // The world's rules: nothing moves but redstone, and commands may change big boxes.
    for (const [rule, value] of Object.entries(GAMERULES)) {
        const reply = await rcon.command(`gamerule ${rule} ${value}`);
        if (!reply.includes('is now set to')) throw new Error(`gamerule ${rule}: ${reply}`);
    }
    const limitReply = await rcon.command('gamerule max_block_modifications 1000000');
    const bigFills = limitReply.includes('is now set to');
    if (!bigFills) log(`fills stay under ${FILL_LIMIT} blocks (${limitReply.trim()})`);
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
        command: (text) => rcon.command(text),
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
        async setblock([x, y, z], state) {
            const reply = await rcon.command(`setblock ${x} ${y} ${z} ${state}`);
            if (!reply.startsWith('Changed the block') && !reply.startsWith('Could not set the block')) throw new Error(`setblock: ${reply}`);
        },
        async clear([[x0, y0, z0], [x1, y1, z1]]) {
            const layer = (x1 - x0 + 1) * (y1 - y0 + 1);
            const rows = bigFills ? z1 - z0 + 1 : Math.max(1, Math.floor(FILL_LIMIT / layer));
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
    };

    return {
        world,
        runDir,
        source: { kind: 'live', mc_version: pins.MC_VERSION, jar_sha1: pins.MC_SERVER_SHA1, redstone_experiments: false },
        async close() {
            stream.close();
            try {
                await rcon.command('stop');
            } catch {
                // the connection drops as the server stops
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
