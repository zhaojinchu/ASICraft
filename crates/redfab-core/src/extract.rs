//! Reads the circuit back out of a block world: which wires form one net, and
//! which torches, lamps and inputs each net connects.
//!
//! Nothing here knows about tiles or the netlist. It only asks the simulator
//! (whose rules were checked against the game) what powers what, so the
//! layout-versus-netlist check judges the blocks that will actually be placed.

use std::collections::BTreeMap;

use crate::block::{Block, Wire};
use crate::geom::Pos;
use crate::sim::Sim;
use crate::world::World;

/// A torch, a lamp, or an input source, with the nets it touches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub pos: Pos,
    /// Nets that power this component.
    pub reads: Vec<usize>,
    /// Nets this component powers.
    pub drives: Vec<usize>,
    /// Other components this one powers without a wire in between.
    pub direct: Vec<Pos>,
}

#[derive(Clone, Debug, Default)]
pub struct Circuit {
    /// Each net is the positions of its wires.
    pub nets: Vec<Vec<Pos>>,
    pub torches: Vec<Component>,
    pub lamps: Vec<Component>,
    /// Input sources: the probe blocks, plus any redstone block found in the world.
    pub sources: Vec<Component>,
    /// Pairs of wires where power can pass one way only.
    pub one_way: Vec<(Pos, Pos)>,
}

impl Circuit {
    pub fn net_of(&self, pos: Pos) -> Option<usize> {
        self.nets.iter().position(|net| net.contains(&pos))
    }
}

/// The same world with every signal off: wires at power 0, torches and lamps
/// unlit, levers off, the input probes as plain stone, and any other redstone
/// block as glass (which, like a redstone block, does not conduct).
fn quiet(world: &World, probes: &[Pos]) -> World {
    let mut out = world.clone();
    for (pos, block) in world.iter() {
        let off = match block {
            Block::Wire(w) => Block::Wire(Wire { power: 0, ..w }),
            Block::Torch { .. } => Block::Torch { lit: false },
            Block::WallTorch { facing, .. } => Block::WallTorch { facing, lit: false },
            Block::Lamp { .. } => Block::Lamp { lit: false },
            Block::Lever { face, facing, .. } => Block::Lever {
                face,
                facing,
                powered: false,
            },
            Block::RedstoneBlock if probes.contains(&pos) => Block::Stone,
            Block::RedstoneBlock => Block::Glass,
            other => other,
        };
        out.set(pos, off);
    }
    out
}

/// Extracts the circuit. `probes` are the input blocks (stone when off).
pub fn extract(world: &World, probes: &[Pos]) -> Circuit {
    let base = quiet(world, probes);
    let sim = Sim::new(base.clone());

    // Nets: wires joined by the same adjacency the power rules use.
    let wires: Vec<Pos> = base
        .iter()
        .filter(|(_, b)| matches!(b, Block::Wire(_)))
        .map(|(p, _)| p)
        .collect();
    let mut net_of: BTreeMap<Pos, usize> = wires.iter().enumerate().map(|(i, &p)| (p, i)).collect();
    let mut one_way = Vec::new();
    for &wire in &wires {
        for from in sim.wire_neighbors(wire) {
            if !sim.wire_neighbors(from).contains(&wire) && wire < from {
                one_way.push((from, wire));
            }
            let (a, b) = (net_of[&wire], net_of[&from]);
            if a != b {
                let (keep, drop) = (a.min(b), a.max(b));
                for id in net_of.values_mut().filter(|id| **id == drop) {
                    *id = keep;
                }
            }
        }
    }
    let mut ids: Vec<usize> = net_of.values().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let nets: Vec<Vec<Pos>> = ids
        .iter()
        .map(|&id| wires.iter().copied().filter(|w| net_of[w] == id).collect())
        .collect();

    let positions = |pred: fn(&Block) -> bool| -> Vec<Pos> {
        base.iter()
            .filter(|(_, b)| pred(b))
            .map(|(p, _)| p)
            .collect()
    };
    let torch_positions = positions(|b| b.is_torch());
    let lamp_positions = positions(|b| matches!(b, Block::Lamp { .. }));
    let mut source_positions: Vec<Pos> = probes.to_vec();
    source_positions.extend(
        world
            .iter()
            .filter(|(p, b)| *b == Block::RedstoneBlock && !probes.contains(p))
            .map(|(p, _)| p),
    );
    let component = |pos: Pos| Component {
        pos,
        reads: Vec::new(),
        drives: Vec::new(),
        direct: Vec::new(),
    };
    let mut torches: Vec<Component> = torch_positions.iter().map(|&p| component(p)).collect();
    let mut lamps: Vec<Component> = lamp_positions.iter().map(|&p| component(p)).collect();
    let mut sources: Vec<Component> = source_positions.iter().map(|&p| component(p)).collect();

    // What does each net power? Raise that net alone and look at every component.
    for (id, net) in nets.iter().enumerate() {
        let mut world = base.clone();
        for &pos in net {
            if let Block::Wire(w) = world.get(pos) {
                world.set(pos, Block::Wire(Wire { power: 15, ..w }));
            }
        }
        let sim = Sim::new(world);
        for torch in &mut torches {
            if sim.torch_is_powered(torch.pos) {
                torch.reads.push(id);
            }
        }
        for lamp in &mut lamps {
            if sim.is_powered(lamp.pos) {
                lamp.reads.push(id);
            }
        }
    }

    // What does each torch or source power? Switch that one alone on and look.
    let drive = |component: &mut Component, on: Block, others: &[Pos]| {
        let mut world = base.clone();
        world.set(component.pos, on);
        let sim = Sim::new(world);
        for (id, net) in nets.iter().enumerate() {
            if net.iter().any(|&w| sim.wire_block_signal(w) > 0) {
                component.drives.push(id);
            }
        }
        for &other in others {
            let powered = match base.get(other) {
                b if b.is_torch() => sim.torch_is_powered(other),
                _ => sim.is_powered(other),
            };
            if other != component.pos && powered {
                component.direct.push(other);
            }
        }
    };
    let all_sinks: Vec<Pos> = torch_positions
        .iter()
        .chain(&lamp_positions)
        .copied()
        .collect();
    for torch in &mut torches {
        let lit = match base.get(torch.pos) {
            Block::WallTorch { facing, .. } => Block::WallTorch { facing, lit: true },
            _ => Block::Torch { lit: true },
        };
        drive(torch, lit, &all_sinks);
    }
    for source in &mut sources {
        drive(source, Block::RedstoneBlock, &all_sinks);
    }

    Circuit {
        nets,
        torches,
        lamps,
        sources,
        one_way,
    }
}

/// With each driver switched on alone, every wire of the nets it drives must
/// carry some power. Returns a message for each wire the signal does not reach.
pub fn weak_signals(world: &World, probes: &[Pos], circuit: &Circuit) -> Vec<String> {
    let base = quiet(world, probes);
    let mut problems = Vec::new();
    let drivers = circuit
        .torches
        .iter()
        .map(|t| (t, true))
        .chain(circuit.sources.iter().map(|s| (s, false)));
    for (driver, is_torch) in drivers {
        let on = match (is_torch, base.get(driver.pos)) {
            (true, Block::WallTorch { facing, .. }) => Block::WallTorch { facing, lit: true },
            (true, _) => Block::Torch { lit: true },
            (false, _) => Block::RedstoneBlock,
        };
        let mut sim = Sim::new(base.clone());
        if sim.set_block(driver.pos, on, true).is_err() {
            continue;
        }
        sim.wake(driver.pos);
        for &net in &driver.drives {
            for &wire in &circuit.nets[net] {
                if sim
                    .world()
                    .get(wire)
                    .as_wire()
                    .is_some_and(|w| w.power == 0)
                {
                    problems.push(format!(
                        "the signal from {} dies before reaching the wire at {wire}",
                        driver.pos
                    ));
                }
            }
        }
    }
    problems
}
