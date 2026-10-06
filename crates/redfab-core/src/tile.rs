//! The tile library: the text templates in `cells/`, parsed.
//!
//! See `cells/README.md` for the format and the rules tiles follow.

use std::collections::BTreeMap;

use crate::block::Block;
use crate::geom::{Dir, Pos};

/// Blocks per tile edge, and the height every tile fits in.
pub const PITCH: i32 = 5;
pub const HEIGHT: i32 = 4;

/// One of a tile's four edges.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Edge {
    West,
    East,
    North,
    South,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::West, Edge::East, Edge::North, Edge::South];

    pub fn letter(self) -> char {
        match self {
            Edge::West => 'W',
            Edge::East => 'E',
            Edge::North => 'N',
            Edge::South => 'S',
        }
    }

    pub fn from_letter(c: &str) -> Option<Edge> {
        Edge::ALL.into_iter().find(|e| e.letter().to_string() == c)
    }

    pub fn opposite(self) -> Edge {
        match self {
            Edge::West => Edge::East,
            Edge::East => Edge::West,
            Edge::North => Edge::South,
            Edge::South => Edge::North,
        }
    }

    pub fn dir(self) -> Dir {
        match self {
            Edge::West => Dir::West,
            Edge::East => Dir::East,
            Edge::North => Dir::North,
            Edge::South => Dir::South,
        }
    }

    /// This edge after the tile is turned `quarter_turns` clockwise.
    pub fn rotated(self, quarter_turns: u8) -> Edge {
        (0..quarter_turns % 4).fold(self, |e, _| match e {
            Edge::North => Edge::East,
            Edge::East => Edge::South,
            Edge::South => Edge::West,
            Edge::West => Edge::North,
        })
    }

    /// Step to the neighbouring tile across this edge, as (column, row) deltas.
    pub fn step(self) -> (i32, i32) {
        match self {
            Edge::West => (-1, 0),
            Edge::East => (1, 0),
            Edge::North => (0, -1),
            Edge::South => (0, 1),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PortKind {
    In,
    Out,
    /// Wiring: the signal may flow either way.
    Io,
}

#[derive(Clone, Debug)]
pub struct Port {
    pub edge: Edge,
    pub kind: PortKind,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
struct LegendEntry {
    block: Block,
    /// Present only when this port is in use.
    when: Option<Edge>,
}

#[derive(Clone, Debug)]
pub struct Tile {
    pub name: String,
    /// Width in blocks: a whole number of cells of `PITCH`. Depth is one cell.
    pub width: i32,
    pub ports: Vec<Port>,
    /// `Some("nor")` for a gate, `None` for wiring and I/O.
    pub function: Option<String>,
    /// Game ticks from any input to the output.
    pub delay: u32,
    /// Input tiles: the block tests swap between stone (off) and a redstone block (on).
    pub probe: Option<Pos>,
    /// Output tiles: the lamp to read.
    pub lamp: Option<Pos>,
    /// Flip-flops: the figures measured in stage 8, as `timing` lines in the file,
    /// and which clock edge they act on (`edge rising` or `edge falling`).
    pub timing: BTreeMap<String, i32>,
    pub clock_edge: Option<String>,
    legend: BTreeMap<char, LegendEntry>,
    /// `layers[y][z][x]`
    layers: Vec<Vec<Vec<char>>>,
}

impl Tile {
    pub fn port(&self, edge: Edge) -> Option<&Port> {
        self.ports.iter().find(|p| p.edge == edge)
    }

    /// How many cells wide the tile is.
    pub fn cells(&self) -> i32 {
        self.width / PITCH
    }

    pub fn inputs(&self) -> impl Iterator<Item = &Port> {
        self.ports.iter().filter(|p| p.kind == PortKind::In)
    }

    /// The tile's blocks, relative to its north-west bottom corner, with only the
    /// parts belonging to `used` ports included. `used` names the ports as written
    /// in the file; the result is then turned `rotation` quarter turns clockwise.
    pub fn blocks(&self, used: &[Edge], rotation: u8) -> Vec<(Pos, Block)> {
        let mut out = Vec::new();
        for (y, layer) in self.layers.iter().enumerate() {
            for (z, row) in layer.iter().enumerate() {
                for (x, &c) in row.iter().enumerate() {
                    let Some(entry) = self.legend.get(&c) else { continue };
                    if entry.when.is_some_and(|edge| !used.contains(&edge)) || entry.block.is_air() {
                        continue;
                    }
                    let pos = rotate_pos_in(Pos::new(x as i32, y as i32, z as i32), rotation, self.width);
                    out.push((pos, entry.block.rotated(rotation)));
                }
            }
        }
        out.sort_by_key(|(pos, _)| (pos.y, pos.z, pos.x));
        out
    }

    /// Parses one tile file.
    pub fn parse(text: &str) -> Result<Tile, String> {
        let mut name = None;
        let mut ports = Vec::new();
        let mut function = None;
        let mut delay = 0;
        let mut probe = None;
        let mut lamp = None;
        let mut timing = BTreeMap::new();
        let mut clock_edge = None;
        let mut legend = BTreeMap::new();
        let mut width = PITCH;
        let mut layers: Vec<Vec<Vec<char>>> = vec![vec![vec!['.'; PITCH as usize]; PITCH as usize]; HEIGHT as usize];
        let mut section = Section::Header;

        for (number, raw) in text.lines().enumerate() {
            // '#' is also a block symbol, so comments are whole lines and only
            // allowed in the header, before `legend`.
            if matches!(section, Section::Header) && raw.trim_start().starts_with('#') {
                continue;
            }
            let line = raw.trim_end();
            let at = |msg: String| format!("line {}: {msg}", number + 1);
            let words: Vec<&str> = line.split_whitespace().collect();
            let Some(&first) = words.first() else {
                continue;
            };
            let pos_at = |i: usize| -> Result<Pos, String> {
                let n = |k: usize| words.get(i + k).and_then(|w| w.parse::<i32>().ok());
                match (n(0), n(1), n(2)) {
                    (Some(x), Some(y), Some(z)) => Ok(Pos::new(x, y, z)),
                    _ => Err(at(format!("expected three numbers in {line:?}"))),
                }
            };
            match (first, &section) {
                ("tile", _) => name = Some(words.get(1).ok_or_else(|| at("tile needs a name".into()))?.to_string()),
                ("size", _) => {
                    let size = pos_at(1)?;
                    if size.x % PITCH != 0 || size.x < PITCH || (size.y, size.z) != (HEIGHT, PITCH) {
                        return Err(at(format!(
                            "tiles are a whole number of cells wide: W {HEIGHT} {PITCH} with W a multiple of {PITCH}"
                        )));
                    }
                    width = size.x;
                    layers = vec![vec![vec!['.'; width as usize]; PITCH as usize]; HEIGHT as usize];
                }
                ("timing", _) => {
                    // timing setup 1 hold 0 clk_to_q 8 min_pulse 4
                    for pair in words[1..].chunks(2) {
                        let [key, value] = pair else { return Err(at("timing needs name value pairs".into())) };
                        let value: i32 = value.parse().map_err(|_| at(format!("timing {key}: not a number")))?;
                        timing.insert(key.to_string(), value);
                    }
                }
                ("edge", _) => match words.get(1) {
                    Some(&"rising") | Some(&"falling") => clock_edge = Some(words[1].to_string()),
                    _ => return Err(at("edge needs rising or falling".into())),
                },
                ("port", _) => {
                    let edge = words
                        .get(1)
                        .and_then(|w| Edge::from_letter(w))
                        .ok_or_else(|| at("port needs W, E, N or S".into()))?;
                    let kind = match words.get(2) {
                        Some(&"in") => PortKind::In,
                        Some(&"out") => PortKind::Out,
                        Some(&"io") => PortKind::Io,
                        _ => return Err(at("port needs in, out or io".into())),
                    };
                    ports.push(Port { edge, kind, pos: pos_at(3)? });
                }
                ("function", _) => {
                    function = Some(words.get(1).ok_or_else(|| at("function needs a name".into()))?.to_string())
                }
                ("delay", _) => {
                    delay =
                        words.get(1).and_then(|w| w.parse().ok()).ok_or_else(|| at("delay needs a number".into()))?
                }
                ("probe", _) => probe = Some(pos_at(1)?),
                ("lamp", _) => lamp = Some(pos_at(1)?),
                ("legend", _) => section = Section::Legend,
                ("layer", _) => {
                    let y: usize =
                        words.get(1).and_then(|w| w.parse().ok()).ok_or_else(|| at("layer needs a number".into()))?;
                    if y >= HEIGHT as usize {
                        return Err(at(format!("layer {y} is above the tile height {HEIGHT}")));
                    }
                    section = Section::Layer { y, row: 0 };
                }
                (_, Section::Legend) => {
                    let symbol: Vec<char> = first.chars().collect();
                    if symbol.len() != 1 {
                        return Err(at(format!("legend symbols are one character, got {first:?}")));
                    }
                    let state = words.get(1).ok_or_else(|| at("legend entry needs a block".into()))?;
                    let block: Block = state.parse().map_err(at)?;
                    let when = match (words.get(2), words.get(3)) {
                        (None, _) => None,
                        (Some(&"if"), Some(w)) => {
                            Some(Edge::from_letter(w).ok_or_else(|| at(format!("unknown port {w:?}")))?)
                        }
                        _ => return Err(at(format!("expected 'if <port>' in {line:?}"))),
                    };
                    legend.insert(symbol[0], LegendEntry { block, when });
                }
                (_, Section::Layer { y, row }) => {
                    let (y, row) = (*y, *row);
                    let cells: Vec<char> = first.chars().collect();
                    if cells.len() != width as usize || words.len() != 1 || row >= PITCH as usize {
                        return Err(at(format!("each layer is {PITCH} rows of {width} characters")));
                    }
                    layers[y][row] = cells;
                    section = Section::Layer { y, row: row + 1 };
                }
                _ => return Err(at(format!("unexpected {first:?}"))),
            }
        }

        let tile = Tile {
            name: name.ok_or("missing 'tile <name>'")?,
            width,
            ports,
            function,
            delay,
            probe,
            lamp,
            timing,
            clock_edge,
            legend,
            layers,
        };
        tile.validate()?;
        Ok(tile)
    }

    /// Checks the rules from `cells/README.md` that can be checked on one tile alone.
    fn validate(&self) -> Result<(), String> {
        let last = PITCH - 1;
        let mid = PITCH / 2;
        for port in &self.ports {
            // North and south ports sit on the first cell of a wide tile.
            let expected = match port.edge {
                Edge::West => Pos::new(0, 1, mid),
                Edge::East => Pos::new(self.width - 1, 1, mid),
                Edge::North => Pos::new(mid, 1, 0),
                Edge::South => Pos::new(mid, 1, last),
            };
            if port.pos != expected {
                return Err(format!("{}: port {} must be at {expected}", self.name, port.edge.letter()));
            }
        }
        let all: Vec<Edge> = self.ports.iter().map(|p| p.edge).collect();
        let port_cells: Vec<Pos> = self.ports.iter().map(|p| p.pos).collect();
        for (y, layer) in self.layers.iter().enumerate() {
            for &c in layer.iter().flatten() {
                if c != '.' && !self.legend.contains_key(&c) {
                    return Err(format!("{}: symbol {c:?} at layer {y} is not in the legend", self.name));
                }
            }
        }
        for (pos, block) in self.blocks(&all, 0) {
            let on_border = pos.x == 0 || pos.x == self.width - 1 || pos.z == 0 || pos.z == last;
            if !on_border || pos.y == 0 {
                continue; // the floor may reach the border; it is glass
            }
            let allowed = port_cells.contains(&pos) && matches!(block, Block::Wire(_));
            if !allowed {
                return Err(format!("{}: {} at {pos} touches the tile border", self.name, block.name()));
            }
        }
        for (pos, block) in self.blocks(&all, 0) {
            if pos.y == 0 && block != Block::Glass {
                return Err(format!("{}: the floor must be glass, found {} at {pos}", self.name, block.name()));
            }
        }
        Ok(())
    }
}

/// A position inside a one-cell tile, after the tile is turned clockwise about its centre.
pub fn rotate_pos(pos: Pos, quarter_turns: u8) -> Pos {
    rotate_pos_in(pos, quarter_turns, PITCH)
}

/// The same for a tile `width` blocks wide and one cell deep: each quarter turn
/// swaps the footprint's sides, so after one the tile is a cell wide and `width` deep.
pub fn rotate_pos_in(pos: Pos, quarter_turns: u8, width: i32) -> Pos {
    let mut depth = PITCH;
    let mut width = width;
    (0..quarter_turns % 4).fold(pos, |p, _| {
        let turned = Pos::new(depth - 1 - p.z, p.y, p.x);
        std::mem::swap(&mut width, &mut depth);
        turned
    })
}

enum Section {
    Header,
    Legend,
    Layer { y: usize, row: usize },
}

/// Every tile, parsed from the files in `cells/`.
pub struct Library {
    pub input: Tile,
    pub output: Tile,
    pub wire: Tile,
    pub nor: Tile,
    pub cross: Tile,
    pub tie_high: Tile,
    pub tie_low: Tile,
    /// The flip-flop, and the other candidate characterized in stage 8.
    pub dff: Tile,
    pub dff_alt: Tile,
}

impl Library {
    /// The built-in library. The tile files are compiled in, so the same library
    /// is available in a browser.
    pub fn builtin() -> Result<Library, String> {
        let parse = |name: &str, text: &str| Tile::parse(text).map_err(|e| format!("cells/{name}.tile: {e}"));
        Ok(Library {
            input: parse("input", include_str!("../../../cells/input.tile"))?,
            output: parse("output", include_str!("../../../cells/output.tile"))?,
            wire: parse("wire", include_str!("../../../cells/wire.tile"))?,
            nor: parse("nor", include_str!("../../../cells/nor.tile"))?,
            cross: parse("cross", include_str!("../../../cells/cross.tile"))?,
            tie_high: parse("tie_high", include_str!("../../../cells/tie_high.tile"))?,
            tie_low: parse("tie_low", include_str!("../../../cells/tie_low.tile"))?,
            dff: parse("dff", include_str!("../../../cells/dff.tile"))?,
            dff_alt: parse("dff_alt", include_str!("../../../cells/dff_alt.tile"))?,
        })
    }

    /// The synthesis cells this library offers, as (name, input pins) pairs.
    /// `NOT`, `NOR2` and `NOR3` are the NOR tile with one to three inputs.
    pub fn cells(&self) -> Vec<(&'static str, Vec<&'static str>)> {
        vec![("NOT", vec!["A"]), ("NOR2", vec!["A", "B"]), ("NOR3", vec!["A", "B", "C"])]
    }

    /// The cell library in Liberty format, for Yosys's `abc -liberty`.
    ///
    /// `BUF` is listed because the mapper refuses a library without a buffer; it
    /// is priced so it is never preferred, and a buffer in a netlist is realised
    /// as plain wire.
    pub fn liberty(&self) -> String {
        let mut out = String::from("library(redfab) {\n");
        out.push_str("  cell(BUF) {\n    area: 4;\n    pin(A) { direction: input; }\n    pin(Y) { direction: output; function: \"A\"; }\n  }\n");
        for (name, pins) in self.cells() {
            out.push_str(&format!("  cell({name}) {{\n    area: 1;\n"));
            for pin in &pins {
                out.push_str(&format!("    pin({pin}) {{ direction: input; }}\n"));
            }
            out.push_str(&format!("    pin(Y) {{ direction: output; function: \"!({})\"; }}\n  }}\n", pins.join("+")));
        }
        out.push_str("}\n");
        out
    }

    /// Verilog models of the same cells, for the equivalence proof and for
    /// simulating a mapped netlist.
    pub fn verilog_models(&self) -> String {
        let mut out = String::from("module BUF(input A, output Y);\n  assign Y = A;\nendmodule\n");
        for (name, pins) in self.cells() {
            let ports: Vec<String> = pins.iter().map(|p| format!("input {p}")).collect();
            out.push_str(&format!(
                "module {name}({}, output Y);\n  assign Y = ~({});\nendmodule\n",
                ports.join(", "),
                pins.join(" | ")
            ));
        }
        out
    }
}

/// Which tile.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    Input,
    Output,
    Wire,
    Nor,
    Cross,
    TieHigh,
    TieLow,
    /// The flip-flop: D on the west, the clock on the north, Q on the east.
    Dff,
    /// The other flip-flop candidate, kept for its characterization.
    DffAlt,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Input,
        Kind::Output,
        Kind::Wire,
        Kind::Nor,
        Kind::Cross,
        Kind::TieHigh,
        Kind::TieLow,
        Kind::Dff,
        Kind::DffAlt,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Input => "input",
            Kind::Output => "output",
            Kind::Wire => "wire",
            Kind::Nor => "nor",
            Kind::Cross => "cross",
            Kind::TieHigh => "tie_high",
            Kind::TieLow => "tie_low",
            Kind::Dff => "dff",
            Kind::DffAlt => "dff_alt",
        }
    }
}

/// One concrete way to use a tile: which of its ports are used, how it is turned,
/// and for a wire whether it carries a repeater.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Variant {
    pub kind: Kind,
    /// Used ports, named as in the tile file (before rotation).
    pub edges: Vec<Edge>,
    pub rotation: u8,
    /// For a wire: the edge the signal enters by when the tile carries a repeater
    /// (named before rotation). The repeater replaces the dust next to that port.
    pub repeater_entry: Option<Edge>,
    /// Repeater setting 1 to 4; ignored without a repeater.
    pub delay: u8,
}

/// A port of a placed variant, in world orientation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VariantPort {
    pub edge: Edge,
    pub kind: PortKind,
    /// Position inside the tile.
    pub pos: Pos,
    /// Which of the tile's internal nets the port belongs to (ports on one net are joined).
    pub net: u8,
}

impl Variant {
    pub fn new(kind: Kind, edges: &[Edge]) -> Variant {
        Variant { kind, edges: edges.to_vec(), rotation: 0, repeater_entry: None, delay: 1 }
    }

    /// A wire joining `edges` with a repeater in the arm of `entry`.
    pub fn repeater(edges: &[Edge], entry: Edge, delay: u8) -> Variant {
        Variant { kind: Kind::Wire, edges: edges.to_vec(), rotation: 0, repeater_entry: Some(entry), delay }
    }

    pub fn tile<'a>(&self, library: &'a Library) -> &'a Tile {
        match self.kind {
            Kind::Input => &library.input,
            Kind::Output => &library.output,
            Kind::Wire => &library.wire,
            Kind::Nor => &library.nor,
            Kind::Cross => &library.cross,
            Kind::TieHigh => &library.tie_high,
            Kind::TieLow => &library.tie_low,
            Kind::Dff => &library.dff,
            Kind::DffAlt => &library.dff_alt,
        }
    }

    /// Width in cells: one, or two for a flip-flop.
    pub fn cells(&self, library: &Library) -> i32 {
        self.tile(library).cells()
    }

    /// A short name, e.g. `wire_WS`, `nor_WN`, `wire_WE_repW_d3`.
    pub fn name(&self) -> String {
        let mut name = self.kind.name().to_string();
        if matches!(self.kind, Kind::Wire | Kind::Nor) {
            name.push('_');
            name.extend(self.edges.iter().map(|e| e.letter()));
        }
        if let Some(entry) = self.repeater_entry {
            name.push_str(&format!("_rep{}_d{}", entry.letter(), self.delay));
        }
        if self.rotation != 0 {
            name.push_str(&format!("_r{}", self.rotation));
        }
        name
    }

    /// The blocks, rotated, with the repeater (if any) in place.
    pub fn blocks(&self, library: &Library) -> Vec<(Pos, Block)> {
        let mut blocks = self.tile(library).blocks(&self.edges, self.rotation);
        if let Some(entry) = self.repeater_entry {
            // The cell next to the entry port, one step towards the centre.
            let port = self.tile(library).port(entry).expect("the repeater's entry is a port of the tile");
            let inner = rotate_pos_in(port.pos.offset(entry.dir().opposite()), self.rotation, self.tile(library).width);
            let facing = entry.rotated(self.rotation).dir();
            for (pos, block) in &mut blocks {
                if *pos == inner {
                    *block = Block::Repeater { facing, delay: self.delay, locked: false, powered: false };
                }
            }
        }
        blocks
    }

    /// The used ports after rotation, with their internal net numbers.
    pub fn ports(&self, library: &Library) -> Vec<VariantPort> {
        let tile = self.tile(library);
        tile.ports
            .iter()
            .filter(|p| self.edges.contains(&p.edge))
            .map(|p| {
                let net = match self.kind {
                    // Each NOR input is its own net; the output is another.
                    Kind::Nor => match p.edge {
                        Edge::West => 0,
                        Edge::North => 1,
                        Edge::South => 2,
                        Edge::East => 3,
                    },
                    // West-east and north-south never meet.
                    Kind::Cross => u8::from(matches!(p.edge, Edge::North | Edge::South)),
                    // D, the clock and Q are three nets.
                    Kind::Dff | Kind::DffAlt => match p.edge {
                        Edge::West => 0,
                        Edge::North | Edge::South => 1,
                        Edge::East => 2,
                    },
                    _ => 0,
                };
                let kind = match (self.repeater_entry, p.kind) {
                    // With a repeater the entry port is an input and the ports it feeds
                    // are outputs; in a crossing the other axis is unaffected.
                    (Some(entry), PortKind::Io) if self.kind == Kind::Cross => {
                        if p.edge == entry {
                            PortKind::In
                        } else if p.edge == entry.opposite() {
                            PortKind::Out
                        } else {
                            PortKind::Io
                        }
                    }
                    (Some(entry), PortKind::Io) => {
                        if p.edge == entry {
                            PortKind::In
                        } else {
                            PortKind::Out
                        }
                    }
                    (_, kind) => kind,
                };
                VariantPort {
                    edge: p.edge.rotated(self.rotation),
                    kind,
                    pos: rotate_pos_in(p.pos, self.rotation, tile.width),
                    net,
                }
            })
            .collect()
    }

    /// Game ticks from an input change to the output change.
    pub fn delay_ticks(&self, library: &Library) -> u32 {
        match self.repeater_entry {
            Some(_) => 2 * u32::from(self.delay),
            None => self.tile(library).delay,
        }
    }

    /// Every variant the library offers: wiring with every combination of two or
    /// more edges, each of those with a repeater on each possible entry, NOR with
    /// every non-empty set of inputs, and the fixed tiles.
    pub fn all() -> Vec<Variant> {
        let mut out = vec![Variant::new(Kind::Input, &[Edge::East]), Variant::new(Kind::Output, &[Edge::West])];
        for mask in 1u8..16 {
            let edges: Vec<Edge> =
                Edge::ALL.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, e)| *e).collect();
            if edges.len() >= 2 {
                out.push(Variant::new(Kind::Wire, &edges));
                for &entry in &edges {
                    out.push(Variant::repeater(&edges, entry, 1));
                }
            }
        }
        for delay in 2..=4 {
            out.push(Variant::repeater(&[Edge::West, Edge::East], Edge::West, delay));
        }
        for mask in 1u8..8 {
            let inputs = [Edge::West, Edge::North, Edge::South];
            let mut edges: Vec<Edge> =
                inputs.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, e)| *e).collect();
            edges.push(Edge::East);
            out.push(Variant::new(Kind::Nor, &edges));
        }
        out.push(Variant::new(Kind::Cross, &Edge::ALL));
        for entry in [Edge::West, Edge::East] {
            out.push(Variant {
                kind: Kind::Cross,
                edges: Edge::ALL.to_vec(),
                rotation: 0,
                repeater_entry: Some(entry),
                delay: 1,
            });
        }
        out.push(Variant::new(Kind::Dff, &[Edge::West, Edge::North, Edge::East]));
        out.push(Variant::new(Kind::DffAlt, &[Edge::West, Edge::North, Edge::East]));
        out.push(Variant::new(Kind::TieHigh, &[Edge::East]));
        out.push(Variant::new(Kind::TieLow, &[Edge::East]));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_library_parses_and_obeys_the_border_rules() {
        Library::builtin().unwrap();
    }

    #[test]
    fn unused_ports_leave_their_dust_out() {
        let lib = Library::builtin().unwrap();
        let wires = |used: &[Edge]| lib.nor.blocks(used, 0).iter().filter(|(_, b)| matches!(b, Block::Wire(_))).count();
        assert_eq!(wires(&[Edge::West, Edge::North, Edge::South, Edge::East]), 7);
        assert_eq!(wires(&[Edge::West, Edge::East]), 3);
        assert_eq!(wires(&[Edge::East]), 1);
    }

    #[test]
    fn a_block_on_the_border_is_rejected() {
        let text = include_str!("../../../cells/output.tile").replace("---L.", "---.L");
        let err = Tile::parse(&text).unwrap_err();
        assert!(err.contains("touches the tile border"), "{err}");
    }

    #[test]
    fn a_repeater_sits_in_the_entry_arm_facing_its_port() {
        let lib = Library::builtin().unwrap();
        // A west-to-south turn with a repeater on the west arm, turned a quarter: north-to-west.
        let turn = Variant { rotation: 1, ..Variant::repeater(&[Edge::West, Edge::South], Edge::West, 2) };
        let ports = turn.ports(&lib);
        assert_eq!(
            ports.iter().map(|p| (p.edge, p.kind)).collect::<Vec<_>>(),
            vec![(Edge::North, PortKind::In), (Edge::West, PortKind::Out)]
        );
        let repeater = turn.blocks(&lib).into_iter().find(|(_, b)| matches!(b, Block::Repeater { .. })).unwrap();
        assert_eq!(repeater.1.to_string(), "minecraft:repeater[delay=2,facing=north,locked=false,powered=false]");
        assert_eq!(repeater.0, Pos::new(2, 1, 1));
        assert_eq!(turn.blocks(&lib).iter().filter(|(_, b)| matches!(b, Block::Wire(_))).count(), 4);
        // 2 io tiles, 11 wires + 28 repeater placements + 3 more delays, 7 NORs, 3 crossings, 2 ties
        assert_eq!(Variant::all().len(), 2 + 11 + 28 + 3 + 7 + 3 + 2 + 2);
    }

    #[test]
    fn liberty_and_verilog_list_the_same_cells() {
        let lib = Library::builtin().unwrap();
        for (name, _) in lib.cells() {
            assert!(lib.liberty().contains(&format!("cell({name})")));
            assert!(lib.verilog_models().contains(&format!("module {name}(")));
        }
    }
}
