// Plots: where submissions live in the world. Each plot holds one design: its
// origin, the state of its blocks as the stream reports them, the levers the
// controller has asked for, a recording of everything in the trace format, the
// clock driver for a clocked design, and the viewers watching it.
//
// Positions inside a plot are in design coordinates ("x,y,z" strings, as in the
// design file); the world position is the origin plus that. Tick numbers in the
// recording are relative to the tick the design was placed between.
//
// Lever changes are paced by the design's timing: a combinational design takes
// one change per input every `input_spacing` ticks (its torches' burnout
// budget), a clocked one takes changes `input_change_at` ticks after an active
// edge (the window its flip-flops were measured for). A change asked for too
// early waits for the next slot, and a newer request for the same input
// replaces it, so a flood of toggles costs the server one command per slot.

import { randomBytes } from 'node:crypto';
import { AIR, FIXTURE_FORMAT, TRACE_FORMAT, canonicalState, joinPos, parsePos } from './trace.mjs';

const LIT = 'minecraft:redstone_lamp[lit=true]';
const MAX_RECORDED_CHANGES = 500_000; // about 30 MB of export; a running counter4 makes ~25 changes a tick
const MIN_SPACING = 2; // ticks between changes of one input, whatever the design says
const PENDING_MS = 30_000; // a lever change the stream has not shown after this long is forgotten
const LOST_MS = 2000; // a clock flip not shown a period plus this after it went out is lost

export class PlotError extends Error {
    constructor(message, status = 400) {
        super(message);
        this.status = status;
    }
}

export class Plot {
    constructor(index, origin, world, log, maxRecordedChanges = MAX_RECORDED_CHANGES) {
        this.index = index;
        this.origin = origin;
        this.world = world;
        this.log = log;
        this.maxRecordedChanges = maxRecordedChanges;
        this.state = 'free';
        this.viewers = new Set();
        this.generation = 0;
        this.brokenBox = null;
        this.retryTimer = null;
        this.reset();
    }

    reset() {
        this.token = null;
        this.ip = null;
        this.design = null;
        this.structure = null;
        this.name = null;
        this.blocks = new Map();
        this.placedTick = null;
        this.matched = null;
        this.lastTick = null;
        this.lastTickAt = 0;
        this.ticks = new Map();
        this.events = new Map();
        this.recorded = 0;
        this.truncated = false;
        this.stale = false;
        this.pendingLevers = [];
        this.maxPending = 8;
        this.wanted = new Map();
        this.applied = new Map();
        this.clock = null;
        this.idleTimer = null;
        this.lifeTimer = null;
        this.placementWaiters = [];
        this.liveWaiters = [];
    }

    /// Holds the plot for a submission while it compiles.
    reserve(name, ip = null) {
        this.state = 'reserved';
        this.name = name;
        this.ip = ip;
        this.token = randomBytes(16).toString('hex');
        this.generation += 1;
        this.createdAt = Date.now();
    }

    get id() {
        return this.index;
    }

    /// The world box the design occupies.
    box() {
        const [sx, sy, sz] = this.design.size;
        const [x, y, z] = this.origin;
        return [[x, y, z], [x + sx - 1, y + sy - 1, z + sz - 1]];
    }

    inside(pos) {
        const [[x0, y0, z0], [x1, y1, z1]] = this.box();
        return pos[0] >= x0 && pos[0] <= x1 && pos[1] >= y0 && pos[1] <= y1 && pos[2] >= z0 && pos[2] <= z1;
    }

    toDesign(pos) {
        return joinPos([pos[0] - this.origin[0], pos[1] - this.origin[1], pos[2] - this.origin[2]]);
    }

    toWorld(key) {
        const p = parsePos(key);
        return [p[0] + this.origin[0], p[1] + this.origin[1], p[2] + this.origin[2]];
    }

    lamps() {
        return this.design.outputs.map((pin) => (this.blocks.get(pin.lamp) === LIT ? 1 : 0));
    }

    inputs() {
        return this.design.inputs.map((pin) => (this.blocks.get(pin.probe) === canonicalState(pin.on) ? 1 : 0));
    }

    /// How lever changes are paced, for the page: the slot a change waits for.
    leverInfo() {
        if (!this.design) return null;
        const seq = this.design.timing?.sequential;
        if (seq) return { clock: seq.clock, window: [seq.input_change_from, seq.input_change_by], at: seq.input_change_at };
        return { spacing: Math.max(this.design.timing?.input_spacing ?? 0, MIN_SPACING) };
    }

    /// The clock driver's record: where its edges landed against their schedule.
    clockInfo() {
        const c = this.clock;
        if (!c) return null;
        return {
            period: c.period,
            high: c.high,
            edges: c.edges,
            late: c.late,
            lost: c.lost,
            max_jitter: c.maxJitter,
            last_jitter: c.lastJitter,
            next_edge: this.nextEdgeTick(),
        };
    }

    info() {
        return {
            plot: this.index,
            name: this.name,
            state: this.state,
            stale: this.stale,
            tick: this.lastTick,
            placement: this.placedTick === null ? null : { tick: this.placedTick, matched: this.matched },
            lamps: this.design ? this.lamps() : [],
            inputs: this.design ? this.inputs() : [],
            viewers: this.viewers.size,
            size: this.design?.size ?? null,
            origin: this.origin,
            levers: this.leverInfo(),
            clock: this.clockInfo(),
            recorded: this.recorded,
            truncated: this.truncated,
        };
    }

    // --- placement

    /// Called before the place command goes out: the next changes inside the box are the placement.
    expectPlacement(design, structure) {
        this.design = design;
        this.structure = structure;
        this.state = 'placing';
        this.blocks = new Map();
        this.maxPending = 4 * design.inputs.length + 4;
    }

    /// Resolves with the placement tick once the stream shows the design placed.
    placed(timeout) {
        if (this.placedTick !== null) return Promise.resolve(this.placedTick);
        return new Promise((resolve, reject) => {
            const timer = setTimeout(() => reject(new PlotError('the server did not show the design placed in time', 500)), timeout);
            this.placementWaiters.push({ resolve, reject, timer });
        });
    }

    becameLive(timeout) {
        if (this.state === 'live') return Promise.resolve();
        return new Promise((resolve, reject) => {
            const timer = setTimeout(() => reject(new PlotError('the design did not settle in time', 500)), timeout);
            this.liveWaiters.push({ resolve, reject, timer });
        });
    }

    // --- the stream

    /// The stream reported `changes` (design coordinates, canonical states) with tick `tick`.
    onBatch(tick, changes) {
        this.lastTick = tick;
        this.lastTickAt = Date.now();
        if (this.state === 'placing') {
            if (!changes.length) return;
            for (const [key, state] of changes) {
                if (state === AIR) this.blocks.delete(key);
                else this.blocks.set(key, state);
            }
            this.placedTick = tick;
            const expected = Object.entries(this.design.blocks);
            this.matched = this.blocks.size === expected.length && expected.every(([key, state]) => this.blocks.get(key) === canonicalState(state));
            if (!this.matched) this.log(`plot ${this.index}: the stream shows the design placed differently from the design file`);
            this.state = 'settling';
            for (const w of this.placementWaiters.splice(0)) {
                clearTimeout(w.timer);
                w.resolve(tick);
            }
            this.broadcast({ state: this.state, tick });
            return;
        }
        if (this.state !== 'settling' && this.state !== 'live') return;
        const t = tick - this.placedTick;
        const real = [];
        for (const [key, state] of changes) {
            const before = this.blocks.get(key) ?? AIR;
            if (before === state) continue;
            if (state === AIR) this.blocks.delete(key);
            else this.blocks.set(key, state);
            real.push([key, state]);
            const pending = this.pendingLevers.findIndex((p) => p.key === key && p.state === state);
            if (pending >= 0) {
                const [entry] = this.pendingLevers.splice(pending, 1);
                if (!this.events.has(t)) this.events.set(t, new Map());
                this.events.get(t).set(key, state);
                if (entry.clock) this.edgeLanded(entry.clock, tick);
            }
        }
        // Waits are measured by the wall clock, not the stream's ticks: when the backend
        // stalls, the queued tick messages fly by in a burst while the stream's answer to a
        // command sent during the burst is still on its way.
        while (this.pendingLevers.length && Date.now() - this.pendingLevers[0].sentAt > PENDING_MS) {
            const old = this.pendingLevers.shift();
            this.log(`plot ${this.index}: the stream never showed ${old.key} become ${old.state}; forgotten`);
        }
        if (real.length && t >= 1) {
            if (this.recorded + real.length > this.maxRecordedChanges) {
                this.truncated = true;
            } else {
                if (!this.ticks.has(t)) this.ticks.set(t, new Map());
                const entry = this.ticks.get(t);
                for (const [key, state] of real) entry.set(key, state);
                this.recorded += real.length;
            }
        }
        if (this.state === 'settling' && tick >= this.placedTick + this.design.settle_ticks) {
            this.state = 'live';
            for (const w of this.liveWaiters.splice(0)) {
                clearTimeout(w.timer);
                w.resolve();
            }
            this.broadcast({ state: 'live', tick, levers: this.leverInfo() });
            this.startClock();
        }
        if (this.state === 'live') {
            if (this.clock && tick >= this.clock.nextTick) this.fireClock(tick);
            this.applyWanted(tick);
        }
        this.broadcast({ tick, changes: real, lamps: this.lamps(), inputs: this.inputs() });
    }

    // --- levers

    /// Asks for an input to be set. Resolves with the tick the change goes out
    /// at and whether it had to wait for its slot; the change itself shows up
    /// in the stream. A newer request for the same input replaces a waiting one.
    async setLever(name, on) {
        if (this.state !== 'live') throw new PlotError(this.state === 'settling' ? 'the design is still settling' : 'the plot is not live');
        const pin = this.design.inputs.find((p) => p.name === name);
        if (!pin) throw new PlotError(`no input named ${JSON.stringify(name)}`);
        const seq = this.design.timing?.sequential;
        if (seq && seq.clock === name) throw new PlotError('the server drives the clock input');
        const plan = this.leverPlan(pin);
        if (plan.now) {
            this.wanted.delete(pin.name);
            await this.drive(pin, on);
            return { tick: this.lastTick, queued: false };
        }
        this.wanted.set(pin.name, { pin, on, ...plan });
        return { tick: plan.tick, queued: true };
    }

    /// When a change of `pin` may go out: now, or the slot to wait for.
    leverPlan(pin) {
        const seq = this.design.timing?.sequential;
        const c = this.clock;
        if (seq && c) {
            const offset = seq.input_change_at;
            if (c.edges > 0 && this.lastTick <= c.edgeTick + offset) {
                // One change per input per window: a second request in the window waits for the next edge's.
                if (this.lastTick === c.edgeTick + offset && this.applied.get(pin.probe) !== this.lastTick) return { now: true };
                if (this.lastTick < c.edgeTick + offset) return { now: false, afterEdge: c.edges, offset, tick: c.edgeTick + offset };
            }
            return { now: false, afterEdge: c.edges + 1, offset, tick: this.nextEdgeTick() + offset };
        }
        const spacing = Math.max(this.design.timing?.input_spacing ?? 0, MIN_SPACING);
        const last = this.applied.get(pin.probe);
        if (last === undefined || this.lastTick >= last + spacing) return { now: true };
        return { now: false, at: last + spacing, tick: last + spacing };
    }

    /// Sends the waiting changes whose slot has come, at the end of tick `tick`.
    applyWanted(tick) {
        for (const [name, w] of this.wanted) {
            const due = w.at !== undefined
                ? tick >= w.at
                : this.clock !== null && this.clock.edges >= w.afterEdge && tick >= this.clock.edgeTick + w.offset;
            if (!due) continue;
            this.wanted.delete(name);
            this.drive(w.pin, w.on).catch((err) => this.log(`plot ${this.index}: lever ${name}: ${err.message}`));
        }
    }

    /// Writes a probe block, unless it already is (or is about to be) in that state.
    async drive(pin, on, { lane = 'levers', clock = null } = {}) {
        const state = canonicalState(on ? pin.on : pin.off);
        const pending = this.pendingLevers.filter((p) => p.key === pin.probe).at(-1);
        const current = pending ? pending.state : (this.blocks.get(pin.probe) ?? AIR);
        if (current === state) return false;
        const entry = { key: pin.probe, state, since: this.lastTick, sentAt: Date.now(), clock };
        this.pendingLevers.push(entry);
        // Past the bound the oldest lever change is forgotten, never the clock flip in flight.
        while (this.pendingLevers.length > this.maxPending) {
            const oldest = this.pendingLevers.findIndex((p) => !p.clock);
            if (oldest < 0) break;
            this.pendingLevers.splice(oldest, 1);
        }
        this.applied.set(pin.probe, this.lastTick);
        try {
            await this.world.setblock(this.toWorld(pin.probe), state, { lane });
        } catch (err) {
            this.pendingLevers = this.pendingLevers.filter((p) => p !== entry);
            throw new PlotError(`the server refused the change: ${err.message}`, 500);
        }
        return true;
    }

    // --- the clock of a clocked design

    // The stream reports the end of every tick; an edge due at tick T goes out when
    // tick T's message arrives, so the command lands between T and T+1, which the
    // trace format counts as the last thing in tick T. The stream then shows the
    // probe change under tick T: where it actually landed. The next edge is
    // timed from there, so a late edge stretches a half-period rather than
    // shortening the next one, and every edge's jitter is kept for /api/plots/N.
    // One flip is in flight at a time: while the backend is behind (a stall, a
    // burst of queued ticks), no further flip goes out until the last one shows.
    startClock() {
        const seq = this.design.timing?.sequential;
        if (!seq) return;
        const pin = this.design.inputs.find((p) => p.name === seq.clock);
        const activeOn = seq.active === 'rising';
        this.clock = {
            pin,
            period: seq.period,
            high: seq.high,
            activeOn,
            next: activeOn, // the level the next flip drives: the first flip is the active edge
            nextTick: this.lastTick + 2,
            edges: 0,
            edgeTick: null,
            inFlight: null,
            late: 0,
            lost: 0,
            maxJitter: 0,
            lastJitter: 0,
        };
    }

    /// The tick the next active edge is due at.
    nextEdgeTick() {
        const c = this.clock;
        return c.next === c.activeOn ? c.nextTick : c.nextTick + (c.period - c.high);
    }

    fireClock(tick) {
        const clock = this.clock;
        if (!clock || this.state !== 'live') return;
        if (clock.inFlight) {
            if (Date.now() - clock.inFlight.sentAt <= clock.period * 50 + LOST_MS) return; // the last flip has not shown up yet
            this.log(`plot ${this.index}: the clock flip scheduled at tick ${clock.inFlight.scheduled} never showed up in the stream`);
            clock.lost += 1;
            clock.inFlight = null;
        }
        const on = clock.next;
        const active = on === clock.activeOn;
        clock.next = !on;
        clock.nextTick = tick + (active ? clock.high : clock.period - clock.high);
        if (active) {
            clock.edges += 1;
            clock.edgeTick = tick;
        }
        const flight = { scheduled: tick, active, sentAt: Date.now() };
        clock.inFlight = flight;
        this.drive(clock.pin, on, { lane: 'clock', clock: flight }).then((written) => {
            if (!written) this.edgeLanded(flight, tick); // the probe already was in that state
        }, (err) => {
            this.log(`plot ${this.index}: clock: ${err.message}`);
            if (clock.inFlight !== flight) return;
            clock.inFlight = null;
            clock.next = on;
            clock.nextTick = this.lastTick + 1;
            if (active) clock.edges -= 1;
        });
    }

    /// The stream showed the flip `flight` under tick `tick`: the schedule continues from there.
    edgeLanded(flight, tick) {
        const clock = this.clock;
        if (!clock || clock.inFlight !== flight) return;
        clock.inFlight = null;
        const jitter = tick - flight.scheduled;
        clock.lastJitter = jitter;
        clock.maxJitter = Math.max(clock.maxJitter, Math.abs(jitter));
        if (jitter !== 0) clock.late += 1;
        clock.nextTick = tick + (flight.active ? clock.high : clock.period - clock.high);
        if (flight.active) clock.edgeTick = tick;
    }

    stopClock() {
        this.clock = null;
    }

    // --- viewers

    broadcast(message) {
        const text = JSON.stringify(message);
        for (const viewer of this.viewers) viewer.send(text);
    }

    hello() {
        return {
            hello: 'redfab-live',
            plot: this.index,
            name: this.name,
            state: this.state,
            tick: this.lastTick,
            lamps: this.design ? this.lamps() : [],
            inputs: this.design ? this.inputs() : [],
            levers: this.leverInfo(),
        };
    }

    // --- the recording

    /// The fixture and trace of everything since placement, in the trace format.
    recording(source) {
        if (this.placedTick === null) throw new PlotError('the design has not been placed yet', 409);
        if (this.stale) throw new PlotError('the stream dropped during the recording; it is incomplete', 409);
        if (this.truncated) throw new PlotError(`the recording passed ${this.maxRecordedChanges} changes and stopped; it cannot be exported`, 409);
        const ticks = [...this.ticks.keys()].sort((a, b) => a - b).map((t) => ({ tick: t, changes: Object.fromEntries([...this.ticks.get(t)].sort()) }));
        const events = [...this.events.keys()].sort((a, b) => a - b).map((t) => ({ tick: t, set: Object.fromEntries([...this.events.get(t)].sort()) }));
        const fixture = {
            format: FIXTURE_FORMAT,
            name: this.name,
            doc: 'recorded live on the server',
            size: this.design.size,
            placement: 'strict',
            blocks: this.design.blocks,
            events,
            ticks: Math.max(0, this.lastTick - this.placedTick),
            expect: [],
        };
        const trace = {
            format: TRACE_FORMAT,
            fixture: this.name,
            source,
            size: this.design.size,
            initial: this.design.blocks,
            ticks,
        };
        return { fixture, trace };
    }

    // --- release

    clearTimers() {
        this.stopClock();
        clearTimeout(this.idleTimer);
        clearTimeout(this.lifeTimer);
        for (const w of this.placementWaiters.splice(0)) {
            clearTimeout(w.timer);
            w.reject(new PlotError('the plot was released', 409));
        }
        for (const w of this.liveWaiters.splice(0)) {
            clearTimeout(w.timer);
            w.reject(new PlotError('the plot was released', 409));
        }
    }
}

export class Plots {
    constructor({ world, count, plotSize, plotY, idleSeconds, maxSeconds, freshSeconds = 60, clearRetrySeconds = 15, maxRecordedChanges = MAX_RECORDED_CHANGES, log }) {
        this.world = world;
        this.plotSize = plotSize.map((s) => Math.ceil(s / 16) * 16);
        this.idleSeconds = idleSeconds;
        this.maxSeconds = maxSeconds;
        this.freshSeconds = freshSeconds;
        this.clearRetrySeconds = clearRetrySeconds;
        this.log = log;
        this.placements = 0;
        const pitch = this.plotSize[0] + 16;
        this.plots = Array.from({ length: count }, (_, i) => new Plot(i, [i * pitch, plotY, 0], world, log, maxRecordedChanges));
        world.onBatch(({ tick, changes }) => this.onBatch(tick, changes));
        world.onStreamReconnect?.(() => this.streamReconnected());
    }

    get free() {
        return this.plots.filter((p) => p.state === 'free');
    }

    /// Plots whose clear failed; they are retried and come back as free.
    get broken() {
        return this.plots.filter((p) => p.state === 'broken');
    }

    get(id) {
        const plot = this.plots[Number(id)];
        return plot && plot.state !== 'free' && plot.state !== 'broken' ? plot : null;
    }

    /// The plots an address holds (reserved, placed or live).
    heldBy(ip) {
        return this.plots.filter((p) => p.ip === ip && p.state !== 'free' && p.state !== 'broken');
    }

    fits(size) {
        return size[0] <= this.plotSize[0] && size[2] <= this.plotSize[1];
    }

    /// Takes a free plot for a submission, or throws.
    reserve(name, ip = null) {
        const plot = this.free[0];
        if (!plot) throw new PlotError('no free plot right now; try again later', 503);
        plot.reserve(name, ip);
        return plot;
    }

    /// Places a compiled design on a reserved plot and waits until the stream shows it.
    async place(plot, { design, structure }) {
        if (!this.fits(design.size)) {
            throw new PlotError(`the design is ${design.size[0]} by ${design.size[2]} blocks; a plot is ${this.plotSize[0]} by ${this.plotSize[1]}`);
        }
        this.placements += 1;
        const name = `plot${plot.index}-${this.placements}`;
        plot.expectPlacement(design, structure);
        const [[x0, , z0], [x1, , z1]] = plot.box();
        await this.world.forceload([[x0 - 16, 0, z0 - 16], [x1 + 16, 0, z1 + 16]]);
        const showed = plot.placed(30_000);
        // When the place command itself fails, the release that follows rejects
        // this waiter with nobody left to await it; that is not an unhandled
        // rejection. `await showed` below still throws when it is the failure.
        showed.catch(() => {});
        await this.world.place({ name, structure, design, origin: plot.origin });
        await showed;
        // The template has done its job: the game forgets it and the file goes.
        Promise.resolve(this.world.forget?.(name)).catch((err) => this.log(`plot ${plot.index}: forgetting the template: ${err.message}`));
        const generation = plot.generation;
        plot.lifeTimer = setTimeout(() => {
            if (plot.generation === generation && plot.state !== 'free') this.release(plot, 'its time was up');
        }, this.maxSeconds * 1000);
        return plot;
    }

    /// Gives a reserved plot back without clearing (nothing was placed).
    unreserve(plot) {
        plot.clearTimers();
        plot.reset();
        plot.state = 'free';
    }

    /// Frees a plot: stops its clock, drops its viewers, clears its blocks. A plot
    /// whose clear fails is broken, not free, until a retry clears it.
    async release(plot, why = 'released') {
        if (plot.state === 'free' || plot.state === 'releasing' || plot.state === 'broken') return;
        const box = plot.design ? plot.box() : null;
        const stale = plot.stale;
        plot.clearTimers();
        plot.state = 'releasing';
        plot.broadcast({ state: 'released', why, stale });
        for (const viewer of plot.viewers) viewer.close(1000, why);
        plot.viewers.clear();
        this.log(`plot ${plot.index}: ${why}`);
        plot.reset();
        if (box) {
            try {
                await this.world.clear(box);
            } catch (err) {
                this.log(`plot ${plot.index}: clearing failed: ${err.message}; the plot is out of service until a retry clears it`);
                plot.state = 'broken';
                plot.brokenBox = box;
                this.retryClear(plot);
                return;
            }
        }
        plot.state = 'free';
    }

    retryClear(plot) {
        clearTimeout(plot.retryTimer);
        plot.retryTimer = setTimeout(async () => {
            if (plot.state !== 'broken') return;
            try {
                await this.world.clear(plot.brokenBox);
            } catch (err) {
                this.log(`plot ${plot.index}: clearing failed again: ${err.message}`);
                this.retryClear(plot);
                return;
            }
            this.log(`plot ${plot.index}: cleared on retry; back in service`);
            plot.brokenBox = null;
            plot.state = 'free';
        }, this.clearRetrySeconds * 1000);
    }

    /// A controller left; the plot goes after a while with none. A plot nobody
    /// has controlled yet goes sooner: its submitter never connected.
    controllerLeft(plot, { fresh = false } = {}) {
        if (plot.state === 'free' || plot.state === 'releasing' || plot.state === 'broken') return;
        clearTimeout(plot.idleTimer);
        const generation = plot.generation;
        const seconds = fresh ? this.freshSeconds : this.idleSeconds;
        plot.idleTimer = setTimeout(() => {
            if (plot.generation === generation && ![...plot.viewers].some((v) => v.controls)) {
                this.release(plot, fresh ? 'nobody connected to control it' : 'nobody was controlling it');
            }
        }, seconds * 1000);
    }

    controllerJoined(plot) {
        clearTimeout(plot.idleTimer);
    }

    /// The stream dropped and came back: what the server did meanwhile is unknown,
    /// so every plot's blocks and recording are stale and the plots are released.
    streamReconnected() {
        for (const plot of this.plots) {
            if (plot.design && plot.state !== 'free' && plot.state !== 'broken' && plot.state !== 'releasing') {
                plot.stale = true;
                this.release(plot, 'the observer stream dropped and reconnected; what the server did meanwhile was lost, so the plot is stale');
            }
        }
    }

    onBatch(tick, changes) {
        const active = this.plots.filter((p) => p.design && (p.state === 'placing' || p.state === 'settling' || p.state === 'live'));
        if (!active.length) return;
        const perPlot = active.map(() => []);
        for (const [key, state] of changes) {
            const pos = parsePos(key);
            for (const [i, plot] of active.entries()) {
                if (plot.inside(pos)) {
                    perPlot[i].push([plot.toDesign(pos), canonicalState(state)]);
                    break;
                }
            }
        }
        for (const [i, plot] of active.entries()) plot.onBatch(tick, perPlot[i]);
    }

    async closeAll() {
        for (const plot of this.plots) {
            clearTimeout(plot.retryTimer);
            if (plot.state === 'broken') {
                plot.state = 'free';
                plot.brokenBox = null;
            } else if (plot.state !== 'free') {
                await this.release(plot, 'the server is stopping');
            }
        }
    }
}
