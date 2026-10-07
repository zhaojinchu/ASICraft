// The compiler core: the WebAssembly build of redfab-core behind one entry
// point, `redfab_call`, that takes and returns JSON text.
//
//   const core = await loadCore(wasmBytes);
//   core.call({ op: 'build', mapped, top, name })  -> { ok, design, structure, report } | { ok: false, error }
//   core.call({ op: 'library' })                   -> { liberty, models, map, equiv }
//   core.call({ op: 'sim_new', design })           -> { handle }
//   core.call({ op: 'sim_set', handle, input, on })   -> { changes: [[pos, state], ...] }  (what changed at once)
//   core.call({ op: 'sim_step', handle, ticks })   -> { tick, changes: [[pos, state], ...], lamps: [0|1, ...] }
//   core.call({ op: 'sim_trace', handle })         -> { fixture, trace }  (the trace format, for comparing with the native simulator)
//   core.call({ op: 'sim_free', handle })

export async function loadCore(wasmBytes) {
    const { instance } = await WebAssembly.instantiate(wasmBytes, {});
    const { memory, redfab_alloc, redfab_call, redfab_free } = instance.exports;
    if (!redfab_call) {
        throw new Error('this redfab_wasm.wasm has no redfab_call; rebuild it');
    }
    const encoder = new TextEncoder();
    const decoder = new TextDecoder();
    return {
        call(request) {
            const input = encoder.encode(JSON.stringify(request));
            const inPtr = redfab_alloc(input.length);
            new Uint8Array(memory.buffer, inPtr, input.length).set(input);
            const outPtr = redfab_call(inPtr, input.length);
            redfab_free(inPtr, input.length);
            const length = new DataView(memory.buffer).getUint32(outPtr, true);
            const reply = JSON.parse(decoder.decode(new Uint8Array(memory.buffer, outPtr + 4, length)));
            redfab_free(outPtr, 4 + length);
            if (reply.ok === false) {
                throw new Error(reply.error);
            }
            return reply;
        },
    };
}
