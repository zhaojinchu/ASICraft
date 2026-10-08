// The playground page in a headless Chromium (Brave). See `check`.
//
//   node tools/playground_check.mjs [--show]
//
// Serves the playground, opens it, and for each design: loads its source,
// compiles, stops the free-running simulation, applies input vectors through the
// panel (and one through a click on a lever in the 3D view), steps the
// simulator, reads the lamps, and compares with the native simulator on the
// natively built design. Then the page as a user meets it: the status while
// compiling and after, a module not named top, a broken source, the time
// limit's message, the examples dropdown with an edited draft and the draft
// across a reload, Ctrl+Enter, Pause/Run, the three downloads (world.nbt,
// report.txt and design.json, byte for byte the native build's files), the
// dust behind a lever recolouring at once, a clocked design both free-running
// under the page's own clock and driven cycle by cycle against the native
// simulator, live mode against a faked backend, and that nothing the page does
// violates the server's Content-Security-Policy. Screenshots go to
// build/playground/. Needs Brave or Chrome; set REDFAB_BROWSER to another
// Chromium binary.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import puppeteer from 'puppeteer-core';
import { buildWasm, findBrowser, root } from './common.mjs';
import { serve } from './playground_serve.mjs';

const out = join(root, 'build/playground');
mkdirSync(out, { recursive: true });
const show = process.argv.includes('--show');

const browserPath = findBrowser();
if (!browserPath) {
    console.log('no Chromium found; install Brave or Chrome, or set REDFAB_BROWSER');
    process.exit(1);
}

// REDFAB_NO_BUILD=1 skips the cargo builds and uses the core and the CLI as
// last built, to iterate on the page while the crates are being worked on.
const noBuild = process.env.REDFAB_NO_BUILD === '1';
if (!noBuild) buildWasm();

/// Runs the CLI with the arguments and returns its stdout.
function redfab(args) {
    const built = ['target/release/redfab', 'target/debug/redfab'].map((p) => join(root, p)).find((p) => existsSync(p));
    if (noBuild && built) return execFileSync(built, args, { cwd: root });
    return execFileSync('cargo', ['run', '-q', '-p', 'redfab-cli', '--', ...args], { cwd: root });
}

/// The native simulator's outputs for the vectors, on the natively built design.
function nativeOutputs(name, vectors) {
    const file = join(out, `${name}.vectors.json`);
    writeFileSync(file, JSON.stringify(vectors));
    return JSON.parse(redfab(['simulate', join(root, 'build/designs', name, 'design.json'), file]).toString());
}

// Combinational examples: every input combination is compared with the native simulator.
const DESIGNS = ['not1', 'nor2', 'fa1', 'consts'];
const INV = 'module inv(input logic a, output logic y);\n  assign y = ~a;\nendmodule\n';
const BROKEN = 'module top(input logic a, output logic y)\n  assign y = ~a;\nendmodule\n';
const ESCAPED = 'module top(input logic a, output logic \\y<b> );\n  assign \\y<b> = ~a;\nendmodule\n';

const { server, port } = await serve(0);
const base = `http://127.0.0.1:${port}`;
const browser = await puppeteer.launch({ executablePath: browserPath, headless: !show, args: ['--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--no-sandbox'] });
const failures = [];
const check = (ok, message) => { if (!ok) failures.push(message); };
const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms));

/// Opens the page fresh and waits until it is ready. A Content-Security-Policy
/// violation anywhere on the page is reported as an error.
async function open(page, errors) {
    await page.evaluateOnNewDocument(() => {
        document.addEventListener('securitypolicyviolation', (e) => console.error(`CSP violation: ${e.violatedDirective} blocked ${e.blockedURI}`));
    });
    await page.goto(`${base}/`);
    try {
        await page.waitForFunction(() => window.redfab !== undefined, { timeout: 30_000 });
    } catch {
        throw new Error(`the page did not start: ${errors.join(' | ') || 'no errors reported'}`);
    }
    await page.evaluate(() => window.redfab.ready());
}

/// Clicks Compile and returns [className, text] of the status once it settles,
/// and every status text seen on the way.
async function compileAndWait(page, timeout = 120_000) {
    const seen = [];
    await page.click('#compile');
    const started = Date.now();
    while (Date.now() - started < timeout) {
        const [kind, text] = await page.$eval('#status', (e) => [e.className, e.textContent]);
        if (!seen.includes(text)) seen.push(text);
        if (kind !== '') return { kind, text, seen };
        await sleep(100);
    }
    throw new Error('the compile did not finish');
}

/// Loads an example through the page's own path (dropdown included).
async function loadExample(page, name) {
    await page.evaluate((n) => window.redfab.loadExample(n), name);
    await page.select('#examples', name);
}

/// Lets the free-running page reach tick `min` or later, stops it there and
/// returns the tick it stopped at (a few ticks past `min` at most), once the
/// step in flight has landed.
async function stopAt(page, min) {
    const started = Date.now();
    while (Date.now() - started < 60_000) {
        const at = await page.evaluate(async (m) => {
            const r = window.redfab;
            if (r.tick < m) return null;
            r.stop();
            await r.idle();
            return r.tick;
        }, min);
        if (at !== null) return at;
        await sleep(20);
    }
    throw new Error(`the simulation did not reach tick ${min}`);
}

/// Clicks a download button and returns the file's bytes once it has landed.
async function downloaded(page, button, file) {
    rmSync(file, { force: true });
    await page.click(button);
    for (let i = 0; i < 50 && !existsSync(file); i++) await sleep(100);
    await sleep(200);
    return existsSync(file) ? readFileSync(file) : null;
}

try {
    const page = await browser.newPage();
    await page.setViewport({ width: 1400, height: 900 });
    const errors = [];
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
    const dialogs = [];
    let answer = true;
    page.on('dialog', (d) => { dialogs.push(d.message()); d[answer ? 'accept' : 'dismiss'](); });
    await open(page, errors);
    check(/^Ready\. About \d+ cells \(an 8-bit adder\) compile inside the \d+ s limit\.$/.test(await page.$eval('#status', (e) => e.textContent)), 'the page does not state the size that works before a compile');

    // --- the examples: every input combination against the native simulator
    for (const name of DESIGNS) {
        await loadExample(page, name);
        const status = await compileAndWait(page);
        if (status.kind !== 'ok') {
            failures.push(`${name}: compile failed: ${status.text}`);
            continue;
        }
        check(/^Built \w+: \d+ cells \(the router manages about \d+ today\); \d+ blocks in \d+ x \d+; outputs settle within \d+ ticks\.$/.test(status.text), `${name}: the built status reads ${JSON.stringify(status.text)}`);
        await page.evaluate(() => window.redfab.stop());
        const design = await page.evaluate(() => window.redfab.design);
        const inputs = design.inputs.map((p) => p.name);
        // Every combination, in Gray order so each step flips one input.
        const vectors = [];
        for (let k = 0; k < 1 << inputs.length; k++) {
            const gray = k ^ (k >> 1);
            vectors.push(Object.fromEntries(inputs.map((n, i) => [n, (gray >> i) & 1])));
        }
        const expected = nativeOutputs(name, vectors);
        const current = Object.fromEntries(inputs.map((n) => [n, 0]));
        const got = [];
        for (const [k, vector] of vectors.entries()) {
            for (const n of inputs) {
                if (vector[n] !== current[n]) {
                    if (k === 1) {
                        // The first flip goes through a click on the lever in the 3D view.
                        const at = await page.evaluate((n) => window.redfab.leverOnScreen(n), n);
                        await page.mouse.click(at.x, at.y);
                    } else {
                        await page.click(`[data-input="${n}"]`);
                    }
                    current[n] = vector[n];
                }
            }
            const states = await page.evaluate(() => window.redfab.inputs());
            if (states.some((s, i) => s !== vector[inputs[i]])) {
                failures.push(`${name}: after setting ${JSON.stringify(vector)} the page shows inputs ${states}`);
            }
            await page.evaluate((t) => window.redfab.step(t), design.settle_ticks + 2);
            got.push(await page.evaluate(() => window.redfab.lamps()));
        }
        const wrong = vectors.filter((_, k) => JSON.stringify(got[k]) !== JSON.stringify(expected[k]));
        if (wrong.length) {
            failures.push(`${name}: ${wrong.length} of ${vectors.length} vectors show the wrong lamps, e.g. ${JSON.stringify(wrong[0])}: page ${JSON.stringify(got[vectors.indexOf(wrong[0])])}, native ${JSON.stringify(expected[vectors.indexOf(wrong[0])])}`);
        } else {
            console.log(`${name}: ${vectors.length} input combinations show the right lamps in the browser`);
        }
        await page.screenshot({ path: join(out, `${name}.png`) });
    }

    // --- the dust behind a lever recolours the moment the lever flips, before any tick
    {
        await loadExample(page, 'not1');
        const status = await compileAndWait(page);
        check(status.kind === 'ok', `not1 again: ${status.text}`);
        await page.evaluate(() => window.redfab.stop());
        await page.evaluate((t) => window.redfab.step(t), 40);
        const changed = await page.evaluate(async () => {
            const dust = Object.keys(window.redfab.design.blocks).filter((p) => window.redfab.design.blocks[p].startsWith('minecraft:redstone_wire'));
            const before = dust.map((p) => window.redfab.colourAt(p));
            await window.redfab.toggle(0);
            const after = dust.map((p) => window.redfab.colourAt(p));
            return dust.filter((_, i) => before[i] !== after[i]).length;
        });
        check(changed > 0, 'not1: no dust changed colour when the lever flipped (before any tick)');
        if (changed > 0) console.log(`not1: ${changed} dust blocks recoloured at once when the lever flipped`);
        // Pause and Run: the tick stands still while paused and moves again after.
        await page.click('#run');
        check(await page.evaluate(() => window.redfab.running), 'Run did not start the simulation');
        await sleep(500);
        await page.click('#run');
        const paused = await page.evaluate(async () => { await window.redfab.idle(); return window.redfab.tick; });
        await sleep(300);
        check(paused === await page.evaluate(() => window.redfab.tick), 'the tick moved while paused');
        check(paused > 40, 'Run did not advance the tick');
        // A hand-driven step waits for the step in flight instead of being dropped.
        const stepped = await page.evaluate(async () => {
            const r = window.redfab;
            const before = r.tick;
            const [, after] = await Promise.all([r.step(3), r.step(4)]);
            return after - before;
        });
        check(stepped === 7, `two hand-driven steps of 3 and 4 ticks advanced the tick by ${stepped}, not 7`);
    }

    // --- the downloads are the native build's files, byte for byte
    {
        const client = await page.createCDPSession();
        await client.send('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: out, eventsEnabled: true });
        for (const [button, file, nativeFile] of [['#download', 'not1.nbt', 'world.nbt'], ['#report', 'not1.report.txt', 'report.txt'], ['#design-json', 'not1.design.json', 'design.json']]) {
            const bytes = await downloaded(page, button, join(out, file));
            if (!bytes) failures.push(`the ${nativeFile} button downloaded nothing`);
            else check(bytes.equals(readFileSync(join(root, 'build/designs/not1', nativeFile))), `the downloaded ${file} differs from the native ${nativeFile}`);
        }
        await client.detach();
    }

    // --- a module not named top, the top field left to the page; port names are text, never markup
    {
        await page.evaluate((t) => window.redfab.setSource(t), INV);
        check(await page.$eval('#examples', (e) => e.value) === 'custom', 'editing the source did not select the custom entry');
        const status = await compileAndWait(page);
        check(status.kind === 'ok' && status.text.startsWith('Built inv: 1 cells'), `inv: ${status.text}`);
        check(await page.evaluate(() => window.redfab.built?.top) === 'inv', 'the top module was not detected as inv');
        await page.evaluate((t) => window.redfab.setSource(t), ESCAPED);
        const escaped = await compileAndWait(page);
        check(escaped.kind === 'ok', `escaped identifier: ${escaped.text}`);
        const lamp = await page.$eval('#outputs', (e) => [e.querySelector('b') === null, e.querySelector('.lamp')?.textContent]);
        check(lamp[0] && lamp[1] === 'y<b>', `an escaped port name was rendered as markup: ${JSON.stringify(lamp)}`);
    }

    // --- a broken source: the first error line, without colour codes, as the status
    {
        await page.evaluate((t) => window.redfab.setSource(t), BROKEN);
        const status = await compileAndWait(page);
        check(status.kind === 'bad' && /^top\.sv:\d+:\d+: error: expected ';'/.test(status.text), `broken source: status ${JSON.stringify(status.text)}`);
        const logText = await page.$eval('#log', (e) => e.textContent);
        check(!status.text.includes('\x1b') && !logText.includes('\x1b'), 'colour codes reached the page');
        check(await page.evaluate(() => window.redfab.design) === null, 'a failed compile left a design');
        // Nothing of the previous design stays on screen.
        check(await page.evaluate(() => window.redfab.colourAt('1,1,2')) === null, 'the view still shows the previous design after a failed compile');
        // The module that is not there: told in plain words.
        await page.evaluate((t) => window.redfab.setSource(t), INV);
        await page.evaluate(() => window.redfab.setTop('top'));
        const noTop = await compileAndWait(page);
        check(noTop.kind === 'bad' && noTop.text.startsWith('the source has no module named "top"'), `wrong top: ${noTop.text}`);
        await page.evaluate(() => window.redfab.setTop(''));
    }

    // --- the time limit's message survives the worker restart, and the worker comes back
    {
        await loadExample(page, 'adder4');
        await page.evaluate(() => { window.redfab.limits.compileSeconds = 1; });
        const status = await compileAndWait(page);
        check(status.kind === 'bad' && status.text === 'Compiling took longer than 1 s; stopped. Try a smaller design.', `time limit: ${JSON.stringify(status.text)}`);
        check(status.seen.some((t) => /^Compiling adder4: (synthesizing|proving|placing and routing), \d+ s$/.test(t)), `no phase and elapsed time shown while compiling: ${JSON.stringify(status.seen)}`);
        check(await page.$eval('#compile', (e) => !e.disabled), 'Compile stayed disabled after the time limit');
        await page.evaluate(() => { window.redfab.limits.compileSeconds = 90; });
        await loadExample(page, 'not1');
        const again = await compileAndWait(page);
        check(again.kind === 'ok', `after the time limit: ${again.text}`);
    }

    // --- Ctrl+Enter compiles
    {
        await page.evaluate(() => window.redfab.stop());
        await page.focus('#source');
        await page.keyboard.down('Control');
        await page.keyboard.press('Enter');
        await page.keyboard.up('Control');
        const started = Date.now();
        let status;
        do {
            status = await page.$eval('#status', (e) => [e.className, e.textContent]);
            await sleep(100);
        } while (status[0] === '' && Date.now() - started < 60_000);
        check(status[0] === 'ok', `Ctrl+Enter did not compile: ${status[1]}`);
    }

    // --- the dropdown: an edited source is a draft, replacing it asks, and it survives a reload
    {
        const draft = INV.replace('inv', 'mine');
        await page.evaluate((t) => window.redfab.setSource(t), draft);
        answer = false;
        await page.select('#examples', 'nor2');
        check(dialogs.at(-1)?.includes('nor2 example'), `no confirmation before replacing the draft: ${JSON.stringify(dialogs)}`);
        check(await page.$eval('#source', (e) => e.value) === draft, 'dismissing the confirmation still replaced the source');
        check(await page.$eval('#examples', (e) => e.value) === 'custom', 'dismissing the confirmation moved the dropdown');
        answer = true;
        await page.select('#examples', 'nor2');
        await sleep(300);
        check(await page.$eval('#source', (e) => e.value.includes('module top')), 'accepting the confirmation did not load the example');
        await page.select('#examples', 'custom');
        await sleep(100);
        check(await page.$eval('#source', (e) => e.value) === draft, 'the custom entry did not bring the draft back');
        await open(page, errors);
        check(await page.$eval('#source', (e) => e.value) === draft, 'the draft did not survive a reload');
        check(await page.$eval('#examples', (e) => e.value) === 'custom', 'the dropdown does not show custom after a reload with a draft');
        check(await page.$eval('footer', (e) => e.textContent.includes('NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT.')), 'the footer lacks the Minecraft notice');
    }

    // --- a clocked design: driven by hand through the native cycle protocol, then
    // free-running under the page's own clock, counted in simulated ticks
    {
        await loadExample(page, 'counter4');
        const status = await compileAndWait(page, 180_000);
        if (status.kind !== 'ok') {
            failures.push(`counter4: compile failed: ${status.text}`);
        } else {
            check(/clock clk at \d+ ticks a cycle\.$/.test(status.text), `counter4: the built status reads ${JSON.stringify(status.text)}`);
            await page.evaluate(() => window.redfab.stop());
            const design = await page.evaluate(() => window.redfab.design);
            const seq = design.timing.sequential;
            const index = Object.fromEntries(design.inputs.map((p, i) => [p.name, i]));
            // By hand, the native cycle protocol: the clock at rest for a period (any
            // pulse still in the clock tree flushes), then per cycle the active edge, the
            // inputs, the other edge, and the lamps read at sample_at.
            const driven = [{ en: 1, rst: 1 }, { en: 1, rst: 0 }, { en: 1, rst: 0 }, { en: 0, rst: 0 }, { en: 1, rst: 0 }, { en: 1, rst: 0 }, { en: 1, rst: 1 }, { en: 1, rst: 0 }];
            // Cycle 6's rst is clicked in the middle of cycle 5 instead of at its start:
            // the page must hold the click until cycle 6's input window (as the server
            // does in live mode), so the native run sees it in cycle 6 and not before.
            const lateClick = { cycle: 5, name: 'rst' };
            // Then the page runs on with the inputs the last cycle left (en 1, rst 0):
            // the same protocol, so the native run simply continues.
            const free = 4;
            const expected = nativeOutputs('counter4', [...driven, ...Array.from({ length: free }, () => ({ en: 1, rst: 0 }))]);
            const { results: got, held } = await page.evaluate(async (vectors, seq, index, late) => {
                const r = window.redfab;
                const button = (name) => document.querySelector(`[data-input="${CSS.escape(name)}"]`);
                const observe = (what) => ({ what, tick: r.tick, input: r.inputs()[index[late.name]], pending: r.pendingInputs(), marked: button(late.name).dataset.pending });
                await r.clock(seq.active !== 'rising');
                // To the next period boundary after a full period at rest, so the free
                // run below continues on the page's own grid.
                await r.step(2 * seq.period - (r.tick % seq.period));
                const results = [];
                const held = [];
                for (const [k, vector] of vectors.entries()) {
                    await r.clock(seq.active === 'rising');
                    if (k === late.cycle + 1) held.push(observe('at the next input window'));
                    const state = r.inputs();
                    const pending = r.pendingInputs();
                    for (const [name, value] of Object.entries(vector)) {
                        const effective = name in pending ? Number(pending[index[name]]) : state[index[name]];
                        if (effective !== value) await r.toggle(index[name]);
                    }
                    await r.step(seq.high);
                    await r.clock(seq.active !== 'rising');
                    if (k === late.cycle) {
                        // After the release, well past the input window: the click is held.
                        await r.step(3);
                        await r.toggle(index[late.name]);
                        held.push(observe('right after the click'));
                        // A second click before the window takes the request back, a third makes it again.
                        await r.toggle(index[late.name]);
                        held.push(observe('after a second click'));
                        await r.toggle(index[late.name]);
                        await r.step(seq.sample_at - seq.high - 3);
                    } else {
                        await r.step(seq.sample_at - seq.high);
                    }
                    results.push(r.lamps());
                    if (k === late.cycle) held.push(observe('at the sample tick'));
                    await r.step(seq.period - seq.sample_at);
                }
                return { results, held };
            }, driven, seq, index, lateClick);
            {
                const rst = index[lateClick.name];
                const [clicked, cancelled, sampled, atWindow] = held;
                check(clicked.input === 0 && clicked.pending[rst] === true && clicked.marked === '1',
                    `counter4: a click at cycle tick ${clicked.tick % seq.period} moved ${lateClick.name} at once (input ${clicked.input}, pending ${JSON.stringify(clicked.pending)}, marked ${clicked.marked}); it must wait for cycle tick ${seq.input_change_at}`);
                check(cancelled.input === 0 && !(rst in cancelled.pending) && cancelled.marked === '0', `counter4: a second click did not take the held request back: ${JSON.stringify(cancelled)}`);
                check(sampled.input === 0 && sampled.pending[rst] === true, `counter4: the held click moved ${lateClick.name} before the input window: ${JSON.stringify(sampled)}`);
                check(atWindow.input === 1 && !(rst in atWindow.pending) && atWindow.marked === '0' && atWindow.tick % seq.period === seq.input_change_at,
                    `counter4: the held click was not applied at cycle tick ${seq.input_change_at}: ${JSON.stringify(atWindow)}`);
            }
            const start = await page.evaluate(() => window.redfab.tick);
            check(start % seq.period === 0, `counter4: the hand-driven cycles ended at tick ${start}, not on a period boundary`);
            // Free-running: the page applies the edges itself. Each cycle is caught
            // between its release and its sample tick (no edge in between, so a
            // hand-driven step to the sample tick is exact), and the first cycle is
            // caught inside its pulse as well, so the lever is seen at both levels
            // and Run after Pause inside a pulse must keep the level.
            const edge = (k) => start + k * seq.period;
            const level = (on) => (on === (seq.active === 'rising') ? 1 : 0);
            await page.click('#run');
            check(await page.evaluate(() => window.redfab.running), 'counter4: Run did not start the simulation');
            for (let k = 0; k < free; k++) {
                if (k === 0) {
                    const inPulse = await stopAt(page, edge(k) + 1);
                    check(inPulse < edge(k) + seq.high, `counter4: stopped at tick ${inPulse}, past the pulse that ends at ${edge(k) + seq.high}`);
                    const [lever, label, implied] = await page.evaluate((i) => [window.redfab.inputs()[i], document.getElementById('tick').textContent, window.redfab.clockLevelAt(window.redfab.tick)], index[seq.clock]);
                    check(lever === level(true), `counter4: at tick ${inPulse}, cycle tick ${inPulse % seq.period} of a ${seq.high}-tick pulse, the clock lever shows ${lever}`);
                    check(implied === (level(true) === 1), `counter4: the page's own clockLevelAt disagrees with the pulse at tick ${inPulse}`);
                    check(/cycle tick \d+ of \d+/.test(label), `the tick label does not show the clock phase: ${label}`);
                    await page.click('#run');
                    const resumed = await page.evaluate((i) => [window.redfab.tick, window.redfab.inputs()[i]], index[seq.clock]);
                    check(resumed[1] === level(true), `counter4: Run at tick ${resumed[0]}, inside the pulse, put the clock lever at ${resumed[1]}: the pulse was cut short`);
                }
                const released = await stopAt(page, edge(k) + seq.high);
                check(released <= edge(k) + seq.sample_at, `counter4: stopped at tick ${released}, past cycle ${k}'s sample tick ${edge(k) + seq.sample_at}`);
                const lever = await page.evaluate((i) => window.redfab.inputs()[i], index[seq.clock]);
                check(lever === level(false), `counter4: at tick ${released}, cycle tick ${released % seq.period}, after the ${seq.high}-tick pulse, the clock lever shows ${lever}`);
                await page.evaluate((t) => window.redfab.step(t), edge(k) + seq.sample_at - released);
                got.push(await page.evaluate(() => window.redfab.lamps()));
                await page.click('#run');
            }
            await page.evaluate(() => window.redfab.stop());
            const wrong = expected.map((_, k) => k).filter((k) => JSON.stringify(got[k]) !== JSON.stringify(expected[k]));
            if (wrong.length) failures.push(`counter4: ${wrong.length} of ${expected.length} cycles show the wrong count, e.g. cycle ${wrong[0]} (${wrong[0] < driven.length ? 'driven by hand' : 'free-running'}): page ${JSON.stringify(got[wrong[0]])}, native ${JSON.stringify(expected[wrong[0]])}`);
            else console.log(`counter4: ${driven.length} cycles driven by hand and ${free} free-running under the page's clock show the native simulator's counts`);
            await page.screenshot({ path: join(out, 'counter4.png') });
        }
    }

    if (errors.length) failures.push(`page errors: ${errors.slice(0, 3).join(' | ')}`);
    errors.length = 0;

    // --- live mode against a faked backend: the plot size is shown and enforced, the plot is
    // given back on Stop and when the page goes away, and a failed run leaves nothing behind
    {
        const requests = [];
        await page.setRequestInterception(true);
        let submitAnswer = { status: 503, body: { error: 'the game is down' } };
        page.on('request', (req) => {
            const url = new URL(req.url());
            if (url.pathname === '/api/status') return req.respond({ status: 200, contentType: 'application/json', body: JSON.stringify({ live: true, limits: { plotSize: [16, 16] } }) });
            if (url.pathname.startsWith('/api/')) {
                requests.push(`${req.method()} ${url.pathname}${url.search}`);
                if (url.pathname === '/api/submit') return req.respond({ status: submitAnswer.status, contentType: 'application/json', body: JSON.stringify(submitAnswer.body) });
                return req.respond({ status: 200, contentType: 'application/json', body: '{"released":true}' });
            }
            return req.continue();
        });
        await open(page, errors);
        check(await page.$eval('#live', (e) => !e.hidden), 'the live button is hidden although the backend says live');
        await loadExample(page, 'not1');
        const status = await compileAndWait(page);
        check(status.kind === 'warn' && status.text.includes("too large for the server's 16 x 16 plot"), `plot size: ${JSON.stringify(status.text)}`);
        await page.evaluate(() => window.redfab.stop());
        await page.click('#live');
        await sleep(300);
        const refused = await page.$eval('#status', (e) => [e.className, e.textContent]);
        check(refused[0] === 'bad' && /^This design is \d+ by \d+ blocks; the server's plots are 16 by 16\. Not sent\.$/.test(refused[1]), `too large for the plot: ${JSON.stringify(refused)}`);
        check(!requests.some((r) => r.startsWith('POST /api/submit')), 'a design too large for the plot was still submitted');
        check(await page.evaluate(() => window.redfab.design !== null && window.redfab.mode === 'sim'), 'a refused submission disturbed the local simulation');
        // The server refuses: the view and the panel are left empty.
        await page.evaluate(() => { window.redfab.limits.plotSize = [512, 512]; });
        await page.click('#live');
        await page.waitForFunction(() => document.getElementById('status').className === 'bad', { timeout: 10_000 });
        check(await page.$eval('#status', (e) => e.textContent) === 'the game is down', 'the server error was not shown');
        check(requests.some((r) => r.startsWith('POST /api/submit')), 'the design was not submitted');
        check(await page.evaluate(() => window.redfab.design === null && window.redfab.colourAt('1,1,2') === null && window.redfab.mode === 'sim'), 'a failed live run left a design or a view behind');
        check(await page.$eval('#inputs', (e) => e.querySelectorAll('.lever').length) === 0, 'a failed live run left levers in the panel');
        // The server accepts: the download comes from the matching local build, Stop gives the plot back.
        const design = await page.evaluate(() => window.redfab.built.design);
        submitAnswer = { status: 200, body: { plot: 3, token: 'tok', design, report: 'design not1', origin: [0, 64, 0], placed_tick: 1, state: 'settling' } };
        await page.click('#live');
        await page.waitForFunction(() => window.redfab.livePlot !== null, { timeout: 10_000 });
        check(await page.$eval('#download', (e) => !e.disabled), 'the world.nbt button is disabled in live mode');
        check(await page.$eval('#inputs', (e) => e.querySelectorAll('.lever').length) === 1, 'live mode shows no levers');
        requests.length = 0;
        await page.click('#release');
        await sleep(300);
        check(requests.includes('DELETE /api/plots/3?token=tok'), `Stop did not give the plot back: ${JSON.stringify(requests)}`);
        check(await page.evaluate(() => window.redfab.design === null && window.redfab.colourAt('1,1,2') === null), 'Stop left the design on screen');
        // Leaving the page gives the plot back too.
        await page.click('#live');
        await page.waitForFunction(() => window.redfab.livePlot !== null, { timeout: 10_000 });
        requests.length = 0;
        await page.evaluate(() => window.dispatchEvent(new Event('pagehide')));
        await sleep(300);
        check(requests.includes('DELETE /api/plots/3?token=tok'), `leaving the page did not give the plot back: ${JSON.stringify(requests)}`);
        console.log('live mode against a faked backend: plot size shown and enforced, plot given back on Stop and on leaving');
    }

    // The faked backend refuses once and has no WebSocket; the console reports both.
    // Anything else, a Content-Security-Policy violation included, is a failure.
    const unexpected = errors.filter((e) => !/503|WebSocket connection/.test(e));
    if (unexpected.length) failures.push(`page errors: ${unexpected.slice(0, 3).join(' | ')}`);
} finally {
    await browser.close();
    server.close();
}
for (const f of failures) console.log(`FAIL: ${f}`);
if (!failures.length) console.log(`screenshots in ${out}`);
process.exit(failures.length ? 1 : 0);
