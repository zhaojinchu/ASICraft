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
// /core/redfab_wasm.wasm       the core, built for wasm32 in release mode
// /api/status                  {"live": false}: no live backend here (see server/)
//
// Every answer carries a Content-Security-Policy that keeps the page to this
// origin (see `contentSecurityPolicy`). `serveStatic`, `resolve` and
// `cacheControl` are shared with the live backend (server/).

import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildWasm, root } from './common.mjs';
const TYPES = {
    '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.mjs': 'text/javascript; charset=utf-8',
    '.wasm': 'application/wasm', '.json': 'application/json', '.sv': 'text/plain; charset=utf-8', '.tar': 'application/x-tar',
    '.css': 'text/css', '.map': 'application/json', '.d.ts': 'text/plain',
};

// The only packages the page loads; nothing else under node_modules is served.
export const PACKAGES = ['@yowasp/yosys', 'three'];

/// The file a request path maps to, or null when there is none to serve.
export function resolve(url) {
    let path;
    try {
        path = normalize(decodeURIComponent(url.split('?')[0]));
    } catch {
        return null; // a malformed escape such as /%
    }
    if (path.includes('..') || path.includes('\0')) return null;
    if (path === '/core/redfab_wasm.wasm') return join(root, 'target/wasm32-unknown-unknown/release/redfab_wasm.wasm');
    const versioned = path.match(/^\/pkg\/((?:@[^/@]+\/)?[^/@]+)@[^/]+\/(.+)$/);
    if (versioned) return PACKAGES.includes(versioned[1]) ? join(root, 'node_modules', versioned[1], versioned[2]) : null;
    if (path.startsWith('/node_modules/')) {
        const name = PACKAGES.find((p) => path.startsWith(`/node_modules/${p}/`));
        return name ? join(root, path) : null;
    }
    if (path.startsWith('/designs/')) return join(root, path);
    return join(root, 'playground', path === '/' ? 'index.html' : path);
}

/// The cache-control header for a request path: the page and the designs
/// are never cached, versioned packages are kept for good, and the core and
/// the unversioned package paths are kept but revalidated (with an ETag).
export function cacheControl(url) {
    const path = url.split('?')[0];
    if (path.startsWith('/pkg/')) return 'public, max-age=31536000, immutable';
    if (path === '/core/redfab_wasm.wasm' || path.startsWith('/node_modules/')) return 'no-cache';
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

/// Answers a static request: the file, 304 when the client has it, 404 otherwise.
export async function serveStatic(req, res) {
    const file = resolve(req.url);
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
    const etag = `"${info.size.toString(16)}-${Math.floor(info.mtimeMs).toString(16)}"`;
    const body = await readFile(file);
    const headers = {
        'content-type': TYPES[extname(file)] ?? 'application/octet-stream',
        'cache-control': cacheControl(req.url),
        etag,
        'content-security-policy': contentSecurityPolicy(extname(file) === '.html' ? body.toString('utf8') : ''),
        'x-content-type-options': 'nosniff',
    };
    if (req.headers['if-none-match'] === etag) {
        res.writeHead(304, headers);
        return res.end();
    }
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
    // Build the core first, so the page is always running the current one.
    buildWasm();
    const { port } = await serve(Number(process.argv[2] || 8765));
    console.log(`playground at http://127.0.0.1:${port}/`);
}
