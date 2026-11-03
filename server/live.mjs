// The live playground's backend: serves the page, takes submissions, compiles
// them in the sandbox, places them in plots on the Minecraft server, maps web
// levers to probe blocks, and relays the observer mod's stream to viewers,
// filtered to their plot. See README.md.
//
//   node server/live.mjs [--port 8766] [--host 127.0.0.1] [--plots 8] [--run-dir build/mc/live] [--trust-proxy] [--wasm PATH]
//
// `createLive` takes any world (the checks give it a fake one); `startLive`
// launches the modded server and gives it the real one. `--wasm` names a
// prebuilt core (deploy/ship_core.sh), so the machine needs no Rust toolchain;
// without it the core is built here first.

import { existsSync } from 'node:fs';
import { createServer } from 'node:http';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LIMITS } from '../playground/compile.js';
import { contentSecurityPolicy, prepareOpening, serveStatic } from '../tools/playground_serve.mjs';
import { PlotError, Plots } from './plots.mjs';
import { SandboxError, createSandbox } from './sandbox.mjs';
import { acceptWebSocket } from './ws.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

export const DEFAULTS = {
    plots: 8,
    plotSize: [512, 512],
    plotY: 64,
    idleSeconds: 600,
    maxSeconds: 3600,
    freshSeconds: 60, // a plot whose submitter never connects goes after this
    clearRetrySeconds: 15,
    maxRecordedChanges: 500_000, // a recording past this is kept no further and cannot be exported
    staleAfterSeconds: 5, // the stream reports every tick; older than this, the backend is not live
    compile: { seconds: LIMITS.compileSeconds, memoryMb: 2048, queue: 4, cacheEntries: 32 }, // a normal compile peaks near 900 MB
    viewers: { perPlot: 16, perIp: 8, maxBufferedBytes: 256 * 1024 }, // a viewer not reading past the bound is dropped
    perIp: { plots: 1, compiles: 1, submissionsPerHour: 30 },
    origins: [], // Origins allowed besides the page's own host
    trustProxy: false, // take the client address from X-Forwarded-For (behind a reverse proxy only)
    tickEverySeconds: 5,
    wasm: join(root, 'target/wasm32-unknown-unknown/release/redfab_wasm.wasm'),
    source: { kind: 'live' },
};

const SUBMIT_FIELDS = new Set(['source', 'top', 'name']);
const HOUR = 3600 * 1000;

// Every answer the backend writes itself carries the page's policy too, so a
// response body can never run as a page (the static files get it in serveStatic).
const API_HEADERS = { 'content-security-policy': contentSecurityPolicy(), 'x-content-type-options': 'nosniff' };

function json(res, status, body) {
    res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store', ...API_HEADERS });
    res.end(JSON.stringify(body));
}

function readBody(req, limit) {
    return new Promise((resolve, reject) => {
        const chunks = [];
        let size = 0;
        req.on('data', (chunk) => {
            size += chunk.length;
            if (size > limit) {
                reject(new PlotError(`the request is larger than ${limit} bytes`, 413));
                req.destroy();
                return;
            }
            chunks.push(chunk);
        });
        req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
        req.on('error', reject);
    });
}

export async function createLive(options) {
    const opts = {
        ...DEFAULTS,
        ...options,
        compile: { ...DEFAULTS.compile, ...(options.compile || {}) },
        viewers: { ...DEFAULTS.viewers, ...(options.viewers || {}) },
        perIp: { ...DEFAULTS.perIp, ...(options.perIp || {}) },
    };
    const { world, log = () => {} } = opts;
    const sandbox = createSandbox({ wasm: opts.wasm, ...opts.compile, testHooks: opts.testHooks === true, log });
    const plots = new Plots({
        world,
        count: opts.plots,
        plotSize: opts.plotSize,
        plotY: opts.plotY,
        idleSeconds: opts.idleSeconds,
        maxSeconds: opts.maxSeconds,
        freshSeconds: opts.freshSeconds,
        clearRetrySeconds: opts.clearRetrySeconds,
        maxRecordedChanges: opts.maxRecordedChanges,
        log,
    });

    let tickTime = null;
    const sampleTick = async () => {
        try {
            tickTime = await world.tickTime();
        } catch (err) {
            tickTime = null;
            log(`tick time: ${err.message}`);
        }
    };
    await sampleTick();
    const sampler = setInterval(sampleTick, opts.tickEverySeconds * 1000);

    /// The client's address: what the socket says, or the proxy's header when told to trust it.
    const clientIp = (req) => {
        if (opts.trustProxy) {
            const forwarded = String(req.headers['x-forwarded-for'] || '').split(',')[0].trim();
            if (forwarded) return forwarded;
        }
        return req.socket.remoteAddress ?? 'unknown';
    };

    /// A request with an Origin header must come from the page's own host (or a listed one).
    const originAllowed = (req) => {
        const origin = req.headers.origin;
        if (origin === undefined) return true;
        if (opts.origins.includes(origin)) return true;
        try {
            return new URL(origin).host === req.headers.host;
        } catch {
            return false;
        }
    };

    const submissions = new Map(); // ip -> times of recent submissions
    const compiling = new Map(); // ip -> compiles in hand

    /// Forgets addresses with no submission in the last hour, so the map holds only what the limit needs.
    const pruneSubmissions = () => {
        const now = Date.now();
        for (const [ip, times] of submissions) if (times.every((t) => now - t >= HOUR)) submissions.delete(ip);
    };
    const compileDone = (ip) => {
        const left = (compiling.get(ip) ?? 1) - 1;
        if (left > 0) compiling.set(ip, left);
        else compiling.delete(ip);
    };

    const status = () => {
        const health = world.health?.() ?? { running: true, rcon_up: true, stream_up: true, stream_age_ms: 0, stream_reconnects: 0 };
        const streamFresh = health.stream_up && health.stream_age_ms !== null && health.stream_age_ms <= opts.staleAfterSeconds * 1000;
        return {
            live: Boolean(health.running && health.rcon_up && streamFresh && tickTime !== null),
            minecraft: health,
            plots: { total: plots.plots.length, free: plots.free.length, broken: plots.broken.length },
            placements: plots.placements,
            tick: tickTime,
            limits: {
                sourceBytes: LIMITS.sourceBytes,
                cells: LIMITS.cells,
                blocks: LIMITS.blocks,
                compileSeconds: opts.compile.seconds,
                plotSize: plots.plotSize,
                plotsPerAddress: opts.perIp.plots,
                compilesPerAddress: opts.perIp.compiles,
                submissionsPerHour: opts.perIp.submissionsPerHour,
                viewersPerPlot: opts.viewers.perPlot,
            },
            compiling: sandbox.busy,
            compile_cache: sandbox.cache,
        };
    };

    async function submit(req, res) {
        if (!/^application\/json\b/i.test(String(req.headers['content-type'] || ''))) {
            throw new PlotError('the body must be JSON, sent as application/json', 415);
        }
        if (!originAllowed(req)) throw new PlotError('the request comes from another site', 403);
        const ip = clientIp(req);
        const text = await readBody(req, LIMITS.sourceBytes * 2 + 4096);
        let body;
        try {
            body = JSON.parse(text);
        } catch {
            throw new PlotError('the body must be JSON');
        }
        if (!body || typeof body !== 'object' || Array.isArray(body)) throw new PlotError('the body must be an object');
        for (const field of Object.keys(body)) {
            if (!SUBMIT_FIELDS.has(field)) throw new PlotError(`unexpected field ${JSON.stringify(field)}: a submission is source only`);
        }
        if (typeof body.source !== 'string') throw new PlotError('the source must be text');
        if (Buffer.byteLength(body.source, 'utf8') > LIMITS.sourceBytes) {
            throw new PlotError(`the source is ${Buffer.byteLength(body.source, 'utf8')} bytes; the limit is ${LIMITS.sourceBytes}`, 413);
        }
        const top = body.top === undefined ? 'top' : body.top;
        if (typeof top !== 'string' || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(top)) throw new PlotError(`${JSON.stringify(top)} is not a module name`);
        const name = typeof body.name === 'string' && /^[A-Za-z0-9_-]{1,32}$/.test(body.name) ? body.name : 'design';

        // One plot, one compile at a time, and so many submissions an hour per address.
        const held = plots.heldBy(ip);
        if (held.length >= opts.perIp.plots) {
            throw new PlotError(`you already hold plot ${held[0].index}; release it before submitting another design`, 429);
        }
        if ((compiling.get(ip) ?? 0) >= opts.perIp.compiles) throw new PlotError('your previous submission is still compiling', 429);
        pruneSubmissions();
        const recent = (submissions.get(ip) ?? []).filter((t) => Date.now() - t < HOUR);
        if (recent.length >= opts.perIp.submissionsPerHour) {
            throw new PlotError(`more than ${opts.perIp.submissionsPerHour} submissions in an hour; try again later`, 429);
        }
        recent.push(Date.now());
        submissions.set(ip, recent);

        const plot = plots.reserve(name, ip);
        compiling.set(ip, (compiling.get(ip) ?? 0) + 1);
        try {
            const built = await sandbox.compile({ source: body.source, top, name }).finally(() => compileDone(ip));
            await plots.place(plot, built);
            log(`plot ${plot.index}: ${name} placed at ${plot.origin.join(',')}, tick ${plot.placedTick}`);
            json(res, 200, {
                plot: plot.index,
                token: plot.token,
                design: built.design,
                report: built.report,
                origin: plot.origin,
                placed_tick: plot.placedTick,
                state: plot.state,
                levers: plot.leverInfo(),
            });
            plots.controllerLeft(plot, { fresh: true }); // until a controller connects, the short clock runs
        } catch (err) {
            if (plot.state === 'reserved') plots.unreserve(plot);
            else await plots.release(plot, `placing failed: ${err.message}`);
            throw err;
        }
    }

    const server = createServer(async (req, res) => {
        const url = new URL(req.url, 'http://localhost');
        try {
            if (url.pathname === '/api/status' && req.method === 'GET') return json(res, 200, status());
            if (url.pathname === '/api/fits' && req.method === 'GET') {
                // The page asks before submitting whether a footprint it has built would fit a plot.
                const x = Number(url.searchParams.get('x'));
                const z = Number(url.searchParams.get('z'));
                if (!Number.isInteger(x) || !Number.isInteger(z) || x < 1 || z < 1) return json(res, 400, { error: 'x and z must be positive whole numbers' });
                return json(res, 200, { fits: plots.fits([x, 0, z]), plotSize: plots.plotSize, free: plots.free.length });
            }
            if (url.pathname === '/api/submit' && req.method === 'POST') return await submit(req, res);
            const plotPath = url.pathname.match(/^\/api\/plots\/(\d+)(\/trace|\/structure)?$/);
            if (plotPath) {
                const plot = plots.get(plotPath[1]);
                if (!plot) return json(res, 404, { error: 'no such plot' });
                const owner = url.searchParams.get('token') === plot.token;
                if (plotPath[2] === '/trace' && req.method === 'GET') {
                    if (!owner) return json(res, 403, { error: 'not your plot: the recording is for its owner' });
                    return json(res, 200, plot.recording(opts.source));
                }
                if (plotPath[2] === '/structure' && req.method === 'GET') {
                    if (!owner) return json(res, 403, { error: 'not your plot: the structure file is for its owner' });
                    if (!plot.structure) return json(res, 409, { error: 'the design has not been placed yet' });
                    res.writeHead(200, {
                        'content-type': 'application/octet-stream',
                        'content-disposition': `attachment; filename="${plot.name}.nbt"`,
                        'cache-control': 'no-store',
                        ...API_HEADERS,
                    });
                    return res.end(Buffer.from(plot.structure));
                }
                if (plotPath[2]) return json(res, 405, { error: 'method not allowed' });
                if (req.method === 'GET') return json(res, 200, plot.info());
                if (req.method === 'DELETE') {
                    if (!owner) return json(res, 403, { error: 'not your plot' });
                    await plots.release(plot, 'released by its owner');
                    return json(res, 200, { released: true });
                }
            }
            if (url.pathname.startsWith('/api/')) return json(res, 404, { error: 'no such endpoint' });
            if (req.method !== 'GET') return json(res, 405, { error: 'method not allowed' });
            // The page compiles with the same core the sandbox runs: on a deployment
            // that is the shipped file, not a build under target/ (there is none).
            return await serveStatic(req, res, { core: opts.wasm });
        } catch (err) {
            const known = err instanceof PlotError || err instanceof SandboxError;
            // The path only: a query string may carry a plot token.
            if (!known) log(`request ${req.method} ${url.pathname} failed: ${err.stack || err}`);
            if (res.headersSent) return res.end();
            return json(res, known ? err.status : 500, { error: known ? err.message : 'internal error' });
        }
    });

    const refuse = (socket, status, text) => {
        socket.write(`HTTP/1.1 ${status} ${text}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
        socket.destroy();
    };

    server.on('upgrade', (req, socket, head) => {
        const url = new URL(req.url, 'http://localhost');
        if (url.pathname !== '/live') return refuse(socket, 404, 'Not Found');
        if (!originAllowed(req)) return refuse(socket, 403, 'Forbidden');
        const id = url.searchParams.get('plot');
        if (!/^\d+$/.test(id ?? '')) return refuse(socket, 400, 'Bad Request');
        const plot = plots.get(id);
        if (!plot) return refuse(socket, 404, 'Not Found');
        const ip = clientIp(req);
        if (plot.viewers.size >= opts.viewers.perPlot) return refuse(socket, 503, 'Service Unavailable');
        let mine = 0;
        for (const p of plots.plots) for (const v of p.viewers) if (v.ip === ip) mine += 1;
        if (mine >= opts.viewers.perIp) return refuse(socket, 429, 'Too Many Requests');
        const connection = acceptWebSocket(req, socket, head, { maxBuffered: opts.viewers.maxBufferedBytes });
        if (!connection) return;
        const viewer = {
            ip,
            controls: url.searchParams.get('token') === plot.token,
            send: (text) => connection.send(text),
            close: (code, reason) => connection.close(code, reason),
        };
        const generation = plot.generation;
        plot.viewers.add(viewer);
        if (viewer.controls) plots.controllerJoined(plot);
        connection.send(JSON.stringify(plot.hello()));
        connection.onMessage = async (text) => {
            let message;
            try {
                message = JSON.parse(text);
            } catch {
                return connection.send(JSON.stringify({ ok: false, error: 'the message must be JSON' }));
            }
            if (!message || typeof message !== 'object') return connection.send(JSON.stringify({ ok: false, error: 'the message must be an object' }));
            const fields = Object.keys(message).sort().join(',');
            if (fields !== 'on,set') return connection.send(JSON.stringify({ ok: false, error: 'a lever message has the fields "set" (an input name) and "on" (true or false), nothing else' }));
            if (typeof message.set !== 'string' || typeof message.on !== 'boolean') {
                return connection.send(JSON.stringify({ ok: false, error: '"set" must name an input and "on" must be true or false' }));
            }
            if (!viewer.controls) return connection.send(JSON.stringify({ ok: false, error: 'not your plot: viewing only' }));
            if (plot.generation !== generation) return connection.send(JSON.stringify({ ok: false, error: 'the plot was released' }));
            try {
                const { tick, queued } = await plot.setLever(message.set, message.on);
                connection.send(JSON.stringify({ ok: true, set: message.set, on: message.on, tick, queued }));
            } catch (err) {
                connection.send(JSON.stringify({ ok: false, error: err.message }));
            }
        };
        connection.onClose = () => {
            plot.viewers.delete(viewer);
            if (connection.dropped) log(`plot ${plot.index}: dropped a viewer that was not reading`);
            if (viewer.controls && plot.generation === generation) plots.controllerLeft(plot);
        };
    });

    const host = opts.host ?? '127.0.0.1';
    await new Promise((ok) => server.listen(opts.port ?? 0, host, ok));
    const port = server.address().port;
    log(`live playground at http://${host}:${port}/${opts.trustProxy ? ' (client addresses from X-Forwarded-For)' : ''}`);
    return {
        server,
        port,
        plots,
        sandbox,
        status,
        /// The addresses the submission limit remembers (for the checks).
        get addresses() {
            return [...new Set([...submissions.keys(), ...compiling.keys()])].sort();
        },
        async close() {
            clearInterval(sampler);
            await plots.closeAll();
            server.closeAllConnections?.();
            await new Promise((ok) => server.close(() => ok()));
        },
    };
}

/// The backend with its own Minecraft server: launches it, then serves. The
/// opening circuit is built first (once per core), so the page shows it at once.
export async function startLive({ runDir, log = () => {}, ...options }) {
    await prepareOpening({ wasm: options.wasm ?? DEFAULTS.wasm, log });
    const { launchMinecraft } = await import('./minecraft.mjs');
    const minecraft = await launchMinecraft({ runDir, log });
    let live;
    try {
        live = await createLive({ ...options, world: minecraft.world, source: minecraft.source, log });
    } catch (err) {
        await minecraft.close();
        throw err;
    }
    return {
        port: live.port,
        plots: live.plots,
        status: live.status,
        minecraft,
        async close() {
            await live.close();
            await minecraft.close();
        },
    };
}

const OPTIONS = ['--port', '--host', '--plots', '--run-dir', '--trust-proxy', '--wasm'];

/// The command line's options (the usage at the top). Paths are taken from the
/// working directory; `wasm` is null when the core is to be built here.
export function cliOptions(args) {
    for (const arg of args.filter((a, i) => a.startsWith('--') && (i === 0 || !OPTIONS.includes(args[i - 1]) || args[i - 1] === '--trust-proxy'))) {
        if (!OPTIONS.includes(arg)) throw new Error(`unknown option ${arg}; the options are ${OPTIONS.join(', ')}`);
    }
    const flag = (name, fallback) => {
        const i = args.indexOf(name);
        if (i >= 0 && (i + 1 >= args.length || args[i + 1].startsWith('--'))) throw new Error(`${name} needs a value`);
        return i >= 0 ? args[i + 1] : fallback;
    };
    const wasm = flag('--wasm', null);
    return {
        runDir: resolve(root, flag('--run-dir', 'build/mc/live')),
        port: Number(flag('--port', 8766)),
        host: flag('--host', '127.0.0.1'),
        plots: Number(flag('--plots', DEFAULTS.plots)),
        // Behind a reverse proxy only: every client would otherwise share the proxy's address and its limits.
        trustProxy: args.includes('--trust-proxy'),
        wasm: wasm === null ? null : resolve(wasm),
    };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
    const options = cliOptions(process.argv.slice(2));
    let wasm = options.wasm;
    if (wasm === null) {
        const { buildWasm } = await import('../tools/common.mjs');
        wasm = buildWasm();
    } else if (!existsSync(wasm)) {
        console.error(`no core at ${wasm} (deploy/ship_core.sh puts one there)`);
        process.exit(2);
    }
    const stamp = () => new Date().toISOString().slice(11, 19);
    const live = await startLive({
        ...options,
        wasm,
        log: (m) => console.log(`[live ${stamp()}] ${m}`),
    });
    const stop = async () => {
        console.log(`[live ${stamp()}] stopping`);
        await live.close();
        process.exit(0);
    };
    process.on('SIGINT', stop);
    process.on('SIGTERM', stop);
    // When the game dies, so does the backend, with a non-zero status for whatever supervises it.
    live.minecraft.onExit(({ code, signal, expected }) => {
        if (expected) return;
        console.log(`[live ${stamp()}] Minecraft is gone (${signal || code}); exiting`);
        process.exit(2);
    });
}
