// Shared by the live playground's checks: trace comparison in the trace
// format (from server/trace.mjs, the same code the backend records with), a
// WebSocket client with a message queue, and small waits.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { root, wasmPath } from './common.mjs';

export { root, wasmPath };
export { canonicalState, changesAt, firstTraceDifference, stateAt } from '../server/trace.mjs';

export const nativeDesign = (name) => readFileSync(join(root, 'build/designs', name, 'design.json'), 'utf8');
export const designSource = (name) => readFileSync(join(root, 'designs', name, 'top.sv'), 'utf8');

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

export async function api(base, path, { method = 'GET', body, headers = {} } = {}) {
    const reply = await fetch(`${base}${path}`, {
        method,
        headers: { ...(body === undefined ? {} : { 'content-type': 'application/json' }), ...headers },
        body: body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body),
    });
    const text = await reply.text();
    let json = null;
    try {
        json = JSON.parse(text);
    } catch {
        json = { error: text };
    }
    return { status: reply.status, body: json, headers: reply.headers };
}
