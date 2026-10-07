// The live playground on the real modded server, in a headless browser. See `check`.
//
//   node tools/live_check.mjs [--show]
//
// Starts the backend with its own Fabric server and the observer mod, then:
// an inverter is submitted through the API and driven by its lever, and what
// the stream said must replay natively tick for tick; a scripted browser
// session submits the counter, runs it live with its levers, and the page must
// show what the server shows and the recording must replay tick for tick; six
// counters running at once must leave the server's tick time within budget.
// Screenshots go to build/playground/. Needs Brave or Chrome; set
// REDFAB_BROWSER to another Chromium binary.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import puppeteer from 'puppeteer-core';
import { startLive } from '../server/live.mjs';
import { api, changesAt, connect, designSource, firstTraceDifference, root, sleep, waitFor } from './live_common.mjs';

const out = join(root, 'build/playground');
mkdirSync(out, { recursive: true });
const show = process.argv.includes('--show');
const TICK_BUDGET = { avg_ms: 25, p95_ms: 45 };
const PLOTS = 6;

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
execFileSync(join(root, 'tools/build_mod.sh'), { cwd: root, stdio: 'pipe' });

const failures = [];
const fail = (text) => { failures.push(text); console.log(`FAIL: ${text}`); };

/// Replays the recording natively and compares, tick for tick.
function replays(name, fixture, trace) {
    const dir = mkdtempSync(join(tmpdir(), 'redfab-live-'));
    writeFileSync(join(dir, `${name}.fixture.json`), JSON.stringify(fixture));
    writeFileSync(join(dir, `${name}.live.trace.json`), JSON.stringify(trace));
    execFileSync('cargo', ['run', '-q', '-p', 'redfab-cli', '--', 'check-fixtures', dir], { cwd: root, stdio: 'pipe' });
    const replayed = JSON.parse(readFileSync(join(dir, `${name}.sim.trace.json`), 'utf8'));
    const diff = firstTraceDifference(trace, replayed);
    if (diff) fail(`${name}: the live recording differs from its native replay: ${diff} (files in ${dir})`);
    else console.log(`${name}: ${fixture.ticks} ticks and ${fixture.events.length} lever changes replay natively tick for tick`);
    return diff === null;
}

async function controller(base, wsBase, name, extra = {}) {
    const reply = await api(base, '/api/submit', { method: 'POST', body: { source: designSource(name), top: 'top', name, ...extra } });
    if (reply.status !== 200) throw new Error(`${name}: submit: ${reply.status} ${reply.body.error}`);
    const client = connect(`${wsBase}/live?plot=${reply.body.plot}&token=${reply.body.token}`);
    await client.opened;
    const hello = await client.next((m) => m.hello !== undefined);
    if (hello.state !== 'live') await client.next((m) => m.state === 'live', 60_000);
    const info = await api(base, `/api/plots/${reply.body.plot}`);
    if (!info.body.placement?.matched) throw new Error(`${name}: the stream did not show the design placed exactly: ${JSON.stringify(info.body.placement)}`);
    return { ...reply.body, client };
}

const live = await startLive({ runDir: join(root, 'build/mc/live-check'), port: 0, plots: PLOTS, log: (m) => console.log(`  [live] ${m}`) });
const base = `http://127.0.0.1:${live.port}`;
const wsBase = `ws://127.0.0.1:${live.port}`;
let browser = null;
try {
    // --- an inverter through the API: its lever, and the recording
    {
        const not1 = await controller(base, wsBase, 'not1');
        const flip = async (on, ticks) => {
            not1.client.send({ set: 'a', on });
            const ack = await not1.client.next((m) => m.ok !== undefined);
            if (!ack.ok) throw new Error(`not1: set a: ${ack.error}`);
            const now = (await not1.client.next((m) => m.tick !== undefined)).tick;
            await not1.client.next((m) => m.tick >= now + ticks, 15_000);
        };
        await flip(true, 24);
        await flip(false, 24);
        await flip(true, 3);
        await flip(false, 30);
        const info = await api(base, `/api/plots/${not1.plot}`);
        if (JSON.stringify(info.body.lamps) !== '[1]') fail(`not1: with a off the lamp should be lit; the server shows ${JSON.stringify(info.body.lamps)}`);
        const exported = await api(base, `/api/plots/${not1.plot}/trace`);
        if (exported.status !== 200) fail(`not1: trace export: ${exported.body.error}`);
        else {
            const y = not1.design.outputs[0].lamp;
            if (changesAt(exported.body.trace, y) < 4) fail(`not1: the lamp changed only ${changesAt(exported.body.trace, y)} times in the recording`);
            replays('not1', exported.body.fixture, exported.body.trace);
        }
        not1.client.close();
        await api(base, `/api/plots/${not1.plot}?token=${not1.token}`, { method: 'DELETE' });
    }

    // --- the counter, through the page
    browser = await puppeteer.launch({ executablePath: browserPath, headless: !show, args: ['--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--no-sandbox'] });
    const page = await browser.newPage();
    await page.setViewport({ width: 1400, height: 900 });
    const errors = [];
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
    await page.goto(`${base}/`);
    try {
        await page.waitForFunction(() => window.redfab !== undefined, { timeout: 30_000 });
    } catch {
        throw new Error(`the page did not start: ${errors.join(' | ') || 'no errors reported'}`);
    }
    await page.waitForSelector('#live', { visible: true, timeout: 10_000 });
    await page.evaluate((n) => window.redfab.loadExample(n), 'counter4');
    await page.select('#examples', 'counter4');
    await page.click('#live');
    await page.waitForFunction(() => document.getElementById('status').className !== '' && /live|fail|stopped|error/i.test(document.getElementById('status').textContent), { timeout: 180_000 });
    const status = await page.$eval('#status', (e) => [e.className, e.textContent]);
    if (status[0] !== 'ok') {
        fail(`counter4: going live failed: ${status[1]}`);
    } else {
        const plot = await page.evaluate(() => window.redfab.livePlot);
        const design = await page.evaluate(() => window.redfab.design);
        const period = design.timing.sequential.period;
        const wait = (periods) => sleep((periods * period * 50) + 500);
        console.log(`counter4: live on the server at plot ${plot.plot}, period ${period} ticks`);
        await page.click('[data-input="en"]');
        await wait(3);
        await page.click('[data-input="rst"]');
        await wait(1);
        await page.click('[data-input="rst"]');
        await wait(2);
        await page.screenshot({ path: join(out, 'live-counter4.png') });
        const shown = await page.evaluate(() => window.redfab.lamps());
        const info = await api(base, `/api/plots/${plot.plot}`);
        const server = Object.fromEntries(design.outputs.map((p, i) => [p.name, info.body.lamps[i]]));
        if (JSON.stringify(shown) !== JSON.stringify(server)) fail(`counter4: the page shows lamps ${JSON.stringify(shown)}, the server ${JSON.stringify(server)}`);
        else console.log(`counter4: the page shows the server's lamps ${JSON.stringify(shown)} at server tick ${info.body.tick}`);
        const pageTick = await page.evaluate(() => window.redfab.liveTick);
        if (Math.abs(pageTick - info.body.tick) > 40) fail(`counter4: the page is at tick ${pageTick}, the server at ${info.body.tick}`);
        const exported = await api(base, `/api/plots/${plot.plot}/trace`);
        if (exported.status !== 200) fail(`counter4: trace export: ${exported.body.error}`);
        else {
            const bit0 = design.outputs.find((p) => p.name === 'count[0]').lamp;
            const toggles = changesAt(exported.body.trace, bit0);
            if (toggles < 4) fail(`counter4: count[0] changed only ${toggles} times; the counter did not count`);
            else console.log(`counter4: count[0] changed ${toggles} times while running live`);
            replays('counter4', exported.body.fixture, exported.body.trace);
        }
    }
    if (errors.length) fail(`page errors: ${errors.slice(0, 3).join(' | ')}`);

    // --- tick time with every plot running a counter
    {
        const extra = [];
        const free = (await api(base, '/api/status')).body.plots.free;
        for (let k = 0; k < free; k++) {
            const c = await controller(base, wsBase, 'counter4', { name: `counter4_${k}` });
            c.client.send({ set: 'en', on: true });
            await c.client.next((m) => m.ok !== undefined);
            extra.push(c);
        }
        const running = (await api(base, '/api/status')).body.plots;
        if (running.free !== 0) fail(`not every plot is in use: ${JSON.stringify(running)}`);
        await sleep(12_000);
        const tick = (await api(base, '/api/status')).body.tick;
        if (!tick || tick.avg_ms > TICK_BUDGET.avg_ms || tick.p95_ms > TICK_BUDGET.p95_ms) {
            fail(`tick time with ${running.total} plots: ${JSON.stringify(tick)}; the budget is ${JSON.stringify(TICK_BUDGET)}`);
        } else {
            console.log(`tick time with ${running.total} counters running: average ${tick.avg_ms} ms, P95 ${tick.p95_ms} ms (budget ${TICK_BUDGET.avg_ms}/${TICK_BUDGET.p95_ms})`);
        }
        for (const c of extra) c.client.close();
    }
} catch (err) {
    fail(err.stack || String(err));
} finally {
    if (browser) await browser.close();
    await live.close();
}
for (const f of failures) console.log(`FAIL: ${f}`);
if (!failures.length) console.log(`screenshots in ${out}`);
process.exit(failures.length ? 1 : 0);
