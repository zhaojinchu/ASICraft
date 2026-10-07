// From SystemVerilog source to a built design, the way `redfab build` does it:
// synthesis to the tile library with YoWASP Yosys, the equivalence proof, then
// placement, routing and the checks in the WebAssembly core. Runs in the page's
// worker and, for the checks, under Node; both pass in `runYosys` and the core.

export const LIMITS = {
    sourceBytes: 64 * 1024,
    cells: 400,
    blocks: 80_000,
    // Enforced by whoever runs the worker: synthesis cannot be interrupted from inside.
    compileSeconds: 90,
};

export async function compile({ source, top = 'top', name = 'design', core, runYosys, limits = LIMITS, log = () => {} }) {
    if (typeof source !== 'string') {
        throw new Error('the source must be text');
    }
    const bytes = new TextEncoder().encode(source).length;
    if (bytes > limits.sourceBytes) {
        throw new Error(`the source is ${bytes} bytes; the limit is ${limits.sourceBytes}`);
    }
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(top)) {
        throw new Error(`"${top}" is not a module name`);
    }
    const lib = core.call({ op: 'library' });
    const files = {
        'top.sv': source,
        'cells.lib': lib.liberty,
        'cells.v': lib.models,
        'map.ys': lib.map.replaceAll('{top}', top),
        'equiv.ys': lib.equiv.replaceAll('{top}', top),
    };
    let text = '';
    const sink = (chunk) => {
        if (chunk) {
            text += new TextDecoder().decode(chunk);
        }
    };
    const yosys = async (script, inputs) => {
        text = '';
        let outputs;
        try {
            outputs = await runYosys(['-s', script], inputs, { stdout: sink, stderr: sink, fetchProgress: () => {} });
        } catch (err) {
            const tail = text.split('\n').filter((l) => l.trim()).slice(-8).join('\n');
            throw new Error(`${script} failed:\n${tail || err.message}`);
        }
        log(text);
        return outputs;
    };
    log(`synthesizing ${name} (${bytes} bytes of source)\n`);
    const mapped = await yosys('map.ys', files);
    if (!mapped['mapped.json'] || !mapped['mapped.v']) {
        throw new Error('synthesis wrote no netlist');
    }
    log('proving the mapped netlist equals the source\n');
    await yosys('equiv.ys', { ...files, 'mapped.v': mapped['mapped.v'] });
    log('placing, routing and checking\n');
    const reply = core.call({ op: 'build', mapped: mapped['mapped.json'], top, name });
    const design = JSON.parse(reply.design);
    const cells = (reply.report.match(/cells: (\d+)/) || [0, 0])[1];
    if (Number(cells) > limits.cells) {
        throw new Error(`the design has ${cells} cells; the limit is ${limits.cells}`);
    }
    if (Object.keys(design.blocks).length > limits.blocks) {
        throw new Error(`the design has ${Object.keys(design.blocks).length} blocks; the limit is ${limits.blocks}`);
    }
    log(reply.report);
    return { design, designText: reply.design, structure: reply.structure, report: reply.report, mapped: mapped['mapped.json'] };
}
