// The live playground's backend under Node, without Minecraft. See `check`.
//
//   node --test tools/live_test.mjs
//
// The backend is given a fake world that records what it is asked to do and
// plays the part of the game's stream. For the tick-for-tick check the fake
// world is backed by the native simulator, so what it streams is what the game
// would say, and the trace the backend records must equal the simulator's own.
// The parts that talk to the game itself (the stream client, the RCON client)
// are tested against fake servers that drop, stall and refuse.

import { mock, test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { createServer as createHttpServer } from 'node:http';
import { connect as netConnect, createServer as createNetServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { loadCore } from '../playground/core.js';
import { LIMITS } from '../playground/compile.js';
import { DEFAULTS, createLive } from '../server/live.mjs';
import { GAMERULES, MOTD, PROFILE_FILE, javaArgs, javaPath, killLeftover, observe, requireEula, serverProperties } from '../server/minecraft.mjs';
import { Rcon } from '../server/rcon.mjs';
import { acceptWebSocket } from '../server/ws.mjs';
import { buildWasm } from './common.mjs';
import { api, changesAt, connect, designSource, firstTraceDifference, nativeDesign, root, sleep, waitFor, wasmPath } from './live_common.mjs';

buildWasm();
const core = await loadCore(readFileSync(wasmPath));

const pos = (text) => text.split(',').map(Number);
const key = (p) => p.join(',');
const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];

/// `name[a=1,b=2]` with its properties in the other order: what a game that
/// serialised them differently would send.
const scrambled = (state) => {
    const open = state.indexOf('[');
    if (open < 0) return state;
    return `${state.slice(0, open)}[${state.slice(open + 1, -1).split(',').reverse().join(',')}]`;
};

/// A world that records what the backend asks of it. `emit` plays the game's
/// stream. With `core`, every placed design is also simulated natively, so the
/// stream carries what the simulator says the blocks did.
class FakeWorld {
    constructor({ simulate = false } = {}) {
        this.simulate = simulate;
        this.tick = 100; // the game has been running for a while
        this.placed = [];
        this.setblocks = [];
        this.cleared = [];
        this.forgotten = [];
        this.forceloaded = [];
        this.listeners = [];
        this.reconnectListeners = [];
        this.sims = [];
        this.tickTimes = { avg_ms: 1.5, p95_ms: 2.5 };
        this.failClears = 0; // how many of the next clears fail
        this.lateLanes = new Set(); // setblocks on these lanes show up one tick late
        this.failPlaces = 0; // placements the game refuses, as a count
        this.holdAll = false; // every setblock's change waits until this is cleared (a stalled backend)
        this.held = [];
        this.scramble = false; // stream states with their properties in the other order
        this.healthOverride = {};
        this.reconnects = 0;
    }
    onBatch(cb) {
        this.listeners.push(cb);
    }
    onStreamReconnect(cb) {
        this.reconnectListeners.push(cb);
    }
    /// The stream dropped and came back.
    reconnectStream() {
        this.reconnects += 1;
        for (const cb of this.reconnectListeners) cb();
    }
    health() {
        return { running: true, rcon_up: true, stream_up: true, stream_age_ms: 0, stream_reconnects: this.reconnects, ...this.healthOverride };
    }
    emit(tick, changes) {
        this.tick = Math.max(this.tick, tick);
        if (this.scramble) changes = changes.map(([p, s]) => [p, scrambled(s)]);
        for (const cb of this.listeners) cb({ tick, changes });
    }
    async forceload(box) {
        this.forceloaded.push(box);
    }
    async clear(box) {
        if (this.failClears > 0) {
            this.failClears -= 1;
            throw new Error('fill: the server did not answer');
        }
        this.cleared.push(box);
        const [[x0, , z0], [x1, , z1]] = box;
        this.sims = this.sims.filter((s) => !(s.origin[0] >= x0 && s.origin[0] <= x1 && s.origin[2] >= z0 && s.origin[2] <= z1));
    }
    async forget(name) {
        this.forgotten.push(name);
    }
    async tickTime() {
        return this.tickTimes;
    }
    /// A strict placement: the blocks appear between ticks, labelled with the last completed tick.
    async place({ name, structure, design, origin }) {
        assert.ok(structure instanceof Uint8Array && structure.length > 0, 'a structure file is placed');
        if (this.failPlaces > 0) {
            // What the game answers when the reply is not the placement's (seen live: another command's reply glued on).
            this.failPlaces -= 1;
            throw new Error('place template: Changed the block at 1, 65, 117Loaded template "redfab:plot2-4" at 1056, 64, 0');
        }
        this.placed.push({ name, origin, blocks: Object.keys(design.blocks).length, structure });
        if (this.simulate) {
            this.sims = this.sims.filter((s) => key(s.origin) !== key(origin));
            const { handle } = core.call({ op: 'sim_new', design: JSON.stringify(design) });
            this.sims.push({ handle, design, origin });
        }
        this.emit(this.tick, Object.entries(design.blocks).map(([p, s]) => [key(add(pos(p), origin)), s]));
    }
    /// A setblock between ticks. On a simulated design it is a lever: the simulator
    /// takes it and reports what changed at once (the probe, dust, lamps).
    async setblock(at, state, { lane = 'levers' } = {}) {
        this.setblocks.push([at, state, this.tick, lane]);
        const sim = this.sims.find((s) => s.design.inputs.some((pin) => key(add(pos(pin.probe), s.origin)) === key(at)));
        let changes;
        if (sim) {
            const input = sim.design.inputs.findIndex((pin) => key(add(pos(pin.probe), sim.origin)) === key(at));
            const reply = core.call({ op: 'sim_set', handle: sim.handle, input, on: state === sim.design.inputs[input].on });
            changes = reply.changes.map(([p, s]) => [key(add(pos(p), sim.origin)), s]);
        } else {
            changes = [[key(at), state]];
        }
        if (this.holdAll || this.lateLanes.has(lane)) this.held.push(...changes);
        else this.emit(this.tick, changes);
    }
    /// Runs the simulated designs: one message at the end of every tick, as the
    /// mod sends, and a pause between ticks so the backend can react to each as it
    /// would with a real server.
    async step(ticks) {
        for (let k = 0; k < ticks; k++) {
            this.tick += 1;
            const changes = this.holdAll ? [] : this.held.splice(0);
            for (const sim of this.sims) {
                const reply = core.call({ op: 'sim_step', handle: sim.handle, ticks: 1 });
                changes.push(...reply.changes.map(([p, s]) => [key(add(pos(p), sim.origin)), s]));
            }
            this.emit(this.tick, changes);
            await new Promise((ok) => setImmediate(ok));
            await new Promise((ok) => setImmediate(ok));
        }
    }
    simTrace(index = 0) {
        return core.call({ op: 'sim_trace', handle: this.sims[index].handle });
    }
}

// Every test runs from one address, so the per-address limits are raised unless a test lowers them.
async function live(options = {}, world = new FakeWorld()) {
    const backend = await createLive({ world, wasm: wasmPath, port: 0, testHooks: true, perIp: { plots: 8, compiles: 8, submissionsPerHour: 1000 }, ...options });
    return { backend, world, base: `http://127.0.0.1:${backend.port}`, wsBase: `ws://127.0.0.1:${backend.port}` };
}

const submit = (base, name, extra = {}, headers = {}) => api(base, '/api/submit', { method: 'POST', body: { source: designSource(name), top: 'top', name, ...extra }, headers });

/// Opens the plot's stream as its controller and waits for the hello.
async function control(wsBase, plot) {
    const client = connect(`${wsBase}/live?plot=${plot.plot}&token=${plot.token}`);
    await client.opened;
    const hello = await client.next((m) => m.hello !== undefined);
    return { client, hello };
}

/// A fake RCON server: answers auth and commands, stalls or drops its clients on request.
function fakeRcon(password) {
    const sockets = new Set();
    let stall = false;
    let connections = 0;
    const server = createNetServer((socket) => {
        connections += 1;
        sockets.add(socket);
        socket.on('close', () => sockets.delete(socket));
        socket.on('error', () => {});
        let buf = Buffer.alloc(0);
        const reply = (id, type, text) => {
            const t = Buffer.from(text);
            const p = Buffer.alloc(14 + t.length);
            p.writeInt32LE(10 + t.length, 0);
            p.writeInt32LE(id, 4);
            p.writeInt32LE(type, 8);
            t.copy(p, 12);
            socket.write(p);
        };
        socket.on('data', (chunk) => {
            buf = Buffer.concat([buf, chunk]);
            while (buf.length >= 4) {
                const len = buf.readInt32LE(0);
                if (buf.length < 4 + len) break;
                const id = buf.readInt32LE(4);
                const type = buf.readInt32LE(8);
                const body = buf.subarray(12, 4 + len - 2).toString();
                buf = buf.subarray(4 + len);
                if (type === 3) reply(body === password ? id : -1, 2, '');
                else if (type === 2) {
                    if (!stall) reply(id, 0, `ran ${body}`);
                } else reply(id, 0, 'Unknown request 7f');
            }
        });
    });
    return {
        listen: () => new Promise((ok) => server.listen(0, '127.0.0.1', () => ok(server.address().port))),
        dropAll: () => {
            for (const s of sockets) s.destroy();
        },
        set stall(v) {
            stall = v;
        },
        get connections() {
            return connections;
        },
        close: () => new Promise((ok) => {
            for (const s of sockets) s.destroy();
            server.close(() => ok());
        }),
    };
}

/// A fake observer mod: a WebSocket server that sends a hello and whatever it is told to.
function fakeStream() {
    const clients = new Set();
    let accepted = 0;
    const server = createHttpServer((req, res) => {
        res.writeHead(404);
        res.end();
    });
    server.on('upgrade', (req, socket, head) => {
        const connection = acceptWebSocket(req, socket, head);
        if (!connection) return;
        accepted += 1;
        clients.add(connection);
        connection.onClose = () => clients.delete(connection);
        connection.send(JSON.stringify({ hello: 'redfab-observer', tick: 1000 + accepted }));
    });
    return {
        listen: () => new Promise((ok) => server.listen(0, '127.0.0.1', () => ok(server.address().port))),
        send: (text) => {
            for (const c of clients) c.send(text);
        },
        dropAll: () => {
            for (const c of clients) c.close(1001, 'bye');
        },
        get accepted() {
            return accepted;
        },
        get clients() {
            return clients.size;
        },
        close: () => new Promise((ok) => {
            for (const c of clients) c.drop();
            server.closeAllConnections?.();
            server.close(() => ok());
        }),
    };
}

/// A WebSocket client that never reads: the upgrade by hand (with `headers` added
/// to the request), then the socket paused. Rejects with the status line of a refusal.
function rawViewer(port, path, headers = {}) {
    return new Promise((resolve, reject) => {
        const socket = netConnect({ host: '127.0.0.1', port }, () => {
            const extra = Object.entries(headers).map(([k, v]) => `${k}: ${v}\r\n`).join('');
            socket.write(`GET ${path} HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: ${Buffer.from('0123456789abcdef').toString('base64')}\r\nSec-WebSocket-Version: 13\r\n${extra}\r\n`);
        });
        socket.once('data', (chunk) => {
            if (!String(chunk).startsWith('HTTP/1.1 101')) return reject(new Error(`upgrade refused: ${String(chunk).split('\r\n')[0]}`));
            socket.pause();
            resolve(socket);
        });
        socket.on('error', reject);
    });
}

test('the backend reports itself live and placed designs are the native builds', async () => {
    const { backend, world, base } = await live({ plots: 3 });
    try {
        const status = await api(base, '/api/status');
        assert.equal(status.status, 200);
        assert.equal(status.body.live, true);
        assert.deepEqual(status.body.plots, { total: 3, free: 3, broken: 0 });
        assert.equal(status.body.tick.avg_ms, 1.5);
        assert.deepEqual(status.body.limits.plotSize, [512, 512], 'a plot is 512 by 512 (every shipped design fits; the test below holds them to it)');

        const reply = await submit(base, 'nor2');
        assert.equal(reply.status, 200, reply.body.error);
        const { plot, token, design, origin, placed_tick } = reply.body;
        assert.ok(plot !== undefined && typeof token === 'string' && token.length >= 16);
        assert.equal(JSON.stringify(design), JSON.stringify(JSON.parse(nativeDesign('nor2'))), 'the server built the same design as the native build');
        assert.equal(world.placed.length, 1);
        assert.deepEqual(world.placed[0].origin, origin);
        assert.equal(origin[0] % 16, 0, 'plots start at chunk corners');
        assert.equal(origin[2] % 16, 0);
        assert.equal(placed_tick, 100, 'the placement is labelled with the tick it landed between');
        assert.ok(world.forceloaded.some((box) => box[0][0] <= origin[0] && box[1][0] >= origin[0] + design.size[0] - 1), 'the design sits in forceloaded chunks');
        assert.deepEqual(reply.body.levers, { spacing: 8 }, 'the reply says how lever changes are paced');
        await waitFor(() => world.forgotten.length === 1);
        assert.equal(world.forgotten[0], world.placed[0].name, 'the template is forgotten once the stream shows the design placed');
        assert.deepEqual((await api(base, '/api/status')).body.plots, { total: 3, free: 2, broken: 0 });
        const info = await api(base, `/api/plots/${plot}`);
        assert.equal(info.body.placement.matched, true, 'the stream showed the design placed exactly');
        assert.equal(info.body.placement.tick, 100);
        assert.equal(info.body.stale, false);

        // The placed structure file is the owner's to download.
        const refused = await fetch(`${base}/api/plots/${plot}/structure`);
        assert.equal(refused.status, 403);
        const file = await fetch(`${base}/api/plots/${plot}/structure?token=${token}`);
        assert.equal(file.status, 200);
        assert.match(file.headers.get('content-disposition'), /nor2\.nbt/);
        assert.deepEqual(new Uint8Array(await file.arrayBuffer()), world.placed[0].structure, 'the file is the one that was placed');
    } finally {
        await backend.close();
    }
});

test('a plot settles, then its levers reach the probe blocks and nothing else does', async () => {
    const { backend, world, base, wsBase } = await live({ plots: 2 });
    try {
        const reply = await submit(base, 'nor2');
        assert.equal(reply.status, 200, reply.body.error);
        const { design, origin } = reply.body;
        const { client, hello } = await control(wsBase, reply.body);
        assert.equal(hello.state, 'settling');
        assert.deepEqual(hello.lamps, [1], 'a NOR with both inputs off lights its lamp');
        assert.deepEqual(hello.inputs, [0, 0]);
        assert.deepEqual(hello.levers, { spacing: 8 });

        // Levers are refused until the design has settled, and nothing is written.
        client.send({ set: 'a', on: true });
        let ack = await client.next((m) => m.ok !== undefined);
        assert.equal(ack.ok, false);
        assert.equal(world.setblocks.length, 0);

        world.emit(100 + design.settle_ticks + 1, []);
        const went = await client.next((m) => m.state === 'live');
        assert.ok(went.tick >= 100 + design.settle_ticks);

        client.send({ set: 'a', on: true });
        ack = await client.next((m) => m.ok !== undefined);
        assert.deepEqual([ack.ok, ack.set, ack.on, ack.queued, ack.tick], [true, 'a', true, false, world.tick]);
        const a = design.inputs.find((p) => p.name === 'a');
        assert.deepEqual(world.setblocks.map((s) => s.slice(0, 2)), [[add(pos(a.probe), origin), a.on]]);
        const update = await client.next((m) => m.changes?.length > 0);
        assert.deepEqual(update.changes, [[a.probe, a.on]], 'the change comes back in design coordinates');
        assert.deepEqual(update.inputs, [1, 0]);

        // The same lever again is a no-op, not a second write.
        client.send({ set: 'a', on: true });
        ack = await client.next((m) => m.ok !== undefined);
        assert.equal(ack.ok, true);
        assert.equal(world.setblocks.length, 1);

        for (const bad of [{ set: 'nope', on: true }, { set: 'a', on: 'yes' }, { set: 'a', on: false, pos: '1,1,1' }, { setblock: '1 1 1 minecraft:tnt' }, { set: 'y', on: true }]) {
            client.send(bad);
            ack = await client.next((m) => m.ok !== undefined);
            assert.equal(ack.ok, false, JSON.stringify(bad));
            assert.ok(/input|field|message/.test(ack.error), ack.error);
        }
        assert.equal(world.setblocks.length, 1, 'bad messages write nothing');

        // A viewer without the token can watch but not touch.
        const viewer = connect(`${wsBase}/live?plot=${reply.body.plot}`);
        await viewer.opened;
        const seen = await viewer.next((m) => m.hello !== undefined);
        assert.equal(seen.state, 'live');
        viewer.send({ set: 'b', on: true });
        ack = await viewer.next((m) => m.ok !== undefined);
        assert.equal(ack.ok, false);
        assert.equal(world.setblocks.length, 1);

        // The stream reaches viewers filtered to the plot and translated to its coordinates.
        const y = design.outputs.find((p) => p.name === 'y');
        const lampWorld = key(add(pos(y.lamp), origin));
        world.emit(world.tick + 1, [[lampWorld, 'minecraft:redstone_lamp[lit=false]'], ['5000,64,5000', 'minecraft:stone'], [key(add([0, 0, -1], origin)), 'minecraft:stone']]);
        const dark = await viewer.next((m) => m.changes?.length > 0);
        assert.deepEqual(dark.changes, [[y.lamp, 'minecraft:redstone_lamp[lit=false]']]);
        assert.deepEqual(dark.lamps, [0]);
        const again = await client.next((m) => m.changes?.length > 0);
        assert.deepEqual(again.lamps, [0]);
        const info = await api(base, `/api/plots/${reply.body.plot}`);
        assert.equal(info.status, 200);
        assert.deepEqual([info.body.state, info.body.lamps, info.body.inputs], ['live', [0], [1, 0]]);

        // A close reason is cut to what a close frame can carry, between characters.
        await backend.plots.release(backend.plots.get(reply.body.plot), `x${'\u20ac'.repeat(100)}`);
        await waitFor(() => client.closed);
        assert.ok(Buffer.byteLength(client.closed.reason) <= 120, `close reason of ${Buffer.byteLength(client.closed.reason)} bytes`);
        assert.equal(client.closed.reason, `x${'\u20ac'.repeat(39)}`, 'the cut leaves whole characters (118 bytes, not a third of a euro sign)');
        assert.equal(client.closed.code, 1000);
        viewer.close();
    } finally {
        await backend.close();
    }
});

test('the trace the backend records equals the simulator trace of the same stimuli, and replays natively', async () => {
    const world = new FakeWorld({ simulate: true });
    const { backend, base, wsBase } = await live({ plots: 2 }, world);
    try {
        for (const name of ['not1', 'shift2']) {
            const reply = await submit(base, name);
            assert.equal(reply.status, 200, reply.body.error);
            const { design, plot, token } = reply.body;
            const { client } = await control(wsBase, reply.body);
            await world.step(design.settle_ticks + 20);
            await client.next((m) => m.state === 'live');
            const seq = design.timing?.sequential;
            const data = seq ? design.inputs.find((p) => p.name !== seq.clock).name : 'a';
            // Changes come at least a lever slot apart (8 ticks, or a clock period), so none is coalesced away.
            const gaps = seq ? [seq.period + 1, seq.period + 3, seq.period, 2 * seq.period, 2 * seq.period, seq.period] : [11, 9, 8, 10, 25, 30];
            for (const [i, on] of [true, false, true, false, true, false].entries()) {
                client.send({ set: data, on });
                const ack = await client.next((m) => m.ok !== undefined);
                assert.equal(ack.ok, true, ack.error);
                if (seq) assert.equal(ack.queued, true, 'a clocked design takes the change in its input window');
                await world.step(gaps[i]);
            }
            await world.step(20);
            assert.equal((await api(base, `/api/plots/${plot}/trace`)).status, 403, 'the recording is for the owner');
            const exported = await api(base, `/api/plots/${plot}/trace?token=${token}`);
            assert.equal(exported.status, 200, exported.body.error);
            const { fixture, trace } = exported.body;
            assert.equal(fixture.format, 'redfab-fixture/1');
            assert.equal(trace.format, 'redfab-trace/1');
            assert.equal(trace.source.kind, 'live');
            assert.equal(fixture.placement, 'strict');
            assert.deepEqual(fixture.size, design.size);
            assert.ok(fixture.events.length >= 6, `${name}: the lever changes were recorded as events (${fixture.events.length})`);
            const lamp = design.outputs[0].lamp;
            assert.ok(changesAt(trace, lamp) >= 2, `${name}: the lamp changed in the recording (${changesAt(trace, lamp)} times)`);
            if (seq) {
                const info = await api(base, `/api/plots/${plot}`);
                assert.ok(info.body.clock.edges >= 6, `${name}: the clock ran (${info.body.clock.edges} edges)`);
                assert.deepEqual([info.body.clock.late, info.body.clock.max_jitter, info.body.clock.lost], [0, 0, 0], `${name}: every edge landed on schedule`);
                // Every data change landed input_change_at ticks after an active edge.
                const clockProbe = design.inputs.find((p) => p.name === seq.clock).probe;
                const dataProbe = design.inputs.find((p) => p.name === data).probe;
                const edges = fixture.events.filter((e) => e.set[clockProbe] === design.inputs.find((p) => p.name === seq.clock).on).map((e) => e.tick);
                const dataTicks = fixture.events.filter((e) => dataProbe in e.set).map((e) => e.tick);
                assert.ok(dataTicks.length >= 6);
                for (const t of dataTicks) assert.ok(edges.includes(t - seq.input_change_at), `${name}: data changed at tick ${t}, not ${seq.input_change_at} after an edge (${edges})`);
            }

            const simulated = world.simTrace(world.sims.length - 1);
            assert.equal(firstTraceDifference(trace, simulated.trace), null, `${name}: live recording vs the simulator that played the game`);
            assert.deepEqual(fixture.events, simulated.fixture.events, `${name}: the events landed on the ticks the simulator saw`);

            const dir = mkdtempSync(join(tmpdir(), 'redfab-live-'));
            writeFileSync(join(dir, `${name}.fixture.json`), JSON.stringify(fixture));
            execFileSync('cargo', ['run', '-q', '-p', 'redfab-cli', '--', 'check-fixtures', dir], { cwd: root, stdio: 'pipe' });
            const replayed = JSON.parse(readFileSync(join(dir, `${name}.sim.trace.json`), 'utf8'));
            assert.equal(firstTraceDifference(trace, replayed), null, `${name}: live recording vs the native replay of its fixture`);
            client.close();
            await api(base, `/api/plots/${plot}?token=${token}`, { method: 'DELETE' });
        }
    } finally {
        await backend.close();
    }
});

test('a design that does not fit a plot is refused before anything is placed, and the page can ask first', async () => {
    // Two chunks square: nor2 (21 by 10) fits, fa1 (59 by 30) does not.
    const { backend, world, base } = await live({ plots: 2, plotSize: [32, 32] });
    try {
        const small = await submit(base, 'nor2');
        assert.equal(small.status, 200, small.body.error);
        const big = await submit(base, 'fa1');
        assert.equal(big.status, 400);
        assert.match(big.body.error, /fit|plot/);
        assert.equal(world.placed.length, 1);
        assert.deepEqual((await api(base, '/api/status')).body.plots, { total: 2, free: 1, broken: 0 });
        const fits = await api(base, '/api/fits?x=95&z=70');
        assert.deepEqual([fits.status, fits.body.fits, fits.body.plotSize], [200, false, [32, 32]]);
        assert.equal((await api(base, '/api/fits?x=25&z=10')).body.fits, true);
        assert.equal((await api(base, '/api/fits?x=abc&z=1')).status, 400);
    } finally {
        await backend.close();
    }
});

test('the submission is source only: limits hold, unexpected fields are refused, hostile sources are stopped', async () => {
    // A normal compile peaks near 900 MB (Yosys in WebAssembly); the limit sits above that.
    const { backend, world, base } = await live({ plots: 2, compile: { seconds: 20, memoryMb: 1280, queue: 2 } });
    try {
        const big = await api(base, '/api/submit', { method: 'POST', body: { source: 'x'.repeat(LIMITS.sourceBytes + 1), top: 'top' } });
        assert.ok([400, 413].includes(big.status));
        assert.match(big.body.error, /limit/);

        const name = await submit(base, 'not1', { top: 'top; rm -rf /' });
        assert.equal(name.status, 400);
        assert.match(name.body.error, /module name/);

        for (const extra of [{ blocks: { '0,0,0': 'minecraft:tnt' } }, { design: '{}' }, { structure: 'ff' }, { origin: [0, 64, 0] }]) {
            const reply = await submit(base, 'not1', extra);
            assert.equal(reply.status, 400, JSON.stringify(extra));
            assert.match(reply.body.error, /unexpected/);
        }
        const notJson = await api(base, '/api/submit', { method: 'POST', body: 'module top; endmodule' });
        assert.equal(notJson.status, 400);
        const notText = await api(base, '/api/submit', { method: 'POST', body: { source: 42 } });
        assert.equal(notText.status, 400);
        const notJsonType = await api(base, '/api/submit', { method: 'POST', body: { source: designSource('not1') }, headers: { 'content-type': 'text/plain' } });
        assert.equal(notJsonType.status, 415, 'a submission must say it is JSON');
        const elsewhere = await submit(base, 'not1', {}, { origin: 'http://evil.example' });
        assert.equal(elsewhere.status, 403, 'another site cannot submit on a visitor\'s behalf');
        const here = await submit(base, 'not1', {}, { origin: `http://127.0.0.1:${backend.port}` });
        assert.equal(here.status, 200, here.body.error);
        await api(base, `/api/plots/${here.body.plot}?token=${here.body.token}`, { method: 'DELETE' });
        assert.equal(world.placed.length, 1);

        const broken = await api(base, '/api/submit', { method: 'POST', body: { source: 'module top(input a, output y); assign y = ; endmodule', top: 'top' } });
        assert.equal(broken.status, 400);
        assert.ok(broken.body.error.length > 0, 'a compile error carries a message');

        // A hostile source: a generate loop of four million gates, stopped within the limit.
        const hostile = `module top(input logic a, output logic y);
  logic [3999999:0] chain;
  assign chain[0] = a;
  genvar i;
  generate for (i = 1; i < 4000000; i++) begin : g
    assign chain[i] = ~chain[i-1];
  end endgenerate
  assign y = chain[3999999];
endmodule
`;
        let started = Date.now();
        const stopped = await api(base, '/api/submit', { method: 'POST', body: { source: hostile, top: 'top' } });
        assert.equal(stopped.status, 400);
        assert.ok(Date.now() - started <= 25_000, `refused in ${Date.now() - started} ms`);
        // The same hostile source again costs nothing: the refusal is cached.
        started = Date.now();
        const again = await api(base, '/api/submit', { method: 'POST', body: { source: hostile, top: 'top' } });
        assert.equal(again.status, 400);
        assert.ok(Date.now() - started <= 2_000, `refused again in ${Date.now() - started} ms`);

        // The sandbox: a compile that hangs is killed at the time limit ...
        started = Date.now();
        const hung = await api(base, '/api/submit', { method: 'POST', body: { source: `// redfab-test: hang\n${designSource('not1')}`, top: 'top' } });
        assert.equal(hung.status, 400);
        assert.match(hung.body.error, /stopped|time/);
        assert.ok(Date.now() - started <= 23_000);
        // ... one that eats memory is killed at the memory limit, well before the time limit ...
        started = Date.now();
        const fat = await api(base, '/api/submit', { method: 'POST', body: { source: `// redfab-test: allocate 2400\n${designSource('not1')}`, top: 'top' } });
        assert.equal(fat.status, 400);
        assert.match(fat.body.error, /memory/);
        assert.ok(Date.now() - started <= 15_000, `killed in ${Date.now() - started} ms`);
        // ... and the backend is fine afterwards.
        const fine = await submit(base, 'not1');
        assert.equal(fine.status, 200, fine.body.error);
        assert.equal(world.placed.length, 2);
    } finally {
        await backend.close();
    }
});

test('plots are limited, released when their controller leaves, and cleared for the next design', async () => {
    const { backend, world, base, wsBase } = await live({ plots: 2, idleSeconds: 1 });
    try {
        const first = await submit(base, 'not1');
        const second = await submit(base, 'nor2');
        assert.equal(first.status, 200, first.body.error);
        assert.equal(second.status, 200, second.body.error);
        assert.notDeepEqual(first.body.origin, second.body.origin, 'two plots, two places');
        const third = await submit(base, 'nor3');
        assert.equal(third.status, 503);
        assert.match(third.body.error, /plot|busy|free/);

        const { client } = await control(wsBase, first.body);
        client.close();
        await waitFor(() => world.cleared.length >= 1, 5000);
        const [[lo, hi]] = world.cleared;
        const o = first.body.origin;
        const s = first.body.design.size;
        assert.ok(lo[0] <= o[0] && lo[2] <= o[2] && hi[0] >= o[0] + s[0] - 1 && hi[2] >= o[2] + s[2] - 1, 'the cleared box covers the design');
        await waitFor(async () => (await api(base, '/api/status')).body.plots.free === 1, 5000);
        const fourth = await submit(base, 'nor3');
        assert.equal(fourth.status, 200, fourth.body.error);
        assert.deepEqual(fourth.body.origin, first.body.origin, 'the freed plot is reused');

        // A plot can be released on purpose, with its token only.
        const refused = await api(base, `/api/plots/${second.body.plot}?token=wrong`, { method: 'DELETE' });
        assert.equal(refused.status, 403);
        const released = await api(base, `/api/plots/${second.body.plot}?token=${second.body.token}`, { method: 'DELETE' });
        assert.equal(released.status, 200);
        await waitFor(async () => (await api(base, '/api/status')).body.plots.free === 1, 5000);
        const gone = await api(base, `/api/plots/${second.body.plot}`);
        assert.equal(gone.status, 404);
    } finally {
        await backend.close();
    }
});

test('a plot goes when its time is up, and a recording past the cap is kept no further', async () => {
    const { backend, world, base, wsBase } = await live({ plots: 1, maxSeconds: 1, maxRecordedChanges: 50 });
    try {
        const reply = await submit(base, 'nor2');
        assert.equal(reply.status, 200, reply.body.error);
        const { design, origin } = reply.body;
        const { client } = await control(wsBase, reply.body);
        // Sixty changes of real blocks after placement: the recording stops at the cap.
        for (let t = 1; t <= 60; t++) world.emit(world.tick + 1, [[key(add([t % 25, 2, Math.floor(t / 25)], origin)), 'minecraft:gold_block']]);
        await sleep(20);
        const info = await api(base, `/api/plots/${reply.body.plot}`);
        assert.equal(info.body.truncated, true);
        assert.ok(info.body.recorded <= 50, `recorded ${info.body.recorded}`);
        const exported = await api(base, `/api/plots/${reply.body.plot}/trace?token=${reply.body.token}`);
        assert.equal(exported.status, 409);
        assert.match(exported.body.error, /cannot be exported/);
        assert.ok(design.settle_ticks < 60);
        const released = await client.next((m) => m.state === 'released', 5000);
        assert.match(released.why, /time was up/);
        await waitFor(() => world.cleared.length === 1);
    } finally {
        await backend.close();
    }
});

test('a fresh plot nobody connects to is released after a short grace, not the idle time', async () => {
    const { backend, world, base } = await live({ plots: 1, idleSeconds: 600, freshSeconds: 0.5 });
    try {
        const reply = await submit(base, 'not1');
        assert.equal(reply.status, 200, reply.body.error);
        const started = Date.now();
        await waitFor(() => world.cleared.length === 1, 5000);
        assert.ok(Date.now() - started < 3000, 'released within the grace period');
        assert.equal((await api(base, `/api/plots/${reply.body.plot}`)).status, 404);
    } finally {
        await backend.close();
    }
});

test('compiles run one at a time, the queue is bounded, and a source seen before is served from the cache', async () => {
    const { backend, base } = await live({ plots: 4, compile: { seconds: 60, memoryMb: 1536, queue: 1 } });
    try {
        const replies = await Promise.all([submit(base, 'not1'), submit(base, 'nor2'), submit(base, 'nor3')]);
        const statuses = replies.map((r) => r.status).sort();
        assert.ok(statuses.includes(429), `one submission was turned away while two were in hand: ${statuses}`);
        assert.ok(statuses.filter((s) => s === 200).length >= 1, `at least one got through: ${statuses}`);
        assert.match(replies.find((r) => r.status === 429).body.error, /busy|queue|again/);
        for (const r of replies.filter((r) => r.status === 200)) await api(base, `/api/plots/${r.body.plot}?token=${r.body.token}`, { method: 'DELETE' });
        const before = (await api(base, '/api/status')).body.compile_cache;
        const started = Date.now();
        const cached = await submit(base, 'not1');
        assert.equal(cached.status, 200, cached.body.error);
        assert.ok(Date.now() - started < 2000, `a cached compile took ${Date.now() - started} ms`);
        const after = (await api(base, '/api/status')).body.compile_cache;
        assert.equal(after.hits, before.hits + 1, 'the second not1 came from the cache');
        assert.equal(JSON.stringify(cached.body.design), JSON.stringify(JSON.parse(nativeDesign('not1'))));
    } finally {
        await backend.close();
    }
});

test('submissions and viewers are limited per address', async () => {
    const { backend, base, wsBase } = await live({ plots: 4, perIp: { plots: 1, compiles: 1, submissionsPerHour: 3 }, viewers: { perPlot: 2, perIp: 3 } });
    try {
        const release = (r) => api(base, `/api/plots/${r.body.plot}?token=${r.body.token}`, { method: 'DELETE' });
        const first = await submit(base, 'not1');
        assert.equal(first.status, 200, first.body.error);
        const second = await submit(base, 'nor2');
        assert.equal(second.status, 429, 'one plot per address');
        assert.match(second.body.error, /release/);
        await release(first);
        const pair = await Promise.all([submit(base, 'nor3'), submit(base, 'nor3')]);
        assert.deepEqual(pair.map((r) => r.status).sort(), [200, 429], 'one compile at a time per address');
        await release(pair.find((r) => r.status === 200));
        const third = await submit(base, 'nor2');
        assert.equal(third.status, 200, third.body.error);
        await release(third);
        const fourth = await submit(base, 'nor3');
        assert.equal(fourth.status, 429, 'three submissions an hour');
        assert.match(fourth.body.error, /hour/);

        // Viewers: so many per plot, so many per address, and only with an explicit plot number.
        const bare = connect(`${wsBase}/live`);
        await assert.rejects(bare.opened, '/live needs a plot number');
        const named = connect(`${wsBase}/live?plot=abc`);
        await assert.rejects(named.opened);
        assert.equal((await api(base, '/api/status')).body.plots.free, 4);
        // This address has spent its hour; a second backend gives it a fresh budget.
        const fresh = await live({ plots: 2, viewers: { perPlot: 2, perIp: 3 } });
        try {
            const one = await submit(fresh.base, 'not1');
            const two = await submit(fresh.base, 'nor2');
            const v1 = connect(`${fresh.wsBase}/live?plot=${one.body.plot}`);
            const v2 = connect(`${fresh.wsBase}/live?plot=${one.body.plot}`);
            await Promise.all([v1.opened, v2.opened]);
            const v3 = connect(`${fresh.wsBase}/live?plot=${one.body.plot}`);
            await assert.rejects(v3.opened, 'two viewers per plot');
            const v4 = connect(`${fresh.wsBase}/live?plot=${two.body.plot}`);
            await v4.opened;
            const v5 = connect(`${fresh.wsBase}/live?plot=${two.body.plot}`);
            await assert.rejects(v5.opened, 'three viewers per address');
            v1.close();
            await waitFor(() => fresh.backend.plots.get(one.body.plot).viewers.size === 1);
            const v6 = connect(`${fresh.wsBase}/live?plot=${two.body.plot}`);
            await v6.opened;
            for (const v of [v2, v4, v6]) v.close();
        } finally {
            await fresh.backend.close();
        }
    } finally {
        await backend.close();
    }
});

test('a viewer that does not read is dropped, and the others keep getting the stream', async () => {
    const { backend, world, base, wsBase } = await live({ plots: 1, viewers: { perPlot: 4, perIp: 4, maxBufferedBytes: 4096 } });
    try {
        const reply = await submit(base, 'nor2');
        assert.equal(reply.status, 200, reply.body.error);
        const { origin } = reply.body;
        const slow = await rawViewer(backend.port, `/live?plot=${reply.body.plot}`);
        const good = connect(`${wsBase}/live?plot=${reply.body.plot}`);
        await good.opened;
        await waitFor(() => backend.plots.get(reply.body.plot).viewers.size === 2);
        // A lot of changes inside the plot: the reader sees them all, the sleeper fills up and goes.
        let flips = 0;
        const closed = new Promise((ok) => slow.on('close', ok));
        for (let round = 0; round < 2000 && backend.plots.get(reply.body.plot).viewers.size === 2; round++) {
            const changes = [];
            for (let x = 0; x < 25; x++) for (let z = 0; z < 10; z++) changes.push([key(add([x, 2, z], origin)), round % 2 ? 'minecraft:stone' : 'minecraft:redstone_block']);
            world.emit(world.tick + 1, changes);
            flips += 1;
            if (round % 20 === 19) await sleep(5);
        }
        assert.equal(backend.plots.get(reply.body.plot).viewers.size, 1, `the sleeper was dropped after ${flips} batches`);
        slow.resume(); // a paused socket never sees the end the server sent
        await closed;
        const last = await good.next((m) => m.tick === world.tick, 5000);
        assert.ok(last.changes.length > 0, 'the reader got the last batch');
        good.close();
    } finally {
        await backend.close();
    }
});

test('lever changes are paced by the design timing, coalesced to the latest request, and bounded', async () => {
    const { backend, world, base, wsBase } = await live({ plots: 1 });
    try {
        const reply = await submit(base, 'nor2');
        assert.equal(reply.status, 200, reply.body.error);
        const { design, origin } = reply.body;
        const { client } = await control(wsBase, reply.body);
        world.emit(100 + design.settle_ticks + 1, []);
        await client.next((m) => m.state === 'live');
        const plot = backend.plots.get(reply.body.plot);
        const a = design.inputs.find((p) => p.name === 'a');
        const probe = add(pos(a.probe), origin);

        // A flood: 200 toggles as fast as the socket takes them.
        for (let i = 0; i < 200; i++) client.send({ set: 'a', on: i % 2 === 0 });
        const acks = [];
        for (let i = 0; i < 200; i++) acks.push(await client.next((m) => m.ok !== undefined));
        assert.ok(acks.every((m) => m.ok), 'every request is accepted');
        assert.equal(acks.filter((m) => m.queued === false).length, 1, 'one change goes out at once');
        assert.ok(acks.filter((m) => m.queued).every((m) => m.tick === world.tick + 8), `the rest wait for the next slot, 8 ticks on: ${JSON.stringify(acks.find((m) => m.queued))}`);
        assert.equal(world.setblocks.length, 1, 'one command went to the server');
        assert.deepEqual(world.setblocks[0].slice(0, 2), [probe, a.on]);
        assert.ok(plot.pendingLevers.length <= plot.maxPending, `pending levers stay bounded (${plot.pendingLevers.length})`);
        // The slot comes: the latest request (off, the 200th) is what goes out, once.
        for (let t = 1; t <= 8; t++) {
            world.emit(world.tick + 1, []);
            await sleep(5);
        }
        assert.equal(world.setblocks.length, 2, 'one more command at the slot');
        assert.deepEqual(world.setblocks[1].slice(0, 2), [probe, a.off]);
        assert.equal(world.setblocks[1][2], world.tick, 'sent at the slot tick');
        for (let t = 0; t < 20; t++) world.emit(world.tick + 1, []);
        await sleep(20);
        assert.equal(world.setblocks.length, 2, 'nothing else was written');
        assert.deepEqual((await api(base, `/api/plots/${reply.body.plot}`)).body.inputs, [0, 0]);
        // Two inputs are paced apart: b is free to change while a waits.
        client.send({ set: 'a', on: true });
        client.send({ set: 'a', on: false });
        client.send({ set: 'b', on: true });
        const three = [];
        for (let i = 0; i < 3; i++) three.push(await client.next((m) => m.ok !== undefined));
        assert.deepEqual(three.map((m) => `${m.set}:${m.on}:${m.queued}`).sort(), ['a:false:true', 'a:true:false', 'b:true:false']);
        assert.equal(world.setblocks.length, 4);
        client.close();
    } finally {
        await backend.close();
    }
});

test('a flood of lever changes on a clocked design costs one command per input window', async () => {
    const world = new FakeWorld();
    const { backend, base, wsBase } = await live({ plots: 1 }, world);
    try {
        const reply = await submit(base, 'shift2');
        assert.equal(reply.status, 200, reply.body.error);
        const { design, origin } = reply.body;
        const seq = design.timing.sequential;
        const { client } = await control(wsBase, reply.body);
        await world.step(design.settle_ticks + 2);
        await client.next((m) => m.state === 'live');
        const plot = backend.plots.get(reply.body.plot);
        const d = design.inputs.find((p) => p.name !== seq.clock);
        const probe = add(pos(d.probe), origin);
        const leverWrites = () => world.setblocks.filter((s) => s[3] === 'levers');
        // Step to a change tick: input_change_at after an active edge, where a request goes out at once.
        for (let n = 0; n < 4 * seq.period && !(plot.clock.edges >= 2 && plot.leverPlan(d).now); n++) await world.step(1);
        assert.ok(plot.leverPlan(d).now, 'at the change tick');
        const changeTick = world.tick;
        assert.equal(changeTick, plot.clock.edgeTick + seq.input_change_at);

        // A flood in that one tick: 200 toggles as fast as the socket takes them.
        for (let i = 0; i < 200; i++) client.send({ set: d.name, on: i % 2 === 0 });
        const acks = [];
        for (let i = 0; i < 200; i++) acks.push(await client.next((m) => m.ok !== undefined));
        assert.ok(acks.every((m) => m.ok), 'every request is accepted');
        assert.equal(acks.filter((m) => m.queued === false).length, 1, 'one change goes out at once');
        assert.equal(leverWrites().length, 1, 'one lever command went to the server in the window');
        assert.deepEqual(leverWrites()[0].slice(0, 3), [probe, d.on, changeTick]);
        const nextWindow = changeTick + seq.period;
        assert.ok(acks.filter((m) => m.queued).every((m) => m.tick === nextWindow), `the rest wait for the next edge's window at ${nextWindow}: ${JSON.stringify(acks.find((m) => m.queued))}`);
        assert.ok(plot.pendingLevers.length <= plot.maxPending);

        // The next window: the latest request (off, the 200th) goes out, once, at the edge plus input_change_at.
        await world.step(seq.period + 2);
        assert.equal(leverWrites().length, 2, 'one more command at the next window');
        assert.deepEqual(leverWrites()[1].slice(0, 3), [probe, d.off, nextWindow]);
        const info = await api(base, `/api/plots/${reply.body.plot}`);
        assert.deepEqual([info.body.clock.late, info.body.clock.lost], [0, 0], 'the clock was not disturbed');
        client.close();
    } finally {
        await backend.close();
    }
});

test('clock edges are timed from where they landed, late ones are counted, and the plot reports it', async () => {
    const world = new FakeWorld();
    world.lateLanes.add('clock'); // every clock setblock shows up one tick late
    const { backend, base, wsBase } = await live({ plots: 1 }, world);
    try {
        const reply = await submit(base, 'shift2');
        assert.equal(reply.status, 200, reply.body.error);
        const { design } = reply.body;
        const seq = design.timing.sequential;
        const { client } = await control(wsBase, reply.body);
        await world.step(design.settle_ticks + 2);
        await client.next((m) => m.state === 'live');
        await world.step(3 * seq.period + 10);
        const info = await api(base, `/api/plots/${reply.body.plot}`);
        const clock = info.body.clock;
        assert.ok(clock.edges >= 3, `the clock ran (${clock.edges} edges)`);
        assert.equal(clock.max_jitter, 1, 'every edge landed a tick late');
        assert.equal(clock.late, world.setblocks.filter((s) => s[3] === 'clock').length - (backend.plots.get(reply.body.plot).clock.inFlight ? 1 : 0), 'every landed flip was counted late');
        assert.equal(clock.lost, 0);
        // The half-periods are kept from the landed ticks: each flip is scheduled a half-period
        // after the previous one landed, and lands a tick late itself, so landed flips are
        // high+1 and period-high+1 apart (a late edge stretches the period, never shortens it).
        const exported = await api(base, `/api/plots/${reply.body.plot}/trace?token=${reply.body.token}`);
        const clockProbe = design.inputs.find((p) => p.name === seq.clock).probe;
        const flips = exported.body.fixture.events.filter((e) => clockProbe in e.set).map((e) => e.tick);
        assert.ok(flips.length >= 6, `flips recorded: ${flips}`);
        const gaps = flips.slice(1).map((t, i) => t - flips[i]);
        assert.deepEqual(gaps, gaps.map((_, i) => (i % 2 === 0 ? seq.high : seq.period - seq.high) + 1), `flip gaps ${gaps}`);
        assert.ok(world.setblocks.filter((s) => s[3] === 'clock').length === flips.length + (backend.plots.get(reply.body.plot).clock.inFlight ? 1 : 0), 'clock commands go on the clock lane');
        client.close();
    } finally {
        await backend.close();
    }
});

test('a stalled backend sends one flip and waits for it: the period stretches, nothing is lost or doubled', async () => {
    const world = new FakeWorld();
    const { backend, base, wsBase } = await live({ plots: 1 }, world);
    try {
        const reply = await submit(base, 'shift2');
        assert.equal(reply.status, 200, reply.body.error);
        const { design } = reply.body;
        const seq = design.timing.sequential;
        const { client } = await control(wsBase, reply.body);
        await world.step(design.settle_ticks + 2);
        await client.next((m) => m.state === 'live');
        await world.step(seq.period + 5);
        const sentBefore = world.setblocks.length;
        assert.ok(sentBefore >= 2, `the clock ran (${sentBefore} flips)`);
        // The backend falls three periods behind: the ticks come, the answers to its commands do not.
        world.holdAll = true;
        await world.step(3 * seq.period);
        assert.equal(world.setblocks.length, sentBefore + 1, 'one flip went out during the stall, no more');
        world.holdAll = false;
        await world.step(2 * seq.period + 5);
        const info = await api(base, `/api/plots/${reply.body.plot}`);
        assert.equal(info.body.clock.lost, 0, 'no flip was given up on');
        assert.equal(info.body.clock.late, 1, 'the one flip sent into the stall landed late');
        // The flip went out up to a period before the stall and landed when it ended.
        assert.ok(info.body.clock.max_jitter >= 2 * seq.period, `its jitter was the stall (${info.body.clock.max_jitter} ticks)`);
        // After it landed, the flips went on at the half-periods, from where it landed.
        const exported = await api(base, `/api/plots/${reply.body.plot}/trace?token=${reply.body.token}`);
        const clockProbe = design.inputs.find((p) => p.name === seq.clock).probe;
        const flips = exported.body.fixture.events.filter((e) => clockProbe in e.set).map((e) => e.tick);
        const gaps = flips.slice(1).map((t, i) => t - flips[i]);
        assert.ok(gaps.every((g) => g === seq.high || g === seq.period - seq.high || g >= 2 * seq.period), `no half-period was shortened: ${gaps}`);
        assert.equal(gaps.filter((g) => g >= 2 * seq.period).length, 1, `exactly one stretched gap: ${gaps}`);
        client.close();
    } finally {
        await backend.close();
    }
});

test('when the stream drops and reconnects, the plots are released as stale and the status says so', async () => {
    const { backend, world, base, wsBase } = await live({ plots: 2 });
    try {
        const reply = await submit(base, 'nor2');
        assert.equal(reply.status, 200, reply.body.error);
        const { client } = await control(wsBase, reply.body);
        world.reconnectStream();
        const released = await client.next((m) => m.state === 'released');
        assert.match(released.why, /stream/);
        assert.equal(released.stale, true);
        await waitFor(() => world.cleared.length === 1);
        const status = await api(base, '/api/status');
        assert.equal(status.body.minecraft.stream_reconnects, 1);
        assert.deepEqual(status.body.plots, { total: 2, free: 2, broken: 0 });
        assert.equal((await api(base, `/api/plots/${reply.body.plot}`)).status, 404);
    } finally {
        await backend.close();
    }
});

test('the stream client reconnects once per drop, with a backoff, never in parallel', async () => {
    const stream = fakeStream();
    const port = await stream.listen();
    const messages = [];
    const reconnects = [];
    const logs = [];
    const client = observe(port, (m) => messages.push(m), { log: (m) => logs.push(m), onReconnect: () => reconnects.push(Date.now()), maxDelay: 300 });
    try {
        await client.open();
        await waitFor(() => messages.length === 1);
        assert.equal(messages[0].hello, 'redfab-observer');
        assert.equal(client.up, true);
        stream.send(JSON.stringify({ tick: 5, changes: [] }));
        await waitFor(() => messages.length === 2);
        assert.ok(client.age !== null && client.age < 1000);

        // The mod drops the client: one reconnection, one new hello.
        stream.dropAll();
        await waitFor(() => reconnects.length === 1, 5000);
        await waitFor(() => messages.length === 3);
        assert.equal(stream.accepted, 2, 'exactly one new connection was made');
        assert.equal(client.reconnects, 1);
        assert.equal(stream.clients, 1);

        // The mod goes away: attempts are spaced out, not multiplied.
        await stream.close();
        await waitFor(() => client.up === false, 5000);
        const before = client.attempts;
        await sleep(1200);
        const made = client.attempts - before;
        assert.ok(made >= 2 && made <= 6, `${made} attempts in 1.2 s with a backoff from 250 ms to 300 ms`);
        assert.ok(logs.some((l) => /closed; reconnecting/.test(l)));
    } finally {
        client.close();
        await stream.close();
    }
});

test('the RCON client re-dials after a close and after a timeout, and rejects the queue at once', async () => {
    const server = fakeRcon('secret');
    const port = await server.listen();
    try {
        await assert.rejects(Rcon.connect('127.0.0.1', port, 'wrong', { timeout: 2000 }), /authentication/);
        const rcon = await Rcon.connect('127.0.0.1', port, 'secret', { timeout: 2000, commandTimeout: 300 });
        assert.equal(await rcon.command('list'), 'ran list');
        assert.equal(rcon.reconnects, 0);

        // The server drops every socket: the next command dials and authenticates again.
        server.dropAll();
        await waitFor(() => rcon.up === false);
        assert.equal(await rcon.command('list'), 'ran list');
        assert.equal(rcon.reconnects, 1);

        // The server stops answering: the command in flight and the one behind it fail
        // together, within the command timeout, and the one after re-dials.
        server.stall = true;
        const started = Date.now();
        const results = await Promise.allSettled([rcon.command('one'), rcon.command('two')]);
        assert.ok(Date.now() - started < 1500, `both rejected in ${Date.now() - started} ms`);
        assert.deepEqual(results.map((r) => r.status), ['rejected', 'rejected']);
        assert.match(results[0].reason.message, /no reply/);
        assert.match(results[1].reason.message, /no reply|closed/);
        server.stall = false;
        assert.equal(await rcon.command('three'), 'ran three');
        assert.equal(rcon.reconnects, 2);
        assert.equal(server.connections, 4, 'one socket per dial: the wrong password, then three good ones');

        rcon.close();
        await assert.rejects(rcon.command('four'), /closed/);
    } finally {
        await server.close();
    }
});

test('an urgent RCON command goes out ahead of the queue, behind the one in flight', async () => {
    const server = fakeRcon('secret');
    const port = await server.listen();
    try {
        const rcon = await Rcon.connect('127.0.0.1', port, 'secret', { timeout: 2000 });
        const order = [];
        const done = (text) => order.push(text);
        const all = Promise.all([
            rcon.command('first').then(done),
            rcon.command('second').then(done),
            rcon.command('edge', { urgent: true }).then(done),
            rcon.command('third').then(done),
        ]);
        await all;
        assert.deepEqual(order, ['ran first', 'ran edge', 'ran second', 'ran third']);
        rcon.close();
    } finally {
        await server.close();
    }
});

test('the status is not live when the game, its RCON or the stream is gone', async () => {
    const { backend, world, base } = await live({ plots: 1 });
    try {
        assert.equal((await api(base, '/api/status')).body.live, true);
        world.healthOverride = { rcon_up: false };
        assert.equal((await api(base, '/api/status')).body.live, false, 'RCON down');
        world.healthOverride = { stream_age_ms: 6000 };
        assert.equal((await api(base, '/api/status')).body.live, false, 'the stream is stale');
        world.healthOverride = { stream_up: false, stream_age_ms: null };
        assert.equal((await api(base, '/api/status')).body.live, false, 'the stream is down');
        world.healthOverride = { running: false };
        const dead = await api(base, '/api/status');
        assert.equal(dead.body.live, false, 'the game died');
        assert.equal(dead.body.minecraft.running, false);
        world.healthOverride = {};
        assert.equal((await api(base, '/api/status')).body.live, true);
    } finally {
        await backend.close();
    }
});

test('a placement the game refuses is answered as an error, the plot is given back, and the backend lives on', async () => {
    const { backend, world, base } = await live({ plots: 2 });
    try {
        world.failPlaces = 1;
        const failed = await submit(base, 'not1');
        assert.equal(failed.status, 500);
        // The failed placement's waiter was rejected with nobody awaiting it: an
        // unhandled rejection here would have ended this process (seen live).
        await sleep(50);
        await waitFor(async () => (await api(base, '/api/status')).body.plots.free === 2, 5000);
        assert.equal(world.cleared.length, 1, 'the plot of the failed placement is cleared');
        const again = await submit(base, 'not1');
        assert.equal(again.status, 200, again.body.error);
    } finally {
        await backend.close();
    }
});

test('a plot whose clear fails is broken, not free, until a retry clears it', async () => {
    const { backend, world, base } = await live({ plots: 2, clearRetrySeconds: 0.2 });
    try {
        const reply = await submit(base, 'not1');
        assert.equal(reply.status, 200, reply.body.error);
        world.failClears = 1;
        const released = await api(base, `/api/plots/${reply.body.plot}?token=${reply.body.token}`, { method: 'DELETE' });
        assert.equal(released.status, 200);
        let status = await api(base, '/api/status');
        assert.deepEqual(status.body.plots, { total: 2, free: 1, broken: 1 }, 'the plot is out of service');
        assert.equal((await api(base, `/api/plots/${reply.body.plot}`)).status, 404);
        const other = await submit(base, 'nor2');
        assert.equal(other.status, 200, other.body.error);
        assert.notDeepEqual(other.body.origin, reply.body.origin, 'the broken plot is not handed out');
        await waitFor(() => world.cleared.length === 1, 5000);
        await waitFor(async () => (await api(base, '/api/status')).body.plots.broken === 0, 5000);
        status = await api(base, '/api/status');
        assert.deepEqual(status.body.plots, { total: 2, free: 1, broken: 0 }, 'cleared on retry, back in service');
    } finally {
        await backend.close();
    }
});

test('stream states are matched by meaning: property order does not matter', async () => {
    const world = new FakeWorld();
    world.scramble = true;
    const { backend, base, wsBase } = await live({ plots: 1 }, world);
    try {
        const reply = await submit(base, 'nor2');
        assert.equal(reply.status, 200, reply.body.error);
        const info = await api(base, `/api/plots/${reply.body.plot}`);
        assert.equal(info.body.placement.matched, true, 'the placement matched although the game wrote properties in another order');
        const { client, hello } = await control(wsBase, reply.body);
        assert.deepEqual(hello.lamps, [1], 'the lit lamp is recognised');
        client.close();
    } finally {
        await backend.close();
    }
});

test('the game does not start without the EULA accepted, and Java is found through the link', () => {
    assert.throws(() => requireEula('/nonexistent/eula-accepted'), /aka\.ms\/MinecraftEULA.*--accept-eula/);
    const file = join(mkdtempSync(join(tmpdir(), 'redfab-eula-')), 'eula-accepted');
    writeFileSync(file, 'accepted\n');
    assert.doesNotThrow(() => requireEula(file));
    // The link setup.sh makes is what the backend uses whenever it exists; the macOS layout is only the fallback before it.
    if (existsSync(join(root, '.tools/java'))) assert.equal(javaPath(), join(root, '.tools/java'));
    else assert.equal(javaPath(), join(root, '.tools/jdk/Contents/Home/bin/java'));
});

test('a server left behind is killed by its pid file before the run directory goes', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'redfab-pid-'));
    const logs = [];
    assert.equal(killLeftover(dir, (m) => logs.push(m)), null, 'no pid file, nothing to kill');
    writeFileSync(join(dir, 'server.pid'), 'not a pid\n');
    assert.equal(killLeftover(dir, (m) => logs.push(m)), null, 'a garbled pid file is ignored');
    const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });
    const exited = new Promise((ok) => child.on('exit', (code, signal) => ok({ code, signal })));
    writeFileSync(join(dir, 'server.pid'), `${child.pid}\n`);
    assert.equal(killLeftover(dir, (m) => logs.push(m)), child.pid);
    assert.deepEqual(await exited, { code: null, signal: 'SIGKILL' }, 'the leftover is killed, not asked');
    assert.ok(logs.some((l) => l.includes(`pid ${child.pid}`)), 'and it is logged');
    assert.equal(killLeftover(dir, (m) => logs.push(m)), null, 'a pid that is gone is not an error');
    assert.equal(logs.length, 1);
});

test('the server configuration is the shared profile plus the backend\'s own differences', () => {
    const profile = JSON.parse(readFileSync(PROFILE_FILE, 'utf8'));
    assert.deepEqual(GAMERULES, profile.gamerules, 'the gamerules are the profile\'s, the same the harness sets');
    assert.equal(Object.keys(GAMERULES).length, 14);
    const written = serverProperties({ gamePort: 25565, rconPort: 25575, password: 'hunter2' });
    for (const [k, v] of Object.entries(profile.properties)) assert.equal(written[k], v, k);
    assert.deepEqual([written['server-port'], written['rcon.port'], written['rcon.password']], ['25565', '25575', 'hunter2']);
    const own = Object.keys(written).filter((k) => !(k in profile.properties)).sort();
    assert.deepEqual(own, ['motd', 'online-mode', 'rcon.password', 'rcon.port', 'server-port'], 'this run\'s values and the backend\'s two differences, nothing else');
    assert.equal(written['online-mode'], 'false');
    assert.equal(written.motd, MOTD);
    const args = javaArgs({ runDir: '/run', observerPort: 7 });
    assert.deepEqual(args.slice(0, profile.jvm_args.length), profile.jvm_args, 'the JVM arguments are the profile\'s');
    assert.ok(args.includes('-Dredfab.run=/run') && args.includes('-Dredfab.observer.port=7') && args.at(-1) === '--nogui');
});

test('every shipped design fits the default plot', () => {
    const names = readdirSync(join(root, 'designs')).filter((n) => !n.startsWith('_') && !n.startsWith('.')).sort();
    assert.ok(names.includes('adder4') && names.length >= 15, `the shipped designs: ${names}`);
    const sizes = {};
    for (const name of names) {
        let file = join(root, 'build/designs', name, 'design.json');
        if (!existsSync(file)) {
            // Not built by the gate yet (a fresh checkout): build it here.
            const dir = mkdtempSync(join(tmpdir(), 'redfab-fit-'));
            execFileSync('cargo', ['run', '-q', '--locked', '-p', 'redfab-cli', '--', 'build', join(root, 'designs', name, 'top.sv'), '--top', 'top', '-o', dir], { cwd: root, stdio: 'pipe' });
            file = join(dir, 'design.json');
        }
        sizes[name] = JSON.parse(readFileSync(file, 'utf8')).size;
    }
    const [w, d] = DEFAULTS.plotSize;
    for (const [name, size] of Object.entries(sizes)) {
        assert.ok(size[0] <= w && size[2] <= d, `${name} is ${size[0]} by ${size[2]} blocks; a plot is ${w} by ${d}, so the page's example would be refused`);
    }
});

test('the backend serves the playground page and its files, and nothing else', async () => {
    const { backend, base } = await live({ plots: 1 });
    try {
        const page = await fetch(`${base}/`);
        assert.equal(page.status, 200);
        assert.match(await page.text(), /redfab playground/);
        const core = await fetch(`${base}/core/redfab_wasm.wasm`);
        assert.equal(core.status, 200);
        assert.equal(core.headers.get('content-type'), 'application/wasm');
        const outside = await fetch(`${base}/../versions.env`);
        assert.equal(outside.status, 404);
        const other = await fetch(`${base}/node_modules/puppeteer-core/package.json`);
        assert.equal(other.status, 404, 'only the page\'s two packages are served');
        const needed = await fetch(`${base}/node_modules/three/package.json`);
        assert.equal(needed.status, 200);
        const malformed = await fetch(`${base}/%`);
        assert.equal(malformed.status, 404, 'a malformed path is not found, not an error');
        const log = (await api(base, '/api/nothing?token=secret')).status;
        assert.equal(log, 404);
        // The answers the backend writes itself carry the page's policy too.
        for (const path of ['/api/status', '/api/nothing']) {
            const answer = await fetch(`${base}${path}`);
            assert.match(answer.headers.get('content-security-policy') ?? '', /default-src 'self'/, `${path} carries the policy`);
            assert.equal(answer.headers.get('x-content-type-options'), 'nosniff', `${path} is nosniff`);
        }
    } finally {
        await backend.close();
    }
});

test('a failing request is logged by its path alone, never its query string', async () => {
    const world = new FakeWorld();
    const logs = [];
    const { backend, base } = await live({ plots: 2, log: (m) => logs.push(m) }, world);
    try {
        world.failPlaces = 1; // the game's refusal is not a known error: the request is logged with its stack
        const failed = await api(base, '/api/submit?token=secret&plot=0', { method: 'POST', body: { source: designSource('not1'), top: 'top', name: 'not1' } });
        assert.equal(failed.status, 500);
        assert.ok(logs.some((l) => /request POST \/api\/submit failed/.test(l)), `the failure is logged: ${logs.join(' | ')}`);
        assert.equal((await api(base, '/api/plots/0/trace?token=secret')).status, 404);
        assert.equal((await api(base, '/%?token=secret')).status, 404);
        assert.ok(logs.length >= 2, 'the sandbox and the plot logged too');
        for (const line of logs) assert.ok(!line.includes('secret') && !line.includes('?'), `a log line carries the query string: ${line}`);
    } finally {
        await backend.close();
    }
});

test('behind a proxy, addresses come from X-Forwarded-For only when told to, and old ones are forgotten', async () => {
    const forwarded = (ip) => ({ 'x-forwarded-for': `${ip}, 10.9.9.9` });
    // Not trusted: the header is ignored, so both submissions are one address's.
    const plain = await live({ plots: 4, perIp: { plots: 1, compiles: 8, submissionsPerHour: 1000 } });
    try {
        const first = await submit(plain.base, 'not1', {}, forwarded('203.0.113.1'));
        assert.equal(first.status, 200, first.body.error);
        const second = await submit(plain.base, 'nor2', {}, forwarded('203.0.113.2'));
        assert.equal(second.status, 429, 'the header does not make a second address');
        assert.deepEqual(plain.backend.addresses, ['127.0.0.1']);
    } finally {
        await plain.backend.close();
    }
    // Trusted (behind the reverse proxy): the first address in the header is the client.
    mock.timers.enable({ apis: ['Date'], now: Date.now() });
    const proxied = await live({ plots: 4, trustProxy: true, perIp: { plots: 1, compiles: 8, submissionsPerHour: 1000 }, viewers: { perPlot: 8, perIp: 1 } });
    try {
        const first = await submit(proxied.base, 'not1', {}, forwarded('203.0.113.1'));
        assert.equal(first.status, 200, first.body.error);
        const second = await submit(proxied.base, 'nor2', {}, forwarded('203.0.113.2'));
        assert.equal(second.status, 200, second.body.error);
        const third = await submit(proxied.base, 'nor3', {}, forwarded('203.0.113.1'));
        assert.equal(third.status, 429, 'one plot per forwarded address');
        assert.deepEqual(proxied.backend.addresses, ['203.0.113.1', '203.0.113.2']);
        // Viewers are counted by the forwarded address too.
        const path = `/live?plot=${first.body.plot}`;
        const a = await rawViewer(proxied.backend.port, path, forwarded('203.0.113.7'));
        await assert.rejects(rawViewer(proxied.backend.port, path, forwarded('203.0.113.7')), /429/, 'one viewer per address');
        const b = await rawViewer(proxied.backend.port, path, forwarded('203.0.113.8'));
        a.destroy();
        b.destroy();
        // An hour on, the addresses with no submission since are forgotten at the next submission.
        mock.timers.tick(3600 * 1000 + 1);
        const later = await submit(proxied.base, 'not1', {}, forwarded('203.0.113.3'));
        assert.equal(later.status, 200, later.body.error);
        assert.deepEqual(proxied.backend.addresses, ['203.0.113.3'], 'the old addresses are gone');
    } finally {
        mock.timers.reset();
        await proxied.backend.close();
    }
});

test('the upgrade refuses another site\'s Origin and takes the page\'s own or a listed one', async () => {
    const { backend, base } = await live({ plots: 1, origins: ['https://page.example'] });
    try {
        const reply = await submit(base, 'nor2');
        assert.equal(reply.status, 200, reply.body.error);
        const path = `/live?plot=${reply.body.plot}`;
        await assert.rejects(rawViewer(backend.port, path, { Origin: 'http://evil.example' }), /403/, 'another site');
        await assert.rejects(rawViewer(backend.port, path, { Origin: 'not a url' }), /403/);
        const own = await rawViewer(backend.port, path, { Origin: `http://127.0.0.1:${backend.port}` });
        const listed = await rawViewer(backend.port, path, { Origin: 'https://page.example' });
        const none = await rawViewer(backend.port, path);
        await waitFor(() => backend.plots.get(reply.body.plot).viewers.size === 3);
        for (const s of [own, listed, none]) s.destroy();
    } finally {
        await backend.close();
    }
});

test('the compile child sees only the environment it needs', async () => {
    const { backend, base } = await live({ plots: 1 });
    process.env.REDFAB_PROBE_SECRET = 'do-not-leak';
    try {
        const reply = await api(base, '/api/submit', { method: 'POST', body: { source: `// redfab-test: env\n${designSource('not1')}`, top: 'top' } });
        assert.equal(reply.status, 400);
        assert.match(reply.body.error, /^env: /);
        const keys = reply.body.error.slice(5).split(',');
        assert.ok(keys.includes('REDFAB_WASM') && keys.includes('REDFAB_TEST_HOOKS') && keys.includes('PATH'), `the child has what it needs: ${keys}`);
        assert.ok(!keys.includes('REDFAB_PROBE_SECRET'), 'and not the parent\'s secrets');
        // Beyond the allow-list: NODE_CHANNEL_* is the IPC channel Node adds, __CF_USER_TEXT_ENCODING the macOS loader.
        for (const k of keys) assert.match(k, /^(PATH|HOME|TMPDIR|LANG|LC_ALL|REDFAB_WASM|REDFAB_TEST_HOOKS|NODE_CHANNEL_[A-Z_]+|__CF_USER_TEXT_ENCODING)$/, `${k} is not on the allow-list`);
    } finally {
        delete process.env.REDFAB_PROBE_SECRET;
        await backend.close();
    }
});

await sleep(0);
