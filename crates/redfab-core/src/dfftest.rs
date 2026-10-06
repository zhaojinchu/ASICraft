//! Flip-flop characterization (stage 8).
//!
//! A flip-flop variant gets sweep fixtures instead of a truth-table fixture: the
//! clock runs continuously and, trial by trial, the data edge is moved one game
//! tick at a time against the active clock edge, in both directions. Each trial
//! is two clock cycles: the data changes once, near the first active edge, and
//! Q is watched until the second. A pulse fixture then clocks the flip-flop with
//! shorter and shorter high and low times while the data is held well ahead.
//!
//! The game runs these fixtures with every other tile fixture and its traces must
//! equal the simulator's (stage 5's check). [`characterize`] replays them in the
//! simulator, reads off when Q changed in each trial, and turns the table into
//! the figures a tile's description must state: setup, hold, clock-to-output and
//! the minimum high and low times. It also replays the sweep under shuffled
//! orders of the ticks that fall due together, ignoring the game's priorities,
//! to find every trial whose outcome depends on same-tick ordering.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::block::Block;
use crate::geom::Pos;
use crate::sim::Sim;
use crate::tile::{Library, PortKind, Variant};
use crate::tiletest::{Fixture, Rng, stamp};
use crate::trace::{FixtureFile, event_order};

/// Game ticks of margin the reported setup and hold keep from the capture boundary.
pub const MARGIN: i32 = 2;

/// One trial of a sweep: the data changes at `data_tick`, and Q is watched from
/// the active clock edge at `edge` to the next one at `next_edge`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trial {
    /// Ticks from the active edge to the data change (sweeps), or the clock
    /// high/low time under test (pulse fixtures).
    pub delta: i32,
    pub data_rises: bool,
    pub data_tick: u32,
    pub edge: u32,
    pub next_edge: u32,
}

/// A generated flip-flop fixture with what it tests.
pub struct DffFixture {
    pub name: String,
    pub json: Value,
    pub kind: FixtureKind,
    pub trials: Vec<Trial>,
    /// The Q reader cell, whose power says what the flip-flop holds.
    pub q: Pos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureKind {
    /// Data edge against the active clock edge, clock high `0` and low `1` ticks.
    Sweep(u32, u32),
    /// Clock high times under test, low fixed.
    High,
    /// Clock low times under test, high fixed.
    Low,
}

const FIRST_EDGE: u32 = 20;
/// Clock shapes swept: (high, low) game ticks, slowest first.
const SWEEP_CLOCKS: [(u32, u32); 3] = [(8, 8), (6, 6), (4, 4)];
/// Data offsets swept, relative to the active edge: from -6 to the smaller of
/// 12 and P - 4 for a clock of period P (a later change belongs to the next cycle).
const SWEEP_FROM: i32 = -6;
const SWEEP_TO: i32 = 12;
/// Pulse widths tried, against a 12-tick other half.
const PULSE_WIDTHS: [u32; 4] = [2, 4, 6, 8];
const PULSE_OTHER: u32 = 12;
/// Data is set this long before the active edge in the pulse fixtures.
const PULSE_LEAD: u32 = 6;

/// Whether the flip-flop acts on the rising or the falling clock edge, from the
/// tile's `edge` line.
fn falling_edge(variant: &Variant, library: &Library) -> bool {
    variant.tile(library).clock_edge.as_deref() == Some("falling")
}

/// The sweep and pulse fixtures of a flip-flop variant.
pub fn fixtures(variant: &Variant, library: &Library) -> Vec<DffFixture> {
    let falling = falling_edge(variant, library);
    let mut out = Vec::new();
    for (high, low) in SWEEP_CLOCKS {
        let period = (high + low) as i32;
        let deltas: Vec<i32> = (SWEEP_FROM..=SWEEP_TO.min(period - 4)).collect();
        out.push(build(variant, library, FixtureKind::Sweep(high, low), falling, &deltas));
    }
    out.push(build(variant, library, FixtureKind::High, falling, &[]));
    out.push(build(variant, library, FixtureKind::Low, falling, &[]));
    out
}

fn build(variant: &Variant, library: &Library, kind: FixtureKind, falling: bool, deltas: &[i32]) -> DffFixture {
    let base = variant.name();
    let (name, doc) = match kind {
        FixtureKind::Sweep(h, l) => (
            format!("{base}_sweep_h{h}_l{l}"),
            format!(
                "{base}: the data edge moved a tick at a time against the {} clock edge (clock {h} high, {l} low); no expectations, the game is compared with the simulator",
                if falling { "falling" } else { "rising" }
            ),
        ),
        FixtureKind::High => (format!("{base}_pulse_high"), format!("{base}: shorter and shorter clock high times")),
        FixtureKind::Low => (format!("{base}_pulse_low"), format!("{base}: shorter and shorter clock low times")),
    };
    let mut fx = Fixture::new(name.clone(), doc, crate::tiletest::box_for(variant, library));
    let hooks = stamp(&mut fx, variant, library);
    let d = hooks.iter().find(|h| h.port.edge == crate::tile::Edge::West).expect("D on the west").driver;
    let clk = hooks.iter().find(|h| h.port.edge == crate::tile::Edge::North).expect("clock on the north").driver;
    let q = hooks.iter().find(|h| h.port.kind == PortKind::Out).expect("Q").reader;
    let on = Block::RedstoneBlock;
    let off = Block::Air;
    let level = |b: bool| if b { on } else { off };

    let mut trials = Vec::new();
    let mut data = false;
    let mut t = FIRST_EDGE; // the next rising clock edge
    match kind {
        FixtureKind::Sweep(high, low) => {
            // Each delta twice: data rising, then falling. Two cycles per trial.
            for &delta in deltas {
                for _ in 0..2 {
                    let rise = t;
                    let fall = t + high;
                    let edge = if falling { fall } else { rise };
                    let next_rise = t + high + low;
                    let next_edge = if falling { next_rise + high } else { next_rise };
                    fx.at(rise, clk, on);
                    fx.at(fall, clk, off);
                    fx.at(next_rise, clk, on);
                    fx.at(next_rise + high, clk, off);
                    data = !data;
                    let data_tick = (edge as i32 + delta) as u32;
                    fx.at(data_tick, d, level(data));
                    trials.push(Trial { delta, data_rises: data, data_tick, edge, next_edge });
                    t = next_rise + high + low;
                }
            }
        }
        FixtureKind::High | FixtureKind::Low => {
            for &width in &PULSE_WIDTHS {
                let (high, low) = if kind == FixtureKind::High { (width, PULSE_OTHER) } else { (PULSE_OTHER, width) };
                let rise = t;
                let fall = t + high;
                let edge = if falling { fall } else { rise };
                data = !data;
                let data_tick = edge - PULSE_LEAD;
                fx.at(data_tick, d, level(data));
                fx.at(rise, clk, on);
                fx.at(fall, clk, off);
                // A second, generous cycle lets a missed value through, so a miss shows.
                let next_rise = fall + low;
                let next_edge = if falling { next_rise + PULSE_OTHER } else { next_rise };
                fx.at(next_rise, clk, on);
                fx.at(next_rise + PULSE_OTHER, clk, off);
                trials.push(Trial { delta: width as i32, data_rises: data, data_tick, edge, next_edge });
                t = next_rise + PULSE_OTHER + PULSE_OTHER;
            }
        }
    }
    // A clock too fast for the tile can burn a torch out, which relights 160
    // ticks later; the fixture must end quiet all the same.
    let quiet = match kind {
        FixtureKind::Sweep(high, low) if high + low <= 8 => 180,
        _ => 24,
    };
    let json = fx.finish(quiet);
    DffFixture { name, json, kind, trials, q }
}

/// What one replay showed: per trial, the ticks (relative to the active edge) at
/// which Q changed before the next active edge, and after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub before: Vec<i32>,
    pub after: Vec<i32>,
}

impl Outcome {
    pub fn captured(&self) -> bool {
        !self.before.is_empty()
    }
}

/// Replays a fixture in the simulator, with the ticks that fall due together run
/// in a random order (ignoring priorities) when `shuffle` is given, and reads Q.
pub fn replay(fixture: &DffFixture, shuffle: Option<u64>) -> Result<Vec<Outcome>, String> {
    let file: FixtureFile = serde_json::from_value(fixture.json.clone()).map_err(|e| format!("fixture: {e}"))?;
    let mut world = crate::world::World::new((file.size[0], file.size[1], file.size[2]));
    for (pos, state) in &file.blocks {
        world.set(pos.parse()?, state.parse()?);
    }
    let mut sim = Sim::new(world);
    sim.check_all_supported()?;
    sim.fix_wire_shapes();
    sim.poke_all();
    sim.settle(200)?;
    let mut events: BTreeMap<u32, Vec<(Pos, Block)>> = BTreeMap::new();
    for event in &file.events {
        for (pos, state) in &event.set {
            events.entry(event.tick).or_default().push((pos.parse()?, state.parse()?));
        }
    }
    for list in events.values_mut() {
        list.sort_by_key(|(pos, _)| event_order(*pos));
    }
    let mut rng = shuffle.map(Rng::new);
    let mut q_was = crate::trace::is_powered(sim.world().get(fixture.q));
    let mut changes: Vec<u32> = Vec::new();
    for tick in 1..=file.ticks {
        match &mut rng {
            Some(rng) => {
                let due: usize = sim.due_groups().iter().sum();
                let mut order: Vec<usize> = (0..due).collect();
                for i in (1..due).rev() {
                    order.swap(i, rng.below(i + 1));
                }
                sim.step_in_order(&order);
            }
            None => sim.step(),
        }
        for (pos, block) in events.get(&tick).into_iter().flatten() {
            sim.set_block(*pos, *block, false)?;
        }
        let q = crate::trace::is_powered(sim.world().get(fixture.q));
        if q != q_was {
            q_was = q;
            changes.push(tick);
        }
    }
    Ok(fixture
        .trials
        .iter()
        .map(|trial| Outcome {
            before: changes
                .iter()
                .filter(|&&c| c >= trial.edge && c < trial.next_edge)
                .map(|&c| c as i32 - trial.edge as i32)
                .collect(),
            after: changes
                .iter()
                .filter(|&&c| c >= trial.next_edge && c < 2 * trial.next_edge - trial.edge)
                .map(|&c| c as i32 - trial.edge as i32)
                .collect(),
        })
        .collect())
}

/// The characterization of one flip-flop variant.
#[derive(Clone, Debug, Default)]
pub struct Characterization {
    pub name: String,
    pub falling: bool,
    /// Per sweep fixture: the table, one line per trial.
    pub tables: Vec<String>,
    /// Latest data offset captured under every ordering; earliest offset ignored
    /// under every ordering (the capture boundary lies between them).
    pub captured_until: Option<i32>,
    pub ignored_from: Option<i32>,
    /// Clock to output with the data well ahead of the edge.
    pub clk_to_q: Option<i32>,
    pub min_high: Option<i32>,
    pub min_low: Option<i32>,
    /// The shortest clock period swept at which the flip-flop captures.
    pub min_period: Option<i32>,
    /// Offsets whose outcome depends on same-tick ordering.
    pub ordering_zone: Vec<i32>,
    /// Trials whose outcome depends on ordering, by fixture and trial description.
    pub ordering_elsewhere: Vec<String>,
    /// Limits worth knowing: clocks too fast to use, boundaries that move.
    pub findings: Vec<String>,
    /// Behaviour short of a flip-flop's (transparency): fatal for the chosen tile.
    pub defects: Vec<String>,
    pub problems: Vec<String>,
}

impl Characterization {
    /// The figures as a tile's `timing` line states them.
    pub fn timing(&self) -> BTreeMap<String, i32> {
        let mut out = BTreeMap::new();
        if let (Some(until), Some(from)) = (self.captured_until, self.ignored_from) {
            out.insert("setup".to_string(), MARGIN - until);
            out.insert("hold".to_string(), from + MARGIN);
        }
        if let Some(c) = self.clk_to_q {
            out.insert("clk_to_q".to_string(), c);
        }
        if let Some(h) = self.min_high {
            out.insert("min_high".to_string(), h);
        }
        if let Some(l) = self.min_low {
            out.insert("min_low".to_string(), l);
        }
        if let Some(p) = self.min_period {
            out.insert("min_period".to_string(), p);
        }
        out
    }
}

/// Characterizes a flip-flop variant: replays its fixtures, `shuffles` times
/// more under random same-tick orders. The slowest clock swept defines the
/// figures; each faster clock is usable only if it gives the same capture
/// boundary cleanly, and the fastest usable one sets the minimum period.
pub fn characterize(variant: &Variant, library: &Library, shuffles: u32) -> Result<Characterization, String> {
    let fixtures = fixtures(variant, library);
    let mut out =
        Characterization { name: variant.name(), falling: falling_edge(variant, library), ..Default::default() };
    for fixture in &fixtures {
        let base = replay(fixture, None)?;
        let mut shuffled: Vec<Vec<Outcome>> = Vec::new();
        for seed in 0..shuffles {
            shuffled.push(replay(fixture, Some(1000 + u64::from(seed)))?);
        }
        let differs = |i: usize| shuffled.iter().any(|s| s[i] != base[i]);
        match fixture.kind {
            FixtureKind::Sweep(high, low) => {
                let period = (high + low) as i32;
                let sweep = analyze_sweep(fixture, &base, &differs);
                out.tables.push(sweep.table);
                let reference = out.captured_until.is_none() && out.ignored_from.is_none() && out.clk_to_q.is_none();
                match sweep.boundary {
                    Some((u, f)) if reference => {
                        out.captured_until = Some(u);
                        out.ignored_from = Some(f);
                        out.clk_to_q = sweep.clk_to_q;
                        out.min_period = Some(period);
                        out.ordering_zone = sweep.zone.iter().copied().filter(|&d| d > u && d < f).collect();
                        out.ordering_elsewhere.extend(sweep.zone.iter().filter(|&&d| d <= u || d >= f).map(|d| format!("{}: offset {d}", fixture.name)));
                        out.defects.extend(sweep.findings);
                    }
                    Some((u, f)) if Some((u, f)) == out.captured_until.zip(out.ignored_from) && sweep.clk_to_q == out.clk_to_q => {
                        // Same behaviour at a faster clock: usable, unless ordering creeps in.
                        let elsewhere: Vec<i32> = sweep.zone.iter().copied().filter(|&d| d <= u || d >= f).collect();
                        if elsewhere.is_empty() && sweep.findings.is_empty() {
                            out.min_period = Some(period);
                        } else {
                            out.findings.push(format!("{}: not usable at a clock period of {period}: ordering-dependent at offsets {elsewhere:?}{}", fixture.name, sweep.findings.join("; ")));
                        }
                    }
                    Some((u, f)) => out.findings.push(format!("{}: at a clock period of {period} the capture boundary moves to between {u} and {f} (clock-to-output {:?})", fixture.name, sweep.clk_to_q)),
                    None if reference => out.problems.push(format!("{}: no clean capture boundary at the slowest clock swept", fixture.name)),
                    None => out.findings.push(format!("{}: not usable at a clock period of {period}: {}", fixture.name, sweep.why_not)),
                }
            }
            FixtureKind::High | FixtureKind::Low => {
                let mut lines = vec![format!("{}: clock width -> Q changes", fixture.name)];
                let mut min_width = None;
                for (i, trial) in fixture.trials.iter().enumerate() {
                    let o = &base[i];
                    let ok = o.captured() && o.before.len() == 1 && o.after.is_empty();
                    lines.push(format!(
                        "  {:>4}: {:<10} {:?}{}{}",
                        trial.delta,
                        if ok { "captured" } else { "missed" },
                        o.before,
                        if o.after.is_empty() { String::new() } else { format!(" then {:?}", o.after) },
                        if differs(i) { "  (depends on ordering)" } else { "" }
                    ));
                    if ok && min_width.is_none() {
                        min_width = Some(trial.delta);
                    } else if !ok && min_width.is_some() {
                        out.problems
                            .push(format!("{}: width {} fails after a shorter one worked", fixture.name, trial.delta));
                    }
                    // Below the minimum width anything goes; at or above it, ordering must not matter.
                    if differs(i) && min_width.is_some() {
                        out.ordering_elsewhere.push(format!("{}: width {}", fixture.name, trial.delta));
                    }
                }
                out.tables.push(lines.join("\n"));
                if fixture.kind == FixtureKind::High {
                    out.min_high = min_width;
                } else {
                    out.min_low = min_width;
                }
            }
        }
    }
    if out.clk_to_q.is_none() {
        out.problems.push("nothing was captured".to_string());
    }
    Ok(out)
}

struct SweepResult {
    table: String,
    boundary: Option<(i32, i32)>,
    clk_to_q: Option<i32>,
    zone: Vec<i32>,
    findings: Vec<String>,
    why_not: String,
}

/// Reads one sweep's outcomes: the capture boundary, the clock-to-output with
/// the data well ahead, the ordering-dependent offsets, and signs of transparency.
fn analyze_sweep(fixture: &DffFixture, base: &[Outcome], differs: &dyn Fn(usize) -> bool) -> SweepResult {
    let FixtureKind::Sweep(high, low) = fixture.kind else { unreachable!() };
    let mut lines = vec![format!("{} (clock {high} high, {low} low): data offset -> Q changes", fixture.name)];
    let mut by_delta: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (i, trial) in fixture.trials.iter().enumerate() {
        by_delta.entry(trial.delta).or_default().push(i);
    }
    let mut captured: Vec<(i32, i32)> = Vec::new(); // (offset, delay), clean ones
    let mut missed: Vec<i32> = Vec::new();
    let mut odd: Vec<i32> = Vec::new();
    let mut zone: Vec<i32> = Vec::new();
    for (&delta, trials) in &by_delta {
        let outcomes: Vec<&Outcome> = trials.iter().map(|&i| &base[i]).collect();
        let is_captured = outcomes.iter().all(|o| o.before.len() == 1 && o.after.is_empty());
        let is_missed = outcomes.iter().all(|o| o.before.is_empty() && o.after.len() == 1);
        let ordering = trials.iter().any(|&i| differs(i));
        let desc: Vec<String> = outcomes
            .iter()
            .map(|o| {
                format!(
                    "{:?}{}",
                    o.before,
                    if o.after.is_empty() { String::new() } else { format!(" then {:?}", o.after) }
                )
            })
            .collect();
        lines.push(format!(
            "  {delta:>4}: {:<10} {}{}",
            if is_captured {
                "captured"
            } else if is_missed {
                "missed"
            } else {
                "odd"
            },
            desc.join(" / "),
            if ordering { "  (depends on ordering)" } else { "" }
        ));
        if ordering {
            zone.push(delta);
        } else if is_captured {
            let delays: Vec<i32> = outcomes.iter().map(|o| o.before[0]).collect();
            if delays.iter().all(|&d| d == delays[0]) {
                captured.push((delta, delays[0]));
            } else {
                odd.push(delta);
            }
        } else if is_missed {
            missed.push(delta);
        } else {
            odd.push(delta);
        }
    }
    let table = lines.join("\n");
    let mut findings = Vec::new();
    // A clean boundary: every clean captured offset below every clean missed one.
    let until = captured.iter().map(|c| c.0).max();
    let from = missed.iter().copied().min();
    let clean = match (until, from) {
        (Some(u), Some(f)) => u < f && odd.is_empty() && zone.iter().all(|&d| d > u && d < f),
        _ => false,
    };
    if !clean {
        let why_not = format!(
            "odd offsets {odd:?}, ordering-dependent offsets {zone:?}, captured up to {until:?}, missed from {from:?}"
        );
        return SweepResult { table, boundary: None, clk_to_q: None, zone, findings, why_not };
    }
    let (u, f) = (until.expect("clean"), from.expect("clean"));
    let clk_to_q = captured[0].1;
    // Data arriving as the sampling window closes comes out when it arrives; the
    // last two captured offsets may do so. Earlier ones must come out at clock-to-output,
    // or the flip-flop is transparent for part of the cycle.
    for &(delta, delay) in &captured {
        if delay != clk_to_q && delta < u - 1 {
            findings.push(format!(
                "{}: transparent: data at offset {delta} comes out {delay} ticks after the edge, not {clk_to_q}",
                fixture.name
            ));
        }
    }
    SweepResult { table, boundary: Some((u, f)), clk_to_q: Some(clk_to_q), zone, findings, why_not: String::new() }
}

/// Checks a characterization against its tile's description and, for the chosen
/// flip-flop, against the margin rule.
pub fn check(c: &Characterization, variant: &Variant, library: &Library, chosen: bool, margin: i32) -> Vec<String> {
    let mut problems = c.problems.clone();
    if chosen {
        problems.extend(c.defects.iter().map(|f| format!("{}: {f}", c.name)));
    }
    let stated = &variant.tile(library).timing;
    let measured = c.timing();
    for (key, value) in &measured {
        match stated.get(key) {
            Some(s) if s == value => {}
            Some(s) => problems.push(format!("{}: the tile says {key} {s}, measured {value}", c.name)),
            None => problems.push(format!("{}: the tile does not state {key} (measured {value})", c.name)),
        }
    }
    for key in stated.keys() {
        if !measured.contains_key(key) {
            problems.push(format!("{}: the tile states {key}, which was not measured", c.name));
        }
    }
    if chosen {
        if !c.ordering_elsewhere.is_empty() {
            problems.push(format!(
                "{}: the outcome depends on same-tick ordering away from the capture boundary: {}",
                c.name,
                c.ordering_elsewhere.join(", ")
            ));
        }
        if let (Some(u), Some(f)) = (c.captured_until, c.ignored_from) {
            // The ordering zone is what lies between; setup and hold must keep `margin` from it.
            let zone_width = f - u - 1;
            if zone_width > 2 {
                problems.push(format!("{}: the capture boundary is {zone_width} ticks wide", c.name));
            }
            if MARGIN < margin {
                problems
                    .push(format!("{}: the reported figures keep {MARGIN} ticks of margin, {margin} asked", c.name));
            }
        }
    }
    problems
}
