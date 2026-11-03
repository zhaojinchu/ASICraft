// The playground's pipeline under Node, without a browser. See `check`.
//
//   node --test tools/playground_test.mjs

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Worker } from 'node:worker_threads';
import { runYosys } from '@yowasp/yosys';
import { loadCore } from '../playground/core.js';
import { compile, countCells, detectTop, explain, LIMITS, moduleNames, nameFor } from '../playground/compile.js';
import { buildWasm, root, wasmPath as wasm } from './common.mjs';
import { OPENING, PACKAGES, cacheControl, contentSecurityPolicy, prepareOpening, resolve, serve } from './playground_serve.mjs';

// REDFAB_NO_BUILD=1 skips the cargo builds and uses the core and the CLI as
// last built, to iterate on the page while the crates are being worked on.
const noBuild = process.env.REDFAB_NO_BUILD === '1';
if (!noBuild) buildWasm();
const cli = ['target/release/redfab', 'target/debug/redfab'].map((p) => join(root, p)).find((p) => existsSync(p));
const redfab = (args, options) => (noBuild && cli
    ? execFileSync(cli, args, { cwd: root, ...options })
    : execFileSync('cargo', ['run', '-q', '-p', 'redfab-cli', '--', ...args], { cwd: root, ...options }));
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
    test(`${name}: the pipeline gives the design and the report the native build wrote`, async () => {
        const built = await compile({ source: source(name), top: 'top', name, core, runYosys });
        assert.equal(built.designText, native(name, 'design.json'));
        assert.equal(built.report, native(name, 'report.txt'));
        // The count taken after synthesis is the one the report states.
        assert.equal(built.cells, Number(built.report.match(/cells: (\d+)/)[1]));
    });
}

/// An n-bit ripple-carry adder: 2n + 1 inputs, n + 1 outputs, about 12n cells.
const adder = (n) => `module top(input logic [${n - 1}:0] a, input logic [${n - 1}:0] b, input logic cin, output logic [${n - 1}:0] sum, output logic cout);
    assign {cout, sum} = a + b + cin;
endmodule
`;

/// A core that records the operations asked of it.
function recording(c) {
    const ops = [];
    return { ops, call: (request) => { ops.push(request.op); return c.call(request); } };
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
    redfab(['check-fixtures', dir], { stdio: 'pipe' });
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

test('a design over the cell limit is refused right after synthesis, before the proof and the build', async () => {
    const seen = recording(core);
    const started = Date.now();
    await assert.rejects(
        compile({ source: source('adder4'), top: 'top', name: 'adder4', core: seen, runYosys, limits: { ...LIMITS, cells: 10 } }),
        /the design has 49 cells; the limit is 10/,
    );
    assert.ok(!seen.ops.includes('build'), `the core was asked to build: ${seen.ops}`);
    assert.ok(!seen.ops.includes('check_inits'), 'the proof was still run');
    assert.ok(Date.now() - started < 10_000, 'the refusal waited for more than synthesis');
    assert.equal(countCells(readFileSync(join(root, 'build/designs/adder4/mapped.json'), 'utf8'), 'top'), 49);
    assert.equal(countCells('{"modules":{}}', 'top'), 0);
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

// --- the top module, the design name, and the refusals in plain words

test('the top module is detected from the source when none is named', () => {
    assert.deepEqual(moduleNames('module a; endmodule module b; endmodule'), ['a', 'b']);
    assert.deepEqual(moduleNames('// module c\n/* module d */ module e (input x); endmodule'), ['e']);
    assert.deepEqual(moduleNames('module automatic f; endmodule'), ['f']);
    assert.equal(detectTop(source('not1')), 'top');
    assert.equal(detectTop('module inv(input a, output y); assign y = ~a; endmodule'), 'inv');
    assert.equal(detectTop('module top; endmodule module helper; endmodule'), 'top');
    assert.throws(() => detectTop('module a; endmodule module b; endmodule'), /2 modules \(a, b\)/);
    assert.throws(() => detectTop('// nothing'), /declares no module/);
    assert.equal(nameFor('inv'), 'inv');
    assert.equal(nameFor('a$b'), 'ab');
    assert.equal(nameFor('x'.repeat(40)).length, 32);
    assert.equal(nameFor('$'), 'design');
});

test('a module not named top builds under its own name when the top is left to the page', async () => {
    const inv = 'module inv(input logic a, output logic y);\n  assign y = ~a;\nendmodule\n';
    const built = await compile({ source: inv, top: '', core, runYosys });
    assert.equal(built.top, 'inv');
    assert.equal(built.name, 'inv');
    assert.equal(built.design.name, 'inv');
    assert.equal(built.cells, 1);
});

test('refusals come as one plain line first, with no terminal colour codes', async () => {
    const inv = 'module inv(input logic a, output logic y);\n  assign y = ~a;\nendmodule\n';
    const cases = [
        [inv, 'top', /^the source has no module named "top": name your top module "top" or set the top module$/],
        ['module top(input logic a, output logic y)\n  assign y = ~a;\nendmodule\n', 'top', /^top\.sv:\d+:\d+: error: expected ';'/],
        ['module top(input logic clk, input logic rst, input logic d, output logic q);\n  always_ff @(posedge clk or posedge rst) if (rst) q <= 0; else q <= d;\nendmodule\n', 'top', /^register `q` has an asynchronous reset \(at top\.sv:2\): asynchronous resets are not supported; write a synchronous one/],
        // A latch: the core's lint refuses it from pre.json before the proof (abc would turn it into a loop).
        ['module top(input logic en, input logic d, output logic q);\n  always_comb if (en) q = d;\nendmodule\n', 'top', /^`q` (is a latch|depends on itself through logic with no register in the loop) \(at top\.sv:2\): .*latch/],
    ];
    for (const [text, top, expected] of cases) {
        let message = null;
        try {
            await compile({ source: text, top, core, runYosys });
        } catch (err) {
            message = err.message;
        }
        assert.ok(message, `${expected} was expected but the source compiled`);
        assert.match(message.split('\n')[0], expected);
        assert.ok(!message.includes('\x1b'), `colour codes survive in ${JSON.stringify(message)}`);
    }
});

test('known Yosys and core refusals are explained in plain words', () => {
    assert.match(explain("ERROR: Module 'top' is not a valid top-level module", 'top'), /name your top module "top"/);
    assert.match(explain('warning: no top-level modules found', 'top'), /no module named "top"/);
    assert.match(explain('ERROR: FF $x cannot be legalized: dffs with async set or reset are not supported'), /asynchronous reset/);
    assert.match(explain('cell $1 has type $print, which is not in the tile library'), /\$display/);
    assert.match(explain('the layout does not implement the netlist: timing: flip-flops are clocked by 2 different inputs'), /only one clock/);
    assert.match(explain('cell $2 has type $dlatch'), /latches are not supported/);
    assert.equal(explain('something else entirely'), 'something else entirely');
});

test('the measured cell capacity sits between the shipped examples and the hard limit', async () => {
    const built = await compile({ source: source('adder4'), top: 'top', name: 'adder4', core, runYosys });
    assert.ok(built.cells <= LIMITS.measuredCells, `adder4 has ${built.cells} cells, measuredCells is ${LIMITS.measuredCells}`);
    assert.ok(LIMITS.measuredCells < LIMITS.cells);
});

test('a design of the measured size compiles through the WebAssembly pipeline well inside the time limit', async () => {
    // The 8-bit adder measured for measuredCells: 102 cells, 17 inputs. A router
    // regression, or a measuredCells that no longer holds, fails here.
    const lines = [];
    const started = Date.now();
    const built = await compile({ source: adder(8), top: 'top', name: 'adder8', core, runYosys, log: (t) => lines.push(t) });
    const seconds = (Date.now() - started) / 1000;
    assert.ok(built.cells >= LIMITS.measuredCells, `the 8-bit adder has ${built.cells} cells, below measuredCells ${LIMITS.measuredCells}`);
    assert.ok(seconds < LIMITS.compileSeconds / 2, `${built.cells} cells took ${seconds} s, not well inside ${LIMITS.compileSeconds} s`);
    assert.ok(lines.some((l) => /cells: more than the \d+ the router had managed/.test(l)), 'no warning in the log for a design above the measured size');
});

// --- the dev server

test('the dev server survives a malformed URL and serves nothing outside its allowlist', async () => {
    const { server, port } = await serve(0);
    try {
        const base = `http://127.0.0.1:${port}`;
        assert.equal((await fetch(`${base}/%`)).status, 404);
        assert.equal((await fetch(`${base}/%E0%A4%A`)).status, 404);
        const page = await fetch(`${base}/`);
        assert.equal(page.status, 200, 'the server is still up after the malformed request');
        const html = await page.text();
        assert.match(html, /NOT AN OFFICIAL MINECRAFT PRODUCT/);
        assert.match(html, /kept only as the placed circuit and its recording while the plot is yours/, 'the footer says what happens to a submitted source');
        assert.equal((await fetch(`${base}/node_modules/puppeteer-core/package.json`)).status, 404);
        assert.equal((await fetch(`${base}/pkg/puppeteer-core@1.0.0/package.json`)).status, 404);
        assert.equal((await fetch(`${base}/pkg/three@0.0.0/package.json`)).status, 200, 'the version in the path is a cache key only');
        assert.equal((await fetch(`${base}/../versions.env`)).status, 404);
    } finally {
        server.close();
    }
    assert.equal(resolve('/%'), null);
    assert.equal(resolve('/node_modules/zod/package.json'), null);
    assert.match(resolve('/pkg/@yowasp/yosys@0.68.1207/gen/bundle.js'), /node_modules\/@yowasp\/yosys\/gen\/bundle\.js$/);
});

test('the dev server keeps the page to its origin with a Content-Security-Policy that allows exactly its inline blocks', async () => {
    const html = readFileSync(join(root, 'playground/index.html'), 'utf8');
    const hash = (text) => `'sha256-${createHash('sha256').update(text).digest('base64')}'`;
    const importMap = html.match(/<script type="importmap">([\s\S]*?)<\/script>/)[1];
    const style = html.match(/<style>([\s\S]*?)<\/style>/)[1];
    const { server, port } = await serve(0);
    try {
        const base = `http://127.0.0.1:${port}`;
        const policy = async (path) => Object.fromEntries((await fetch(`${base}${path}`)).headers.get('content-security-policy').split('; ').map((d) => [d.split(' ')[0], d.split(' ').slice(1)]));
        const page = await policy('/');
        assert.deepEqual(page['default-src'], ["'self'"]);
        assert.deepEqual(page['worker-src'], ["'self'"]);
        assert.deepEqual(page['connect-src'], ["'self'"]);
        assert.deepEqual(page['script-src'], ["'self'", "'wasm-unsafe-eval'", hash(importMap)]);
        assert.deepEqual(page['style-src'], ["'self'", hash(style)]);
        assert.deepEqual(page['object-src'], ["'none'"]);
        // A script or the core carries the policy too (a worker takes its own), without the page's hashes.
        for (const path of ['/worker.js', '/core/redfab_wasm.wasm', '/pkg/three@0.0.0/package.json']) {
            const other = await policy(path);
            assert.deepEqual(other['script-src'], ["'self'", "'wasm-unsafe-eval'"], path);
            assert.deepEqual(other['worker-src'], ["'self'"], path);
        }
        assert.equal((await fetch(`${base}/app.js`)).headers.get('x-content-type-options'), 'nosniff');
    } finally {
        server.close();
    }
    // No other inline script or style can slip in: the hashes come from the markup, one per block.
    assert.equal((html.match(/<script(?![^>]*\ssrc=)[^>]*>/g) || []).length, 1, 'the page has one inline script block, the import map');
    assert.equal((html.match(/<style[^>]*>/g) || []).length, 1, 'the page has one style block');
    assert.ok(!/\sstyle="/.test(html) && !/\son[a-z]+="/.test(html), 'no style or event attributes in the markup');
    assert.equal(contentSecurityPolicy('<p>no blocks</p>'), contentSecurityPolicy(''));
});

test('the dev server lets the browser keep the packages and the core, never the page or a design', async () => {
    const { server, port } = await serve(0);
    try {
        const base = `http://127.0.0.1:${port}`;
        const header = async (path) => (await fetch(`${base}${path}`)).headers.get('cache-control');
        assert.equal(await header('/'), 'no-store');
        assert.equal(await header('/app.js'), 'no-store');
        assert.equal(await header('/designs/not1/top.sv'), 'no-store');
        assert.equal(await header('/pkg/three@0.186.1/package.json'), 'public, max-age=31536000, immutable');
        assert.equal(await header('/node_modules/three/package.json'), 'no-cache');
        assert.equal(await header('/core/redfab_wasm.wasm'), 'no-cache');
        const first = await fetch(`${base}/core/redfab_wasm.wasm`);
        const etag = first.headers.get('etag');
        assert.ok(etag, 'the core carries an ETag');
        const again = await fetch(`${base}/core/redfab_wasm.wasm`, { headers: { 'if-none-match': etag } });
        assert.equal(again.status, 304);
    } finally {
        server.close();
    }
    assert.equal(cacheControl('/pkg/x@1/y.js?q=1'), 'public, max-age=31536000, immutable');
});

// --- the page a visitor meets: the opening circuit, the font, the compiler loaded late

test('the opening circuit is prepared once from the examples and served as the native build wrote it', async () => {
    const first = await prepareOpening({ wasm, name: OPENING });
    assert.equal(OPENING, 'counter4', 'the opening example is the counter: it moves on its own');
    const started = Date.now();
    const again = await prepareOpening({ wasm, name: OPENING });
    assert.equal(again.built, false, 'the second preparation built again');
    assert.ok(Date.now() - started < 1000, 'the second preparation was not answered from the cache');
    assert.equal(again.dir, first.dir);
    const { server, port } = await serve(0);
    try {
        const base = `http://127.0.0.1:${port}`;
        for (const [file, type] of [['design.json', 'application/json'], ['report.txt', 'text/plain; charset=utf-8'], ['world.nbt', 'application/octet-stream']]) {
            const reply = await fetch(`${base}/designs/${OPENING}/${file}`);
            assert.equal(reply.status, 200, file);
            assert.equal(reply.headers.get('content-type'), type, file);
            assert.equal(reply.headers.get('cache-control'), 'no-cache', `${file} is kept by the browser and revalidated`);
            assert.ok(reply.headers.get('etag'), `${file} carries an ETag`);
            const bytes = Buffer.from(await reply.arrayBuffer());
            assert.ok(bytes.equals(readFileSync(join(root, 'build/designs', OPENING, file))), `${file} differs from the native build's`);
        }
        assert.equal((await fetch(`${base}/designs/${OPENING}/top.sv`)).status, 200, 'the source is still served from designs/');
        assert.equal((await fetch(`${base}/designs/${OPENING}/key`)).status, 404, 'the cache key is not served');
        assert.equal((await fetch(`${base}/designs/not1/design.json`)).status, 404, 'only the prepared example has a design to open');
    } finally {
        server.close();
    }
    assert.match(resolve(`/designs/${OPENING}/design.json`), /build\/playground\/opening\/counter4\/design\.json$/);
    assert.match(resolve(`/designs/${OPENING}/top.sv`), /designs\/counter4\/top\.sv$/);
});

test('the page carries its font, its introduction and the repository link, and loads the compiler only when asked', async () => {
    const html = readFileSync(join(root, 'playground/index.html'), 'utf8');
    assert.match(html, /href="https:\/\/github\.com\/zhaojinchu\/ASICraft"/, 'the introduction links to the repository');
    assert.match(html, /@font-face[^}]*Monocraft/, 'the heading font is declared in the page');
    assert.match(html, /Run in the game/, 'the live button says what it does');
    assert.ok(existsSync(join(root, 'playground/fonts/Monocraft-LICENSE.txt')), 'the font ships with its license');
    const worker = readFileSync(join(root, 'playground/worker.js'), 'utf8');
    assert.ok(!/^import .*yosys/m.test(worker), 'the worker imports Yosys at start: 67 MB before the page can show anything');
    assert.match(worker, /import\(['"]\/pkg\/@yowasp\/yosys@/, 'the worker loads Yosys on demand, from the versioned path');
    const { server, port } = await serve(0);
    try {
        const base = `http://127.0.0.1:${port}`;
        const font = await fetch(`${base}/fonts/Monocraft.ttf`);
        assert.equal(font.status, 200);
        assert.equal(font.headers.get('content-type'), 'font/ttf');
        assert.equal(font.headers.get('cache-control'), 'no-cache', 'the font is kept by the browser and revalidated');
        assert.equal((await fetch(`${base}/fonts/Monocraft-LICENSE.txt`)).status, 200);
    } finally {
        server.close();
    }
});

test('the versions the page pins in its package paths are the installed ones', () => {
    const pinned = (text) => [...text.matchAll(/\/pkg\/((?:@[^/@]+\/)?[^/@]+)@([^/]+)\//g)].map((m) => [m[1], m[2]]);
    const pins = [...pinned(readFileSync(join(root, 'playground/index.html'), 'utf8')), ...pinned(readFileSync(join(root, 'playground/worker.js'), 'utf8'))];
    assert.ok(pins.length >= 2, 'the page pins Yosys and three.js');
    for (const [name, version] of pins) {
        assert.ok(PACKAGES.includes(name), `${name} is served`);
        const installed = JSON.parse(readFileSync(join(root, 'node_modules', name, 'package.json'), 'utf8')).version;
        assert.equal(version, installed, `${name}: the page pins ${version}, node_modules has ${installed}; update the path so the browser's cache rolls over`);
    }
});
