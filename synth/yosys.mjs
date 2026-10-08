// Runs YoWASP Yosys (the WebAssembly build, the same one the browser uses) on a
// directory of files.
//
//   node synth/yosys.mjs WORKDIR SCRIPT.ys
//
// Every regular file in WORKDIR is visible to Yosys under its own name; files
// Yosys creates or changes are written back. The script is a file of Yosys
// commands, one per line. Exits non-zero if Yosys fails; its log goes to
// WORKDIR/<script name>.log.

import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { runYosys } from '@yowasp/yosys';

const [workdir, scriptName] = process.argv.slice(2);
if (!workdir || !scriptName) {
    console.error('usage: node synth/yosys.mjs WORKDIR SCRIPT.ys');
    process.exit(2);
}

const inputs = {};
for (const name of readdirSync(workdir)) {
    const path = join(workdir, name);
    if (statSync(path).isFile() && !name.endsWith('.log')) {
        inputs[name] = readFileSync(path, 'utf8');
    }
}

let log = '';
const sink = (bytes) => {
    if (bytes) log += new TextDecoder().decode(bytes);
};
let outputs = null;
let failure = null;
try {
    outputs = await runYosys(['-s', basename(scriptName)], inputs, { stdout: sink, stderr: sink, fetchProgress: () => {} });
} catch (err) {
    failure = err;
    outputs = err.files ?? null;
}
writeFileSync(join(workdir, basename(scriptName) + '.log'), log);
for (const [name, data] of Object.entries(outputs ?? {})) {
    if (typeof data === 'object' && !(data instanceof Uint8Array)) continue; // a directory
    const text = typeof data === 'string' ? data : new TextDecoder().decode(data);
    if (inputs[name] !== text) writeFileSync(join(workdir, name), text);
}
if (failure) {
    // Plain text: the frontend colours its diagnostics for a terminal.
    const lines = log.replace(/\x1b\[[0-9;]*[A-Za-z]/g, '').trimEnd().split('\n');
    const error = lines.find((l) => /^ERROR:|: error: |^error: /.test(l.trim()));
    if (error) console.error(error.trim());
    console.error(lines.slice(-8).join('\n'));
    console.error(`yosys failed: ${failure.message ?? failure}`);
    process.exit(1);
}
