//! The tile library: the text templates in `cells/`, parsed.
//!
//! See `cells/README.md` for the format and the rules tiles follow.

use std::collections::{BTreeMap, VecDeque};

use crate::block::Block;
use crate::geom::{Dir, Pos};
use crate::sim::Sim;
use crate::world::World;

/// Blocks per tile edge, and the height every tile fits in.
pub const PITCH: i32 = 5;
pub const HEIGHT: i32 = 4;
/// The width of a routing column: the narrow wiring tiles are this wide and
/// `PITCH` deep.
pub const NARROW: i32 = 3;

/// The power a torch, a powered repeater or a powered block gives the dust beside it.
const FULL_POWER: u8 = 15;

/// The heights of the two wire levels: a port's dust is at one of them. The
/// lower dust stands on glass, the upper on stone.
pub const LOWER_Y: i32 = 1;
pub const UPPER_Y: i32 = 3;

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
    /// Width and depth in blocks: a whole number of cells of `PITCH` wide, and
    /// one cell deep.
    pub width: i32,
    pub depth: i32,
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
    /// Blocks whose state at rest (every input off, the clock low) differs from
    /// the legend's: `rest X Y Z <state>` lines.
    pub rest: Vec<(Pos, Block)>,
    /// Flip-flops: the repeaters whose `powered` holds the stored value (`state X Y Z`).
    pub state: Vec<Pos>,
    legend: BTreeMap<char, LegendEntry>,
    /// `layers[y][z][x]`
    layers: Vec<Vec<Vec<char>>>,
    /// The strength model read off the dust once the file is parsed; see
    /// [`Tile::dust_steps`], [`Tile::repeater_out`], [`Tile::repeater_needs`],
    /// [`Tile::repeater_place`], [`Tile::source_out`] and [`Tile::port_needs`].
    steps: BTreeMap<(Edge, Edge), u8>,
    repeater_outs: BTreeMap<(Edge, Edge), u8>,
    repeater_needs: BTreeMap<(Edge, Edge), u8>,
    /// Per entry and exit: where a repeater carrying the signal between them
    /// stands, and the edge its input faces.
    repeater_places: BTreeMap<(Edge, Edge), (Pos, Edge)>,
    source_outs: BTreeMap<Edge, u8>,
    needs: BTreeMap<Edge, u8>,
}

impl Tile {
    pub fn port(&self, edge: Edge) -> Option<&Port> {
        self.ports.iter().find(|p| p.edge == edge)
    }

    /// How many cells wide the tile is: one for a narrow tile.
    pub fn cells(&self) -> i32 {
        (self.width + PITCH - 1) / PITCH
    }

    /// What a signal loses crossing the tile from the `entry` port to the port of
    /// the neighbouring tile beyond `exit`: the dust steps from port to port, found
    /// by breadth-first search over the file's dust with the game's wire adjacency,
    /// plus the one step across the border. `None` when no dust joins the two ports.
    pub fn dust_steps(&self, entry: Edge, exit: Edge) -> Option<u8> {
        self.steps.get(&(entry, exit)).copied()
    }

    /// The power at the port of the neighbouring tile beyond `exit` when a
    /// repeater carries the signal from `entry` (where [`Tile::repeater_place`]
    /// puts it): full power at the dust in front of the repeater, one less per
    /// step. `None` when no repeater can.
    pub fn repeater_out(&self, entry: Edge, exit: Edge) -> Option<u8> {
        self.repeater_outs.get(&(entry, exit)).copied()
    }

    /// What the `entry` port must carry for that repeater to switch: one more
    /// than the dust steps from the port to the dust behind the repeater (one
    /// in its entry arm, where it reads the port's own dust).
    pub fn repeater_needs(&self, entry: Edge, exit: Edge) -> Option<u8> {
        self.repeater_needs.get(&(entry, exit)).copied()
    }

    /// Where a repeater carrying the signal from `entry` to every one of
    /// `exits` stands, before rotation, and the edge its input faces: the dust
    /// next to the entry port, the entry arm's own, when the dust in front of it
    /// leads on to every exit; where that cell is the centre the arms meet in
    /// (the west and east arms of a narrow tile), a straight still carries it
    /// there, and a turn carries it in the arm it leaves by, next to that port,
    /// reading the centre. `None` when no one repeater serves every exit, as
    /// for a fan-out from such a centre.
    pub fn repeater_place(&self, entry: Edge, exits: &[Edge]) -> Option<(Pos, Edge)> {
        let mut places = exits.iter().map(|&exit| self.repeater_places.get(&(entry, exit)).copied());
        let first = places.next()??;
        places.all(|p| p == Some(first)).then_some(first)
    }

    /// The power at the port of the neighbouring tile beyond `exit` from the
    /// tile's own driver (a torch, a repeater or the probe block), when it is on:
    /// full power at the dust it powers, one less per step. Zero for a tile that
    /// drives nothing that way.
    pub fn source_out(&self, exit: Edge) -> u8 {
        self.source_outs.get(&exit).copied().unwrap_or(0)
    }

    /// What an input port must carry for every dust behind it to be powered: one
    /// more than the steps to the farthest dust the port's dust reaches, so that
    /// the part at the end of the arm sees at least one level. `None` for a port
    /// that is not an input.
    pub fn port_needs(&self, entry: Edge) -> Option<u8> {
        self.needs.get(&entry).copied()
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
                    let pos = rotate_pos_in(Pos::new(x as i32, y as i32, z as i32), rotation, self.width, self.depth);
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
        let mut rest = Vec::new();
        let mut state = Vec::new();
        let mut legend = BTreeMap::new();
        let mut width = PITCH;
        let mut depth = PITCH;
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
                    if (size.x % PITCH != 0 && size.x != NARROW)
                        || size.x < NARROW
                        || (size.y, size.z) != (HEIGHT, PITCH)
                    {
                        return Err(at(format!(
                            "tiles are {NARROW} wide or a whole number of cells: W {HEIGHT} {PITCH} with W {NARROW} or a multiple of {PITCH}"
                        )));
                    }
                    width = size.x;
                    depth = size.z;
                    layers = vec![vec![vec!['.'; width as usize]; depth as usize]; HEIGHT as usize];
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
                ("rest", _) => {
                    let pos = pos_at(1)?;
                    let block: Block =
                        words.get(4).ok_or_else(|| at("rest needs a block state".into()))?.parse().map_err(at)?;
                    rest.push((pos, block));
                }
                ("state", _) => state.push(pos_at(1)?),
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

        let mut tile = Tile {
            name: name.ok_or("missing 'tile <name>'")?,
            width,
            depth,
            ports,
            function,
            delay,
            probe,
            lamp,
            timing,
            clock_edge,
            rest,
            state,
            legend,
            layers,
            steps: BTreeMap::new(),
            repeater_outs: BTreeMap::new(),
            repeater_needs: BTreeMap::new(),
            repeater_places: BTreeMap::new(),
            source_outs: BTreeMap::new(),
            needs: BTreeMap::new(),
        };
        tile.validate()?;
        tile.measure();
        Ok(tile)
    }

    /// The tile's blocks in a world of their own, every port used; with `on`, the
    /// drivers are on: torches lit, repeaters powered, the probe a redstone block.
    fn world(&self, on: bool) -> World {
        let all: Vec<Edge> = self.ports.iter().map(|p| p.edge).collect();
        let mut world = World::new((self.width, HEIGHT, self.depth));
        for (pos, block) in self.blocks(&all, 0) {
            let block = match block {
                Block::Torch { .. } if on => Block::Torch { lit: true },
                Block::WallTorch { facing, .. } if on => Block::WallTorch { facing, lit: true },
                Block::Repeater { facing, delay, locked, .. } if on => {
                    Block::Repeater { facing, delay, locked, powered: true }
                }
                other => other,
            };
            world.set(pos, block);
        }
        if on && let Some(probe) = self.probe {
            world.set(probe, Block::RedstoneBlock);
        }
        world
    }

    /// Fills the strength tables from the dust: every number the router needs
    /// about this tile, read off the file by the simulator's own wire rules.
    fn measure(&mut self) {
        let quiet = Sim::new(self.world(false));
        let wires: Vec<Pos> =
            quiet.world().iter().filter(|(_, b)| matches!(b, Block::Wire(_))).map(|(p, _)| p).collect();
        // Per dust, the dust it hands power to.
        let mut feeds: BTreeMap<Pos, Vec<Pos>> = BTreeMap::new();
        for &wire in &wires {
            for from in quiet.wire_neighbors(wire) {
                feeds.entry(from).or_default().push(wire);
            }
        }
        // Steps from the nearest of `from` to every dust reached, `skip` left out.
        let distances = |from: &[Pos], skip: Option<Pos>| -> BTreeMap<Pos, u8> {
            let mut dist: BTreeMap<Pos, u8> = BTreeMap::new();
            let mut queue: VecDeque<Pos> = VecDeque::new();
            for &start in from {
                if wires.contains(&start) && Some(start) != skip && !dist.contains_key(&start) {
                    dist.insert(start, 0);
                    queue.push_back(start);
                }
            }
            while let Some(pos) = queue.pop_front() {
                let d = dist[&pos];
                for &next in feeds.get(&pos).map_or(&[][..], Vec::as_slice) {
                    if Some(next) != skip && !dist.contains_key(&next) {
                        dist.insert(next, d + 1);
                        queue.push_back(next);
                    }
                }
            }
            dist
        };
        let ports: Vec<(Edge, Pos, PortKind)> = self.ports.iter().map(|p| (p.edge, p.pos, p.kind)).collect();
        for &(entry, entry_pos, kind) in &ports {
            let from_port = distances(&[entry_pos], None);
            // The repeater takes the dust next to the port and powers the dust in
            // front of it; an arm without those two dust cannot carry one.
            let inner = entry_pos.offset(entry.dir().opposite());
            let front = inner.offset(entry.dir().opposite());
            let from_repeater = if wires.contains(&inner) && wires.contains(&front) {
                distances(&[front], Some(inner))
            } else {
                BTreeMap::new()
            };
            for &(exit, exit_pos, _) in ports.iter().filter(|(e, _, _)| *e != entry) {
                if let Some(&d) = from_port.get(&exit_pos) {
                    self.steps.insert((entry, exit), d + 1);
                }
                if let Some(&d) = from_repeater.get(&exit_pos) {
                    self.repeater_outs.insert((entry, exit), FULL_POWER.saturating_sub(d + 1));
                    self.repeater_needs.insert((entry, exit), 1);
                    self.repeater_places.insert((entry, exit), (inner, entry));
                    continue;
                }
                // Otherwise the dust next to the exit port, reading the dust
                // behind it, which the entry must reach around it.
                let last = exit_pos.offset(exit.dir().opposite());
                let behind = last.offset(exit.dir().opposite());
                if !wires.contains(&last) || !wires.contains(&behind) {
                    continue;
                }
                if let Some(&d) = distances(&[entry_pos], Some(last)).get(&behind) {
                    self.repeater_outs.insert((entry, exit), FULL_POWER - 1);
                    self.repeater_needs.insert((entry, exit), d + 1);
                    self.repeater_places.insert((entry, exit), (last, exit.opposite()));
                }
            }
            if kind == PortKind::In {
                self.needs.insert(entry, 1 + from_port.values().copied().max().unwrap_or(0));
            }
        }
        // The drivers on: the dust they power at full strength, and what is left
        // of that at each port.
        let on = Sim::new(self.world(true));
        for &(exit, exit_pos, _) in &ports {
            let mut best = 0u8;
            for &wire in &wires {
                let power = on.wire_block_signal(wire);
                if power == 0 {
                    continue;
                }
                if let Some(&d) = distances(&[wire], None).get(&exit_pos) {
                    best = best.max(power.saturating_sub(d + 1));
                }
            }
            if best > 0 {
                self.source_outs.insert(exit, best);
            }
        }
    }

    /// Checks the rules from `cells/README.md` that can be checked on one tile alone.
    fn validate(&self) -> Result<(), String> {
        let last = PITCH - 1;
        let mid = PITCH / 2;
        // North and south ports at the middle of the first cell: of the tile, when it is narrow.
        let mid_x = self.width.min(PITCH) / 2;
        for port in &self.ports {
            // North and south ports sit on the first cell of a wide tile; a port is
            // on the lower wire level (y = 1) or the upper one (y = 3).
            let y = if port.pos.y == UPPER_Y { UPPER_Y } else { LOWER_Y };
            let expected = match port.edge {
                Edge::West => Pos::new(0, y, mid),
                Edge::East => Pos::new(self.width - 1, y, mid),
                Edge::North => Pos::new(mid_x, y, 0),
                Edge::South => Pos::new(mid_x, y, last),
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
            // A port's dust, and the stone an upper port's dust stands on.
            let allowed = (port_cells.contains(&pos) && matches!(block, Block::Wire(_)))
                || (block == Block::Stone && pos.y == UPPER_Y - 1 && port_cells.contains(&pos.up()));
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
    rotate_pos_in(pos, quarter_turns, PITCH, PITCH)
}

/// The same for a tile `width` blocks wide and `depth` deep: each quarter turn
/// swaps the footprint's sides, so after one the tile is `depth` wide and `width` deep.
pub fn rotate_pos_in(pos: Pos, quarter_turns: u8, width: i32, depth: i32) -> Pos {
    let mut depth = depth;
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
    /// The wire on the upper level, and the ramp between the levels.
    pub wire_up: Tile,
    pub ramp: Tile,
    pub nor: Tile,
    pub cross: Tile,
    /// The wiring of a routing column, `NARROW` wide: the wire on each level,
    /// the crossing, and the ramp west to east and north to south.
    pub wire_n: Tile,
    pub wire_up_n: Tile,
    pub cross_n: Tile,
    pub ramp_n: Tile,
    pub ramp_ns_n: Tile,
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
            wire_up: parse("wire_up", include_str!("../../../cells/wire_up.tile"))?,
            ramp: parse("ramp", include_str!("../../../cells/ramp.tile"))?,
            nor: parse("nor", include_str!("../../../cells/nor.tile"))?,
            cross: parse("cross", include_str!("../../../cells/cross.tile"))?,
            wire_n: parse("wire_n", include_str!("../../../cells/wire_n.tile"))?,
            wire_up_n: parse("wire_up_n", include_str!("../../../cells/wire_up_n.tile"))?,
            cross_n: parse("cross_n", include_str!("../../../cells/cross_n.tile"))?,
            ramp_n: parse("ramp_n", include_str!("../../../cells/ramp_n.tile"))?,
            ramp_ns_n: parse("ramp_ns_n", include_str!("../../../cells/ramp_ns_n.tile"))?,
            tie_high: parse("tie_high", include_str!("../../../cells/tie_high.tile"))?,
            tie_low: parse("tie_low", include_str!("../../../cells/tie_low.tile"))?,
            dff: parse("dff", include_str!("../../../cells/dff.tile"))?,
            dff_alt: parse("dff_alt", include_str!("../../../cells/dff_alt.tile"))?,
        })
    }

    /// The synthesis cells this library offers, as (name, input pins) pairs.
    /// `NOT`, `NOR2` and `NOR3` are the NOR tile with one to three inputs;
    /// `DFF` is the flip-flop, with pins `D` and `CLK`.
    pub fn cells(&self) -> Vec<(&'static str, Vec<&'static str>)> {
        vec![("NOT", vec!["A"]), ("NOR2", vec!["A", "B"]), ("NOR3", vec!["A", "B", "C"]), ("DFF", vec!["D", "CLK"])]
    }

    /// The combinational cells among [`Library::cells`].
    pub fn gates(&self) -> Vec<(&'static str, Vec<&'static str>)> {
        self.cells().into_iter().filter(|(name, _)| *name != "DFF").collect()
    }

    /// The cell library in Liberty format, for Yosys's `abc -liberty`.
    ///
    /// `BUF` is listed because the mapper refuses a library without a buffer; it
    /// is priced so it is never preferred, and a buffer in a netlist is realised
    /// as plain wire.
    pub fn liberty(&self) -> String {
        let mut out = String::from("library(redfab) {\n");
        out.push_str("  cell(BUF) {\n    area: 4;\n    pin(A) { direction: input; }\n    pin(Y) { direction: output; function: \"A\"; }\n  }\n");
        for (name, pins) in self.gates() {
            out.push_str(&format!("  cell({name}) {{\n    area: 1;\n"));
            for pin in &pins {
                out.push_str(&format!("    pin({pin}) {{ direction: input; }}\n"));
            }
            out.push_str(&format!("    pin(Y) {{ direction: output; function: \"!({})\"; }}\n  }}\n", pins.join("+")));
        }
        // The flip-flop acts on the falling clock edge (stage 8); `dfflegalize`
        // turns a design's rising-edge flip-flops into these with an inverter on
        // the clock.
        out.push_str(
            "  cell(DFF) {\n    area: 4;\n    ff(IQ, IQN) { clocked_on: \"!CLK\"; next_state: \"D\"; }\n    pin(D) { direction: input; }\n    pin(CLK) { direction: input; clock: true; }\n    pin(Q) { direction: output; function: \"IQ\"; }\n  }\n",
        );
        out.push_str("}\n");
        out
    }

    /// Verilog models of the same cells, for the equivalence proof and for
    /// simulating a mapped netlist.
    pub fn verilog_models(&self) -> String {
        let mut out = String::from("module BUF(input A, output Y);\n  assign Y = A;\nendmodule\n");
        out.push_str("module DFF(input D, input CLK, output reg Q);\n  always @(negedge CLK) Q <= D;\nendmodule\n");
        for (name, pins) in self.gates() {
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
    /// Wiring on the upper level.
    WireUp,
    /// From the lower level to the upper one: west lower, east upper, before turning.
    Ramp,
    /// The same three, `NARROW` wide, for routing columns; and the narrow
    /// crossing and the narrow ramp north to south (a narrow tile turns only
    /// half round, so north-south needs a file of its own).
    WireN,
    WireUpN,
    RampN,
    RampNsN,
    CrossN,
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
    pub const ALL: [Kind; 16] = [
        Kind::Input,
        Kind::Output,
        Kind::Wire,
        Kind::WireUp,
        Kind::Ramp,
        Kind::WireN,
        Kind::WireUpN,
        Kind::RampN,
        Kind::RampNsN,
        Kind::CrossN,
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
            Kind::WireUp => "wire_up",
            Kind::Ramp => "ramp",
            Kind::WireN => "wire_n",
            Kind::WireUpN => "wire_up_n",
            Kind::RampN => "ramp_n",
            Kind::RampNsN => "ramp_ns_n",
            Kind::CrossN => "cross_n",
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
            Kind::WireUp => &library.wire_up,
            Kind::Ramp => &library.ramp,
            Kind::WireN => &library.wire_n,
            Kind::WireUpN => &library.wire_up_n,
            Kind::RampN => &library.ramp_n,
            Kind::RampNsN => &library.ramp_ns_n,
            Kind::CrossN => &library.cross_n,
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
        if matches!(self.kind, Kind::Wire | Kind::WireUp | Kind::WireN | Kind::WireUpN | Kind::Nor) {
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

    /// The blocks, rotated, with the repeater (if any) in place, and every block
    /// in its state at rest.
    pub fn blocks(&self, library: &Library) -> Vec<(Pos, Block)> {
        let tile = self.tile(library);
        let mut blocks = tile.blocks(&self.edges, self.rotation);
        for (pos, block) in &tile.rest {
            let turned = rotate_pos_in(*pos, self.rotation, tile.width, tile.depth);
            for (p, b) in &mut blocks {
                if *p == turned {
                    *b = block.rotated(self.rotation);
                }
            }
        }
        if let Some(entry) = self.repeater_entry {
            // Where the tile puts it: as a rule the cell next to the entry port.
            let (at, input) = tile
                .repeater_place(entry, &self.repeater_exits())
                .expect("a repeater variant has a place for its repeater");
            let inner = rotate_pos_in(at, self.rotation, tile.width, tile.depth);
            let facing = input.rotated(self.rotation).dir();
            for (pos, block) in &mut blocks {
                if *pos == inner {
                    *block = Block::Repeater { facing, delay: self.delay, locked: false, powered: false };
                }
            }
        }
        blocks
    }

    /// The edges a repeater entered by `repeater_entry` must reach (named before
    /// rotation): every other used edge, or in a crossing the one straight on.
    pub fn repeater_exits(&self) -> Vec<Edge> {
        let Some(entry) = self.repeater_entry else { return Vec::new() };
        match self.kind {
            Kind::Cross | Kind::CrossN => vec![entry.opposite()],
            _ => self.edges.iter().copied().filter(|&e| e != entry).collect(),
        }
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
                    Kind::Cross | Kind::CrossN => u8::from(matches!(p.edge, Edge::North | Edge::South)),
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
                    (Some(entry), PortKind::Io) if matches!(self.kind, Kind::Cross | Kind::CrossN) => {
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
                    pos: rotate_pos_in(p.pos, self.rotation, tile.width, tile.depth),
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

    /// Every variant the library offers: wiring on each level with every
    /// combination of two or more edges, each of those with a repeater on each
    /// possible entry, the ramp turned four ways, NOR with every non-empty set of
    /// inputs, and the fixed tiles.
    pub fn all() -> Vec<Variant> {
        let mut out = vec![Variant::new(Kind::Input, &[Edge::East]), Variant::new(Kind::Output, &[Edge::West])];
        // The same wiring on each level.
        for kind in [Kind::Wire, Kind::WireUp] {
            for mask in 1u8..16 {
                let edges: Vec<Edge> =
                    Edge::ALL.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, e)| *e).collect();
                if edges.len() >= 2 {
                    out.push(Variant::new(kind, &edges));
                    for &entry in &edges {
                        out.push(Variant { kind, ..Variant::repeater(&edges, entry, 1) });
                    }
                }
            }
            for delay in 2..=4 {
                out.push(Variant { kind, ..Variant::repeater(&[Edge::West, Edge::East], Edge::West, delay) });
            }
        }
        // The ramp turned every way: lower west to upper east, lower north to upper south, and so on.
        for rotation in 0..4 {
            out.push(Variant { rotation, ..Variant::new(Kind::Ramp, &[Edge::West, Edge::East]) });
        }
        // The narrow family: wiring on each level, with a repeater entered from the
        // north or south in any shape, and from the west or east in a straight or
        // a turn (that arm is the centre: a turn carries it in the arm it leaves
        // by); the crossing, with a repeater in its flat arm; the ramps turned
        // half round.
        for kind in [Kind::WireN, Kind::WireUpN] {
            for mask in 1u8..16 {
                let edges: Vec<Edge> =
                    Edge::ALL.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, e)| *e).collect();
                if edges.len() < 2 {
                    continue;
                }
                out.push(Variant::new(kind, &edges));
                for &entry in edges.iter().filter(|&&e| crate::layout::can_carry_repeater(NARROW, &edges, e)) {
                    out.push(Variant { kind, ..Variant::repeater(&edges, entry, 1) });
                }
            }
            for delay in 2..=4 {
                out.push(Variant { kind, ..Variant::repeater(&[Edge::West, Edge::East], Edge::West, delay) });
                out.push(Variant { kind, ..Variant::repeater(&[Edge::North, Edge::South], Edge::North, delay) });
            }
        }
        out.push(Variant::new(Kind::CrossN, &Edge::ALL));
        for entry in [Edge::West, Edge::East] {
            out.push(Variant { kind: Kind::CrossN, ..Variant::repeater(&Edge::ALL, entry, 1) });
        }
        for rotation in [0, 2] {
            out.push(Variant { rotation, ..Variant::new(Kind::RampN, &[Edge::West, Edge::East]) });
            out.push(Variant { rotation, ..Variant::new(Kind::RampNsN, &[Edge::North, Edge::South]) });
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
        // 2 io tiles, on each of the two levels 11 wires + 28 repeater placements +
        // 3 more delays, 4 ramps, 7 NORs, 3 crossings, 2 flip-flops, 2 ties; and
        // the narrow family: on each level 11 wires + 20 repeater placements (the
        // north and south arms of every shape, the west and east of the straight
        // and of the four turns) + 6 more delays, the crossing and its 2
        // repeaters, 4 ramps; the shallow family the same with west and east
        // swapped for north and south, plus 7 NORs, the input, the output and the
        // two ties; and the square family, on each level 11 wires + 4 repeater
        // placements (the two straights, each way) + 6 more delays, and 4 ramps
        let narrow = 2 * (11 + 20 + 6) + 3 + 4;
        let shallow = narrow + 7 + 4;
        let square = 2 * (11 + 4 + 6) + 4;
        assert_eq!(Variant::all().len(), 2 + 2 * (11 + 28 + 3) + 4 + 7 + 3 + 2 + 2 + narrow + shallow + square);
    }

    #[test]
    fn the_strengths_read_off_the_dust_are_the_known_ones() {
        let lib = Library::builtin().unwrap();
        // A tile is five blocks, so every hop from port to port costs five, straight
        // or turned, in a wire or along either axis of a crossing; a repeater hands
        // on 12 from any arm of a wire, and only from the flat arms of a crossing.
        for entry in Edge::ALL {
            for exit in Edge::ALL.into_iter().filter(|e| *e != entry) {
                assert_eq!(lib.wire.dust_steps(entry, exit), Some(5), "wire {entry:?} to {exit:?}");
                assert_eq!(lib.wire.repeater_out(entry, exit), Some(12), "wire {entry:?} to {exit:?}");
                let crosses = exit == entry.opposite();
                let flat = crosses && matches!(entry, Edge::West | Edge::East);
                assert_eq!(lib.cross.dust_steps(entry, exit), crosses.then_some(5), "cross {entry:?} to {exit:?}");
                assert_eq!(lib.cross.repeater_out(entry, exit), flat.then_some(12), "cross {entry:?} to {exit:?}");
            }
        }
        // A gate's input dust is not joined to its output.
        assert_eq!(lib.nor.dust_steps(Edge::West, Edge::East), None);
        // What each driver hands the next tile's port: the torch gives 15 to the
        // port dust beside it; the input and the flip-flop lose two more dust; the
        // constant 1 one; a constant 0 and plain wiring drive nothing.
        assert_eq!(lib.nor.source_out(Edge::East), 14);
        assert_eq!(lib.input.source_out(Edge::East), 12);
        assert_eq!(lib.tie_high.source_out(Edge::East), 13);
        assert_eq!(lib.dff.source_out(Edge::East), 12);
        assert_eq!(lib.tie_low.source_out(Edge::East), 0);
        assert_eq!(lib.wire.source_out(Edge::East), 0);
        assert_eq!(lib.nor.source_out(Edge::West), 0);
        // What each input port needs: a gate's dust must reach its block with one
        // level, a lamp's dust must point into it with one, the flip-flop's D runs
        // four dust to the master repeater, and its clock port powers the stone
        // beside it directly.
        for pin in [Edge::West, Edge::North, Edge::South] {
            assert_eq!(lib.nor.port_needs(pin), Some(2), "nor {pin:?}");
        }
        assert_eq!(lib.output.port_needs(Edge::West), Some(3));
        assert_eq!(lib.dff.port_needs(Edge::West), Some(5));
        assert_eq!(lib.dff.port_needs(Edge::North), Some(1));
        assert_eq!(lib.nor.port_needs(Edge::East), None);
        assert_eq!(lib.wire.port_needs(Edge::West), None);
        assert_eq!(lib.input.port_needs(Edge::East), None);
    }

    #[test]
    fn every_tile_is_a_cell_deep_and_turns_within_its_footprint() {
        let lib = Library::builtin().unwrap();
        for tile in [
            &lib.input,
            &lib.output,
            &lib.wire,
            &lib.wire_up,
            &lib.ramp,
            &lib.nor,
            &lib.cross,
            &lib.tie_high,
            &lib.tie_low,
            &lib.dff,
        ] {
            assert_eq!(tile.depth, PITCH, "{}", tile.name);
            assert_eq!(tile.width % PITCH, 0, "{}", tile.name);
        }
        // A quarter turn of a two-cell tile makes it one cell wide and two deep.
        assert_eq!(rotate_pos_in(Pos::new(0, 1, 0), 1, 10, 5), Pos::new(4, 1, 0));
        assert_eq!(rotate_pos_in(Pos::new(9, 1, 4), 1, 10, 5), Pos::new(0, 1, 9));
        assert_eq!(rotate_pos_in(Pos::new(9, 1, 4), 2, 10, 5), Pos::new(0, 1, 0));
        assert_eq!(rotate_pos_in(Pos::new(1, 1, 2), 4, 10, 5), Pos::new(1, 1, 2));
        assert_eq!(rotate_pos(Pos::new(0, 1, 2), 1), Pos::new(2, 1, 0));
    }

    #[test]
    fn liberty_and_verilog_list_the_same_cells() {
        let lib = Library::builtin().unwrap();
        for (name, _) in lib.cells() {
            assert!(lib.liberty().contains(&format!("cell({name})")));
            assert!(lib.verilog_models().contains(&format!("module {name}(")));
        }
    }
    /// The second wire level: the upper wire and the ramp each parse under the
    /// border rule, where the one block allowed on the border besides a port's
    /// dust is the stone under an upper port. A hop costs five either way and a
    /// repeater in an upper arm hands on twelve, as on the lower level; the ramp
    /// joins its lower west port to its upper east port, either way, with no
    /// room for a repeater. Stacked in one tile, a lower and an upper wire of
    /// any two shapes are two nets.
    #[test]
    fn the_upper_wire_and_the_ramp_keep_the_levels_apart() {
        let lib = Library::builtin().unwrap();
        let text = include_str!("../../../cells/wire_up.tile");
        let up = Tile::parse(text).unwrap();
        let ramp = Tile::parse(include_str!("../../../cells/ramp.tile")).unwrap();
        for entry in Edge::ALL {
            assert_eq!(up.port(entry).unwrap().pos.y, 3, "{entry:?}");
            for exit in Edge::ALL.into_iter().filter(|e| *e != entry) {
                assert_eq!(up.dust_steps(entry, exit), Some(5), "upper wire {entry:?} to {exit:?}");
                assert_eq!(up.repeater_out(entry, exit), Some(12), "upper wire {entry:?} to {exit:?}");
            }
        }
        assert_eq!((ramp.port(Edge::West).unwrap().pos.y, ramp.port(Edge::East).unwrap().pos.y), (1, 3));
        assert_eq!(ramp.dust_steps(Edge::West, Edge::East), Some(5));
        assert_eq!(ramp.dust_steps(Edge::East, Edge::West), Some(5));
        assert_eq!(ramp.repeater_out(Edge::West, Edge::East), None);
        assert_eq!(ramp.repeater_out(Edge::East, Edge::West), None);
        // A stone on the border that is not under an upper port is still refused.
        let stray = text.replacen("layer 2\n  ..N..", "layer 2\n  N.N..", 1);
        assert_ne!(stray, text);
        let err = Tile::parse(&stray).unwrap_err();
        assert!(err.contains("touches the tile border"), "{err}");
        // Every lower shape under every upper shape: two nets, each joining its own ports.
        let shapes: Vec<Vec<Edge>> = (1u8..16)
            .map(|mask| {
                Edge::ALL.into_iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, e)| e).collect()
            })
            .filter(|edges: &Vec<Edge>| edges.len() >= 2)
            .collect();
        assert_eq!(shapes.len(), 11);
        for lower in &shapes {
            for upper in &shapes {
                let mut world = World::new((PITCH, HEIGHT, PITCH));
                for (pos, block) in lib.wire.blocks(lower, 0).into_iter().chain(up.blocks(upper, 0)) {
                    world.set(pos, block);
                }
                let circuit = crate::extract::extract(&world, &[]);
                let net = |tile: &Tile, edge: Edge| circuit.net_of(tile.port(edge).unwrap().pos);
                let low = net(&lib.wire, lower[0]);
                let high = net(&up, upper[0]);
                assert!(low.is_some() && high.is_some() && low != high, "{lower:?} under {upper:?}");
                assert!(lower.iter().all(|&e| net(&lib.wire, e) == low), "{lower:?} under {upper:?}");
                assert!(upper.iter().all(|&e| net(&up, e) == high), "{lower:?} under {upper:?}");
                assert_eq!(circuit.nets.len(), 2, "{lower:?} under {upper:?}");
            }
        }
    }

    /// The narrow family, for routing columns three blocks wide: the wire, the
    /// upper wire, the crossing and the two ramps parse under the border rules,
    /// one cell wide and deep. A west-east hop costs three dust steps, a
    /// north-south one five, a turn four; a repeater entered from the north or
    /// south reaches every arm (thirteen to a side, twelve straight on); one
    /// entered from the west sits in the centre, which is its arm, and reaches
    /// the east (fourteen), or for a turn sits in the arm the signal leaves by,
    /// next to that port (fourteen beyond it); the crossing's flat arm carries
    /// one and its climbing arm none; the ramps climb three blocks west to east
    /// and five north to south.
    #[test]
    fn the_narrow_tiles_cost_three_steps_across_and_five_along() {
        let parse = |text: &str| Tile::parse(text).unwrap();
        let wire = parse(include_str!("../../../cells/wire_n.tile"));
        let up = parse(include_str!("../../../cells/wire_up_n.tile"));
        let cross = parse(include_str!("../../../cells/cross_n.tile"));
        let ramp = parse(include_str!("../../../cells/ramp_n.tile"));
        let ramp_ns = parse(include_str!("../../../cells/ramp_ns_n.tile"));
        use Edge::{East, North, South, West};
        for tile in [&wire, &up, &cross, &ramp, &ramp_ns] {
            assert_eq!((tile.width, tile.depth, tile.cells()), (3, PITCH, 1), "{}", tile.name);
        }
        for tile in [&wire, &up] {
            assert_eq!(tile.dust_steps(West, East), Some(3), "{}", tile.name);
            assert_eq!(tile.dust_steps(North, South), Some(5), "{}", tile.name);
            assert_eq!(tile.dust_steps(West, North), Some(4), "{}", tile.name);
            assert_eq!(tile.dust_steps(South, East), Some(4), "{}", tile.name);
            // From a repeater in the north arm: one dust to the centre, then one to
            // a side port or two to the south one.
            assert_eq!(tile.repeater_out(North, West), Some(13), "{}", tile.name);
            assert_eq!(tile.repeater_out(North, East), Some(13), "{}", tile.name);
            assert_eq!(tile.repeater_out(North, South), Some(12), "{}", tile.name);
            assert_eq!(tile.repeater_out(West, East), Some(14), "{}", tile.name);
            assert_eq!(tile.repeater_out(West, North), Some(14), "{}", tile.name);
            assert_eq!(tile.repeater_out(East, South), Some(14), "{}", tile.name);
        }
        assert_eq!(cross.dust_steps(West, East), Some(3));
        assert_eq!(cross.dust_steps(North, South), Some(5));
        assert_eq!(cross.dust_steps(West, North), None);
        assert_eq!(cross.repeater_out(West, East), Some(14));
        assert_eq!(cross.repeater_out(North, South), None);
        assert_eq!((ramp.port(West).unwrap().pos.y, ramp.port(East).unwrap().pos.y), (1, 3));
        assert_eq!(ramp.dust_steps(West, East), Some(3));
        assert_eq!(ramp.dust_steps(East, West), Some(3));
        assert_eq!((ramp_ns.port(North).unwrap().pos.y, ramp_ns.port(South).unwrap().pos.y), (1, 3));
        assert_eq!(ramp_ns.dust_steps(North, South), Some(5));
        assert_eq!(ramp_ns.repeater_out(North, South), None);
    }

    /// The shallow family, for rows three blocks deep in a column five wide, and
    /// the square one, three by three: the wire, the upper wire, the ramps, and
    /// in the shallow family the crossing, the NOR, the input, the output and
    /// the ties, parse under the border rules, one cell wide and deep. In a
    /// shallow row a hop along it costs five dust steps and one across three; a
    /// repeater entered from the west reaches every arm, one entered from the
    /// north sits in the centre and reaches the south, or for a turn sits in the
    /// arm it leaves by. The shallow crossing climbs west to east, so its flat
    /// north-south arm carries a repeater and its climbing arm none. Three by
    /// three, every hop costs three and only a straight carries a repeater. The
    /// NOR reads its north and south inputs from the ports' own dust.
    #[test]
    fn the_shallow_tiles_cost_five_steps_along_and_three_across() {
        let parse = |text: &str| Tile::parse(text).unwrap();
        let wire = parse(include_str!("../../../cells/wire_s.tile"));
        let up = parse(include_str!("../../../cells/wire_up_s.tile"));
        let cross = parse(include_str!("../../../cells/cross_s.tile"));
        let ramp = parse(include_str!("../../../cells/ramp_s.tile"));
        let ramp_ns = parse(include_str!("../../../cells/ramp_ns_s.tile"));
        let nor = parse(include_str!("../../../cells/nor_s.tile"));
        let input = parse(include_str!("../../../cells/input_s.tile"));
        let output = parse(include_str!("../../../cells/output_s.tile"));
        let high = parse(include_str!("../../../cells/tie_high_s.tile"));
        let low = parse(include_str!("../../../cells/tie_low_s.tile"));
        let square = parse(include_str!("../../../cells/wire_sq.tile"));
        let square_up = parse(include_str!("../../../cells/wire_up_sq.tile"));
        let square_ramp = parse(include_str!("../../../cells/ramp_sq.tile"));
        let square_ramp_ns = parse(include_str!("../../../cells/ramp_ns_sq.tile"));
        use Edge::{East, North, South, West};
        for tile in [&wire, &up, &cross, &ramp, &ramp_ns, &nor, &input, &output, &high, &low] {
            assert_eq!((tile.width, tile.depth, tile.cells()), (PITCH, 3, 1), "{}", tile.name);
        }
        for tile in [&square, &square_up, &square_ramp, &square_ramp_ns] {
            assert_eq!((tile.width, tile.depth, tile.cells()), (3, 3, 1), "{}", tile.name);
        }
        for tile in [&wire, &up] {
            assert_eq!(tile.dust_steps(West, East), Some(5), "{}", tile.name);
            assert_eq!(tile.dust_steps(North, South), Some(3), "{}", tile.name);
            assert_eq!(tile.dust_steps(West, North), Some(4), "{}", tile.name);
            // From a repeater in the west arm: one dust to the centre, then one to
            // the north port or two to the east one.
            assert_eq!(tile.repeater_out(West, North), Some(13), "{}", tile.name);
            assert_eq!(tile.repeater_out(West, East), Some(12), "{}", tile.name);
            assert_eq!(tile.repeater_needs(West, East), Some(1), "{}", tile.name);
            assert_eq!(tile.repeater_out(North, South), Some(14), "{}", tile.name);
            assert_eq!(tile.repeater_out(North, East), Some(14), "{}", tile.name);
            assert_eq!(tile.repeater_needs(North, East), Some(2), "{}", tile.name);
            assert_eq!(tile.repeater_place(North, &[East, South]), None, "{}", tile.name);
        }
        assert_eq!(cross.dust_steps(West, East), Some(5));
        assert_eq!(cross.dust_steps(North, South), Some(3));
        assert_eq!(cross.dust_steps(West, North), None);
        assert_eq!(cross.repeater_out(North, South), Some(14));
        assert_eq!(cross.repeater_out(West, East), None);
        assert_eq!((ramp.port(West).unwrap().pos.y, ramp.port(East).unwrap().pos.y), (1, 3));
        assert_eq!(ramp.dust_steps(West, East), Some(5));
        assert_eq!((ramp_ns.port(North).unwrap().pos.y, ramp_ns.port(South).unwrap().pos.y), (1, 3));
        assert_eq!(ramp_ns.dust_steps(North, South), Some(3));
        assert_eq!(ramp_ns.repeater_out(North, South), None);
        // Along the row the gates, sources and outputs are the wide tiles' own.
        let lib = Library::builtin().unwrap();
        assert_eq!(nor.port_needs(West), lib.nor.port_needs(West));
        assert_eq!(nor.port_needs(North), Some(1));
        assert_eq!(nor.port_needs(South), Some(1));
        assert_eq!(nor.source_out(East), lib.nor.source_out(East));
        assert_eq!(input.source_out(East), lib.input.source_out(East));
        assert_eq!(output.port_needs(West), lib.output.port_needs(West));
        assert_eq!(high.source_out(East), lib.tie_high.source_out(East));
        assert_eq!(low.source_out(East), 0);
        for tile in [&square, &square_up] {
            assert_eq!(tile.dust_steps(West, East), Some(3), "{}", tile.name);
            assert_eq!(tile.dust_steps(North, South), Some(3), "{}", tile.name);
            assert_eq!(tile.dust_steps(West, North), Some(3), "{}", tile.name);
            assert_eq!(tile.repeater_out(West, East), Some(14), "{}", tile.name);
            assert_eq!(tile.repeater_out(North, South), Some(14), "{}", tile.name);
            assert_eq!(tile.repeater_out(West, North), None, "{}", tile.name);
            assert_eq!(tile.repeater_out(North, East), None, "{}", tile.name);
        }
        assert_eq!(square_ramp.dust_steps(West, East), Some(3));
        assert_eq!(square_ramp_ns.dust_steps(North, South), Some(3));
    }

    /// In a narrow tile the cell next to the west or east port is the centre,
    /// where the arms meet, so a repeater there can only pass the signal straight
    /// on. A turn from there carries its repeater in the arm it leaves by
    /// instead, next to that port, facing back towards the centre it reads;
    /// a fan-out from the west or east has no single place for one.
    #[test]
    fn a_narrow_turn_entered_from_the_west_carries_its_repeater_in_the_arm_it_leaves_by() {
        let lib = Library::builtin().unwrap();
        use Edge::{East, North, South, West};
        for (kind, y) in [(Kind::WireN, 1), (Kind::WireUpN, 3)] {
            let turn = Variant { kind, ..Variant::repeater(&[West, North], West, 1) };
            let ports = turn.ports(&lib);
            assert_eq!(
                ports.iter().map(|p| (p.edge, p.kind)).collect::<Vec<_>>(),
                vec![(West, PortKind::In), (North, PortKind::Out)]
            );
            let blocks = turn.blocks(&lib);
            let repeaters: Vec<&(Pos, Block)> =
                blocks.iter().filter(|(_, b)| matches!(b, Block::Repeater { .. })).collect();
            assert_eq!(repeaters.len(), 1, "{}", turn.name());
            assert_eq!(repeaters[0].0, Pos::new(1, y, 1), "{}", turn.name());
            assert_eq!(
                repeaters[0].1.to_string(),
                "minecraft:repeater[delay=1,facing=south,locked=false,powered=false]",
                "{}",
                turn.name()
            );
            // The west port, the centre and the north port stay dust.
            let wires = blocks.iter().filter(|(_, b)| matches!(b, Block::Wire(_))).count();
            assert_eq!(wires, 3, "{}", turn.name());
            // Turned half round it is an east-to-south turn with the repeater in the south arm.
            let back = Variant { rotation: 2, ..turn.clone() };
            let repeater = back.blocks(&lib).into_iter().find(|(_, b)| matches!(b, Block::Repeater { .. })).unwrap();
            assert_eq!(repeater.0, Pos::new(1, y, 3), "{}", back.name());
            let names: Vec<String> = Variant::all().iter().map(Variant::name).collect();
            for shape in ["WN", "WS", "EN", "ES"] {
                let entry = &shape[..1];
                let name = format!("{}_{shape}_rep{entry}_d1", kind.name());
                assert!(names.contains(&name), "{name} is a variant");
            }
            // A fan-out entered from the west has no single place for a repeater.
            for shape in ["WNS", "WEN", "WES", "WENS"] {
                assert!(!names.contains(&format!("{}_{shape}_repW_d1", kind.name())), "{shape}");
            }
        }
        // That repeater reads the centre, a step in from the port, so the port
        // must carry two; one in its entry arm reads the port's own dust.
        assert_eq!(lib.wire_n.repeater_needs(West, North), Some(2));
        assert_eq!(lib.wire_up_n.repeater_needs(East, South), Some(2));
        assert_eq!(lib.wire_n.repeater_needs(West, East), Some(1));
        assert_eq!(lib.wire_n.repeater_needs(North, West), Some(1));
        assert_eq!(lib.wire.repeater_needs(West, North), Some(1));
        // The rule the router and the hold fixer ask agrees with the tile file
        // for every narrow shape and entry.
        for mask in 1u8..16 {
            let edges: Vec<Edge> =
                Edge::ALL.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, e)| *e).collect();
            if edges.len() < 2 {
                continue;
            }
            for &entry in &edges {
                let exits: Vec<Edge> = edges.iter().copied().filter(|&e| e != entry).collect();
                assert_eq!(
                    crate::layout::can_carry_repeater(NARROW, &edges, entry),
                    lib.wire_n.repeater_place(entry, &exits).is_some(),
                    "{edges:?} entered by {entry:?}"
                );
            }
        }
        assert!(crate::layout::can_carry_repeater(NARROW, &[West, North], West));
        assert!(crate::layout::can_carry_repeater(NARROW, &[East, South], East));
        assert!(!crate::layout::can_carry_repeater(NARROW, &[West, North, South], West));
        assert!(!crate::layout::can_carry_repeater(NARROW, &[West, East, North], West));
    }
}
