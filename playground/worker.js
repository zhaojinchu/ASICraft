// The page's worker: compiles on request, then runs the simulator on request.
// Everything slow lives here so the page stays responsive; the page terminates
// the worker when a compile takes too long.

import { loadCore } from './core.js';
import { compile } from './compile.js';

// The Yosys core is 67 MB (about 10 MB on the wire, precompressed): it is not
// fetched when the page opens, so the opening circuit shows at once, but when
// the visitor shows they may compile (the page asks for `warm`), or at the
// first compile. `run(null)` fetches and compiles its modules without running
// it. The package path carries its version so the server can let the browser
// keep it for good (see tools/playground_serve.mjs).
let yosysLoad = null;
let yosysReady = false;
function loadYosys() {
    if (!yosysLoad) {
        yosysLoad = (async () => {
            const { runYosys } = await import('/pkg/@yowasp/yosys@0.68.1207/gen/bundle.js');
            await runYosys(null, {}, { fetchProgress: () => {} });
            yosysReady = true;
            return runYosys;
        })();
        yosysLoad.catch(() => { yosysLoad = null; }); // a failed fetch is tried again next time
    }
    return yosysLoad;
}
async function runYosys(...args) {
    return (await loadYosys())(...args);
}

let core = null; // the one load in flight or done: two requests arriving together share it
let handle = null;
let structure = null;
let designText = null;

function ready() {
    if (!core) {
        core = (async () => loadCore(await (await fetch('/core/redfab_wasm.wasm')).arrayBuffer()))();
    }
    return core;
}

/// A new simulation of `text` (a design file) in place of the one in hand.
function simulate(c, text) {
    if (handle !== null) {
        c.call({ op: 'sim_free', handle });
        handle = null;
    }
    handle = c.call({ op: 'sim_new', design: text }).handle;
    designText = text;
}

self.onmessage = async ({ data }) => {
    const { id, op } = data;
    const reply = (result) => self.postMessage({ id, ok: true, ...result });
    try {
        const c = await ready();
        switch (op) {
            case 'compile': {
                if (!yosysReady) {
                    self.postMessage({ id, phase: 'fetching the compiler' });
                    await loadYosys();
                }
                const built = await compile({
                    source: data.source,
                    top: data.top,
                    name: data.name,
                    core: c,
                    runYosys,
                    log: (text) => self.postMessage({ id, log: text }),
                    phase: (name) => self.postMessage({ id, phase: name }),
                });
                simulate(c, built.designText);
                structure = built.structure;
                reply({ design: built.design, report: built.report, top: built.top, name: built.name, cells: built.cells });
                break;
            }
            case 'open':
                // A design built elsewhere (the opening circuit): simulated as is; its structure is the server's.
                simulate(c, data.design);
                structure = null;
                reply({ design: JSON.parse(data.design) });
                break;
            case 'structure':
                reply({ structure });
                break;
            case 'design':
                reply({ design: designText });
                break;
            case 'ping':
                reply({});
                break;
            case 'warm':
                // The compiler fetched and compiled ahead of a compile; nothing is run.
                await loadYosys();
                reply({ ready: yosysReady });
                break;
            case 'set':
                // What changed at once: the probe and the dust and lamps that repowered with it.
                reply(c.call({ op: 'sim_set', handle, input: data.input, on: data.on }));
                break;
            case 'step':
                reply(c.call({ op: 'sim_step', handle, ticks: data.ticks }));
                break;
            case 'trace':
                reply(c.call({ op: 'sim_trace', handle }));
                break;
            default:
                throw new Error(`unknown op ${op}`);
        }
    } catch (err) {
        self.postMessage({ id, ok: false, error: err.message || String(err) });
    }
};
