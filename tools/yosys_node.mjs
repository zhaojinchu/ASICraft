// Run tools/smoke.ys on one SystemVerilog file with the npm build of YoWASP Yosys
// (the build the browser playground will use).
//
// usage: node yosys_node.mjs <src.sv> <top> <out.json>

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runYosys } from '@yowasp/yosys';

const [src, top, out] = process.argv.slice(2);
const here = dirname(fileURLToPath(import.meta.url));

const script = readFileSync(join(here, 'smoke.ys'), 'utf8')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith('#'))
    .join('; ')
    .replaceAll('{top}', top);

function sortKeys(value) {
    if (Array.isArray(value)) return value.map(sortKeys);
    if (value && typeof value === 'object') {
        return Object.fromEntries(Object.keys(value).sort().map((k) => [k, sortKeys(value[k])]));
    }
    return value;
}

let files;
try {
    files = await runYosys(['-q', '-p', script], { 'in.sv': readFileSync(src, 'utf8') }, {
        fetchProgress: () => {},
    });
} catch (err) {
    console.error(`yosys failed: ${err.message ?? err}`);
    process.exit(1);
}

const raw = files['out.json'];
const netlist = JSON.parse(typeof raw === 'string' ? raw : new TextDecoder().decode(raw));
const module = netlist.modules[top];
if (!module || Object.keys(module.ports).length === 0) {
    console.error(`top module '${top}' missing or has no ports`);
    process.exit(1);
}
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify(sortKeys(netlist), null, 1));
console.log(`${netlist.creator}\nports: ${Object.keys(module.ports).sort()}`);
