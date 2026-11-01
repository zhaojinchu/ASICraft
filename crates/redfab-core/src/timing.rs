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
/// Fewest ticks between two input changes while measuring. A torch that turns
/// off eight times within sixty ticks burns out, which is not what is measured,
/// so the walk also waits until no torch has turned off for sixty ticks before
/// each change, and refuses to go on if one burns out anyway.
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
    /// For a clocked design: ticks after the clock edge, with the inputs
    /// changing when `sequential` says.
    pub outputs: BTreeMap<String, OutputTiming>,
    /// Whether `exact` was computed over every pair of input combinations.
    #[serde(default)]
    pub exhaustive: bool,
    /// For a clocked design: the clock and the flip-flops.
    #[serde(default)]
    pub sequential: Option<Sequential>,
    /// The fewest ticks between input changes at which no torch turns off more
    /// than the budget allows within sixty ticks (combinational designs; a
    /// clocked design's period already respects it).
    #[serde(default)]
    pub input_spacing: u32,
}

/// The timing of a clocked design, in game ticks after the active edge of the
/// clock input (the edge the harness treats as the start of a cycle).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Sequential {
    /// The design input that is the clock, and which of its edges starts a cycle.
    pub clock: String,
    pub active: String,
    /// The shortest clock period the design works at, and the half the clock is high.
    pub period: u32,
    pub high: u32,
    /// Inputs may change from this many ticks after the edge ...
    pub input_change_from: u32,
    /// ... and must be settled by this many (both measured from the edge).
    pub input_change_by: u32,
    /// When the harness changes inputs, and when it reads outputs.
    pub input_change_at: u32,
    pub sample_at: u32,
    pub flip_flops: Vec<FlipFlopTiming>,
    /// Hold violations left after the fixes, if any (cell names).
    #[serde(default)]
    pub hold_violations: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct FlipFlopTiming {
    pub cell: String,
    /// When its clock port falls after the active edge.
    pub clock_arrival: u32,
    /// Earliest and latest change of its D from other flip-flops (absolute) and
    /// from the inputs (ticks after the inputs change); `None` if nothing reaches it.
    pub d_from_flip_flops: Option<(u32, u32)>,
    pub d_from_inputs: Option<(u32, u32)>,
    /// Hold and setup slack in ticks (negative is a violation); none when no
    /// flip-flop feeds this one.
    pub hold_slack: Option<i32>,
    pub setup_slack: Option<i32>,
}

impl Sequential {
    /// The same report at a longer `period`: the clock is high for half of it,
    /// the outputs are read a tick before its end, the inputs may change until
    /// as long before the next edge as before, and every setup slack grows by
    /// the difference. Slowing the clock changes nothing else.
    pub fn at_period(&self, period: u32) -> Sequential {
        assert!(period >= self.period, "a period of {period} is shorter than the {} analysed", self.period);
        let longer = period - self.period;
        let mut slowed = self.clone();
        slowed.period = period;
        slowed.high = period / 2;
        slowed.sample_at = period - 1;
        slowed.input_change_by += longer;
        for t in &mut slowed.flip_flops {
            if let Some(slack) = &mut t.setup_slack {
                *slack += longer as i32;
            }
        }
        slowed
    }
}

/// The longest clock period offered: past it the analysis gives up.
pub const MAX_PERIOD: i32 = 400;

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

/// The most times a torch may turn off within sixty ticks: the game burns one
/// out on the eighth, so this leaves one to spare.
pub const BURNOUT_BUDGET: usize = 6;

/// What a sequential analysis needs to know about each flip-flop.
pub struct FlipFlop {
    pub cell: String,
    /// Nets at its ports, in the circuit the analysis runs on.
    pub d: Pos,
    pub clk: Pos,
    pub q: Pos,
}

/// The flip-flop tile's figures (stage 8).
#[derive(Clone, Copy, Debug)]
pub struct FlipFlopSpec {
    pub setup: i32,
    pub hold: i32,
    pub clk_to_q: i32,
    pub min_high: i32,
    pub min_low: i32,
    pub min_period: i32,
}

/// Timing of a clocked design: the circuit must be the one *outside* the
/// flip-flops (see [`Circuit::outside`]), `clock` the probe of the clock input.
/// Returns the report and, per flip-flop, the hold slack, so that short paths
/// can be lengthened and the analysis run again.
pub fn analyze_sequential(
    world: &World,
    circuit: &Circuit,
    probes: &[Pos],
    clock: (String, Pos),
    lamps: &[(String, Pos)],
    flip_flops: &[FlipFlop],
    spec: FlipFlopSpec,
) -> Result<Timing, String> {
    let paths = Paths::new(world, circuit)?;
    let net_at = |pos: Pos, what: &str| circuit.net_of(pos).ok_or_else(|| format!("{what}: no wire at {pos}"));
    let both = |t: u32| Edges { rise: Some((t, t)), fall: Some((t, t)) };

    // The clock: which edge of the lever makes every flip-flop's port fall, and when.
    let clock_nets: Vec<usize> = probe_nets(circuit, &[clock.1]);
    if clock_nets.is_empty() {
        return Err(format!("the clock input {} drives no wire", clock.0));
    }
    let mut active = None;
    let mut arrivals = Vec::new();
    for (edge, name) in
        [(Edges { rise: Some((0, 0)), fall: None }, "rising"), (Edges { rise: None, fall: Some((0, 0)) }, "falling")]
    {
        let starts: Vec<(usize, Edges)> = clock_nets.iter().map(|&n| (n, edge)).collect();
        let edges = paths.arrivals(circuit, &starts)?;
        let falls: Vec<Option<(u32, u32)>> =
            flip_flops.iter().map(|f| circuit.net_of(f.clk).and_then(|n| edges[n]).and_then(|e| e.fall)).collect();
        if falls.iter().all(|f| f.is_some()) {
            active = Some(name);
            arrivals = falls.into_iter().map(|f| f.expect("some")).collect();
            break;
        }
    }
    let active = active.ok_or("no edge of the clock input reaches every flip-flop's clock port as a falling edge")?;
    let mut flip_flop_timing: Vec<FlipFlopTiming> = Vec::new();
    for (f, (lo, hi)) in flip_flops.iter().zip(&arrivals) {
        if lo != hi {
            return Err(format!(
                "flip-flop {}: its clock arrives by paths of different lengths ({lo} to {hi} ticks)",
                f.cell
            ));
        }
        flip_flop_timing.push(FlipFlopTiming { cell: f.cell.clone(), clock_arrival: *lo, ..Default::default() });
    }

    // Data from the flip-flops: each Q changes clk_to_q after its clock port falls.
    let launches: Vec<(usize, Edges)> = flip_flops
        .iter()
        .zip(&flip_flop_timing)
        .map(|(f, t)| {
            net_at(f.q, &format!("flip-flop {} Q", f.cell)).map(|n| (n, both(t.clock_arrival + spec.clk_to_q as u32)))
        })
        .collect::<Result<_, _>>()?;
    let from_flip_flops = paths.arrivals(circuit, &launches)?;
    // Data from the inputs other than the clock, as if they changed at tick 0.
    let data_probes: Vec<Pos> = probes.iter().copied().filter(|&p| p != clock.1).collect();
    let input_starts: Vec<(usize, Edges)> =
        probe_nets(circuit, &data_probes).into_iter().map(|n| (n, both(0))).collect();
    let from_inputs = paths.arrivals(circuit, &input_starts)?;
    let range = |e: Option<Edges>| -> Option<(u32, u32)> {
        let e = e?;
        match (e.rise, e.fall) {
            (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
            (Some(r), None) | (None, Some(r)) => Some(r),
            (None, None) => None,
        }
    };

    // Constraints. T is the period; inputs change at tau after the edge.
    //   flip-flop to flip-flop: hold  clk_i + C + min >= clk_j + H   (fixed by lengthening paths)
    //                           setup clk_i + C + max + S <= clk_j + T
    //   inputs:                 hold  tau + min >= clk_j + H
    //                           setup tau + max + S <= clk_j + T
    let mut period = spec.min_period.max(2 * spec.min_high).max(2 * spec.min_low);
    let mut tau_from: i32 = 0;
    let mut tau_by_minus_period: i32 = i32::MAX; // tau <= T + this
    let mut violations = Vec::new();
    for (f, t) in flip_flops.iter().zip(&mut flip_flop_timing) {
        let d = net_at(f.d, &format!("flip-flop {} D", f.cell))?;
        let clk = t.clock_arrival as i32;
        t.d_from_flip_flops = range(from_flip_flops[d]);
        t.d_from_inputs = range(from_inputs[d]);
        if let Some((lo, hi)) = t.d_from_flip_flops {
            let slack = lo as i32 - clk - spec.hold;
            t.hold_slack = Some(slack);
            if slack < 0 {
                violations.push(f.cell.clone());
            }
            period = period.max(hi as i32 + spec.setup - clk);
        }
        if let Some((lo, hi)) = t.d_from_inputs {
            tau_from = tau_from.max(clk + spec.hold - lo as i32);
            tau_by_minus_period = tau_by_minus_period.min(clk - spec.setup - hi as i32);
        }
    }
    // Outputs must settle within the cycle too: from the flip-flops, and from the inputs.
    let mut outputs = BTreeMap::new();
    let mut settle_from_edge: i32 = 0;
    let mut settle_after_inputs: i32 = 0;
    for (name, pos) in lamps {
        let lamp =
            circuit.lamps.iter().find(|l| l.pos == *pos).ok_or_else(|| format!("output {name}: no lamp at {pos}"))?;
        let lit = |edges: &[Option<Edges>]| -> Option<(u32, u32)> {
            lamp.reads.iter().filter_map(|&n| edges[n]).fold(None, |acc: Option<(u32, u32)>, e| {
                let r = range(Some(Edges {
                    rise: e.rise,
                    fall: e.fall.map(|(a, b)| (a + LAMP_OFF_TICKS, b + LAMP_OFF_TICKS)),
                }));
                match (acc, r) {
                    (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
                    (x, None) => x,
                    (None, y) => y,
                }
            })
        };
        let from_ff = lit(&from_flip_flops);
        let from_in = lit(&from_inputs);
        if let Some((_, hi)) = from_ff {
            settle_from_edge = settle_from_edge.max(hi as i32);
        }
        if let Some((_, hi)) = from_in {
            settle_after_inputs = settle_after_inputs.max(hi as i32);
        }
        let (min, max) = match (from_ff, from_in) {
            (Some((a, b)), Some((c, d))) => (a.min(c), b.max(d)),
            (Some(r), None) | (None, Some(r)) => r,
            (None, None) => (0, 0),
        };
        outputs.insert(name.clone(), OutputTiming { min, max, path: Vec::new(), exact: None });
    }
    // Pick the period: setup from flip-flops, the input window, outputs settled
    // before the next edge, an even number, high and low each at least the minimum.
    let tau = tau_from.max(0);
    let period = pick_period(period, tau, tau_by_minus_period, settle_from_edge, settle_after_inputs)?;
    for t in &mut flip_flop_timing {
        if let Some((_, hi)) = t.d_from_flip_flops {
            t.setup_slack = Some(period - (hi as i32 + spec.setup - t.clock_arrival as i32));
        }
    }
    let tau_by = if tau_by_minus_period == i32::MAX { period - 1 } else { period + tau_by_minus_period };
    let sequential = Sequential {
        clock: clock.0,
        active: active.to_string(),
        period: period as u32,
        high: (period / 2) as u32,
        input_change_from: tau as u32,
        input_change_by: tau_by.max(tau) as u32,
        input_change_at: tau as u32,
        sample_at: (period - 1) as u32,
        flip_flops: flip_flop_timing,
        hold_violations: violations,
    };
    Ok(Timing { outputs, exhaustive: false, sequential: Some(sequential), input_spacing: 0 })
}

/// The least time, in game ticks, between the earliest and the latest an input
/// may change after the edge: one redstone tick, so that a lever landing a tick
/// late (the live backend's levers can) still meets setup, the margin stage 8
/// keeps around the flip-flop's own capture.
pub const MIN_INPUT_WINDOW: i32 = 2;

/// The shortest even period from `least` up at which the inputs may change from
/// `tau` after the edge for at least [`MIN_INPUT_WINDOW`] ticks and still meet
/// setup at the next one (an input change may come as late as `period +
/// tau_by_minus_period`; `i32::MAX` when no input reaches a flip-flop), and
/// every output settles within the cycle, from the edge and from the input change.
fn pick_period(
    least: i32,
    tau: i32,
    tau_by_minus_period: i32,
    settle_from_edge: i32,
    settle_after_inputs: i32,
) -> Result<i32, String> {
    let mut period = least;
    loop {
        // An input change may come as late as the next edge, and must come early
        // enough to reach every flip-flop's D in time for it; when that is before
        // the edge the inputs change after, only a longer period helps.
        let tau_by = if tau_by_minus_period == i32::MAX { period - 1 } else { period + tau_by_minus_period.min(0) };
        let outputs_ok = settle_from_edge < period && tau + settle_after_inputs < period;
        if tau + MIN_INPUT_WINDOW <= tau_by && outputs_ok {
            break;
        }
        period += 2;
        if period > MAX_PERIOD {
            return Err(format!("no clock period below {MAX_PERIOD} ticks satisfies the design"));
        }
    }
    if period % 2 == 1 {
        period += 1;
    }
    Ok(period)
}

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

/// The parts of a circuit with their delay rules, and the arrivals they give.
struct Paths<'a> {
    parts: Vec<(Part, &'a [usize], &'a [usize])>,
}

impl<'a> Paths<'a> {
    fn new(world: &World, circuit: &'a Circuit) -> Result<Paths<'a>, String> {
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
        Ok(Paths { parts })
    }

    /// Arrival at every net when the given nets change as `starts` says and
    /// nothing else does, in topological order. Fails on a loop.
    fn arrivals(&self, circuit: &Circuit, starts: &[(usize, Edges)]) -> Result<Vec<Option<Edges>>, String> {
        let parts = &self.parts;
        let mut partial: Vec<Edges> = vec![Edges::NEVER; circuit.nets.len()];
        let mut drivers_left: Vec<usize> = vec![0; circuit.nets.len()];
        for (_, _, drives) in parts {
            for &net in *drives {
                drivers_left[net] += 1;
            }
        }
        for &(net, edges) in starts {
            partial[net] = partial[net].join(edges);
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
            return Err(format!(
                "the circuit has a loop through the part at {pos}; timing needs a combinational circuit"
            ));
        }
        Ok(net_edges)
    }
}

/// What the probes drive: the nets an input change starts at.
fn probe_nets(circuit: &Circuit, probes: &[Pos]) -> Vec<usize> {
    circuit.sources.iter().filter(|s| probes.contains(&s.pos)).flat_map(|s| s.drives.iter().copied()).collect()
}

/// The earliest and latest arrival at each output along any path.
pub fn bound_by_path(
    world: &World,
    circuit: &Circuit,
    probes: &[Pos],
    lamps: &[(String, Pos)],
) -> Result<Timing, String> {
    let paths = Paths::new(world, circuit)?;
    let at_zero = Edges { rise: Some((0, 0)), fall: Some((0, 0)) };
    let starts: Vec<(usize, Edges)> = probe_nets(circuit, probes).into_iter().map(|n| (n, at_zero)).collect();
    let net_edges = paths.arrivals(circuit, &starts)?;
    let parts = &paths.parts;
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
                path = critical_path(circuit, parts, &net_edges, net, falling);
                path.push(format!("lamp at {pos}{}", if falling { " going dark" } else { " lighting" }));
            }
        }
        outputs.insert(name.clone(), OutputTiming { min, max, path, exact: None });
    }
    Ok(Timing { outputs, exhaustive: false, sequential: None, input_spacing: 0 })
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
/// changes far enough apart for that), as long as the order it schedules parts
/// in within a tick is the simulator's, which decides races: two parts due in
/// the same tick fire in the order they were scheduled, so a torch whose input
/// changes in the tick it fires in lights or not by that order (cmp4's `lt`
/// output, 2026-10-31: a two-tick pulse in one order, none in the other). The
/// model applies simultaneous input changes in the simulator's and the harness's
/// order (`trace::event_order` of the probes) for that reason; the propagation
/// order within a net it takes to be the part order, which has agreed so far.
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
    /// The inputs in the order the block simulator and the harness apply
    /// simultaneous changes (`trace::event_order` of the probe blocks): the
    /// order decides which parts' ticks are scheduled first, and so which fires
    /// first when two fall due together.
    input_order: Vec<usize>,
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
        let mut input_order: Vec<usize> = (0..probes.len()).collect();
        input_order.sort_by_key(|&i| crate::trace::event_order(probes[i]));
        Ok(Model { parts, readers, drivers, input_nets, lamps: lamp_parts, rest, input_order })
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
        for &i in &self.input_order {
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
            // A part that has already run this tick takes a new tick for a later
            // change in the same tick (the game drains its running set as it goes).
            let mut running: Vec<usize> = due.iter().map(|t| t.3).collect();
            for (_, _, _, p) in due {
                if let Some(i) = running.iter().position(|&r| r == p) {
                    running.remove(i);
                }
                self.fire(state, p, &running);
            }
            // Once per tick: a lamp that goes dark and lights again within one
            // tick has not changed to anyone watching the blocks tick by tick.
            note(state, &mut lamp_was, &mut changes);
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
    for (step, &next) in walk.iter().enumerate() {
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
            if let Some((pos, count)) =
                sim.torch_turn_offs().into_iter().find(|(_, c)| *c >= crate::sim::BURNOUT_TOGGLES)
            {
                return Err(format!(
                    "the torch at {pos} turned off {count} times within sixty ticks and burned out at step {step} of the walk, {tick} ticks after the change; the measurement cannot go on"
                ));
            }
        }
        // A clean window for the next change: its torches start from no turn-offs.
        while !sim.torch_turn_offs().is_empty() {
            sim.step();
        }
        out.push(changes);
    }
    Ok(out)
}

/// Drives the design's inputs through `walk` with `spacing` ticks between
/// changes, whether or not it has settled, and returns the most times any one
/// torch turned off within sixty ticks, with that torch.
pub fn torch_stress(design: &Design, walk: &[u64], spacing: u32) -> Result<(usize, Option<Pos>), String> {
    let probes = design.probes()?;
    let on: Vec<Block> = design.inputs.iter().map(|p| p.on.parse()).collect::<Result<_, _>>()?;
    let off: Vec<Block> = design.inputs.iter().map(|p| p.off.parse()).collect::<Result<_, _>>()?;
    let n = design.inputs.len();
    let mut sim = Sim::new(design.world()?);
    let mut current = 0u64;
    let mut worst = (0usize, None);
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
        for _ in 0..spacing {
            sim.step();
            for (pos, count) in sim.torch_turn_offs() {
                if count > worst.0 {
                    worst = (count, Some(pos));
                }
            }
        }
    }
    Ok(worst)
}

/// Runs a clocked design for `cycles` cycles of random inputs at its reported
/// period, the inputs changing when the report says, and returns the most times
/// any one torch turned off within sixty ticks, with that torch.
pub fn clock_stress(design: &Design, cycles: u32, seed: u64) -> Result<(usize, Option<Pos>), String> {
    let seq = design.timing.sequential.as_ref().ok_or("not a clocked design")?;
    let mut rng = Rng::new(seed);
    let data: Vec<usize> = (0..design.inputs.len()).filter(|&i| design.inputs[i].name != seq.clock).collect();
    let vectors: Vec<BTreeMap<String, u8>> = (0..cycles)
        .map(|_| data.iter().map(|&i| (design.inputs[i].name.clone(), (rng.draw() & 1) as u8)).collect())
        .collect();
    let mut worst = (0usize, None);
    run_cycles(design, &vectors, None, &mut |sim| {
        for (pos, count) in sim.torch_turn_offs() {
            if count > worst.0 {
                worst = (count, Some(pos));
            }
        }
    })?;
    Ok(worst)
}

/// Drives a clocked design through one vector of inputs per cycle, as the
/// harness does: the clock input makes its active edge at the start of each
/// cycle and the other edge halfway, the inputs change `input_change_at` ticks
/// in, and the outputs are read `sample_at` ticks in. With `shuffle`, the ticks
/// that fall due together run in a random order within each priority. `watch`
/// sees the simulator after every tick.
pub fn run_cycles(
    design: &Design,
    vectors: &[BTreeMap<String, u8>],
    shuffle: Option<u64>,
    watch: &mut dyn FnMut(&Sim),
) -> Result<Vec<BTreeMap<String, u8>>, String> {
    let seq = design.timing.sequential.as_ref().ok_or("not a clocked design")?;
    let probes = design.probes()?;
    let lamps = design.lamps()?;
    let clock = design.inputs.iter().position(|p| p.name == seq.clock).ok_or("the clock input is missing")?;
    let on: Vec<Block> = design.inputs.iter().map(|p| p.on.parse()).collect::<Result<_, _>>()?;
    let off: Vec<Block> = design.inputs.iter().map(|p| p.off.parse()).collect::<Result<_, _>>()?;
    let mut sim = Sim::new(design.world()?);
    let mut rng = shuffle.map(Rng::new);
    let mut step = |sim: &mut Sim| match &mut rng {
        Some(rng) => {
            let groups = sim.due_groups();
            let mut order = Vec::new();
            let mut base = 0;
            for g in groups {
                let mut group: Vec<usize> = (base..base + g).collect();
                for i in (1..g).rev() {
                    group.swap(i, rng.below(i + 1));
                }
                order.extend(group);
                base += g;
            }
            sim.step_in_order(&order);
        }
        None => sim.step(),
    };
    let active_high = seq.active == "rising";
    // The clock rests at the level before its active edge.
    sim.set_block(probes[clock], if active_high { off[clock] } else { on[clock] }, false)?;
    for _ in 0..10 {
        step(&mut sim);
    }
    let mut results = Vec::new();
    for vector in vectors {
        let mut outputs = BTreeMap::new();
        for tick in 0..seq.period {
            if tick == 0 {
                sim.set_block(probes[clock], if active_high { on[clock] } else { off[clock] }, false)?;
            }
            if tick == seq.high {
                sim.set_block(probes[clock], if active_high { off[clock] } else { on[clock] }, false)?;
            }
            if tick == seq.input_change_at {
                let mut changes: Vec<(Pos, Block)> = Vec::new();
                for (i, input) in design.inputs.iter().enumerate() {
                    if i == clock {
                        continue;
                    }
                    let value =
                        *vector.get(&input.name).ok_or_else(|| format!("vector is missing input {}", input.name))?;
                    changes.push((probes[i], if value != 0 { on[i] } else { off[i] }));
                }
                changes.sort_by_key(|(pos, _)| crate::trace::event_order(*pos));
                for (pos, block) in changes {
                    sim.set_block(pos, block, false)?;
                }
            }
            if tick == seq.sample_at {
                for (output, &lamp) in design.outputs.iter().zip(&lamps) {
                    outputs.insert(output.name.clone(), u8::from(sim.world().get(lamp) == Block::Lamp { lit: true }));
                }
            }
            step(&mut sim);
            watch(&sim);
        }
        results.push(outputs);
    }
    Ok(results)
}

/// The walks that stress torches: every input toggling on its own, every pair
/// toggling together (which pulses a gate whose two paths from them differ in
/// length), and random changes.
pub fn stress_walks(n: usize) -> Vec<(String, Vec<u64>)> {
    let mut walks = Vec::new();
    let toggles = |mask: u64| -> Vec<u64> { (0..16).map(|k| if k % 2 == 0 { mask } else { 0 }).collect() };
    for i in 0..n {
        walks.push((format!("input {i}"), toggles(1 << i)));
    }
    for i in 0..n {
        for j in i + 1..n {
            walks.push((format!("inputs {i} and {j} together"), toggles(1 << i | 1 << j)));
        }
    }
    let mut rng = Rng::new(7);
    let mask = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
    walks.push(("random changes".to_string(), (0..128).map(|_| rng.draw() & mask).collect()));
    walks
}

/// The most times any torch turns off within sixty ticks over every stress walk
/// at `spacing` ticks between changes, with the torch and the walk.
pub fn worst_stress(design: &Design, spacing: u32) -> Result<(usize, Option<Pos>, String), String> {
    let mut worst = (0usize, None, String::new());
    for (name, walk) in stress_walks(design.inputs.len()) {
        let (count, pos) = torch_stress(design, &walk, spacing)?;
        if count > worst.0 {
            worst = (count, pos, name);
        }
    }
    Ok(worst)
}

/// The fewest ticks between input changes the stress walks try: less than this
/// no design is offered.
pub const MIN_INPUT_SPACING: u32 = 8;

/// The fewest ticks between input changes that keep every torch within the
/// burnout budget over the stress walks, tried from [`MIN_INPUT_SPACING`] upwards.
pub fn input_spacing(design: &Design) -> Result<u32, String> {
    let mut spacing = MIN_INPUT_SPACING;
    loop {
        let (worst, _, _) = worst_stress(design, spacing)?;
        if worst <= BURNOUT_BUDGET {
            return Ok(spacing);
        }
        spacing += 2;
        if spacing > 200 {
            return Err("some torch turns off too often even with 200 ticks between input changes".to_string());
        }
    }
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
        let reported = design
            .timing
            .outputs
            .get(name)
            .ok_or_else(|| format!("the design file has no timing report for output {name}; build it again"))?;
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

    /// Slowing the clock keeps the report consistent: the high half, the
    /// sample tick and the end of the input window follow the period, every
    /// setup slack grows by the difference, and nothing else moves.
    #[test]
    fn a_report_at_a_longer_period_moves_only_what_the_period_sets() {
        let seq = Sequential {
            clock: "clk".to_string(),
            active: "rising".to_string(),
            period: 92,
            high: 46,
            input_change_from: 3,
            input_change_by: 17,
            input_change_at: 3,
            sample_at: 91,
            flip_flops: vec![
                FlipFlopTiming {
                    cell: "q0".to_string(),
                    clock_arrival: 4,
                    hold_slack: Some(3),
                    setup_slack: Some(10),
                    ..Default::default()
                },
                FlipFlopTiming { cell: "q1".to_string(), clock_arrival: 7, ..Default::default() },
            ],
            hold_violations: Vec::new(),
        };
        let slowed = seq.at_period(100);
        assert_eq!((slowed.period, slowed.high, slowed.sample_at), (100, 50, 99));
        assert_eq!((slowed.input_change_from, slowed.input_change_by, slowed.input_change_at), (3, 25, 3));
        assert_eq!(slowed.flip_flops[0].setup_slack, Some(18));
        assert_eq!(slowed.flip_flops[0].hold_slack, Some(3));
        assert_eq!(slowed.flip_flops[1].setup_slack, None);
        assert_eq!(seq.at_period(92), seq);
    }
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

    /// The full adder's mapped netlist: glitchy enough that a lamp can go dark and
    /// relight within one tick, and a torch can be told of a change in the tick it
    /// has already run in; the model must count neither as the simulator does not.
    const FA1: &str = r#"{"modules":{"top":{"ports":{"a":{"direction":"input","bits":[2]},"b":{"direction":"input","bits":[3]},"cin":{"direction":"input","bits":[4]},"sum":{"direction":"output","bits":[5]},"cout":{"direction":"output","bits":[6]}},"cells":{"c0":{"type":"NOT","connections":{"A":[4],"Y":[7]}},"c1":{"type":"NOT","connections":{"A":[2],"Y":[8]}},"c2":{"type":"NOT","connections":{"A":[3],"Y":[9]}},"c3":{"type":"NOR2","connections":{"A":[8],"B":[9],"Y":[10]}},"c4":{"type":"NOR2","connections":{"A":[2],"B":[3],"Y":[11]}},"c5":{"type":"NOR2","connections":{"A":[10],"B":[11],"Y":[12]}},"c6":{"type":"NOR3","connections":{"A":[7],"B":[10],"C":[11],"Y":[13]}},"c7":{"type":"NOR2","connections":{"A":[4],"B":[12],"Y":[14]}},"c8":{"type":"NOR2","connections":{"A":[13],"B":[14],"Y":[5]}},"c9":{"type":"NOR2","connections":{"A":[10],"B":[13],"Y":[15]}},"c10":{"type":"NOT","connections":{"A":[15],"Y":[6]}}}}}}"#;

    #[test]
    fn the_model_agrees_with_the_simulator_on_the_full_adder_over_random_changes() {
        let design = build(FA1, "top", "fa1").unwrap().design;
        let measurement = measure(&design, 10, 256, 1).unwrap();
        assert_eq!(measurement.problems, Vec::<String>::new());
        assert!(measurement.changes > 256);
    }

    #[test]
    fn edges_join_and_shift() {
        let a = Edges { rise: Some((2, 4)), fall: None };
        let b = Edges { rise: Some((3, 9)), fall: Some((1, 1)) };
        assert_eq!(a.join(b), Edges { rise: Some((2, 9)), fall: Some((1, 1)) });
        assert_eq!(a.shifted(2).inverted(), Edges { rise: None, fall: Some((4, 6)) });
    }

    /// The period: long enough for an input change to reach every flip-flop in
    /// time for the next edge, with a window of at least a redstone tick, even
    /// when that path is longer than the period the flip-flops alone would allow
    /// (the inputs change after the edge, never before it), and for every output
    /// to settle within the cycle.
    #[test]
    fn an_input_path_longer_than_the_period_lengthens_it() {
        // mem2 in two wire levels: the flip-flops allow 58 ticks, but an input
        // reaches a D 74 ticks after it changes, with the clock there at 8 and a
        // setup of 1, so an input changing at the edge needs a period of 67, and
        // the window of two ticks 69, 70 even.
        assert_eq!(pick_period(58, 0, 8 - 1 - 74, 54, 30).unwrap(), 70);
        // Changing 3 ticks after the edge needs 3 more, 72.
        assert_eq!(pick_period(58, 3, 8 - 1 - 74, 54, 30).unwrap(), 72);
        // A short input path leaves the flip-flops' period, rounded up to even.
        assert_eq!(pick_period(57, 0, 8 - 1 - 20, 54, 30).unwrap(), 58);
        // No input reaches a flip-flop: only the outputs bind.
        assert_eq!(pick_period(12, 0, i32::MAX, 30, 40).unwrap(), 42);
        assert_eq!(pick_period(12, 0, -MAX_PERIOD + MIN_INPUT_WINDOW, 0, 0).unwrap(), MAX_PERIOD);
        assert!(pick_period(12, 0, -MAX_PERIOD, 0, 0).is_err());
    }
}
