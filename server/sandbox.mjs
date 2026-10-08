// Compiles submissions in a child process, one at a time, and kills a child
// that runs past the time limit or grows past the memory limit. The queue is
// bounded: beyond it, submissions are turned away at once. A source seen
// before is answered from a cache keyed by its hash, so the examples cost
// nothing after the first time and a repeated hostile source cannot hold the
// compiler again.

import { execFile, fork } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const child = join(dirname(fileURLToPath(import.meta.url)), 'compile_child.mjs');

export class SandboxError extends Error {
    constructor(message, status = 400) {
        super(message);
        this.status = status;
    }
}

/// The child's resident set size in bytes, or null if it cannot be read.
async function residentBytes(pid) {
    try {
        const status = await readFile(`/proc/${pid}/status`, 'utf8');
        const match = status.match(/VmRSS:\s+(\d+) kB/);
        if (match) return Number(match[1]) * 1024;
    } catch {
        // not Linux
    }
    return new Promise((resolve) => {
        execFile('ps', ['-o', 'rss=', '-p', String(pid)], (err, stdout) => {
            const kb = Number(String(stdout).trim());
            resolve(err || !Number.isFinite(kb) ? null : kb * 1024);
        });
    });
}

export function createSandbox({ wasm, seconds, memoryMb, queue, cacheEntries = 32, testHooks = false, log = () => {} }) {
    let running = null;
    const waiting = [];
    const cache = new Map(); // source hash -> { built } or { error }, in order of last use
    const stats = { hits: 0, misses: 0 };

    const keyOf = (request) => createHash('sha256').update(`${request.top}\0${request.name}\0${request.source}`).digest('hex');

    function remember(key, entry) {
        cache.delete(key);
        cache.set(key, entry);
        while (cache.size > cacheEntries) cache.delete(cache.keys().next().value);
    }

    function next() {
        if (running || !waiting.length) return;
        running = waiting.shift();
        run(running.request).then(running.resolve, running.reject).finally(() => {
            running = null;
            next();
        });
    }

    function run(request) {
        return new Promise((resolve, reject) => {
            const started = Date.now();
            // The child sees untrusted source, so it gets only the environment it needs.
            const env = { REDFAB_WASM: wasm, REDFAB_TEST_HOOKS: testHooks ? '1' : '' };
            for (const name of ['PATH', 'HOME', 'TMPDIR', 'LANG', 'LC_ALL']) if (process.env[name] !== undefined) env[name] = process.env[name];
            const proc = fork(child, [], {
                env,
                stdio: ['ignore', 'ignore', 'pipe', 'ipc'],
                serialization: 'json',
            });
            let stderr = '';
            proc.stderr.on('data', (chunk) => {
                stderr = (stderr + chunk).slice(-2000);
            });
            let done = false;
            const finish = (fn, value) => {
                if (done) return;
                done = true;
                clearTimeout(clock);
                clearInterval(meter);
                if (proc.exitCode === null && !proc.killed) proc.kill('SIGKILL');
                fn(value);
            };
            const clock = setTimeout(() => {
                log(`compile of ${request.name}: stopped after ${seconds} s`);
                finish(reject, new SandboxError(`compiling took longer than ${seconds} s; stopped. Try a smaller design (about 100 cells compile in time)`));
            }, seconds * 1000);
            const meter = setInterval(async () => {
                const bytes = await residentBytes(proc.pid);
                if (bytes !== null && bytes > memoryMb * 1024 * 1024 && !done) {
                    log(`compile of ${request.name}: used ${Math.round(bytes / 1048576)} MB, more than the ${memoryMb} MB allowed`);
                    finish(reject, new SandboxError(`compiling needed more than ${memoryMb} MB of memory; stopped`));
                }
            }, 250);
            proc.on('message', (message) => {
                if (message.ready) {
                    proc.send(request);
                    return;
                }
                if (message.ok) {
                    log(`compile of ${request.name}: ${Date.now() - started} ms`);
                    finish(resolve, {
                        design: JSON.parse(message.designText),
                        designText: message.designText,
                        structure: Uint8Array.from(message.structure.match(/../g), (h) => parseInt(h, 16)),
                        report: message.report,
                    });
                } else {
                    finish(reject, new SandboxError(message.error));
                }
            });
            proc.on('exit', (code, signal) => {
                finish(reject, new SandboxError(`the compiler stopped unexpectedly (${signal || code})${stderr ? `: ${stderr.trim().split('\n').at(-1)}` : ''}`, 500));
            });
            proc.on('error', (err) => finish(reject, new SandboxError(`the compiler could not start: ${err.message}`, 500)));
        });
    }

    return {
        /// Compiles a submission, or answers from the cache. Rejects with a SandboxError.
        compile(request) {
            const key = keyOf(request);
            const known = cache.get(key);
            if (known) {
                stats.hits += 1;
                remember(key, known);
                return known.error ? Promise.reject(known.error) : Promise.resolve({ ...known.built });
            }
            if (waiting.length >= queue) {
                return Promise.reject(new SandboxError('the compiler is busy; try again in a moment', 429));
            }
            stats.misses += 1;
            return new Promise((resolve, reject) => {
                waiting.push({ request, resolve, reject });
                next();
            }).then((built) => {
                remember(key, { built });
                return { ...built };
            }, (err) => {
                // A refusal of the source is as good next time; a crash or a busy compiler is not.
                if (err instanceof SandboxError && err.status === 400) remember(key, { error: err });
                throw err;
            });
        },
        get busy() {
            return (running ? 1 : 0) + waiting.length;
        },
        get cache() {
            return { entries: cache.size, hits: stats.hits, misses: stats.misses };
        },
    };
}
