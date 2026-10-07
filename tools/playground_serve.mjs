// Serves the playground for local use and for the checks:
//
//   node tools/playground_serve.mjs [PORT]
//
// /                      the page (playground/)
// /node_modules/...      YoWASP Yosys and three.js
// /designs/NAME/top.sv   the example designs
// /core/redfab_wasm.wasm the core, built for wasm32 in release mode
// /api/status            {"live": false}: no live backend here (see server/)

import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { dirname, extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const TYPES = {
    '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.mjs': 'text/javascript; charset=utf-8',
    '.wasm': 'application/wasm', '.json': 'application/json', '.sv': 'text/plain; charset=utf-8', '.tar': 'application/x-tar',
    '.css': 'text/css', '.map': 'application/json', '.d.ts': 'text/plain',
};

export function resolve(url) {
    const path = normalize(decodeURIComponent(url.split('?')[0]));
    if (path.includes('..')) return null;
    if (path === '/core/redfab_wasm.wasm') return join(root, 'target/wasm32-unknown-unknown/release/redfab_wasm.wasm');
    if (path.startsWith('/node_modules/') || path.startsWith('/designs/')) return join(root, path);
    return join(root, 'playground', path === '/' ? 'index.html' : path);
}

export function serve(port = 0) {
    const server = createServer(async (req, res) => {
        // The page asks whether a live backend (server/) is behind it. Here there is none.
        if (req.url.split('?')[0] === '/api/status') {
            res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
            return res.end(JSON.stringify({ live: false }));
        }
        const file = resolve(req.url);
        try {
            if (!file || !(await stat(file)).isFile()) throw new Error('not found');
            res.writeHead(200, { 'content-type': TYPES[extname(file)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
            res.end(await readFile(file));
        } catch {
            res.writeHead(404);
            res.end('not found');
        }
    });
    return new Promise((ok) => server.listen(port, '127.0.0.1', () => ok({ server, port: server.address().port })));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
    // Build the core first, so the page is always running the current one.
    const { execFileSync } = await import('node:child_process');
    execFileSync('cargo', ['build', '-q', '-p', 'redfab-wasm', '--target', 'wasm32-unknown-unknown', '--release'], { cwd: root, stdio: 'inherit' });
    const { port } = await serve(Number(process.argv[2] || 8765));
    console.log(`playground at http://127.0.0.1:${port}/`);
}
