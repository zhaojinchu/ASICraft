// The page: an editor, a compile button, the worker that compiles and
// simulates, a panel of levers and lamps, and the 3D view. The simulator runs
// at the game's 20 ticks a second. Served by the live backend (server/), the
// page can also send the source there: the server builds it, places it on the
// Minecraft server, and the view then shows the server's blocks instead.

import { View } from './view.js';
import { LIMITS, detectTop, nameFor } from './compile.js';

// The designs under designs/, served alongside the page.
const EXAMPLES = ['not1', 'nor2', 'nor3', 'fa1', 'adder4', 'consts', 'counter4', 'fsm', 'shift2'];
// The dropdown entry for the user's own source, kept in this browser.
const CUSTOM = 'custom';
const DRAFT_KEY = 'redfab.draft';

const TICKS_PER_SECOND = 20;

const el = (id) => document.getElementById(id);
const source = el('source');
const topField = el('top');
const status = el('status');
const log = el('log');
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
const examples = el('examples');

const view = new View(el('view'));

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
let running = false;
let tick = 0;
let timer = null;
// The step in flight, so a hand-driven step can wait for it instead of being dropped.
let stepping = null;
// The last successful local build: the worker holds its structure, and its
// footprint is known before a live submission.
let built = null;
// 'sim': the worker's simulator drives the view. 'live': the server's stream does.
let mode = 'sim';
let livePlot = null;
let liveSocket = null;
let liveTick = 0;
// What the examples dropdown last put in the editor, to tell an edit from it.
let loaded = { example: null, text: '' };
let lastPick = null;

function setStatus(text, kind = '') {
    status.textContent = text;
    status.className = kind;
}

/// Starts (or restarts) the worker; whatever was waiting fails with `reason`.
function startWorker(reason = 'the worker was restarted') {
    if (worker) worker.terminate();
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

// --- compiling

function describeBuilt(reply) {
    const { design: d, cells, name } = reply;
    const capacity = cells > limits.measuredCells
        ? `more than the ${limits.measuredCells} the router had managed when last measured`
        : `the router manages about ${limits.measuredCells} today`;
    const parts = [`${cells} cells (${capacity})`, `${Object.keys(d.blocks).length} blocks in ${d.size[0]} x ${d.size[2]}`];
    let kind = 'ok';
    if (limits.plotSize) {
        if (fitsPlot(d)) parts.push(`fits the server's ${limits.plotSize[0]} x ${limits.plotSize[1]} plot`);
        else {
            parts.push(`too large for the server's ${limits.plotSize[0]} x ${limits.plotSize[1]} plot`);
            kind = 'warn';
        }
    }
    const seq = d.timing?.sequential;
    parts.push(seq ? `clock ${seq.clock} at ${seq.period} ticks a cycle` : `outputs settle within ${d.settle_ticks} ticks`);
    return { text: `Built ${name}: ${parts.join('; ')}.`, kind };
}

function fitsPlot(d) {
    return !limits.plotSize || (d.size[0] <= limits.plotSize[0] && d.size[2] <= limits.plotSize[1]);
}

async function compile() {
    stopRunning();
    leaveLive();
    design = null;
    built = null;
    view.clear();
    compileButton.disabled = true;
    runButton.disabled = true;
    enableDownloads(false);
    log.textContent = '';
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
    const progress = () => setStatus(`Compiling ${name}: ${phase}, ${Math.round((Date.now() - started) / 1000)} s`);
    progress();
    const ticker = setInterval(progress, 1000);
    const deadline = setTimeout(() => {
        startWorker(`Compiling took longer than ${limits.compileSeconds} s; stopped. Try a smaller design.`);
    }, limits.compileSeconds * 1000);
    try {
        const reply = await ask({ op: 'compile', source: text, top, name }, { phase: (p) => { phase = p; progress(); } });
        design = reply.design;
        built = { source: text, top, name, design: reply.design, cells: reply.cells, report: reply.report };
        const { text: summary, kind } = describeBuilt(reply);
        setStatus(summary, kind);
        showDesign();
    } catch (err) {
        setStatus(err.message.split('\n')[0], 'bad');
        log.textContent += `\n${err.message}\n`;
    } finally {
        clearInterval(ticker);
        clearTimeout(deadline);
        compileButton.disabled = false;
    }
}

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
}

function showDesign() {
    view.show(design);
    inputState = design.inputs.map(() => 0);
    pendingInputs = new Map();
    tick = 0;
    const seq = design.timing?.sequential;
    for (const [i, pin] of design.inputs.entries()) {
        const button = document.createElement('button');
        button.className = 'lever';
        button.textContent = pin.name;
        button.dataset.input = pin.name;
        button.dataset.on = '0';
        button.disabled = Boolean(seq && pin.name === seq.clock); // the clock is driven, not clicked
        button.addEventListener('click', () => toggle(i));
        inputsPanel.appendChild(button);
    }
    for (const pin of design.outputs) {
        const lamp = document.createElement('span');
        lamp.className = 'lamp';
        lamp.dataset.output = pin.name;
        lamp.dataset.lit = '0';
        const bulb = document.createElement('span');
        bulb.className = 'bulb';
        lamp.append(bulb, pin.name); // text, never markup: escaped identifiers reach here
        outputsPanel.appendChild(lamp);
    }
    // A clocked design gets its clock from the page (or the server), at the reported period.
    if (seq) {
        const note = document.createElement('span');
        note.className = 'label';
        note.textContent = `clock ${seq.clock}: period ${seq.period} ticks, driven by the ${mode === 'live' ? 'server' : 'page'}; outputs are valid ${seq.sample_at} ticks after the edge`;
        inputsPanel.appendChild(note);
    }
    runButton.disabled = mode === 'live';
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
    if (seq && design.inputs[i].name === seq.clock) return; // the page or the server drives the clock
    if (mode === 'live') {
        // The server flips the probe block; the lever shows the new state once the stream reports it.
        if (liveSocket?.readyState === WebSocket.OPEN) liveSocket.send(JSON.stringify({ set: design.inputs[i].name, on: inputState[i] !== 1 }));
        return;
    }
    // A clocked design's inputs move only in the cycle's input window, at
    // input_change_at after the edge (the report's rule, and what the server
    // does in live mode): a click at any other tick is held until then, and a
    // second click before that takes the request back.
    const wanted = !(pendingInputs.has(i) ? pendingInputs.get(i) : inputState[i] === 1);
    if (!seq || tick % seq.period === seq.input_change_at) {
        pendingInputs.delete(i);
        await setInput(i, wanted);
        return;
    }
    if (wanted === (inputState[i] === 1)) pendingInputs.delete(i);
    else pendingInputs.set(i, wanted);
    showPending();
}

/// Marks the levers whose click is waiting for the input window.
function showPending() {
    const seq = design?.timing?.sequential;
    for (const [i, pin] of design.inputs.entries()) {
        const button = inputsPanel.querySelector(`[data-input="${CSS.escape(pin.name)}"]`);
        const pending = pendingInputs.has(i);
        button.dataset.pending = pending ? '1' : '0';
        button.title = pending ? `${pin.name} changes at cycle tick ${seq.input_change_at}, the next input window` : '';
    }
}

/// Applies the held clicks when the tick is the cycle's input window.
async function applyPendingInputs() {
    const seq = design?.timing?.sequential;
    if (!seq || !pendingInputs.size || tick % seq.period !== seq.input_change_at) return;
    const due = [...pendingInputs];
    pendingInputs.clear();
    for (const [i, on] of due) await setInput(i, on);
    showPending();
}

/// Flips an input in the simulator and recolours what changed at once.
async function setInput(i, on) {
    inputState[i] = on ? 1 : 0;
    inputsPanel.querySelector(`[data-input="${CSS.escape(design.inputs[i].name)}"]`).dataset.on = String(inputState[i]);
    const reply = await ask({ op: 'set', input: i, on });
    view.update(reply.changes);
}

async function setClock(on) {
    const seq = design?.timing?.sequential;
    if (!seq) return;
    await setInput(design.inputs.findIndex((p) => p.name === seq.clock), on);
}

/// The clock, counted in simulated ticks: its active edge at the start of
/// each period, the other edge `high` ticks later, whatever the wall clock does.
async function clockEdges(at) {
    const seq = design.timing?.sequential;
    if (!seq) return;
    const activeHigh = seq.active === 'rising';
    const phase = at % seq.period;
    if (phase === 0) await setClock(activeHigh);
    else if (phase === seq.high) await setClock(!activeHigh);
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
        // The page may have moved on while the worker stepped (Run on the
        // server, a new compile): the reply belongs to a design that is gone.
        if (!design) return;
        tick = reply.tick;
        view.update(reply.changes);
        for (const [o, pin] of design.outputs.entries()) {
            outputsPanel.querySelector(`[data-output="${CSS.escape(pin.name)}"]`).dataset.lit = String(reply.lamps[o]);
        }
        const seq = design.timing?.sequential;
        tickLabel.textContent = seq ? `tick ${tick}, cycle tick ${tick % seq.period} of ${seq.period}` : `tick ${tick}`;
        if (auto) await clockEdges(tick);
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
    const active = at >= seq.period && at % seq.period < seq.high;
    return active === (seq.active === 'rising');
}

function startRunning() {
    running = true;
    runButton.textContent = 'Pause';
    // The clock takes the level its phase implies, so Pause and Run inside a
    // pulse do not cut the pulse short.
    const seq = design?.timing?.sequential;
    if (seq) {
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

// --- the live server

/// Sends the source to the server, which builds it and places it in the game.
async function goLive() {
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
        setStatus(`This design is ${built.design.size[0]} by ${built.design.size[2]} blocks; the server's plots are ${limits.plotSize[0]} by ${limits.plotSize[1]}. Not sent.`, 'bad');
        return;
    }
    design = null;
    mode = 'live';
    compileButton.disabled = true;
    liveButton.disabled = true;
    runButton.disabled = true;
    enableDownloads(false);
    log.textContent = '';
    clearPanels();
    view.clear();
    setStatus('Sent to the server; building…');
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
        log.textContent = body.report;
        setStatus(`On the server at plot ${body.plot}, settling…`);
        showDesign();
        releaseButton.hidden = false;
        connectLive();
    } catch (err) {
        mode = 'sim';
        setStatus(err.message.split('\n')[0], 'bad');
        log.textContent += `\n${err.message}\n`;
    } finally {
        compileButton.disabled = false;
        liveButton.disabled = false;
    }
}

/// How the server paces lever changes, from the `levers` it sends with the
/// submit reply and the live message: one change per input every so many
/// ticks, or a window after the clock edge.
function leverPacing(levers) {
    if (!levers) return '';
    if (typeof levers.spacing === 'number') return `; levers move at most every ${levers.spacing} ticks each`;
    if (Array.isArray(levers.window)) return `; levers move ${levers.window[0]} to ${levers.window[1]} ticks after each ${levers.clock} edge`;
    return '';
}

function connectLive() {
    const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
    liveSocket = new WebSocket(`${scheme}://${location.host}/live?plot=${livePlot.plot}&token=${livePlot.token}`);
    liveSocket.onmessage = ({ data }) => {
        const m = JSON.parse(data);
        if (!design) return;
        if (typeof m.tick === 'number') {
            liveTick = m.tick;
            tickLabel.textContent = `server tick ${liveTick}`;
        }
        if (m.changes) view.update(m.changes);
        if (m.lamps) {
            for (const [o, pin] of design.outputs.entries()) {
                outputsPanel.querySelector(`[data-output="${CSS.escape(pin.name)}"]`).dataset.lit = String(m.lamps[o]);
            }
        }
        if (m.inputs) {
            inputState = m.inputs.slice();
            for (const [i, pin] of design.inputs.entries()) {
                inputsPanel.querySelector(`[data-input="${CSS.escape(pin.name)}"]`).dataset.on = String(m.inputs[i]);
            }
        }
        if (m.state === 'live') setStatus(`Live on the server at plot ${livePlot.plot}${leverPacing(m.levers || livePlot.levers)}.`, 'ok');
        // A lever change asked for before its slot goes out at the tick the server names.
        if (m.ok === true && m.queued) setStatus(`Lever ${m.set} queued for tick ${m.tick}, the next slot its timing allows.`, 'ok');
        if (m.state === 'released') {
            const why = `The server released plot ${livePlot.plot}: ${m.why}.`;
            leaveLive({ release: false });
            clearAfterLive();
            setStatus(why, 'bad');
        }
        if (m.ok === false) setStatus(m.error, 'bad');
    };
    liveSocket.onclose = () => {
        if (mode === 'live') setStatus('The connection to the server closed.', 'bad');
    };
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
    clearPanels();
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
    setStatus(`Released plot ${plot}. Compile to simulate here again.`);
}

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
            setStatus('The server does not offer the structure; Compile here to download world.nbt for the same source.', 'bad');
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
        setStatus('No report for this design.', 'bad');
        return;
    }
    save(`${name}.report.txt`, report, 'text/plain');
}

async function downloadDesign() {
    if (!design) return;
    const name = livePlot ? livePlot.name : built?.name || 'design';
    // The worker keeps the text the core wrote, byte for byte what `redfab
    // build` writes; the server's design comes parsed.
    const text = mode === 'live' ? JSON.stringify(design) : (await ask({ op: 'design' })).design;
    save(`${name}.design.json`, text, 'application/json');
}

// The live button appears only when the page is served by the live backend.
fetch('/api/status').then((r) => (r.ok ? r.json() : null)).then((s) => {
    if (!s?.live) return;
    liveButton.hidden = false;
    if (Array.isArray(s.limits?.plotSize)) limits.plotSize = s.limits.plotSize;
    liveButton.title = `build on the server and run it in the game (plots are ${limits.plotSize ? limits.plotSize.join(' x ') : 'of limited'} blocks)`;
}).catch(() => {});
liveButton.addEventListener('click', goLive);
releaseButton.addEventListener('click', release);
window.addEventListener('pagehide', () => leaveLive());

runButton.addEventListener('click', () => (running ? stopRunning() : startRunning()));
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

{
    const draft = readDraft();
    if (draft) {
        source.value = draft.source;
        topField.value = draft.top || '';
        examples.value = CUSTOM;
        lastPick = CUSTOM;
    } else {
        loadExample(EXAMPLES[0]);
    }
}

// What size works, before the first compile.
setStatus(`Ready. About ${limits.measuredCells} cells (an 8-bit adder) compile inside the ${limits.compileSeconds} s limit.`);
startWorker();

// For the automated checks: the state the page holds, and a way to step
// deterministically with the clock stopped.
window.redfab = {
    get design() { return design; },
    get tick() { return tick; },
    get built() { return built; },
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
};
