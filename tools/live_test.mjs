// The live playground's backend under Node, without Minecraft. See `check`.
//
//   node --test tools/live_test.mjs
//
// The backend is given a fake world that records what it is asked to do and
// plays the part of the game's stream. For the tick-for-tick check the fake
// world is backed by the native simulator, so what it streams is what the game
// would say, and the trace the backend records must equal the simulator's own.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { loadCore } from '../playground/core.js';
import { LIMITS } from '../playground/compile.js';
import { createLive } from '../server/live.mjs';
import { api, changesAt, connect, designSource, firstTraceDifference, nativeDesign, root, sleep, waitFor, wasmPath } from './live_common.mjs';

execFileSync('cargo', ['build', '-q', '-p', 'redfab-wasm', '--target', 'wasm32-unknown-unknown', '--release'], { cwd: root, stdio: 'inherit' });
const core = await loadCore(readFileSync(wasmPath));

const pos = (text) => text.split(',').map(Number);
const key = (p) => p.join(',');
const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];

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
        this.forceloaded = [];
        this.listeners = [];
        this.sims = [];
        this.tickTimes = { avg_ms: 1.5, p95_ms: 2.5 };
    }
    onBatch(cb) {
        this.listeners.push(cb);
    }
    emit(tick, changes) {
        this.tick = Math.max(this.tick, tick);
        for (const cb of this.listeners) cb({ tick, changes });
    }
    async forceload(box) {
        this.forceloaded.push(box);
    }
    async clear(box) {
        this.cleared.push(box);
    }
    async tickTime() {
        return this.tickTimes;
    }
    /// A strict placement: the blocks appear between ticks, labelled with the last completed tick.
    async place({ name, structure, design, origin }) {
        assert.ok(structure instanceof Uint8Array && structure.length > 0, 'a structure file is placed');
        this.placed.push({ name, origin, blocks: Object.keys(design.blocks).length });
        if (this.simulate) {
            const { handle } = core.call({ op: 'sim_new', design: JSON.stringify(design) });
            this.sims.push({ handle, design, origin });
        }
        this.emit(this.tick, Object.entries(design.blocks).map(([p, s]) => [key(add(pos(p), origin)), s]));
    }
    /// A setblock between ticks. On a simulated design it is a lever: the simulator
    /// takes it and reports what changed at once (the probe, dust, lamps).
    async setblock(at, state) {
        this.setblocks.push([at, state]);
        const sim = this.sims.find((s) => s.design.inputs.some((pin) => key(add(pos(pin.probe), s.origin)) === key(at)));
        if (!sim) {
            this.emit(this.tick, [[key(at), state]]);
            return;
        }
        const input = sim.design.inputs.findIndex((pin) => key(add(pos(pin.probe), sim.origin)) === key(at));
        const reply = core.call({ op: 'sim_set', handle: sim.handle, input, on: state === sim.design.inputs[input].on });
        this.emit(this.tick, reply.changes.map(([p, s]) => [key(add(pos(p), sim.origin)), s]));
    }
    /// Runs the simulated designs: one message per tick with changes, a heartbeat
    /// every twenty, and a pause between ticks so the backend can react as it
    /// would to a real server.
    async step(ticks) {
        for (let k = 0; k < ticks; k++) {
            this.tick += 1;
            const changes = [];
            for (const sim of this.sims) {
                const reply = core.call({ op: 'sim_step', handle: sim.handle, ticks: 1 });
                changes.push(...reply.changes.map(([p, s]) => [key(add(pos(p), sim.origin)), s]));
            }
            if (changes.length || this.tick % 20 === 0) this.emit(this.tick, changes);
            await new Promise((ok) => setImmediate(ok));
            await new Promise((ok) => setImmediate(ok));
        }
    }
    simTrace(index = 0) {
        return core.call({ op: 'sim_trace', handle: this.sims[index].handle });
    }
}

async function live(options = {}, world = new FakeWorld()) {
    const backend = await createLive({ world, wasm: wasmPath, port: 0, testHooks: true, ...options });
    return { backend, world, base: `http://127.0.0.1:${backend.port}`, wsBase: `ws://127.0.0.1:${backend.port}` };
}

const submit = (base, name, extra = {}) => api(base, '/api/submit', { method: 'POST', body: { source: designSource(name), top: 'top', name, ...extra } });

/// Opens the plot's stream as its controller and waits for the hello.
async function control(wsBase, plot) {
    const client = connect(`${wsBase}/live?plot=${plot.plot}&token=${plot.token}`);
    await client.opened;
    const hello = await client.next((m) => m.hello !== undefined);
    return { client, hello };
}

test('the backend reports itself live and placed designs are the native builds', async () => {
    const { backend, world, base } = await live({ plots: 3 });
    try {
        const status = await api(base, '/api/status');
        assert.equal(status.status, 200);
        assert.equal(status.body.live, true);
        assert.deepEqual(status.body.plots, { total: 3, free: 3 });
        assert.equal(status.body.tick.avg_ms, 1.5);

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
        assert.deepEqual((await api(base, '/api/status')).body.plots, { total: 3, free: 2 });
        const info = await api(base, `/api/plots/${plot}`);
        assert.equal(info.body.placement.matched, true, 'the stream showed the design placed exactly');
        assert.equal(info.body.placement.tick, 100);
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
        assert.deepEqual(hello.lamps, [0]);
        assert.deepEqual(hello.inputs, [0, 0]);

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
        assert.deepEqual([ack.ok, ack.set, ack.on], [true, 'a', true]);
        const a = design.inputs.find((p) => p.name === 'a');
        assert.deepEqual(world.setblocks, [[add(pos(a.probe), origin), a.on]]);
        const update = await client.next((m) => m.changes !== undefined);
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
        world.emit(world.tick + 1, [[lampWorld, 'minecraft:redstone_lamp[lit=true]'], ['5000,64,5000', 'minecraft:stone'], [key(add([0, 0, -1], origin)), 'minecraft:stone']]);
        const lit = await viewer.next((m) => m.changes !== undefined && m.changes.length > 0);
        assert.deepEqual(lit.changes, [[y.lamp, 'minecraft:redstone_lamp[lit=true]']]);
        assert.deepEqual(lit.lamps, [1]);
        const again = await client.next((m) => m.changes !== undefined && m.changes.length > 0);
        assert.deepEqual(again.lamps, [1]);
        const info = await api(base, `/api/plots/${reply.body.plot}`);
        assert.equal(info.status, 200);
        assert.deepEqual([info.body.state, info.body.lamps, info.body.inputs], ['live', [1], [1, 0]]);
        client.close();
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
            const { design } = reply.body;
            const { client } = await control(wsBase, reply.body);
            await world.step(design.settle_ticks + 20);
            await client.next((m) => m.state === 'live');
            const seq = design.timing?.sequential;
            const data = seq ? design.inputs.find((p) => p.name !== seq.clock).name : 'a';
            for (const [on, ticks] of [[true, 11], [false, 7], [true, 3], [false, 2], [true, seq ? 2 * seq.period : 25], [false, 30]]) {
                client.send({ set: data, on });
                const ack = await client.next((m) => m.ok !== undefined);
                assert.equal(ack.ok, true, ack.error);
                await world.step(ticks);
            }
            await world.step(20);
            const exported = await api(base, `/api/plots/${reply.body.plot}/trace`);
            assert.equal(exported.status, 200, exported.body.error);
            const { fixture, trace } = exported.body;
            assert.equal(fixture.format, 'redfab-fixture/1');
            assert.equal(trace.format, 'redfab-trace/1');
            assert.equal(trace.source.kind, 'live');
            assert.equal(fixture.placement, 'strict');
            assert.deepEqual(fixture.size, design.size);
            assert.ok(fixture.events.length >= 6, `the lever changes were recorded as events (${fixture.events.length})`);
            const lamp = design.outputs[0].lamp;
            assert.ok(changesAt(trace, lamp) >= 2, `the lamp changed in the recording (${changesAt(trace, lamp)} times)`);

            const simulated = world.simTrace(world.sims.length - 1);
            assert.equal(firstTraceDifference(trace, simulated.trace), null, `${name}: live recording vs the simulator that played the game`);
            assert.deepEqual(fixture.events, simulated.fixture.events, `${name}: the events landed on the ticks the simulator saw`);

            const dir = mkdtempSync(join(tmpdir(), 'redfab-live-'));
            writeFileSync(join(dir, `${name}.fixture.json`), JSON.stringify(fixture));
            execFileSync('cargo', ['run', '-q', '-p', 'redfab-cli', '--', 'check-fixtures', dir], { cwd: root, stdio: 'pipe' });
            const replayed = JSON.parse(readFileSync(join(dir, `${name}.sim.trace.json`), 'utf8'));
            assert.equal(firstTraceDifference(trace, replayed), null, `${name}: live recording vs the native replay of its fixture`);
            client.close();
            await api(base, `/api/plots/${reply.body.plot}?token=${reply.body.token}`, { method: 'DELETE' });
        }
    } finally {
        await backend.close();
    }
});

test('a design that does not fit a plot is refused before anything is placed', async () => {
    const { backend, world, base } = await live({ plots: 2, plotSize: [64, 64] });
    try {
        const small = await submit(base, 'nor2');
        assert.equal(small.status, 200, small.body.error);
        const big = await submit(base, 'fa1');
        assert.equal(big.status, 400);
        assert.match(big.body.error, /fit|plot/);
        assert.equal(world.placed.length, 1);
        assert.deepEqual((await api(base, '/api/status')).body.plots, { total: 2, free: 1 });
    } finally {
        await backend.close();
    }
});

test('the submission is source only: limits hold, unexpected fields are refused, hostile sources are stopped', async () => {
    const { backend, world, base } = await live({ plots: 2, compile: { seconds: 20, memoryMb: 512, queue: 2 } });
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
        assert.equal(world.placed.length, 0);

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

        // The sandbox: a compile that hangs is killed at the time limit ...
        started = Date.now();
        const hung = await api(base, '/api/submit', { method: 'POST', body: { source: `// redfab-test: hang\n${designSource('not1')}`, top: 'top' } });
        assert.equal(hung.status, 400);
        assert.match(hung.body.error, /stopped|time/);
        assert.ok(Date.now() - started <= 23_000);
        // ... one that eats memory is killed at the memory limit, well before the time limit ...
        started = Date.now();
        const fat = await api(base, '/api/submit', { method: 'POST', body: { source: `// redfab-test: allocate 1200\n${designSource('not1')}`, top: 'top' } });
        assert.equal(fat.status, 400);
        assert.match(fat.body.error, /memory/);
        assert.ok(Date.now() - started <= 15_000, `killed in ${Date.now() - started} ms`);
        // ... and the backend is fine afterwards.
        const fine = await submit(base, 'not1');
        assert.equal(fine.status, 200, fine.body.error);
        assert.equal(world.placed.length, 1);
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

test('compiles run one at a time and the queue is bounded', async () => {
    const { backend, base } = await live({ plots: 4, compile: { seconds: 60, memoryMb: 1536, queue: 1 } });
    try {
        const replies = await Promise.all([submit(base, 'not1'), submit(base, 'nor2'), submit(base, 'nor3')]);
        const statuses = replies.map((r) => r.status).sort();
        assert.ok(statuses.includes(429), `one submission was turned away while two were in hand: ${statuses}`);
        assert.ok(statuses.filter((s) => s === 200).length >= 1, `at least one got through: ${statuses}`);
        assert.match(replies.find((r) => r.status === 429).body.error, /busy|queue|again/);
    } finally {
        await backend.close();
    }
});

test('the backend serves the playground page and its files', async () => {
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
    } finally {
        await backend.close();
    }
});

await sleep(0);
