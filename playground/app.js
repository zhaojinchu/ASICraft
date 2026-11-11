// The page: the circuit view with its levers and lamps, a status line, the
// editor, and the result. The worker compiles and simulates at the game's 20
// ticks a second. The page opens with an example the server built already (the
// counter, counting), before the compiler is fetched. Served by the live backend
// (server/), the page can also send the source there: the server builds it,
// places it on the Minecraft server, and the view then shows the server's
// blocks instead.

import { View } from './view.js';
import { createCameraPanel } from './camera.js';
import { EXAMPLES, LIMITS, detectTop, nameFor } from './compile.js';
// The dropdown entry for the user's own source, kept in this browser.
const CUSTOM = 'custom';
const DRAFT_KEY = 'redfab.draft';
// The example the page opens with, as the server built it (tools/playground_serve.mjs, OPENING).
const OPENING = 'counter4';

const TICKS_PER_SECOND = 20;

const el = (id) => document.getElementById(id);
const source = el('source');
const topField = el('top');
const status = el('status');
const log = el('log');
const logDetails = el('log-details');
const reportText = el('report-text');
const modeBadge = el('mode');
const inputsPanel = el('inputs');
const outputsPanel = el('outputs');
const tickLabel = el('tick');
const compileButton = el('compile');
const runButton = el('run');
const downloadButton = el('download');
const reportButton = el('report');
const designButton = el('design-json');
const liveButton = el('live');
const releaseButton = el('release');
const clockModes = el('clock-mode');
const clockAutoButton = el('clock-auto');
const clockManualButton = el('clock-manual');
const examples = el('examples');

const view = new View(el('view'));
// The camera: the game itself, streamed; its tab is shown only when the backend has one.
const camera = createCameraPanel({ panel: el('camera'), video: el('camera-video'), status: el('camera-status'), auto: el('camera-auto'), manual: el('camera-manual'), turn: el('camera-turn') });
const tabs = el('tabs');
const tabButtons = { circuit: el('tab-circuit'), camera: el('tab-camera') };
const stagePanels = { circuit: el('view'), camera: el('camera') };
let tab = 'circuit';

/// Shows the circuit or the camera on the stage; the other stops drawing or streaming.
function selectTab(name) {
    if (name === 'camera' && !camera.available) name = 'circuit';
    tab = name;
    for (const [key, button] of Object.entries(tabButtons)) button.setAttribute('aria-selected', String(key === name));
    for (const [key, panel] of Object.entries(stagePanels)) panel.hidden = key !== name;
    view.setVisible(name === 'circuit');
    camera.show(name === 'camera');
}
tabButtons.circuit.addEventListener('click', () => selectTab('circuit'));
tabButtons.camera.addEventListener('click', () => selectTab('camera'));

// The limits the page enforces or shows; the live backend adds its plot size.
const limits = { ...LIMITS, plotSize: null };

let worker = null;
let nextId = 1;
const pending = new Map();
let design = null;
let inputState = [];
// Clicks on a clocked design's levers wait for the cycle's input window
// (index -> wanted level), as the server holds them in live mode.
let pendingInputs = new Map();
// The clock of a clocked design in the browser's simulation: 'auto' (the page
// drives it at the reported period) or 'manual' (the visitor flips the clk
// lever). The automatic clock's periods count from clockOrigin; the manual
// clock keeps its last edges and the last input change, since a flip must
// wait for the design's own timing: the pulse's `high` ticks, the period, and
// the inputs' setup before an active edge. A flip that comes too soon waits,
// dashed, and lands the tick it is safe.
let clockMode = 'auto';
let clockOrigin = 0;
let edges = { active: -Infinity, inactive: -Infinity, input: -Infinity };
let pendingClock = null; // the level a flip that came too soon will give the clock
let running = false;
let tick = 0;
let timer = null;
// The step in flight, so a hand-driven step can wait for it instead of being dropped.
let stepping = null;
// The last successful build: the worker holds its structure (or, for the
// opening circuit, the server does under `files`), and its footprint is known
// before a live submission.
let built = null;
// 'sim': the worker's simulator drives the view. 'live': the server's stream does.
let mode = 'sim';
let livePlot = null;
let liveSocket = null;
let liveTick = 0;
// What the examples dropdown last put in the editor, to tell an edit from it.
let loaded = { example: null, text: '' };
let lastPick = null;
// The opening circuit: 'loading' until it shows ('shown') or cannot ('none').
let opening = 'loading';
// Counts the compiles and live runs, so an opening still loading stands down.
let generation = 0;
// The bus rows on show: bus name -> its pins' indices, most significant bit first.
let buses = { inputs: new Map(), outputs: new Map() };

function setStatus(text, kind = '') {
    status.textContent = text;
    status.className = kind;
}

/// The badge on the view: what drives what is shown, or nothing when the view is empty.
function setMode(text, live = false) {
    modeBadge.hidden = text === null;
    modeBadge.textContent = text ?? '';
    modeBadge.classList.toggle('live', live);
}

/// Starts (or restarts) the worker; whatever was waiting fails with `reason`.
function startWorker(reason = 'the worker was restarted') {
    if (worker) worker.terminate();
    warmed = false; // a new worker has no compiler loaded
    worker = new Worker('./worker.js', { type: 'module' });
    worker.onmessage = ({ data }) => {
        const waiting = pending.get(data.id);
        if (!waiting) return;
        if ('log' in data) {
            log.textContent += data.log;
            log.scrollTop = log.scrollHeight;
            return;
        }
        if ('phase' in data) {
            waiting.phase?.(data.phase);
            return;
        }
        pending.delete(data.id);
        if (data.ok) waiting.resolve(data);
        else waiting.reject(new Error(data.error));
    };
    worker.onerror = (e) => {
        // A crashed worker answers nothing; fail what waits rather than leave it hanging.
        const message = `the worker failed: ${e.message || 'unknown error'}`;
        setStatus(message, 'bad');
        for (const waiting of pending.values()) waiting.reject(new Error(message));
        pending.clear();
    };
    for (const waiting of pending.values()) waiting.reject(new Error(reason));
    pending.clear();
}

function ask(message, { phase } = {}) {
    const id = nextId++;
    return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject, phase });
        worker.postMessage({ id, ...message });
    });
}

// --- the source: examples, the draft, the top module

function readDraft() {
    try {
        const draft = JSON.parse(localStorage.getItem(DRAFT_KEY));
        return draft && typeof draft.source === 'string' ? draft : null;
    } catch {
        return null;
    }
}

function saveDraft() {
    if (examples.value !== CUSTOM) return;
    try {
        localStorage.setItem(DRAFT_KEY, JSON.stringify({ source: source.value, top: topField.value }));
    } catch {
        // private mode or storage full: the draft simply is not kept
    }
}

/// Whether the editor holds something other than what the dropdown loaded.
function edited() {
    return source.value.trim() !== '' && source.value !== loaded.text;
}

async function loadExample(name) {
    const reply = await fetch(`/designs/${name}/top.sv`);
    source.value = reply.ok ? await reply.text() : `// could not load ${name}`;
    loaded = { example: name, text: source.value };
    examples.value = name;
    lastPick = name;
}

/// The top module and the design name for the source in the editor.
function topAndName() {
    const top = topField.value.trim() || detectTop(source.value);
    const name = examples.value === CUSTOM || !examples.value ? nameFor(top) : examples.value;
    return { top, name };
}

// --- compiling, and the result

/// The cell count a report states.
function cellsOf(report) {
    return Number(report?.match(/cells: (\d+)/)?.[1] ?? 0);
}

/// The status after a build, as a build tool prints it: `lead` ("built NAME in
/// 3.2 s"), and a warning when the server's plot cannot take the design.
function describeBuilt({ design: d }, lead) {
    if (limits.plotSize && !fitsPlot(d)) {
        return { text: `${lead}, too large for the server's ${limits.plotSize[0]} x ${limits.plotSize[1]} plot`, kind: 'warn' };
    }
    return { text: lead, kind: 'ok' };
}

function fitsPlot(d) {
    return !limits.plotSize || (d.size[0] <= limits.plotSize[0] && d.size[2] <= limits.plotSize[1]);
}

const seconds = (ticks) => `${(ticks / 20).toFixed(2)} s`;

/// Pin names with each bus folded to name[msb:lsb], in the design's order.
function pinNames(pins) {
    const out = [];
    const buses = new Map();
    for (const pin of pins) {
        const b = busOf(pin.name);
        if (!b) {
            out.push(pin.name);
            continue;
        }
        if (!buses.has(b.bus)) {
            buses.set(b.bus, []);
            out.push(b.bus);
        }
        buses.get(b.bus).push(b.bit);
    }
    return out.map((name) => {
        const bits = buses.get(name);
        return bits ? `${name}[${Math.max(...bits)}:${Math.min(...bits)}]` : name;
    }).join(' ');
}

/// The report the page shows, in the terms of a timing report: area, the
/// propagation delay or the clock period, clock-to-output, slack, the
/// equivalence check. A tick is the game's, 50 ms. The core's full report, with
/// the placement, the lever and lamp positions and the /forceload line for a
/// world of your own, is the report.txt export.
function technicalReport(d, cells, report) {
    const rows = [];
    const row = (key, value) => rows.push(`${key.padEnd(12)} ${value}`);
    const seq = d.timing?.sequential;
    const edge = seq?.active === 'falling' ? '↓' : '↑';
    const head = seq ? `${d.name}, sequential, ${seq.clock} ${edge}` : `${d.name}, combinational`;
    if (cells) row('cells', cells);
    if (seq?.flip_flops?.length) row('flip-flops', seq.flip_flops.length);
    if (typeof d.repeaters === 'number') row('repeaters', d.repeaters);
    row('blocks', Object.keys(d.blocks).length);
    row('area', `${d.size[0]} x ${d.size[2]} x ${d.size[1]}`);
    if (seq) {
        row('T_clk', `${seq.period} ticks, ${seconds(seq.period)}`);
        row('t_co', `${seq.sample_at} ticks`);
        row('t_in', `${seq.input_change_from} to ${seq.input_change_by} ticks after ${edge}`);
        const ffs = seq.flip_flops ?? [];
        if (ffs.length) row('slack', `setup ${Math.min(...ffs.map((f) => f.setup_slack))}, hold ${Math.min(...ffs.map((f) => f.hold_slack))} ticks`);
    } else {
        const fastest = Math.min(...Object.values(d.timing?.outputs ?? {}).map((o) => o.min).filter((n) => typeof n === 'number'));
        row('t_pd', Number.isFinite(fastest) && fastest < d.settle_ticks ? `min ${fastest}, max ${d.settle_ticks} ticks, ${seconds(d.settle_ticks)}` : `${d.settle_ticks} ticks, ${seconds(d.settle_ticks)}`);
        if (d.timing?.input_spacing) row('t_in,min', `${d.timing.input_spacing} ticks`);
    }
    row('inputs', pinNames(d.inputs));
    row('outputs', pinNames(d.outputs));
    if (/proven equal to the source/.test(report ?? '')) row('LEC', 'proven');
    return [head, ...rows.map((r) => `  ${r}`)].join('\n');
}

/// The report, in the terminal.
function renderResult({ design: d, cells, report }) {
    reportText.textContent = technicalReport(d, cells, report);
}

function clearResult() {
    reportText.textContent = '';
    log.textContent = '';
    logDetails.open = false;
}

async function compile() {
    generation += 1;
    stopRunning();
    leaveLive();
    selectTab('circuit');
    design = null;
    built = null;
    view.clear();
    setMode(null);
    compileButton.disabled = true;
    runButton.disabled = true;
    enableDownloads(false);
    clearResult();
    clearPanels();
    let top;
    let name;
    try {
        ({ top, name } = topAndName());
    } catch (err) {
        setStatus(err.message, 'bad');
        compileButton.disabled = false;
        return;
    }
    const text = source.value;
    const started = Date.now();
    let phase = 'starting';
    const progress = () => setStatus(`compiling ${name}: ${phase}, ${Math.round((Date.now() - started) / 1000)} s`);
    progress();
    const ticker = setInterval(progress, 1000);
    const deadline = setTimeout(() => {
        startWorker(`stopped: compiling took longer than ${limits.compileSeconds} s, try a smaller design`);
    }, limits.compileSeconds * 1000);
    try {
        const reply = await ask({ op: 'compile', source: text, top, name }, { phase: (p) => { phase = p; progress(); } });
        design = reply.design;
        built = { source: text, top, name, design: reply.design, cells: reply.cells, report: reply.report };
        const { text: summaryText, kind } = describeBuilt(reply, `built ${name} in ${((Date.now() - started) / 1000).toFixed(1)} s`);
        setStatus(summaryText, kind);
        renderResult(built);
        showDesign();
    } catch (err) {
        setStatus(err.message.split('\n')[0], 'bad');
        log.textContent += `\n${err.message}\n`;
        logDetails.open = true;
    } finally {
        clearInterval(ticker);
        clearTimeout(deadline);
        compileButton.disabled = false;
    }
}

// --- the panel: levers, lamps, buses

function label(text) {
    const span = document.createElement('span');
    span.className = 'label';
    span.textContent = text;
    return span;
}

function clearPanels() {
    inputsPanel.replaceChildren(label('inputs'));
    outputsPanel.replaceChildren(label('outputs'));
    tickLabel.textContent = '';
    buses = { inputs: new Map(), outputs: new Map() };
    clockModes.hidden = true;
}

/// `name[3]` is bit 3 of the bus `name`; a plain name is a single pin.
function busOf(name) {
    const m = name.match(/^(.+)\[(\d+)\]$/);
    return m ? { bus: m[1], bit: Number(m[2]) } : null;
}

/// Fills a pins row: single pins as they come, the bits of a bus in one box,
/// most significant bit first, with the bus's value beside them. `make(i, pin,
/// inBus)` gives the lever or lamp for a pin. Returns the bus rows' pin
/// indices, most significant bit first.
function fillPins(container, pins, make) {
    const members = new Map();
    for (const [i, pin] of pins.entries()) {
        const b = busOf(pin.name);
        if (!b) continue;
        if (!members.has(b.bus)) members.set(b.bus, []);
        members.get(b.bus).push({ i, bit: b.bit });
    }
    const rows = new Map();
    for (const [i, pin] of pins.entries()) {
        const b = busOf(pin.name);
        if (!b) {
            container.appendChild(make(i, pin, false));
            continue;
        }
        if (rows.has(b.bus)) continue;
        const bits = members.get(b.bus).sort((p, q) => q.bit - p.bit);
        const row = document.createElement('span');
        row.className = 'bus';
        row.dataset.bus = b.bus;
        const name = document.createElement('span');
        name.className = 'name';
        name.textContent = b.bus;
        row.appendChild(name);
        for (const m of bits) row.appendChild(make(m.i, pins[m.i], true));
        const value = document.createElement('span');
        value.className = 'value';
        row.appendChild(value);
        container.appendChild(row);
        rows.set(b.bus, bits.map((m) => m.i));
    }
    return rows;
}

/// Writes every bus's value from its levers or lamps: the bits, then the number.
function renderValues() {
    if (!design) return;
    const lit = (o) => outputsPanel.querySelector(`[data-output="${CSS.escape(design.outputs[o].name)}"]`).dataset.lit;
    const write = (container, rows, bit) => {
        for (const [bus, indices] of rows) {
            const bits = indices.map(bit).join('');
            container.querySelector(`.bus[data-bus="${CSS.escape(bus)}"] > .value`).textContent = `${bits} (${parseInt(bits, 2)})`;
        }
    };
    write(inputsPanel, buses.inputs, (i) => inputState[i]);
    write(outputsPanel, buses.outputs, lit);
}

/// Shows the lamps as the simulator or the server reports them.
function setLamps(lamps) {
    for (const [o, pin] of design.outputs.entries()) {
        outputsPanel.querySelector(`[data-output="${CSS.escape(pin.name)}"]`).dataset.lit = String(lamps[o]);
    }
    renderValues();
}

/// Shows the levers as the state says (the server's word in live mode).
function setLevers(states) {
    inputState = states.slice();
    for (const [i, pin] of design.inputs.entries()) {
        inputsPanel.querySelector(`[data-input="${CSS.escape(pin.name)}"]`).dataset.on = String(inputState[i]);
    }
    renderValues();
}

function showDesign() {
    view.show(design);
    inputState = design.inputs.map(() => 0);
    pendingInputs = new Map();
    tick = 0;
    const seq = design.timing?.sequential;
    buses.inputs = fillPins(inputsPanel, design.inputs, (i, pin, inBus) => {
        const button = document.createElement('button');
        button.className = 'lever';
        button.dataset.input = pin.name;
        button.dataset.on = '0';
        if (inBus) {
            button.title = pin.name;
            button.setAttribute('aria-label', pin.name);
        } else {
            button.textContent = pin.name; // text, never markup: escaped identifiers reach here
        }
        button.addEventListener('click', () => toggle(i));
        return button;
    });
    buses.outputs = fillPins(outputsPanel, design.outputs, (o, pin, inBus) => {
        const lamp = document.createElement('span');
        lamp.className = 'lamp';
        lamp.dataset.output = pin.name;
        lamp.dataset.lit = '0';
        const bulb = document.createElement('span');
        bulb.className = 'bulb';
        lamp.appendChild(bulb);
        if (inBus) lamp.title = pin.name;
        else lamp.append(pin.name);
        return lamp;
    });
    // Every design starts on the automatic clock.
    clockMode = 'auto';
    clockOrigin = 0;
    edges = { active: -Infinity, inactive: -Infinity, input: -Infinity };
    pendingClock = null;
    renderClock();
    renderValues();
    setMode(mode === 'live' ? `plot ${livePlot.plot}, settling` : 'simulated here', mode === 'live');
    runButton.disabled = mode === 'live';
    runButton.hidden = mode === 'live'; // the server runs it; there is nothing to pause
    // In live mode the structure comes from the matching local build, or from the server.
    enableDownloads(true);
    if (mode === 'sim') startRunning();
}

function enableDownloads(on) {
    for (const button of [downloadButton, reportButton, designButton]) button.disabled = !on;
}

async function toggle(i) {
    if (!design) return;
    const seq = design.timing?.sequential;
    if (seq && design.inputs[i].name === seq.clock) {
        // The visitor's own clock, in the browser's simulation only; the server drives its own.
        if (mode === 'sim' && clockMode === 'manual') await clockClick();
        return;
    }
    if (mode === 'live') {
        // The server flips the probe block; the lever shows the new state once the stream reports it.
        if (liveSocket?.readyState === WebSocket.OPEN) liveSocket.send(JSON.stringify({ set: design.inputs[i].name, on: inputState[i] !== 1 }));
        return;
    }
    // A clocked design's inputs move only in the cycle's input window, at
    // input_change_at after the edge (the report's rule, and what the server
    // does in live mode): a click at any other tick is held until then, and a
    // second click before that takes the request back.
    // With the manual clock the window is the hold time after the last active edge instead.
    const wanted = !(pendingInputs.has(i) ? pendingInputs.get(i) : inputState[i] === 1);
    if (!seq || inputWindowOpen(seq)) {
        pendingInputs.delete(i);
        await changeInput(i, wanted);
        return;
    }
    if (wanted === (inputState[i] === 1)) pendingInputs.delete(i);
    else pendingInputs.set(i, wanted);
    showPending();
}

/// Whether a clocked design's inputs may move at this tick: at the cycle's input
/// window with the automatic clock, or past the hold time after the last active
/// edge with the manual one.
function inputWindowOpen(seq) {
    if (clockMode === 'manual') return tick - edges.active >= seq.input_change_from;
    return tick >= clockOrigin && (tick - clockOrigin) % seq.period === seq.input_change_at;
}

/// An input change, remembered for the manual clock's setup rule.
async function changeInput(i, on) {
    edges.input = tick;
    await setInput(i, on);
}

/// Marks the levers whose click is waiting: an input for its window, the clock for its timing.
function showPending() {
    const seq = design?.timing?.sequential;
    for (const [i, pin] of design.inputs.entries()) {
        const button = inputsPanel.querySelector(`[data-input="${CSS.escape(pin.name)}"]`);
        if (seq && pin.name === seq.clock) {
            button.dataset.pending = pendingClock !== null ? '1' : '0';
            continue;
        }
        const held = pendingInputs.has(i);
        button.dataset.pending = held ? '1' : '0';
        const when = clockMode === 'manual' ? 'after the clock edge\'s hold time' : `at cycle tick ${seq?.input_change_at}, the next input window`;
        button.title = held ? `${pin.name} changes ${when}` : (button.textContent ? '' : pin.name);
    }
}

/// Applies the held clicks when the window opens.
async function applyPendingInputs() {
    const seq = design?.timing?.sequential;
    if (!seq || !pendingInputs.size || !inputWindowOpen(seq)) return;
    const due = [...pendingInputs];
    pendingInputs.clear();
    for (const [i, on] of due) await changeInput(i, on);
    showPending();
}

// --- the manual clock (the browser's simulation only)

const clockIndex = () => design.inputs.findIndex((p) => p.name === design.timing.sequential.clock);
const isActiveLevel = (level, seq) => level === (seq.active === 'rising');

/// Whether the clock may take `level` now: an active edge a full period after the
/// last, the rest of the period after the last release, and the inputs' setup
/// after their last change (the report's period less its input window); a release
/// `high` ticks after the active edge.
function edgeAllowed(level) {
    const seq = design.timing.sequential;
    if (isActiveLevel(level, seq)) {
        return tick - edges.active >= seq.period && tick - edges.inactive >= seq.period - seq.high && tick - edges.input >= seq.period - seq.input_change_by;
    }
    return tick - edges.active >= seq.high;
}

async function flipClock(level) {
    const seq = design.timing.sequential;
    if (isActiveLevel(level, seq)) edges.active = tick;
    else edges.inactive = tick;
    await setClock(level);
}

/// A click on the clock lever: flips it now if the timing allows, else at the first tick it does;
/// a second click before then takes the flip back.
async function clockClick() {
    const current = inputState[clockIndex()] === 1;
    const wanted = !(pendingClock !== null ? pendingClock : current);
    if (wanted === current) pendingClock = null;
    else if (edgeAllowed(wanted)) {
        pendingClock = null;
        await flipClock(wanted);
    } else pendingClock = wanted;
    showPending();
}

async function applyPendingClock() {
    if (pendingClock === null || !edgeAllowed(pendingClock)) return;
    const level = pendingClock;
    pendingClock = null;
    await flipClock(level);
    showPending();
}

/// Auto or manual. To manual, the automatic clock's last edges carry over so the first flip keeps
/// its timing; back to auto, the clock rests a full period from its release (held until the pulse
/// has had its `high` ticks), as at the start, before the first automatic edge.
async function setClockMode(next) {
    const seq = design?.timing?.sequential;
    if (!seq || mode !== 'sim' || next === clockMode) return;
    const clock = clockIndex();
    if (next === 'manual') {
        const t = tick - clockOrigin;
        if (t >= seq.period) {
            const phase = t % seq.period;
            edges.active = tick - phase;
            edges.inactive = phase >= seq.high ? edges.active + seq.high : edges.active - (seq.period - seq.high);
            edges.input = phase >= seq.input_change_at ? edges.active + seq.input_change_at : -Infinity;
        } else {
            edges = { active: -Infinity, inactive: Math.max(clockOrigin, 0), input: -Infinity };
        }
        clockMode = 'manual';
        renderClock();
        showPending();
        return;
    }
    pendingClock = null;
    const activeNow = isActiveLevel(inputState[clock] === 1, seq);
    const runningOut = activeNow && tick - edges.active < seq.high;
    // The pulse runs out, then the rest begins; or the rest begins now.
    clockOrigin = runningOut ? edges.active + seq.high : tick;
    clockMode = 'auto';
    renderClock(); // the buttons answer the click at once; the release below takes a round trip
    showPending();
    if (activeNow && !runningOut) await flipClock(seq.active !== 'rising'); // the resting level
}

/// The clock's controls: shown for a clocked design in the browser, the lever clickable in manual.
function renderClock() {
    const seq = design?.timing?.sequential;
    clockModes.hidden = !(seq && mode === 'sim');
    clockAutoButton.setAttribute('aria-pressed', String(clockMode === 'auto'));
    clockManualButton.setAttribute('aria-pressed', String(clockMode === 'manual'));
    if (!seq) return;
    const button = inputsPanel.querySelector(`[data-input="${CSS.escape(seq.clock)}"]`);
    if (!button) return;
    const manual = mode === 'sim' && clockMode === 'manual';
    button.disabled = !manual;
    button.title = manual
        ? `${seq.clock}: click to flip it; a flip too soon for the circuit waits, dashed`
        : `${seq.clock}: driven by the ${mode === 'live' ? 'server' : 'page'}, ${seq.period} ticks a cycle`;
}

/// Flips an input in the simulator and recolours what changed at once.
async function setInput(i, on) {
    inputState[i] = on ? 1 : 0;
    inputsPanel.querySelector(`[data-input="${CSS.escape(design.inputs[i].name)}"]`).dataset.on = String(inputState[i]);
    renderValues();
    const reply = await ask({ op: 'set', input: i, on });
    view.update(reply.changes);
}

async function setClock(on) {
    const seq = design?.timing?.sequential;
    if (!seq) return;
    await setInput(design.inputs.findIndex((p) => p.name === seq.clock), on);
}

/// The clock, counted in simulated ticks from clockOrigin: at rest through the
/// first period, then its active edge at the start of each period and the other
/// edge `high` ticks later, whatever the wall clock does. Before the origin (a
/// pulse of the manual clock running out) it is left alone; at the origin it rests.
async function clockEdges(at) {
    const seq = design.timing?.sequential;
    if (!seq) return;
    const activeHigh = seq.active === 'rising';
    const t = at - clockOrigin;
    if (t < 0) return;
    if (t === 0) {
        if (clockOrigin > 0 && isActiveLevel(inputState[clockIndex()] === 1, seq)) await flipClock(!activeHigh);
        return;
    }
    const phase = t % seq.period;
    if (phase === 0) await flipClock(activeHigh);
    else if (phase === seq.high) await flipClock(!activeHigh);
}

/// Steps the simulator; a step asked for while one is in flight is skipped
/// (the timer's way of stretching time on a slow machine, never shortening a
/// clock period), so a caller that must not lose its step waits first.
async function step(ticks) {
    if (stepping || !design) return;
    // Edges belong to the timer's steps: a Pause while one is in flight still
    // lands that step with its edge, and a hand-driven step never applies one.
    const auto = running;
    stepping = (async () => {
        const reply = await ask({ op: 'step', ticks });
        // The page may have moved on while the worker stepped (Run in the
        // game, a new compile): the reply belongs to a design that is gone.
        if (!design) return;
        tick = reply.tick;
        view.update(reply.changes);
        setLamps(reply.lamps);
        const seq = design.timing?.sequential;
        if (!seq) tickLabel.textContent = `tick ${tick}`;
        else if (clockMode === 'manual') tickLabel.textContent = `tick ${tick}, manual clock`;
        else tickLabel.textContent = `tick ${tick}, cycle tick ${(((tick - clockOrigin) % seq.period) + seq.period) % seq.period} of ${seq.period}`;
        if (auto && clockMode === 'auto') await clockEdges(tick);
        if (clockMode === 'manual') await applyPendingClock();
        await applyPendingInputs();
    })();
    try {
        await stepping;
    } finally {
        stepping = null;
    }
}

/// The level the clock has at `at`, by the page's own convention: at rest
/// through the first period (a pulse still in the clock tree flushes, as the
/// native cycle protocol has it), then active from each period's start for
/// `high` ticks.
function clockLevelAt(at, seq) {
    const t = at - clockOrigin;
    if (t < 0) return seq.active === 'rising'; // a manual pulse still running out: active
    const active = t >= seq.period && t % seq.period < seq.high;
    return active === (seq.active === 'rising');
}

function startRunning() {
    running = true;
    runButton.textContent = 'Pause';
    // The clock takes the level its phase implies, so Pause and Run inside a
    // pulse do not cut the pulse short. The manual clock is the visitor's.
    const seq = design?.timing?.sequential;
    if (seq && clockMode === 'auto') {
        const level = clockLevelAt(tick, seq);
        const clock = design.inputs.findIndex((p) => p.name === seq.clock);
        if (inputState[clock] !== (level ? 1 : 0)) setClock(level);
    }
    timer = setInterval(() => step(1), 1000 / TICKS_PER_SECOND);
}

function stopRunning() {
    running = false;
    runButton.textContent = 'Run';
    clearInterval(timer);
    timer = null;
}

// --- the opening circuit: built by the server from an example, shown at once

/// Shows the example the server built, running, before any compile. With a
/// draft in the editor the status says so: the view and the editor differ.
async function openWith(name, { draft }) {
    const g = generation;
    try {
        const [d, r] = await Promise.all([fetch(`/designs/${name}/design.json`), fetch(`/designs/${name}/report.txt`)]);
        if (!d.ok) throw new Error(`no opening circuit (${d.status})`);
        const text = await d.text();
        const report = r.ok ? await r.text() : null;
        if (g !== generation) return; // the visitor compiled or went live first
        const reply = await ask({ op: 'open', design: text });
        if (g !== generation) return;
        design = reply.design;
        const cells = cellsOf(report);
        built = { source: null, top: null, name, design, cells, report, designText: text, files: `/designs/${name}` };
        const { text: summaryText, kind } = describeBuilt(built, `simulating ${name} in the browser`);
        setStatus(`${summaryText}${draft ? ', your draft is in the editor' : ''}`, kind);
        renderResult(built);
        showDesign();
        // The counter counts only when enabled: on, so the circuit moves on its own.
        const en = design.inputs.findIndex((p) => p.name === 'en');
        if (en >= 0) await toggle(en);
        opening = 'shown';
    } catch (err) {
        if (g === generation) setStatus('pick an example or paste a module, then compile');
        log.textContent += `the opening circuit did not load: ${err.message}\n`;
    } finally {
        if (opening === 'loading') opening = 'none';
    }
}

// --- the live server

/// Sends the source to the server, which builds it and places it in the game.
async function goLive() {
    generation += 1;
    stopRunning();
    leaveLive();
    let top;
    let name;
    try {
        ({ top, name } = topAndName());
    } catch (err) {
        setStatus(err.message, 'bad');
        return;
    }
    // A design built here already tells its footprint; do not send what cannot fit.
    if (built && built.source === source.value && built.top === top && !fitsPlot(built.design)) {
        setStatus(`not sent: ${built.design.size[0]} x ${built.design.size[2]} blocks, the server's plots are ${limits.plotSize[0]} x ${limits.plotSize[1]}`, 'bad');
        return;
    }
    design = null;
    mode = 'live';
    compileButton.disabled = true;
    liveButton.disabled = true;
    runButton.disabled = true;
    enableDownloads(false);
    clearResult();
    clearPanels();
    view.clear();
    setMode('building on the server', true);
    setStatus('building on the server…');
    try {
        const reply = await fetch('/api/submit', {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ source: source.value, top, name }),
        });
        const body = await reply.json();
        if (!reply.ok) throw new Error(body.error || `the server answered ${reply.status}`);
        design = body.design;
        livePlot = { plot: body.plot, token: body.token, origin: body.origin, source: source.value, top, name, levers: body.levers, report: body.report };
        renderResult({ design: body.design, cells: cellsOf(body.report), report: body.report });
        setStatus(`placing on plot ${body.plot}…`);
        showDesign();
        releaseButton.hidden = false;
        connectLive();
        // In the game, the game is the picture: the camera's tab, when the backend has a camera.
        selectTab('camera');
    } catch (err) {
        mode = 'sim';
        setMode(null);
        setStatus(err.message.split('\n')[0], 'bad');
        log.textContent += `\n${err.message}\n`;
        logDetails.open = true;
    } finally {
        compileButton.disabled = false;
        liveButton.disabled = false;
    }
}

function connectLive() {
    const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
    liveSocket = new WebSocket(`${scheme}://${location.host}/live?plot=${livePlot.plot}&token=${livePlot.token}`);
    liveSocket.onmessage = ({ data }) => {
        const m = JSON.parse(data);
        if (!design) return;
        if (typeof m.tick === 'number') {
            liveTick = m.tick;
            tickLabel.textContent = `tick ${liveTick}`;
        }
        if (m.changes) view.update(m.changes);
        if (m.lamps) setLamps(m.lamps);
        if (m.inputs) setLevers(m.inputs);
        if (m.state === 'live') {
            setStatus(`live on plot ${livePlot.plot}`, 'ok');
            setMode(`live in the game, plot ${livePlot.plot}`, true);
        }
        // The camera's answers (taking it, a turn, its end) go to the panel.
        if (m.drive !== undefined || (m.ok === false && /driv|camera/.test(m.error ?? ''))) camera.receive(m);
        // A lever change asked for before its slot goes out at the tick the server names.
        if (m.ok === true && m.queued) setStatus(`lever ${m.set} moves at tick ${m.tick}`, 'ok');
        if (m.state === 'released') {
            const why = `the server released plot ${livePlot.plot}: ${m.why}`;
            leaveLive({ release: false });
            clearAfterLive();
            setStatus(why, 'bad');
        }
        if (m.ok === false) setStatus(m.error, 'bad');
    };
    liveSocket.onclose = () => {
        if (mode === 'live') setStatus('the connection to the server closed', 'bad');
    };
    const socket = liveSocket;
    camera.attach((message) => {
        if (socket.readyState !== WebSocket.OPEN) return false;
        socket.send(JSON.stringify(message));
        return true;
    }, livePlot.plot);
}

/// Leaves live mode; the plot goes back to the server unless it already took
/// it. Returns the release request so a caller can wait for it.
function leaveLive({ release = true } = {}) {
    if (liveSocket) {
        liveSocket.onclose = null;
        liveSocket.close();
        liveSocket = null;
    }
    const plot = livePlot;
    livePlot = null;
    mode = 'sim';
    releaseButton.hidden = true;
    runButton.hidden = false;
    camera.attach(null, null);
    if (plot && release) {
        // keepalive: the request completes even when the page is going away.
        return fetch(`/api/plots/${plot.plot}?token=${plot.token}`, { method: 'DELETE', keepalive: true }).catch(() => {});
    }
    return Promise.resolve();
}

/// After a live run ends: nothing to show, nothing to drive.
function clearAfterLive() {
    design = null;
    view.clear();
    setMode(null);
    clearPanels();
    clearResult();
    runButton.disabled = true;
    enableDownloads(false);
}

/// Gives the plot back; the server clears it.
async function release() {
    if (!livePlot) return;
    const { plot } = livePlot;
    const gone = leaveLive();
    clearAfterLive();
    await gone;
    setStatus(`plot ${plot} released`);
}

// --- the files

/// Hands the browser a file to save.
function save(filename, bytes, type) {
    const url = URL.createObjectURL(new Blob([bytes], { type }));
    const a = Object.assign(document.createElement('a'), { href: url, download: filename });
    a.click();
    URL.revokeObjectURL(url);
}

/// The three files `redfab build` writes, named after the design: the
/// structure, the report and the design (the layout, timing and pins the
/// simulator and the view use).
async function download() {
    let bytes;
    const name = livePlot ? livePlot.name : built?.name || 'design';
    if (mode === 'live' && !(built && built.source === livePlot.source && built.top === livePlot.top)) {
        // Nothing built here matches what runs on the server; ask the server for it.
        const reply = await fetch(`/api/plots/${livePlot.plot}/structure?token=${livePlot.token}`).catch(() => null);
        if (!reply?.ok) {
            setStatus('the server does not offer the structure; compile here to export world.nbt for the same source', 'bad');
            return;
        }
        bytes = new Uint8Array(await reply.arrayBuffer());
    } else if (built?.files) {
        // The opening circuit: the server built it and keeps its structure.
        const reply = await fetch(`${built.files}/world.nbt`).catch(() => null);
        if (!reply?.ok) {
            setStatus('the server does not offer the structure; compile here to export world.nbt for the same source', 'bad');
            return;
        }
        bytes = new Uint8Array(await reply.arrayBuffer());
    } else {
        // The structure file comes back from the core as hex in the build reply; ask again.
        const reply = await ask({ op: 'structure' });
        bytes = Uint8Array.from(reply.structure.match(/../g), (h) => parseInt(h, 16));
    }
    save(`${name}.nbt`, bytes, 'application/octet-stream');
}

function downloadReport() {
    if (!design) return;
    const report = mode === 'live' ? livePlot.report : built?.report;
    const name = livePlot ? livePlot.name : built?.name || 'design';
    if (typeof report !== 'string') {
        setStatus('no report for this design', 'bad');
        return;
    }
    save(`${name}.report.txt`, report, 'text/plain');
}

async function downloadDesign() {
    if (!design) return;
    const name = livePlot ? livePlot.name : built?.name || 'design';
    // The worker keeps the text the core wrote, byte for byte what `redfab
    // build` writes (the opening circuit's came from the server as text); the
    // server's design comes parsed.
    const text = mode === 'live' ? JSON.stringify(design) : (built?.designText ?? (await ask({ op: 'design' })).design);
    save(`${name}.design.json`, text, 'application/json');
}

// --- wiring

// The live button appears, as the main action, only when the page is served by the live backend.
fetch('/api/status').then((r) => (r.ok ? r.json() : null)).then((s) => {
    camera.init(s?.camera ?? null);
    tabs.hidden = !camera.available;
    if (!s?.live) return;
    liveButton.hidden = false;
    liveButton.classList.add('primary');
    compileButton.classList.remove('primary');
    if (Array.isArray(s.limits?.plotSize)) limits.plotSize = s.limits.plotSize;
    liveButton.title = `build on the server and run it in the game (plots are ${limits.plotSize ? limits.plotSize.join(' x ') : 'of limited'} blocks)`;
}).catch(() => {});
liveButton.addEventListener('click', goLive);
releaseButton.addEventListener('click', release);
window.addEventListener('pagehide', () => leaveLive());

runButton.addEventListener('click', () => (running ? stopRunning() : startRunning()));
clockAutoButton.addEventListener('click', () => setClockMode('auto'));
clockManualButton.addEventListener('click', () => setClockMode('manual'));
compileButton.addEventListener('click', compile);
downloadButton.addEventListener('click', download);
reportButton.addEventListener('click', downloadReport);
designButton.addEventListener('click', downloadDesign);
view.onClick = (pos) => {
    if (!design) return;
    // The lever stands on the probe block; either toggles the input.
    const [x, y, z] = pos.split(',').map(Number);
    const below = `${x},${y - 1},${z}`;
    const i = design.inputs.findIndex((p) => p.probe === pos || p.probe === below);
    if (i >= 0) toggle(i);
};

for (const name of [...EXAMPLES, CUSTOM]) {
    const option = document.createElement('option');
    option.value = name;
    option.textContent = name === CUSTOM ? 'custom (your draft)' : name;
    examples.appendChild(option);
}
examples.addEventListener('change', async () => {
    const pick = examples.value;
    if (pick === CUSTOM) {
        // Back to the draft, when there is one and the editor shows something else.
        const draft = readDraft();
        if (draft && draft.source !== source.value) {
            if (edited() && !confirm('Replace the edited source with your saved draft?')) {
                examples.value = lastPick;
                return;
            }
            source.value = draft.source;
            topField.value = draft.top || '';
        }
        lastPick = CUSTOM;
        return;
    }
    if (edited() && !confirm(`Replace your source with the ${pick} example? Your draft stays saved in this browser.`)) {
        examples.value = lastPick;
        return;
    }
    await loadExample(pick);
});
source.addEventListener('input', () => {
    if (examples.value !== CUSTOM && source.value !== loaded.text) {
        examples.value = CUSTOM;
        lastPick = CUSTOM;
    }
    saveDraft();
});
topField.addEventListener('input', saveDraft);
source.addEventListener('keydown', (e) => {
    if (e.key === 'Tab') {
        e.preventDefault();
        const { selectionStart: s, selectionEnd: t } = source;
        source.setRangeText('    ', s, t, 'end');
        source.dispatchEvent(new Event('input'));
    }
});
document.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter' && !compileButton.disabled) {
        e.preventDefault();
        compile();
    }
});

// The compiler (about 10 MB) is fetched ahead of a compile once the visitor
// shows they may compile: a click into the editor, another example, the
// pointer on Compile. Not at page open, so a visitor who only watches pays for
// none of it.
let warmed = false;
function warmCompiler() {
    if (warmed || !worker) return;
    warmed = true;
    ask({ op: 'warm' }).catch(() => { warmed = false; });
}
source.addEventListener('focus', warmCompiler);
examples.addEventListener('change', warmCompiler);
compileButton.addEventListener('pointerenter', warmCompiler);
compileButton.addEventListener('focus', warmCompiler);

const draft = readDraft();
if (draft) {
    source.value = draft.source;
    topField.value = draft.top || '';
    examples.value = CUSTOM;
    lastPick = CUSTOM;
} else {
    loadExample(OPENING);
}
startWorker();
openWith(OPENING, { draft: draft !== null });

// For the automated checks: the state the page holds, and a way to step
// deterministically with the clock stopped.
window.redfab = {
    get design() { return design; },
    get tick() { return tick; },
    get built() { return built; },
    get opening() { return opening; },
    limits,
    inputs: () => inputState.slice(),
    pendingInputs: () => Object.fromEntries(pendingInputs),
    lamps: () => Object.fromEntries(design.outputs.map((p) => [p.name, Number(outputsPanel.querySelector(`[data-output="${CSS.escape(p.name)}"]`).dataset.lit)])),
    toggle,
    clock: setClock,
    stop: stopRunning,
    get running() { return running; },
    step: async (ticks) => {
        // A hand-driven step is never dropped: wait for the timer's step first.
        while (stepping) await stepping;
        await step(ticks);
        return tick;
    },
    clockLevelAt: (at) => clockLevelAt(at, design.timing.sequential),
    // Resolves once the step in flight, if any, has landed.
    idle: () => stepping ?? Promise.resolve(),
    trace: () => ask({ op: 'trace' }),
    structure: () => ask({ op: 'structure' }),
    colourAt: (pos) => view.colourAt(pos),
    leverOnScreen: (name) => {
        const pin = design.inputs.find((p) => p.name === name);
        const [x, y, z] = pin.probe.split(',').map(Number);
        return view.screenPosition(`${x},${y + 1},${z}`);
    },
    compile,
    loadExample,
    setSource: (text) => { source.value = text; source.dispatchEvent(new Event('input')); },
    setTop: (text) => { topField.value = text; topField.dispatchEvent(new Event('input')); },
    get loadedExample() { return loaded.example; },
    edited,
    ready: () => ask({ op: 'ping' }),
    live: goLive,
    release,
    get mode() { return mode; },
    get livePlot() { return livePlot; },
    get liveTick() { return liveTick; },
    camera,
    selectTab,
    get tab() { return tab; },
    setClockMode,
    get clockMode() { return clockMode; },
    get pendingClock() { return pendingClock; },
    // Frames the view has drawn: a still, hidden or unchanged view draws none.
    get frames() { return view.frames; },
};
