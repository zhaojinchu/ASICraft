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
use crate::place::{estimate_routing, gap_count, place};
use crate::route::{History, RouteFailure, route};
use crate::sim::Sim;
use crate::structure::structure_file;
use crate::tile::Library;
use crate::timing::Timing;
use crate::world::World;

pub const DESIGN_FORMAT: &str = "redfab-design/1";
/// The most routing columns one gap may grow to before the build gives up.
const MAX_GAP_COLS: i32 = 64;
/// The most routing attempts (each widens the congested gaps) before giving up.
const MAX_ROUTING_ATTEMPTS: usize = 40;
/// The most design inputs: the timing and burnout walks pack the inputs into one
/// 64-bit word.
pub const MAX_INPUTS: usize = 64;

/// Hears what the build is doing, one line at a time.
pub type Log<'a> = &'a mut dyn FnMut(&str);

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
    /// How the gates were placed: `pitch4`, four rows per gate (stage 6).
    #[serde(default)]
    pub placement: String,
    /// Counts of the routed layout, which the area-period baseline
    /// (`tests/baselines/area.json`) holds to: tiles, the tile grid, repeaters
    /// (in their own tiles and in crossings), and the rounds of hold fixes the
    /// build needed. Files written before them read as zero.
    #[serde(default)]
    pub tiles: usize,
    #[serde(default)]
    pub grid: Grid,
    #[serde(default)]
    pub repeaters: usize,
    #[serde(default)]
    pub hold_fix_rounds: usize,
    /// Position (`x,y,z`) to full block state.
    pub blocks: BTreeMap<String, String>,
}

/// The tile grid of a built design, in tiles: `routing` is the routing columns
/// of each gap between gate columns, west to east.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Grid {
    pub cols: i32,
    pub rows: i32,
    pub routing: Vec<i32>,
}

/// The one placement style today, named in `Design::placement`.
pub const PLACEMENT_PITCH4: &str = "pitch4";

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
    build_logged(mapped_json, top, name, &mut |_| {})
}

/// [`build`], telling `log` about each phase.
pub fn build_logged(mapped_json: &str, top: &str, name: &str, log: Log) -> Result<Built, String> {
    let library = Library::builtin()?;
    let netlist = Netlist::from_yosys_json(mapped_json, top, &library)?;
    build_netlist_logged(&netlist, name, log)
}

/// [`build`] from a netlist that is already in memory.
pub fn build_netlist(netlist: &Netlist, name: &str) -> Result<Built, String> {
    build_netlist_logged(netlist, name, &mut |_| {})
}

/// [`build_netlist`], telling `log` about each phase.
pub fn build_netlist_logged(netlist: &Netlist, name: &str, log: Log) -> Result<Built, String> {
    let (built, problems) = build_unchecked_logged(netlist, name, Break::None, log)?;
    if !problems.is_empty() {
        return Err(format!("the layout does not implement the netlist:\n  - {}", problems.join("\n  - ")));
    }
    Ok(built)
}

/// What a netlist must satisfy before any placing is worth doing: not too many
/// inputs for the timing walks, and a clock that comes straight from one input
/// and feeds nothing else. The message says what to change.
pub fn check_netlist(netlist: &Netlist) -> Result<(), String> {
    if netlist.inputs.len() > MAX_INPUTS {
        return Err(format!(
            "the design has {} input bits; at most {MAX_INPUTS} are supported (a 32-bit adder with carry in has 65): narrow the inputs or split the design",
            netlist.inputs.len()
        ));
    }
    if netlist.cells.iter().any(|c| c.is_dff()) {
        clock_input(netlist)?;
    }
    Ok(())
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
    build_unchecked_logged(netlist, name, fault, &mut |_| {})
}

/// [`build_unchecked`], telling `log` about each phase.
pub fn build_unchecked_logged(
    netlist: &Netlist,
    name: &str,
    fault: Break,
    log: Log,
) -> Result<(Built, Vec<String>), String> {
    let library = Library::builtin()?;
    check_netlist(netlist)?;
    let (mut routed, routing_cols) = place_and_route_logged(&library, netlist, log)?;
    log("checking the layout against the netlist and timing it");
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
        log(&format!("hold fix round {rounds}: lengthening the D path of {}", violated.join(", ")));
        for cell in &violated {
            let c = netlist.cells.iter().position(|x| x.label == *cell).expect("a flip-flop of the netlist");
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
    let (cols, rows) = layout.size();

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
        placement: PLACEMENT_PITCH4.to_string(),
        tiles: layout.tiles.len(),
        grid: Grid { cols, rows, routing: routing_cols.clone() },
        repeaters: repeater_count,
        hold_fix_rounds: rounds,
        blocks: world.iter().map(|(pos, block)| (pos.to_string(), block.to_string())).collect(),
    };
    // The burnout budget: how fast the inputs may change (combinational designs),
    // or whether the clock period respects it (clocked ones).
    let design = {
        let mut design = design;
        if problems.is_empty() {
            log("measuring the burnout budget");
        }
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
    // The report prints the counts the design file carries, so the two cannot differ.
    let timing_line = match &design.timing.sequential {
        Some(seq) => sequential_report(seq, design.settle_ticks),
        None if netlist.inputs.is_empty() => "  no inputs: the outputs never change\n".to_string(),
        None => format!(
            "  outputs settle {} to {} game ticks after an input change; inputs may change every {} ticks (found over the stress walks)\n",
            design.timing.outputs.values().map(|t| t.min).min().unwrap_or(0),
            design.settle_ticks,
            design.timing.input_spacing,
        ),
    };
    let mut positions = String::new();
    let max_x = design.blocks.keys().filter_map(|k| k.parse::<Pos>().ok()).map(|p| p.x).max().unwrap_or(0);
    let max_z = design.blocks.keys().filter_map(|k| k.parse::<Pos>().ok()).map(|p| p.z).max().unwrap_or(0);
    positions += &format!(
        "  footprint: x 0 to {max_x}, z 0 to {max_z}, {} x {} chunks when placed at the origin (/forceload add 0 0 {max_x} {max_z})\n",
        max_x / 16 + 1,
        max_z / 16 + 1
    );
    if !design.inputs.is_empty() {
        positions += "  inputs (the lever stands on the probe block; x runs east, z runs south, y up):\n";
        for (input, probe) in design.inputs.iter().zip(&realized.probes) {
            positions += &format!("    {:<16} lever at {},{},{}\n", input.name, probe.x, probe.y + 1, probe.z);
        }
    }
    if !design.outputs.is_empty() {
        positions += "  outputs:\n";
        for (output, lamp) in design.outputs.iter().zip(&realized.lamps) {
            positions += &format!("    {:<16} lamp at {}\n", output.name, lamp);
        }
    }
    let routing_text = match design.grid.routing.as_slice() {
        [one] => format!("{one} routing column{}", if *one == 1 { "" } else { "s" }),
        many => format!(
            "routing columns {} west to east",
            many.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
        ),
    };
    let report = format!(
        "design {name}\n  cells: {}\n  tiles: {} ({} columns x {} rows, {routing_text})\n  repeaters: {}\n  blocks: {} in {sx} x {sy} x {sz}\n{timing_line}{positions}{}",
        netlist.cells.len(),
        design.tiles,
        design.grid.cols,
        design.grid.rows,
        design.repeaters,
        design.blocks.len(),
        if problems.is_empty() { "  layout matches the netlist\n" } else { "" },
    );
    Ok((Built { structure: structure_file(&world), design, report }, problems))
}

/// The clocked design's lines of report.txt: the measured timing, and the rule a
/// player follows in the world, in game ticks and seconds (20 ticks to a second).
fn sequential_report(seq: &crate::timing::Sequential, settle_ticks: u32) -> String {
    let arrivals: Vec<u32> = seq.flip_flops.iter().map(|f| f.clock_arrival).collect();
    let slack = seq.flip_flops.iter().filter_map(|f| f.hold_slack).min();
    let seconds = |ticks: u32| format!("{:.1} s", f64::from(ticks) / 20.0);
    format!(
        "  clock: {} ({} edge), period {} game ticks; inputs change {} ticks after the edge; outputs settle within {}\n  flip-flops: {}, clock arriving {} to {} ticks after the edge, hold slack {}\n  in the world: give {} a period of at least {} ticks ({}), high for {} ({}); move the other levers {} to {} ticks ({} to {}) after its {} edge, and read the lamps {} ticks ({}) after it\n",
        seq.clock,
        seq.active,
        seq.period,
        seq.input_change_at,
        settle_ticks,
        seq.flip_flops.len(),
        arrivals.iter().min().unwrap_or(&0),
        arrivals.iter().max().unwrap_or(&0),
        slack.map_or("n/a (no flip-flop feeds another)".to_string(), |s| s.to_string()),
        seq.clock,
        seq.period,
        seconds(seq.period),
        seq.high,
        seconds(seq.high),
        seq.input_change_from,
        seq.input_change_by,
        seconds(seq.input_change_from),
        seconds(seq.input_change_by),
        seq.active,
        seq.sample_at,
        seconds(seq.sample_at),
    )
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
            let c = netlist.cells.iter().position(|x| x.label == victim.cell).expect("a flip-flop of the netlist");
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
        .map(|m| crate::timing::FlipFlop { cell: netlist.cells[m.cell].label.clone(), d: m.d, clk: m.clk, q: m.q })
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

/// Places and routes, giving the congested gaps more routing columns each time
/// routing fails. Returns the routed layout and the routing columns per gap.
pub fn place_and_route(library: &Library, netlist: &Netlist) -> Result<(crate::route::Routed, Vec<i32>), String> {
    place_and_route_logged(library, netlist, &mut |_| {})
}

/// [`place_and_route`], telling `log` about each attempt.
pub fn place_and_route_logged(
    library: &Library,
    netlist: &Netlist,
    log: Log,
) -> Result<(crate::route::Routed, Vec<i32>), String> {
    let gaps = gap_count(netlist)?;
    let first = place(netlist, &vec![1; gaps])?;
    let mut routing = estimate_routing(netlist, &first);
    log(&format!(
        "placing {} cells in {} gate columns and {} rows; routing columns estimated at {}",
        netlist.cells.len(),
        gaps - 1,
        first.rows,
        list(&routing)
    ));
    let mut history: Option<History> = None;
    let mut last = String::new();
    for attempt in 1..=MAX_ROUTING_ATTEMPTS {
        let placement = place(netlist, &routing)?;
        log(&format!(
            "routing attempt {attempt}: {} columns x {} rows, routing columns {}",
            placement.cols,
            placement.rows,
            list(&routing)
        ));
        match route(library, netlist, &placement, history.as_ref(), log) {
            Ok(routed) => {
                log(&format!("routed in {} rounds", routed.rounds));
                return Ok((routed, routing));
            }
            Err(RouteFailure::Congested { tracks, users, rounds, history: learned }) => {
                // Widen every gap that holds a fair share of the contended tracks.
                let mut per_gap = vec![0usize; gaps];
                for (cell, _) in &tracks {
                    if let Some(g) = placement.gap_of(cell.0) {
                        per_gap[g] += 1;
                    }
                }
                let most = per_gap.iter().copied().max().unwrap_or(0);
                let widen: Vec<usize> = (0..gaps).filter(|&g| per_gap[g] > 0 && per_gap[g] * 4 >= most).collect();
                last = format!(
                    "after {rounds} rounds {} tracks are still wanted by several nets, {} of them in the gap before stage {}",
                    tracks.len(),
                    most,
                    per_gap.iter().position(|&n| n == most).unwrap_or(0) + 1
                );
                if widen.iter().any(|&g| routing[g] >= MAX_GAP_COLS) {
                    return Err(format!(
                        "routing failed: {last}, which already has {MAX_GAP_COLS} routing columns (the most a gap may have); the design is too wide for the router"
                    ));
                }
                for &g in &widen {
                    routing[g] += 1;
                }
                let detail = if tracks.len() <= 12 {
                    tracks
                        .iter()
                        .zip(&users)
                        .map(|(((col, row), track), nets)| {
                            format!("{:?} row {row} {track:?} wanted by nets {}", placement.column(*col), list(nets))
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                } else {
                    let mut by_row = vec![0usize; (placement.rows as usize).div_ceil(10)];
                    for ((_, row), _) in &tracks {
                        by_row[*row as usize / 10] += 1;
                    }
                    format!("by rows of ten: {}", list(&by_row))
                };
                log(&format!(
                    "  {last}; widening gap{} {} ({detail})",
                    if widen.len() == 1 { "" } else { "s" },
                    list(&widen)
                ));
                history = Some(learned);
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(format!("routing failed after {MAX_ROUTING_ATTEMPTS} attempts: {last}"))
}

fn list<T: std::fmt::Display>(items: &[T]) -> String {
    items.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
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

    /// The in-world rule of report.txt, with every figure in ticks and in seconds
    /// (20 game ticks to a second), is what README.md quotes to the player.
    #[test]
    fn the_report_states_the_in_world_rule_in_ticks_and_seconds() {
        use crate::timing::{FlipFlopTiming, Sequential};
        let seq = Sequential {
            clock: "clk".to_string(),
            active: "rising".to_string(),
            period: 92,
            high: 46,
            input_change_from: 0,
            input_change_by: 17,
            input_change_at: 2,
            sample_at: 91,
            flip_flops: vec![
                FlipFlopTiming { cell: "q0".to_string(), clock_arrival: 4, hold_slack: Some(3), ..Default::default() },
                FlipFlopTiming { cell: "q1".to_string(), clock_arrival: 7, hold_slack: Some(5), ..Default::default() },
            ],
            hold_violations: Vec::new(),
        };
        let text = sequential_report(&seq, 60);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "  clock: clk (rising edge), period 92 game ticks; inputs change 2 ticks after the edge; outputs settle within 60",
                "  flip-flops: 2, clock arriving 4 to 7 ticks after the edge, hold slack 3",
                "  in the world: give clk a period of at least 92 ticks (4.6 s), high for 46 (2.3 s); move the other levers 0 to 17 ticks (0.0 s to 0.8 s) after its rising edge, and read the lamps 91 ticks (4.5 s) after it",
            ]
        );
        let alone = Sequential { flip_flops: vec![FlipFlopTiming::default()], ..seq };
        assert!(sequential_report(&alone, 60).contains("hold slack n/a (no flip-flop feeds another)"));
    }

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
        // The build says so directly, before placing anything.
        let Err(err) = build(&misuse, "top", "misuse") else { panic!("the misuse built") };
        assert_eq!(err, "the clock clk is also an output");
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
    fn a_malformed_design_file_is_an_error_not_a_panic() {
        assert!(Design::from_json("not json").unwrap_err().starts_with("design file:"));
        assert!(Design::from_json(r#"{"format":"redfab-design/1"}"#).unwrap_err().starts_with("design file:"));
        let other = r#"{"format":"something/9","name":"x","size":[1,1,1],"inputs":[],"outputs":[],"settle_ticks":0,"blocks":{}}"#;
        assert_eq!(Design::from_json(other).unwrap_err(), "unexpected design format \"something/9\"");
        // A design without a timing block loads (older files), and measuring it says so.
        let bare = r#"{"format":"redfab-design/1","name":"x","size":[1,1,1],"inputs":[],"outputs":[{"name":"y","lamp":"0,0,0"}],"settle_ticks":0,"blocks":{"0,0,0":"minecraft:redstone_lamp[lit=false]"}}"#;
        let design = Design::from_json(bare).unwrap();
        let Err(err) = crate::timing::measure(&design, 10, 1, 1) else { panic!("measured without a report") };
        assert!(err.contains("no timing report for output y"), "{err}");
        // ... and one without the layout counts reads them as zero.
        assert_eq!((design.placement.as_str(), design.tiles, design.repeaters, design.hold_fix_rounds), ("", 0, 0, 0));
        assert_eq!(design.grid, Grid::default());
    }

    /// The counts the area-period baseline reads from design.json are the ones
    /// report.txt prints, and they are counts of the layout, not of the report.
    #[test]
    fn the_design_file_carries_the_counts_the_report_prints() {
        let built = build(NOR2, "top", "nor2").unwrap();
        let design = &built.design;
        assert_eq!(design.placement, PLACEMENT_PITCH4);
        assert_eq!(design.hold_fix_rounds, 0);
        let line = built.report.lines().find(|l| l.contains("tiles:")).unwrap().trim();
        assert_eq!(
            line,
            format!(
                "tiles: {} ({} columns x {} rows, routing columns {} west to east)",
                design.tiles,
                design.grid.cols,
                design.grid.rows,
                design.grid.routing.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
            )
        );
        assert!(built.report.contains(&format!("\n  repeaters: {}\n", design.repeaters)));
        // One gate, one input tile per input, one output tile, and the wire between.
        assert!(design.tiles >= 4 && design.grid.cols >= 3 && design.grid.rows >= 1);
        assert_eq!(design.grid.routing.len(), 2);
        let repeater_blocks = design.blocks.values().filter(|b| b.contains("repeater")).count();
        assert_eq!(design.repeaters, repeater_blocks);
    }

    /// The estimate under-counts the feedback nets of clocked netlists, so the
    /// router reports contended tracks and only the gaps holding them are widened,
    /// until it routes.
    #[test]
    fn the_congested_gaps_are_widened_until_routing_succeeds() {
        let mut widened_runs = 0;
        for seed in 100..106u64 {
            let mut rng = crate::tiletest::Rng::new(seed);
            let (gates, inputs, flip_flops) = (2 + rng.below(6), 1 + rng.below(3), 1 + rng.below(3));
            let netlist = crate::pnrtest::random_netlist_sized(&mut rng, gates, inputs, flip_flops);
            let gaps = gap_count(&netlist).unwrap();
            let estimate = estimate_routing(&netlist, &place(&netlist, &vec![1; gaps]).unwrap());
            let mut lines: Vec<String> = Vec::new();
            let library = Library::builtin().unwrap();
            let (_, routing) = place_and_route_logged(&library, &netlist, &mut |l| lines.push(l.to_string())).unwrap();
            assert_eq!(routing.len(), gaps);
            assert!(routing.iter().zip(&estimate).all(|(r, e)| r >= e), "seed {seed}: {routing:?} < {estimate:?}");
            // Each widening names the gaps it widens; those gaps end wider than estimated.
            let mut widened = vec![false; gaps];
            for line in lines.iter().filter(|l| l.contains("widening gap")) {
                let after = line.split("widening gap").nth(1).unwrap().trim_start_matches('s').trim();
                let named = after.split(" (").next().unwrap();
                for g in named.split(", ") {
                    widened[g.trim().parse::<usize>().unwrap()] = true;
                }
            }
            for g in 0..gaps {
                assert_eq!(routing[g] > estimate[g], widened[g], "seed {seed}, gap {g}: {lines:#?}");
            }
            if widened.iter().any(|&w| w) {
                widened_runs += 1;
            }
        }
        assert!(widened_runs >= 2, "only {widened_runs} of the clocked netlists needed a widening");
    }

    /// A column of inverters reads only on the west, so the packed style lays
    /// them in consecutive rows and the pick takes it over four rows per gate;
    /// the clocked designs keep the four-row pitch in this step.
    #[test]
    fn inverters_pack_into_consecutive_rows_and_clocked_designs_keep_the_pitch() {
        let mut ports = Vec::new();
        let mut cells = Vec::new();
        for i in 0..6 {
            ports.push(format!(r#""a{i}":{{"direction":"input","bits":[{}]}}"#, 2 + i));
            ports.push(format!(r#""y{i}":{{"direction":"output","bits":[{}]}}"#, 10 + i));
            cells.push(format!(r#""n{i}":{{"type":"NOT","connections":{{"A":[{}],"Y":[{}]}}}}"#, 2 + i, 10 + i));
        }
        let json =
            format!(r#"{{"modules":{{"top":{{"ports":{{{}}},"cells":{{{}}}}}}}}}"#, ports.join(","), cells.join(","));
        let built = build(&json, "top", "inverters").unwrap();
        assert_eq!(built.design.placement, "packed");
        // Six inputs, six gates and six outputs, one row each, nothing between.
        assert!(built.design.grid.rows <= 7, "{} rows for six inverters", built.design.grid.rows);
        let vectors = r#"[{"a0":0,"a1":1,"a2":0,"a3":1,"a4":0,"a5":1}]"#;
        assert_eq!(outputs(&built.design, vectors), r#"[{"y0":1,"y1":0,"y2":1,"y3":0,"y4":1,"y5":0}]"#);
        let toggle = build(TOGGLE, "top", "toggle").unwrap();
        assert_eq!(toggle.design.placement, "pitch4");
    }

    #[test]
    fn too_many_inputs_are_refused_before_placing() {
        let mut ports = String::new();
        for i in 0..65 {
            ports += &format!(r#""a{i}":{{"direction":"input","bits":[{}]}},"#, i + 2);
        }
        let json = format!(
            r#"{{"modules":{{"top":{{"ports":{{{ports}"y":{{"direction":"output","bits":[2]}}}},"cells":{{}}}}}}}}"#
        );
        let Err(err) = build(&json, "top", "wide") else { panic!("65 inputs built") };
        assert!(err.starts_with("the design has 65 input bits; at most 64 are supported"), "{err}");
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
