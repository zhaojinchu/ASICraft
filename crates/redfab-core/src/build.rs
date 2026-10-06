//! From a mapped netlist to a finished, checked block world.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::bake::bake;
use crate::block::Block;
use crate::extract::{Circuit, extract, weak_signals};
use crate::geom::Pos;
use crate::layout::{Macro, Placed};
use crate::lvs::lvs;
use crate::netlist::{Driver, Netlist};
use crate::place::place;
use crate::route::route;
use crate::sim::Sim;
use crate::structure::structure_file;
use crate::tile::Library;
use crate::timing::Timing;
use crate::world::World;

pub const DESIGN_FORMAT: &str = "redfab-design/1";
/// A lamp goes out this many game ticks after it loses power.
/// The most routing columns tried between gate columns before giving up.
const MAX_ROUTING_COLS: i32 = 6;

/// A built design: its blocks in their settled all-inputs-off state, and how to
/// drive and read it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Design {
    pub format: String,
    pub name: String,
    pub size: [i32; 3],
    pub inputs: Vec<InputPin>,
    pub outputs: Vec<OutputPin>,
    /// Constant nets: the block that anchors each (a torch for 1, a dust for 0).
    #[serde(default)]
    pub constants: Vec<ConstantPin>,
    /// Game ticks after an input change by which every output is final: the
    /// worst case in `timing`.
    pub settle_ticks: u32,
    /// Best and worst case for each output, in game ticks after an input change.
    #[serde(default)]
    pub timing: Timing,
    /// Position (`x,y,z`) to full block state.
    pub blocks: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InputPin {
    pub name: String,
    /// The block to swap: `off` for 0, `on` for 1.
    pub probe: String,
    pub on: String,
    pub off: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OutputPin {
    pub name: String,
    pub lamp: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConstantPin {
    pub net: usize,
    pub value: bool,
    pub anchor: String,
}

pub struct Built {
    pub design: Design,
    /// The same blocks as a vanilla structure file.
    pub structure: Vec<u8>,
    pub report: String,
}

impl Design {
    pub fn from_json(json: &str) -> Result<Design, String> {
        let design: Design = serde_json::from_str(json).map_err(|e| format!("design file: {e}"))?;
        if design.format != DESIGN_FORMAT {
            return Err(format!("unexpected design format {:?}", design.format));
        }
        Ok(design)
    }

    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a design always serializes");
        text.push('\n');
        text
    }

    pub fn world(&self) -> Result<World, String> {
        let mut world = World::new((self.size[0], self.size[1], self.size[2]));
        for (pos, state) in &self.blocks {
            world.set(pos.parse()?, state.parse()?);
        }
        Ok(world)
    }

    pub fn probes(&self) -> Result<Vec<Pos>, String> {
        self.inputs.iter().map(|i| i.probe.parse()).collect()
    }

    pub fn lamps(&self) -> Result<Vec<Pos>, String> {
        self.outputs.iter().map(|o| o.lamp.parse()).collect()
    }

    pub fn ties(&self) -> Result<Vec<(usize, bool, Pos)>, String> {
        self.constants.iter().map(|c| Ok((c.net, c.value, c.anchor.parse()?))).collect()
    }
}

/// Places, routes, bakes and checks the design in a Yosys JSON netlist that has
/// been mapped to the tile library. Fails if the layout does not implement the netlist.
pub fn build(mapped_json: &str, top: &str, name: &str) -> Result<Built, String> {
    let library = Library::builtin()?;
    let netlist = Netlist::from_yosys_json(mapped_json, top, &library)?;
    build_netlist(&netlist, name)
}

/// [`build`] from a netlist that is already in memory.
pub fn build_netlist(netlist: &Netlist, name: &str) -> Result<Built, String> {
    let (built, problems) = build_unchecked(netlist, name, Break::None)?;
    if !problems.is_empty() {
        return Err(format!("the layout does not implement the netlist:\n  - {}", problems.join("\n  - ")));
    }
    Ok(built)
}

/// A fault to build in on purpose, for the checks that must catch it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Break {
    None,
    /// Slow the clock to one flip-flop so that a path into it violates hold.
    Hold,
}

/// Rounds of lengthening short paths before giving up on hold.
const HOLD_FIX_ROUNDS: usize = 20;

/// [`build_netlist`], but a layout that fails the checks is returned along with
/// the problems instead of being an error, so it can be looked at.
pub fn build_unchecked(netlist: &Netlist, name: &str, fault: Break) -> Result<(Built, Vec<String>), String> {
    let library = Library::builtin()?;
    let (mut routed, routing_cols) = place_and_route(netlist)?;
    let lamp_names: Vec<String> = netlist.outputs.iter().map(|p| p.name.clone()).collect();

    // Realize, check and time; lengthen the D path of any flip-flop whose hold is
    // violated (a repeater's delay up, or a repeater into a wire), and go again.
    let mut rounds = 0;
    let (realized, world, circuit, mut problems, mut timing) = loop {
        let realized = routed.layout.realize(&library, netlist)?;
        let world = bake(realized.world.clone())?;
        let circuit = extract(&world, &realized.probes);
        let mut problems = check_circuit(
            &world,
            &circuit,
            netlist,
            &realized.probes,
            &realized.lamps,
            &realized.ties,
            &realized.macros,
        );
        let lamps_named: Vec<(String, Pos)> = lamp_names.iter().cloned().zip(realized.lamps.iter().copied()).collect();
        let timing = match analyze_timing(&library, netlist, &world, &circuit, &realized, &lamps_named) {
            Ok(timing) => timing,
            Err(e) => {
                problems.push(format!("timing: {e}"));
                Timing::default()
            }
        };
        let violated: Vec<String> = timing.sequential.as_ref().map(|s| s.hold_violations.clone()).unwrap_or_default();
        if violated.is_empty() || !problems.is_empty() || rounds >= HOLD_FIX_ROUNDS {
            break (realized, world, circuit, problems, timing);
        }
        rounds += 1;
        for cell in &violated {
            let c = netlist.cells.iter().position(|x| x.name == *cell).expect("a flip-flop of the netlist");
            lengthen(&mut routed, netlist, netlist.cells[c].inputs[0], (c, 0))
                .map_err(|e| format!("flip-flop {cell}: cannot lengthen its D path for hold: {e}"))?;
        }
    };
    if let Some(s) = &timing.sequential
        && !s.hold_violations.is_empty()
    {
        problems.push(format!(
            "hold violated at flip-flops {} after {rounds} rounds of fixes",
            s.hold_violations.join(", ")
        ));
    }

    // A fault built in on purpose, after the fixes, so that the checks can catch it.
    let (realized, world, _circuit) = match fault {
        Break::None => (realized, world, circuit),
        Break::Hold => {
            break_layout(&mut routed, netlist, &timing, fault)?;
            let realized = routed.layout.realize(&library, netlist)?;
            let world = bake(realized.world.clone())?;
            let circuit = extract(&world, &realized.probes);
            problems = check_circuit(
                &world,
                &circuit,
                netlist,
                &realized.probes,
                &realized.lamps,
                &realized.ties,
                &realized.macros,
            );
            let lamps_named: Vec<(String, Pos)> =
                lamp_names.iter().cloned().zip(realized.lamps.iter().copied()).collect();
            timing = match analyze_timing(&library, netlist, &world, &circuit, &realized, &lamps_named) {
                Ok(timing) => timing,
                Err(e) => {
                    problems.push(format!("timing: {e}"));
                    Timing::default()
                }
            };
            if let Some(s) = &timing.sequential
                && !s.hold_violations.is_empty()
            {
                problems.push(format!("hold violated at flip-flops {}", s.hold_violations.join(", ")));
            }
            (realized, world, circuit)
        }
    };
    let layout = &routed.layout;
    let settle_ticks = timing.settle_ticks();
    let repeater_count = layout
        .tiles
        .values()
        .filter(|p| matches!(p, Placed::Repeater { .. } | Placed::Cross { repeater_entry: Some(_), .. }))
        .count();

    let (sx, sy, sz) = world.size();
    let design = Design {
        format: DESIGN_FORMAT.to_string(),
        name: name.to_string(),
        size: [sx, sy, sz],
        inputs: netlist
            .inputs
            .iter()
            .zip(&realized.probes)
            .map(|(port, probe)| InputPin {
                name: port.name.clone(),
                probe: probe.to_string(),
                on: Block::RedstoneBlock.to_string(),
                off: Block::Stone.to_string(),
            })
            .collect(),
        outputs: netlist
            .outputs
            .iter()
            .zip(&realized.lamps)
            .map(|(port, lamp)| OutputPin { name: port.name.clone(), lamp: lamp.to_string() })
            .collect(),
        constants: realized
            .ties
            .iter()
            .map(|&(net, value, pos)| ConstantPin { net, value, anchor: pos.to_string() })
            .collect(),
        settle_ticks,
        timing: timing.clone(),
        blocks: world.iter().map(|(pos, block)| (pos.to_string(), block.to_string())).collect(),
    };
    // The burnout budget: how fast the inputs may change (combinational designs),
    // or whether the clock period respects it (clocked ones).
    let design = {
        let mut design = design;
        match &timing.sequential {
            None if problems.is_empty() => {
                timing.input_spacing = crate::timing::input_spacing(&design)?;
                design.timing.input_spacing = timing.input_spacing;
            }
            Some(seq) if problems.is_empty() => {
                let (worst, pos) = crate::timing::clock_stress(&design, 64, 1)?;
                if worst > crate::timing::BURNOUT_BUDGET {
                    problems.push(format!(
                        "the torch at {} turns off {worst} times within sixty ticks at a clock period of {}; the budget is {}",
                        pos.map_or("?".to_string(), |p| p.to_string()),
                        seq.period,
                        crate::timing::BURNOUT_BUDGET
                    ));
                }
            }
            _ => {}
        }
        design
    };
    let (cols, rows) = layout.size();
    let timing_line = match &design.timing.sequential {
        Some(seq) => {
            let arrivals: Vec<u32> = seq.flip_flops.iter().map(|f| f.clock_arrival).collect();
            let slack = seq.flip_flops.iter().filter_map(|f| f.hold_slack).min();
            format!(
                "  clock: {} ({} edge), period {} game ticks; inputs change {} ticks after the edge; outputs settle within {}\n  flip-flops: {}, clock arriving {} to {} ticks after the edge, hold slack {}\n",
                seq.clock,
                seq.active,
                seq.period,
                seq.input_change_at,
                design.settle_ticks,
                seq.flip_flops.len(),
                arrivals.iter().min().unwrap_or(&0),
                arrivals.iter().max().unwrap_or(&0),
                slack.map_or("-".to_string(), |s| s.to_string()),
            )
        }
        None => format!(
            "  outputs settle {} to {} game ticks after an input change; inputs may change every {} ticks\n",
            design.timing.outputs.values().map(|t| t.min).min().unwrap_or(0),
            design.settle_ticks,
            design.timing.input_spacing,
        ),
    };
    let report = format!(
        "design {name}\n  cells: {}\n  tiles: {} ({cols} columns x {rows} rows, {} routing columns between gate columns)\n  repeaters: {repeater_count}\n  blocks: {} in {sx} x {sy} x {sz}\n{timing_line}{}",
        netlist.cells.len(),
        layout.tiles.len(),
        routing_cols,
        design.blocks.len(),
        if problems.is_empty() { "  layout matches the netlist\n" } else { "" },
    );
    Ok((Built { structure: structure_file(&world), design, report }, problems))
}

/// Lengthens the path of `net` to `sink` by two game ticks: the last tile of the
/// branch that no other sink of the net shares gets a repeater, or its repeater
/// one more setting. Crossings cannot take one.
fn lengthen(
    routed: &mut crate::route::Routed,
    netlist: &Netlist,
    net: usize,
    sink: (usize, usize),
) -> Result<(), String> {
    let index = netlist.sinks_of(net).iter().position(|s| *s == sink).ok_or("not a sink of the net")?;
    let branches = routed.branches.get(&net).ok_or("the net was not routed")?;
    let own = &branches[index];
    let shared = |cell: &crate::place::Cell| {
        branches.iter().enumerate().any(|(i, b)| i != index && b.iter().any(|(c, _)| c == cell))
    };
    for &(cell, entry) in own.iter().rev() {
        if shared(&cell) {
            continue;
        }
        match routed.layout.tiles.get(&cell).cloned() {
            Some(Placed::Wire { edges }) => {
                routed.layout.tiles.insert(cell, Placed::Repeater { edges, entry, delay: 1 });
                return Ok(());
            }
            Some(Placed::Repeater { edges, entry, delay }) if delay < 4 => {
                routed.layout.tiles.insert(cell, Placed::Repeater { edges, entry, delay: delay + 1 });
                return Ok(());
            }
            _ => continue,
        }
    }
    Err("no tile of its own on the way can take a repeater or a longer one".to_string())
}

/// Builds a fault in: slows the clock to the flip-flop with the least hold slack
/// on its own clock branch until a path into it violates hold.
fn break_layout(
    routed: &mut crate::route::Routed,
    netlist: &Netlist,
    timing: &Timing,
    fault: Break,
) -> Result<(), String> {
    match fault {
        Break::None => Ok(()),
        Break::Hold => {
            let seq = timing.sequential.as_ref().ok_or("the design has no flip-flops to break")?;
            let victim = seq
                .flip_flops
                .iter()
                .filter(|f| f.hold_slack.is_some())
                .min_by_key(|f| f.hold_slack)
                .ok_or("no flip-flop is fed by another")?;
            let c = netlist.cells.iter().position(|x| x.name == victim.cell).expect("a flip-flop of the netlist");
            let clock_net = netlist.cells[c].inputs[1];
            // Two ticks per step: past the slack, past the hold time's margin, and
            // two more, so that the new value is captured beyond doubt.
            let hold = Library::builtin()?.dff.timing.get("hold").copied().unwrap_or(0);
            let steps = (victim.hold_slack.unwrap_or(0) + hold) / 2 + 2;
            let mut done = 0;
            for _ in 0..steps {
                if lengthen(routed, netlist, clock_net, (c, 1)).is_err() {
                    break;
                }
                done += 1;
            }
            if done < steps {
                return Err(format!(
                    "flip-flop {}: its clock branch takes only {done} of the {steps} delay steps a clear hold violation needs",
                    victim.cell
                ));
            }
            Ok(())
        }
    }
}

/// The timing report: combinational designs get the bound and the exact figures,
/// clocked designs the sequential analysis on the circuit outside the flip-flops.
fn analyze_timing(
    library: &Library,
    netlist: &Netlist,
    world: &World,
    circuit: &Circuit,
    realized: &crate::layout::Realized,
    lamps: &[(String, Pos)],
) -> Result<Timing, String> {
    if realized.macros.is_empty() {
        return crate::timing::analyze_circuit(world, circuit, &realized.probes, lamps);
    }
    let (outside, _) = circuit.outside(&|pos| realized.macros.iter().any(|m| m.contains(pos)));
    let clock = clock_input(netlist)?;
    let clock = (netlist.inputs[clock].name.clone(), realized.probes[clock]);
    let flip_flops: Vec<crate::timing::FlipFlop> = realized
        .macros
        .iter()
        .map(|m| crate::timing::FlipFlop { cell: netlist.cells[m.cell].name.clone(), d: m.d, clk: m.clk, q: m.q })
        .collect();
    let t = &library.dff.timing;
    let figure = |key: &str| t.get(key).copied().ok_or_else(|| format!("the flip-flop tile does not state {key}"));
    let spec = crate::timing::FlipFlopSpec {
        setup: figure("setup")?,
        hold: figure("hold")?,
        clk_to_q: figure("clk_to_q")?,
        min_high: figure("min_high")?,
        min_low: figure("min_low")?,
        min_period: figure("min_period")?,
    };
    crate::timing::analyze_sequential(world, &outside, &realized.probes, clock, lamps, &flip_flops, spec)
}

/// The design input that clocks the flip-flops: every flip-flop's clock pin must
/// trace back, through gates, to one and the same input, which drives nothing else.
pub fn clock_input(netlist: &Netlist) -> Result<usize, String> {
    let drivers = netlist.drivers()?;
    let mut inputs: BTreeSet<usize> = BTreeSet::new();
    for cell in netlist.cells.iter().filter(|c| c.is_dff()) {
        let mut net = cell.inputs[1];
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > netlist.cells.len() + 2 {
                return Err(format!("flip-flop {}: its clock does not come from an input", cell.name));
            }
            match drivers[net] {
                Driver::Input(i) => {
                    inputs.insert(i);
                    break;
                }
                Driver::Cell(c) if netlist.cells[c].kind == "NOT" => net = netlist.cells[c].inputs[0],
                Driver::Cell(c) => {
                    return Err(format!(
                        "flip-flop {}: its clock comes through gate {}, not straight from an input",
                        cell.name, netlist.cells[c].name
                    ));
                }
                Driver::Const(_) => return Err(format!("flip-flop {}: its clock is a constant", cell.name)),
            }
        }
    }
    match inputs.iter().copied().collect::<Vec<_>>().as_slice() {
        [one] => {
            // The clock input may feed only the clock path.
            let name = &netlist.inputs[*one].name;
            for (c, _) in netlist.sinks_of(netlist.inputs[*one].net) {
                if c == usize::MAX {
                    return Err(format!("the clock {name} is also an output"));
                }
                let cell = &netlist.cells[c];
                let on_clock_path = cell.is_dff()
                    || (cell.kind == "NOT"
                        && netlist
                            .sinks_of(cell.output)
                            .iter()
                            .all(|&(d, pin)| d != usize::MAX && netlist.cells[d].is_dff() && pin == 1));
                if !on_clock_path {
                    return Err(format!("the clock {name} is also used as data by {}", cell.name));
                }
            }
            Ok(*one)
        }
        [] => Err("the design has flip-flops but no clock input".to_string()),
        many => Err(format!("flip-flops are clocked by {} different inputs; one clock only", many.len())),
    }
}

/// Places and routes with more routing room each time routing fails. Returns the
/// routed layout and the number of routing columns it took.
pub fn place_and_route(netlist: &Netlist) -> Result<(crate::route::Routed, i32), String> {
    let mut last = String::from("no routing attempt made");
    for routing_cols in 1..=MAX_ROUTING_COLS {
        let placement = place(netlist, routing_cols)?;
        match route(netlist, &placement) {
            Ok(routed) => return Ok((routed, routing_cols)),
            Err(e) => last = e,
        }
    }
    Err(format!("routing failed even with {MAX_ROUTING_COLS} routing columns: {last}"))
}

/// The layout-versus-netlist check and the signal-strength check on a block world.
pub fn check_world(
    world: &World,
    netlist: &Netlist,
    probes: &[Pos],
    lamps: &[Pos],
    ties: &[(usize, bool, Pos)],
) -> Vec<String> {
    check_circuit(world, &extract(world, probes), netlist, probes, lamps, ties, &[])
}

/// [`check_world`] on a circuit already read back from the world. Flip-flops are
/// checked block by block against their tile, and as parts with three ports.
pub fn check_circuit(
    world: &World,
    circuit: &Circuit,
    netlist: &Netlist,
    probes: &[Pos],
    lamps: &[Pos],
    ties: &[(usize, bool, Pos)],
    macros: &[Macro],
) -> Vec<String> {
    let mut problems = Vec::new();
    for m in macros {
        for (pos, want) in &m.blocks {
            let got = world.get(*pos);
            if !same_part(got, *want) {
                problems
                    .push(format!("flip-flop {}: expected {want} at {pos}, found {got}", netlist.cells[m.cell].name));
            }
        }
    }
    problems.extend(lvs(netlist, circuit, probes, lamps, ties, macros));
    problems.extend(weak_signals(world, probes, circuit));
    problems
}

/// Whether two blocks are the same part, ignoring what changes as the circuit
/// runs (power, lit, locked, dust shape).
fn same_part(a: Block, b: Block) -> bool {
    match (a, b) {
        (Block::Wire(_), Block::Wire(_)) => true,
        (Block::Repeater { facing: fa, delay: da, .. }, Block::Repeater { facing: fb, delay: db, .. }) => {
            fa == fb && da == db
        }
        (Block::WallTorch { facing: fa, .. }, Block::WallTorch { facing: fb, .. }) => fa == fb,
        (Block::Torch { .. }, Block::Torch { .. }) => true,
        (Block::Lamp { .. }, Block::Lamp { .. }) => true,
        (a, b) => a == b,
    }
}

/// Re-checks a design file against a netlist. Used to prove that broken layouts are caught.
pub fn check_design(design_json: &str, mapped_json: &str, top: &str) -> Result<Vec<String>, String> {
    let design = Design::from_json(design_json)?;
    let netlist = Netlist::from_yosys_json(mapped_json, top, &Library::builtin()?)?;
    let ties = design.ties()?;
    Ok(check_world(&design.world()?, &netlist, &design.probes()?, &design.lamps()?, &ties))
}

/// Applies input vectors to a design in the simulator, one after another, and
/// returns the outputs after each has settled.
///
/// `vectors` is a JSON list of objects mapping input names to 0 or 1; the
/// result is a JSON list of objects mapping output names to 0 or 1.
pub fn simulate(design_json: &str, vectors_json: &str) -> Result<String, String> {
    simulate_with(design_json, vectors_json, None)
}

/// [`simulate`]; for a clocked design each vector is one clock cycle, applied as
/// the timing report says, and with `shuffle` the ticks that fall due together
/// run in a random order within each priority.
pub fn simulate_with(design_json: &str, vectors_json: &str, shuffle: Option<u64>) -> Result<String, String> {
    let design = Design::from_json(design_json)?;
    let vectors: Vec<BTreeMap<String, u8>> = serde_json::from_str(vectors_json).map_err(|e| format!("vectors: {e}"))?;
    if design.timing.sequential.is_some() {
        let results = crate::timing::run_cycles(&design, &vectors, shuffle, &mut |_| {})?;
        return serde_json::to_string(&results).map_err(|e| e.to_string());
    }
    if shuffle.is_some() {
        return Err("--shuffle is for clocked designs".to_string());
    }
    let probes = design.probes()?;
    let lamps = design.lamps()?;
    let mut sim = Sim::new(design.world()?);
    let mut results = Vec::new();
    for vector in &vectors {
        let mut changes = Vec::new();
        for (input, &probe) in design.inputs.iter().zip(&probes) {
            let value = *vector.get(&input.name).ok_or_else(|| format!("vector is missing input {}", input.name))?;
            let block: Block = if value != 0 { input.on.parse()? } else { input.off.parse()? };
            changes.push((probe, block));
        }
        // Same order as the trace format gives stimuli that share a tick.
        changes.sort_by_key(|(pos, _)| crate::trace::event_order(*pos));
        for (probe, block) in changes {
            sim.set_block(probe, block, false)?;
        }
        for _ in 0..design.settle_ticks {
            sim.step();
        }
        if !sim.pending().is_empty() {
            return Err(format!("the design is still active {} ticks after an input change", design.settle_ticks));
        }
        let mut outputs = BTreeMap::new();
        for (output, &lamp) in design.outputs.iter().zip(&lamps) {
            outputs.insert(output.name.clone(), u8::from(sim.world().get(lamp) == Block::Lamp { lit: true }));
        }
        results.push(outputs);
    }
    serde_json::to_string(&results).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOR2: &str = r#"{"modules":{"top":{
        "ports":{"a":{"direction":"input","bits":[2]},"b":{"direction":"input","bits":[3]},"y":{"direction":"output","bits":[4]}},
        "cells":{"g":{"type":"NOR2","connections":{"A":[2],"B":[3],"Y":[4]}}}}}}"#;

    fn outputs(design: &Design, vectors: &str) -> String {
        simulate(&design.to_json(), vectors).unwrap()
    }

    /// A toggle flip-flop: q <= ~q on the rising edge of clk, starting at 1.
    const TOGGLE: &str = r#"{"modules":{"top":{
        "ports":{"clk":{"direction":"input","bits":[2]},"q":{"direction":"output","bits":[3]}},
        "netnames":{"q":{"bits":[3],"attributes":{"init":"1"}}},
        "cells":{"inv":{"type":"NOT","connections":{"A":[3],"Y":[4]}},
                 "nclk":{"type":"NOT","connections":{"A":[2],"Y":[5]}},
                 "ff":{"type":"DFF","connections":{"D":[4],"CLK":[5],"Q":[3]}}}}}}"#;

    #[test]
    fn a_toggle_flip_flop_builds_and_toggles_from_its_initial_value() {
        let built = build(TOGGLE, "top", "toggle").unwrap();
        let seq = built.design.timing.sequential.clone().expect("a clocked design");
        assert_eq!((seq.clock.as_str(), seq.active.as_str()), ("clk", "rising"));
        assert!(seq.period >= 12 && seq.period.is_multiple_of(2));
        assert_eq!(seq.flip_flops.len(), 1);
        assert!(seq.flip_flops[0].hold_slack.unwrap() >= 0);
        let got = outputs(&built.design, r#"[{},{},{},{}]"#);
        // The first cycle's edge toggles the initial 1 to 0 before the cycle's end;
        // started at 0 the sequence would be the other way round.
        assert_eq!(got, r#"[{"q":0},{"q":1},{"q":0},{"q":1}]"#);
    }

    #[test]
    fn the_clock_must_come_straight_from_one_input_and_feed_nothing_else() {
        let library = Library::builtin().unwrap();
        let netlist = Netlist::from_yosys_json(TOGGLE, "top", &library).unwrap();
        assert_eq!(clock_input(&netlist).unwrap(), 0);
        // The clock also driving a lamp is data.
        let misuse = TOGGLE.replace(
            r#""q":{"direction":"output","bits":[3]}"#,
            r#""q":{"direction":"output","bits":[3]},"c":{"direction":"output","bits":[2]}"#,
        );
        let netlist = Netlist::from_yosys_json(&misuse, "top", &library).unwrap();
        assert!(clock_input(&netlist).unwrap_err().contains("also an output"));
    }

    #[test]
    fn flip_flops_sit_at_level_zero_and_break_loops() {
        let library = Library::builtin().unwrap();
        let netlist = Netlist::from_yosys_json(TOGGLE, "top", &library).unwrap();
        let levels = crate::place::levels(&netlist, &netlist.drivers().unwrap()).unwrap();
        let ff = netlist.cells.iter().position(|c| c.is_dff()).unwrap();
        assert_eq!(levels[ff], 0);
        assert!(levels.iter().enumerate().all(|(c, &l)| c == ff || l == 1));
    }

    #[test]
    fn a_nor_gate_builds_and_computes_nor() {
        let built = build(NOR2, "top", "nor2").unwrap();
        let vectors = r#"[{"a":0,"b":0},{"a":0,"b":1},{"a":1,"b":0},{"a":1,"b":1}]"#;
        assert_eq!(outputs(&built.design, vectors), r#"[{"y":1},{"y":0},{"y":0},{"y":0}]"#);
    }

    #[test]
    fn the_built_state_is_already_settled() {
        let built = build(NOR2, "top", "nor2").unwrap();
        let world = built.design.world().unwrap();
        let mut sim = Sim::new(world.clone());
        sim.poke_all();
        assert!(sim.pending().is_empty());
        assert_eq!(sim.world(), &world);
    }

    #[test]
    fn removing_the_torch_is_caught() {
        let mut design = build(NOR2, "top", "nor2").unwrap().design;
        let torch = design.blocks.iter().find(|(_, s)| s.contains("wall_torch")).map(|(p, _)| p.clone()).unwrap();
        design.blocks.remove(&torch);
        let problems = check_design(&design.to_json(), NOR2, "top").unwrap();
        assert!(problems.iter().any(|p| p.contains("no torch implements cell g")), "{problems:?}");
    }

    #[test]
    fn a_wrong_netlist_is_caught() {
        // The layout is a NOR of a and b; a netlist that says NOT a must not match it.
        let design = build(NOR2, "top", "nor2").unwrap().design;
        let not_a = NOR2.replace(
            r#""type":"NOR2","connections":{"A":[2],"B":[3],"Y":[4]}"#,
            r#""type":"NOT","connections":{"A":[2],"Y":[4]}"#,
        );
        let problems = check_design(&design.to_json(), &not_a, "top").unwrap();
        assert!(!problems.is_empty());
    }

    #[test]
    fn a_two_cell_design_builds_through_placement_and_routing() {
        let two_cells = NOR2
            .replace(r#""cells":{"#, r#""cells":{"h":{"type":"NOT","connections":{"A":[4],"Y":[5]}},"#)
            .replace(r#""y":{"direction":"output","bits":[4]}"#, r#""y":{"direction":"output","bits":[5]}"#);
        let built = build(&two_cells, "top", "x").unwrap();
        let vectors = r#"[{"a":0,"b":0},{"a":0,"b":1},{"a":1,"b":0},{"a":1,"b":1}]"#;
        assert_eq!(outputs(&built.design, vectors), r#"[{"y":0},{"y":1},{"y":1},{"y":1}]"#);
    }

    #[test]
    fn constants_build_as_tie_tiles() {
        // y = NOR(a, 0) = NOT a; z = constant 1
        let json = r#"{"modules":{"top":{
            "ports":{"a":{"direction":"input","bits":[2]},"y":{"direction":"output","bits":[4]},"z":{"direction":"output","bits":["1"]}},
            "cells":{"g":{"type":"NOR2","connections":{"A":[2],"B":["0"],"Y":[4]}}}}}}"#;
        let built = build(json, "top", "c").unwrap();
        assert_eq!(built.design.constants.len(), 2);
        assert_eq!(outputs(&built.design, r#"[{"a":0},{"a":1}]"#), r#"[{"y":1,"z":1},{"y":0,"z":1}]"#);
    }
}
