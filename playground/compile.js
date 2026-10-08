// From SystemVerilog source to a built design, the way `redfab build` does it:
// synthesis to the tile library with YoWASP Yosys, the equivalence proof, then
// placement, routing and the checks in the WebAssembly core. Runs in the page's
// worker and, for the checks, under Node; both pass in `runYosys` and the core.

export const LIMITS = {
    sourceBytes: 64 * 1024,
    // The hard limit: a larger netlist is refused right after synthesis, before
    // the core is asked to place it.
    cells: 400,
    // Guidance, measured on 2026-10-07 (repair pass): about 100 cells build in
    // under a minute natively (an 8-bit adder, 102 cells, in 10 s; the
    // WebAssembly core is about 5 percent slower), 150 cells take 2 to 3
    // minutes, which is past compileSeconds. The page shows a built design's
    // cell count against this, and the log warns before placing a larger one.
    // Update it when the router improves; tools/playground_test.mjs compiles
    // an 8-bit adder through the WebAssembly pipeline to hold it to its word.
    measuredCells: 100,
    blocks: 80_000,
    // Enforced by whoever runs the worker: synthesis cannot be interrupted from
    // inside. Long enough for the designs under measuredCells.
    compileSeconds: 90,
};

/// The names of the modules declared in a source, in order, as a regex sees
/// them (comments are skipped; strings and macros are not understood).
export function moduleNames(source) {
    const code = source.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/\/\/[^\n]*/g, ' ');
    const names = [];
    for (const m of code.matchAll(/(?:^|[\s;])(?:macro)?module\s+(?:automatic\s+|static\s+)?([A-Za-z_][A-Za-z0-9_$]*)/g)) {
        if (!names.includes(m[1])) names.push(m[1]);
    }
    return names;
}

/// The top module of a source when the user names none: `top` when a module
/// has that name, else the only module there is. Throws when it cannot tell.
export function detectTop(source) {
    const names = moduleNames(source);
    if (names.includes('top')) return 'top';
    if (names.length === 1) return names[0];
    if (names.length === 0) throw new Error('the source declares no module');
    throw new Error(`the source declares ${names.length} modules (${names.join(', ')}); name the top one "top" or set the top module`);
}

/// The design name a top module gives: what the live server accepts as a name.
export function nameFor(top) {
    const name = top.replace(/[^A-Za-z0-9_-]/g, '').slice(0, 32);
    return name || 'design';
}

const ANSI = /\x1b\[[0-9;]*[A-Za-z]/g;

/// Known refusals, as Yosys or the core words them, in plain words.
const KNOWN = [
    [/is not a valid top-level module|no top-level modules found/i, (top) => `the source has no module named "${top}": name your top module "${top}" or set the top module`],
    [/async set or reset are not supported|\$_DFFSR|\$adff|\$aldff/i, () => 'asynchronous reset is not supported; use a synchronous one (if (rst) inside always_ff @(posedge clk))'],
    [/\$dlatch|\$_DLATCH/i, () => 'latches are not supported: give every always_comb output a value on every path'],
    [/\$tribuf|\$_TBUF|tristate/i, () => 'inout and tristate ports are not supported'],
    [/\$print|\$display/i, () => '$display and other system tasks are not supported; remove them'],
    [/\$mem\b|\$memrd|\$memwr/i, () => 'memories and arrays are not supported yet'],
    [/clocked by \d+ different inputs/i, () => 'only one clock is supported: every flip-flop must be clocked by the same input'],
];

/// A refusal in plain words when the raw line is a known one, else the line itself.
export function explain(line, top = 'top') {
    for (const [pattern, words] of KNOWN) {
        if (pattern.test(line)) return words(top);
    }
    return line;
}

/// Turns a Yosys log into the message the user sees: the first error line,
/// explained in plain words where it names a Yosys internal, and the log's
/// tail without terminal colour codes.
export function explainYosys(script, text, top) {
    const lines = text.replace(ANSI, '').split('\n').map((l) => l.trim()).filter(Boolean);
    const first = lines.find((l) => /\berror:|^ERROR:/i.test(l));
    const tail = lines.slice(-8).join('\n');
    return { message: explain(first || tail || `${script} failed`, top), tail };
}

/// A core operation that an older core may not have ("unknown op" is not a
/// refusal; anything else the core throws is one). `lint` {pre, mapped, top}
/// refuses latches, asynchronous resets, tristates, $display, assertions,
/// memories left over and clock misuse before the proof; `check_inits`
/// {gold, mapped, top} compares the flip-flop initial values after it.
function optional(core, op, request) {
    try {
        return core.call({ op, ...request });
    } catch (err) {
        if (/unknown op/.test(err.message)) return null;
        throw err;
    }
}

export async function compile({ source, top = 'top', name, core, runYosys, limits = LIMITS, log = () => {}, phase = () => {} }) {
    if (typeof source !== 'string') {
        throw new Error('the source must be text');
    }
    const bytes = new TextEncoder().encode(source).length;
    if (bytes > limits.sourceBytes) {
        throw new Error(`the source is ${bytes} bytes; the limit is ${limits.sourceBytes}`);
    }
    if (top === '' || top === undefined || top === null) top = detectTop(source);
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(top)) {
        throw new Error(`"${top}" is not a module name`);
    }
    if (name === undefined || name === null || name === '') name = nameFor(top);
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
            // The lint speaks first, as in `redfab build`: the map script writes
            // pre.json before the pass that refuses a latch or an asynchronous
            // reset, and the core names the construct and its source line.
            const pre = err.files?.['pre.json'];
            if (typeof pre === 'string') optional(core, 'lint', { pre, top });
            const { message, tail } = explainYosys(script, text, top);
            throw new Error(`${message}\n${tail || err.message}`);
        }
        log(text.replace(ANSI, ''));
        return outputs;
    };
    phase('synthesizing');
    log(`synthesizing ${name} (${bytes} bytes of source, top module ${top})\n`);
    const mapped = await yosys('map.ys', files);
    if (!mapped['mapped.json'] || !mapped['mapped.v']) {
        throw new Error('synthesis wrote no netlist');
    }
    // The lint refuses what cannot be built, in plain words, before the proof
    // is paid for: `pre` for the constructs, `mapped` for the input count and
    // the clock shape. A refusal is thrown by call().
    optional(core, 'lint', { pre: mapped['pre.json'], mapped: mapped['mapped.json'], top });
    // The cell count is known now, so a design over the limit is refused
    // before the proof and the build, not after the time limit kills them.
    const cells = countCells(mapped['mapped.json'], top);
    if (cells > limits.cells) {
        throw new Error(`the design has ${cells} cells; the limit is ${limits.cells}`);
    }
    if (cells > limits.measuredCells) {
        log(`${cells} cells: more than the ${limits.measuredCells} the router had managed inside ${limits.compileSeconds} s when last measured; this may not finish\n`);
    }
    phase('proving');
    log('proving the mapped netlist equals the source\n');
    const proven = await yosys('equiv.ys', { ...files, 'mapped.v': mapped['mapped.v'] });
    // The proof ignores initial values; the core compares them as `redfab build` does.
    if (typeof proven['gold.json'] === 'string') {
        optional(core, 'check_inits', { gold: proven['gold.json'], mapped: mapped['mapped.json'], top });
    }
    phase('placing and routing');
    log('placing, routing and checking\n');
    let reply;
    try {
        reply = core.call({ op: 'build', mapped: mapped['mapped.json'], top, name });
    } catch (err) {
        throw new Error(explain(err.message, top));
    }
    const design = JSON.parse(reply.design);
    if (Object.keys(design.blocks).length > limits.blocks) {
        throw new Error(`the design has ${Object.keys(design.blocks).length} blocks; the limit is ${limits.blocks}`);
    }
    // The report as `redfab build` writes it: the core's, then the proof's line.
    const report = `${reply.report}  mapped netlist proven equal to the source\n`;
    if (reply.log) log(reply.log);
    log(report);
    return { design, designText: reply.design, structure: reply.structure, report, mapped: mapped['mapped.json'], top, name, cells };
}

/// The number of cells in the mapped netlist's top module: what the report
/// counts, available a second in, before the proof and the build.
export function countCells(mappedText, top) {
    const module = JSON.parse(mappedText).modules?.[top];
    return module ? Object.keys(module.cells ?? {}).length : 0;
}
