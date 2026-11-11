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
// violates the server's Content-Security-Policy. Also the page as a visitor
// meets it: it opens with the counter already built and counting before the
// compiler is fetched, the heading font loads, the repository is linked, a bus
// reads as a value most significant bit first, the result card shows the
// numbers with the report open and the log closed, the main action follows
// whether a game is behind the page, and a phone gets no horizontal scroll and
// levers big enough to touch. Screenshots go to build/playground/. Needs Brave
// or Chrome; set REDFAB_BROWSER to another Chromium binary.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import puppeteer from 'puppeteer-core';
import { buildWasm, findBrowser, root, wasmPath } from './common.mjs';
import { OPENING, prepareOpening, serve } from './playground_serve.mjs';

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

await prepareOpening({ wasm: wasmPath, name: OPENING });
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
    // The page opens with a circuit, or says it could not.
    await page.waitForFunction(() => window.redfab.opening !== 'loading', { timeout: 30_000 });
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
    const fetched = [];
    page.on('request', (r) => fetched.push(new URL(r.url()).pathname));
    const dialogs = [];
    let answer = true;
    page.on('dialog', (d) => { dialogs.push(d.message()); d[answer ? 'accept' : 'dismiss'](); });
    await open(page, errors);

    // --- the page opens with the counter already built and counting, before the compiler is fetched
    {
        check(await page.evaluate(() => window.redfab.opening) === 'shown', `the page did not open with the ${OPENING} circuit`);
        check(await page.evaluate((n) => window.redfab.design?.name === n && window.redfab.mode === 'sim' && window.redfab.running, OPENING), 'the opening circuit is not running in the simulator');
        check(await page.$eval('#examples', (e) => e.value) === OPENING, 'the dropdown does not show the opening example');
        const status = await page.$eval('#status', (e) => [e.className, e.textContent]);
        // The status line says what is on show in plain words; the numbers are the result card's (asked for 2026-11-11).
        // The status is the terminal's first line, as a build tool prints it (2026-11-11: the green sentence read as generated).
        check(status[0] === 'ok' && status[1] === `simulating ${OPENING} in the browser`, `the opening status reads ${JSON.stringify(status)}`);
        check(await page.$eval('#status', (e) => e.parentElement.id) === 'terminal' && await page.$('#summary') === null, 'the status is not the terminal\'s first line, or the chips are back');
        check(/^\s*export\s+world\.nbt\s+report\.txt\s+design\.json\s*$/.test(await page.$eval('#result', (e) => e.textContent)), 'the files are not labelled as exports');
        check(await page.$('#capacity') === null, 'the capacity line is back under the editor');
        // With no camera the tabs are hidden, and the circuit still fills the stage (it once collapsed to nothing).
        check(await page.$eval('#tabs', (e) => e.hidden) && await page.$eval('#view', (e) => e.getBoundingClientRect().height) > 300, 'without a camera the circuit view does not fill the stage');
        check(!fetched.some((p) => p.includes('/pkg/@yowasp/yosys')), 'the compiler was fetched before anyone asked to compile');
        // A still view draws nothing: with the simulation paused, no frame in half a second.
        await page.evaluate(async () => { window.redfab.stop(); await window.redfab.idle(); });
        await sleep(300);
        const still = await page.evaluate(() => window.redfab.frames);
        await sleep(500);
        check(await page.evaluate(() => window.redfab.frames) === still, 'the view keeps drawing frames with nothing changing');
        await page.evaluate(() => document.getElementById('run').click());
        check(await page.$('a[href="https://github.com/zhaojinchu/ASICraft"]') !== null, 'no link to the repository');
        check(await page.evaluate(async () => { await document.fonts.ready; return document.fonts.check('20px Monocraft'); }), 'the heading font did not load');
        check(await page.$eval('#mode', (e) => e.textContent.trim()) === 'simulated here', 'the mode badge does not say simulated here');
        check(await page.$eval('#compile', (e) => e.classList.contains('primary')) && await page.$eval('#live', (e) => e.hidden), 'without a game behind the page, Compile is not the main action');
        // The counter counts on its own: the page switched its enable on, and the count row reads as a value.
        const value = () => page.$eval('.bus[data-bus="count"] .value', (e) => e.textContent.trim());
        const started = Date.now();
        const before = await value();
        let after = before;
        while (after === before && Date.now() - started < 15_000) {
            await sleep(500);
            after = await value();
        }
        check(/^[01]{4} \(\d+\)$/.test(after), `the count row reads ${JSON.stringify(after)}`);
        check(after !== before, `the count did not move in 15 s (${before})`);
        const bus = await page.evaluate(() => {
            const row = document.querySelector('.bus[data-bus="count"]');
            const lamps = [...row.querySelectorAll('.lamp')];
            const bits = lamps.map((l) => l.dataset.lit).join('');
            return { bits, text: row.querySelector('.value').textContent.trim(), order: lamps.map((l) => l.dataset.output).join(','), value: parseInt(bits, 2) };
        });
        check(bus.order === 'count[3],count[2],count[1],count[0]', `the bus lamps are not most-significant-first: ${bus.order}`);
        check(bus.text === `${bus.bits} (${bus.value})`, `the bus value ${JSON.stringify(bus.text)} does not read its lamps ${bus.bits}`);
        // The opening circuit's structure downloads as the native build wrote it.
        const client = await page.createCDPSession();
        await client.send('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: out, eventsEnabled: true });
        const bytes = await downloaded(page, '#download', join(out, `${OPENING}.nbt`));
        if (!bytes) failures.push('the opening circuit offered no world.nbt');
        else check(bytes.equals(readFileSync(join(root, 'build/designs', OPENING, 'world.nbt'))), "the opening circuit's world.nbt differs from the native build's");
        await client.detach();
        // The report on the page is a timing report's: area, T_clk, t_co, slack, LEC; no placement internals,
        // no positions, no asides (the full one is the report.txt export).
        const report = await page.$eval('#report-text', (e) => e.textContent);
        const line = (key, value) => new RegExp(`^ {2}${key.padEnd(12).replace(/[[\]]/g, '\\$&')} ${value}$`, 'm').test(report);
        check(report.startsWith(`${OPENING}, sequential, clk ↑\n`) && line('cells', '\\d+') && line('flip-flops', '4') && line('area', '\\d+ x \\d+ x \\d+')
            && line('T_clk', '\\d+ ticks, [\\d.]+ s') && line('t_co', '\\d+ ticks') && line('slack', 'setup -?\\d+, hold -?\\d+ ticks')
            && line('outputs', 'count\\[3:0\\]') && line('inputs', 'clk en rst'), `the page's report reads ${JSON.stringify(report)}`);
        check(!/[()]|forceload|lever at|lamp at|placement|tiles:/.test(report), `the page's report carries the full report's detail: ${JSON.stringify(report)}`);
        // Showing intent to compile (a click into the editor) fetches the compiler ahead of the compile.
        await page.focus('#source');
        const warmStarted = Date.now();
        while (!fetched.some((p) => p.endsWith('/yosys.core.wasm')) && Date.now() - warmStarted < 10_000) await sleep(100);
        check(fetched.some((p) => p.endsWith('/yosys.core.wasm')), 'a click into the editor did not fetch the compiler ahead of a compile');
        console.log(`the page opened with ${OPENING} counting (${before}, then ${after}) before the compiler was fetched, which a click into the editor fetched`);
    }

    // --- the examples: every input combination against the native simulator
    for (const name of DESIGNS) {
        await loadExample(page, name);
        const status = await compileAndWait(page);
        if (status.kind !== 'ok') {
            failures.push(`${name}: compile failed: ${status.text}`);
            continue;
        }
        check(new RegExp(`^built ${name} in [\\d.]+ s$`).test(status.text), `${name}: the built status reads ${JSON.stringify(status.text)}`);
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
    check(fetched.some((p) => p.includes('/pkg/@yowasp/yosys')), 'the compiler was never fetched for a compile');

    // --- a bus reads as a value, most significant bit first; the result card shows the numbers and the report
    {
        await loadExample(page, 'adder4');
        const status = await compileAndWait(page, 180_000);
        check(status.kind === 'ok', `adder4: ${status.text}`);
        await page.evaluate(() => window.redfab.stop());
        for (const n of ['a[0]', 'a[1]', 'b[2]']) await page.click(`[data-input="${n}"]`);
        await page.evaluate(async () => window.redfab.step(window.redfab.design.settle_ticks + 2));
        const rows = await page.evaluate(() => Object.fromEntries([...document.querySelectorAll('.bus')].map((b) => [b.dataset.bus, {
            value: b.querySelector('.value').textContent.trim(),
            order: [...b.querySelectorAll('[data-input],[data-output]')].map((e) => e.dataset.input ?? e.dataset.output).join(','),
        }])));
        check(rows.a?.value === '0011 (3)' && rows.b?.value === '0100 (4)', `the input buses read ${JSON.stringify(rows)}`);
        check(rows.sum?.value === '0111 (7)', `the sum reads ${JSON.stringify(rows.sum)}`);
        check(rows.a?.order === 'a[3],a[2],a[1],a[0]' && rows.sum?.order === 'sum[3],sum[2],sum[1],sum[0]', `the buses are not most-significant-first: ${JSON.stringify(rows)}`);
        check(await page.$('#outputs > .lamp[data-output="cout"]') !== null && await page.$('#inputs > .lever[data-input="cin"]') !== null, 'a single-bit pin was put in a bus');
        const card = await page.evaluate(() => ({
            report: document.getElementById('report-text').textContent,
            reportOpen: document.getElementById('report-details').open,
            logOpen: document.getElementById('log-details').open,
        }));
        check(/^ {2}cells {8}49$/m.test(card.report) && card.reportOpen && !card.logOpen, `the report is ${card.reportOpen ? 'open' : 'closed'} (${/cells {8}49/.test(card.report) ? 'with' : 'without'} its text), the log ${card.logOpen ? 'open' : 'closed'}`);
        check(card.report.startsWith('adder4, combinational\n') && /^ {2}t_pd {9}(min \d+, max )?\d+ ticks, [\d.]+ s$/m.test(card.report) && /^ {2}t_in,min {5}\d+ ticks$/m.test(card.report)
            && /^ {2}inputs {7}a\[3:0\] b\[3:0\] cin$/m.test(card.report) && /^ {2}LEC {10}proven$/m.test(card.report), `adder4's report reads ${JSON.stringify(card.report)}`);
        console.log('adder4: the buses read 3 + 4 = 7, most significant bit first; the report gives t_pd, t_in,min and the LEC');
    }

    // --- a phone: nothing wider than the screen, the circuit and the levers usable by touch
    {
        await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true });
        await sleep(500);
        const phone = await page.evaluate(() => {
            const view = document.getElementById('view').getBoundingClientRect();
            const lever = document.querySelector('#inputs .lever')?.getBoundingClientRect();
            return { overflow: document.documentElement.scrollWidth - window.innerWidth, viewHeight: view.height, viewWidth: view.width, lever: lever ? [lever.width, lever.height] : null };
        });
        check(phone.overflow <= 0, `the page is ${phone.overflow} px wider than a phone`);
        check(phone.viewHeight >= 200 && phone.viewWidth >= 360, `the view is ${phone.viewWidth} x ${phone.viewHeight} on a phone`);
        check(phone.lever !== null && phone.lever[1] >= 28, `a lever is ${phone.lever} px on a phone, too small to touch`);
        await page.screenshot({ path: join(out, 'phone.png') });
        await page.setViewport({ width: 1400, height: 900 });
        console.log('on a phone: no horizontal scroll, the view and the levers sized for touch');
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
        const invReport = await page.$eval('#report-text', (e) => e.textContent);
        check(status.kind === 'ok' && /^built inv in [\d.]+ s$/.test(status.text) && /^ {2}cells {8}1$/m.test(invReport), `inv: ${status.text}, report ${JSON.stringify(invReport)}`);
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
        check(status.kind === 'bad' && status.text === 'stopped: compiling took longer than 1 s, try a smaller design', `time limit: ${JSON.stringify(status.text)}`);
        check(status.seen.some((t) => /^compiling adder4: (synthesizing|proving|placing and routing), \d+ s$/.test(t)), `no phase and elapsed time shown while compiling: ${JSON.stringify(status.seen)}`);
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
        check(await page.evaluate(() => window.redfab.opening === 'shown') && /your draft is in the editor/.test(await page.$eval('#status', (e) => e.textContent)), 'with a draft, the page did not open its circuit and say the draft is in the editor');
        // The footer is the notice Mojang's guidelines require, and nothing else (asked for 2026-11-11).
        check(await page.$eval('footer', (e) => e.textContent.trim()) === 'NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT.', 'the footer is not the Minecraft notice alone');
    }

    // --- a clocked design: driven by hand through the native cycle protocol, then
    // free-running under the page's own clock, counted in simulated ticks
    {
        await loadExample(page, 'counter4');
        const status = await compileAndWait(page, 180_000);
        if (status.kind !== 'ok') {
            failures.push(`counter4: compile failed: ${status.text}`);
        } else {
            check(/^built counter4 in [\d.]+ s$/.test(status.text) && /^ {2}T_clk {8}\d+ ticks/m.test(await page.$eval('#report-text', (e) => e.textContent)), `counter4: the built status reads ${JSON.stringify(status.text)}`);
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
        // The camera (stage 14): the status carries it, and the relay's WHEP endpoint is faked.
        let cameraStatus = { connected: true, player: 'Steve_7', driver: null, queue: 0, plot: 3, streaming: false };
        const whep = [];
        page.on('request', (req) => {
            const url = new URL(req.url());
            if (url.pathname === '/api/status') return req.respond({ status: 200, contentType: 'application/json', body: JSON.stringify({ live: true, limits: { plotSize: [16, 16] }, camera: cameraStatus }) });
            if (url.pathname === '/camera/camera/whep') {
                whep.push({ method: req.method(), type: req.headers()['content-type'], body: req.postData() });
                return req.respond({ status: 503, contentType: 'text/plain', body: 'no publisher' });
            }
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
        check(refused[0] === 'bad' && /^not sent: \d+ x \d+ blocks, the server's plots are 16 x 16$/.test(refused[1]), `too large for the plot: ${JSON.stringify(refused)}`);
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
        check(/plot 3/.test(await page.$eval('#mode', (e) => e.textContent)), 'the mode badge does not name the plot in live mode');
        check(await page.$eval('#live', (e) => e.classList.contains('primary') && !e.hidden) && !(await page.$eval('#compile', (e) => e.classList.contains('primary'))), 'with a game behind the page, Run in the game is not the main action');
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

        // --- the camera tab (stage 14, reworked 2026-11-11): a tab beside the circuit when the backend has a
        // camera, the stream played while the tab is open and retried when the relay refuses, Auto view and
        // Manual control as two buttons, and Run in the game opening the camera in place of the circuit
        await open(page, errors);
        check(await page.$eval('#tabs', (e) => !e.hidden), 'the tabs are hidden although the backend has a camera');
        check(await page.evaluate(() => window.redfab.tab) === 'circuit' && await page.$eval('#camera', (e) => e.hidden), 'the page did not open on the circuit');
        // With no stream the tab says so and asks the relay for nothing.
        await page.click('#tab-camera');
        check(await page.$eval('#view', (e) => e.hidden) && await page.$eval('#camera', (e) => !e.hidden), 'the camera tab did not take the stage');
        check(/offline/i.test(await page.$eval('#camera-status', (e) => e.textContent)), `with no stream the camera status reads ${JSON.stringify(await page.$eval('#camera-status', (e) => e.textContent))}`);
        check(whep.length === 0, 'the page asked the relay for a stream that is not up');
        // Hidden behind the camera, the circuit draws nothing while its simulation runs.
        const hiddenFrames = await page.evaluate(() => window.redfab.frames);
        await sleep(500);
        check(await page.evaluate(() => window.redfab.frames) === hiddenFrames, 'the hidden circuit kept drawing');
        // The stream comes up: the open tab asks the relay with an SDP offer, shows the refusal and tries again.
        cameraStatus = { ...cameraStatus, streaming: true };
        await page.evaluate(() => window.redfab.camera.refresh());
        await page.waitForFunction(() => /503/.test(document.getElementById('camera-status').textContent), { timeout: 10_000 });
        check(whep.length >= 1 && whep[0].method === 'POST' && /^application\/sdp/.test(whep[0].type) && /^v=0/.test(whep[0].body ?? ''), `the WHEP request was ${JSON.stringify(whep.map((w) => [w.method, w.type, (w.body ?? '').slice(0, 10)]))}`);
        check(await page.evaluate(() => window.redfab.camera.playing === false), 'the page says it is playing after the relay refused');
        const asked = whep.length;
        await sleep(3500);
        check(whep.length > asked, 'the page did not try the relay again after a refusal');
        // Back to the circuit: the stream stops and the relay is asked no more.
        await page.click('#tab-circuit');
        await sleep(200);
        const afterCircuit = whep.length;
        await sleep(3500);
        check(whep.length === afterCircuit && await page.evaluate(() => !window.redfab.camera.shown), 'the circuit tab kept asking the relay for the stream');
        // The two modes, through the page's own hooks (the faked backend has no socket): take, the
        // backend's answers, the keys into move and look, back to auto view.
        const pressedState = () => [document.getElementById('camera-auto').getAttribute('aria-pressed'), document.getElementById('camera-manual').getAttribute('aria-pressed')];
        check(JSON.stringify(await page.evaluate(pressedState)) === '["true","false"]', 'the camera does not start in auto view');
        check(await page.$eval('#camera-manual', (e) => e.disabled), 'manual control is offered without a plot to drive from');
        const protocol = await page.evaluate(() => {
            const pressed = () => [document.getElementById('camera-auto').getAttribute('aria-pressed'), document.getElementById('camera-manual').getAttribute('aria-pressed')];
            const c = window.redfab.camera;
            const out = [];
            out.push(c.take());
            c.receive({ drive: 'yours', seconds: 60 });
            out.push([pressed(), document.getElementById('camera-turn').textContent.trim(), c.driving, document.getElementById('camera-status').textContent]);
            c.press('w', true);
            c.press('d', true);
            c.press('shift', true);
            c.look(12, -3);
            out.push(c.input());
            c.press('w', false);
            c.press('d', false);
            c.press('shift', false);
            out.push(c.input());
            c.receive({ drive: 'over', why: 'time: the turn is over' });
            out.push([pressed(), c.driving, document.getElementById('camera-status').textContent]);
            c.receive({ drive: 'queued', position: 2 });
            out.push(document.getElementById('camera-turn').textContent.trim());
            return out;
        });
        check(JSON.stringify(protocol[0]) === JSON.stringify({ drive: 'take' }), `take sends ${JSON.stringify(protocol[0])}`);
        check(JSON.stringify(protocol[1][0]) === '["false","true"]' && /60/.test(protocol[1][1]) && protocol[1][2] === true, `after yours: ${JSON.stringify(protocol[1])}`);
        check(JSON.stringify(protocol[2]) === JSON.stringify({ drive: { move: [1, -1, 1], look: [12, -3] } }), `w, d, shift and a look give ${JSON.stringify(protocol[2])}`);
        check(protocol[3] === null, `with nothing pressed the input is ${JSON.stringify(protocol[3])}, not null`);
        // After the turn: Auto view pressed again says it; the status drops the driving hint and adds nothing.
        check(JSON.stringify(protocol[4][0]) === '["true","false"]' && protocol[4][1] === false && !/WASD|over|auto/i.test(protocol[4][2]), `after over: ${JSON.stringify(protocol[4])}`);
        check(protocol[1][3] === 'WASD to move, space and shift for up and down, drag video to look', `while driving the hint reads ${JSON.stringify(protocol[1][3])}`);
        check(/2/.test(protocol[5]), `queued at 2 shows ${JSON.stringify(protocol[5])}`);
        // Run in the game: the camera takes the stage in place of the circuit, and knows the plot is yours.
        await loadExample(page, 'not1');
        const beforeLive = await compileAndWait(page);
        if (!(await page.evaluate(() => window.redfab.built))) throw new Error(`the compile before Run in the game left no build: ${JSON.stringify(beforeLive)}`);
        check(await page.evaluate(() => window.redfab.tab) === 'circuit', 'a compile did not show the circuit');
        await page.evaluate(() => { window.redfab.limits.plotSize = [512, 512]; });
        submitAnswer = { status: 200, body: { plot: 3, token: 'tok', design: await page.evaluate(() => window.redfab.built.design), report: 'design not1', origin: [0, 64, 0], placed_tick: 1, state: 'settling' } };
        await page.click('#live');
        await page.waitForFunction(() => window.redfab.livePlot !== null, { timeout: 10_000 });
        check(await page.evaluate(() => window.redfab.tab) === 'camera' && await page.$eval('#view', (e) => e.hidden), 'Run in the game did not open the camera in place of the circuit');
        check(await page.$eval('#camera-manual', (e) => !e.disabled), 'manual control is not offered while a plot is yours');
        await page.evaluate(() => window.redfab.camera.refresh());
        check(JSON.stringify(await page.evaluate(() => window.redfab.camera.watching)) === '{"plot":3,"yours":true}', 'the camera does not know it is on your plot');
        cameraStatus = { ...cameraStatus, plot: 1 };
        await page.evaluate(() => window.redfab.camera.refresh());
        check(JSON.stringify(await page.evaluate(() => window.redfab.camera.watching)) === '{"plot":1,"yours":false}', 'the camera still says your plot after it moved to another');
        // A hidden page stops the stream to save the relay's bandwidth, and asks for nothing while hidden.
        await page.evaluate(() => { window.redfab.camera.hidden(true); });
        const whileHidden = whep.length;
        await sleep(3500);
        check(await page.evaluate(() => window.redfab.camera.playing === false) && whep.length === whileHidden, 'a hidden page kept playing or kept asking the relay');
        await page.evaluate(() => { window.redfab.camera.hidden(false); });
        await page.click('#release');
        console.log('the camera tab: beside the circuit, streamed while open and retried on a refusal, auto view and manual control, opened by Run in the game');
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
