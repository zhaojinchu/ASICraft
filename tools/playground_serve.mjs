// Serves the playground for local use and for the checks:
//
//   node tools/playground_serve.mjs [PORT]
//
// /                            the page (playground/)
// /pkg/NAME@VERSION/...        YoWASP Yosys and three.js from node_modules; the
//                              version in the path is a cache key the page sets
//                              (index.html, worker.js), so these are immutable
// /node_modules/...            the same two packages, unversioned (revalidated)
// /designs/NAME/top.sv         the example designs
// /designs/NAME/design.json    the opening circuit, as built by `prepareOpening`
//   (also report.txt, world.nbt) from the example, so the page can show it at
//                              once without fetching the compiler
// /fonts/...                   the heading font, with its license
// /core/redfab_wasm.wasm       the core, built for wasm32 in release mode (the live
//                              backend serves the file its --wasm names instead)
// /api/status                  {"live": false}: no live backend here (see server/)
//
// Every answer carries a Content-Security-Policy that keeps the page to this
// origin (see `contentSecurityPolicy`). `serveStatic`, `resolve`,
// `cacheControl` and `prepareOpening` are shared with the live backend (server/).

import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { readFile, rename, stat, writeFile } from 'node:fs/promises';
import { dirname, extname, join, posix } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { Worker } from 'node:worker_threads';
import zlib from 'node:zlib';
import { buildWasm, root } from './common.mjs';
const TYPES = {
    '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.mjs': 'text/javascript; charset=utf-8',
    '.wasm': 'application/wasm', '.json': 'application/json', '.sv': 'text/plain; charset=utf-8', '.tar': 'application/x-tar',
    '.css': 'text/css', '.map': 'application/json', '.d.ts': 'text/plain', '.txt': 'text/plain; charset=utf-8',
    '.ttf': 'font/ttf', '.nbt': 'application/octet-stream',
};

const brotli = promisify(zlib.brotliCompress);
const zstd = zlib.zstdCompress ? promisify(zlib.zstdCompress) : null; // Node 26 has it; an older one sends br or gzip
const gzip = promisify(zlib.gzip);
const Z = zlib.constants;

// The only packages the page loads; nothing else under node_modules is served.
export const PACKAGES = ['@yowasp/yosys', 'three'];

/// The core served at /core/redfab_wasm.wasm unless a caller names another: the
/// release build in this checkout. A deployment has no Rust and no target/; the
/// live backend passes the file deploy/ship_core.sh put in dist/ (its --wasm),
/// the same one its own compiles run.
export const CORE_DEFAULT = join(root, 'target/wasm32-unknown-unknown/release/redfab_wasm.wasm');

// The example the page opens with, already built: the counter, which moves on
// its own once the page switches its enable on.
export const OPENING = 'counter4';
const OPENING_FILES = ['design.json', 'report.txt', 'world.nbt'];
const openingDir = (name) => join(root, 'build/playground/opening', name);

/// Builds the opening example once with the page's own pipeline (YoWASP Yosys
/// and the core at `wasm`, in a worker thread that is gone afterwards) and
/// keeps its design, report and structure under build/playground/opening/,
/// keyed by the core and the source, so a changed core rebuilds it. Returns
/// the directory and whether it was built now.
export async function prepareOpening({ wasm, name = OPENING, log = () => {} }) {
    const source = readFileSync(join(root, 'designs', name, 'top.sv'), 'utf8');
    const key = createHash('sha256').update(readFileSync(wasm)).update(source).digest('hex');
    const dir = openingDir(name);
    const keyFile = join(dir, 'key');
    if (existsSync(keyFile) && readFileSync(keyFile, 'utf8') === key && OPENING_FILES.every((f) => existsSync(join(dir, f)))) {
        return { dir, built: false };
    }
    log(`building the opening circuit (${name})`);
    const started = Date.now();
    const built = await new Promise((resolve, reject) => {
        const worker = new Worker(join(root, 'tools/playground_worker.mjs'), { workerData: { wasm } });
        worker.on('message', (reply) => {
            if (reply.ready) return worker.postMessage({ source, top: 'top', name });
            worker.terminate();
            if (reply.ok) resolve(reply);
            else reject(new Error(`the opening circuit ${name} does not build: ${reply.error}`));
        });
        worker.on('error', reject);
    });
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, 'design.json'), built.designText);
    writeFileSync(join(dir, 'report.txt'), built.report);
    writeFileSync(join(dir, 'world.nbt'), Buffer.from(built.structure, 'hex'));
    writeFileSync(keyFile, key);
    log(`the opening circuit ${name} built in ${Math.round((Date.now() - started) / 1000)} s`);
    return { dir, built: true };
}

/// The file a request path maps to, or null when there is none to serve.
export function resolve(url, core = CORE_DEFAULT) {
    let path;
    try {
        path = posix.normalize(decodeURIComponent(url.split('?')[0]));
    } catch {
        return null; // a malformed escape such as /%
    }
    if (path.includes('..') || path.includes('\0')) return null;
    if (path === '/core/redfab_wasm.wasm') return core;
    const versioned = path.match(/^\/pkg\/((?:@[^/@]+\/)?[^/@]+)@[^/]+\/(.+)$/);
    if (versioned) return PACKAGES.includes(versioned[1]) ? join(root, 'node_modules', versioned[1], versioned[2]) : null;
    if (path.startsWith('/node_modules/')) {
        const name = PACKAGES.find((p) => path.startsWith(`/node_modules/${p}/`));
        return name ? join(root, path) : null;
    }
    const opening = path.match(/^\/designs\/([^/]+)\/(design\.json|report\.txt|world\.nbt)$/);
    if (opening) return join(openingDir(opening[1]), opening[2]);
    if (path.startsWith('/designs/')) return join(root, path);
    return join(root, 'playground', path === '/' ? 'index.html' : path);
}

/// The cache-control header for a request path: the page and the designs
/// are never cached, versioned packages are kept for good, and the core, the
/// font, the opening circuit's files and the unversioned package paths are
/// kept but revalidated (with an ETag).
export function cacheControl(url) {
    const path = url.split('?')[0];
    if (path.startsWith('/pkg/')) return 'public, max-age=31536000, immutable';
    if (path === '/core/redfab_wasm.wasm' || path.startsWith('/node_modules/') || path.startsWith('/fonts/')) return 'no-cache';
    if (/^\/designs\/[^/]+\/(design\.json|report\.txt|world\.nbt)$/.test(path)) return 'no-cache';
    return 'no-store';
}

/// The Content-Security-Policy for a response: scripts, workers, styles,
/// fetches and WebSockets from this origin only, WebAssembly allowed
/// ('wasm-unsafe-eval': the core and the Yosys core are compiled from bytes),
/// and for a page, its own inline script and style blocks (the import map and
/// the stylesheet) allowed by hash, so no other inline code runs. A worker
/// takes the policy from its own script's response, so every answer carries it.
export function contentSecurityPolicy(html = '') {
    const hashes = (tag) => [...html.matchAll(new RegExp(`<${tag}\\b[^>]*>([\\s\\S]*?)</${tag}>`, 'g'))]
        .filter((m) => !/\ssrc=/.test(m[0]))
        .map((m) => ` 'sha256-${createHash('sha256').update(m[1]).digest('base64')}'`)
        .join('');
    return [
        "default-src 'self'",
        `script-src 'self' 'wasm-unsafe-eval'${hashes('script')}`,
        `style-src 'self'${hashes('style')}`,
        "worker-src 'self'",
        "connect-src 'self'",
        "img-src 'self' data:",
        "object-src 'none'",
        "base-uri 'self'",
        "frame-ancestors 'none'",
    ].join('; ');
}

// --- versioned package files, precompressed once at the strongest settings
//
// The proxy compresses on the fly at its fast default, which left the Yosys core
// at 14.3 MB, and does not compress its resources tar at all (10.6 MB, a type
// outside its list): about 25 MB before a first compile, from a box far from
// most visitors. Compressed once and kept, they are 9.5 and 0.4 MB with brotli
// (10.1 and 0.5 with zstd). A /pkg path carries its version, so a file there
// never changes; its variants live under build/pkg-compressed/, keyed by the
// file's size and time as well, and are made in the background on the first
// request (the raw file is served meanwhile), one file at a time.

export const PRECOMPRESS_MIN_BYTES = 1_000_000;
// The client's preference decides among these; the order is ours: smallest first.
const ENCODINGS = [['br', 'br'], ['zstd', 'zst'], ['gzip', 'gz']].filter(([name]) => name !== 'zstd' || zstd);
const compressedRoot = join(root, 'build/pkg-compressed');
const making = new Map(); // variant base path -> the promise making its variants
let queue = Promise.resolve();

function variantBase(url, info) {
    const path = posix.normalize(decodeURIComponent(url.split('?')[0])).replace(/^[/\\]+/, '');
    return join(compressedRoot, `${path}.${info.size.toString(16)}-${Math.floor(info.mtimeMs).toString(16)}`);
}

/// Makes the three variants of `file` under `base`, once; resolves when they are on disk.
export function precompress(file, base) {
    if (!making.has(base)) {
        const job = queue.then(async () => {
            if (ENCODINGS.every(([, ext]) => existsSync(`${base}.${ext}`))) return;
            const raw = await readFile(file);
            mkdirSync(dirname(base), { recursive: true });
            const levels = {
                br: (b) => brotli(b, { params: { [Z.BROTLI_PARAM_QUALITY]: 11, [Z.BROTLI_PARAM_SIZE_HINT]: b.length } }),
                zst: (b) => zstd(b, { params: { [Z.ZSTD_c_compressionLevel]: 19 } }),
                gz: (b) => gzip(b, { level: 9 }),
            };
            for (const [, ext] of ENCODINGS) {
                const out = `${base}.${ext}`;
                if (existsSync(out)) continue;
                const packed = await levels[ext](raw);
                await writeFile(`${out}.tmp`, packed);
                await rename(`${out}.tmp`, out); // never half a file under the served name
            }
        });
        queue = job.catch(() => {});
        making.set(base, job);
    }
    return making.get(base);
}

/// The encoding to send: the first of ours the client accepts (q > 0) whose variant is ready.
function pickEncoding(acceptEncoding, base) {
    const accepted = new Set();
    for (const part of String(acceptEncoding ?? '').split(',')) {
        const [token, ...params] = part.trim().toLowerCase().split(';');
        const q = params.map((p) => p.trim()).find((p) => p.startsWith('q='));
        if (token && !(q && Number(q.slice(2)) === 0)) accepted.add(token);
    }
    return ENCODINGS.find(([name, ext]) => accepted.has(name) && existsSync(`${base}.${ext}`)) ?? null;
}

/// Answers a static request: the file, 304 when the client has it, 404 otherwise.
/// `core` is the file served at /core/redfab_wasm.wasm (see CORE_DEFAULT). A
/// versioned package file of PRECOMPRESS_MIN_BYTES or more goes out precompressed
/// when the client accepts one of its variants and the variant is ready.
export async function serveStatic(req, res, { core = CORE_DEFAULT } = {}) {
    const file = resolve(req.url, core);
    let info;
    try {
        info = file ? await stat(file) : null;
    } catch {
        info = null;
    }
    if (!info?.isFile()) {
        res.writeHead(404, { 'content-type': 'text/plain', 'cache-control': 'no-store', 'content-security-policy': contentSecurityPolicy() });
        return res.end('not found');
    }
    let etag = `"${info.size.toString(16)}-${Math.floor(info.mtimeMs).toString(16)}"`;
    const headers = {
        'content-type': TYPES[extname(file)] ?? 'application/octet-stream',
        'cache-control': cacheControl(req.url),
        'x-content-type-options': 'nosniff',
    };
    let path = file;
    if (req.url.startsWith('/pkg/') && info.size >= PRECOMPRESS_MIN_BYTES) {
        const base = variantBase(req.url, info);
        precompress(file, base).catch(() => {}); // a failure leaves the raw file served
        const chosen = pickEncoding(req.headers['accept-encoding'], base);
        headers.vary = 'Accept-Encoding';
        if (chosen) {
            const [name, ext] = chosen;
            path = `${base}.${ext}`;
            headers['content-encoding'] = name;
            etag = `${etag.slice(0, -1)}-${ext}"`;
        }
    }
    headers.etag = etag;
    if (req.headers['if-none-match'] === etag) {
        res.writeHead(304, { ...headers, 'content-security-policy': contentSecurityPolicy() });
        return res.end();
    }
    const body = await readFile(path);
    headers['content-security-policy'] = contentSecurityPolicy(extname(file) === '.html' ? body.toString('utf8') : '');
    res.writeHead(200, headers);
    return res.end(body);
}

export function serve(port = 0) {
    const server = createServer(async (req, res) => {
        try {
            // The page asks whether a live backend (server/) is behind it. Here there is none.
            if (req.url.split('?')[0] === '/api/status') {
                res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
                return res.end(JSON.stringify({ live: false }));
            }
            await serveStatic(req, res);
        } catch (err) {
            // Nothing a request does may take the server down.
            if (!res.headersSent) res.writeHead(500, { 'content-type': 'text/plain' });
            res.end(`error: ${err.message}`);
        }
    });
    return new Promise((ok) => server.listen(port, '127.0.0.1', () => ok({ server, port: server.address().port })));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
    // Build the core first, so the page is always running the current one, and
    // the opening circuit with it.
    const wasm = buildWasm();
    await prepareOpening({ wasm, log: console.log });
    const { port } = await serve(Number(process.argv[2] || 8765));
    console.log(`playground at http://127.0.0.1:${port}/`);
}
