//! Routing: wires between the placed tiles, then repeaters where the signal
//! would otherwise die.
//!
//! Every routing tile offers two tracks, west-east and north-south. A straight
//! wire uses one track; a turn or a fan-out uses both; two straight wires on
//! different tracks share the tile as a crossing. The router is PathFinder:
//! each net is routed by shortest path, and tracks that several nets want
//! become more expensive each round until every track has one net. A run
//! whose count of contended tracks stops falling is given up, and reports
//! which tracks are still wanted so that the gap they lie in can be widened;
//! the history costs it built up carry over to the next attempt.
//!
//! Signal strength is exact on the tile grid: dust loses one level per block,
//! and each tile says what a hop through it costs, what its own driver hands
//! on and what its pins need ([`Tile::dust_steps`] and the methods beside it,
//! read off the tile files). From that, repeaters are inserted into straight
//! tiles wherever a signal would arrive too weak.

use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use crate::layout::{Layout, Placed};
use crate::netlist::{Driver, NetId, Netlist};
use crate::place::{Cell, Column, Placement};
use crate::tile::{Edge, Library, Tile};

/// A track in a routing tile.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Track {
    X,
    Z,
}

type Node = (Cell, Track);

/// Where a net starts: the first routing node, and the edge the signal enters it by.
struct Source {
    node: Node,
    entry: Edge,
    /// Power at the entry port of that first tile.
    strength: u8,
}

/// Where a net must arrive: the last routing node, the edge it leaves it by,
/// and the power the pin behind that edge needs at the node's exit.
struct Sink {
    node: Node,
    exit: Edge,
    needed_after: u8,
}

/// What a repeater needs at the port it sits behind: it reads that port's own
/// dust (the repeater takes the cell next to the port, see `Variant::blocks`),
/// so any power there is enough.
const REPEATER_NEEDS: u8 = 1;

/// The most rounds of negotiation in one run.
const MAX_ITERATIONS: usize = 200;
/// Rounds without the number of contended tracks falling below its best so far
/// before a run is given up as stuck.
const STALL_ROUNDS: usize = 12;
/// How many times routing is redone after a signal could not be kept alive.
const MAX_STRENGTH_RETRIES: usize = 100;

/// The routed design.
#[derive(Clone)]
pub struct Routed {
    pub layout: Layout,
    /// Per net, per sink (in `Netlist::sinks_of` order): repeaters on the path from the driver.
    pub repeaters: BTreeMap<NetId, Vec<u32>>,
    /// Per net, per sink: the tiles from the driver to the sink, each with the
    /// edge the signal enters it by. The timing fixes lengthen a path here.
    pub branches: BTreeMap<NetId, Vec<Vec<(Cell, Edge)>>>,
    /// How many rounds of negotiation the final run took.
    pub rounds: usize,
}

/// PathFinder's history costs, named by column kind, row and track so that they
/// survive a gap being widened (the tiles keep their names).
#[derive(Clone, Debug, Default)]
pub struct History {
    pub costs: BTreeMap<(Column, i32, Track), f64>,
}

/// Why routing failed.
#[derive(Debug)]
pub enum RouteFailure {
    /// The negotiation got stuck: these tracks are still wanted by several nets
    /// (`users[i]` names the nets at `tracks[i]`).
    Congested { tracks: Vec<Node>, users: Vec<Vec<NetId>>, rounds: usize, history: History },
    /// A signal could not be kept alive, or the placement cannot be routed at all.
    Other(String),
}

impl std::fmt::Display for RouteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteFailure::Congested { tracks, rounds, .. } => write!(
                f,
                "routing did not converge: after {rounds} rounds {} tracks are still wanted by several nets",
                tracks.len()
            ),
            RouteFailure::Other(e) => write!(f, "{e}"),
        }
    }
}

/// Routes every net of the placement. Fails if some net cannot be routed, or
/// a signal cannot be kept alive with repeaters. `history` seeds the negotiation
/// with what an earlier attempt learned; `log` hears about progress.
pub fn route(
    library: &Library,
    netlist: &Netlist,
    placement: &Placement,
    history: Option<&History>,
    log: &mut dyn FnMut(&str),
) -> Result<Routed, RouteFailure> {
    let drivers = netlist.drivers().map_err(RouteFailure::Other)?;
    // What a pin needs at the port of the tile in front of it.
    let needs = |tile: &Tile, pin: Edge| -> Result<u8, RouteFailure> {
        tile.port_needs(pin)
            .ok_or_else(|| RouteFailure::Other(format!("the {} tile has no {} input", tile.name, pin.letter())))
    };
    let mut nets: Vec<(NetId, Source, Vec<Sink>)> = Vec::new();
    for (net, &driver) in drivers.iter().enumerate() {
        let sinks = netlist.sinks_of(net);
        if sinks.is_empty() {
            continue;
        }
        let source = match driver {
            Driver::Cell(c) => {
                let (col, row) = placement.gates[c];
                let tile = if netlist.cells[c].is_dff() { &library.dff } else { &library.nor };
                Source {
                    node: ((col + placement.widths[c], row), Track::X),
                    entry: Edge::West,
                    strength: tile.source_out(Edge::East),
                }
            }
            Driver::Input(i) => {
                let (col, row) = placement.inputs[i];
                Source {
                    node: ((col + 1, row), Track::X),
                    entry: Edge::West,
                    strength: library.input.source_out(Edge::East),
                }
            }
            Driver::Const(v) => {
                let (_, _, (col, row)) = *placement
                    .ties
                    .iter()
                    .find(|t| t.0 == net)
                    .ok_or_else(|| RouteFailure::Other("constant without a tie".to_string()))?;
                let tile = if v { &library.tie_high } else { &library.tie_low };
                Source { node: ((col + 1, row), Track::X), entry: Edge::West, strength: tile.source_out(Edge::East) }
            }
        };
        let mut sink_list = Vec::with_capacity(sinks.len());
        for &(c, pin) in &sinks {
            let sink = if c == usize::MAX {
                let (col, row) = placement.outputs[pin];
                Sink {
                    node: ((col - 1, row), Track::X),
                    exit: Edge::East,
                    needed_after: needs(&library.output, Edge::West)?,
                }
            } else {
                let (col, row) = placement.gates[c];
                let tile = if netlist.cells[c].is_dff() { &library.dff } else { &library.nor };
                let edge = placement.pin_edges[c][pin];
                let needed_after = needs(tile, edge)?;
                match edge {
                    Edge::West => Sink { node: ((col - 1, row), Track::X), exit: Edge::East, needed_after },
                    Edge::North => Sink { node: ((col, row - 1), Track::Z), exit: Edge::South, needed_after },
                    Edge::South => Sink { node: ((col, row + 1), Track::Z), exit: Edge::North, needed_after },
                    Edge::East => unreachable!("gates never take input on the east"),
                }
            };
            sink_list.push(sink);
        }
        nets.push((net, source, sink_list));
    }

    let graph = Graph::new(placement);
    let key = |node: Node| (placement.column(node.0.0), node.0.1, node.1);
    let seed: Vec<f64> =
        graph.nodes.iter().map(|&n| history.and_then(|h| h.costs.get(&key(n)).copied()).unwrap_or(0.0)).collect();
    let mut negotiation = Negotiation::new(&graph, &nets, seed);
    // Tracks that a net could not get a repeater past (crossings) get more expensive
    // for that net each time, until every signal survives. The negotiation picks
    // up where it was: only that net has to move, and whatever it then collides with.
    let mut retries = 0;
    loop {
        let (trees, rounds) = match negotiation.run(log) {
            Ok(done) => done,
            Err(Stuck::Unroutable(e)) => return Err(RouteFailure::Other(e)),
            Err(Stuck::Congested { overused, rounds }) => {
                let history = History {
                    costs: graph
                        .nodes
                        .iter()
                        .zip(&negotiation.history)
                        .filter(|(_, c)| **c > 0.0)
                        .map(|(&n, &c)| (key(n), c))
                        .collect(),
                };
                let tracks = overused.iter().map(|&i| graph.nodes[i]).collect();
                let users = overused
                    .iter()
                    .map(|&i| {
                        negotiation
                            .trees
                            .iter()
                            .enumerate()
                            .filter(|(_, t)| t.contains(&i))
                            .map(|(k, _)| nets[k].0)
                            .collect()
                    })
                    .collect();
                return Err(RouteFailure::Congested { tracks, users, rounds, history });
            }
        };
        match realize_trees(library, netlist, placement, &nets, &trees) {
            Ok(mut routed) => {
                routed.rounds = rounds;
                return Ok(routed);
            }
            Err(Failure::Strength(failed)) if retries < MAX_STRENGTH_RETRIES => {
                retries += 1;
                // Heavier each time, so that the nets leave their runs of crossings
                // rather than shuffle along them.
                let weight = 4.0 * f64::from(retries.min(16) as u8);
                for (net_index, crossings) in failed {
                    for node in crossings {
                        let index = graph.index[&node];
                        *negotiation.penalties[net_index].entry(index).or_insert(0.0) += weight;
                    }
                    negotiation.dirty.push(net_index);
                }
                // Let the nets push others aside again rather than keep their paths.
                negotiation.present_factor = 0.5;
            }
            Err(Failure::Strength(_)) => {
                return Err(RouteFailure::Other(
                    "a signal cannot be kept alive: every path it finds runs through too many crossings".to_string(),
                ));
            }
            Err(Failure::Other(e)) => return Err(RouteFailure::Other(e)),
        }
    }
}

/// What each tile carries: per net, the edges it uses and the one the signal enters by.
type Tiles = BTreeMap<Cell, Vec<(NetId, BTreeSet<Edge>, Option<Edge>)>>;

/// Why a routing attempt could not be turned into tiles.
enum Failure {
    /// Repeater insertion failed for these nets (by index into the net list);
    /// with each, the nodes it used in crossing tiles on the failing path.
    Strength(Vec<(usize, Vec<Node>)>),
    Other(String),
}

/// Turns routed trees into tiles and inserts repeaters.
fn realize_trees(
    library: &Library,
    netlist: &Netlist,
    placement: &Placement,
    nets: &[(NetId, Source, Vec<Sink>)],
    trees: &[Tree],
) -> Result<Routed, Failure> {
    // Tile contents from the trees.
    let mut tiles: Tiles = BTreeMap::new();
    for ((net, source, sinks), tree) in nets.iter().zip(trees) {
        for &node in tree.parent.keys() {
            let mut edges = BTreeSet::new();
            let mut entry = None;
            let (cell, track) = node;
            // Towards the parent and each child.
            let related =
                std::iter::once(tree.parent[&node]).flatten().chain(tree.children(node)).collect::<Vec<Node>>();
            for other in related {
                if other.0 != cell {
                    let edge = edge_between(cell, other.0);
                    edges.insert(edge);
                    if tree.parent[&node] == Some(other) {
                        entry = Some(edge);
                    }
                }
            }
            if node == source.node {
                edges.insert(source.entry);
                entry = Some(source.entry);
            }
            for sink in sinks.iter().filter(|s| s.node == node) {
                edges.insert(sink.exit);
            }
            // The other track of the same tile, if this net uses it, means a turn:
            // all edges of the tile belong to one wire; record under the tile once.
            let _ = track;
            let list = tiles.entry(cell).or_default();
            match list.iter_mut().find(|(n, _, _)| *n == *net) {
                Some((_, e, en)) => {
                    e.extend(edges);
                    if en.is_none() {
                        *en = entry;
                    }
                }
                None => list.push((*net, edges, entry)),
            }
        }
    }

    let mut layout = Layout {
        tiles: BTreeMap::new(),
        col_width: placement.col_width.clone(),
        row_depth: placement.row_depth.clone(),
    };
    for (c, cell) in netlist.cells.iter().enumerate() {
        let placed = if cell.is_dff() {
            Placed::Dff { cell: c, init: cell.init }
        } else {
            Placed::Gate { cell: c, pin_edges: placement.pin_edges[c].clone() }
        };
        layout.tiles.insert(placement.gates[c], placed);
    }
    for (i, &cell) in placement.inputs.iter().enumerate() {
        let read = !netlist.sinks_of(netlist.inputs[i].net).is_empty();
        layout.tiles.insert(cell, Placed::Input { port: i, read });
    }
    for (o, &cell) in placement.outputs.iter().enumerate() {
        layout.tiles.insert(cell, Placed::Output { port: o });
    }
    for &(net, value, cell) in &placement.ties {
        layout.tiles.insert(cell, if value { Placed::TieHigh { net } } else { Placed::TieLow { net } });
    }
    for (cell, list) in &tiles {
        let placed = match list.as_slice() {
            [(_, edges, _)] => Placed::Wire { edges: edges.iter().copied().collect() },
            [a, b] => {
                let straight = |e: &BTreeSet<Edge>| {
                    *e == BTreeSet::from([Edge::West, Edge::East]) || *e == BTreeSet::from([Edge::North, Edge::South])
                };
                if straight(&a.1) && straight(&b.1) && a.1 != b.1 {
                    Placed::Cross { rotation: 0, repeater_entry: None, delay: 1 }
                } else {
                    return Err(Failure::Other(format!("two nets meet in tile {cell:?} other than as a crossing")));
                }
            }
            _ => return Err(Failure::Other(format!("more than two nets in tile {cell:?}"))),
        };
        layout.tiles.insert(*cell, placed);
    }

    // Repeaters: walk each net from its source with exact strengths. Every net
    // whose signal dies is reported, so that one re-route moves them all.
    let mut repeater_counts: BTreeMap<NetId, Vec<u32>> = BTreeMap::new();
    let mut branches: BTreeMap<NetId, Vec<Vec<(Cell, Edge)>>> = BTreeMap::new();
    let mut died: Vec<(usize, Vec<Node>)> = Vec::new();
    for (net_index, ((net, source, sinks), tree)) in nets.iter().zip(trees).enumerate() {
        // A constant 0 carries no signal, so nothing can die on the way.
        let counts = if source.strength == 0 {
            vec![0; sinks.len()]
        } else {
            match insert_repeaters(library, &mut layout, &tiles, *net, source, sinks, tree) {
                Ok(counts) => counts,
                Err(Dies { path }) => {
                    let crossings = path
                        .iter()
                        .filter(|cell| tiles[*cell].len() > 1)
                        .flat_map(|cell| tree.parent.keys().filter(|n| n.0 == *cell).copied().collect::<Vec<_>>())
                        .collect();
                    died.push((net_index, crossings));
                    continue;
                }
            }
        };
        repeater_counts.insert(*net, counts);
        // The branch to each sink: from the sink's tile back to the source's.
        let entry_of =
            |cell: Cell| -> Option<Edge> { tiles[&cell].iter().find(|(n, _, _)| n == net).and_then(|(_, _, e)| *e) };
        let parent_cell = |cell: Cell| -> Option<Cell> {
            let node = tree.parent.keys().find(|n| n.0 == cell && tree.parent[n].is_none_or(|p| p.0 != cell))?;
            tree.parent[node].map(|p| p.0)
        };
        let mut per_sink = Vec::new();
        for sink in sinks {
            let mut path = Vec::new();
            let mut cell = Some(sink.node.0);
            while let Some(c) = cell {
                path.push((c, entry_of(c).unwrap_or(source.entry)));
                cell = parent_cell(c);
            }
            path.reverse();
            per_sink.push(path);
        }
        branches.insert(*net, per_sink);
    }
    if !died.is_empty() {
        return Err(Failure::Strength(died));
    }
    Ok(Routed { layout, repeaters: repeater_counts, branches, rounds: 0 })
}

/// A signal that could not be kept alive: the tiles from the source to where it died.
struct Dies {
    path: Vec<Cell>,
}

/// A text picture of a layout: one character per tile.
pub fn render(layout: &Layout) -> String {
    let (cols, rows) = layout.size();
    let mut out = String::new();
    for row in 0..rows {
        for col in 0..cols {
            let c = match layout.tiles.get(&(col, row)) {
                None => '.',
                Some(Placed::Input { .. }) => 'I',
                Some(Placed::Output { .. }) => 'O',
                Some(Placed::Gate { .. }) => 'G',
                Some(Placed::Dff { .. }) => 'D',
                Some(Placed::TieHigh { .. }) => '1',
                Some(Placed::TieLow { .. }) => '0',
                Some(Placed::Cross { repeater_entry: None, .. }) => '╬',
                Some(Placed::Cross { rotation: 0, .. }) => '╪',
                Some(Placed::Cross { .. }) => '╫',
                Some(Placed::Repeater { edges, .. }) => wire_char(edges, true),
                Some(Placed::Wire { edges }) => wire_char(edges, false),
            };
            out.push(c);
        }
        out.push('\n');
    }
    out
}

fn wire_char(edges: &[Edge], repeater: bool) -> char {
    let has = |e: Edge| edges.contains(&e);
    let bits = (u8::from(has(Edge::West)) << 3)
        | (u8::from(has(Edge::East)) << 2)
        | (u8::from(has(Edge::North)) << 1)
        | u8::from(has(Edge::South));
    let plain = match bits {
        0b1100 => '─',
        0b0011 => '│',
        0b0101 => '┌',
        0b1001 => '┐',
        0b0110 => '└',
        0b1010 => '┘',
        0b0111 => '├',
        0b1011 => '┤',
        0b1101 => '┬',
        0b1110 => '┴',
        0b1111 => '┼',
        _ => '?',
    };
    if repeater {
        match plain {
            '─' => '═',
            '│' => '║',
            _ => 'R',
        }
    } else {
        plain
    }
}

fn edge_between(from: Cell, to: Cell) -> Edge {
    match (to.0 - from.0, to.1 - from.1) {
        (1, 0) => Edge::East,
        (-1, 0) => Edge::West,
        (0, 1) => Edge::South,
        (0, -1) => Edge::North,
        _ => unreachable!("tiles are adjacent"),
    }
}

/// The routing graph: every track of every free tile, joined to the same track
/// of the neighbouring tiles and to the other track of its own tile.
struct Graph {
    nodes: Vec<Node>,
    index: BTreeMap<Node, usize>,
    neighbours: Vec<Vec<usize>>,
}

impl Graph {
    fn new(placement: &Placement) -> Graph {
        let mut nodes = Vec::new();
        let mut index = BTreeMap::new();
        for row in 0..placement.rows {
            for col in 0..placement.cols {
                if placement.is_free((col, row)) {
                    for track in [Track::X, Track::Z] {
                        index.insert(((col, row), track), nodes.len());
                        nodes.push(((col, row), track));
                    }
                }
            }
        }
        let mut neighbours = vec![Vec::new(); nodes.len()];
        for (i, &((col, row), track)) in nodes.iter().enumerate() {
            let candidates: Vec<Node> = match track {
                Track::X => vec![((col - 1, row), Track::X), ((col + 1, row), Track::X), ((col, row), Track::Z)],
                Track::Z => vec![((col, row - 1), Track::Z), ((col, row + 1), Track::Z), ((col, row), Track::X)],
            };
            neighbours[i] = candidates.into_iter().filter_map(|n| index.get(&n).copied()).collect();
        }
        Graph { nodes, index, neighbours }
    }
}

/// A routed net: every node it uses, with the node it was reached from.
struct Tree {
    parent: BTreeMap<Node, Option<Node>>,
}

impl Tree {
    fn children(&self, node: Node) -> Vec<Node> {
        self.parent.iter().filter(|(_, p)| **p == Some(node)).map(|(n, _)| *n).collect()
    }
}

/// Why a negotiation did not finish.
enum Stuck {
    /// Stuck: the nodes still overused, and the rounds it ran.
    Congested { overused: Vec<usize>, rounds: usize },
    /// A pin faces no free tile, or a sink cannot be reached at all.
    Unroutable(String),
}

/// Negotiated-congestion routing of all nets, resumable: after a run ends with
/// every track to one net, the caller may penalise nodes for a net and run
/// again, and only that net and whatever it then collides with move.
struct Negotiation<'a> {
    graph: &'a Graph,
    nets: &'a [(NetId, Source, Vec<Sink>)],
    /// Per node: how many nets use it.
    occupancy: Vec<u32>,
    /// Per net: its nodes, and each node's parent.
    trees: Vec<Vec<usize>>,
    parents: Vec<BTreeMap<usize, Option<usize>>>,
    /// Per node: the history cost, seeded by the caller.
    history: Vec<f64>,
    /// Per net: extra cost on particular nodes.
    penalties: Vec<BTreeMap<usize, f64>>,
    /// Nets that must be routed again whatever their congestion.
    dirty: Vec<usize>,
    present_factor: f64,
    rounds: usize,
}

impl<'a> Negotiation<'a> {
    fn new(graph: &'a Graph, nets: &'a [(NetId, Source, Vec<Sink>)], history: Vec<f64>) -> Negotiation<'a> {
        Negotiation {
            graph,
            nets,
            occupancy: vec![0; graph.nodes.len()],
            trees: vec![Vec::new(); nets.len()],
            parents: vec![BTreeMap::new(); nets.len()],
            history,
            penalties: vec![BTreeMap::new(); nets.len()],
            dirty: (0..nets.len()).collect(),
            present_factor: 0.5,
            rounds: 0,
        }
    }

    /// Rounds of rip-up and re-route until no node is overused. Each round
    /// re-routes the nets that run through an overused node (and the dirty
    /// ones); the first round routes everything.
    fn run(&mut self, log: &mut dyn FnMut(&str)) -> Result<(Vec<Tree>, usize), Stuck> {
        let graph = self.graph;
        let n = graph.nodes.len();
        let mut best = usize::MAX;
        let mut stalled = 0;
        let lookup = |node: Node| -> Result<usize, Stuck> {
            graph.index.get(&node).copied().ok_or_else(|| Stuck::Unroutable(format!("no routing tile at {:?}", node.0)))
        };
        for round in 0..MAX_ITERATIONS {
            let mut to_route: Vec<usize> = std::mem::take(&mut self.dirty);
            to_route
                .extend((0..self.nets.len()).filter(|&k| self.trees[k].iter().any(|&node| self.occupancy[node] > 1)));
            to_route.sort_unstable();
            to_route.dedup();
            for k in to_route {
                let (_, source, sinks) = &self.nets[k];
                for &node in &self.trees[k] {
                    self.occupancy[node] -= 1;
                }
                let start = lookup(source.node)?;
                let mut tree = vec![start];
                let mut parent: BTreeMap<usize, Option<usize>> = BTreeMap::from([(start, None)]);
                let mut targets: Vec<usize> = sinks.iter().map(|s| lookup(s.node)).collect::<Result<_, _>>()?;
                // Nearest sinks first.
                let dist = |a: usize, b: usize| {
                    let (ca, cb) = (graph.nodes[a].0, graph.nodes[b].0);
                    (ca.0 - cb.0).abs() + (ca.1 - cb.1).abs()
                };
                targets.sort_by_key(|&t| dist(start, t));
                for target in targets {
                    if tree.contains(&target) {
                        continue;
                    }
                    let (occupancy, history, penalties, present_factor) =
                        (&self.occupancy, &self.history, &self.penalties[k], self.present_factor);
                    let path = shortest_path(graph, &tree, target, |node| {
                        let overuse = occupancy[node] as f64; // this net's own nodes are not in occupancy now
                        let penalty = penalties.get(&node).copied().unwrap_or(0.0);
                        (1.0 + history[node] + penalty + jitter(k, node)) * (1.0 + present_factor * overuse)
                    })
                    .ok_or_else(|| {
                        Stuck::Unroutable(format!("net cannot reach the tile at {:?}", graph.nodes[target].0))
                    })?;
                    for window in path.windows(2) {
                        if let std::collections::btree_map::Entry::Vacant(slot) = parent.entry(window[1]) {
                            slot.insert(Some(window[0]));
                            tree.push(window[1]);
                        }
                    }
                }
                for &node in &tree {
                    self.occupancy[node] += 1;
                }
                self.trees[k] = tree;
                self.parents[k] = parent;
            }
            self.rounds += 1;
            let overused: Vec<usize> = (0..n).filter(|&i| self.occupancy[i] > 1).collect();
            if overused.is_empty() {
                return Ok((
                    self.parents
                        .iter()
                        .map(|p| Tree {
                            parent: p
                                .iter()
                                .map(|(&node, &par)| (graph.nodes[node], par.map(|q| graph.nodes[q])))
                                .collect(),
                        })
                        .collect(),
                    self.rounds,
                ));
            }
            for &i in &overused {
                self.history[i] += f64::from(self.occupancy[i] - 1);
            }
            self.present_factor = (self.present_factor * 1.6).min(1e6);
            if overused.len() < best {
                best = overused.len();
                stalled = 0;
            } else {
                stalled += 1;
            }
            if self.rounds.is_multiple_of(10) {
                log(&format!("  round {}: {} tracks wanted by several nets", self.rounds, overused.len()));
            }
            if stalled >= STALL_ROUNDS || round + 1 == MAX_ITERATIONS {
                return Err(Stuck::Congested { overused, rounds: self.rounds });
            }
        }
        unreachable!()
    }
}

/// A tiny cost that differs per net and node, so that nets with several
/// equally short paths (every staircase between two rows is one) do not all
/// pick the same one and then have to be pried apart. Deterministic, so the
/// same netlist always routes the same way.
fn jitter(net: usize, node: usize) -> f64 {
    let mut h = (net as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (node as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 31;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    (h >> 40) as f64 / (1u64 << 24) as f64 * 1e-3
}

/// A* from every node of `tree` (cost 0) to `target`, with the distance in
/// tiles as the estimate (every node costs at least one). Returns the path
/// from the tree to the target, starting with a tree node.
///
/// A net may use both tracks of a tile only as a turn or fan-out, where the
/// two are joined inside the tile. So a path may not enter a tile from outside
/// if the net already holds that tile's other track.
fn shortest_path(graph: &Graph, tree: &[usize], target: usize, cost: impl Fn(usize) -> f64) -> Option<Vec<usize>> {
    #[derive(PartialEq)]
    struct Item(f64, usize);
    impl Eq for Item {}
    impl PartialOrd for Item {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for Item {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            other.0.partial_cmp(&self.0).unwrap_or(std::cmp::Ordering::Equal).then(other.1.cmp(&self.1))
        }
    }
    let n = graph.nodes.len();
    let goal = graph.nodes[target].0;
    let estimate = |node: usize| {
        let cell = graph.nodes[node].0;
        f64::from((cell.0 - goal.0).abs() + (cell.1 - goal.1).abs())
    };
    let mut best = vec![f64::INFINITY; n];
    let mut from: Vec<Option<usize>> = vec![None; n];
    let mut heap = BinaryHeap::new();
    for &node in tree {
        best[node] = 0.0;
        heap.push(Item(estimate(node), node));
    }
    while let Some(Item(f, node)) = heap.pop() {
        let d = best[node];
        if f > d + estimate(node) {
            continue;
        }
        if node == target {
            let mut path = vec![node];
            while let Some(p) = from[*path.last().expect("non-empty")] {
                path.push(p);
            }
            path.reverse();
            return Some(path);
        }
        for &next in &graph.neighbours[node] {
            let (cell, track) = graph.nodes[next];
            let from_outside = graph.nodes[node].0 != cell;
            let other = graph.index.get(&(cell, if track == Track::X { Track::Z } else { Track::X })).copied();
            if from_outside && other.is_some_and(|o| tree.contains(&o)) {
                continue;
            }
            let nd = d + cost(next);
            if nd < best[next] {
                best[next] = nd;
                from[next] = Some(node);
                heap.push(Item(nd + estimate(next), next));
            }
        }
    }
    None
}

/// Walks the net from its source, converting straight wire tiles into repeaters
/// where the signal would arrive too weak. Returns the repeater count on the path
/// to each sink.
fn insert_repeaters(
    library: &Library,
    layout: &mut Layout,
    tiles: &Tiles,
    net: NetId,
    source: &Source,
    sinks: &[Sink],
    tree: &Tree,
) -> Result<Vec<u32>, Dies> {
    // Tiles of this net, by cell, with their edges and entry.
    let mut cells: BTreeMap<Cell, (BTreeSet<Edge>, Edge)> = BTreeMap::new();
    for (cell, list) in tiles {
        if let Some((_, edges, entry)) = list.iter().find(|(n, _, _)| *n == net) {
            cells.insert(*cell, (edges.clone(), entry.expect("every routed tile of a net has an entry")));
        }
    }
    // Children by cell (the tree is over nodes; a turn has both nodes in one cell).
    let child_cells = |cell: Cell| -> Vec<Cell> {
        let mut out: Vec<Cell> = tree
            .parent
            .iter()
            .filter(|(node, p)| p.is_some_and(|p| p.0 == cell) && node.0 != cell)
            .map(|(node, _)| node.0)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    };
    // A tile that belongs to this net alone can carry a repeater, and so can a
    // crossing whose flat arm no other net has taken for one.
    let can_repeat = |cell: Cell, layout: &Layout| match layout.tiles.get(&cell) {
        Some(Placed::Cross { repeater_entry: Some(_), .. }) => false,
        Some(Placed::Cross { .. }) => true,
        _ => tiles[&cell].len() == 1,
    };
    let repeater_tile = |cell: Cell, layout: &Layout| -> Placed {
        let (edges, entry) = &cells[&cell];
        match layout.tiles.get(&cell) {
            // The repeater goes in the flat west-east arm: turn the tile when this
            // net runs north-south, and name the entry before the turn.
            Some(Placed::Cross { .. }) => {
                let rotation = cross_turn_for(*entry);
                Placed::Cross { rotation, repeater_entry: Some(unturned(*entry, rotation)), delay: 1 }
            }
            _ => Placed::Repeater { edges: edges.iter().copied().collect(), entry: *entry, delay: 1 },
        }
    };
    // The tile at a cell of this net's as the file describes it (a wire or a
    // crossing), and how it is turned: a crossing is turned a quarter when the
    // repeater it carries, or is about to carry, belongs to a net running
    // north-south. Repeaters of this net are placed after the walk, so the one
    // it is about to carry is read from `repeaters`.
    let tile_at = |cell: Cell, at_repeater: bool, layout: &Layout| -> (&Tile, u8) {
        match layout.tiles.get(&cell) {
            Some(Placed::Cross { rotation, repeater_entry, .. }) => match (at_repeater, repeater_entry) {
                (true, None) => (&library.cross, cross_turn_for(cells[&cell].1)),
                _ => (&library.cross, *rotation),
            },
            _ => (&library.wire, 0),
        }
    };
    let parent_cell = |cell: Cell| -> Option<Cell> {
        let node = tree.parent.keys().find(|n| n.0 == cell && tree.parent[n].is_none_or(|p| p.0 != cell))?;
        tree.parent[node].map(|p| p.0)
    };
    let mut repeaters: BTreeSet<Cell> = BTreeSet::new();
    let root = source.node.0;

    loop {
        // Strength at each cell's entry, in DFS order; stop at the first violation.
        let mut stack = vec![(root, source.strength)];
        let mut violation: Option<Cell> = None;
        let mut sink_counts = vec![0u32; sinks.len()];
        while let Some((cell, v)) = stack.pop() {
            let at_repeater = repeaters.contains(&cell);
            let (tile, rotation) = tile_at(cell, at_repeater, layout);
            // The edges in the tile's own frame.
            let entry = unturned(cells[&cell].1, rotation);
            // What the tile costs from its entry to the port beyond `exit`, and what
            // reaches that port.
            let steps = |exit: Edge| {
                tile.dust_steps(entry, unturned(exit, rotation))
                    .expect("the tile's dust joins the entry to every edge the net leaves by")
            };
            let out = |exit: Edge| {
                if at_repeater {
                    tile.repeater_out(entry, unturned(exit, rotation))
                        .expect("a repeater in the entry arm reaches every exit")
                } else {
                    v.saturating_sub(steps(exit))
                }
            };
            let mut needed = if at_repeater { REPEATER_NEEDS } else { 1 };
            let children = child_cells(cell);
            if !at_repeater {
                // Every child's port must still see one level.
                for child in &children {
                    needed = needed.max(1 + steps(edge_between(cell, *child)));
                }
            }
            for (i, sink) in sinks.iter().enumerate() {
                if sink.node.0 == cell {
                    if at_repeater {
                        // The pin reads the repeater's own output dust: always strong enough.
                    } else {
                        needed = needed.max(sink.needed_after + steps(sink.exit));
                    }
                    // Count repeaters on the path to this sink.
                    let mut c = Some(cell);
                    while let Some(x) = c {
                        if repeaters.contains(&x) {
                            sink_counts[i] += 1;
                        }
                        c = parent_cell(x);
                    }
                }
            }
            if v < needed {
                violation = Some(cell);
                break;
            }
            for child in children {
                stack.push((child, out(edge_between(cell, child))));
            }
        }
        let Some(bad) = violation else {
            for &cell in &repeaters {
                let placed = repeater_tile(cell, layout);
                layout.tiles.insert(cell, placed);
            }
            return Ok(sink_counts);
        };
        // Place a repeater at the latest tile on the way here that can take one and
        // is not one already, including this tile itself; but a tile of this net's
        // own before a crossing, so that the crossing's one slot stays free for the
        // net that has no other choice there.
        let mut candidate = Some(bad);
        let mut crossing: Option<Cell> = None;
        let mut chosen: Option<Cell> = None;
        while let Some(cell) = candidate {
            if repeaters.contains(&cell) {
                break;
            }
            if can_repeat(cell, layout) {
                if tiles[&cell].len() == 1 {
                    chosen = Some(cell);
                    break;
                }
                crossing.get_or_insert(cell);
            }
            candidate = parent_cell(cell);
        }
        let candidate = chosen.or(crossing).or_else(|| {
            // Nothing since the last repeater: anywhere earlier that can take one.
            let mut candidate = Some(bad);
            while let Some(cell) = candidate {
                if can_repeat(cell, layout) && !repeaters.contains(&cell) {
                    break;
                }
                candidate = parent_cell(cell);
            }
            candidate
        });
        match candidate {
            Some(cell) => {
                repeaters.insert(cell);
            }
            None => {
                let mut path = vec![bad];
                while let Some(p) = parent_cell(*path.last().expect("non-empty")) {
                    path.push(p);
                }
                path.reverse();
                return Err(Dies { path });
            }
        }
    }
}

/// How a crossing is turned to carry a repeater for a net entering it by `entry`:
/// the repeater fits only the flat west-east arm, so a quarter turn when the net
/// runs north-south.
fn cross_turn_for(entry: Edge) -> u8 {
    u8::from(matches!(entry, Edge::North | Edge::South))
}

/// A world edge of a tile turned `rotation` quarters, named as the tile file names it.
fn unturned(edge: Edge, rotation: u8) -> Edge {
    edge.rotated((4 - rotation % 4) % 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::{Style, estimate_routing, gap_count, place};
    use crate::tile::Library;

    /// `n` inputs and `n` NOR3 gates, gate k reading inputs k, k+1 and k+3 (mod n):
    /// every input feeds three gates spread over the column, whatever order the
    /// gates are placed in, so the gap in front of the gates needs several routing
    /// columns.
    fn braid(n: usize) -> Netlist {
        let mut ports = Vec::new();
        let mut cells = Vec::new();
        for k in 0..n {
            ports.push(format!(r#""i{k}":{{"direction":"input","bits":[{}]}}"#, 2 + k));
            ports.push(format!(r#""o{k}":{{"direction":"output","bits":[{}]}}"#, 2 + n + k));
            cells.push(format!(
                r#""g{k}":{{"type":"NOR3","connections":{{"A":[{}],"B":[{}],"C":[{}],"Y":[{}]}}}}"#,
                2 + k,
                2 + (k + 1) % n,
                2 + (k + 3) % n,
                2 + n + k
            ));
        }
        let json =
            format!(r#"{{"modules":{{"top":{{"ports":{{{}}},"cells":{{{}}}}}}}}}"#, ports.join(","), cells.join(","));
        Netlist::from_yosys_json(&json, "top", &Library::builtin().unwrap()).unwrap()
    }

    /// The column index of the first routing column of the first gap.
    fn first_routing_column(placement: &Placement) -> i32 {
        placement.columns.iter().position(|c| *c == Column::Routing { gap: 0, offset: 0 }).unwrap() as i32
    }

    /// Whether any tile of column `col` carries a north-south wire (a turn, a
    /// vertical run or a crossing).
    fn runs_north_south_in(layout: &Layout, col: i32) -> bool {
        layout.tiles.iter().any(|(&(c, _), placed)| {
            c == col
                && match placed {
                    Placed::Wire { edges } | Placed::Repeater { edges, .. } => {
                        edges.iter().any(|e| matches!(e, Edge::North | Edge::South))
                    }
                    Placed::Cross { .. } => true,
                    _ => false,
                }
        })
    }

    #[test]
    fn a_gap_too_narrow_is_given_up_after_stalling_and_named() {
        let netlist = braid(5);
        let gaps = gap_count(&netlist, Style::Pitch4).unwrap();
        let narrow = place(&netlist, &vec![1; gaps], Style::Pitch4).unwrap();
        let estimate = estimate_routing(&netlist, &narrow);
        assert!(estimate[0] > 1, "the first gap must need more than one column: {estimate:?}");
        let library = Library::builtin().unwrap();
        let mut log = |_: &str| {};
        let Err(RouteFailure::Congested { tracks, users, rounds, history }) =
            route(&library, &netlist, &narrow, None, &mut log)
        else {
            panic!("one routing column in front of the gates must be congested");
        };
        // Given up by the stall rule, well before the round limit.
        assert!(rounds > STALL_ROUNDS, "the stall rule needs {STALL_ROUNDS} rounds without progress, got {rounds}");
        assert!(rounds <= 3 * STALL_ROUNDS, "gave up only after {rounds} rounds");
        assert!(rounds < MAX_ITERATIONS);
        // Every contended track names the nets that want it, and most of the
        // tracks lie in the gap that is too narrow.
        assert!(!tracks.is_empty());
        assert_eq!(tracks.len(), users.len());
        assert!(users.iter().all(|u| u.len() >= 2), "{users:?}");
        let mut per_gap = vec![0usize; gaps];
        for (cell, _) in &tracks {
            per_gap[narrow.gap_of(cell.0).expect("contended tracks are not in the output column")] += 1;
        }
        assert_eq!(per_gap.iter().position(|&n| n == *per_gap.iter().max().unwrap()), Some(0), "{per_gap:?}");
        // The history carries what was learned, named by column so that it
        // survives the widening, and only where there was contention.
        assert!(history.costs.values().all(|&c| c > 0.0));
        assert!(history.costs.keys().any(|(column, _, _)| matches!(column, Column::Routing { gap: 0, .. })));
        // Widened to the estimate, the placement routes, cold and from the history.
        let wide = place(&netlist, &estimate, Style::Pitch4).unwrap();
        let cold = route(&library, &netlist, &wide, None, &mut log).unwrap();
        let warm = route(&library, &netlist, &wide, Some(&history), &mut log).unwrap();
        assert!(cold.rounds > 0 && warm.rounds > 0);
        assert_eq!(cold.layout.size(), warm.layout.size());
    }

    #[test]
    fn a_seeded_history_steers_nets_off_the_costly_tracks() {
        let netlist = braid(5);
        let gaps = gap_count(&netlist, Style::Pitch4).unwrap();
        let narrow = place(&netlist, &vec![1; gaps], Style::Pitch4).unwrap();
        let wide = place(&netlist, &estimate_routing(&netlist, &narrow), Style::Pitch4).unwrap();
        let first = first_routing_column(&wide);
        let library = Library::builtin().unwrap();
        let mut log = |_: &str| {};
        let cold = route(&library, &netlist, &wide, None, &mut log).unwrap();
        assert!(runs_north_south_in(&cold.layout, first), "cold:\n{}", render(&cold.layout));
        // A history that makes the north-south track of that column prohibitive
        // moves every row change to the other columns.
        let mut steer = History::default();
        for row in 0..wide.rows {
            steer.costs.insert((Column::Routing { gap: 0, offset: 0 }, row, Track::Z), 1e6);
        }
        let steered = route(&library, &netlist, &wide, Some(&steer), &mut log).unwrap();
        assert!(!runs_north_south_in(&steered.layout, first), "steered:\n{}", render(&steered.layout));
    }
}
