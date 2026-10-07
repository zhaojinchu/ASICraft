// The live playground's backend: serves the page, takes submissions, compiles
// them in the sandbox, places them in plots on the Minecraft server, maps web
// levers to probe blocks, and relays the observer mod's stream to viewers,
// filtered to their plot. See README.md.
//
//   node server/live.mjs [--port 8766] [--plots 8] [--run-dir build/mc/live]
//
// `createLive` takes any world (the checks give it a fake one); `startLive`
// launches the modded server and gives it the real one.

import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { dirname, extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LIMITS } from '../playground/compile.js';
import { resolve as resolveStatic } from '../tools/playground_serve.mjs';
import { PlotError, Plots } from './plots.mjs';
import { SandboxError, createSandbox } from './sandbox.mjs';
import { acceptWebSocket } from './ws.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

export const DEFAULTS = {
    plots: 8,
    plotSize: [256, 256],
    plotY: 64,
    idleSeconds: 600,
    maxSeconds: 3600,
    compile: { seconds: LIMITS.compileSeconds, memoryMb: 2048, queue: 4 }, // a normal compile peaks near 900 MB
    tickEverySeconds: 5,
    wasm: join(root, 'target/wasm32-unknown-unknown/release/redfab_wasm.wasm'),
    source: { kind: 'live' },
};

const TYPES = {
    '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.mjs': 'text/javascript; charset=utf-8',
    '.wasm': 'application/wasm', '.json': 'application/json', '.sv': 'text/plain; charset=utf-8',
    '.css': 'text/css', '.map': 'application/json', '.d.ts': 'text/plain',
};

const SUBMIT_FIELDS = new Set(['source', 'top', 'name']);

function json(res, status, body) {
    res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
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
    const opts = { ...DEFAULTS, ...options, compile: { ...DEFAULTS.compile, ...(options.compile || {}) } };
    const { world, log = () => {} } = opts;
    const sandbox = createSandbox({ wasm: opts.wasm, ...opts.compile, testHooks: opts.testHooks === true, log });
    const plots = new Plots({ world, count: opts.plots, plotSize: opts.plotSize, plotY: opts.plotY, idleSeconds: opts.idleSeconds, maxSeconds: opts.maxSeconds, log });

    let tickTime = null;
    const sampleTick = async () => {
        try {
            tickTime = await world.tickTime();
        } catch (err) {
            log(`tick time: ${err.message}`);
        }
    };
    await sampleTick();
    const sampler = setInterval(sampleTick, opts.tickEverySeconds * 1000);

    const status = () => ({
        live: true,
        plots: { total: plots.plots.length, free: plots.free.length },
        tick: tickTime,
        limits: { sourceBytes: LIMITS.sourceBytes, cells: LIMITS.cells, blocks: LIMITS.blocks, compileSeconds: opts.compile.seconds, plotSize: plots.plotSize },
        compiling: sandbox.busy,
    });

    async function submit(req, res) {
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

        const plot = plots.reserve(name);
        try {
            const built = await sandbox.compile({ source: body.source, top, name });
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
            });
            plots.controllerLeft(plot, { fresh: true }); // until a controller connects, the idle clock runs
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
            if (url.pathname === '/api/submit' && req.method === 'POST') return await submit(req, res);
            const plotPath = url.pathname.match(/^\/api\/plots\/(\d+)(\/trace)?$/);
            if (plotPath) {
                const plot = plots.get(plotPath[1]);
                if (!plot) return json(res, 404, { error: 'no such plot' });
                if (plotPath[2] && req.method === 'GET') return json(res, 200, plot.recording(opts.source));
                if (req.method === 'GET') return json(res, 200, plot.info());
                if (req.method === 'DELETE') {
                    if (url.searchParams.get('token') !== plot.token) return json(res, 403, { error: 'not your plot' });
                    await plots.release(plot, 'released by its owner');
                    return json(res, 200, { released: true });
                }
            }
            if (url.pathname.startsWith('/api/')) return json(res, 404, { error: 'no such endpoint' });
            if (req.method !== 'GET') return json(res, 405, { error: 'method not allowed' });
            const file = resolveStatic(req.url);
            if (!file || !(await stat(file).catch(() => null))?.isFile()) {
                res.writeHead(404);
                return res.end('not found');
            }
            res.writeHead(200, { 'content-type': TYPES[extname(file)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
            return res.end(await readFile(file));
        } catch (err) {
            const known = err instanceof PlotError || err instanceof SandboxError;
            if (!known) log(`request ${req.method} ${req.url} failed: ${err.stack || err}`);
            return json(res, known ? err.status : 500, { error: known ? err.message : 'internal error' });
        }
    });

    server.on('upgrade', (req, socket, head) => {
        const url = new URL(req.url, 'http://localhost');
        if (url.pathname !== '/live') {
            socket.write('HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n');
            socket.destroy();
            return;
        }
        const plot = plots.get(url.searchParams.get('plot') ?? '');
        if (!plot) {
            socket.write('HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n');
            socket.destroy();
            return;
        }
        const connection = acceptWebSocket(req, socket, head);
        if (!connection) return;
        const viewer = {
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
                await plot.setLever(message.set, message.on);
                connection.send(JSON.stringify({ ok: true, set: message.set, on: message.on }));
            } catch (err) {
                connection.send(JSON.stringify({ ok: false, error: err.message }));
            }
        };
        connection.onClose = () => {
            plot.viewers.delete(viewer);
            if (viewer.controls && plot.generation === generation) plots.controllerLeft(plot);
        };
    });

    await new Promise((ok) => server.listen(opts.port ?? 0, opts.host ?? '127.0.0.1', ok));
    const port = server.address().port;
    log(`live playground at http://127.0.0.1:${port}/`);
    return {
        server,
        port,
        plots,
        async close() {
            clearInterval(sampler);
            await plots.closeAll();
            server.closeAllConnections?.();
            await new Promise((ok) => server.close(() => ok()));
        },
    };
}

/// The backend with its own Minecraft server: launches it, then serves.
export async function startLive({ runDir, log = () => {}, ...options }) {
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
        minecraft,
        async close() {
            await live.close();
            await minecraft.close();
        },
    };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
    const args = process.argv.slice(2);
    const flag = (name, fallback) => {
        const i = args.indexOf(name);
        return i >= 0 ? args[i + 1] : fallback;
    };
    const { execFileSync } = await import('node:child_process');
    execFileSync('cargo', ['build', '-q', '-p', 'redfab-wasm', '--target', 'wasm32-unknown-unknown', '--release'], { cwd: root, stdio: 'inherit' });
    const live = await startLive({
        runDir: join(root, flag('--run-dir', 'build/mc/live')),
        port: Number(flag('--port', 8766)),
        plots: Number(flag('--plots', DEFAULTS.plots)),
        log: (m) => console.log(`[live] ${m}`),
    });
    const stop = async () => {
        console.log('[live] stopping');
        await live.close();
        process.exit(0);
    };
    process.on('SIGINT', stop);
    process.on('SIGTERM', stop);
}
