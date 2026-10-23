//! A design as tiles on a grid, and its realisation as blocks.

use std::collections::BTreeMap;

use crate::block::Block;
use crate::geom::Pos;
use crate::netlist::Netlist;
use crate::tile::{Edge, HEIGHT, Kind, Library, PITCH, Variant};
use crate::world::World;

/// Whether wiring in a cell `width` blocks wide and `depth` deep can carry a
/// repeater entered by `entry` joining `edges`. An arm along an axis less than a
/// cell long is the centre itself (west and east in a narrow column, north and
/// south in a shallow row): a straight carries the repeater there, and a turn in
/// the arm it leaves by when that arm has a cell of its own, but a fan-out has
/// no one place for it ([`crate::tile::Tile::repeater_place`], which a test
/// holds this to).
pub fn can_carry_repeater(width: i32, depth: i32, edges: &[Edge], entry: Edge) -> bool {
    let has_arm = |edge: Edge| if matches!(edge, Edge::West | Edge::East) { width >= PITCH } else { depth >= PITCH };
    if has_arm(entry) {
        return true;
    }
    match edges.iter().copied().filter(|&e| e != entry).collect::<Vec<_>>()[..] {
        [exit] => exit == entry.opposite() || has_arm(exit),
        _ => false,
    }
}

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
    /// From the lower wire level to the upper one: the lower port on `lower`, the
    /// upper port on the opposite edge. A ramp is its cell's alone, on both levels.
    Ramp { lower: Edge },
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
            Placed::Ramp { lower } => vec![*lower],
            Placed::TieHigh { .. } | Placed::TieLow { .. } => vec![Edge::East],
            Placed::Dff { .. } => vec![Edge::West, Edge::North, Edge::East],
        }
    }

    /// The edges this tile of the lower map uses on the upper wire level: a
    /// ramp's upper port. (A tile of the upper map uses its [`Placed::edges`] there.)
    pub fn upper_edges(&self) -> Vec<Edge> {
        match self {
            Placed::Ramp { lower } => vec![lower.opposite()],
            _ => Vec::new(),
        }
    }

    /// Whether wiring may run on the upper level above this tile of the lower
    /// map: only above wiring, never above a gate, an input, an output, a
    /// constant or a flip-flop, whose torches and lamps must not be beside the
    /// upper level's stone, nor above a ramp, which is its cell's alone.
    pub fn may_be_covered(&self) -> bool {
        matches!(self, Placed::Wire { .. } | Placed::Repeater { .. })
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
            Placed::Ramp { lower } => {
                Variant { rotation: ramp_turn(*lower), ..Variant::new(Kind::Ramp, &[Edge::West, Edge::East]) }
            }
            Placed::TieHigh { .. } => Variant::new(Kind::TieHigh, &[Edge::East]),
            Placed::TieLow { .. } => Variant::new(Kind::TieLow, &[Edge::East]),
            Placed::Dff { .. } => Variant::new(Kind::Dff, &[Edge::West, Edge::North, Edge::East]),
        }
    }

    /// The tile variant this placement uses in the upper map: the upper wire.
    pub fn upper_variant(&self) -> Result<Variant, String> {
        match self {
            Placed::Wire { edges } => Ok(Variant::new(Kind::WireUp, edges)),
            Placed::Repeater { edges, entry, delay } => {
                Ok(Variant { kind: Kind::WireUp, ..Variant::repeater(edges, *entry, *delay) })
            }
            other => Err(format!("only wiring runs on the upper level, not {other:?}")),
        }
    }

    /// The variant this placement uses in a cell `width` blocks wide and `depth`
    /// deep: the full cell's, or the narrow, shallow or square family's (the
    /// crossing then only unturned, with its repeater in the arm that is flat
    /// there, and the ramps by the axis they climb along).
    pub fn variant_in(&self, width: i32, depth: i32, upper: bool) -> Result<Variant, String> {
        let wide = if upper { self.upper_variant()? } else { self.variant() };
        if width >= PITCH && depth >= PITCH {
            return Ok(wide);
        }
        let (kind, edges, rotation, repeater_entry) = match self {
            Placed::Ramp { lower } => {
                let along_x = matches!(lower, Edge::West | Edge::East);
                let edges = if along_x { vec![Edge::West, Edge::East] } else { vec![Edge::North, Edge::South] };
                let rotation = if matches!(lower, Edge::West | Edge::North) { 0 } else { 2 };
                (Kind::ramp_sized(width, depth, along_x), edges, rotation, None)
            }
            Placed::Cross { rotation, repeater_entry, .. } => {
                let entry = repeater_entry.map(|e| e.rotated(*rotation));
                let flat_x = width < PITCH;
                if let Some(e) = entry
                    && matches!(e, Edge::West | Edge::East) != flat_x
                {
                    return Err(format!(
                        "a repeater entered by {e:?} in the climbing arm of {self:?}, {width} by {depth}"
                    ));
                }
                let kind = Kind::Cross
                    .sized(width, depth)
                    .ok_or_else(|| format!("no crossing fits {width} by {depth} blocks"))?;
                (kind, Edge::ALL.to_vec(), 0, entry)
            }
            _ => {
                if let Some(entry) = wide.repeater_entry
                    && !can_carry_repeater(width, depth, &wide.edges, entry)
                {
                    return Err(format!("a repeater entered by {entry:?} in {self:?}, {width} by {depth}"));
                }
                let kind = wide
                    .kind
                    .sized(width, depth)
                    .ok_or_else(|| format!("no tile {width} by {depth} blocks for {self:?}"))?;
                (kind, wide.edges.clone(), 0, wide.repeater_entry)
            }
        };
        Ok(Variant { kind, edges, rotation, repeater_entry, ..wide })
    }
}

/// How far the ramp tile (lower west, upper east as written) is turned so that
/// its lower port is on `lower`.
pub fn ramp_turn(lower: Edge) -> u8 {
    (0..4).find(|&r| Edge::West.rotated(r) == lower).expect("every edge is a turn of the west")
}

/// Tiles by (column, row), on the lower wire level (gates, inputs, outputs,
/// constants, flip-flops, wiring and ramps) and the upper one (wiring only).
/// Columns run east, rows run south. Each column has a width and each row a
/// depth in blocks (every one `PITCH` today), and a tile's blocks start where
/// the columns and rows before it end.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub tiles: BTreeMap<(i32, i32), Placed>,
    /// The upper wire level: wiring and repeaters above cells of `tiles` that
    /// hold wiring or nothing (a cell with one of each is a stack).
    pub upper: BTreeMap<(i32, i32), Placed>,
    /// Blocks per column, west to east.
    pub col_width: Vec<i32>,
    /// Blocks per row, north to south.
    pub row_depth: Vec<i32>,
}

/// Where each of a run of columns (or rows) starts in blocks, and where the last
/// one ends: the prefix sums of their widths, one entry more than there are widths.
pub fn origins(widths: &[i32]) -> Vec<i32> {
    let mut out = Vec::with_capacity(widths.len() + 1);
    let mut at = 0;
    out.push(at);
    for &w in widths {
        at += w;
        out.push(at);
    }
    out
}

/// The column (or row) whose span in an `origins` table holds the block
/// coordinate `at`; `None` before the first or beyond the last.
pub fn index_at(origins: &[i32], at: i32) -> Option<i32> {
    let n = origins.len().checked_sub(1)?;
    (0..n).map(|i| i as i32).find(|&i| origins[i as usize] <= at && at < origins[i as usize + 1])
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
    /// Cells wide.
    pub width: i32,
    /// Blocks along x and z from the origin: the cells it covers.
    pub extent: (i32, i32),
    pub d: Pos,
    pub clk: Pos,
    pub q: Pos,
    pub blocks: Vec<(Pos, Block)>,
}

impl Macro {
    pub fn contains(&self, pos: Pos) -> bool {
        pos.x >= self.origin.x
            && pos.x < self.origin.x + self.extent.0
            && pos.z >= self.origin.z
            && pos.z < self.origin.z + self.extent.1
    }
}

impl Layout {
    /// An empty layout whose columns are all `PITCH` wide and rows all `PITCH` deep.
    pub fn uniform(cols: i32, rows: i32) -> Layout {
        Layout {
            tiles: BTreeMap::new(),
            upper: BTreeMap::new(),
            col_width: vec![PITCH; cols as usize],
            row_depth: vec![PITCH; rows as usize],
        }
    }

    pub fn size(&self) -> (i32, i32) {
        let cols = self.tiles.iter().chain(&self.upper).map(|(k, p)| k.0 + p.width()).max().unwrap_or(0);
        let rows = self.tiles.keys().chain(self.upper.keys()).map(|k| k.1).max().map_or(0, |m| m + 1);
        (cols, rows)
    }

    /// Cells holding anything, on either level.
    pub fn cells_used(&self) -> usize {
        self.tiles.keys().chain(self.upper.keys()).collect::<std::collections::BTreeSet<_>>().len()
    }

    /// Repeaters, on either level.
    pub fn repeater_count(&self) -> usize {
        self.tiles
            .values()
            .chain(self.upper.values())
            .filter(|p| matches!(p, Placed::Repeater { .. } | Placed::Cross { repeater_entry: Some(_), .. }))
            .count()
    }

    /// The edges used on the upper level at `cell`: its upper wiring's, or a ramp's upper port.
    fn upper_at(&self, cell: (i32, i32)) -> Vec<Edge> {
        let mut edges = self.upper.get(&cell).map(Placed::edges).unwrap_or_default();
        edges.extend(self.tiles.get(&cell).map(Placed::upper_edges).unwrap_or_default());
        edges
    }

    /// Where the tiles start in blocks, by column and by row: the prefix sums of
    /// the widths and depths, one entry past the last column and row. Fails when
    /// the tables do not cover every column or row that holds a tile.
    pub fn block_origins(&self) -> Result<(Vec<i32>, Vec<i32>), String> {
        let (cols, rows) = self.size();
        if self.col_width.len() < cols as usize || self.row_depth.len() < rows as usize {
            return Err(format!(
                "the layout has {cols} columns and {rows} rows but widths for {} and depths for {}",
                self.col_width.len(),
                self.row_depth.len()
            ));
        }
        Ok((origins(&self.col_width[..cols as usize]), origins(&self.row_depth[..rows as usize])))
    }

    /// The tile at `cell`, counting a wide tile's further cells.
    pub fn at(&self, cell: (i32, i32)) -> Option<&Placed> {
        self.tiles.get(&cell).or_else(|| self.tiles.get(&(cell.0 - 1, cell.1)).filter(|p| p.width() > 1))
    }

    /// Every used edge must face a tile that uses the opposite edge on the same
    /// wire level; a signal must never run off into nothing or into the side of
    /// a tile. Upper wiring sits only above wiring or nothing.
    pub fn check_ports(&self) -> Result<(), String> {
        for &(col, row) in self.upper.keys() {
            if let Some(below) = self.at((col, row))
                && !below.may_be_covered()
            {
                return Err(format!("an upper wire at column {col}, row {row} is above {below:?}"));
            }
            for edge in self.upper_at((col, row)) {
                let (dc, dr) = edge.step();
                if !self.upper_at((col + dc, row + dr)).contains(&edge.opposite()) {
                    return Err(format!(
                        "the upper wire at column {col}, row {row} has an open port on its {} edge",
                        edge.letter()
                    ));
                }
            }
        }
        for (&(col, row), placed) in &self.tiles {
            for edge in placed.upper_edges() {
                let (dc, dr) = edge.step();
                if !self.upper_at((col + dc, row + dr)).contains(&edge.opposite()) {
                    return Err(format!(
                        "the ramp at column {col}, row {row} has an open upper port on its {} edge",
                        edge.letter()
                    ));
                }
            }
        }
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
        let (xs, zs) = self.block_origins()?;
        let mut world = World::new((xs[cols as usize], HEIGHT, zs[rows as usize]));
        let mut probes = vec![None; netlist.inputs.len()];
        let mut lamps = vec![None; netlist.outputs.len()];
        let mut ties = Vec::new();
        let mut macros = Vec::new();
        for (&(col, row), placed) in &self.tiles {
            let origin = Pos::new(xs[col as usize], 0, zs[row as usize]);
            let at = |p: Pos| Pos::new(origin.x + p.x, origin.y + p.y, origin.z + p.z);
            let variant = placed.variant_in(self.col_width[col as usize], self.row_depth[row as usize], false)?;
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
                // The torch, and the first dust, on the tile's middle row.
                Placed::TieHigh { net } => ties.push((*net, true, at(Pos::new(2, 1, tile.depth / 2)))),
                Placed::TieLow { net } => ties.push((*net, false, at(Pos::new(3, 1, tile.depth / 2)))),
                Placed::Dff { cell, .. } => {
                    let port = |edge: Edge| {
                        tile.port(edge)
                            .map(|p| at(p.pos))
                            .ok_or_else(|| format!("the flip-flop has no {} port", edge.letter()))
                    };
                    let (end_col, end_row) = ((col + tile.cells()) as usize, (row + 1) as usize);
                    macros.push(Macro {
                        cell: *cell,
                        origin,
                        width: tile.cells(),
                        extent: (xs[end_col] - origin.x, zs[end_row] - origin.z),
                        d: port(Edge::West)?,
                        clk: port(Edge::North)?,
                        q: port(Edge::East)?,
                        blocks: blocks.iter().map(|(p, b)| (at(*p), *b)).collect(),
                    });
                }
                _ => {}
            }
        }
        for (&(col, row), placed) in &self.upper {
            let origin = Pos::new(xs[col as usize], 0, zs[row as usize]);
            let (width, depth) = (self.col_width[col as usize], self.row_depth[row as usize]);
            for (pos, block) in placed.variant_in(width, depth, true)?.blocks(library) {
                world.set(Pos::new(origin.x + pos.x, origin.y + pos.y, origin.z + pos.z), block);
            }
        }
        let all = |found: Vec<Option<Pos>>, what: &str| -> Result<Vec<Pos>, String> {
            found.into_iter().enumerate().map(|(i, p)| p.ok_or_else(|| format!("{what} {i} was not placed"))).collect()
        };
        Ok(Realized { world, probes: all(probes, "input")?, lamps: all(lamps, "output")?, ties, macros })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::place_and_route;

    /// A toggle flip-flop: one flip-flop, two inverters.
    const TOGGLE: &str = r#"{"modules":{"top":{
        "ports":{"clk":{"direction":"input","bits":[2]},"q":{"direction":"output","bits":[3]}},
        "netnames":{"q":{"bits":[3],"attributes":{"init":"1"}}},
        "cells":{"inv":{"type":"NOT","connections":{"A":[3],"Y":[4]}},
                 "nclk":{"type":"NOT","connections":{"A":[2],"Y":[5]}},
                 "ff":{"type":"DFF","connections":{"D":[4],"CLK":[5],"Q":[3]}}}}}}"#;

    #[test]
    fn origins_are_prefix_sums_and_index_at_finds_the_span() {
        assert_eq!(origins(&[]), vec![0]);
        assert_eq!(origins(&[5, 5, 5]), vec![0, 5, 10, 15]);
        assert_eq!(origins(&[3, 5, 3]), vec![0, 3, 8, 11]);
        let table = origins(&[3, 5, 3]);
        assert_eq!(index_at(&table, 0), Some(0));
        assert_eq!(index_at(&table, 2), Some(0));
        assert_eq!(index_at(&table, 3), Some(1));
        assert_eq!(index_at(&table, 7), Some(1));
        assert_eq!(index_at(&table, 10), Some(2));
        assert_eq!(index_at(&table, 11), None);
        assert_eq!(index_at(&table, -1), None);
        assert_eq!(index_at(&[0], 0), None);
        assert_eq!(index_at(&[], 0), None);
    }

    #[test]
    fn tiles_are_realized_where_the_tables_put_them() {
        let library = Library::builtin().unwrap();
        let netlist = Netlist::from_yosys_json(TOGGLE, "top", &library).unwrap();
        let (routed, _) = place_and_route(&library, &netlist, crate::place::Style::Pitch4).unwrap();
        let layout = &routed.layout;
        let (cols, rows) = layout.size();
        // Routing columns are narrow, every other column a cell; the flip-flop's
        // row is a cell deep and every other row shallow.
        let (&(col, row), _) = layout.tiles.iter().find(|(_, p)| matches!(p, Placed::Dff { .. })).unwrap();
        assert!(layout.col_width.iter().all(|&w| w == PITCH || w == crate::tile::NARROW));
        assert!(layout.col_width.contains(&crate::tile::NARROW));
        for (r, &depth) in layout.row_depth.iter().enumerate() {
            assert_eq!(depth, if r == row as usize { PITCH } else { crate::tile::SHALLOW }, "row {r}");
        }
        assert!(layout.col_width.len() >= cols as usize && layout.row_depth.len() >= rows as usize);
        let realized = layout.realize(&library, &netlist).unwrap();
        let xs = origins(&layout.col_width[..cols as usize]);
        let zs = origins(&layout.row_depth[..rows as usize]);
        assert_eq!(realized.world.size(), (xs[cols as usize], HEIGHT, zs[rows as usize]));
        // The flip-flop sits at its cell's corner and covers two cells by one.
        let [m] = realized.macros.as_slice() else { panic!("one flip-flop") };
        assert_eq!(m.origin, Pos::new(xs[col as usize], 0, zs[row as usize]));
        assert_eq!((m.width, m.extent), (2, (2 * PITCH, PITCH)));
        assert!(m.contains(m.origin) && m.contains(Pos::new(m.origin.x + 9, 3, m.origin.z + 4)));
        assert!(
            !m.contains(Pos::new(m.origin.x + 10, 0, m.origin.z))
                && !m.contains(Pos::new(m.origin.x, 0, m.origin.z + 5))
        );
        assert!(m.blocks.iter().all(|(p, _)| m.contains(*p)));
        // A layout whose tables do not cover its tiles cannot be realized.
        let short = Layout { tiles: layout.tiles.clone(), upper: layout.upper.clone(), ..Layout::uniform(1, 1) };
        let Err(err) = short.realize(&library, &netlist) else { panic!("a short table must be refused") };
        assert!(err.contains("widths for 1"), "{err}");
    }
}
