//! A design as tiles on a grid, and its realisation as blocks.

use std::collections::BTreeMap;

use crate::block::Block;
use crate::geom::Pos;
use crate::netlist::Netlist;
use crate::tile::{Edge, HEIGHT, Kind, Library, PITCH, Variant};
use crate::world::World;

/// What occupies one grid position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placed {
    /// A design input; drives east.
    /// A design input; `read` is false when nothing reads it, and then it has no dust.
    Input { port: usize, read: bool },
    /// A design output; fed from the west.
    Output { port: usize },
    /// Wiring joining the listed edges.
    Wire { edges: Vec<Edge> },
    /// A NOR gate for netlist cell `cell`. `pin_edges[i]` is the edge input pin i enters by.
    Gate { cell: usize, pin_edges: Vec<Edge> },
    /// Two wires crossing. With `rotation` 0 the west-east wire is the flat one,
    /// with 1 the north-south wire. `repeater_entry` (named before rotation, so
    /// west or east) puts a repeater in the flat arm the signal enters by.
    Cross { rotation: u8, repeater_entry: Option<Edge>, delay: u8 },
    /// Wiring with a repeater in the arm the signal enters by.
    Repeater { edges: Vec<Edge>, entry: Edge, delay: u8 },
    /// Constant 1 for `net`; drives east.
    TieHigh { net: usize },
    /// Constant 0 for `net`; drives east.
    TieLow { net: usize },
    /// The flip-flop for netlist cell `cell`, two cells wide: D from the west,
    /// the clock from the north, Q to the east of its second cell.
    Dff { cell: usize, init: bool },
}

impl Placed {
    /// The edges this tile uses.
    pub fn edges(&self) -> Vec<Edge> {
        match self {
            Placed::Input { read: true, .. } => vec![Edge::East],
            Placed::Input { read: false, .. } => Vec::new(),
            Placed::Output { .. } => vec![Edge::West],
            Placed::Wire { edges } => edges.clone(),
            Placed::Gate { pin_edges, .. } => {
                let mut edges = pin_edges.clone();
                edges.push(Edge::East);
                edges
            }
            Placed::Cross { .. } => Edge::ALL.to_vec(),
            Placed::Repeater { edges, .. } => edges.clone(),
            Placed::TieHigh { .. } | Placed::TieLow { .. } => vec![Edge::East],
            Placed::Dff { .. } => vec![Edge::West, Edge::North, Edge::East],
        }
    }

    /// How many grid cells wide the tile is.
    pub fn width(&self) -> i32 {
        match self {
            Placed::Dff { .. } => 2,
            _ => 1,
        }
    }

    /// The tile variant this placement uses.
    pub fn variant(&self) -> Variant {
        match self {
            Placed::Input { .. } => Variant::new(Kind::Input, &self.edges()),
            Placed::Output { .. } => Variant::new(Kind::Output, &self.edges()),
            Placed::Wire { .. } => Variant::new(Kind::Wire, &self.edges()),
            Placed::Gate { .. } => Variant::new(Kind::Nor, &self.edges()),
            Placed::Cross { rotation, repeater_entry, delay } => Variant {
                kind: Kind::Cross,
                edges: Edge::ALL.to_vec(),
                rotation: *rotation,
                repeater_entry: *repeater_entry,
                delay: *delay,
            },
            Placed::Repeater { edges, entry, delay } => Variant::repeater(edges, *entry, *delay),
            Placed::TieHigh { .. } => Variant::new(Kind::TieHigh, &[Edge::East]),
            Placed::TieLow { .. } => Variant::new(Kind::TieLow, &[Edge::East]),
            Placed::Dff { .. } => Variant::new(Kind::Dff, &[Edge::West, Edge::North, Edge::East]),
        }
    }
}

/// Tiles by (column, row). Columns run east, rows run south.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub tiles: BTreeMap<(i32, i32), Placed>,
}

/// A layout turned into blocks, with where to drive and read it.
pub struct Realized {
    pub world: World,
    /// For each design input, the block to swap between stone and a redstone block.
    pub probes: Vec<Pos>,
    /// For each design output, the lamp.
    pub lamps: Vec<Pos>,
    /// For each constant net: a block of its tile that belongs to the net (the
    /// torch for a 1, a dust for a 0).
    pub ties: Vec<(usize, bool, Pos)>,
    /// Every flip-flop, for the checks that treat it as one part.
    pub macros: Vec<Macro>,
}

/// A placed flip-flop: where it is, its port cells, and the blocks it is made of.
#[derive(Clone, Debug)]
pub struct Macro {
    pub cell: usize,
    pub origin: Pos,
    pub width: i32,
    pub d: Pos,
    pub clk: Pos,
    pub q: Pos,
    pub blocks: Vec<(Pos, Block)>,
}

impl Macro {
    pub fn contains(&self, pos: Pos) -> bool {
        pos.x >= self.origin.x
            && pos.x < self.origin.x + self.width * PITCH
            && pos.z >= self.origin.z
            && pos.z < self.origin.z + PITCH
    }
}

impl Layout {
    pub fn size(&self) -> (i32, i32) {
        let cols = self.tiles.iter().map(|(k, p)| k.0 + p.width()).max().unwrap_or(0);
        let rows = self.tiles.keys().map(|k| k.1).max().map_or(0, |m| m + 1);
        (cols, rows)
    }

    /// The tile at `cell`, counting a wide tile's further cells.
    pub fn at(&self, cell: (i32, i32)) -> Option<&Placed> {
        self.tiles.get(&cell).or_else(|| self.tiles.get(&(cell.0 - 1, cell.1)).filter(|p| p.width() > 1))
    }

    /// Every used edge must face a tile that uses the opposite edge; a signal
    /// must never run off into nothing or into the side of a tile.
    pub fn check_ports(&self) -> Result<(), String> {
        for (&(col, row), placed) in &self.tiles {
            for edge in placed.edges() {
                let (dc, dr) = edge.step();
                // The east port of a wide tile is on its last cell.
                let from = if edge == Edge::East { col + placed.width() - 1 } else { col };
                let faces =
                    self.at((from + dc, row + dr)).is_some_and(|other| other.edges().contains(&edge.opposite()));
                if !faces {
                    return Err(format!(
                        "tile at column {col}, row {row} has an open port on its {} edge",
                        edge.letter()
                    ));
                }
            }
        }
        Ok(())
    }

    /// Stamps every tile's blocks into a world. Dust has no shape or power yet;
    /// [`crate::bake::bake`] computes those.
    pub fn realize(&self, library: &Library, netlist: &Netlist) -> Result<Realized, String> {
        self.check_ports()?;
        let (cols, rows) = self.size();
        if cols == 0 || rows == 0 {
            return Err("the layout is empty".into());
        }
        let mut world = World::new((cols * PITCH, HEIGHT, rows * PITCH));
        let mut probes = vec![None; netlist.inputs.len()];
        let mut lamps = vec![None; netlist.outputs.len()];
        let mut ties = Vec::new();
        let mut macros = Vec::new();
        for (&(col, row), placed) in &self.tiles {
            let origin = Pos::new(col * PITCH, 0, row * PITCH);
            let at = |p: Pos| Pos::new(origin.x + p.x, origin.y + p.y, origin.z + p.z);
            let variant = placed.variant();
            let tile = variant.tile(library);
            let mut blocks = variant.blocks(library);
            if let Placed::Dff { init, .. } = placed {
                // The stored value lives in the state repeaters' `powered`.
                for (pos, block) in &mut blocks {
                    if tile.state.contains(pos)
                        && let Block::Repeater { facing, delay, locked, .. } = *block
                    {
                        *block = Block::Repeater { facing, delay, locked, powered: *init };
                    }
                }
            }
            for (pos, block) in &blocks {
                world.set(at(*pos), *block);
            }
            match placed {
                Placed::Input { port, .. } => probes[*port] = tile.probe.map(at),
                Placed::Output { port } => lamps[*port] = tile.lamp.map(at),
                Placed::TieHigh { net } => ties.push((*net, true, at(Pos::new(2, 1, 2)))),
                Placed::TieLow { net } => ties.push((*net, false, at(Pos::new(3, 1, 2)))),
                Placed::Dff { cell, .. } => {
                    let port = |edge: Edge| {
                        tile.port(edge)
                            .map(|p| at(p.pos))
                            .ok_or_else(|| format!("the flip-flop has no {} port", edge.letter()))
                    };
                    macros.push(Macro {
                        cell: *cell,
                        origin,
                        width: tile.cells(),
                        d: port(Edge::West)?,
                        clk: port(Edge::North)?,
                        q: port(Edge::East)?,
                        blocks: blocks.iter().map(|(p, b)| (at(*p), *b)).collect(),
                    });
                }
                _ => {}
            }
        }
        let all = |found: Vec<Option<Pos>>, what: &str| -> Result<Vec<Pos>, String> {
            found.into_iter().enumerate().map(|(i, p)| p.ok_or_else(|| format!("{what} {i} was not placed"))).collect()
        };
        Ok(Realized { world, probes: all(probes, "input")?, lamps: all(lamps, "output")?, ties, macros })
    }
}
