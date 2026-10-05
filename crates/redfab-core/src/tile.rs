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
    pub ports: Vec<Port>,
    /// `Some("nor")` for a gate, `None` for wiring and I/O.
    pub function: Option<String>,
    /// Game ticks from any input to the output.
    pub delay: u32,
    /// Input tiles: the block tests swap between stone (off) and a redstone block (on).
    pub probe: Option<Pos>,
    /// Output tiles: the lamp to read.
    pub lamp: Option<Pos>,
    legend: BTreeMap<char, LegendEntry>,
    /// `layers[y][z][x]`
    layers: Vec<Vec<Vec<char>>>,
}

impl Tile {
    pub fn port(&self, edge: Edge) -> Option<&Port> {
        self.ports.iter().find(|p| p.edge == edge)
    }

    pub fn inputs(&self) -> impl Iterator<Item = &Port> {
        self.ports.iter().filter(|p| p.kind == PortKind::In)
    }

    /// The tile's blocks, relative to its north-west bottom corner, with only the
    /// parts belonging to `used` ports included.
    pub fn blocks(&self, used: &[Edge]) -> Vec<(Pos, Block)> {
        let mut out = Vec::new();
        for (y, layer) in self.layers.iter().enumerate() {
            for (z, row) in layer.iter().enumerate() {
                for (x, &c) in row.iter().enumerate() {
                    let Some(entry) = self.legend.get(&c) else {
                        continue;
                    };
                    if entry.when.is_some_and(|edge| !used.contains(&edge)) || entry.block.is_air()
                    {
                        continue;
                    }
                    out.push((Pos::new(x as i32, y as i32, z as i32), entry.block));
                }
            }
        }
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
        let mut legend = BTreeMap::new();
        let mut layers: Vec<Vec<Vec<char>>> =
            vec![vec![vec!['.'; PITCH as usize]; PITCH as usize]; HEIGHT as usize];
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
                ("tile", _) => {
                    name = Some(
                        words
                            .get(1)
                            .ok_or_else(|| at("tile needs a name".into()))?
                            .to_string(),
                    )
                }
                ("size", _) => {
                    let size = pos_at(1)?;
                    if (size.x, size.y, size.z) != (PITCH, HEIGHT, PITCH) {
                        return Err(at(format!("tiles must be {PITCH} {HEIGHT} {PITCH}")));
                    }
                }
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
                    ports.push(Port {
                        edge,
                        kind,
                        pos: pos_at(3)?,
                    });
                }
                ("function", _) => {
                    function = Some(
                        words
                            .get(1)
                            .ok_or_else(|| at("function needs a name".into()))?
                            .to_string(),
                    )
                }
                ("delay", _) => {
                    delay = words
                        .get(1)
                        .and_then(|w| w.parse().ok())
                        .ok_or_else(|| at("delay needs a number".into()))?
                }
                ("probe", _) => probe = Some(pos_at(1)?),
                ("lamp", _) => lamp = Some(pos_at(1)?),
                ("legend", _) => section = Section::Legend,
                ("layer", _) => {
                    let y: usize = words
                        .get(1)
                        .and_then(|w| w.parse().ok())
                        .ok_or_else(|| at("layer needs a number".into()))?;
                    if y >= HEIGHT as usize {
                        return Err(at(format!("layer {y} is above the tile height {HEIGHT}")));
                    }
                    section = Section::Layer { y, row: 0 };
                }
                (_, Section::Legend) => {
                    let symbol: Vec<char> = first.chars().collect();
                    if symbol.len() != 1 {
                        return Err(at(format!(
                            "legend symbols are one character, got {first:?}"
                        )));
                    }
                    let state = words
                        .get(1)
                        .ok_or_else(|| at("legend entry needs a block".into()))?;
                    let block: Block = state.parse().map_err(at)?;
                    let when = match (words.get(2), words.get(3)) {
                        (None, _) => None,
                        (Some(&"if"), Some(w)) => Some(
                            Edge::from_letter(w)
                                .ok_or_else(|| at(format!("unknown port {w:?}")))?,
                        ),
                        _ => return Err(at(format!("expected 'if <port>' in {line:?}"))),
                    };
                    legend.insert(symbol[0], LegendEntry { block, when });
                }
                (_, Section::Layer { y, row }) => {
                    let (y, row) = (*y, *row);
                    let cells: Vec<char> = first.chars().collect();
                    if cells.len() != PITCH as usize || words.len() != 1 || row >= PITCH as usize {
                        return Err(at(format!(
                            "each layer is {PITCH} rows of {PITCH} characters"
                        )));
                    }
                    layers[y][row] = cells;
                    section = Section::Layer { y, row: row + 1 };
                }
                _ => return Err(at(format!("unexpected {first:?}"))),
            }
        }

        let tile = Tile {
            name: name.ok_or("missing 'tile <name>'")?,
            ports,
            function,
            delay,
            probe,
            lamp,
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
            let expected = match port.edge {
                Edge::West => Pos::new(0, 1, mid),
                Edge::East => Pos::new(last, 1, mid),
                Edge::North => Pos::new(mid, 1, 0),
                Edge::South => Pos::new(mid, 1, last),
            };
            if port.pos != expected {
                return Err(format!(
                    "{}: port {} must be at {expected}",
                    self.name,
                    port.edge.letter()
                ));
            }
        }
        let all: Vec<Edge> = self.ports.iter().map(|p| p.edge).collect();
        let port_cells: Vec<Pos> = self.ports.iter().map(|p| p.pos).collect();
        for (y, layer) in self.layers.iter().enumerate() {
            for &c in layer.iter().flatten() {
                if c != '.' && !self.legend.contains_key(&c) {
                    return Err(format!(
                        "{}: symbol {c:?} at layer {y} is not in the legend",
                        self.name
                    ));
                }
            }
        }
        for (pos, block) in self.blocks(&all) {
            let on_border = pos.x == 0 || pos.x == last || pos.z == 0 || pos.z == last;
            if !on_border || pos.y == 0 {
                continue; // the floor may reach the border; it is glass
            }
            let allowed = port_cells.contains(&pos) && matches!(block, Block::Wire(_));
            let io_block = Some(pos) == self.probe || Some(pos.down()) == self.probe;
            if !allowed && !io_block {
                return Err(format!(
                    "{}: {} at {pos} touches the tile border",
                    self.name,
                    block.name()
                ));
            }
        }
        for (pos, block) in self.blocks(&all) {
            if pos.y == 0 && block != Block::Glass {
                return Err(format!(
                    "{}: the floor must be glass, found {} at {pos}",
                    self.name,
                    block.name()
                ));
            }
        }
        Ok(())
    }
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
}

impl Library {
    /// The built-in library. The tile files are compiled in, so the same library
    /// is available in a browser.
    pub fn builtin() -> Result<Library, String> {
        let parse = |name: &str, text: &str| {
            Tile::parse(text).map_err(|e| format!("cells/{name}.tile: {e}"))
        };
        Ok(Library {
            input: parse("input", include_str!("../../../cells/input.tile"))?,
            output: parse("output", include_str!("../../../cells/output.tile"))?,
            wire: parse("wire", include_str!("../../../cells/wire.tile"))?,
            nor: parse("nor", include_str!("../../../cells/nor.tile"))?,
        })
    }

    /// The synthesis cells this library offers, as (name, input pins) pairs.
    /// `NOT`, `NOR2` and `NOR3` are the NOR tile with one to three inputs.
    pub fn cells(&self) -> Vec<(&'static str, Vec<&'static str>)> {
        vec![
            ("NOT", vec!["A"]),
            ("NOR2", vec!["A", "B"]),
            ("NOR3", vec!["A", "B", "C"]),
        ]
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
            out.push_str(&format!(
                "    pin(Y) {{ direction: output; function: \"!({})\"; }}\n  }}\n",
                pins.join("+")
            ));
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
        let wires = |used: &[Edge]| {
            lib.nor
                .blocks(used)
                .iter()
                .filter(|(_, b)| matches!(b, Block::Wire(_)))
                .count()
        };
        assert_eq!(
            wires(&[Edge::West, Edge::North, Edge::South, Edge::East]),
            7
        );
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
    fn liberty_and_verilog_list_the_same_cells() {
        let lib = Library::builtin().unwrap();
        for (name, _) in lib.cells() {
            assert!(lib.liberty().contains(&format!("cell({name})")));
            assert!(lib.verilog_models().contains(&format!("module {name}(")));
        }
    }
}
