// One compile in its own process: the parent kills it at the time and memory
// limits, so a hostile source can take down nothing but this process. The
// request and reply travel over the IPC channel.

import { readFileSync } from 'node:fs';
import { runYosys } from '@yowasp/yosys';
import { loadCore } from '../playground/core.js';
import { compile } from '../playground/compile.js';

const core = await loadCore(readFileSync(process.env.REDFAB_WASM));
const hooks = process.env.REDFAB_TEST_HOOKS === '1';

process.on('message', async (request) => {
    try {
        if (hooks && typeof request.source === 'string') {
            // For the checks only: a compile that never ends, one that eats memory, and one that tells what it can see.
            const hook = request.source.match(/^\/\/ redfab-test: (hang|env|allocate (\d+))/);
            if (hook?.[1] === 'env') {
                process.send({ ok: false, error: `env: ${Object.keys(process.env).sort().join(',')}` });
                return;
            }
            if (hook) {
                if (hook[2]) {
                    const kept = [];
                    for (let mb = 0; mb < Number(hook[2]); mb += 64) {
                        const chunk = new Uint8Array(64 * 1024 * 1024);
                        for (let i = 0; i < chunk.length; i += 4096) chunk[i] = 1;
                        kept.push(chunk);
                    }
                    globalThis.kept = kept;
                }
                await new Promise(() => {});
            }
        }
        const built = await compile({ source: request.source, top: request.top, name: request.name, core, runYosys, log: () => {} });
        process.send({ ok: true, designText: built.designText, structure: built.structure, report: built.report });
    } catch (err) {
        process.send({ ok: false, error: err.message || String(err) });
    }
});
process.send({ ready: true });
