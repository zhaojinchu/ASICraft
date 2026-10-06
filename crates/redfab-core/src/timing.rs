//! Timing analysis on the circuit read back from the blocks: for every output,
//! the earliest and latest game tick, after an input change, at which its lamp
//! can change.
//!
//! Rising and falling edges are followed separately, because the parts are not
//! symmetric: a torch answers a rising input with a falling output two ticks
//! later and the other way round, a repeater passes either edge after twice its
//! setting, a lamp lights at once but goes dark four ticks late, and dust and
//! inputs take no time at all. The circuit must be combinational; a loop is an
//! error until flip-flops arrive.
//!
//! Two answers are given. The *bound* is a longest/shortest path over the part
//! graph: quick, safe, and sometimes pessimistic, because a path can exist that
//! no input change can drive (the ripple-carry adder has one). The *exact*
//! figures come from a timed model of the same parts, with the simulator's rules
//! for torches, repeaters and lamps on the net graph instead of on blocks, run
//! over every ordered pair of input combinations when there are few enough
//! inputs. `measure` checks the model against the block simulator change by
//! change, and the bound against both.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::block::Block;
use crate::build::Design;
use crate::extract::{Circuit, extract};
use crate::geom::Pos;
use crate::sim::Sim;
use crate::tiletest::Rng;
use crate::world::World;

pub const TORCH_TICKS: u32 = 2;
pub const LAMP_OFF_TICKS: u32 = 4;
/// Fewest ticks between two input changes while measuring: a torch that turns
/// off eight times within sixty ticks burns out, which is not what is measured.
pub const MEASURE_GAP: u32 = 10;

/// Earliest and latest arrival of each edge at a net or a part, in game ticks
/// after the input change. `None` means the signal never changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Edges {
    rise: Option<(u32, u32)>,
    fall: Option<(u32, u32)>,
}

impl Edges {
    const NEVER: Edges = Edges { rise: None, fall: None };

    fn shifted(self, by: u32) -> Edges {
        let s = |e: Option<(u32, u32)>| e.map(|(lo, hi)| (lo + by, hi + by));
        Edges { rise: s(self.rise), fall: s(self.fall) }
    }

    fn inverted(self) -> Edges {
        Edges { rise: self.fall, fall: self.rise }
    }

    fn join(self, other: Edges) -> Edges {
        let j = |a: Option<(u32, u32)>, b: Option<(u32, u32)>| match (a, b) {
            (Some((alo, ahi)), Some((blo, bhi))) => Some((alo.min(blo), ahi.max(bhi))),
            (a, None) => a,
            (None, b) => b,
        };
        Edges { rise: j(self.rise, other.rise), fall: j(self.fall, other.fall) }
    }
}

/// The report for one output: the first and last tick its lamp can change
/// by path (`min`, `max`), the parts along the path that sets the last, and
/// the earliest and latest change seen over every ordered pair of input
/// combinations in the timed model (`exact`, absent with too many inputs or
/// an output that never changes).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct OutputTiming {
    pub min: u32,
    pub max: u32,
    #[serde(default)]
    pub path: Vec<String>,
    #[serde(default)]
    pub exact: Option<(u32, u32)>,
}

/// The timing report of a design.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Timing {
    /// By output name. An output that never changes (a constant) has 0 and 0.
    pub outputs: BTreeMap<String, OutputTiming>,
    /// Whether `exact` was computed over every pair of input combinations.
    #[serde(default)]
    pub exhaustive: bool,
}

impl Timing {
    /// Game ticks after an input change by which every output is final: the
    /// bound by path, which is never less than the exact figure.
    pub fn settle_ticks(&self) -> u32 {
        self.outputs.values().map(|t| t.max).max().unwrap_or(0)
    }
}

/// Inputs up to which the exact figures are computed over every ordered pair
/// of input combinations (4^n pairs).
pub const EXACT_MAX_INPUTS: usize = 10;

/// Analyzes the circuit of `world`, whose inputs are the `probes` and whose
/// outputs are the `lamps` (named, in order): the bound by path, and the exact
/// figures from the timed model when there are at most `EXACT_MAX_INPUTS` inputs.
pub fn analyze(world: &World, probes: &[Pos], lamps: &[(String, Pos)]) -> Result<Timing, String> {
    let circuit = extract(world, probes);
    analyze_circuit(world, &circuit, probes, lamps)
}

pub fn analyze_circuit(
    world: &World,
    circuit: &Circuit,
    probes: &[Pos],
    lamps: &[(String, Pos)],
) -> Result<Timing, String> {
    let mut timing = bound_by_path(world, circuit, probes, lamps)?;
    if probes.len() <= EXACT_MAX_INPUTS {
        let model = Model::new(world, circuit, probes, lamps)?;
        let seen = model.all_pairs()?;
        for ((name, _), seen) in lamps.iter().zip(seen) {
            timing.outputs.get_mut(name).expect("every output has a bound").exact = seen;
        }
        timing.exhaustive = true;
    }
    Ok(timing)
}

/// The earliest and latest arrival at each output along any path.
pub fn bound_by_path(
    world: &World,
    circuit: &Circuit,
    probes: &[Pos],
    lamps: &[(String, Pos)],
) -> Result<Timing, String> {
    // Parts as graph nodes: what they read, what they drive, and their delay rule.
    let mut parts: Vec<(Part, &[usize], &[usize])> = Vec::new();
    for torch in &circuit.torches {
        parts.push((Part::Torch, &torch.reads, &torch.drives));
    }
    for repeater in &circuit.repeaters {
        let delay = match world.get(repeater.pos) {
            Block::Repeater { delay, .. } => 2 * u32::from(delay),
            other => return Err(format!("expected a repeater at {}, found {other}", repeater.pos)),
        };
        parts.push((Part::Repeater(delay), &repeater.reads, &repeater.drives));
    }

    // Arrival at each net. Inputs change at tick 0; everything else waits for
    // every part that drives it, in topological order.
    let mut partial: Vec<Edges> = vec![Edges::NEVER; circuit.nets.len()];
    let mut drivers_left: Vec<usize> = vec![0; circuit.nets.len()];
    for (_, _, drives) in &parts {
        for &net in *drives {
            drivers_left[net] += 1;
        }
    }
    for source in &circuit.sources {
        if probes.contains(&source.pos) {
            for &net in &source.drives {
                partial[net] = partial[net].join(Edges { rise: Some((0, 0)), fall: Some((0, 0)) });
            }
        }
    }
    // A net is settled once every part driving it has been evaluated; a tie or a
    // wire nothing drives is settled from the start and never changes.
    let mut net_edges: Vec<Option<Edges>> =
        drivers_left.iter().zip(&partial).map(|(&left, &e)| (left == 0).then_some(e)).collect();
    let mut done = vec![false; parts.len()];
    loop {
        let mut progress = false;
        for (p, (part, reads, drives)) in parts.iter().enumerate() {
            if done[p] || reads.iter().any(|&n| net_edges[n].is_none()) {
                continue;
            }
            let input = reads.iter().fold(Edges::NEVER, |acc, &n| acc.join(net_edges[n].expect("settled")));
            let output = match part {
                Part::Torch => input.inverted().shifted(TORCH_TICKS),
                Part::Repeater(delay) => input.shifted(*delay),
            };
            for &net in *drives {
                partial[net] = partial[net].join(output);
                drivers_left[net] -= 1;
                if drivers_left[net] == 0 {
                    net_edges[net] = Some(partial[net]);
                }
            }
            done[p] = true;
            progress = true;
        }
        if !progress {
            break;
        }
    }
    if let Some(p) = done.iter().position(|d| !d) {
        let pos = match p < circuit.torches.len() {
            true => circuit.torches[p].pos,
            false => circuit.repeaters[p - circuit.torches.len()].pos,
        };
        return Err(format!("the circuit has a loop through the part at {pos}; timing needs a combinational circuit"));
    }

    let mut outputs = BTreeMap::new();
    for (name, pos) in lamps {
        let lamp =
            circuit.lamps.iter().find(|l| l.pos == *pos).ok_or_else(|| format!("output {name}: no lamp at {pos}"))?;
        let input = lamp.reads.iter().fold(Edges::NEVER, |acc, &n| acc.join(net_edges[n].unwrap_or(Edges::NEVER)));
        // A lamp lights at once and goes dark four ticks after its power stops.
        let lit =
            Edges { rise: input.rise, fall: input.fall.map(|(lo, hi)| (lo + LAMP_OFF_TICKS, hi + LAMP_OFF_TICKS)) };
        let (min, max, falling) = match (lit.rise, lit.fall) {
            (Some((rlo, rhi)), Some((flo, fhi))) => (rlo.min(flo), rhi.max(fhi), fhi >= rhi),
            (Some((lo, hi)), None) => (lo, hi, false),
            (None, Some((lo, hi))) => (lo, hi, true),
            (None, None) => (0, 0, false),
        };
        let mut path = Vec::new();
        if max > 0 {
            let net = lamp.reads.iter().copied().find(|&n| {
                let e = net_edges[n].unwrap_or(Edges::NEVER);
                let at = if falling { e.fall.map(|(_, hi)| hi + LAMP_OFF_TICKS) } else { e.rise.map(|(_, hi)| hi) };
                at == Some(max)
            });
            if let Some(net) = net {
                path = critical_path(circuit, &parts, &net_edges, net, falling);
                path.push(format!("lamp at {pos}{}", if falling { " going dark" } else { " lighting" }));
            }
        }
        outputs.insert(name.clone(), OutputTiming { min, max, path, exact: None });
    }
    Ok(Timing { outputs, exhaustive: false })
}

enum Part {
    Torch,
    Repeater(u32),
}

/// The parts on the path that sets the latest `falling` (or rising) edge of
/// `net`, from the input onwards, found by walking the arrivals backwards.
fn critical_path(
    circuit: &Circuit,
    parts: &[(Part, &[usize], &[usize])],
    net_edges: &[Option<Edges>],
    net: usize,
    falling: bool,
) -> Vec<String> {
    let edge_max = |n: usize, falling: bool| -> Option<u32> {
        let e = net_edges[n].unwrap_or(Edges::NEVER);
        (if falling { e.fall } else { e.rise }).map(|(_, hi)| hi)
    };
    let mut steps = Vec::new();
    let (mut net, mut falling) = (net, falling);
    let mut guard = 0;
    while let Some(at) = edge_max(net, falling) {
        guard += 1;
        if at == 0 || guard > parts.len() + 2 {
            break;
        }
        // The part driving this net whose output edge lands exactly at `at`.
        let found = parts.iter().enumerate().find_map(|(p, (part, reads, drives))| {
            if !drives.contains(&net) {
                return None;
            }
            let (delay, input_falling) = match part {
                Part::Torch => (TORCH_TICKS, !falling),
                Part::Repeater(d) => (*d, falling),
            };
            let from = reads.iter().copied().find(|&r| edge_max(r, input_falling) == Some(at.saturating_sub(delay)))?;
            Some((p, from, input_falling))
        });
        let Some((p, from, input_falling)) = found else { break };
        let (what, pos) = if p < circuit.torches.len() {
            ("torch", circuit.torches[p].pos)
        } else {
            ("repeater", circuit.repeaters[p - circuit.torches.len()].pos)
        };
        steps.push(format!("{what} at {pos}, {} at {at}", if falling { "falls" } else { "rises" }));
        net = from;
        falling = input_falling;
    }
    if let Some(source) = circuit.sources.iter().find(|s| s.drives.contains(&net)) {
        steps.push(format!("input at {}", source.pos));
    }
    steps.reverse();
    steps
}

/// The parts of a circuit as a timed model: the block simulator's rules for
/// torches, repeaters and lamps, applied on the net graph. Exact for a settled
/// combinational circuit that does not burn a torch out (the walks here keep
/// changes far enough apart for that).
pub struct Model {
    parts: Vec<ModelPart>,
    /// Parts that read each net.
    readers: Vec<Vec<usize>>,
    /// Parts that drive each net (one in a correct layout).
    drivers: Vec<Vec<usize>>,
    /// Nets driven by each design input.
    input_nets: Vec<Vec<usize>>,
    /// The lamp part of each design output.
    lamps: Vec<usize>,
    /// Part states in the world the model was built from (every input off).
    rest: Vec<bool>,
}

struct ModelPart {
    kind: PartKind,
    reads: Vec<usize>,
    drives: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PartKind {
    Torch,
    Repeater(u32),
    Lamp,
}

/// Priorities of scheduled ticks, lowest first (the game's `TickPriority`).
const PRIORITY_REPEATER_OFF: i8 = -2;
const PRIORITY_REPEATER: i8 = -1;
const PRIORITY_NORMAL: i8 = 0;

#[derive(Clone)]
struct ModelState {
    /// Input levels, part states (torch lit, repeater powered, lamp lit) and net levels.
    inputs: Vec<bool>,
    on: Vec<bool>,
    level: Vec<bool>,
    /// Scheduled ticks: (trigger, priority, sequence, part).
    queue: Vec<(u32, i8, u32, usize)>,
    seq: u32,
    time: u32,
}

impl Model {
    pub fn new(world: &World, circuit: &Circuit, probes: &[Pos], lamps: &[(String, Pos)]) -> Result<Model, String> {
        let mut parts = Vec::new();
        let mut rest = Vec::new();
        for torch in &circuit.torches {
            let lit = match world.get(torch.pos) {
                Block::Torch { lit } | Block::WallTorch { lit, .. } => lit,
                other => return Err(format!("expected a torch at {}, found {other}", torch.pos)),
            };
            parts.push(ModelPart { kind: PartKind::Torch, reads: torch.reads.clone(), drives: torch.drives.clone() });
            rest.push(lit);
        }
        for repeater in &circuit.repeaters {
            let (delay, powered) = match world.get(repeater.pos) {
                Block::Repeater { delay, powered, .. } => (2 * u32::from(delay), powered),
                other => return Err(format!("expected a repeater at {}, found {other}", repeater.pos)),
            };
            parts.push(ModelPart {
                kind: PartKind::Repeater(delay),
                reads: repeater.reads.clone(),
                drives: repeater.drives.clone(),
            });
            rest.push(powered);
        }
        let mut lamp_parts = Vec::new();
        for (name, pos) in lamps {
            let lamp = circuit
                .lamps
                .iter()
                .find(|l| l.pos == *pos)
                .ok_or_else(|| format!("output {name}: no lamp at {pos}"))?;
            let lit = match world.get(*pos) {
                Block::Lamp { lit } => lit,
                other => return Err(format!("expected a lamp at {pos}, found {other}")),
            };
            lamp_parts.push(parts.len());
            parts.push(ModelPart { kind: PartKind::Lamp, reads: lamp.reads.clone(), drives: Vec::new() });
            rest.push(lit);
        }
        let mut readers = vec![Vec::new(); circuit.nets.len()];
        let mut drivers = vec![Vec::new(); circuit.nets.len()];
        for (p, part) in parts.iter().enumerate() {
            for &net in &part.reads {
                readers[net].push(p);
            }
            for &net in &part.drives {
                drivers[net].push(p);
            }
        }
        let input_nets = probes
            .iter()
            .map(|probe| circuit.sources.iter().find(|s| s.pos == *probe).map(|s| s.drives.clone()).unwrap_or_default())
            .collect();
        Ok(Model { parts, readers, drivers, input_nets, lamps: lamp_parts, rest })
    }

    /// The model at rest with every input off, as the world was built.
    fn rest_state(&self) -> ModelState {
        let mut state = ModelState {
            inputs: vec![false; self.input_nets.len()],
            on: self.rest.clone(),
            level: vec![false; self.readers.len()],
            queue: Vec::new(),
            seq: 0,
            time: 0,
        };
        for net in 0..state.level.len() {
            state.level[net] = self.net_level(&state, net);
        }
        state
    }

    fn net_level(&self, state: &ModelState, net: usize) -> bool {
        self.drivers[net].iter().any(|&p| state.on[p])
            || self.input_nets.iter().enumerate().any(|(i, nets)| state.inputs[i] && nets.contains(&net))
    }

    fn has_signal(&self, state: &ModelState, p: usize) -> bool {
        self.parts[p].reads.iter().any(|&n| state.level[n])
    }

    fn schedule(&self, state: &mut ModelState, p: usize, delay: u32, priority: i8) {
        // The game keeps at most one pending tick per block.
        if state.queue.iter().any(|&(_, _, _, part)| part == p) {
            return;
        }
        state.queue.push((state.time + delay, priority, state.seq, p));
        state.seq += 1;
    }

    /// A net's drivers changed: recompute it and tell its readers.
    fn net_changed(&self, state: &mut ModelState, net: usize, running: &[usize]) {
        let level = self.net_level(state, net);
        if level == state.level[net] {
            return;
        }
        state.level[net] = level;
        for &p in &self.readers[net] {
            self.neighbor_changed(state, p, running);
        }
    }

    fn neighbor_changed(&self, state: &mut ModelState, p: usize, running: &[usize]) {
        let signal = self.has_signal(state, p);
        match self.parts[p].kind {
            PartKind::Torch => {
                if state.on[p] == signal && !running.contains(&p) {
                    self.schedule(state, p, TORCH_TICKS, PRIORITY_NORMAL);
                }
            }
            PartKind::Repeater(delay) => {
                if state.on[p] != signal && !running.contains(&p) {
                    let priority = if state.on[p] { PRIORITY_REPEATER_OFF } else { PRIORITY_REPEATER };
                    self.schedule(state, p, delay, priority);
                }
            }
            PartKind::Lamp => {
                if state.on[p] != signal {
                    if state.on[p] {
                        self.schedule(state, p, LAMP_OFF_TICKS, PRIORITY_NORMAL);
                    } else {
                        state.on[p] = true;
                    }
                }
            }
        }
    }

    fn set_on(&self, state: &mut ModelState, p: usize, on: bool, running: &[usize]) {
        state.on[p] = on;
        for &net in &self.parts[p].drives {
            self.net_changed(state, net, running);
        }
    }

    fn fire(&self, state: &mut ModelState, p: usize, running: &[usize]) {
        let signal = self.has_signal(state, p);
        match self.parts[p].kind {
            PartKind::Torch => {
                if state.on[p] && signal {
                    self.set_on(state, p, false, running);
                } else if !state.on[p] && !signal {
                    self.set_on(state, p, true, running);
                }
            }
            PartKind::Repeater(delay) => {
                if state.on[p] && !signal {
                    self.set_on(state, p, false, running);
                } else if !state.on[p] {
                    // Turning on does not re-check the input: a short pulse comes out stretched.
                    self.set_on(state, p, true, running);
                    if !signal {
                        self.schedule(state, p, delay, PRIORITY_REPEATER_OFF);
                    }
                }
            }
            PartKind::Lamp => {
                if state.on[p] && !signal {
                    state.on[p] = false;
                }
            }
        }
    }

    /// Applies an input vector at the current time and runs until nothing is
    /// pending. Returns, per output, the ticks after the change at which its lamp
    /// changed. Time advances straight to each pending tick.
    fn apply(&self, state: &mut ModelState, vector: u64, limit: u32) -> Result<Vec<Vec<u32>>, String> {
        let start = state.time;
        let running: Vec<usize> = Vec::new();
        let mut changed_inputs: Vec<usize> = Vec::new();
        for i in 0..state.inputs.len() {
            let on = vector >> i & 1 == 1;
            if state.inputs[i] != on {
                state.inputs[i] = on;
                changed_inputs.push(i);
            }
        }
        for i in changed_inputs {
            for &net in &self.input_nets[i] {
                self.net_changed(state, net, &running);
            }
        }
        let mut lamp_was: Vec<bool> = self.lamps.iter().map(|&p| state.on[p]).collect();
        let mut changes: Vec<Vec<u32>> = vec![Vec::new(); self.lamps.len()];
        let note = |state: &ModelState, lamp_was: &mut Vec<bool>, changes: &mut Vec<Vec<u32>>| {
            for (o, &p) in self.lamps.iter().enumerate() {
                if state.on[p] != lamp_was[o] {
                    lamp_was[o] = state.on[p];
                    changes[o].push(state.time - start);
                }
            }
        };
        note(state, &mut lamp_was, &mut changes);
        while let Some(&next) = state.queue.iter().map(|(t, _, _, _)| t).min() {
            if next - start > limit {
                return Err(format!("the model is still active {limit} ticks after an input change"));
            }
            state.time = next;
            let mut due: Vec<(u32, i8, u32, usize)> = state.queue.iter().copied().filter(|t| t.0 == next).collect();
            state.queue.retain(|t| t.0 != next);
            due.sort_by_key(|&(_, priority, seq, _)| (priority, seq));
            let running: Vec<usize> = due.iter().map(|t| t.3).collect();
            for (_, _, _, p) in due {
                self.fire(state, p, &running);
                note(state, &mut lamp_was, &mut changes);
            }
        }
        Ok(changes)
    }

    /// The lamp changes, per step of a walk of input vectors starting from rest,
    /// with at least `gap` ticks between steps.
    pub fn walk(&self, walk: &[u64], gap: u32, limit: u32) -> Result<Vec<Vec<Vec<u32>>>, String> {
        let mut state = self.rest_state();
        let mut out = Vec::new();
        for &vector in walk {
            let changes = self.apply(&mut state, vector, limit)?;
            state.time += gap;
            out.push(changes);
        }
        Ok(out)
    }

    /// Per output, the earliest and latest lamp change over every ordered pair
    /// of input combinations, or `None` if the lamp never changes.
    pub fn all_pairs(&self) -> Result<Vec<Option<(u32, u32)>>, String> {
        let n = self.input_nets.len();
        let count = 1u64 << n;
        let limit = 100_000;
        // The settled state of every vector, each reached from rest.
        let mut settled = Vec::with_capacity(count as usize);
        for v in 0..count {
            let mut state = self.rest_state();
            self.apply(&mut state, v, limit)?;
            state.time = 0;
            settled.push(state);
        }
        let mut seen: Vec<Option<(u32, u32)>> = vec![None; self.lamps.len()];
        for from in 0..count {
            for to in 0..count {
                if from == to {
                    continue;
                }
                let mut state = settled[from as usize].clone();
                let changes = self.apply(&mut state, to, limit)?;
                for (o, ticks) in changes.iter().enumerate() {
                    for &t in ticks {
                        seen[o] = Some(match seen[o] {
                            None => (t, t),
                            Some((lo, hi)) => (lo.min(t), hi.max(t)),
                        });
                    }
                }
            }
        }
        Ok(seen)
    }
}

/// The walk `measure` takes: every input combination (a sample when there are
/// more than `max_input_bits` inputs), from each one every single-bit change and
/// back, then `multi` random changes of several bits.
pub fn measurement_walk(n: usize, max_input_bits: usize, multi: usize, seed: u64) -> Vec<u64> {
    let mut rng = Rng::new(seed);
    let mask = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
    let vectors: Vec<u64> = if n <= max_input_bits {
        (0..1u64 << n).collect()
    } else {
        (0..1usize << max_input_bits).map(|_| rng.draw() & mask).collect()
    };
    let mut walk: Vec<u64> = Vec::new();
    for &v in &vectors {
        walk.push(v);
        for bit in 0..n {
            walk.push(v ^ (1 << bit));
            walk.push(v);
        }
    }
    for _ in 0..multi {
        walk.push(rng.draw() & mask);
    }
    walk
}

/// Runs the design in the block simulator along a walk of input vectors and
/// returns, per step, per output, the ticks after the step's change at which the
/// lamp changed. Steps are at least `MEASURE_GAP` ticks apart.
pub fn simulate_walk(design: &Design, walk: &[u64]) -> Result<Vec<Vec<Vec<u32>>>, String> {
    let probes = design.probes()?;
    let lamps = design.lamps()?;
    let on: Vec<Block> = design.inputs.iter().map(|p| p.on.parse()).collect::<Result<_, _>>()?;
    let off: Vec<Block> = design.inputs.iter().map(|p| p.off.parse()).collect::<Result<_, _>>()?;
    let n = design.inputs.len();
    let mut sim = Sim::new(design.world()?);
    let limit = 4 * design.settle_ticks.max(LAMP_OFF_TICKS) + 20;
    let mut current = 0u64; // the baked world has every input off
    let mut out = Vec::new();
    for &next in walk {
        let mut stimuli: Vec<(Pos, Block)> = (0..n)
            .filter(|&i| (current ^ next) >> i & 1 == 1)
            .map(|i| (probes[i], if next >> i & 1 == 1 { on[i] } else { off[i] }))
            .collect();
        stimuli.sort_by_key(|(pos, _)| crate::trace::event_order(*pos));
        for (pos, block) in stimuli {
            sim.set_block(pos, block, false)?;
        }
        current = next;
        let mut lit: Vec<Block> = lamps.iter().map(|&l| sim.world().get(l)).collect();
        let mut changes: Vec<Vec<u32>> = vec![Vec::new(); lamps.len()];
        let mut tick = 0;
        while !(sim.pending().is_empty() && tick >= MEASURE_GAP) {
            if tick >= limit {
                return Err(format!("the circuit is still active {limit} ticks after an input change"));
            }
            sim.step();
            tick += 1;
            for (o, &lamp) in lamps.iter().enumerate() {
                let now = sim.world().get(lamp);
                if now != lit[o] {
                    lit[o] = now;
                    changes[o].push(tick);
                }
            }
        }
        out.push(changes);
    }
    Ok(out)
}

/// What a walk showed for each output: the earliest and latest lamp change, or
/// `None` if it never changed.
pub fn extremes(walk: &[Vec<Vec<u32>>], outputs: usize) -> Vec<Option<(u32, u32)>> {
    let mut seen: Vec<Option<(u32, u32)>> = vec![None; outputs];
    for step in walk {
        for (o, ticks) in step.iter().enumerate() {
            for &t in ticks {
                seen[o] = Some(match seen[o] {
                    None => (t, t),
                    Some((lo, hi)) => (lo.min(t), hi.max(t)),
                });
            }
        }
    }
    seen
}

/// The report of a measurement.
pub struct Measurement {
    pub changes: usize,
    /// Per output, by name: what the block simulator showed over the walk.
    pub seen: BTreeMap<String, Option<(u32, u32)>>,
    pub problems: Vec<String>,
}

/// Checks a design's timing report: the timed model must agree with the block
/// simulator on every lamp change of the walk; the exact figures must be what
/// the walk shows; and the bound by path must contain everything.
pub fn measure(design: &Design, max_input_bits: usize, multi: usize, seed: u64) -> Result<Measurement, String> {
    let probes = design.probes()?;
    let lamps: Vec<(String, Pos)> =
        design.outputs.iter().map(|o| o.name.clone()).zip(design.lamps()?.iter().copied()).collect();
    let world = design.world()?;
    let circuit = extract(&world, &probes);
    let model = Model::new(&world, &circuit, &probes, &lamps)?;
    let walk = measurement_walk(design.inputs.len(), max_input_bits, multi, seed);
    let by_blocks = simulate_walk(design, &walk)?;
    let by_model = model.walk(&walk, MEASURE_GAP, 4 * design.settle_ticks.max(LAMP_OFF_TICKS) + 20)?;
    let mut problems = Vec::new();
    for (step, (a, b)) in by_blocks.iter().zip(&by_model).enumerate() {
        if a != b {
            problems.push(format!(
                "step {step} (inputs {:b}): the blocks changed lamps at {:?} ticks, the timed model at {:?}",
                walk[step], a, b
            ));
            if problems.len() >= 5 {
                break;
            }
        }
    }
    let seen = extremes(&by_blocks, lamps.len());
    for ((name, _), seen) in lamps.iter().zip(&seen) {
        let reported = &design.timing.outputs[name];
        match (seen, reported.exact) {
            (Some((lo, hi)), Some((elo, ehi))) => {
                if (*lo, *hi) != (elo, ehi) {
                    problems.push(format!(
                        "{name}: the simulator shows changes {lo} to {hi} ticks after an input change; the exact report says {elo} to {ehi}"
                    ));
                }
            }
            (Some((lo, hi)), None) => {
                if design.timing.exhaustive {
                    problems
                        .push(format!("{name}: changed ({lo} to {hi} ticks) but the exact report says it never does"));
                }
            }
            (None, Some((elo, ehi))) => {
                problems.push(format!("{name}: never changed over the walk, but the exact report says {elo} to {ehi}"))
            }
            (None, None) => {}
        }
        if let Some((lo, hi)) = seen
            && (*hi > reported.max || *lo < reported.min)
        {
            problems.push(format!(
                "{name}: changes {lo} to {hi} ticks after an input change fall outside the bound by path, {} to {}",
                reported.min, reported.max
            ));
        }
    }
    let seen = lamps.iter().map(|(n, _)| n.clone()).zip(seen).collect();
    Ok(Measurement { changes: walk.len(), seen, problems })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::build;

    const NOT: &str = r#"{"modules":{"top":{
        "ports":{"a":{"direction":"input","bits":[2]},"y":{"direction":"output","bits":[3]}},
        "cells":{"g":{"type":"NOT","connections":{"A":[2],"Y":[3]}}}}}}"#;
    const NOR2: &str = r#"{"modules":{"top":{
        "ports":{"a":{"direction":"input","bits":[2]},"b":{"direction":"input","bits":[3]},"y":{"direction":"output","bits":[4]}},
        "cells":{"g":{"type":"NOR2","connections":{"A":[2],"B":[3],"Y":[4]}}}}}}"#;

    #[test]
    fn an_inverter_is_a_torch_and_a_lamp() {
        let design = build(NOT, "top", "not1").unwrap().design;
        let timing = design.timing;
        // Lever to torch: 2 ticks. The lamp lights at once when the torch does and
        // goes dark 4 ticks after it.
        assert_eq!((timing.outputs["y"].min, timing.outputs["y"].max), (2, 6));
        assert_eq!(timing.outputs["y"].path.len(), 3, "{:?}", timing.outputs["y"].path); // input, torch, lamp
        assert_eq!(design.settle_ticks, 6);
    }

    #[test]
    fn the_report_matches_the_simulator_on_small_designs() {
        for (json, name) in [(NOT, "not1"), (NOR2, "nor2")] {
            let design = build(json, "top", name).unwrap().design;
            let measurement = measure(&design, 10, 8, 1).unwrap();
            assert_eq!(measurement.problems, Vec::<String>::new());
            assert!(design.timing.exhaustive);
            assert_eq!(design.timing.outputs["y"].exact, Some((2, 6)));
        }
    }

    #[test]
    fn edges_join_and_shift() {
        let a = Edges { rise: Some((2, 4)), fall: None };
        let b = Edges { rise: Some((3, 9)), fall: Some((1, 1)) };
        assert_eq!(a.join(b), Edges { rise: Some((2, 9)), fall: Some((1, 1)) });
        assert_eq!(a.shifted(2).inverted(), Edges { rise: None, fall: Some((4, 6)) });
    }
}
