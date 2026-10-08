// Builds each design with the WebAssembly build of the core and compares the
// result, byte for byte, with what the native `redfab build` wrote.
//
//   node tools/wasm_build_check.mjs NAME...
//
// Run `redfab build` for each design first.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { buildWasm, root } from './common.mjs';

const wasm = readFileSync(buildWasm());
const { instance } = await WebAssembly.instantiate(wasm, {});
const { memory, redfab_alloc, redfab_call, redfab_free } = instance.exports;

function build(request) {
    const input = new TextEncoder().encode(JSON.stringify(request));
    const inPtr = redfab_alloc(input.length);
    new Uint8Array(memory.buffer, inPtr, input.length).set(input);
    const outPtr = redfab_call(inPtr, input.length);
    redfab_free(inPtr, input.length);
    const length = new DataView(memory.buffer).getUint32(outPtr, true);
    const reply = JSON.parse(new TextDecoder().decode(new Uint8Array(memory.buffer, outPtr + 4, length)));
    redfab_free(outPtr, 4 + length);
    return reply;
}

let failed = false;
for (const name of process.argv.slice(2)) {
    const dir = join(root, 'build/designs', name);
    const reply = build({ mapped: readFileSync(join(dir, 'mapped.json'), 'utf8'), top: 'top', name });
    if (!reply.ok) {
        console.log(`${name}: wasm build failed: ${reply.error}`);
        failed = true;
        continue;
    }
    const sameDesign = reply.design === readFileSync(join(dir, 'design.json'), 'utf8');
    const sameStructure = reply.structure === readFileSync(join(dir, 'world.nbt')).toString('hex');
    console.log(`${name}: design.json ${sameDesign ? 'identical' : 'DIFFERS'}, world.nbt ${sameStructure ? 'identical' : 'DIFFERS'} (${reply.structure.length / 2} bytes)`);
    failed ||= !sameDesign || !sameStructure;
}

// A request the core must refuse, to show failures come back as errors, not crashes.
const bad = build({ mapped: '{"modules":{}}', top: 'top', name: 'bad' });
console.log(`bad request: ${bad.ok ? 'ACCEPTED, which is wrong' : 'refused (' + bad.error + ')'}`);
failed ||= bad.ok;
process.exit(failed ? 1 : 0);
