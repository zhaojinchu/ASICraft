// Shared by the live playground's checks: trace comparison in the trace
// format, a WebSocket client with a message queue, and small waits.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const root = join(dirname(fileURLToPath(import.meta.url)), '..');
export const wasmPath = join(root, 'target/wasm32-unknown-unknown/release/redfab_wasm.wasm');

export const nativeDesign = (name) => readFileSync(join(root, 'build/designs', name, 'design.json'), 'utf8');
export const designSource = (name) => readFileSync(join(root, 'designs', name, 'top.sv'), 'utf8');

/// The block states inside the box after `tick` ticks of a trace, air left out.
export function stateAt(trace, tick) {
    const state = new Map(Object.entries(trace.initial));
    for (const entry of trace.ticks) {
        if (entry.tick > tick) break;
        for (const [pos, block] of Object.entries(entry.changes)) {
            if (block === 'minecraft:air') state.delete(pos);
            else state.set(pos, block);
        }
    }
    return state;
}

/// The first tick at which two traces disagree, as text, or null. Pending ticks
/// are not compared: a live source cannot report them.
export function firstTraceDifference(a, b) {
    const describe = (sa, sb) => {
        const diffs = [];
        for (const pos of [...new Set([...sa.keys(), ...sb.keys()])].sort()) {
            if ((sa.get(pos) ?? 'minecraft:air') !== (sb.get(pos) ?? 'minecraft:air')) {
                diffs.push(`${pos}: ${sa.get(pos) ?? 'minecraft:air'} vs ${sb.get(pos) ?? 'minecraft:air'}`);
            }
            if (diffs.length >= 5) break;
        }
        return diffs.join('; ');
    };
    const sa0 = stateAt(a, 0);
    const sb0 = stateAt(b, 0);
    if (describe(sa0, sb0)) return `initial state differs: ${describe(sa0, sb0)}`;
    const last = Math.max(a.ticks.at(-1)?.tick ?? 0, b.ticks.at(-1)?.tick ?? 0);
    const ticks = new Set([...a.ticks.map((t) => t.tick), ...b.ticks.map((t) => t.tick)]);
    for (let t = 1; t <= last; t++) {
        if (!ticks.has(t)) continue;
        const sa = stateAt(a, t);
        const sb = stateAt(b, t);
        const diff = describe(sa, sb);
        if (diff) return `tick ${t}: ${diff}`;
    }
    return null;
}

/// How many entries of a trace change the given position.
export function changesAt(trace, pos) {
    return trace.ticks.filter((t) => pos in t.changes).length;
}

/// A WebSocket with a queue, so a test can wait for the next message.
export function connect(url) {
    const ws = new WebSocket(url);
    const queue = [];
    const waiting = [];
    let closed = null;
    ws.onmessage = (e) => {
        const message = JSON.parse(e.data);
        const index = waiting.findIndex((w) => w.predicate(message));
        if (index >= 0) waiting.splice(index, 1)[0].resolve(message);
        else queue.push(message);
    };
    ws.onclose = (e) => {
        closed = e;
        for (const w of waiting.splice(0)) w.reject(new Error(`closed: ${e.code} ${e.reason}`));
    };
    ws.onerror = () => {};
    return {
        ws,
        opened: new Promise((ok, bad) => {
            ws.onopen = ok;
            const fail = () => bad(new Error('could not connect'));
            ws.addEventListener('close', fail, { once: true });
            ws.addEventListener('error', fail, { once: true });
        }),
        send(message) {
            ws.send(JSON.stringify(message));
        },
        /// The next message matching `predicate` (queued or to come), within `ms`.
        next(predicate = () => true, ms = 10_000) {
            return new Promise((resolve, reject) => {
                const index = queue.findIndex(predicate);
                if (index >= 0) {
                    resolve(queue.splice(index, 1)[0]);
                    return;
                }
                if (closed) {
                    reject(new Error(`closed: ${closed.code} ${closed.reason}`));
                    return;
                }
                const entry = { predicate, reject };
                const deadline = setTimeout(() => {
                    waiting.splice(waiting.indexOf(entry), 1);
                    reject(new Error(`no matching message within ${ms} ms`));
                }, ms);
                entry.resolve = (message) => {
                    clearTimeout(deadline);
                    resolve(message);
                };
                waiting.push(entry);
            });
        },
        /// Forgets queued messages, so the next wait is for a fresh one.
        drain() {
            queue.length = 0;
        },
        get closed() {
            return closed;
        },
        close() {
            ws.close();
        },
    };
}

export const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms));

/// Polls `fn` until it returns a truthy value, within `ms`.
export async function waitFor(fn, ms = 10_000, every = 50) {
    const deadline = Date.now() + ms;
    for (;;) {
        const value = await fn();
        if (value) return value;
        if (Date.now() > deadline) throw new Error(`timed out after ${ms} ms`);
        await sleep(every);
    }
}

export async function api(base, path, { method = 'GET', body } = {}) {
    const reply = await fetch(`${base}${path}`, {
        method,
        headers: body === undefined ? {} : { 'content-type': 'application/json' },
        body: body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body),
    });
    const text = await reply.text();
    let json = null;
    try {
        json = JSON.parse(text);
    } catch {
        json = { error: text };
    }
    return { status: reply.status, body: json };
}
