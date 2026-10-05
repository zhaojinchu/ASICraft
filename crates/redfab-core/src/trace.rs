//! Fixture and trace files (see `docs/trace-format.md`), and replaying a
//! recorded trace in the simulator to see whether the two agree.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::block::Block;
use crate::geom::Pos;
use crate::sim::{Mutation, Pending, Sim};
use crate::world::World;

#[derive(Deserialize)]
pub struct FixtureFile {
    pub format: String,
    pub name: String,
    pub size: [i32; 3],
    #[serde(default)]
    pub events: Vec<EventFile>,
    pub ticks: u32,
}

#[derive(Deserialize)]
pub struct EventFile {
    pub tick: u32,
    pub set: BTreeMap<String, String>,
}

#[derive(Deserialize)]
pub struct TraceFile {
    pub format: String,
    pub fixture: String,
    pub size: [i32; 3],
    pub initial: BTreeMap<String, String>,
    #[serde(default)]
    pub pending0: Option<Vec<PendingFile>>,
    #[serde(default)]
    pub ticks: Vec<TickFile>,
}

#[derive(Deserialize)]
pub struct TickFile {
    pub tick: u32,
    #[serde(default)]
    pub changes: BTreeMap<String, String>,
    #[serde(default)]
    pub pending: Option<Vec<PendingFile>>,
}

#[derive(Deserialize, Clone)]
pub struct PendingFile {
    pub pos: String,
    pub block: String,
    pub delay: u64,
    pub priority: i8,
}

/// The result of replaying one recorded trace in the simulator.
#[derive(Debug, PartialEq, Eq)]
pub enum Replay {
    /// Every tick matched: block states and pending ticks. `reordered_ticks` counts
    /// the ticks where the simulator's default order of same-moment events gave a
    /// different result from the game, but another order reproduced the game
    /// exactly: either another order of the scheduled ticks due together, or
    /// another order of delivering one update to several neighbours. That marks a
    /// race in the circuit (the game does not record its order, and in experiments
    /// mode picks part of it at random), not a wrong rule.
    Match { reordered_ticks: u32 },
    /// The simulator disagreed with the recording; the text says where.
    Mismatch(String),
    /// The fixture uses a block the simulator does not model yet.
    Unsupported(String),
}

/// Starts the simulator from the trace's recorded tick-0 state, applies the
/// fixture's stimuli, and compares every tick with the recording.
pub fn replay(
    fixture_json: &str,
    trace_json: &str,
    mutation: Option<Mutation>,
) -> Result<Replay, String> {
    let fixture: FixtureFile =
        serde_json::from_str(fixture_json).map_err(|e| format!("fixture: {e}"))?;
    let trace: TraceFile = serde_json::from_str(trace_json).map_err(|e| format!("trace: {e}"))?;
    if fixture.format != "redfab-fixture/1" || trace.format != "redfab-trace/1" {
        return Err(format!(
            "unexpected formats {:?} / {:?}",
            fixture.format, trace.format
        ));
    }
    if fixture.name != trace.fixture || fixture.size != trace.size {
        return Err(format!(
            "trace is for {:?} {:?}, fixture is {:?} {:?}",
            trace.fixture, trace.size, fixture.name, fixture.size
        ));
    }

    // Parse everything up front so an unsupported block is reported as such.
    let mut world = World::new((fixture.size[0], fixture.size[1], fixture.size[2]));
    let mut expected: BTreeMap<Pos, String> = BTreeMap::new();
    for (pos, state) in &trace.initial {
        let pos: Pos = pos.parse()?;
        match state.parse::<Block>() {
            Ok(block) => world.set(pos, block),
            Err(e) if e.starts_with("unsupported block") => return Ok(Replay::Unsupported(e)),
            Err(e) => return Err(e),
        }
        expected.insert(pos, state.clone());
    }
    let mut events: BTreeMap<u32, Vec<(Pos, Block)>> = BTreeMap::new();
    for event in &fixture.events {
        for (pos, state) in &event.set {
            match state.parse::<Block>() {
                Ok(block) => events
                    .entry(event.tick)
                    .or_default()
                    .push((pos.parse()?, block)),
                Err(e) if e.starts_with("unsupported block") => return Ok(Replay::Unsupported(e)),
                Err(e) => return Err(e),
            }
        }
    }
    for list in events.values_mut() {
        list.sort_by_key(|(pos, _)| pos.y); // the harness places lower blocks first
    }

    let mut sim = Sim::new(world).with_mutation(mutation);
    if let Err(e) = sim.check_all_supported() {
        return Ok(Replay::Unsupported(e));
    }
    let compare_pending = trace.pending0.is_some();
    for p in trace.pending0.iter().flatten() {
        sim.seed_pending(p.pos.parse()?, p.delay, p.priority)?;
    }
    let by_tick: BTreeMap<u32, &TickFile> = trace.ticks.iter().map(|t| (t.tick, t)).collect();

    let no_events = Vec::new();
    let mut reordered_ticks = 0;
    for tick in 1..=fixture.ticks {
        let recorded = by_tick.get(&tick);
        for (pos, state) in recorded.iter().flat_map(|t| &t.changes) {
            let pos: Pos = pos.parse()?;
            if state == "minecraft:air" {
                expected.remove(&pos);
            } else {
                expected.insert(pos, state.clone());
            }
        }
        let mut want: Vec<Pending> = Vec::new();
        for p in recorded
            .and_then(|t| t.pending.as_ref())
            .into_iter()
            .flatten()
        {
            want.push(Pending {
                pos: p.pos.parse()?,
                block: p.block.clone(),
                delay: p.delay,
                priority: p.priority,
            });
        }
        want.sort();
        let tick_events = events.get(&tick).unwrap_or(&no_events);
        let check = |sim: &Sim| -> Option<String> {
            if let Some(diff) = first_block_difference(sim.world(), &expected) {
                return Some(diff);
            }
            let got = sim.pending();
            (compare_pending && got != want)
                .then(|| format!("pending ticks: game {want:?}, simulator {got:?}"))
        };

        // One attempt at this tick: a tick order (empty = default) and a delivery
        // shuffle (None = default). Returns the stepped simulator if it matches.
        let attempt = |before: &Sim,
                       order: &[usize],
                       delivery: Option<u64>|
         -> Result<Result<Sim, String>, String> {
            let mut candidate = before.clone();
            candidate.shuffle_delivery(delivery);
            candidate.step_in_order(order);
            for (pos, block) in tick_events {
                match candidate.set_block(*pos, *block, false) {
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }
            candidate.shuffle_delivery(None);
            Ok(match check(&candidate) {
                None => Ok(candidate),
                Some(diff) => Err(diff),
            })
        };

        let before = sim.clone();
        let default_diff = match attempt(&before, &[], None) {
            Ok(Ok(next)) => {
                sim = next;
                continue;
            }
            Ok(Err(diff)) => diff,
            Err(e) if e.starts_with("unsupported") => return Ok(Replay::Unsupported(e)),
            Err(e) => return Err(e),
        };

        // The default order disagrees. Does any other order of the same-moment
        // events reproduce the game? First other tick orders, then shuffled delivery.
        let tick_orders = orders_within_groups(&before.due_groups(), MAX_TICK_ORDERS);
        let mut found = None;
        'search: for delivery in std::iter::once(None).chain((1..=DELIVERY_SHUFFLES).map(Some)) {
            for (i, order) in tick_orders.iter().enumerate() {
                if delivery.is_none() && i == 0 {
                    continue; // the default attempt, already made
                }
                if let Ok(Ok(next)) = attempt(&before, order, delivery) {
                    found = Some(next);
                    break 'search;
                }
            }
        }
        match found {
            Some(next) => {
                sim = next;
                reordered_ticks += 1;
            }
            None => return Ok(Replay::Mismatch(format!("tick {tick}: {default_diff}"))),
        }
    }
    Ok(Replay::Match { reordered_ticks })
}

/// Limits on the search for an order that explains a disagreement.
const MAX_TICK_ORDERS: usize = 120;
const DELIVERY_SHUFFLES: u64 = 48;

/// Every way to order ticks that keeps each tick inside its group, as index
/// lists, starting with the identity. Stops after `limit` orders.
fn orders_within_groups(groups: &[usize], limit: usize) -> Vec<Vec<usize>> {
    let mut orders: Vec<Vec<usize>> = vec![Vec::new()];
    let mut start = 0;
    for &len in groups {
        let group: Vec<usize> = (start..start + len).collect();
        let perms = permutations(&group, limit);
        let mut next = Vec::new();
        'outer: for prefix in &orders {
            for perm in &perms {
                let mut order = prefix.clone();
                order.extend(perm);
                next.push(order);
                if next.len() >= limit {
                    break 'outer;
                }
            }
        }
        orders = next;
        start += len;
    }
    orders
}

/// Permutations of `items` in lexicographic order of positions (identity first).
fn permutations(items: &[usize], limit: usize) -> Vec<Vec<usize>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let first = rest.remove(i);
        for mut tail in permutations(&rest, limit) {
            let mut perm = vec![first];
            perm.append(&mut tail);
            out.push(perm);
            if out.len() >= limit {
                return out;
            }
        }
    }
    out
}

/// The tick-0 state of a recorded trace as a world.
pub fn initial_world(trace_json: &str) -> Result<World, String> {
    let trace: TraceFile = serde_json::from_str(trace_json).map_err(|e| format!("trace: {e}"))?;
    let mut world = World::new((trace.size[0], trace.size[1], trace.size[2]));
    for (pos, state) in &trace.initial {
        world.set(pos.parse()?, state.parse()?);
    }
    Ok(world)
}

/// A text account of a replay for debugging: the layers at tick 0 and, for each
/// tick up to `until`, the layers the game recorded beside the simulator's, with
/// both pending lists.
pub fn replay_report(fixture_json: &str, trace_json: &str, until: u32) -> Result<String, String> {
    let fixture: FixtureFile =
        serde_json::from_str(fixture_json).map_err(|e| format!("fixture: {e}"))?;
    let trace: TraceFile = serde_json::from_str(trace_json).map_err(|e| format!("trace: {e}"))?;
    let size = (fixture.size[0], fixture.size[1], fixture.size[2]);
    let mut game = World::new(size);
    for (pos, state) in &trace.initial {
        game.set(pos.parse()?, state.parse()?);
    }
    let mut sim = Sim::new(game.clone());
    for p in trace.pending0.iter().flatten() {
        sim.seed_pending(p.pos.parse()?, p.delay, p.priority)?;
    }
    let mut out = format!(
        "tick 0\n{}pending: {:?}\n",
        crate::view::layers(&game),
        sim.pending()
    );
    let by_tick: BTreeMap<u32, &TickFile> = trace.ticks.iter().map(|t| (t.tick, t)).collect();
    for tick in 1..=until.min(fixture.ticks) {
        sim.step();
        let mut applied = Vec::new();
        for event in fixture.events.iter().filter(|e| e.tick == tick) {
            let mut sets: Vec<(Pos, Block)> = Vec::new();
            for (pos, state) in &event.set {
                sets.push((pos.parse()?, state.parse()?));
            }
            sets.sort_by_key(|(pos, _)| pos.y);
            for (pos, block) in sets {
                sim.set_block(pos, block, false)?;
                applied.push(format!("{pos}={block}"));
            }
        }
        let recorded = by_tick.get(&tick);
        for (pos, state) in recorded.iter().flat_map(|t| &t.changes) {
            game.set(pos.parse()?, state.parse()?);
        }
        let game_pending: Vec<String> = recorded
            .and_then(|t| t.pending.as_ref())
            .into_iter()
            .flatten()
            .map(|p| {
                format!(
                    "{}@{}+{}p{}",
                    p.block.trim_start_matches("minecraft:"),
                    p.pos,
                    p.delay,
                    p.priority
                )
            })
            .collect();
        let sim_pending: Vec<String> = sim
            .pending()
            .iter()
            .map(|p| {
                format!(
                    "{}@{}+{}p{}",
                    p.block.trim_start_matches("minecraft:"),
                    p.pos,
                    p.delay,
                    p.priority
                )
            })
            .collect();
        let same = &game == sim.world();
        out.push_str(&format!(
            "\ntick {tick}  events: {applied:?}  {}\n",
            if same { "(same)" } else { "(DIFFERENT)" }
        ));
        out.push_str(&format!("game:\n{}", crate::view::layers(&game)));
        if !same {
            out.push_str(&format!("simulator:\n{}", crate::view::layers(sim.world())));
        }
        out.push_str(&format!(
            "game pending: {game_pending:?}\nsim pending:  {sim_pending:?}\n"
        ));
    }
    Ok(out)
}

fn first_block_difference(world: &World, expected: &BTreeMap<Pos, String>) -> Option<String> {
    let (sx, sy, sz) = world.size();
    let mut diffs = Vec::new();
    for y in 0..sy {
        for z in 0..sz {
            for x in 0..sx {
                let pos = Pos::new(x, y, z);
                let got = world.get(pos).to_string();
                let want = expected.get(&pos).map_or("minecraft:air", String::as_str);
                if got != want {
                    diffs.push(format!("{pos}: game {want}, simulator {got}"));
                }
            }
        }
    }
    if diffs.is_empty() {
        None
    } else {
        let shown = diffs.len().min(4);
        Some(format!(
            "{} block(s) differ; {}",
            diffs.len(),
            diffs[..shown].join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_start_with_identity_and_stay_inside_groups() {
        let orders = orders_within_groups(&[2, 1, 3], 1000);
        assert_eq!(orders[0], vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(orders.len(), 2 * 6); // 2! orders of the first group times 3! of the last
        for order in &orders {
            let mut sorted = order.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, vec![0, 1, 2, 3, 4, 5]);
            assert!(order[..2].iter().all(|&i| i < 2));
            assert_eq!(order[2], 2);
            assert!(order[3..].iter().all(|&i| i >= 3));
        }
    }
}
