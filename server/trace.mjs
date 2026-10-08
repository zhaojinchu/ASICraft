// Positions, block states and traces in the trace format, shared by the
// backend (plots.mjs) and the live checks (tools/live_common.mjs). The game
// writes a block state as `name[k=v,...]` with the properties in name order;
// `canonicalState` makes that order certain, so what the stream says is
// compared with the design file by meaning, not by luck.

export const FIXTURE_FORMAT = 'redfab-fixture/1';
export const TRACE_FORMAT = 'redfab-trace/1';
export const AIR = 'minecraft:air';

export const parsePos = (key) => key.split(',').map(Number);
export const joinPos = (p) => `${p[0]},${p[1]},${p[2]}`;

/// A block state with its properties sorted by name and without spaces.
export function canonicalState(text) {
    const open = text.indexOf('[');
    if (open < 0 || !text.endsWith(']')) return text;
    const name = text.slice(0, open);
    const body = text.slice(open + 1, -1).replace(/\s+/g, '');
    if (!body) return name;
    const pairs = body.split(',');
    const sorted = pairs.slice().sort((a, b) => (a.split('=')[0] < b.split('=')[0] ? -1 : 1));
    return `${name}[${sorted.join(',')}]`;
}

/// The block states inside the box after `tick` ticks of a trace, air left out.
export function stateAt(trace, tick) {
    const state = new Map(Object.entries(trace.initial));
    for (const entry of trace.ticks) {
        if (entry.tick > tick) break;
        for (const [pos, block] of Object.entries(entry.changes)) {
            if (block === AIR) state.delete(pos);
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
            const va = canonicalState(sa.get(pos) ?? AIR);
            const vb = canonicalState(sb.get(pos) ?? AIR);
            if (va !== vb) diffs.push(`${pos}: ${va} vs ${vb}`);
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
        const diff = describe(stateAt(a, t), stateAt(b, t));
        if (diff) return `tick ${t}: ${diff}`;
    }
    return null;
}

/// How many entries of a trace change the given position.
export function changesAt(trace, pos) {
    return trace.ticks.filter((t) => pos in t.changes).length;
}
