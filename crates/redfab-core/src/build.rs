//! From a mapped netlist to a finished, checked block world.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::bake::bake;
use crate::block::Block;
use crate::extract::{Circuit, extract, weak_signals};
use crate::geom::Pos;
use crate::layout::{Macro, Placed, Realized};
use crate::lvs::lvs;
use crate::netlist::{Driver, Netlist};
use crate::place::{Placer, Style, estimate_routing};
use crate::route::{Draw, History, Level, RouteFailure, route_with};
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
    /// How the gates were placed: the name of the [`Style`] the build kept.
    #[serde(default)]
    pub placement: String,
    /// How the rows were settled: `swept` (barycenter sweeps) or `searched`
    /// (the best sweep improved by a local search); files written before the
    /// distinction read as empty, and were swept.
    #[serde(default)]
    pub rows: String,
    /// The routing draw the build kept (`route::Draw`): the router's tie-break
    /// seed (0 or 1) and whether the rows below the placed rows were charged
    /// for. Files written before the draws read as the plain router.
    #[serde(default)]
    pub seed: u64,
    #[serde(default)]
    pub rows_below_charged: bool,
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

/// The placement styles a combinational design is laid out in, and a clocked
/// one, in the pick's order (ties go to the earlier). At the five-block pitch
/// the stretched style measured as a loss on every combinational design
/// (adder8 455 x 195 against 470 x 180 packed, adder4 its inputs changing
/// every 12 ticks instead of 8); since the rows are three blocks deep it wins
/// on the mid-size ones (the review of 2026-10-31: divmod 182 x 60 to
/// 146 x 60, adder4, signed_sub, mux4, cmp4 and nor3 smaller), because drawn
/// out over the tallest column's rows the pins of the short columns no longer
/// share the top rows, where the stubs that turn into them must stack over one
/// another's trunks. Above [`STRETCHED_UP_TO_CELLS`] cells it has not won and
/// costs more than the playground's time allows, so only the designs under the
/// gate try it. The packed style never won on a clocked design (over eleven it
/// tied stretched at best, with the same layout).
const COMBINATIONAL_STYLES: &[Style] = &[Style::Pitch4, Style::Packed, Style::Stretched];
const LARGE_COMBINATIONAL_STYLES: &[Style] = &[Style::Pitch4, Style::Packed];
const CLOCKED_STYLES: &[Style] = &[Style::Pitch4, Style::Stretched];

/// Combinational designs with fewer cells than this try the stretched style
/// too: adder4 (49 cells) gains from it, adder8 (102) does not and would take
/// about 29 s more.
const STRETCHED_UP_TO_CELLS: usize = 64;

/// The routing draws every candidate of a small design is routed with, in the
/// pick's order: the plain router; a second tie-break seed, since the
/// negotiation's result moves by up to a tenth of a design's area with any
/// change of the costs it starts from (the I/O columns step, 2026-10-31), so
/// another trajectory through the same placement is another draw; and the
/// plain seed charging the rows below the placed rows (`route::Draw`), which
/// keeps the wiring among the gates and the depth with it on most designs
/// (the depth kill tests, 2026-11-01: -2.5 % of the shipped sum as a third
/// draw, +0.5 % as the only one). The pick keeps the smallest layout and the
/// earlier draw wins ties, so a layout no other draw beats stays as it was.
const ROUTING_DRAWS: &[Draw] = &[
    Draw { seed: 0, charge_below: false },
    Draw { seed: 1, charge_below: false },
    Draw { seed: 0, charge_below: true },
];

/// Designs with fewer cells than this are routed with every draw; a larger one
/// once, for its time. Measured 2026-10-31: the second seed won eight shipped
/// designs of 11 to 31 cells for a tenth to a sixth of their area each, at
/// 0.1 to 3.5 s more a build; adder4 (49 cells) would take twice its 18 s and
/// the capacity designs of 57 and 60 cells twice their 13 to 17 s for layouts
/// it did not beat, and adder8 (102) more than the playground allows.
const EXTRA_DRAWS_UP_TO_CELLS: usize = 48;

/// The draws `netlist`'s candidates are routed with.
fn routing_draws(netlist: &Netlist) -> &'static [Draw] {
    if netlist.cells.len() < EXTRA_DRAWS_UP_TO_CELLS { ROUTING_DRAWS } else { &ROUTING_DRAWS[..1] }
}

/// The placement styles `netlist` is laid out in, in the pick's order.
fn styles_of(netlist: &Netlist) -> &'static [Style] {
    if netlist.cells.iter().any(|c| c.is_dff()) {
        CLOCKED_STYLES
    } else if netlist.cells.len() < STRETCHED_UP_TO_CELLS {
        COMBINATIONAL_STYLES
    } else {
        LARGE_COMBINATIONAL_STYLES
    }
}

/// The pick made again when the kept layout's inputs may change less often
/// than every [`crate::timing::MIN_INPUT_SPACING`] ticks: among the candidates
/// in the pick's order, with their ranks and input spacings, the least spacing,
/// every spacing of the minimum or under alike, then the least rank, ties to
/// the earlier.
fn repick(ranks: &[(u64, u64)], spacings: &[u32]) -> usize {
    (0..ranks.len())
        .min_by_key(|&i| (spacings[i].max(crate::timing::MIN_INPUT_SPACING), ranks[i]))
        .expect("a candidate to pick among")
}

/// [`build_netlist`], but a layout that fails the checks is returned along with
/// the problems instead of being an error, so it can be looked at.
pub fn build_unchecked(netlist: &Netlist, name: &str, fault: Break) -> Result<(Built, Vec<String>), String> {
    build_unchecked_logged(netlist, name, fault, &mut |_| {})
}

/// [`build_unchecked`], telling `log` about each phase.
///
/// Every placement style is taken through placing, routing, the layout check and
/// timing, and the best layout is kept (a combinational design tries pitch4,
/// packed and, under [`STRETCHED_UP_TO_CELLS`] cells, stretched; a clocked one
/// pitch4 and stretched): for a combinational design the smallest footprint,
/// then the shortest settle, provided its inputs may change every
/// [`crate::timing::MIN_INPUT_SPACING`] ticks, else the pick is made again
/// by input spacing first; for a clocked design the least footprint times
/// clock period, then the shorter period (counter4 stretched measured half the
/// footprint of the four-row pitch for a period 6 % longer, negedge packed 6 %
/// less footprint for a period 45 % longer). The pick's order is fixed and ties
/// go to the earlier (the styles' order, the swept rows before the searched), so
/// the same netlist always builds the same. The candidates are laid out in the
/// order they can win, though: the packing style's searched rows, then its
/// swept rows, pitch4 last, and for a combinational design one whose footprint
/// floor already exceeds the footprint kept is given up on, since it can only
/// lose; so pitch4 is seldom laid out at all. Every candidate of a design under
/// [`EXTRA_DRAWS_UP_TO_CELLS`] cells is routed with every draw of
/// [`ROUTING_DRAWS`], the earlier draw earlier in the pick's order. The burnout
/// budget is measured once, on the winner (a clocked design's clock slows until
/// it is met).
pub fn build_unchecked_logged(
    netlist: &Netlist,
    name: &str,
    fault: Break,
    log: Log,
) -> Result<(Built, Vec<String>), String> {
    let library = Library::builtin()?;
    check_netlist(netlist)?;
    let lamp_names: Vec<String> = netlist.outputs.iter().map(|p| p.name.clone()).collect();
    let clocked = netlist.cells.iter().any(|c| c.is_dff());
    let styles = styles_of(netlist);
    // The swept rows in every style; the searched rows in the packing style,
    // where the width is decided, when they promise a smaller footprint; each
    // routed with every draw. Each with its place in the pick's order.
    let draws = routing_draws(netlist);
    let mut placers: Vec<(Placer, (usize, usize))> = Vec::new();
    for (i, &style) in styles.iter().enumerate() {
        let swept = Placer::swept(netlist, style)?;
        if style.packs() {
            let searched = Placer::searched(netlist, style)?;
            let (promised, kept) = (searched.score().0, swept.score().0);
            if promised < kept {
                log(&format!("the searched rows promise {promised} blocks against the swept rows' {kept}"));
                placers.push((searched, (i, 1)));
            }
        }
        placers.push((swept, (i, 0)));
    }
    // Laid out in the order they can win: the searched rows first, pitch4 last.
    placers.sort_by_key(|(placer, _)| placer.style() == Style::Pitch4);
    let mut candidates: Vec<(Candidate, (usize, usize, usize))> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut pitch4_given_up = false;
    for (placer, &(style_at, rows_at)) in placers.iter().map(|(p, at)| (p, at)) {
        for (draw_at, &draw) in draws.iter().enumerate() {
            let name = candidate_name(placer, draw);
            if placers.len() > 1 || draws.len() > 1 {
                log(&format!("{name} placement"));
            }
            // The footprint kept so far bounds a combinational candidate (a clocked
            // design's pick multiplies by the period, which the footprint does not bound).
            let bound = if clocked {
                None
            } else {
                candidates.iter().filter(|(c, _)| c.problems.is_empty()).map(|(c, _)| c.rank(false).0).min()
            };
            match lay_out(&library, netlist, placer, &lamp_names, draw, bound, log) {
                Ok(Some(candidate)) => {
                    log(&format!("  {name}: {}", candidate.summary()));
                    candidates.push((candidate, (style_at, rows_at, draw_at)));
                }
                Ok(None) => pitch4_given_up |= placer.style() == Style::Pitch4 && draw_at == 0,
                Err(e) => {
                    log(&format!("  {name} placement failed: {e}"));
                    failures.push(e);
                }
            }
        }
    }
    if candidates.is_empty() {
        return Err(failures.swap_remove(0));
    }
    // The pick, in its own order, among the layouts that pass the checks (any at all when none does).
    candidates.sort_by_key(|&(_, precedence)| precedence);
    let mut candidates: Vec<Candidate> = candidates.into_iter().map(|(c, _)| c).collect();
    let eligible: Vec<usize> = (0..candidates.len()).filter(|&i| candidates[i].problems.is_empty()).collect();
    let mut chosen = eligible.iter().copied().min_by_key(|&i| candidates[i].rank(clocked)).unwrap_or(0);
    if placers.len() > 1 || draws.len() > 1 {
        log(&format!(
            "keeping the {} placement, {} rows{}",
            candidates[chosen].style.name(),
            candidates[chosen].rows,
            draw_text(candidates[chosen].draw)
        ));
    }

    // A fault built in on purpose, after the fixes, so that the checks can catch it.
    if fault == Break::Hold {
        let faulty = &mut candidates[chosen];
        break_layout(&mut faulty.routed, netlist, &faulty.timing, fault)?;
        let (realized, world, _, mut problems, timing) =
            check_and_time(&library, netlist, &faulty.routed, &lamp_names)?;
        if let Some(s) = &timing.sequential
            && !s.hold_violations.is_empty()
        {
            problems.push(format!("hold violated at flip-flops {}", s.hold_violations.join(", ")));
        }
        (faulty.realized, faulty.world, faulty.problems, faulty.timing) = (realized, world, problems, timing);
    }

    // The burnout budget of the kept layout. A combinational layout whose inputs
    // may change less often than every MIN_INPUT_SPACING ticks is not kept over
    // one whose inputs keep up: the pick is made again over every candidate, by
    // input spacing first, pitch4 laid out now if its footprint made it not
    // worth laying out before.
    let mut finished: Vec<Option<(Design, Vec<String>)>> = candidates.iter().map(|_| None).collect();
    finished[chosen] = Some(finish(&candidates[chosen], netlist, name, log)?);
    let spacing_of =
        |finished: &Option<(Design, Vec<String>)>| finished.as_ref().map_or(0, |(d, _)| d.timing.input_spacing);
    if !clocked && spacing_of(&finished[chosen]) > crate::timing::MIN_INPUT_SPACING {
        log(&format!(
            "the {} placement's inputs may change only every {} ticks; measuring the other candidates' budgets",
            candidates[chosen].style.name(),
            spacing_of(&finished[chosen])
        ));
        if pitch4_given_up {
            match lay_out(
                &library,
                netlist,
                &Placer::swept(netlist, Style::Pitch4)?,
                &lamp_names,
                Draw::default(),
                None,
                log,
            ) {
                Ok(Some(candidate)) => {
                    log(&format!("  pitch4: {}", candidate.summary()));
                    candidates.insert(0, candidate);
                    finished.insert(0, None);
                    chosen += 1;
                }
                Ok(None) => unreachable!("nothing is given up on without a bound"),
                Err(e) => log(&format!("  pitch4 placement failed: {e}")),
            }
        }
        let eligible: Vec<usize> = (0..candidates.len()).filter(|&i| candidates[i].problems.is_empty()).collect();
        for &i in &eligible {
            if finished[i].is_none() {
                finished[i] = Some(finish(&candidates[i], netlist, name, log)?);
            }
        }
        let ranks: Vec<(u64, u64)> = eligible.iter().map(|&i| candidates[i].rank(false)).collect();
        let spacings: Vec<u32> = eligible.iter().map(|&i| spacing_of(&finished[i])).collect();
        let again = eligible[repick(&ranks, &spacings)];
        if again != chosen {
            log(&format!(
                "keeping the {} placement after all: its inputs may change every {} ticks, the {} placement's every {}",
                candidates[again].style.name(),
                spacing_of(&finished[again]),
                candidates[chosen].style.name(),
                spacing_of(&finished[chosen])
            ));
            chosen = again;
        }
    }
    let (design, problems) = finished.swap_remove(chosen).expect("the kept layout is finished");
    let chosen = candidates.swap_remove(chosen);
    let realized = &chosen.realized;
    let (sx, sy, sz) = chosen.world.size();

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
        "design {name}\n  cells: {}\n  tiles: {} ({} columns x {} rows, {routing_text})\n  placement: {}{}{}{}\n  repeaters: {}\n  blocks: {} in {sx} x {sy} x {sz}\n{timing_line}{positions}{}",
        netlist.cells.len(),
        design.tiles,
        design.grid.cols,
        design.grid.rows,
        design.placement,
        if design.rows == "searched" { ", searched rows" } else { "" },
        draw_text(Draw { seed: design.seed, charge_below: design.rows_below_charged }),
        if clocked {
            let rounds = design.hold_fix_rounds;
            format!(", hold fixes: {rounds} round{}", if rounds == 1 { "" } else { "s" })
        } else {
            String::new()
        },
        design.repeaters,
        design.blocks.len(),
        if problems.is_empty() { "  layout matches the netlist\n" } else { "" },
    );
    Ok((Built { structure: structure_file(&chosen.world), design, report }, problems))
}

/// A layout in one placement style, checked and timed, before the pick.
struct Candidate {
    style: Style,
    /// How the rows were settled (`Placer::rows_name`).
    rows: &'static str,
    /// The routing draw (`route::route_with`).
    draw: Draw,
    routed: crate::route::Routed,
    routing_cols: Vec<i32>,
    realized: Realized,
    world: World,
    /// What the layout check and the timing found wrong, if anything.
    problems: Vec<String>,
    timing: Timing,
    hold_fix_rounds: usize,
}

impl Candidate {
    /// What the pick minimises: for a clocked design the footprint times the
    /// clock period, then the period; for the rest the footprint, then the
    /// settle time.
    fn rank(&self, clocked: bool) -> (u64, u64) {
        let (sx, _, sz) = self.world.size();
        let area = (sx * sz) as u64;
        if clocked {
            let period = self.timing.sequential.as_ref().map_or(u64::from(u32::MAX), |s| u64::from(s.period));
            (area * period, period)
        } else {
            (area, u64::from(self.timing.settle_ticks()))
        }
    }

    /// One line of what the layout came to.
    fn summary(&self) -> String {
        let (cols, rows) = self.routed.layout.size();
        let (sx, _, sz) = self.world.size();
        let timing = match &self.timing.sequential {
            Some(seq) => format!("period {}", seq.period),
            None => format!("settle {}", self.timing.settle_ticks()),
        };
        let problems =
            if self.problems.is_empty() { String::new() } else { format!(", {} problems", self.problems.len()) };
        format!("{cols} x {rows} tiles, {sx} x {sz} blocks, {timing}{problems}")
    }
}

/// Places and routes `netlist` as `placer` has it, then realizes, checks and
/// times the layout, lengthening the D path of any flip-flop whose hold is
/// violated (a repeater's delay up, or a repeater into a wire) and going again.
/// The name a candidate's log lines carry: its style, whether its rows are the
/// searched ones, and its routing draw when not the plain router.
fn candidate_name(placer: &Placer, draw: Draw) -> String {
    format!(
        "{}{}{}",
        placer.style().name(),
        if placer.rows_name() == "searched" { ", searched" } else { "" },
        match draw {
            Draw { seed: 0, charge_below: false } => String::new(),
            Draw { seed, charge_below: false } => format!(", seed {seed}"),
            Draw { seed: 0, charge_below: true } => ", rows below charged".to_string(),
            Draw { seed, charge_below: true } => format!(", seed {seed}, rows below charged"),
        }
    )
}

/// How the report and the pick's log name the draw a layout was routed with:
/// nothing for the plain router.
fn draw_text(draw: Draw) -> String {
    let mut text = String::new();
    if draw.seed > 0 {
        text += &format!(", routing seed {}", draw.seed);
    }
    if draw.charge_below {
        text += ", the rows below the gates charged";
    }
    text
}

/// Places, routes (with `draw`), checks and times the rows `placer` settled
/// on. `bound` is a footprint in blocks already kept: a placement whose floor
/// exceeds it can only lose the pick, so it is given up on and `None` returned.
fn lay_out(
    library: &Library,
    netlist: &Netlist,
    placer: &Placer,
    lamp_names: &[String],
    draw: Draw,
    bound: Option<u64>,
    log: Log,
) -> Result<Option<Candidate>, String> {
    let style = placer.style();
    let (mut routed, routing_cols) = match place_and_route_bounded(library, netlist, placer, draw, bound, log)? {
        Routing::Done(routed, routing_cols) => (routed, routing_cols),
        Routing::GivenUp { attempt, floor: (width, depth) } => {
            log(&format!(
                "  {} placement {}: at least {width} x {depth} = {} blocks against the {} kept",
                candidate_name(placer, draw),
                if attempt == 1 { "not laid out".to_string() } else { format!("given up at attempt {attempt}") },
                width as u64 * depth as u64,
                bound.unwrap_or(0)
            ));
            return Ok(None);
        }
    };
    log("checking the layout against the netlist and timing it");
    let mut rounds = 0;
    loop {
        let (realized, world, _, mut problems, timing) = check_and_time(library, netlist, &routed, lamp_names)?;
        let violated: Vec<String> = timing.sequential.as_ref().map(|s| s.hold_violations.clone()).unwrap_or_default();
        if violated.is_empty() || !problems.is_empty() || rounds >= HOLD_FIX_ROUNDS {
            if !violated.is_empty() {
                problems.push(format!(
                    "hold violated at flip-flops {} after {rounds} rounds of fixes",
                    violated.join(", ")
                ));
            }
            problems.extend(unlengthenable(&routed, netlist));
            return Ok(Some(Candidate {
                style,
                rows: placer.rows_name(),
                draw,
                routed,
                routing_cols,
                realized,
                world,
                problems,
                timing,
                hold_fix_rounds: rounds,
            }));
        }
        rounds += 1;
        log(&format!("hold fix round {rounds}: lengthening the D path of {}", violated.join(", ")));
        for cell in &violated {
            let c = netlist.cells.iter().position(|x| x.label == *cell).expect("a flip-flop of the netlist");
            lengthen(&mut routed, netlist, netlist.cells[c].inputs[0], (c, 0))
                .map_err(|e| format!("flip-flop {cell}: cannot lengthen its D path for hold: {e}"))?;
        }
    }
}

/// Realizes a routed layout as blocks, checks them against the netlist and times them.
fn check_and_time(
    library: &Library,
    netlist: &Netlist,
    routed: &crate::route::Routed,
    lamp_names: &[String],
) -> Result<(Realized, World, Circuit, Vec<String>, Timing), String> {
    let realized = routed.layout.realize(library, netlist)?;
    let world = bake(realized.world.clone())?;
    let circuit = extract(&world, &realized.probes);
    let mut problems =
        check_circuit(&world, &circuit, netlist, &realized.probes, &realized.lamps, &realized.ties, &realized.macros);
    let lamps_named: Vec<(String, Pos)> = lamp_names.iter().cloned().zip(realized.lamps.iter().copied()).collect();
    let timing = match analyze_timing(library, netlist, &world, &circuit, &realized, &lamps_named) {
        Ok(timing) => timing,
        Err(e) => {
            problems.push(format!("timing: {e}"));
            Timing::default()
        }
    };
    Ok((realized, world, circuit, problems, timing))
}

/// The design file of a candidate, with the burnout budget measured: how fast
/// the inputs may change (combinational designs), or whether the clock period
/// respects it (clocked ones, a problem when not). Returns the design and the
/// candidate's problems with any the budget adds.
fn finish(candidate: &Candidate, netlist: &Netlist, name: &str, log: Log) -> Result<(Design, Vec<String>), String> {
    let layout = &candidate.routed.layout;
    let realized = &candidate.realized;
    let timing = &candidate.timing;
    let mut problems = candidate.problems.clone();
    let repeater_count = layout.repeater_count();
    let (cols, rows) = layout.size();
    let (sx, sy, sz) = candidate.world.size();
    let mut design = Design {
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
        settle_ticks: timing.settle_ticks(),
        timing: timing.clone(),
        placement: candidate.style.name().to_string(),
        rows: candidate.rows.to_string(),
        seed: candidate.draw.seed,
        rows_below_charged: candidate.draw.charge_below,
        tiles: layout.cells_used(),
        grid: Grid { cols, rows, routing: candidate.routing_cols.clone() },
        repeaters: repeater_count,
        hold_fix_rounds: candidate.hold_fix_rounds,
        blocks: candidate.world.iter().map(|(pos, block)| (pos.to_string(), block.to_string())).collect(),
    };
    if problems.is_empty() {
        log("measuring the burnout budget");
    }
    match &timing.sequential {
        None if problems.is_empty() => {
            design.timing.input_spacing = crate::timing::input_spacing(&design)?;
        }
        Some(_) if problems.is_empty() => {
            if let Err(e) = slow_clock_to_budget(&mut design, log) {
                problems.push(e);
            }
        }
        _ => {}
    }
    Ok((design, problems))
}

/// Runs the clock stress at the design's period and, while some torch turns off
/// more often than the budget allows, slows the clock two ticks at a time; a
/// problem when no period up to the longest offered keeps within the budget.
fn slow_clock_to_budget(design: &mut Design, log: Log) -> Result<(), String> {
    loop {
        let (worst, pos) = crate::timing::clock_stress(design, 64, 1)?;
        if worst <= crate::timing::BURNOUT_BUDGET {
            return Ok(());
        }
        let seq = design.timing.sequential.as_ref().expect("a clocked design");
        let torch = pos.map_or("?".to_string(), |p| p.to_string());
        if seq.period as i32 + 2 > crate::timing::MAX_PERIOD {
            return Err(format!(
                "the torch at {torch} turns off {worst} times within sixty ticks at a clock period of {}; the budget is {}, and no longer period is offered",
                seq.period,
                crate::timing::BURNOUT_BUDGET
            ));
        }
        log(&format!(
            "the torch at {torch} turns off {worst} times within sixty ticks at a clock period of {}; the budget is {}, so the clock slows to {}",
            seq.period,
            crate::timing::BURNOUT_BUDGET,
            seq.period + 2
        ));
        design.timing.sequential = Some(seq.at_period(seq.period + 2));
    }
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
/// one more setting; a branch with no tile of its own (one that is a prefix of
/// another sink's) lengthens at the last tile it shares, and the paths through
/// that tile with it. Crossings cannot take one.
fn lengthen(
    routed: &mut crate::route::Routed,
    netlist: &Netlist,
    net: usize,
    sink: (usize, usize),
) -> Result<(), String> {
    let (cell, level, longer) = lengthening(routed, netlist, net, sink)?;
    match level {
        Level::Upper => routed.layout.upper.insert(cell, longer),
        Level::Lower => routed.layout.tiles.insert(cell, longer),
    };
    Ok(())
}

/// What [`lengthen`] would change: the tile, the level it is on, and what it becomes.
fn lengthening(
    routed: &crate::route::Routed,
    netlist: &Netlist,
    net: usize,
    sink: (usize, usize),
) -> Result<(crate::place::Cell, Level, Placed), String> {
    let index = netlist.sinks_of(net).iter().position(|s| *s == sink).ok_or("not a sink of the net")?;
    let branches = routed.branches.get(&net).ok_or("the net was not routed")?;
    let own = &branches[index];
    let shared = |cell: &crate::place::Cell| {
        branches.iter().enumerate().any(|(i, b)| i != index && b.iter().any(|(c, _, _)| c == cell))
    };
    // Tiles of its own first, from the sink back; then the shared ones.
    for only_own in [true, false] {
        for &(cell, entry, level) in own.iter().rev() {
            if shared(&cell) == only_own {
                continue;
            }
            let map = if level == Level::Upper { &routed.layout.upper } else { &routed.layout.tiles };
            let width = routed.layout.col_width.get(cell.0 as usize).copied().unwrap_or(crate::tile::PITCH);
            let depth = routed.layout.row_depth.get(cell.1 as usize).copied().unwrap_or(crate::tile::PITCH);
            match map.get(&cell).cloned() {
                Some(Placed::Wire { edges }) if crate::layout::can_carry_repeater(width, depth, &edges, entry) => {
                    return Ok((cell, level, Placed::Repeater { edges, entry, delay: 1 }));
                }
                Some(Placed::Repeater { edges, entry, delay }) if delay < 4 => {
                    return Ok((cell, level, Placed::Repeater { edges, entry, delay: delay + 1 }));
                }
                _ => continue,
            }
        }
    }
    Err("no tile on the way can take a repeater or a longer one".to_string())
}

/// The flip-flops whose D path could not be lengthened for hold: the fixer must
/// always have a tile to use, so a layout that leaves one without (a D branch
/// of nothing but crossings and repeaters at their longest) is refused even
/// when its hold is met.
fn unlengthenable(routed: &crate::route::Routed, netlist: &Netlist) -> Vec<String> {
    netlist
        .cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| cell.is_dff())
        .filter_map(|(c, cell)| {
            lengthening(routed, netlist, cell.inputs[0], (c, 0))
                .err()
                .map(|e| format!("flip-flop {}: its D path cannot be lengthened for hold: {e}", cell.label))
        })
        .collect()
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

/// Places (the swept rows) and routes, giving the congested gaps more routing
/// columns each time routing fails. Returns the routed layout and the routing
/// columns per gap.
pub fn place_and_route(
    library: &Library,
    netlist: &Netlist,
    style: Style,
) -> Result<(crate::route::Routed, Vec<i32>), String> {
    place_and_route_logged(library, netlist, &Placer::swept(netlist, style)?, &mut |_| {})
}

/// [`place_and_route`] with the rows `placer` settled on, telling `log` about
/// each attempt: the rows are settled once, and every attempt lays them out in
/// its own columns.
pub fn place_and_route_logged(
    library: &Library,
    netlist: &Netlist,
    placer: &Placer,
    log: Log,
) -> Result<(crate::route::Routed, Vec<i32>), String> {
    match place_and_route_bounded(library, netlist, placer, Draw::default(), None, log)? {
        Routing::Done(routed, routing) => Ok((routed, routing)),
        Routing::GivenUp { .. } => unreachable!("nothing is given up on without a bound"),
    }
}

/// What routing a placement came to.
pub enum Routing {
    /// The routed layout and the routing columns per gap.
    Done(crate::route::Routed, Vec<i32>),
    /// Given up on before attempt `attempt` was routed: the attempt's footprint
    /// floor (width, depth in blocks) exceeded the bound.
    GivenUp { attempt: usize, floor: (i32, i32) },
}

/// [`place_and_route_logged`] with the routing `draw`, giving up as soon as an
/// attempt's footprint floor ([`Placement::footprint_floor`]) exceeds `bound`
/// blocks: attempts only widen gaps and deepen rows, so no later attempt would
/// come under it either.
pub fn place_and_route_bounded(
    library: &Library,
    netlist: &Netlist,
    placer: &Placer,
    draw: Draw,
    bound: Option<u64>,
    log: Log,
) -> Result<Routing, String> {
    let gaps = placer.gaps();
    let first = placer.place(&vec![1; gaps])?;
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
    // Shallow rows made a cell deep because signals kept dying in them.
    let mut deepened: BTreeSet<i32> = BTreeSet::new();
    for attempt in 1..=MAX_ROUTING_ATTEMPTS {
        let mut placement = placer.place(&routing)?;
        for &row in &deepened {
            placement.row_depth[row as usize] = crate::tile::PITCH;
        }
        if let Some(bound) = bound {
            let floor = placement.footprint_floor();
            if floor.0 as u64 * floor.1 as u64 > bound {
                return Ok(Routing::GivenUp { attempt, floor });
            }
        }
        log(&format!(
            "routing attempt {attempt}: {} columns x {} rows, routing columns {}",
            placement.cols,
            placement.rows,
            list(&routing)
        ));
        match route_with(library, netlist, &placement, history.as_ref(), draw, log) {
            Ok(routed) => {
                log(&format!("routed in {} rounds", routed.rounds));
                return Ok(Routing::Done(routed, routing));
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
            Err(RouteFailure::Dying { rows, history: learned }) => {
                let deepen = rows_to_deepen(&rows);
                deepened.extend(&deepen);
                last = format!("signals kept dying in {} shallow rows", rows.len());
                log(&format!("  {last}; making rows {} a cell deep", list(&deepen)));
                history = Some(learned);
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(format!("routing failed after {MAX_ROUTING_ATTEMPTS} attempts: {last}"))
}

/// The shallow rows to make a cell deep after signals kept dying in them: every
/// one that holds a fair share (a quarter of the most) of the tiles the dying
/// signals could not take a repeater in, as the gaps a congested run names are
/// widened. `rows` is per row that count, most first.
fn rows_to_deepen(rows: &[(i32, usize)]) -> Vec<i32> {
    let most = rows.first().map_or(0, |&(_, n)| n);
    rows.iter().filter(|&&(_, n)| n * 4 >= most).map(|&(row, _)| row).collect()
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
        assert!(Style::from_name(&design.placement).is_some(), "placement {:?}", design.placement);
        assert!(["swept", "searched"].contains(&design.rows.as_str()), "rows {:?}", design.rows);
        assert!(["levels", "folded"].contains(&design.columns.as_str()), "columns {:?}", design.columns);
        let draw = Draw { seed: design.seed, charge_below: design.rows_below_charged };
        assert!(ROUTING_DRAWS.contains(&draw), "{draw:?}");
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
            let placer = Placer::swept(&netlist, Style::Pitch4).unwrap();
            let gaps = placer.gaps();
            let estimate = estimate_routing(&netlist, &placer.place(&vec![1; gaps]).unwrap());
            let mut lines: Vec<String> = Vec::new();
            let library = Library::builtin().unwrap();
            let (_, routing) =
                place_and_route_logged(&library, &netlist, &placer, &mut |l| lines.push(l.to_string())).unwrap();
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
    /// them in consecutive rows and the pick takes it over four rows per gate.
    #[test]
    fn inverters_pack_into_consecutive_rows() {
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
    }

    /// Every style the build may pick has a name a design file records and the
    /// harness knows (`harness.area_check.STYLES`), and the name reads back.
    #[test]
    fn every_placement_style_has_a_name_the_design_file_records() {
        for name in ["pitch4", "packed", "stretched"] {
            let style = Style::from_name(name).unwrap_or_else(|| panic!("no placement style named {name}"));
            assert_eq!(style.name(), name);
        }
        assert_eq!(Style::ALL.len(), 3);
        assert!(Style::from_name("stacked").is_none());
    }

    /// The burnout budget: a clocked design whose period would make a torch
    /// turn off more than six times in sixty ticks is slowed, two ticks at a
    /// time, until it keeps within the budget, instead of being refused.
    #[test]
    fn a_clock_too_fast_for_the_burnout_budget_is_slowed_until_it_keeps_within_it() {
        let mut design = build(TOGGLE, "top", "toggle").unwrap().design;
        let analysed = design.timing.sequential.clone().unwrap();
        // At a period of 8 the clock inverter alone turns off 7.5 times in sixty ticks.
        let mut fast = analysed.clone();
        fast.period = 8;
        fast.high = 4;
        fast.sample_at = 7;
        design.timing.sequential = Some(fast);
        let (worst, _) = crate::timing::clock_stress(&design, 64, 1).unwrap();
        assert!(worst > crate::timing::BURNOUT_BUDGET, "{worst} turn-offs at period 8");
        let mut lines: Vec<String> = Vec::new();
        slow_clock_to_budget(&mut design, &mut |l| lines.push(l.to_string())).unwrap();
        let slowed = design.timing.sequential.clone().unwrap();
        assert!(slowed.period > 8 && slowed.period <= analysed.period, "slowed to {}", slowed.period);
        assert!(slowed.period.is_multiple_of(2));
        assert_eq!((slowed.high, slowed.sample_at), (slowed.period / 2, slowed.period - 1));
        let (worst, _) = crate::timing::clock_stress(&design, 64, 1).unwrap();
        assert!(worst <= crate::timing::BURNOUT_BUDGET);
        assert_eq!(lines.len() as u32, (slowed.period - 8) / 2);
        assert!(lines[0].contains("at a clock period of 8; the budget is 6, so the clock slows to 10"), "{}", lines[0]);
    }

    /// The hold fixer always has a tile: in every style, every flip-flop's D
    /// branch of these random clocked netlists can be lengthened, and when a
    /// branch has no tile of its own (it is a prefix of another sink's branch)
    /// the fix goes into a tile it shares.
    #[test]
    fn every_flip_flop_keeps_a_tile_the_hold_fixer_can_use() {
        let library = Library::builtin().unwrap();
        let mut shared_fixes = 0;
        for seed in 100..106u64 {
            let mut rng = crate::tiletest::Rng::new(seed);
            let (gates, inputs, flip_flops) = (2 + rng.below(6), 1 + rng.below(3), 1 + rng.below(3));
            let netlist = crate::pnrtest::random_netlist_sized(&mut rng, gates, inputs, flip_flops);
            for style in Style::ALL {
                let (routed, _) = place_and_route(&library, &netlist, style).unwrap();
                assert!(unlengthenable(&routed, &netlist).is_empty(), "seed {seed}, {}", style.name());
                for (c, cell) in netlist.cells.iter().enumerate().filter(|(_, cell)| cell.is_dff()) {
                    let net = cell.inputs[0];
                    let (at, _, longer) = lengthening(&routed, &netlist, net, (c, 0)).unwrap();
                    assert!(matches!(longer, Placed::Repeater { .. }));
                    let branches = &routed.branches[&net];
                    let index = netlist.sinks_of(net).iter().position(|s| *s == (c, 0)).unwrap();
                    let shared =
                        branches.iter().enumerate().any(|(i, b)| i != index && b.iter().any(|(t, _, _)| *t == at));
                    if shared {
                        shared_fixes += 1;
                        // Only because no tile of its own could take the fix.
                        assert!(branches[index].iter().all(|(t, _, level)| {
                            let map = if *level == Level::Upper { &routed.layout.upper } else { &routed.layout.tiles };
                            branches.iter().enumerate().any(|(i, b)| i != index && b.iter().any(|(u, _, _)| u == t))
                                || !matches!(map.get(t), Some(Placed::Wire { .. }))
                        }));
                    }
                }
            }
        }
        assert!(shared_fixes > 0, "no branch without a tile of its own among the seeds");
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
    /// A clocked design is laid out at the four-row pitch and stretched (the
    /// packed style never won on a clocked design: it ties stretched at best),
    /// and keeps the one with the least footprint times clock period, the period
    /// breaking ties: the toggle's clock inverter can sit beside its flip-flop
    /// when the rows are packed, so stretched beats the four-row pitch, and the
    /// kept layout still toggles.
    #[test]
    fn a_clocked_design_keeps_the_least_area_times_period() {
        let netlist = Netlist::from_yosys_json(TOGGLE, "top", &Library::builtin().unwrap()).unwrap();
        let mut lines: Vec<String> = Vec::new();
        let (built, problems) =
            build_unchecked_logged(&netlist, "toggle", Break::None, &mut |l| lines.push(l.to_string())).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        // Each style's line: "  NAME: C x R tiles, X x Z blocks, period P".
        assert!(!lines.iter().any(|l| l.starts_with("  packed: ")), "{lines:?}");
        let figures: Vec<(&str, u32, u32)> = ["pitch4", "stretched"]
            .iter()
            .map(|&name| {
                let line = lines
                    .iter()
                    .find(|l| l.starts_with(&format!("  {name}: ")))
                    .unwrap_or_else(|| panic!("no {name} layout was tried: {lines:?}"));
                let words: Vec<&str> = line.split_whitespace().collect();
                let at = words.iter().position(|w| w.starts_with("blocks")).unwrap();
                let (x, z): (u32, u32) = (words[at - 3].parse().unwrap(), words[at - 1].parse().unwrap());
                let period: u32 = line.rsplit("period ").next().unwrap().parse().unwrap();
                (name, x * z, period)
            })
            .collect();
        let best = figures.iter().min_by_key(|&&(_, area, period)| (area * period, period)).unwrap();
        assert_eq!(built.design.placement, best.0, "{figures:?}");
        assert_ne!(built.design.placement, "pitch4", "{figures:?}");
        assert_eq!(outputs(&built.design, r#"[{},{},{},{}]"#), r#"[{"q":0},{"q":1},{"q":0},{"q":1}]"#);
    }

    /// A placement's footprint floor is the least footprint any layout of it
    /// can have: its width, which routing adds no column to, by the depth down
    /// to the lowest row a gate, feed, input, output or constant sits in; the
    /// router may only lay rows below that. Widening a gap or deepening a row
    /// before the next attempt only raises it, so a floor above the footprint
    /// already kept says the attempt cannot win.
    #[test]
    fn the_footprint_floor_of_a_placement_never_exceeds_the_footprint_it_routes_to() {
        let library = Library::builtin().unwrap();
        let mut exact = 0;
        for seed in 200..212u64 {
            let mut rng = crate::tiletest::Rng::new(seed);
            let (gates, inputs, flip_flops) = (3 + rng.below(10), 1 + rng.below(4), rng.below(3));
            let netlist = crate::pnrtest::random_netlist_sized(&mut rng, gates, inputs, flip_flops);
            for style in Style::ALL {
                let placer = Placer::swept(&netlist, style).unwrap();
                let first = placer.place(&vec![1; placer.gaps()]).unwrap().footprint_floor();
                let (routed, routing) = place_and_route(&library, &netlist, style).unwrap();
                let last = placer.place(&routing).unwrap().footprint_floor();
                let (sx, _, sz) = routed.layout.realize(&library, &netlist).unwrap().world.size();
                assert!(first.0 <= last.0 && first.1 <= last.1, "seed {seed} {style:?}: {first:?} then {last:?}");
                assert_eq!(last.0, sx, "seed {seed} {style:?}: the width is known before routing");
                assert!(last.1 <= sz, "seed {seed} {style:?}: floor {last:?}, routed {sx} x {sz}");
                if last.1 == sz {
                    exact += 1;
                }
            }
        }
        assert!(exact > 0, "the floor never met the routed depth");
    }

    /// The candidates are laid out in the order they can win (the packing
    /// styles' searched rows, then their swept rows, pitch4 last), and one whose
    /// footprint floor exceeds the footprint already kept is given up on, since
    /// it can only lose: pitch4 is then not laid out at all for most designs.
    /// What the build keeps is what laying every candidate out to completion
    /// and picking as DESIGN 4.3 step 7 says would keep, ties to the earlier
    /// style as before, so no layout changes; and only the kept candidate is
    /// timed for its burnout budget when its inputs keep up.
    #[test]
    fn the_candidates_laid_out_in_the_order_they_can_win_keep_what_laying_them_all_out_keeps() {
        let library = Library::builtin().unwrap();
        let (mut given_up, mut stretched, mut repicked) = (0, 0, 0);
        for seed in 300..306u64 {
            let mut rng = crate::tiletest::Rng::new(seed);
            let gates = 2 + rng.below(10);
            let netlist = crate::pnrtest::random_netlist(&mut rng, gates);
            let lamp_names: Vec<String> = netlist.outputs.iter().map(|p| p.name.clone()).collect();
            // Every candidate to completion, in the order the styles are tried.
            let mut all: Vec<Candidate> = Vec::new();
            for &style in styles_of(&netlist) {
                for columns in [Columns::Levels, Columns::Folded] {
                    // The folded columns only where the fold changes them.
                    if columns == Columns::Folded
                        && gap_count(&netlist, style, Columns::Folded) == gap_count(&netlist, style, Columns::Levels)
                    {
                        continue;
                    }
                    let mut placers = vec![Placer::swept(&netlist, style, columns).unwrap()];
                    if style.packs() {
                        let searched = Placer::searched(&netlist, style, columns).unwrap();
                        if searched.score().0 < placers[0].score().0 {
                            placers.push(searched);
                        }
                    }
                    for placer in &placers {
                        // Each with every routing draw, the earlier draw first in the pick's order.
                        for &draw in routing_draws(&netlist) {
                            all.extend(
                                lay_out(&library, &netlist, placer, &lamp_names, draw, None, &mut |_| {}).unwrap(),
                            );
                        }
                    }
                }
            }
            let expected = all.iter().filter(|c| c.problems.is_empty()).min_by_key(|c| c.rank(false)).unwrap();
            let mut lines: Vec<String> = Vec::new();
            let (built, problems) =
                build_unchecked_logged(&netlist, "random", Break::None, &mut |l| lines.push(l.to_string())).unwrap();
            assert!(problems.is_empty(), "seed {seed}: {problems:?}");
            if lines.iter().any(|l| l.contains(" blocks against the ") && l.contains(" kept")) {
                given_up += 1;
            }
            if lines.iter().any(|l| l == "stretched placement") {
                stretched += 1;
            }
            // The reference pick knows nothing of input spacing: where the first
            // pick's inputs did not keep up and the pick was made again (its own
            // test below), this one has nothing to compare.
            if lines.iter().any(|l| l.contains("measuring the other candidates' budgets")) {
                repicked += 1;
                continue;
            }
            assert_eq!(built.design.timing.input_spacing, crate::timing::MIN_INPUT_SPACING, "seed {seed}");
            let (sx, sy, sz) = expected.world.size();
            assert_eq!(
                (
                    built.design.placement.as_str(),
                    built.design.columns.as_str(),
                    built.design.rows.as_str(),
                    built.design.seed,
                    built.design.rows_below_charged,
                    built.design.size
                ),
                (
                    expected.style.name(),
                    expected.columns.name(),
                    expected.rows,
                    expected.draw.seed,
                    expected.draw.charge_below,
                    [sx, sy, sz]
                ),
                "seed {seed}: {lines:#?}"
            );
            for draw in [", seed 1 placement", ", rows below charged placement"] {
                assert!(
                    lines.iter().any(|l| l.ends_with(draw)),
                    "seed {seed}: no candidate was routed with the draw \"{draw}\": {lines:#?}"
                );
            }
            let timed = lines.iter().filter(|l| l.contains("measuring the burnout budget")).count();
            assert_eq!(timed, 1, "seed {seed}: the burnout budget was measured {timed} times: {lines:#?}");
            let at = |prefix: &str| lines.iter().position(|l| l.starts_with(prefix));
            if let Some(pitch4) = at("pitch4") {
                for style in ["packed", "stretched"] {
                    if let Some(packing) = at(style) {
                        assert!(packing < pitch4, "seed {seed}: pitch4 was laid out before {style}: {lines:#?}");
                    }
                }
            }
        }
        assert!(given_up > 0, "no candidate was ever given up on");
        assert!(stretched > 0, "the stretched style was never a candidate");
        assert!(repicked <= 2, "the pick was made again on {repicked} of 6 netlists");
    }

    /// The stretched style, measured a loss on every combinational design at
    /// the five-block pitch, is a candidate again for the combinational designs
    /// under `STRETCHED_UP_TO_CELLS` cells, since the shallow rows: drawn out
    /// over the tallest column's rows, the pins of the short columns no longer
    /// share the top rows, where the stubs that turn into them must stack over
    /// one another's trunks (the review of 2026-10-31: divmod 182 x 60 to
    /// 146 x 60, adder4, signed_sub, mux4, cmp4 and nor3 smaller). Above the
    /// gate it has not won and costs more than the playground's time allows.
    #[test]
    fn the_stretched_style_is_a_candidate_for_combinational_designs_under_the_gate() {
        let mut rng = crate::tiletest::Rng::new(7);
        let under = crate::pnrtest::random_netlist_sized(&mut rng, STRETCHED_UP_TO_CELLS - 1, 3, 0);
        let at = crate::pnrtest::random_netlist_sized(&mut rng, STRETCHED_UP_TO_CELLS, 3, 0);
        let clocked = crate::pnrtest::random_netlist_sized(&mut rng, 5, 2, 1);
        assert_eq!(under.cells.len(), STRETCHED_UP_TO_CELLS - 1);
        assert_eq!(styles_of(&under), [Style::Pitch4, Style::Packed, Style::Stretched]);
        assert_eq!(styles_of(&at), [Style::Pitch4, Style::Packed]);
        assert_eq!(styles_of(&clocked), [Style::Pitch4, Style::Stretched]);
        // adder4 (49 cells) gains from the style; adder8 (102) does not, and would take about 29 s more.
        assert!((50..=102).contains(&STRETCHED_UP_TO_CELLS), "{STRETCHED_UP_TO_CELLS}");
    }

    /// The negotiation's result moves by up to a tenth of a design's area with
    /// any change of the costs it starts from (the I/O columns step, 2026-10-31),
    /// so every candidate of a design under `EXTRA_DRAWS_UP_TO_CELLS` cells is
    /// routed with every draw of `ROUTING_DRAWS`: the plain router, a second
    /// tie-break seed, and the plain seed with the rows below the placed rows
    /// charged for (the depth kill tests, 2026-11-01), and the pick takes the
    /// smallest layout; a larger design is routed once, for its time (the gate
    /// is where the extra draws cost seconds, not tens of them). The plain
    /// router is first in the pick's order and wins ties, so a layout no other
    /// draw beats stays as it was.
    #[test]
    fn designs_under_the_gate_are_routed_with_every_draw() {
        let mut rng = crate::tiletest::Rng::new(9);
        let under = crate::pnrtest::random_netlist_sized(&mut rng, EXTRA_DRAWS_UP_TO_CELLS - 1, 3, 0);
        let at = crate::pnrtest::random_netlist_sized(&mut rng, EXTRA_DRAWS_UP_TO_CELLS, 3, 0);
        assert_eq!(routing_draws(&under), ROUTING_DRAWS);
        assert_eq!(routing_draws(&at), &ROUTING_DRAWS[..1]);
        assert_eq!(
            ROUTING_DRAWS,
            [
                Draw { seed: 0, charge_below: false },
                Draw { seed: 1, charge_below: false },
                Draw { seed: 0, charge_below: true }
            ]
        );
        assert_eq!(ROUTING_DRAWS[0], Draw::default(), "the plain router is the first draw");
        // The designs the extra draws won have 11 to 32 cells; adder4 (49), the
        // capacity designs and adder8 (102) are routed once, for their time.
        assert!((33..=49).contains(&EXTRA_DRAWS_UP_TO_CELLS), "{EXTRA_DRAWS_UP_TO_CELLS}");
    }

    /// The folded columns (`Columns::Folded`, direction 7 of the 2026-10-31
    /// review) are a candidate beside the levels, in every style and with both
    /// kinds of rows, wherever the fold changes the columns at all. They are laid
    /// out first, being narrower, so that the floor bound can give the levels'
    /// candidates up; the levels are earlier in the pick's order, so a layout the
    /// fold does not beat stays as it was. On a ripple shape the fold wins: the
    /// kept layout is narrower than any layout of the levels can be, and the
    /// design file and the report say so.
    #[test]
    fn the_folded_columns_are_a_candidate_and_win_on_a_ripple_shape() {
        let netlist = crate::pnrtest::ripple_netlist(6, 8);
        let mut lines: Vec<String> = Vec::new();
        let (built, problems) =
            build_unchecked_logged(&netlist, "ripple", Break::None, &mut |l| lines.push(l.to_string())).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(built.design.columns, "folded", "{lines:#?}");
        assert!(built.report.contains("folded columns"), "{}", built.report);
        let narrowest_levels = styles_of(&netlist)
            .iter()
            .map(|&style| {
                let gaps = gap_count(&netlist, style, Columns::Levels).unwrap();
                Placer::swept(&netlist, style, Columns::Levels)
                    .unwrap()
                    .place(&vec![1; gaps])
                    .unwrap()
                    .footprint_floor()
                    .0
            })
            .min()
            .unwrap();
        assert!(
            built.design.size[0] < narrowest_levels,
            "the kept layout is {} wide, the levels at least {narrowest_levels}",
            built.design.size[0]
        );
        let placement_lines: Vec<&String> = lines.iter().filter(|l| l.ends_with(" placement")).collect();
        let first_folded = placement_lines.iter().position(|l| l.contains("folded")).expect("a folded candidate");
        let first_levels = placement_lines
            .iter()
            .position(|l| !l.contains("folded") && !l.starts_with("pitch4"))
            .expect("a levels candidate");
        assert!(first_folded < first_levels, "the folded columns are laid out first: {placement_lines:#?}");
        // Where the fold changes nothing, nothing is laid out twice.
        let flat = crate::pnrtest::random_netlist_sized(&mut crate::tiletest::Rng::new(11), 3, 6, 0);
        for &style in styles_of(&flat) {
            assert_eq!(gap_count(&flat, style, Columns::Folded), gap_count(&flat, style, Columns::Levels), "{style:?}");
        }
        let mut lines: Vec<String> = Vec::new();
        build_unchecked_logged(&flat, "flat", Break::None, &mut |l| lines.push(l.to_string())).unwrap();
        assert!(lines.iter().all(|l| !l.contains("folded")), "{lines:#?}");
    }

    /// The router lays the nets that must change rows along any free row, and an
    /// empty row below the lowest placed row is the freest of all: counter4 kept
    /// 5 of its 18 rows that way, mem2 6 of 27, counter8 6 of 31 (the review of
    /// 2026-10-31, direction 5), each a strip of depth the layout check cannot
    /// trim. The charged draw costs every track in such a row `BELOW_ROW_COST`
    /// more, so a net weaves through the free rows among the gates where it
    /// can and spills below only when it must: over random netlists the rows
    /// the wiring lays below the placed rows fall by a third and the summed
    /// footprint with them. Not a blanket change (the negotiation turns some
    /// designs larger under any cost change: adder8 +6 %, mem2 +9 %), so it is
    /// a draw the pick judges, like the second seed.
    #[test]
    fn charging_the_rows_below_the_placed_rows_keeps_the_wiring_among_the_gates() {
        let library = Library::builtin().unwrap();
        let charged = Draw { seed: 0, charge_below: true };
        let (mut plain_below, mut charged_below, mut plain_area, mut charged_area, mut routed) = (0, 0, 0i64, 0i64, 0);
        for seed in 500..540u64 {
            let mut rng = crate::tiletest::Rng::new(seed);
            let gates = 4 + rng.below(14);
            let flip_flops = if seed % 3 == 0 { 1 + rng.below(3) } else { 0 };
            let inputs = 1 + rng.below(6);
            let netlist = crate::pnrtest::random_netlist_sized(&mut rng, gates, inputs, flip_flops);
            let style = if flip_flops > 0 { Style::Stretched } else { Style::Packed };
            let placer = Placer::swept(&netlist, style).unwrap();
            let lay = |draw: Draw| -> Option<(i32, i64)> {
                let Routing::Done(routed, routing) =
                    place_and_route_bounded(&library, &netlist, &placer, draw, None, &mut |_| {}).ok()?
                else {
                    unreachable!("nothing is given up on without a bound")
                };
                let lowest = placer.place(&routing).unwrap().lowest_row();
                let (cols, rows) = routed.layout.size();
                let width: i32 = routed.layout.col_width[..cols as usize].iter().sum();
                let depth: i32 = routed.layout.row_depth[..rows as usize].iter().sum();
                Some(((rows - 1 - lowest).max(0), i64::from(width) * i64::from(depth)))
            };
            let (Some(plain), Some(with_charge)) = (lay(Draw::default()), lay(charged)) else { continue };
            routed += 1;
            plain_below += plain.0;
            charged_below += with_charge.0;
            plain_area += plain.1;
            charged_area += with_charge.1;
        }
        assert!(routed >= 36, "only {routed} of 40 netlists routed with both draws");
        assert!(
            plain_below >= 40,
            "the plain router laid only {plain_below} rows below the placed rows: nothing to charge for"
        );
        assert!(
            charged_below * 4 <= plain_below * 3,
            "the charged draw laid {charged_below} rows below the placed rows against the plain router's {plain_below}"
        );
        assert!(charged_area < plain_area, "the charged draw's footprints sum to {charged_area} against {plain_area}");
    }

    /// When the kept layout's inputs may change less often than every
    /// `MIN_INPUT_SPACING` ticks, the pick is made again over every candidate:
    /// by input spacing first, every spacing of the minimum or under alike,
    /// then by the rank, ties to the earlier. So the next-best layout whose
    /// inputs keep up is taken, not pitch4 (which the old fallback jumped to:
    /// divmod's stretched searched rows at spacing 10 would have shipped pitch4
    /// at 164 x 69 instead of the stretched swept rows at 146 x 60).
    #[test]
    fn the_pick_made_again_by_input_spacing_takes_the_next_best_whose_inputs_keep_up() {
        // The rank (footprint, settle) per candidate in the pick's order: pitch4, packed, stretched.
        let ranks = [(100u64, 5u64), (60, 4), (70, 3)];
        assert_eq!(repick(&ranks, &[8, 12, 8]), 2, "the stretched layout, not pitch4");
        assert_eq!(repick(&ranks, &[8, 12, 12]), 0, "pitch4 when it alone keeps up");
        assert_eq!(repick(&ranks, &[10, 12, 11]), 0, "the least spacing when none keeps up");
        assert_eq!(repick(&ranks, &[8, 8, 8]), 1, "the rank when all keep up");
        assert_eq!(repick(&ranks, &[12, 10, 10]), 1, "the rank among equal spacings");
        assert_eq!(repick(&[(100, 5), (100, 5)], &[10, 10]), 0, "ties go to the earlier");
    }

    /// Where signals keep dying in shallow rows, the build makes a cell deep
    /// the rows that hold a fair share of the tiles they could not take a
    /// repeater in: a quarter of the most or more.
    #[test]
    fn the_rows_signals_keep_dying_in_are_made_a_cell_deep() {
        assert_eq!(rows_to_deepen(&[(7, 12), (3, 5), (9, 3), (2, 2)]), vec![7, 3, 9]);
        assert_eq!(rows_to_deepen(&[(4, 1)]), vec![4]);
        assert_eq!(rows_to_deepen(&[]), Vec::<i32>::new());
    }

    /// Rows are three blocks deep, except a row a flip-flop sits in, which
    /// stays a full cell: so a combinational design's footprint is three blocks
    /// a row, and a clocked one's three a row and two more for each flip-flop
    /// row; both still compute.
    #[test]
    fn rows_are_three_blocks_deep_but_where_a_flip_flop_sits() {
        let two_cells = NOR2
            .replace(r#""cells":{"#, r#""cells":{"h":{"type":"NOT","connections":{"A":[4],"Y":[5]}},"#)
            .replace(r#""y":{"direction":"output","bits":[4]}"#, r#""y":{"direction":"output","bits":[5]}"#);
        let built = build(&two_cells, "top", "x").unwrap();
        assert_eq!(built.design.size[2], built.design.grid.rows * 3, "{:?}", built.design.grid);
        let vectors = r#"[{"a":0,"b":0},{"a":0,"b":1},{"a":1,"b":0},{"a":1,"b":1}]"#;
        assert_eq!(outputs(&built.design, vectors), r#"[{"y":0},{"y":1},{"y":1},{"y":1}]"#);
        let toggle = build(TOGGLE, "top", "t").unwrap();
        assert_eq!(toggle.design.size[2], toggle.design.grid.rows * 3 + 2, "{:?}", toggle.design.grid);
        assert_eq!(outputs(&toggle.design, r#"[{},{},{},{}]"#), r#"[{"q":0},{"q":1},{"q":0},{"q":1}]"#);
    }

    /// Routing columns take the narrow wiring tiles, three blocks wide; gate,
    /// source and output columns stay a full cell. So a design's footprint is
    /// five blocks for every other column and three for every routing column,
    /// and it still computes.
    #[test]
    fn routing_columns_are_three_blocks_wide() {
        let two_cells = NOR2
            .replace(r#""cells":{"#, r#""cells":{"h":{"type":"NOT","connections":{"A":[4],"Y":[5]}},"#)
            .replace(r#""y":{"direction":"output","bits":[4]}"#, r#""y":{"direction":"output","bits":[5]}"#);
        let built = build(&two_cells, "top", "x").unwrap();
        let routing: i32 = built.design.grid.routing.iter().sum();
        // The sources' and the outputs' columns are three blocks wide too, like a
        // routing column; only the two gate columns are a cell wide.
        let gates = built.design.grid.cols - routing - 2;
        assert!(routing >= 2, "{:?}", built.design.grid);
        assert_eq!(gates, 2, "{:?}", built.design.grid);
        assert_eq!(built.design.size[0], gates * 5 + (routing + 2) * 3, "{:?}", built.design.grid);
        let vectors = r#"[{"a":0,"b":0},{"a":0,"b":1},{"a":1,"b":0},{"a":1,"b":1}]"#;
        assert_eq!(outputs(&built.design, vectors), r#"[{"y":0},{"y":1},{"y":1},{"y":1}]"#);
    }
}
