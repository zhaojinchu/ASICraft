// The playground's pipeline under Node, without a browser. See `check`.
//
//   node --test tools/playground_test.mjs

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Worker } from 'node:worker_threads';
import { runYosys } from '@yowasp/yosys';
import { loadCore } from '../playground/core.js';
import { compile, LIMITS } from '../playground/compile.js';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const wasm = join(root, 'target/wasm32-unknown-unknown/release/redfab_wasm.wasm');
execFileSync('cargo', ['build', '-q', '-p', 'redfab-wasm', '--target', 'wasm32-unknown-unknown', '--release'], { cwd: root, stdio: 'inherit' });
const core = await loadCore(readFileSync(wasm));
const source = (name) => readFileSync(join(root, 'designs', name, 'top.sv'), 'utf8');
const native = (name, file) => readFileSync(join(root, 'build/designs', name, file), 'utf8');
const DESIGNS = ['not1', 'nor2', 'nor3', 'consts', 'fa1', 'adder4', 'counter4', 'fsm', 'shift2'];

/// Runs a compile in a worker thread, killing it after `seconds`.
function compileInWorker(request, seconds) {
    return new Promise((resolve) => {
        const worker = new Worker(join(root, 'tools/playground_worker.mjs'), { workerData: { wasm } });
        const timer = setTimeout(() => {
            worker.terminate();
            resolve({ ok: false, error: `stopped after ${seconds} s`, killed: true });
        }, seconds * 1000);
        worker.on('message', (reply) => {
            if (reply.ready) {
                worker.postMessage(request);
                return;
            }
            clearTimeout(timer);
            worker.terminate();
            resolve(reply);
        });
        worker.on('error', (err) => {
            clearTimeout(timer);
            resolve({ ok: false, error: err.message });
        });
    });
}

for (const name of DESIGNS) {
    test(`${name}: the pipeline gives the design the native build wrote`, async () => {
        const built = await compile({ source: source(name), top: 'top', name, core, runYosys });
        assert.equal(built.designText, native(name, 'design.json'));
    });
}

test('the in-browser simulator traces a stimulus exactly as the native one does', () => {
    const designText = native('fa1', 'design.json');
    const design = JSON.parse(designText);
    const { handle } = core.call({ op: 'sim_new', design: designText });
    // Walk the inputs: every combination, settling between, then a quick burst.
    const n = design.inputs.length;
    for (let v = 1; v < 1 << n; v++) {
        for (let i = 0; i < n; i++) core.call({ op: 'sim_set', handle, input: i, on: ((v >> i) & 1) === 1 });
        core.call({ op: 'sim_step', handle, ticks: design.settle_ticks + 2 });
    }
    for (let k = 0; k < 6; k++) {
        core.call({ op: 'sim_set', handle, input: 0, on: k % 2 === 0 });
        core.call({ op: 'sim_step', handle, ticks: 3 });
    }
    core.call({ op: 'sim_step', handle, ticks: design.settle_ticks + 2 });
    const { fixture, trace } = core.call({ op: 'sim_trace', handle });
    core.call({ op: 'sim_free', handle });
    const dir = mkdtempSync(join(tmpdir(), 'redfab-trace-'));
    writeFileSync(join(dir, 'browser.fixture.json'), JSON.stringify(fixture));
    execFileSync('cargo', ['run', '-q', '-p', 'redfab-cli', '--', 'check-fixtures', dir], { cwd: root, stdio: 'pipe' });
    const nativeTrace = JSON.parse(readFileSync(join(dir, 'browser.sim.trace.json'), 'utf8'));
    assert.deepEqual(trace, nativeTrace);
    assert.ok(trace.ticks.length > 20, 'the stimulus caused changes');
});

test('a source over the size limit is refused at once', async () => {
    const big = source('not1') + '\n' + '// '.repeat(1) + 'x'.repeat(LIMITS.sourceBytes);
    const started = Date.now();
    await assert.rejects(compile({ source: big, top: 'top', name: 'big', core, runYosys }), /limit is/);
    assert.ok(Date.now() - started < 2000);
});

test('a design over the cell limit is refused after synthesis', async () => {
    await assert.rejects(
        compile({ source: source('adder4'), top: 'top', name: 'adder4', core, runYosys, limits: { ...LIMITS, cells: 10 } }),
        /cells; the limit is 10/,
    );
});

test('a module name that is not an identifier is refused', async () => {
    await assert.rejects(compile({ source: source('not1'), top: 'top; rm -rf', name: 'x', core, runYosys }), /not a module name/);
});

test('a compile that never finishes is stopped at the time limit and the worker comes back', async () => {
    const reply = await compileInWorker({ source: source('not1'), top: 'top', name: 'hang', hang: true }, 3);
    assert.equal(reply.killed, true);
    const again = await compileInWorker({ source: source('not1'), top: 'top', name: 'not1' }, 60);
    assert.equal(again.ok, true, again.error);
    assert.equal(again.designText, native('not1', 'design.json'));
});

test('a hostile source is refused within the time limit, one way or another', async () => {
    // A generate loop that would elaborate to millions of cells.
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
    const started = Date.now();
    const reply = await compileInWorker({ source: hostile, top: 'top', name: 'hostile' }, LIMITS.compileSeconds);
    assert.equal(reply.ok, false);
    assert.ok(Date.now() - started <= (LIMITS.compileSeconds + 5) * 1000);
});
