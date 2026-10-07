// Plots: where submissions live in the world. Each plot holds one design: its
// origin, the state of its blocks as the stream reports them, the levers the
// controller has asked for, a recording of everything in the trace format, the
// clock driver for a clocked design, and the viewers watching it.
//
// Positions inside a plot are in design coordinates ("x,y,z" strings, as in the
// design file); the world position is the origin plus that. Tick numbers in the
// recording are relative to the tick the design was placed between.

import { randomBytes } from 'node:crypto';

const AIR = 'minecraft:air';
const LIT = 'minecraft:redstone_lamp[lit=true]';
const MAX_RECORDED_CHANGES = 2_000_000;

export class PlotError extends Error {
    constructor(message, status = 400) {
        super(message);
        this.status = status;
    }
}

const parse = (key) => key.split(',').map(Number);
const join = (p) => `${p[0]},${p[1]},${p[2]}`;

export class Plot {
    constructor(index, origin, world, log) {
        this.index = index;
        this.origin = origin;
        this.world = world;
        this.log = log;
        this.state = 'free';
        this.viewers = new Set();
        this.generation = 0;
        this.reset();
    }

    reset() {
        this.token = null;
        this.design = null;
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
        this.pendingLevers = [];
        this.clock = null;
        this.idleTimer = null;
        this.lifeTimer = null;
        this.placementWaiters = [];
        this.liveWaiters = [];
    }

    /// Holds the plot for a submission while it compiles.
    reserve(name) {
        this.state = 'reserved';
        this.name = name;
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
        return join([pos[0] - this.origin[0], pos[1] - this.origin[1], pos[2] - this.origin[2]]);
    }

    toWorld(key) {
        const p = parse(key);
        return [p[0] + this.origin[0], p[1] + this.origin[1], p[2] + this.origin[2]];
    }

    lamps() {
        return this.design.outputs.map((pin) => (this.blocks.get(pin.lamp) === LIT ? 1 : 0));
    }

    inputs() {
        return this.design.inputs.map((pin) => (this.blocks.get(pin.probe) === pin.on ? 1 : 0));
    }

    info() {
        return {
            plot: this.index,
            name: this.name,
            state: this.state,
            tick: this.lastTick,
            placement: this.placedTick === null ? null : { tick: this.placedTick, matched: this.matched },
            lamps: this.design ? this.lamps() : [],
            inputs: this.design ? this.inputs() : [],
            viewers: this.viewers.size,
            size: this.design?.size ?? null,
            origin: this.origin,
        };
    }

    // --- placement

    /// Called before the place command goes out: the next changes inside the box are the placement.
    expectPlacement(design) {
        this.design = design;
        this.state = 'placing';
        this.blocks = new Map();
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

    /// The stream reported `changes` (design coordinates) with tick `tick`.
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
            this.matched = this.blocks.size === Object.keys(this.design.blocks).length
                && Object.entries(this.design.blocks).every(([key, state]) => this.blocks.get(key) === state);
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
                this.pendingLevers.splice(pending, 1);
                if (!this.events.has(t)) this.events.set(t, new Map());
                this.events.get(t).set(key, state);
            }
        }
        if (real.length && t >= 1) {
            if (this.recorded + real.length > MAX_RECORDED_CHANGES) {
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
            this.broadcast({ state: 'live', tick });
            this.startClock();
        }
        if (this.clock && this.state === 'live' && tick >= this.clock.nextTick) this.fireClock();
        this.broadcast({ tick, changes: real, lamps: this.lamps(), inputs: this.inputs() });
    }

    // --- levers

    /// Sets an input by name. Resolves once the command is out; the change shows up in the stream.
    async setLever(name, on) {
        if (this.state !== 'live') throw new PlotError(this.state === 'settling' ? 'the design is still settling' : 'the plot is not live');
        const index = this.design.inputs.findIndex((pin) => pin.name === name);
        if (index < 0) throw new PlotError(`no input named ${JSON.stringify(name)}`);
        const seq = this.design.timing?.sequential;
        if (seq && seq.clock === name) throw new PlotError('the server drives the clock input');
        return this.drive(this.design.inputs[index], on);
    }

    async drive(pin, on) {
        const state = on ? pin.on : pin.off;
        const pending = this.pendingLevers.filter((p) => p.key === pin.probe).at(-1);
        const current = pending ? pending.state : (this.blocks.get(pin.probe) ?? AIR);
        if (current === state) return false;
        this.pendingLevers.push({ key: pin.probe, state, since: this.lastTick });
        try {
            await this.world.setblock(this.toWorld(pin.probe), state);
        } catch (err) {
            this.pendingLevers = this.pendingLevers.filter((p) => !(p.key === pin.probe && p.state === state));
            throw new PlotError(`the server refused the change: ${err.message}`, 500);
        }
        return true;
    }

    // --- the clock of a clocked design

    // The stream reports the end of every tick; an edge due at tick T goes out when
    // tick T's message arrives, so the command lands between T and T+1, which the
    // trace format counts as the last thing in tick T.
    startClock() {
        const seq = this.design.timing?.sequential;
        if (!seq) return;
        const pin = this.design.inputs.find((p) => p.name === seq.clock);
        const activeOn = seq.active === 'rising';
        this.clock = { pin, period: seq.period, high: seq.high, activeOn, nextTick: this.lastTick + 2, level: activeOn };
    }

    fireClock() {
        const clock = this.clock;
        if (!clock || this.state !== 'live') return;
        const level = clock.level;
        clock.nextTick += level === clock.activeOn ? clock.high : clock.period - clock.high;
        clock.level = !level;
        this.drive(clock.pin, level).catch((err) => this.log(`plot ${this.index}: clock: ${err.message}`));
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
        };
    }

    // --- the recording

    /// The fixture and trace of everything since placement, in the trace format.
    recording(source) {
        if (this.placedTick === null) throw new PlotError('the design has not been placed yet', 409);
        if (this.truncated) throw new PlotError('the recording is too long to export', 409);
        const ticks = [...this.ticks.keys()].sort((a, b) => a - b).map((t) => ({ tick: t, changes: Object.fromEntries([...this.ticks.get(t)].sort()) }));
        const events = [...this.events.keys()].sort((a, b) => a - b).map((t) => ({ tick: t, set: Object.fromEntries([...this.events.get(t)].sort()) }));
        const fixture = {
            format: 'redfab-fixture/1',
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
            format: 'redfab-trace/1',
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
    constructor({ world, count, plotSize, plotY, idleSeconds, maxSeconds, log }) {
        this.world = world;
        this.plotSize = plotSize.map((s) => Math.ceil(s / 16) * 16);
        this.idleSeconds = idleSeconds;
        this.maxSeconds = maxSeconds;
        this.log = log;
        this.placements = 0;
        const pitch = this.plotSize[0] + 16;
        this.plots = Array.from({ length: count }, (_, i) => new Plot(i, [i * pitch, plotY, 0], world, log));
        world.onBatch(({ tick, changes }) => this.onBatch(tick, changes));
    }

    get free() {
        return this.plots.filter((p) => p.state === 'free');
    }

    get(id) {
        const plot = this.plots[Number(id)];
        return plot && plot.state !== 'free' ? plot : null;
    }

    fits(design) {
        return design.size[0] <= this.plotSize[0] && design.size[2] <= this.plotSize[1];
    }

    /// Takes a free plot for a submission, or throws.
    reserve(name) {
        const plot = this.free[0];
        if (!plot) throw new PlotError('no free plot right now; try again later', 503);
        plot.reserve(name);
        return plot;
    }

    /// Places a compiled design on a reserved plot and waits until the stream shows it.
    async place(plot, { design, structure }) {
        if (!this.fits(design)) {
            throw new PlotError(`the design is ${design.size[0]} by ${design.size[2]} blocks; a plot is ${this.plotSize[0]} by ${this.plotSize[1]}`);
        }
        this.placements += 1;
        const name = `plot${plot.index}-${this.placements}`;
        plot.expectPlacement(design);
        const [[x0, , z0], [x1, , z1]] = plot.box();
        await this.world.forceload([[x0 - 16, 0, z0 - 16], [x1 + 16, 0, z1 + 16]]);
        const showed = plot.placed(30_000);
        await this.world.place({ name, structure, design, origin: plot.origin });
        await showed;
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

    /// Frees a plot: stops its clock, drops its viewers, clears its blocks.
    async release(plot, why = 'released') {
        if (plot.state === 'free' || plot.state === 'releasing') return;
        const box = plot.design ? plot.box() : null;
        plot.clearTimers();
        plot.state = 'releasing';
        plot.broadcast({ state: 'released', why });
        for (const viewer of plot.viewers) viewer.close(1000, why);
        plot.viewers.clear();
        this.log(`plot ${plot.index}: ${why}`);
        if (box) {
            try {
                await this.world.clear(box);
            } catch (err) {
                this.log(`plot ${plot.index}: clearing failed: ${err.message}`);
            }
        }
        plot.reset();
        plot.state = 'free';
    }

    /// A controller left; the plot goes after a while with none. A plot nobody
    /// has controlled yet gets at least a short grace period to be connected to.
    controllerLeft(plot, { fresh = false } = {}) {
        if (plot.state === 'free' || plot.state === 'releasing') return;
        clearTimeout(plot.idleTimer);
        const generation = plot.generation;
        const seconds = fresh ? Math.max(this.idleSeconds, 30) : this.idleSeconds;
        plot.idleTimer = setTimeout(() => {
            if (plot.generation === generation && ![...plot.viewers].some((v) => v.controls)) this.release(plot, 'nobody was controlling it');
        }, seconds * 1000);
    }

    controllerJoined(plot) {
        clearTimeout(plot.idleTimer);
    }

    onBatch(tick, changes) {
        const active = this.plots.filter((p) => p.design && (p.state === 'placing' || p.state === 'settling' || p.state === 'live'));
        if (!active.length) return;
        const perPlot = active.map(() => []);
        for (const [key, state] of changes) {
            const pos = parse(key);
            for (const [i, plot] of active.entries()) {
                if (plot.inside(pos)) {
                    perPlot[i].push([plot.toDesign(pos), state]);
                    break;
                }
            }
        }
        for (const [i, plot] of active.entries()) plot.onBatch(tick, perPlot[i]);
    }

    async closeAll() {
        for (const plot of this.plots) {
            if (plot.state !== 'free') await this.release(plot, 'the server is stopping');
        }
    }
}
