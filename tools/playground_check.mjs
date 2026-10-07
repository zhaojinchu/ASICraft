// The playground page in a headless Chromium (Brave). See `check`.
//
//   node tools/playground_check.mjs [--show]
//
// Serves the playground, opens it, and for each design: loads its source,
// compiles, stops the free-running simulation, applies input vectors through the
// panel (and one through a click on a lever in the 3D view), steps the
// simulator, reads the lamps, and compares with the native simulator on the
// natively built design. Screenshots go to build/playground/. Needs Brave or
// Chrome; set REDFAB_BROWSER to another Chromium binary.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import puppeteer from 'puppeteer-core';
import { serve } from './playground_serve.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const out = join(root, 'build/playground');
mkdirSync(out, { recursive: true });
const show = process.argv.includes('--show');

const BROWSERS = [
    process.env.REDFAB_BROWSER,
    '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser',
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/usr/bin/chromium', '/usr/bin/google-chrome',
].filter(Boolean);
const browserPath = BROWSERS.find((p) => existsSync(p));
if (!browserPath) {
    console.log('no Chromium found; install Brave or Chrome, or set REDFAB_BROWSER');
    process.exit(1);
}

execFileSync('cargo', ['build', '-q', '-p', 'redfab-wasm', '--target', 'wasm32-unknown-unknown', '--release'], { cwd: root, stdio: 'inherit' });

/// The native simulator's outputs for the vectors, on the natively built design.
function nativeOutputs(name, vectors) {
    const file = join(out, `${name}.vectors.json`);
    writeFileSync(file, JSON.stringify(vectors));
    const text = execFileSync('cargo', ['run', '-q', '-p', 'redfab-cli', '--', 'simulate', join(root, 'build/designs', name, 'design.json'), file], { cwd: root });
    return JSON.parse(text.toString());
}

const DESIGNS = { not1: 4, nor2: 4, fa1: 8, consts: 4 };

const { server, port } = await serve(0);
const browser = await puppeteer.launch({ executablePath: browserPath, headless: !show, args: ['--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--no-sandbox'] });
let failures = [];
try {
    const page = await browser.newPage();
    await page.setViewport({ width: 1400, height: 900 });
    const errors = [];
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
    await page.goto(`http://127.0.0.1:${port}/`);
    try {
        await page.waitForFunction(() => window.redfab !== undefined, { timeout: 30_000 });
    } catch {
        throw new Error(`the page did not start: ${errors.join(' | ') || 'no errors reported'}`);
    }
    await page.evaluate(() => window.redfab.ready());

    for (const [name, count] of Object.entries(DESIGNS)) {
        await page.evaluate((n) => window.redfab.loadExample(n), name);
        await page.select('#examples', name);
        await page.click('#compile');
        await page.waitForFunction(() => document.getElementById('status').className !== '', { timeout: 120_000 });
        const status = await page.$eval('#status', (e) => [e.className, e.textContent]);
        if (status[0] !== 'ok') {
            failures.push(`${name}: compile failed: ${status[1]}`);
            continue;
        }
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
        let current = Object.fromEntries(inputs.map((n) => [n, 0]));
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

    // A clocked design: compiled, and running on its own with the page driving the clock.
    await page.evaluate((n) => window.redfab.loadExample(n), 'counter4');
    await page.select('#examples', 'counter4');
    await page.click('#compile');
    await page.waitForFunction(() => document.getElementById('status').className !== '', { timeout: 180_000 });
    const status = await page.$eval('#status', (e) => [e.className, e.textContent]);
    if (status[0] !== 'ok') {
        failures.push(`counter4: compile failed: ${status[1]}`);
    } else {
        await page.click('[data-input="en"]');
        const before = await page.evaluate(() => window.redfab.tick);
        await new Promise((ok) => setTimeout(ok, 8000));
        const after = await page.evaluate(() => window.redfab.tick);
        const lamps = await page.evaluate(() => window.redfab.lamps());
        if (after - before < 100) failures.push(`counter4: the simulation advanced only ${after - before} ticks in 8 s`);
        else console.log(`counter4: compiled and running, ${after - before} ticks in 8 s, lamps ${JSON.stringify(lamps)}`);
        await page.screenshot({ path: join(out, 'counter4.png') });
    }
    if (errors.length) failures.push(`page errors: ${errors.slice(0, 3).join(' | ')}`);
} finally {
    await browser.close();
    server.close();
}
for (const f of failures) console.log(`FAIL: ${f}`);
if (!failures.length) console.log(`screenshots in ${out}`);
process.exit(failures.length ? 1 : 0);
