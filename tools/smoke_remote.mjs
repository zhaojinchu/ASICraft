// A deployment's smoke test, from any machine with this checkout:
//
//   node tools/smoke_remote.mjs https://NAME
//
// The status must say live; the core the page compiles with must be served;
// the inverter example is submitted; its plot's
// stream is opened with the token; the lever goes on and the lamp must go off
// within the settle time, then back; the plot is released. Exit 0 only if every
// step passed in time. deploy/README.md runs it after an install and the soak
// runs it on a schedule; tools/live_test.mjs runs it against the backend with a
// simulated world.

import { fileURLToPath } from 'node:url';
import { api, connect, designSource } from './live_common.mjs';

const EXAMPLE = 'not1'; // y = ~a: the lamp is on at rest and goes off when the lever goes on

/// Runs the smoke against `base` (an http or https origin). Resolves with the
/// plot used and the design placed; rejects on the first step that fails.
export async function smoke(base, { log = () => {}, settleMs = 20_000 } = {}) {
    base = base.replace(/\/$/, '');
    const wsBase = base.replace(/^http/, 'ws');
    const status = await api(base, '/api/status');
    if (status.status !== 200 || status.body.live !== true) {
        throw new Error(`not live: ${status.status} ${JSON.stringify(status.body).slice(0, 300)}`);
    }
    log(`live: ${status.body.plots.free} of ${status.body.plots.total} plots free`);
    // The browser compiles with the core too; a deployment serves it from where
    // --wasm points, which the first deploy got wrong (a 404 whose text the page
    // handed to WebAssembly.instantiate: "expected magic word").
    const core = await fetch(`${base}/core/redfab_wasm.wasm`);
    const bytes = core.ok ? new Uint8Array(await core.arrayBuffer()) : new Uint8Array();
    const magic = bytes.length >= 4 && bytes[0] === 0 && bytes[1] === 0x61 && bytes[2] === 0x73 && bytes[3] === 0x6d;
    if (!core.ok || core.headers.get('content-type') !== 'application/wasm' || !magic) {
        throw new Error(`core: ${core.status} ${core.headers.get('content-type')}, ${bytes.length} bytes${magic ? '' : ', not WebAssembly'}`);
    }
    log(`the core is served (${bytes.length} bytes)`);
    const reply = await api(base, '/api/submit', { method: 'POST', body: { source: designSource(EXAMPLE), top: 'top', name: EXAMPLE } });
    if (reply.status !== 200) throw new Error(`submit: ${reply.status} ${reply.body.error}`);
    const { plot, token, design } = reply.body;
    log(`${EXAMPLE} placed on plot ${plot}: ${design.size[0]} x ${design.size[2]} blocks, settle ${design.settle_ticks} ticks`);
    const client = connect(`${wsBase}/live?plot=${plot}&token=${token}`);
    try {
        await client.opened;
        // The hello carries the plot's state: live already when the game settled it
        // before this socket opened, else settling until the stream says live.
        const hello = await client.next((m) => m.hello !== undefined, 10_000);
        const state = hello.state === 'settling' ? (await client.next((m) => m.state !== undefined, settleMs)).state : hello.state;
        if (state !== 'live') throw new Error(`the plot is ${state}, not live`);
        log('the plot settled');
        const input = design.inputs[0].name;
        const lamp = (m, value) => Array.isArray(m.lamps) && m.lamps[0] === value && Array.isArray(m.inputs);
        client.send({ set: input, on: true });
        const ack = await client.next((m) => m.ok !== undefined, 10_000);
        if (!ack.ok) throw new Error(`lever ${input}: ${ack.error}`);
        const off = await client.next((m) => lamp(m, 0) && m.inputs[0] === 1, settleMs);
        log(`the lamp followed the lever at tick ${off.tick}`);
        client.send({ set: input, on: false });
        const back = await client.next((m) => m.ok !== undefined, 10_000);
        if (!back.ok) throw new Error(`lever ${input}: ${back.error}`);
        const on = await client.next((m) => lamp(m, 1) && m.inputs[0] === 0, settleMs);
        log(`and back at tick ${on.tick}`);
    } finally {
        client.close();
        const released = await api(base, `/api/plots/${plot}?token=${token}`, { method: 'DELETE' });
        log(`plot ${plot} released (${released.status})`);
    }
    return { plot, design };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
    const base = process.argv[2];
    if (!base || !/^https?:\/\//.test(base)) {
        console.error('usage: node tools/smoke_remote.mjs https://NAME');
        process.exit(2);
    }
    try {
        await smoke(base, { log: (line) => console.log(`  ${line}`) });
        console.log(`${base}: the live playground answers, places, runs and releases`);
    } catch (err) {
        console.error(`${base}: ${err.message}`);
        process.exit(1);
    }
}
