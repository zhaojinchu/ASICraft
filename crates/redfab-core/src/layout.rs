//! A design as tiles on a grid, and its realisation as blocks.

use std::collections::BTreeMap;

use crate::geom::Pos;
use crate::netlist::Netlist;
use crate::tile::{Edge, HEIGHT, Library, PITCH, Tile};
use crate::world::World;

/// What occupies one grid position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placed {
    /// A design input; drives east.
    Input { port: usize },
    /// A design output; fed from the west.
    Output { port: usize },
    /// Wiring joining the listed edges.
    Wire { edges: Vec<Edge> },
    /// A NOR gate for netlist cell `cell`. `pin_edges[i]` is the edge input pin i enters by.
    Gate { cell: usize, pin_edges: Vec<Edge> },
}

impl Placed {
    /// The edges this tile uses.
    pub fn edges(&self) -> Vec<Edge> {
        match self {
            Placed::Input { .. } => vec![Edge::East],
            Placed::Output { .. } => vec![Edge::West],
            Placed::Wire { edges } => edges.clone(),
            Placed::Gate { pin_edges, .. } => {
                let mut edges = pin_edges.clone();
                edges.push(Edge::East);
                edges
            }
        }
    }

    fn tile<'a>(&self, library: &'a Library) -> &'a Tile {
        match self {
            Placed::Input { .. } => &library.input,
            Placed::Output { .. } => &library.output,
            Placed::Wire { .. } => &library.wire,
            Placed::Gate { .. } => &library.nor,
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
}

impl Layout {
    pub fn size(&self) -> (i32, i32) {
        let cols = self.tiles.keys().map(|k| k.0).max().map_or(0, |m| m + 1);
        let rows = self.tiles.keys().map(|k| k.1).max().map_or(0, |m| m + 1);
        (cols, rows)
    }

    /// Every used edge must face a tile that uses the opposite edge; a signal
    /// must never run off into nothing or into the side of a tile.
    pub fn check_ports(&self) -> Result<(), String> {
        for (&(col, row), placed) in &self.tiles {
            for edge in placed.edges() {
                let (dc, dr) = edge.step();
                let faces =
                    self.tiles.get(&(col + dc, row + dr)).is_some_and(|other| other.edges().contains(&edge.opposite()));
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
        for (&(col, row), placed) in &self.tiles {
            let origin = Pos::new(col * PITCH, 0, row * PITCH);
            let at = |p: Pos| Pos::new(origin.x + p.x, origin.y + p.y, origin.z + p.z);
            let tile = placed.tile(library);
            for (pos, block) in tile.blocks(&placed.edges()) {
                world.set(at(pos), block);
            }
            match placed {
                Placed::Input { port } => probes[*port] = tile.probe.map(at),
                Placed::Output { port } => lamps[*port] = tile.lamp.map(at),
                _ => {}
            }
        }
        let all = |found: Vec<Option<Pos>>, what: &str| -> Result<Vec<Pos>, String> {
            found.into_iter().enumerate().map(|(i, p)| p.ok_or_else(|| format!("{what} {i} was not placed"))).collect()
        };
        Ok(Realized { world, probes: all(probes, "input")?, lamps: all(lamps, "output")? })
    }
}
