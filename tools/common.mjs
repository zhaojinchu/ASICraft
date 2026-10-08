// Shared by the live checks (live_test.mjs, live_check.mjs) and the live
// backend's command line: the wasm build and the browser lookup.

import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const root = join(dirname(fileURLToPath(import.meta.url)), '..');
export const wasmPath = join(root, 'target/wasm32-unknown-unknown/release/redfab_wasm.wasm');

/// Builds the core for wasm32 in release mode, so what runs is the current one.
export function buildWasm() {
    execFileSync('cargo', ['build', '-q', '-p', 'redfab-wasm', '--target', 'wasm32-unknown-unknown', '--release'], { cwd: root, stdio: 'inherit' });
    return wasmPath;
}

const BROWSERS = [
    process.env.REDFAB_BROWSER,
    '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser',
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome', '/usr/bin/brave-browser',
].filter(Boolean);

/// The first Chromium on this machine (REDFAB_BROWSER first), or null.
export function findBrowser() {
    return BROWSERS.find((p) => existsSync(p)) ?? null;
}
