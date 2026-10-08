// The page's worker: compiles on request, then runs the simulator on request.
// Everything slow lives here so the page stays responsive; the page terminates
// the worker when a compile takes too long.

// The package path carries its version so the server can let the browser keep
// the 67 MB Yosys core for good (see tools/playground_serve.mjs).
import { runYosys } from '/pkg/@yowasp/yosys@0.68.1207/gen/bundle.js';
import { loadCore } from './core.js';
import { compile } from './compile.js';

let core = null;
let handle = null;
let structure = null;
let designText = null;

async function ready() {
    if (!core) {
        const bytes = await (await fetch('/core/redfab_wasm.wasm')).arrayBuffer();
        core = await loadCore(bytes);
    }
    return core;
}

self.onmessage = async ({ data }) => {
    const { id, op } = data;
    const reply = (result) => self.postMessage({ id, ok: true, ...result });
    try {
        const c = await ready();
        switch (op) {
            case 'compile': {
                if (handle !== null) {
                    c.call({ op: 'sim_free', handle });
                    handle = null;
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
                handle = c.call({ op: 'sim_new', design: built.designText }).handle;
                structure = built.structure;
                designText = built.designText;
                reply({ design: built.design, report: built.report, top: built.top, name: built.name, cells: built.cells });
                break;
            }
            case 'structure':
                reply({ structure });
                break;
            case 'design':
                reply({ design: designText });
                break;
            case 'ping':
                reply({});
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
