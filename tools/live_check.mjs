// The live playground on the real modded server, in a headless browser. See `check`.
//
//   node tools/live_check.mjs [--show]
//
// Starts the backend with its own Fabric server and the observer mod, then:
// an inverter is submitted through the API and driven by its lever, and what
// the stream said must replay natively tick for tick; a scripted browser
// session submits the counter, runs it live with its levers, and the page must
// show what the server shows, no clock edge may be lost or land more than
// MAX_JITTER ticks after its scheduled tick (a late edge stretches the period;
// it never shortens the next one), and the recording must replay tick for
// tick; six counters running at once (on 512 by 512 plots) must leave the
// server's tick time within budget and their clocks within the same bound;
// every placed template must have been forgotten by the game and deleted from
// disk; and the status must have said live throughout, with no reconnection.
// Screenshots go to build/playground/. Needs Brave or Chrome; set
// REDFAB_BROWSER to another Chromium binary.

import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import puppeteer from 'puppeteer-core';
import { startLive } from '../server/live.mjs';
import { buildWasm, findBrowser } from './common.mjs';
import { api, changesAt, connect, designSource, firstTraceDifference, root, sleep, waitFor } from './live_common.mjs';

const out = join(root, 'build/playground');
mkdirSync(out, { recursive: true });
const show = process.argv.includes('--show');
const TICK_BUDGET = { avg_ms: 25, p95_ms: 45 };
const PLOTS = 6;
// A flip goes out when the stream reports the tick it is due; it lands late
// when the next tick starts before the command is processed, which the host's
// scheduling decides. More than this means a stalled backend, not noise.
const MAX_JITTER = 2;

const browserPath = findBrowser();
if (!browserPath) {
    console.log('no Chromium found; install Brave or Chrome, or set REDFAB_BROWSER');
    process.exit(1);
}

buildWasm();
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

const runDir = join(root, 'build/mc/live-check');
// The checks come from one machine, so the per-address limits are lifted here.
const live = await startLive({ runDir, port: 0, plots: PLOTS, perIp: { plots: PLOTS, compiles: 2, submissionsPerHour: 100 }, log: (m) => console.log(`  [live] ${m}`) });
const base = `http://127.0.0.1:${live.port}`;
const wsBase = `ws://127.0.0.1:${live.port}`;
let browser = null;

/// The clock driver's edges must all have landed, on their scheduled tick or
/// within MAX_JITTER ticks after it.
function onSchedule(name, info) {
    const clock = info.body.clock;
    if (!clock) return fail(`${name}: no clock driver on the plot`);
    const where = clock.late > 0 ? `${clock.late} landed late, by at most ${clock.max_jitter} ticks` : 'every flip landed on its scheduled tick';
    if (clock.lost > 0 || clock.max_jitter > MAX_JITTER) fail(`${name}: ${clock.edges} clock edges, ${where}, ${clock.lost} lost (allowed: none lost, at most ${MAX_JITTER} ticks late)`);
    else console.log(`${name}: ${clock.edges} clock edges, ${where}`);
}

try {
    const startup = await api(base, '/api/status');
    if (!startup.body.live) fail(`the status is not live at the start: ${JSON.stringify(startup.body.minecraft)}`);
    else console.log(`live: plots ${startup.body.plots.total}, plot size ${startup.body.limits.plotSize.join(' by ')}`);

    // --- an inverter through the API: its lever, and the recording
    {
        const not1 = await controller(base, wsBase, 'not1');
        const flip = async (on, ticks) => {
            not1.client.send({ set: 'a', on });
            const ack = await not1.client.next((m) => m.ok !== undefined);
            if (!ack.ok) throw new Error(`not1: set a: ${ack.error}`);
            // Wait `ticks` game ticks from now: the stream reports every tick.
            not1.client.drain();
            const now = (await not1.client.next((m) => m.tick !== undefined)).tick;
            await not1.client.next((m) => m.tick >= now + ticks, 15_000);
        };
        // Two full swings, then a 3-tick pulse too short to reach the lamp: all of it is recorded and must replay.
        await flip(true, 24);
        await flip(false, 24);
        await flip(true, 3);
        await flip(false, 30);
        const info = await api(base, `/api/plots/${not1.plot}`);
        if (JSON.stringify(info.body.lamps) !== '[1]') fail(`not1: with a off the lamp should be lit; the server shows ${JSON.stringify(info.body.lamps)}`);
        const exported = await api(base, `/api/plots/${not1.plot}/trace?token=${not1.token}`);
        if (exported.status !== 200) fail(`not1: trace export: ${exported.body.error}`);
        else {
            const y = not1.design.outputs[0].lamp;
            if (exported.body.fixture.events.length !== 4) fail(`not1: ${exported.body.fixture.events.length} lever changes were recorded, not 4`);
            if (changesAt(exported.body.trace, y) < 2) fail(`not1: the lamp changed only ${changesAt(exported.body.trace, y)} times in the recording`);
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
        // Waits for the server, not the wall clock: a loaded machine runs the game
        // slower than 20 ticks a second, and the counter counts game ticks.
        const serverTick = async () => (await api(base, `/api/plots/${plot.plot}`)).body.tick;
        const wait = async (periods) => {
            const until = (await serverTick()) + periods * period + 10;
            await waitFor(async () => (await serverTick()) >= until, periods * period * 50 * 6 + 5_000, 250);
        };
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
        onSchedule('counter4', info);
        const exported = await api(base, `/api/plots/${plot.plot}/trace?token=${plot.token}`);
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
        const after = await api(base, '/api/status');
        const tick = after.body.tick;
        if (!tick || tick.avg_ms > TICK_BUDGET.avg_ms || tick.p95_ms > TICK_BUDGET.p95_ms) {
            fail(`tick time with ${running.total} plots: ${JSON.stringify(tick)}; the budget is ${JSON.stringify(TICK_BUDGET)}`);
        } else {
            console.log(`tick time with ${running.total} counters running: average ${tick.avg_ms} ms, P95 ${tick.p95_ms} ms (budget ${TICK_BUDGET.avg_ms}/${TICK_BUDGET.p95_ms})`);
        }
        for (const c of extra) onSchedule(`counter4 on plot ${c.plot}`, await api(base, `/api/plots/${c.plot}`));
        for (const c of extra) c.client.close();

        // --- the backend stayed live, the templates are gone from the game and the disk
        const health = after.body.minecraft;
        if (!after.body.live || health.stream_reconnects !== 0 || health.rcon_reconnects !== 0) {
            fail(`the backend did not stay live throughout: ${JSON.stringify(after.body)}`);
        }
        const placements = after.body.placements;
        await waitFor(() => live.minecraft.world.forgotten >= placements, 10_000).catch(() => {});
        const forgotten = live.minecraft.world.forgotten;
        const files = readdirSync(join(runDir, 'world/generated/redfab/structure')).filter((f) => f.endsWith('.nbt'));
        if (forgotten < placements || files.length) {
            fail(`${placements} templates were placed, the game forgot ${forgotten}, ${files.length} .nbt files remain: ${files.join(' ')}`);
        } else {
            console.log(`${placements} templates placed, every one forgotten by the game and deleted from disk`);
        }
    }
} catch (err) {
    fail(err.stack || String(err));
} finally {
    if (browser) await browser.close();
    await live.close();
}
if (!live.minecraft.exited?.expected) fail(`the game's exit was not the expected one: ${JSON.stringify(live.minecraft.exited)}`);
for (const f of failures) console.log(`FAIL: ${f}`);
if (!failures.length) console.log(`screenshots in ${out}`);
process.exit(failures.length ? 1 : 0);
