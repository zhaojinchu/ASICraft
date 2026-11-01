//! Checks of the tile library itself: a fixture per tile variant with drivers and
//! readers on its ports, random tile groups for the game, and the static
//! adjacency check of the tile contract.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::block::Block;
use crate::extract::extract;
use crate::geom::Pos;
use crate::layout::{index_at, origins};
use crate::tile::{Edge, HEIGHT, Kind, Library, NARROW, PITCH, PortKind, SHALLOW, Variant, VariantPort};
use crate::world::World;

/// A fixture under construction, in the trace format's fixture layout.
pub(crate) struct Fixture {
    pub(crate) name: String,
    pub(crate) doc: String,
    pub(crate) size: [i32; 3],
    pub(crate) blocks: BTreeMap<String, String>,
    pub(crate) events: BTreeMap<u32, BTreeMap<String, String>>,
    pub(crate) expect: Vec<Value>,
    pub(crate) ticks: u32,
}

impl Fixture {
    pub(crate) fn new(name: String, doc: String, size: [i32; 3]) -> Fixture {
        Fixture { name, doc, size, blocks: BTreeMap::new(), events: BTreeMap::new(), expect: Vec::new(), ticks: 0 }
    }

    pub(crate) fn put(&mut self, pos: Pos, block: Block) {
        self.blocks.insert(pos.to_string(), block.to_string());
    }

    pub(crate) fn at(&mut self, tick: u32, pos: Pos, block: Block) {
        self.events.entry(tick).or_default().insert(pos.to_string(), block.to_string());
        self.ticks = self.ticks.max(tick);
    }

    pub(crate) fn powered(&mut self, tick: u32, pos: Pos, powered: bool) {
        self.expect.push(json!({"tick": tick, "pos": pos.to_string(), "powered": powered}));
        self.ticks = self.ticks.max(tick);
    }

    pub(crate) fn finish(mut self, quiet_after: u32) -> Value {
        self.ticks += quiet_after;
        let last_event = self.events.keys().max().copied().unwrap_or(0);
        self.expect.push(json!({"from": last_event + quiet_after, "stable": true}));
        json!({
            "format": "redfab-fixture/1",
            "name": self.name,
            "doc": self.doc,
            "size": self.size,
            "placement": "update",
            "blocks": self.blocks,
            "events": self.events.iter().map(|(t, set)| json!({"tick": t, "set": set})).collect::<Vec<_>>(),
            "ticks": self.ticks,
            "expect": self.expect,
        })
    }
}

/// Where a variant sits in its fixture, with room for a reader and a driver on every side.
pub(crate) const ORIGIN: Pos = Pos::new(6, 0, 6);

/// The fixture box around a variant: six blocks of room on every side.
pub(crate) fn box_for(variant: &Variant, library: &Library) -> [i32; 3] {
    [variant.cells(library) * PITCH + 12, HEIGHT, 17]
}

/// A port of the placed variant with its reader cell (just outside the tile) and
/// driver cell (one further out).
pub(crate) struct Hookup {
    pub(crate) port: VariantPort,
    pub(crate) reader: Pos,
    pub(crate) driver: Pos,
}

pub(crate) fn hookups(variant: &Variant, library: &Library) -> Vec<Hookup> {
    variant
        .ports(library)
        .into_iter()
        .map(|port| {
            let world = Pos::new(ORIGIN.x + port.pos.x, ORIGIN.y + port.pos.y, ORIGIN.z + port.pos.z);
            let reader = world.offset(port.edge.dir());
            Hookup { port, reader, driver: reader.offset(port.edge.dir()) }
        })
        .collect()
}

/// Stamps the variant and a reader dust outside each of its ports.
pub(crate) fn stamp(fixture: &mut Fixture, variant: &Variant, library: &Library) -> Vec<Hookup> {
    for (pos, block) in variant.blocks(library) {
        fixture.put(Pos::new(ORIGIN.x + pos.x, ORIGIN.y + pos.y, ORIGIN.z + pos.z), block);
    }
    let hooks = hookups(variant, library);
    for hook in &hooks {
        fixture.put(hook.reader.down(), Block::Glass);
        fixture.put(hook.reader, Block::Wire(crate::block::Wire::DOT));
    }
    hooks
}

/// The fixture that checks one variant against its description.
pub fn variant_fixture(variant: &Variant, library: &Library) -> Value {
    let name = variant.name();
    let doc = match variant.kind {
        Kind::Wire
        | Kind::WireUp
        | Kind::WireN
        | Kind::WireUpN
        | Kind::WireS
        | Kind::WireUpS
        | Kind::WireSq
        | Kind::WireUpSq
            if variant.repeater_entry.is_some() =>
        {
            "Wire with a repeater: every other port follows the entry after 2 game ticks per setting, and not before."
        }
        Kind::Wire
        | Kind::WireUp
        | Kind::WireN
        | Kind::WireUpN
        | Kind::WireS
        | Kind::WireUpS
        | Kind::WireSq
        | Kind::WireUpSq => "Wiring: driving any used port must power every other used port, at once.",
        Kind::Ramp | Kind::RampN | Kind::RampNsN | Kind::RampS | Kind::RampNsS | Kind::RampSq | Kind::RampNsSq => {
            "Ramp: driving either port powers the other, at once, from one wire level to the other."
        }
        Kind::Nor | Kind::NorS => {
            "NOR: the output is powered exactly when no input is, 2 game ticks after the inputs change."
        }
        Kind::Cross | Kind::CrossN | Kind::CrossS if variant.repeater_entry.is_some() => {
            "Crossing with a repeater: the flat axis follows its entry after the delay; the other axis passes at once."
        }
        Kind::Cross | Kind::CrossN | Kind::CrossS => {
            "Crossing: west drives east only, north drives south only, at once."
        }
        Kind::TieHigh | Kind::TieHighS | Kind::TieHighN | Kind::TieHighSq => {
            "Constant 1: the output is always powered."
        }
        Kind::TieLow | Kind::TieLowS | Kind::TieLowN | Kind::TieLowSq => "Constant 0: the output is never powered.",
        Kind::Input | Kind::InputS | Kind::InputN | Kind::InputSq => {
            "Input: the output is powered while the probe block is a redstone block."
        }
        Kind::Output | Kind::OutputS | Kind::OutputN | Kind::OutputSq => {
            "Output: the lamp lights at once and goes out 4 game ticks after the input drops."
        }
        Kind::Dff | Kind::DffAlt => unreachable!("flip-flops have sweep fixtures, see dfftest"),
    };
    let mut fx = Fixture::new(name, doc.to_string(), box_for(variant, library));
    let hooks = stamp(&mut fx, variant, library);
    let on = Block::RedstoneBlock;
    let off = Block::Air;
    let delay = variant.delay_ticks(library);
    let mut t = 2;
    match variant.kind {
        Kind::Wire
        | Kind::WireUp
        | Kind::WireN
        | Kind::WireUpN
        | Kind::WireS
        | Kind::WireUpS
        | Kind::WireSq
        | Kind::WireUpSq
            if variant.repeater_entry.is_some() =>
        {
            let input = hooks.iter().find(|h| h.port.kind == PortKind::In).expect("repeater input");
            let outputs: Vec<&Hookup> = hooks.iter().filter(|h| h.port.kind == PortKind::Out).collect();
            for level in [true, false] {
                fx.at(t, input.driver, if level { on } else { off });
                for output in &outputs {
                    fx.powered(t + delay - 1, output.reader, !level);
                    fx.powered(t + delay, output.reader, level);
                }
                t += delay + 3;
            }
        }
        Kind::Wire
        | Kind::WireUp
        | Kind::Ramp
        | Kind::WireN
        | Kind::WireUpN
        | Kind::RampN
        | Kind::RampNsN
        | Kind::WireS
        | Kind::WireUpS
        | Kind::RampS
        | Kind::RampNsS
        | Kind::WireSq
        | Kind::WireUpSq
        | Kind::RampSq
        | Kind::RampNsSq => {
            for driven in &hooks {
                fx.at(t, driven.driver, on);
                for other in hooks.iter().filter(|h| h.port.edge != driven.port.edge) {
                    fx.powered(t, other.reader, true);
                }
                fx.at(t + 3, driven.driver, off);
                for hook in &hooks {
                    fx.powered(t + 3, hook.reader, false);
                }
                t += 6;
            }
        }
        Kind::Cross | Kind::CrossN | Kind::CrossS if variant.repeater_entry.is_some() => {
            let input = hooks.iter().find(|h| h.port.kind == PortKind::In).expect("entry");
            let output = hooks.iter().find(|h| h.port.kind == PortKind::Out).expect("exit");
            let others: Vec<&Hookup> = hooks.iter().filter(|h| h.port.kind == PortKind::Io).collect();
            for level in [true, false] {
                fx.at(t, input.driver, if level { on } else { off });
                fx.at(t, others[0].driver, if level { on } else { off });
                fx.powered(t + delay - 1, output.reader, !level);
                fx.powered(t + delay, output.reader, level);
                fx.powered(t, others[1].reader, level);
                t += delay + 3;
            }
        }
        Kind::Cross | Kind::CrossN | Kind::CrossS => {
            let hook = |edge: Edge| hooks.iter().find(|h| h.port.edge == edge).expect("cross has all ports");
            let (w, e, n, s) = (hook(Edge::West), hook(Edge::East), hook(Edge::North), hook(Edge::South));
            for (west_on, north_on) in [(true, false), (false, true), (true, true), (false, false)] {
                fx.at(t, w.driver, if west_on { on } else { off });
                fx.at(t, n.driver, if north_on { on } else { off });
                fx.powered(t, e.reader, west_on);
                fx.powered(t, s.reader, north_on);
                if !west_on {
                    fx.powered(t, w.reader, false);
                }
                if !north_on {
                    fx.powered(t, n.reader, false);
                }
                t += 4;
            }
        }
        Kind::Nor | Kind::NorS => {
            let inputs: Vec<&Hookup> = hooks.iter().filter(|h| h.port.kind == PortKind::In).collect();
            let output = hooks.iter().find(|h| h.port.kind == PortKind::Out).expect("nor has an output");
            let mut was_on = true; // all inputs off at the start: the output is on
            for mask in 1u32..(2 << inputs.len()) {
                let mask = mask % (1 << inputs.len()); // ends with every input off again
                for (i, input) in inputs.iter().enumerate() {
                    fx.at(t, input.driver, if mask & (1 << i) != 0 { on } else { off });
                }
                let now_on = mask == 0;
                fx.powered(t + delay - 1, output.reader, was_on);
                fx.powered(t + delay, output.reader, now_on);
                was_on = now_on;
                t += delay + 3;
            }
        }
        Kind::TieHigh
        | Kind::TieLow
        | Kind::TieHighS
        | Kind::TieLowS
        | Kind::TieHighN
        | Kind::TieLowN
        | Kind::TieHighSq
        | Kind::TieLowSq => {
            let output = &hooks[0];
            let high = matches!(variant.kind, Kind::TieHigh | Kind::TieHighS | Kind::TieHighN | Kind::TieHighSq);
            fx.powered(0, output.reader, high);
            fx.powered(6, output.reader, high);
        }
        Kind::Dff | Kind::DffAlt => unreachable!(),
        Kind::Input | Kind::InputS | Kind::InputN | Kind::InputSq => {
            let probe = variant.tile(library).probe.expect("input tile has a probe");
            let probe = Pos::new(ORIGIN.x + probe.x, ORIGIN.y + probe.y, ORIGIN.z + probe.z);
            let output = &hooks[0];
            fx.at(2, probe, Block::RedstoneBlock);
            fx.powered(2, output.reader, true);
            fx.at(6, probe, Block::Stone);
            fx.powered(6, output.reader, false);
        }
        Kind::Output | Kind::OutputS | Kind::OutputN | Kind::OutputSq => {
            let lamp = variant.tile(library).lamp.expect("output tile has a lamp");
            let lamp = Pos::new(ORIGIN.x + lamp.x, ORIGIN.y + lamp.y, ORIGIN.z + lamp.z);
            let input = &hooks[0];
            fx.at(2, input.driver, on);
            fx.powered(2, lamp, true);
            fx.at(6, input.driver, off);
            fx.powered(9, lamp, true);
            fx.powered(10, lamp, false);
        }
    }
    fx.finish(delay + 4)
}

/// The fixture that checks a stack, a lower and an upper wire in one tile: each
/// level does what it does alone (wiring powers its other ports at once; with a
/// repeater the ports beyond follow the entry after the delay), and driving one
/// level never powers the other.
pub fn stack_fixture(lower: &Variant, upper: &Variant, library: &Library) -> Value {
    let name = format!("stack_{}_over_{}", upper.name(), lower.name());
    let doc = "Stack: each wire level does what it does alone, and driving one never powers the other.";
    let mut fx = Fixture::new(name, doc.to_string(), box_for(lower, library));
    let low = stamp(&mut fx, lower, library);
    let high = stamp(&mut fx, upper, library);
    let on = Block::RedstoneBlock;
    let off = Block::Air;
    let mut t = 2;
    let mut longest = 0;
    for (variant, hooks, other) in [(lower, &low, &high), (upper, &high, &low)] {
        let delay = variant.delay_ticks(library);
        longest = longest.max(delay);
        if variant.repeater_entry.is_some() {
            let input = hooks.iter().find(|h| h.port.kind == PortKind::In).expect("repeater input");
            let outputs: Vec<&Hookup> = hooks.iter().filter(|h| h.port.kind == PortKind::Out).collect();
            for level in [true, false] {
                fx.at(t, input.driver, if level { on } else { off });
                for output in &outputs {
                    fx.powered(t + delay - 1, output.reader, !level);
                    fx.powered(t + delay, output.reader, level);
                }
                for quiet in other.iter() {
                    fx.powered(t + delay, quiet.reader, false);
                }
                t += delay + 3;
            }
        } else {
            for driven in hooks.iter() {
                fx.at(t, driven.driver, on);
                for same in hooks.iter().filter(|h| h.port.edge != driven.port.edge) {
                    fx.powered(t, same.reader, true);
                }
                for quiet in other.iter() {
                    fx.powered(t, quiet.reader, false);
                }
                fx.at(t + 3, driven.driver, off);
                for hook in hooks.iter() {
                    fx.powered(t + 3, hook.reader, false);
                }
                t += 6;
            }
        }
    }
    fx.finish(longest + 4)
}

/// The stacks the game checks: every lower wiring shape under every upper one,
/// and a straight with a repeater on either level under or over every shape of
/// the other; and in the narrow family, which differs only in being three
/// blocks across, every shape under and over a straight that crosses it, the
/// two repeater straights stacked, and each turn from the west or east, whose
/// repeater stands in the arm it leaves by, under and over the straight along
/// that arm; the same in the shallow family, three blocks deep, with west-east
/// and north-south swapped; and three by three, every shape under and over a
/// straight, and the two repeater straights stacked.
pub fn checked_stacks() -> Vec<(Variant, Variant)> {
    let shapes: Vec<Vec<Edge>> = (1u8..16)
        .map(|mask| Edge::ALL.into_iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, e)| e).collect())
        .filter(|edges: &Vec<Edge>| edges.len() >= 2)
        .collect();
    let straight = [Edge::West, Edge::East];
    let mut out = Vec::new();
    for lower in &shapes {
        for upper in &shapes {
            out.push((Variant::new(Kind::Wire, lower), Variant::new(Kind::WireUp, upper)));
        }
    }
    for shape in &shapes {
        out.push((Variant::repeater(&straight, Edge::West, 1), Variant::new(Kind::WireUp, shape)));
        out.push((
            Variant::new(Kind::Wire, shape),
            Variant { kind: Kind::WireUp, ..Variant::repeater(&straight, Edge::West, 1) },
        ));
    }
    let along = [Edge::North, Edge::South];
    for shape in &shapes {
        out.push((Variant::new(Kind::WireN, shape), Variant::new(Kind::WireUpN, &along)));
        out.push((Variant::new(Kind::WireN, &along), Variant::new(Kind::WireUpN, shape)));
    }
    let n_repeater = |kind: Kind, edges: &[Edge], entry: Edge| Variant { kind, ..Variant::repeater(edges, entry, 1) };
    out.push((n_repeater(Kind::WireN, &straight, Edge::West), n_repeater(Kind::WireUpN, &along, Edge::North)));
    out.push((n_repeater(Kind::WireN, &along, Edge::South), n_repeater(Kind::WireUpN, &straight, Edge::East)));
    for entry in [Edge::West, Edge::East] {
        for exit in [Edge::North, Edge::South] {
            let turn = [entry, exit];
            out.push((n_repeater(Kind::WireN, &turn, entry), Variant::new(Kind::WireUpN, &along)));
            out.push((Variant::new(Kind::WireN, &along), n_repeater(Kind::WireUpN, &turn, entry)));
        }
    }
    for shape in &shapes {
        out.push((Variant::new(Kind::WireS, shape), Variant::new(Kind::WireUpS, &straight)));
        out.push((Variant::new(Kind::WireS, &straight), Variant::new(Kind::WireUpS, shape)));
    }
    out.push((n_repeater(Kind::WireS, &along, Edge::North), n_repeater(Kind::WireUpS, &straight, Edge::West)));
    out.push((n_repeater(Kind::WireS, &straight, Edge::East), n_repeater(Kind::WireUpS, &along, Edge::South)));
    for entry in [Edge::North, Edge::South] {
        for exit in [Edge::West, Edge::East] {
            let turn = [exit, entry];
            out.push((n_repeater(Kind::WireS, &turn, entry), Variant::new(Kind::WireUpS, &straight)));
            out.push((Variant::new(Kind::WireS, &straight), n_repeater(Kind::WireUpS, &turn, entry)));
        }
    }
    for shape in &shapes {
        out.push((Variant::new(Kind::WireSq, shape), Variant::new(Kind::WireUpSq, &straight)));
        out.push((Variant::new(Kind::WireSq, &along), Variant::new(Kind::WireUpSq, shape)));
    }
    out.push((n_repeater(Kind::WireSq, &straight, Edge::West), n_repeater(Kind::WireUpSq, &along, Edge::North)));
    out.push((n_repeater(Kind::WireSq, &along, Edge::South), n_repeater(Kind::WireUpSq, &straight, Edge::East)));
    out
}

/// Fixtures for every variant, and for the checked stacks, by name.
pub fn all_variant_fixtures(library: &Library) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for v in Variant::all() {
        if matches!(v.kind, Kind::Dff | Kind::DffAlt) {
            out.extend(crate::dfftest::fixtures(&v, library).into_iter().map(|f| (f.name.clone(), f.json)));
        } else {
            out.push((v.name(), variant_fixture(&v, library)));
        }
    }
    for (lower, upper) in checked_stacks() {
        let fixture = stack_fixture(&lower, &upper, library);
        out.push((fixture["name"].as_str().expect("a fixture has a name").to_string(), fixture));
    }
    out
}

/// A small deterministic random source (xorshift), so fixtures are reproducible.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn draw(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.draw() % n as u64) as usize
    }
}

/// A random group of tiles on a `cols` × `rows` grid, with reader dust outside
/// every open port and random redstone blocks driving some of them over time.
/// There are no functional expectations: the game recording is compared with
/// the simulator instead.
pub fn group_fixture(name: &str, cols: i32, rows: i32, rng: &mut Rng, library: &Library) -> Value {
    let variants = Variant::all();
    // Each column narrow or not, the same all the way down, and each row shallow
    // or not, the same all the way across, as in a layout.
    let narrow_col: Vec<bool> = (0..cols).map(|_| rng.below(3) == 0).collect();
    let shallow_row: Vec<bool> = (0..rows).map(|_| rng.below(2) == 0).collect();
    let wide = !narrow_col.contains(&true);
    let grid: Vec<Vec<Vec<Variant>>> = (0..rows as usize)
        .map(|r| {
            (0..cols as usize)
                .map(|c| random_cell(&variants, (narrow_col[c], shallow_row[r]), wide, rng, library))
                .collect()
        })
        .collect();
    let placed = lay_out_stacked(&grid, library);
    let width = placed.iter().map(|(v, (c, _))| c + v.cells(library)).max().unwrap_or(0);
    let (xs, zs) = grid_origins(&placed, library);
    let margin = 2;
    let size = [xs[width as usize] + 2 * margin, HEIGHT, zs[rows as usize] + 2 * margin];
    let mut fx = Fixture::new(name.to_string(), format!("{cols}x{rows} random tiles"), size);
    let mut drivers = Vec::new();
    let covered = |cell: (i32, i32)| {
        placed.iter().any(|(v, (c, r))| *r == cell.1 && (*c..c + v.cells(library)).contains(&cell.0))
    };
    for (variant, (c, r)) in &placed {
        let origin = Pos::new(margin + xs[*c as usize], 0, margin + zs[*r as usize]);
        for (pos, block) in variant.blocks(library) {
            fx.put(Pos::new(origin.x + pos.x, pos.y, origin.z + pos.z), block);
        }
        for port in variant.ports(library) {
            let (dc, dr) = port.edge.step();
            // The cell beyond the port: on a wide tile the east port is on its last cell.
            let port_cell = (c + index_at(&xs, port.pos.x).expect("a port is inside its tile"), *r);
            if covered((port_cell.0 + dc, port_cell.1 + dr)) {
                continue; // whatever is there is part of the test
            }
            let cell = Pos::new(origin.x + port.pos.x, port.pos.y, origin.z + port.pos.z).offset(port.edge.dir());
            fx.put(cell.down(), Block::Glass);
            fx.put(cell, Block::Wire(crate::block::Wire::DOT));
            if port.kind != PortKind::Out {
                drivers.push(cell.offset(port.edge.dir()));
            }
        }
        if let Some(probe) = variant.tile(library).probe {
            drivers.push(Pos::new(origin.x + probe.x, probe.y, origin.z + probe.z));
        }
    }
    let mut t = 2;
    let mut state: BTreeMap<Pos, bool> = BTreeMap::new();
    for _ in 0..8 {
        if drivers.is_empty() {
            break;
        }
        let driver = drivers[rng.below(drivers.len())];
        let now = !state.get(&driver).copied().unwrap_or(false);
        state.insert(driver, now);
        let is_probe = fx.blocks.get(&driver.to_string()).is_some_and(|s| s == "minecraft:stone");
        let block = match (now, is_probe) {
            (true, _) => Block::RedstoneBlock,
            (false, true) => Block::Stone,
            (false, false) => Block::Air,
        };
        fx.at(t, driver, block);
        t += 1 + rng.below(6) as u32;
    }
    // A short pulse into a repeater comes out stretched to its delay, so a chain of
    // them can take the sum of twice each delay to settle.
    let stretch: u32 = grid.iter().flatten().flatten().map(|v| 2 * v.delay_ticks(library)).sum();
    fx.finish(12 + stretch)
}

/// Where the cells of a grid of placed fixture tiles start in blocks: each
/// column as wide as the tiles in it (a narrow tile's `NARROW`, every other
/// `PITCH`), each row as deep as the tiles in it (a shallow tile's `SHALLOW`,
/// every other `PITCH`), as the layout's own table says it (`layout::origins`).
/// Tiles above one another share their column's width, tiles beside one
/// another their row's depth.
fn grid_origins(placed: &[(Variant, (i32, i32))], library: &Library) -> (Vec<i32>, Vec<i32>) {
    let cols = placed.iter().map(|(v, (c, _))| c + v.cells(library)).max().unwrap_or(0);
    let rows = placed.iter().map(|(_, (_, r))| r + 1).max().unwrap_or(0);
    let mut widths = vec![PITCH; cols as usize];
    let mut depths = vec![PITCH; rows as usize];
    for (v, (c, r)) in placed {
        let tile = v.tile(library);
        if tile.width < PITCH {
            widths[*c as usize] = tile.width;
        }
        depths[*r as usize] = depths[*r as usize].min(tile.depth);
    }
    (origins(&widths), origins(&depths))
}

/// Whether a variant's tile is narrow, for a routing column.
fn narrow(variant: &Variant, library: &Library) -> bool {
    variant.tile(library).width < PITCH
}

/// Whether a variant's tile is shallow, for a row no flip-flop sits in.
fn shallow(variant: &Variant, library: &Library) -> bool {
    variant.tile(library).depth < PITCH
}

/// Each variant of a grid with the cell it starts at: a row's tiles follow one
/// another, each as wide as it is.
pub fn lay_out(grid: &[Vec<Variant>], library: &Library) -> Vec<(Variant, (i32, i32))> {
    let mut placed = Vec::new();
    for (r, row) in grid.iter().enumerate() {
        let mut c = 0;
        for variant in row {
            placed.push((variant.clone(), (c, r as i32)));
            c += variant.cells(library);
        }
    }
    placed
}

/// [`lay_out`] for a grid whose cells may hold a stack: the variants of one cell
/// (a lower and an upper wire) are all placed at it.
pub fn lay_out_stacked(grid: &[Vec<Vec<Variant>>], library: &Library) -> Vec<(Variant, (i32, i32))> {
    let mut placed = Vec::new();
    for (r, row) in grid.iter().enumerate() {
        let mut c = 0;
        for cell in row {
            for variant in cell {
                placed.push((variant.clone(), (c, r as i32)));
            }
            c += cell.iter().map(|v| v.cells(library)).max().unwrap_or(1);
        }
    }
    placed
}

/// Problems with a grid of placed variants: any way two tiles affect each other
/// other than through matched ports on a shared edge.
pub fn adjacency_problems(grid: &[Vec<Variant>], library: &Library) -> Vec<String> {
    let stacked: Vec<Vec<Vec<Variant>>> =
        grid.iter().map(|row| row.iter().map(|v| vec![v.clone()]).collect()).collect();
    adjacency_problems_stacked(&stacked, library)
}

/// [`adjacency_problems`] for a grid whose cells may hold a stack.
pub fn adjacency_problems_stacked(grid: &[Vec<Vec<Variant>>], library: &Library) -> Vec<String> {
    let placed = lay_out_stacked(grid, library);
    let rows = grid.len() as i32;
    let cols = placed.iter().map(|(v, (c, _))| c + v.cells(library)).max().unwrap_or(0);
    let (xs, zs) = grid_origins(&placed, library);
    let mut world = World::new((xs[cols as usize], HEIGHT, zs[rows as usize]));
    let mut probes = Vec::new();
    for (variant, (c, r)) in &placed {
        let origin = Pos::new(xs[*c as usize], 0, zs[*r as usize]);
        for (pos, block) in variant.blocks(library) {
            world.set(Pos::new(origin.x + pos.x, pos.y, origin.z + pos.z), block);
        }
        if let Some(probe) = variant.tile(library).probe {
            probes.push(Pos::new(origin.x + probe.x, probe.y, origin.z + probe.z));
        }
    }
    problems_in(&world, &placed, &probes, library)
}

/// The check behind [`adjacency_problems`], on a world that may have been tampered with.
pub fn problems_in(world: &World, placed: &[(Variant, (i32, i32))], probes: &[Pos], library: &Library) -> Vec<String> {
    let circuit = extract(world, probes);
    // Which placed tile a position belongs to, by index: its cell by the grid's
    // origin table, then the tile covering that cell.
    let (xs, zs) = grid_origins(placed, library);
    // The variants of a stack share a cell; the first of them stands for the tile.
    let tile_of = |pos: Pos| -> usize {
        let (Some(c), Some(r)) = (index_at(&xs, pos.x), index_at(&zs, pos.z)) else { return usize::MAX };
        placed
            .iter()
            .position(|(v, (vc, vr))| *vr == r && (*vc..vc + v.cells(library)).contains(&c))
            .unwrap_or(usize::MAX)
    };
    let name_of = |i: usize| match placed.get(i) {
        Some((v, (c, r))) => format!("{} at column {c}, row {r}", v.name()),
        None => "no tile".to_string(),
    };
    // The port cells of tile `i` on `edge`, one per wire level it uses there.
    let port_cells = |i: usize, edge: Edge| -> Vec<Pos> {
        let (_, at) = placed[i];
        placed
            .iter()
            .filter(|(_, other)| *other == at)
            .flat_map(|(v, (c, r))| {
                v.ports(library)
                    .into_iter()
                    .filter(|p| p.edge == edge)
                    .map(|p| Pos::new(xs[*c as usize] + p.pos.x, p.pos.y, zs[*r as usize] + p.pos.z))
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let mut problems = Vec::new();

    for net in &circuit.nets {
        let mut touched: BTreeSet<usize> = net.iter().map(|&p| tile_of(p)).collect();
        let net_id = circuit.net_of(net[0]).expect("a net contains its own wires");
        for component in circuit.torches.iter().chain(&circuit.lamps).chain(&circuit.sources) {
            if component.reads.contains(&net_id) || component.drives.contains(&net_id) {
                touched.insert(tile_of(component.pos));
            }
        }
        if touched.len() < 2 {
            continue;
        }
        // Tiles sharing this net must form one group joined only through matched
        // ports: a's port cell and b's opposite port cell are neighbours across the border.
        let tiles: Vec<usize> = touched.iter().copied().collect();
        let joined = |a: usize, b: usize| -> bool {
            a != usize::MAX
                && b != usize::MAX
                && Edge::ALL.iter().any(|&edge| {
                    let (dx, dz) = edge.step();
                    let theirs = port_cells(b, edge.opposite());
                    port_cells(a, edge).into_iter().any(|pa| {
                        let pb = Pos::new(pa.x + dx, pa.y, pa.z + dz);
                        theirs.contains(&pb) && net.contains(&pa) && net.contains(&pb)
                    })
                })
        };
        let mut reached = vec![tiles[0]];
        let mut frontier = vec![tiles[0]];
        while let Some(a) = frontier.pop() {
            for &b in &tiles {
                if !reached.contains(&b) && joined(a, b) {
                    reached.push(b);
                    frontier.push(b);
                }
            }
        }
        for &tile in &tiles {
            if !reached.contains(&tile) {
                problems.push(format!(
                    "the net containing the wire at {} reaches {} other than through matched ports",
                    net[0],
                    name_of(tile)
                ));
            }
        }
    }
    // The levels of a stack are two nets: none may reach a port of both.
    for (i, (variant, at)) in placed.iter().enumerate() {
        for (other, other_at) in placed.iter().skip(i + 1) {
            if other_at != at {
                continue;
            }
            let nets = |v: &Variant| -> BTreeSet<usize> {
                v.ports(library)
                    .iter()
                    .filter_map(|p| {
                        circuit.net_of(Pos::new(xs[at.0 as usize] + p.pos.x, p.pos.y, zs[at.1 as usize] + p.pos.z))
                    })
                    .collect()
            };
            if !nets(variant).is_disjoint(&nets(other)) {
                problems.push(format!(
                    "a net joins the two wire levels of the stack at column {}, row {} ({} under {})",
                    at.0,
                    at.1,
                    variant.name(),
                    other.name()
                ));
            }
        }
    }
    for component in circuit.torches.iter().chain(&circuit.sources) {
        for &other in &component.direct {
            if tile_of(other) != tile_of(component.pos) {
                problems.push(format!(
                    "{} at {} directly powers the block at {} in {}",
                    world.get(component.pos).name(),
                    component.pos,
                    other,
                    name_of(tile_of(other))
                ));
            }
        }
    }
    problems
}

/// Problems with a stack alone: a lower and an upper wire in one tile must stay
/// apart whatever repeaters they carry, so no net, and no chain of nets joined
/// by repeaters, may reach a port of both levels.
pub fn stack_problems(lower: &Variant, upper: &Variant, library: &Library) -> Vec<String> {
    let mut world = World::new((PITCH, HEIGHT, PITCH));
    for (pos, block) in lower.blocks(library).into_iter().chain(upper.blocks(library)) {
        world.set(pos, block);
    }
    stack_problems_in(&world, lower, upper, library)
}

/// The check behind [`stack_problems`], on a world that may have been tampered with.
pub fn stack_problems_in(world: &World, lower: &Variant, upper: &Variant, library: &Library) -> Vec<String> {
    let circuit = extract(world, &[]);
    // Nets joined through a repeater count as one circuit.
    let mut parent: Vec<usize> = (0..circuit.nets.len()).collect();
    fn root(parent: &mut [usize], mut n: usize) -> usize {
        while parent[n] != n {
            parent[n] = parent[parent[n]];
            n = parent[n];
        }
        n
    }
    for repeater in &circuit.repeaters {
        for &a in &repeater.reads {
            for &b in &repeater.drives {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra] = rb;
            }
        }
    }
    let mut circuits = |variant: &Variant| -> BTreeSet<usize> {
        variant.ports(library).iter().filter_map(|p| circuit.net_of(p.pos)).map(|n| root(&mut parent, n)).collect()
    };
    let (low, high) = (circuits(lower), circuits(upper));
    if low.is_disjoint(&high) {
        Vec::new()
    } else {
        vec![format!("{} under {} join the two wire levels", lower.name(), upper.name())]
    }
}

/// A random stack: a lower wire under an upper one, either with or without a
/// repeater, narrow or not, shallow or not.
fn random_stack(variants: &[Variant], narrow: bool, shallow: bool, rng: &mut Rng) -> Vec<Variant> {
    let width = if narrow { NARROW } else { PITCH };
    let depth = if shallow { SHALLOW } else { PITCH };
    let sized = |kind: Kind| kind.sized(width, depth).expect("wiring comes in every size");
    let (low, high) = (sized(Kind::Wire), sized(Kind::WireUp));
    let lowers: Vec<&Variant> = variants.iter().filter(|v| v.kind == low).collect();
    let uppers: Vec<&Variant> = variants.iter().filter(|v| v.kind == high).collect();
    vec![lowers[rng.below(lowers.len())].clone(), uppers[rng.below(uppers.len())].clone()]
}

/// A random cell of a grid: a stack a quarter of the time, else any variant;
/// narrow ones in a narrow column, shallow ones in a shallow row, and a two-cell
/// tile only when `wide` (a grid with no narrow column, where it cannot push a
/// row out of line).
fn random_cell(
    variants: &[Variant],
    (narrow_col, shallow_row): (bool, bool),
    wide: bool,
    rng: &mut Rng,
    library: &Library,
) -> Vec<Variant> {
    if rng.below(4) == 0 {
        return random_stack(variants, narrow_col, shallow_row, rng);
    }
    let fit: Vec<&Variant> = variants
        .iter()
        .filter(|v| {
            narrow(v, library) == narrow_col && shallow(v, library) == shallow_row && (wide || v.cells(library) == 1)
        })
        .collect();
    vec![fit[rng.below(fit.len())].clone()]
}

/// Every ordered pair of variants side by side (east and south); every lower
/// wire under every upper one alone; every stack of plain wiring beside every
/// variant; plus `quads` random 2×2 groups, a third of their cells stacks.
/// Returns the count of groups checked and the problems found.
pub fn check_all_adjacency(library: &Library, quads: usize, seed: u64) -> (usize, Vec<String>) {
    let variants = Variant::all();
    let mut problems = Vec::new();
    let mut checked = 0;
    for a in &variants {
        for b in &variants {
            // Side by side only in a row of one depth.
            if shallow(a, library) == shallow(b, library) {
                checked += 1;
                problems.extend(adjacency_problems(&[vec![a.clone(), b.clone()]], library));
            }
            // One above the other only in a column of one width.
            if narrow(a, library) == narrow(b, library) && a.cells(library) == b.cells(library) {
                checked += 1;
                problems.extend(adjacency_problems(&[vec![a.clone()], vec![b.clone()]], library));
            }
        }
    }
    for (low, high) in [
        (Kind::Wire, Kind::WireUp),
        (Kind::WireN, Kind::WireUpN),
        (Kind::WireS, Kind::WireUpS),
        (Kind::WireSq, Kind::WireUpSq),
    ] {
        let lowers: Vec<&Variant> = variants.iter().filter(|v| v.kind == low).collect();
        let uppers: Vec<&Variant> = variants.iter().filter(|v| v.kind == high).collect();
        for lower in &lowers {
            for upper in &uppers {
                checked += 1;
                problems.extend(stack_problems(lower, upper, library));
            }
        }
        for lower in lowers.iter().filter(|v| v.repeater_entry.is_none()) {
            for upper in uppers.iter().filter(|v| v.repeater_entry.is_none()) {
                let stack = vec![(*lower).clone(), (*upper).clone()];
                for other in &variants {
                    let row = shallow(other, library) == shallow(lower, library);
                    let column = narrow(other, library) == narrow(lower, library) && other.cells(library) == 1;
                    let other = vec![other.clone()];
                    let mut grids = Vec::new();
                    if row {
                        grids.push(vec![vec![stack.clone(), other.clone()]]);
                        grids.push(vec![vec![other.clone(), stack.clone()]]);
                    }
                    if column {
                        grids.push(vec![vec![stack.clone()], vec![other.clone()]]);
                        grids.push(vec![vec![other.clone()], vec![stack.clone()]]);
                    }
                    for grid in grids {
                        checked += 1;
                        problems.extend(adjacency_problems_stacked(&grid, library));
                    }
                }
            }
        }
    }
    let mut rng = Rng::new(seed);
    for _ in 0..quads {
        let narrow_col = [rng.below(3) == 0, rng.below(3) == 0];
        let shallow_row = [rng.below(2) == 0, rng.below(2) == 0];
        let wide = !narrow_col.contains(&true);
        let row = |rng: &mut Rng, shallow: bool| {
            vec![
                random_cell(&variants, (narrow_col[0], shallow), wide, rng, library),
                random_cell(&variants, (narrow_col[1], shallow), wide, rng, library),
            ]
        };
        let grid = vec![row(&mut rng, shallow_row[0]), row(&mut rng, shallow_row[1])];
        checked += 1;
        problems.extend(adjacency_problems_stacked(&grid, library));
    }
    problems.sort();
    problems.dedup();
    (checked, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Wire;

    #[test]
    fn matched_wires_are_fine_but_a_stray_dust_across_the_border_is_not() {
        let library = Library::builtin().unwrap();
        let matched = vec![vec![
            Variant::new(Kind::Wire, &[Edge::West, Edge::East]),
            Variant::new(Kind::Wire, &[Edge::West, Edge::East]),
        ]];
        assert!(adjacency_problems(&matched, &library).is_empty());

        // A west-east wire beside a north-east wire: nothing should connect them.
        let grid = vec![vec![
            Variant::new(Kind::Wire, &[Edge::West, Edge::East]),
            Variant::new(Kind::Wire, &[Edge::North, Edge::East]),
        ]];
        assert!(adjacency_problems(&grid, &library).is_empty());
        let mut world = World::new((2 * PITCH, HEIGHT, PITCH));
        for (c, variant) in grid[0].iter().enumerate() {
            for (pos, block) in variant.blocks(&library) {
                world.set(Pos::new(c as i32 * PITCH + pos.x, pos.y, pos.z), block);
            }
        }
        // Stray dust from the first tile's port into the second tile's north arm: a leak.
        for x in [4, 5, 6] {
            world.set(Pos::new(x, 0, 1), Block::Glass);
            world.set(Pos::new(x, 1, 1), Block::Wire(Wire::DOT));
        }
        let problems = problems_in(&world, &lay_out(&grid, &library), &[], &library);
        assert!(!problems.is_empty());
    }

    #[test]
    fn a_flip_flop_beside_a_wire_is_two_cells_wide() {
        let library = Library::builtin().unwrap();
        let dff = Variant::new(Kind::Dff, &[Edge::West, Edge::North, Edge::East]);
        let wire = Variant::new(Kind::Wire, &[Edge::West, Edge::East]);
        let placed = lay_out(&[vec![dff.clone(), wire.clone()]], &library);
        assert_eq!(placed[1].1, (2, 0));
        assert!(adjacency_problems(&[vec![dff, wire]], &library).is_empty());
    }

    #[test]
    fn every_variant_gets_a_fixture_with_expectations() {
        let library = Library::builtin().unwrap();
        for (name, fixture) in all_variant_fixtures(&library) {
            // Flip-flop sweeps carry no expectations beyond ending quiet: the game is
            // compared with the simulator, and `characterize` reads the result.
            let least = if name.starts_with("dff") { 1 } else { 2 };
            assert!(fixture["expect"].as_array().unwrap().len() >= least, "{name}");
        }
    }

    /// A stack is two circuits; a dust stair from the lower level up to the
    /// upper one joins them, inside one tile or across a border, and is caught.
    #[test]
    fn a_stair_between_the_wire_levels_is_caught() {
        let library = Library::builtin().unwrap();
        let lower = Variant::new(Kind::Wire, &[Edge::North, Edge::South]);
        let upper = Variant::new(Kind::WireUp, &[Edge::West, Edge::East]);
        assert!(stack_problems(&lower, &upper, &library).is_empty());
        let mut world = World::new((PITCH, HEIGHT, PITCH));
        for (pos, block) in lower.blocks(&library).into_iter().chain(upper.blocks(&library)) {
            world.set(pos, block);
        }
        // From the lower dust at (2, 1, 1) a step up to (1, 2, 1) on a stone, and
        // from there up to the upper dust at (1, 3, 2).
        world.set(Pos::new(1, 1, 1), Block::Stone);
        world.set(Pos::new(1, 2, 1), Block::Wire(Wire::DOT));
        assert_eq!(stack_problems_in(&world, &lower, &upper, &library).len(), 1);

        // Two stacks side by side, joined only on each level: fine. A stair from
        // the first's lower east port up to the second's upper west port is a leak.
        let straight = vec![Variant::new(Kind::Wire, &[Edge::West, Edge::East]), upper.clone()];
        let grid = vec![vec![straight, vec![lower.clone(), upper.clone()]]];
        assert!(adjacency_problems_stacked(&grid, &library).is_empty());
        let placed = lay_out_stacked(&grid, &library);
        let mut world = World::new((2 * PITCH, HEIGHT, PITCH));
        for (variant, (c, _)) in &placed {
            for (pos, block) in variant.blocks(&library) {
                world.set(Pos::new(c * PITCH + pos.x, pos.y, pos.z), block);
            }
        }
        assert!(problems_in(&world, &placed, &[], &library).is_empty());
        // A stair from the first tile's lower east port, south and up two levels
        // into the second tile's upper west port: the merged net still runs
        // between the tiles only through matched (upper) ports, but the first
        // stack's two levels are now one net.
        for (pos, block) in [
            (Pos::new(4, 0, 3), Block::Glass),
            (Pos::new(4, 1, 3), Block::Wire(Wire::DOT)),
            (Pos::new(4, 1, 4), Block::Stone),
            (Pos::new(4, 2, 4), Block::Wire(Wire::DOT)),
            (Pos::new(5, 2, 4), Block::Stone),
            (Pos::new(5, 3, 4), Block::Wire(Wire::DOT)),
            (Pos::new(5, 2, 3), Block::Stone),
            (Pos::new(5, 3, 3), Block::Wire(Wire::DOT)),
        ] {
            world.set(pos, block);
        }
        let problems = problems_in(&world, &placed, &[], &library);
        assert!(problems.iter().any(|p| p.contains("joins the two wire levels")), "{problems:?}");
    }
}
