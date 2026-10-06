//! Checks of the tile library itself: a fixture per tile variant with drivers and
//! readers on its ports, random tile groups for the game, and the static
//! adjacency check of the tile contract.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::block::Block;
use crate::extract::extract;
use crate::geom::Pos;
use crate::tile::{Edge, HEIGHT, Kind, Library, PITCH, PortKind, Variant, VariantPort};
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
        Kind::Wire if variant.repeater_entry.is_some() => {
            "Wire with a repeater: every other port follows the entry after 2 game ticks per setting, and not before."
        }
        Kind::Wire => "Wiring: driving any used port must power every other used port, at once.",
        Kind::Nor => "NOR: the output is powered exactly when no input is, 2 game ticks after the inputs change.",
        Kind::Cross if variant.repeater_entry.is_some() => {
            "Crossing with a repeater: the flat axis follows its entry after the delay; the other axis passes at once."
        }
        Kind::Cross => "Crossing: west drives east only, north drives south only, at once.",
        Kind::TieHigh => "Constant 1: the output is always powered.",
        Kind::TieLow => "Constant 0: the output is never powered.",
        Kind::Input => "Input: the output is powered while the probe block is a redstone block.",
        Kind::Output => "Output: the lamp lights at once and goes out 4 game ticks after the input drops.",
        Kind::Dff | Kind::DffAlt => unreachable!("flip-flops have sweep fixtures, see dfftest"),
    };
    let mut fx = Fixture::new(name, doc.to_string(), box_for(variant, library));
    let hooks = stamp(&mut fx, variant, library);
    let on = Block::RedstoneBlock;
    let off = Block::Air;
    let delay = variant.delay_ticks(library);
    let mut t = 2;
    match variant.kind {
        Kind::Wire if variant.repeater_entry.is_some() => {
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
        Kind::Wire => {
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
        Kind::Cross if variant.repeater_entry.is_some() => {
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
        Kind::Cross => {
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
        Kind::Nor => {
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
        Kind::TieHigh | Kind::TieLow => {
            let output = &hooks[0];
            fx.powered(0, output.reader, variant.kind == Kind::TieHigh);
            fx.powered(6, output.reader, variant.kind == Kind::TieHigh);
        }
        Kind::Dff | Kind::DffAlt => unreachable!(),
        Kind::Input => {
            let probe = variant.tile(library).probe.expect("input tile has a probe");
            let probe = Pos::new(ORIGIN.x + probe.x, ORIGIN.y + probe.y, ORIGIN.z + probe.z);
            let output = &hooks[0];
            fx.at(2, probe, Block::RedstoneBlock);
            fx.powered(2, output.reader, true);
            fx.at(6, probe, Block::Stone);
            fx.powered(6, output.reader, false);
        }
        Kind::Output => {
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

/// Fixtures for every variant, by name.
pub fn all_variant_fixtures(library: &Library) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for v in Variant::all() {
        if matches!(v.kind, Kind::Dff | Kind::DffAlt) {
            out.extend(crate::dfftest::fixtures(&v, library).into_iter().map(|f| (f.name.clone(), f.json)));
        } else {
            out.push((v.name(), variant_fixture(&v, library)));
        }
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
    let grid: Vec<Vec<Variant>> =
        (0..rows).map(|_| (0..cols).map(|_| variants[rng.below(variants.len())].clone()).collect()).collect();
    let placed = lay_out(&grid, library);
    let width = placed.iter().map(|(v, (c, _))| c + v.cells(library)).max().unwrap_or(0);
    let margin = 2;
    let size = [width * PITCH + 2 * margin, HEIGHT, rows * PITCH + 2 * margin];
    let mut fx = Fixture::new(name.to_string(), format!("{cols}x{rows} random tiles"), size);
    let mut drivers = Vec::new();
    let covered = |cell: (i32, i32)| {
        placed.iter().any(|(v, (c, r))| *r == cell.1 && (*c..c + v.cells(library)).contains(&cell.0))
    };
    for (variant, (c, r)) in &placed {
        let origin = Pos::new(margin + c * PITCH, 0, margin + r * PITCH);
        for (pos, block) in variant.blocks(library) {
            fx.put(Pos::new(origin.x + pos.x, pos.y, origin.z + pos.z), block);
        }
        for port in variant.ports(library) {
            let (dc, dr) = port.edge.step();
            // The cell beyond the port: on a wide tile the east port is on its last cell.
            let port_cell = (c + port.pos.x / PITCH, *r);
            if covered((port_cell.0 + dc, port_cell.1 + dr)) {
                continue; // whatever is there is part of the test
            }
            let cell = Pos::new(origin.x + port.pos.x, 1, origin.z + port.pos.z).offset(port.edge.dir());
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
    let stretch: u32 = grid.iter().flatten().map(|v| 2 * v.delay_ticks(library)).sum();
    fx.finish(12 + stretch)
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

/// Problems with a grid of placed variants: any way two tiles affect each other
/// other than through matched ports on a shared edge.
pub fn adjacency_problems(grid: &[Vec<Variant>], library: &Library) -> Vec<String> {
    let placed = lay_out(grid, library);
    let rows = grid.len() as i32;
    let cols = placed.iter().map(|(v, (c, _))| c + v.cells(library)).max().unwrap_or(0);
    let mut world = World::new((cols * PITCH, HEIGHT, rows * PITCH));
    let mut probes = Vec::new();
    for (variant, (c, r)) in &placed {
        let origin = Pos::new(c * PITCH, 0, r * PITCH);
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
    // Which placed tile a position belongs to, by index.
    let tile_of = |pos: Pos| -> usize {
        let (c, r) = (pos.x.div_euclid(PITCH), pos.z.div_euclid(PITCH));
        placed
            .iter()
            .position(|(v, (vc, vr))| *vr == r && (*vc..vc + v.cells(library)).contains(&c))
            .unwrap_or(usize::MAX)
    };
    let name_of = |i: usize| match placed.get(i) {
        Some((v, (c, r))) => format!("{} at column {c}, row {r}", v.name()),
        None => "no tile".to_string(),
    };
    // The port cell of tile `i` on `edge`, if it uses that edge.
    let port_cell = |i: usize, edge: Edge| -> Option<Pos> {
        let (v, (c, r)) = &placed[i];
        v.ports(library)
            .into_iter()
            .find(|p| p.edge == edge)
            .map(|p| Pos::new(c * PITCH + p.pos.x, 1, r * PITCH + p.pos.z))
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
                    match (port_cell(a, edge), port_cell(b, edge.opposite())) {
                        (Some(pa), Some(pb)) => {
                            pb == Pos::new(pa.x + dx, pa.y, pa.z + dz) && net.contains(&pa) && net.contains(&pb)
                        }
                        _ => false,
                    }
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

/// Every ordered pair of variants side by side (east and south), plus `quads`
/// random 2×2 groups. Returns the problems found.
pub fn check_all_adjacency(library: &Library, quads: usize, seed: u64) -> (usize, Vec<String>) {
    let variants = Variant::all();
    let mut problems = Vec::new();
    let mut checked = 0;
    for a in &variants {
        for b in &variants {
            for grid in [vec![vec![a.clone(), b.clone()]], vec![vec![a.clone()], vec![b.clone()]]] {
                checked += 1;
                problems.extend(adjacency_problems(&grid, library));
            }
        }
    }
    let mut rng = Rng::new(seed);
    for _ in 0..quads {
        let pick = |rng: &mut Rng| variants[rng.below(variants.len())].clone();
        let grid = vec![vec![pick(&mut rng), pick(&mut rng)], vec![pick(&mut rng), pick(&mut rng)]];
        checked += 1;
        problems.extend(adjacency_problems(&grid, library));
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
}
