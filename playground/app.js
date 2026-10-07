// The page: an editor, a compile button, the worker that compiles and
// simulates, a panel of levers and lamps, and the 3D view. The simulator runs
// at the game's 20 ticks a second. Served by the live backend (server/), the
// page can also send the source there: the server builds it, places it on the
// Minecraft server, and the view then shows the server's blocks instead.

import { View } from './view.js';
import { LIMITS } from './compile.js';

// The designs under designs/, served alongside the page.
const EXAMPLES = ['not1', 'nor2', 'nor3', 'fa1', 'adder4', 'consts', 'counter4', 'fsm', 'shift2'];

const TICKS_PER_SECOND = 20;

const el = (id) => document.getElementById(id);
const source = el('source');
const status = el('status');
const log = el('log');
const inputsPanel = el('inputs');
const outputsPanel = el('outputs');
const tickLabel = el('tick');
const compileButton = el('compile');
const runButton = el('run');
const downloadButton = el('download');
const liveButton = el('live');
const releaseButton = el('release');
const examples = el('examples');

const view = new View(el('view'));

let worker = null;
let nextId = 1;
const pending = new Map();
let design = null;
let inputState = [];
let running = false;
let tick = 0;
let timer = null;
let stepping = false;
let clockTimer = null;
// 'sim': the worker's simulator drives the view. 'live': the server's stream does.
let mode = 'sim';
let livePlot = null;
let liveSocket = null;
let liveTick = 0;

function setStatus(text, kind = '') {
    status.textContent = text;
    status.className = kind;
}

function startWorker() {
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
        pending.delete(data.id);
        if (data.ok) waiting.resolve(data);
        else waiting.reject(new Error(data.error));
    };
    worker.onerror = (e) => setStatus(`the worker failed: ${e.message}`, 'bad');
    for (const waiting of pending.values()) waiting.reject(new Error('the worker was restarted'));
    pending.clear();
}

function ask(message) {
    const id = nextId++;
    return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        worker.postMessage({ id, ...message });
    });
}

async function compile() {
    stopRunning();
    leaveLive();
    design = null;
    compileButton.disabled = true;
    runButton.disabled = true;
    downloadButton.disabled = true;
    log.textContent = '';
    inputsPanel.replaceChildren(label('inputs'));
    outputsPanel.replaceChildren(label('outputs'));
    tickLabel.textContent = '';
    setStatus('Compiling…');
    const name = examples.value || 'design';
    const deadline = setTimeout(() => {
        startWorker();
        setStatus(`Compiling took longer than ${LIMITS.compileSeconds} s; stopped.`, 'bad');
        compileButton.disabled = false;
    }, LIMITS.compileSeconds * 1000);
    try {
        const reply = await ask({ op: 'compile', source: source.value, top: 'top', name });
        clearTimeout(deadline);
        design = reply.design;
        setStatus(`Built: ${reply.report.split('\n').slice(1, 4).map((l) => l.trim()).join('; ')}`, 'ok');
        showDesign();
    } catch (err) {
        clearTimeout(deadline);
        setStatus(err.message.split('\n')[0], 'bad');
        log.textContent += `\n${err.message}\n`;
    } finally {
        compileButton.disabled = false;
    }
}

function label(text) {
    const span = document.createElement('span');
    span.className = 'label';
    span.textContent = text;
    return span;
}

function showDesign() {
    view.show(design);
    inputState = design.inputs.map(() => 0);
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
        lamp.innerHTML = `<span class="bulb"></span>${pin.name}`;
        outputsPanel.appendChild(lamp);
    }
    // A clocked design gets its clock from the page (or the server), at the reported period.
    if (seq) {
        const note = document.createElement('span');
        note.className = 'label';
        note.textContent = `clock ${seq.clock}: period ${seq.period} ticks, driven by the ${mode === 'live' ? 'server' : 'page'}`;
        inputsPanel.appendChild(note);
    }
    runButton.disabled = mode === 'live';
    downloadButton.disabled = mode === 'live';
    if (mode === 'sim') startRunning();
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
    inputState[i] = inputState[i] ? 0 : 1;
    inputsPanel.querySelector(`[data-input="${CSS.escape(design.inputs[i].name)}"]`).dataset.on = String(inputState[i]);
    await ask({ op: 'set', input: i, on: inputState[i] === 1 });
    view.update([[design.inputs[i].probe, inputState[i] ? design.inputs[i].on : design.inputs[i].off]]);
}

async function step(ticks) {
    if (stepping || !design) return;
    stepping = true;
    try {
        const reply = await ask({ op: 'step', ticks });
        tick = reply.tick;
        view.update(reply.changes);
        for (const [o, pin] of design.outputs.entries()) {
            outputsPanel.querySelector(`[data-output="${CSS.escape(pin.name)}"]`).dataset.lit = String(reply.lamps[o]);
        }
        tickLabel.textContent = `tick ${tick}`;
    } finally {
        stepping = false;
    }
}

function startRunning() {
    running = true;
    runButton.textContent = 'Pause';
    timer = setInterval(() => step(1), 1000 / TICKS_PER_SECOND);
    const seq = design?.timing?.sequential;
    if (seq) {
        // The clock: its active edge at the start of each period, the other halfway.
        const clock = design.inputs.findIndex((p) => p.name === seq.clock);
        const activeHigh = seq.active === 'rising';
        let phase = 0;
        const flip = async (on) => {
            inputState[clock] = on ? 1 : 0;
            inputsPanel.querySelector(`[data-input="${CSS.escape(seq.clock)}"]`).dataset.on = String(inputState[clock]);
            await ask({ op: 'set', input: clock, on });
            view.update([[design.inputs[clock].probe, on ? design.inputs[clock].on : design.inputs[clock].off]]);
        };
        clockTimer = setInterval(() => {
            phase = (phase + 1) % seq.period;
            if (phase === 0) flip(activeHigh);
            if (phase === seq.high) flip(!activeHigh);
        }, 1000 / TICKS_PER_SECOND);
    }
}

function stopRunning() {
    running = false;
    runButton.textContent = 'Run';
    clearInterval(timer);
    clearInterval(clockTimer);
    timer = null;
    clockTimer = null;
}

// --- the live server

function clearPanels() {
    inputsPanel.replaceChildren(label('inputs'));
    outputsPanel.replaceChildren(label('outputs'));
    tickLabel.textContent = '';
}

/// Sends the source to the server, which builds it and places it in the game.
async function goLive() {
    stopRunning();
    leaveLive();
    design = null;
    mode = 'live';
    compileButton.disabled = true;
    liveButton.disabled = true;
    runButton.disabled = true;
    downloadButton.disabled = true;
    log.textContent = '';
    clearPanels();
    setStatus('Sent to the server; building…');
    const name = examples.value || 'design';
    try {
        const reply = await fetch('/api/submit', {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ source: source.value, top: 'top', name }),
        });
        const body = await reply.json();
        if (!reply.ok) throw new Error(body.error || `the server answered ${reply.status}`);
        design = body.design;
        livePlot = { plot: body.plot, token: body.token, origin: body.origin };
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
        if (m.state === 'live') setStatus(`Live on the server at plot ${livePlot.plot}.`, 'ok');
        if (m.state === 'released') {
            setStatus(`The server released plot ${livePlot.plot}: ${m.why}.`, 'bad');
            leaveLive();
        }
        if (m.ok === false) setStatus(m.error, 'bad');
    };
    liveSocket.onclose = () => {
        if (mode === 'live') setStatus('The connection to the server closed.', 'bad');
    };
}

function leaveLive() {
    if (liveSocket) {
        liveSocket.onclose = null;
        liveSocket.close();
        liveSocket = null;
    }
    livePlot = null;
    mode = 'sim';
    releaseButton.hidden = true;
}

/// Gives the plot back; the server clears it.
async function release() {
    if (!livePlot) return;
    const { plot, token } = livePlot;
    leaveLive();
    await fetch(`/api/plots/${plot}?token=${token}`, { method: 'DELETE' }).catch(() => {});
    design = null;
    clearPanels();
    runButton.disabled = true;
    downloadButton.disabled = true;
    setStatus(`Released plot ${plot}. Compile to simulate here again.`);
}

// The live button appears only when the page is served by the live backend.
fetch('/api/status').then((r) => (r.ok ? r.json() : null)).then((s) => {
    if (s?.live) liveButton.hidden = false;
}).catch(() => {});
liveButton.addEventListener('click', goLive);
releaseButton.addEventListener('click', release);

runButton.addEventListener('click', () => (running ? stopRunning() : startRunning()));
compileButton.addEventListener('click', compile);
downloadButton.addEventListener('click', async () => {
    // The structure file comes back from the core as hex in the build reply; ask again.
    const reply = await ask({ op: 'structure' });
    const bytes = Uint8Array.from(reply.structure.match(/../g), (h) => parseInt(h, 16));
    const url = URL.createObjectURL(new Blob([bytes], { type: 'application/octet-stream' }));
    const a = Object.assign(document.createElement('a'), { href: url, download: `${examples.value || 'design'}.nbt` });
    a.click();
    URL.revokeObjectURL(url);
});
view.onClick = (pos) => {
    if (!design) return;
    // The lever stands on the probe block; either toggles the input.
    const [x, y, z] = pos.split(',').map(Number);
    const below = `${x},${y - 1},${z}`;
    const i = design.inputs.findIndex((p) => p.probe === pos || p.probe === below);
    if (i >= 0) toggle(i);
};

for (const name of EXAMPLES) {
    const option = document.createElement('option');
    option.value = name;
    option.textContent = name;
    examples.appendChild(option);
}
async function loadExample(name) {
    const reply = await fetch(`/designs/${name}/top.sv`);
    source.value = reply.ok ? await reply.text() : `// could not load ${name}`;
}
examples.addEventListener('change', () => loadExample(examples.value));
loadExample(examples.value);
source.addEventListener('keydown', (e) => {
    if (e.key === 'Tab') {
        e.preventDefault();
        const { selectionStart: s, selectionEnd: t } = source;
        source.setRangeText('    ', s, t, 'end');
    }
});

startWorker();

// For the automated checks: the state the page holds, and a way to step
// deterministically with the clock stopped.
window.redfab = {
    get design() { return design; },
    get tick() { return tick; },
    inputs: () => inputState.slice(),
    lamps: () => Object.fromEntries(design.outputs.map((p) => [p.name, Number(outputsPanel.querySelector(`[data-output="${CSS.escape(p.name)}"]`).dataset.lit)])),
    toggle,
    stop: stopRunning,
    step: async (ticks) => { await step(ticks); return tick; },
    trace: () => ask({ op: 'trace' }),
    leverOnScreen: (name) => {
        const pin = design.inputs.find((p) => p.name === name);
        const [x, y, z] = pin.probe.split(',').map(Number);
        return view.screenPosition(`${x},${y + 1},${z}`);
    },
    compile,
    loadExample,
    setSource: (text) => { source.value = text; },
    ready: () => ask({ op: 'ping' }),
    live: goLive,
    release,
    get mode() { return mode; },
    get livePlot() { return livePlot; },
    get liveTick() { return liveTick; },
};
