// The playground's compile pipeline as a Node worker thread, for the checks: the
// same `compile` the page's worker runs, with YoWASP Yosys from node_modules and
// the core from target/. The parent terminates it when a compile takes too long.

import { parentPort, workerData } from 'node:worker_threads';
import { readFileSync } from 'node:fs';
import { runYosys } from '@yowasp/yosys';
import { loadCore } from '../playground/core.js';
import { compile } from '../playground/compile.js';

const core = await loadCore(readFileSync(workerData.wasm));
parentPort.on('message', async (request) => {
    try {
        const yosys = request.hang ? () => new Promise(() => {}) : runYosys;
        const built = await compile({ ...request, core, runYosys: yosys, log: () => {} });
        parentPort.postMessage({ ok: true, designText: built.designText, report: built.report, structure: built.structure });
    } catch (err) {
        parentPort.postMessage({ ok: false, error: err.message });
    }
});
parentPort.postMessage({ ready: true });
