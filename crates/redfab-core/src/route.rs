//! Routing: wires between the placed tiles, then repeaters where the signal
//! would otherwise die.
//!
//! Every routing tile offers two tracks, west-east and north-south. A straight
//! wire uses one track; a turn or a fan-out uses both; two straight wires on
//! different tracks share the tile as a crossing. The router is PathFinder:
//! each net is routed by shortest path, and tracks that several nets want
//! become more expensive each round until every track has one net.
//!
//! Signal strength is exact on the tile grid: dust loses one level per block,
//! every tile hop costs five, and each tile's ports are at fixed cells. From
//! that, repeaters are inserted into straight tiles wherever a signal would
//! arrive too weak.

use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use crate::layout::{Layout, Placed};
use crate::netlist::{Driver, NetId, Netlist};
use crate::place::{Cell, Placement};
use crate::tile::Edge;

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

/// Power a gate's output puts on the next tile's entry port: the torch gives 15
/// to the port dust beside it, one less at the neighbour.
const GATE_OUT: u8 = 14;
/// The input tile and the repeater tile hand 12 to the next tile; the constant-1 tile 13.
const INPUT_OUT: u8 = 12;
const REPEATER_OUT: u8 = 12;
const TIE_HIGH_OUT: u8 = 13;
/// A tile is five blocks, so the entry port of the next tile is five levels lower.
const HOP: u8 = 5;
/// What the pins need at their own port cell: a gate's dust must still reach its
/// block with one level, a lamp needs its dust to point into it with one.
const GATE_IN_NEEDS: u8 = 2;
/// A flip-flop's D runs four dust from its port to the master repeater; the
/// clock port's dust powers the stone beside it directly.
const DFF_D_NEEDS: u8 = 5;
const DFF_CLK_NEEDS: u8 = 1;
/// A flip-flop's Q leaves its slave repeater two dust before the port.
const DFF_OUT: u8 = 12;
const LAMP_NEEDS: u8 = 3;
const REPEATER_NEEDS: u8 = 1;

const MAX_ITERATIONS: usize = 80;
/// How many times routing is redone after a signal could not be kept alive.
const MAX_STRENGTH_RETRIES: usize = 30;

/// The routed design.
#[derive(Clone)]
pub struct Routed {
    pub layout: Layout,
    /// Per net, per sink (in `Netlist::sinks_of` order): repeaters on the path from the driver.
    pub repeaters: BTreeMap<NetId, Vec<u32>>,
    /// Per net, per sink: the tiles from the driver to the sink, each with the
    /// edge the signal enters it by. The timing fixes lengthen a path here.
    pub branches: BTreeMap<NetId, Vec<Vec<(Cell, Edge)>>>,
}

/// Routes every net of the placement. Fails if some net cannot be routed, or
/// a signal cannot be kept alive with repeaters.
pub fn route(netlist: &Netlist, placement: &Placement) -> Result<Routed, String> {
    let drivers = netlist.drivers()?;
    let mut nets: Vec<(NetId, Source, Vec<Sink>)> = Vec::new();
    for (net, &driver) in drivers.iter().enumerate() {
        let sinks = netlist.sinks_of(net);
        if sinks.is_empty() {
            continue;
        }
        let source = match driver {
            Driver::Cell(c) => {
                let (col, row) = placement.gates[c];
                let dff = netlist.cells[c].is_dff();
                Source {
                    node: ((col + placement.widths[c], row), Track::X),
                    entry: Edge::West,
                    strength: if dff { DFF_OUT } else { GATE_OUT },
                }
            }
            Driver::Input(i) => {
                let (col, row) = placement.inputs[i];
                Source { node: ((col + 1, row), Track::X), entry: Edge::West, strength: INPUT_OUT }
            }
            Driver::Const(v) => {
                let (_, _, (col, row)) = *placement.ties.iter().find(|t| t.0 == net).ok_or("constant without a tie")?;
                Source {
                    node: ((col + 1, row), Track::X),
                    entry: Edge::West,
                    strength: if v { TIE_HIGH_OUT } else { 0 },
                }
            }
        };
        let sink_list = sinks
            .iter()
            .map(|&(c, pin)| {
                if c == usize::MAX {
                    let (col, row) = placement.outputs[pin];
                    Sink { node: ((col - 1, row), Track::X), exit: Edge::East, needed_after: LAMP_NEEDS }
                } else {
                    let (col, row) = placement.gates[c];
                    let dff = netlist.cells[c].is_dff();
                    match placement.pin_edges[c][pin] {
                        Edge::West => Sink {
                            node: ((col - 1, row), Track::X),
                            exit: Edge::East,
                            needed_after: if dff { DFF_D_NEEDS } else { GATE_IN_NEEDS },
                        },
                        Edge::North => Sink {
                            node: ((col, row - 1), Track::Z),
                            exit: Edge::South,
                            needed_after: if dff { DFF_CLK_NEEDS } else { GATE_IN_NEEDS },
                        },
                        Edge::South => {
                            Sink { node: ((col, row + 1), Track::Z), exit: Edge::North, needed_after: GATE_IN_NEEDS }
                        }
                        Edge::East => unreachable!("gates never take input on the east"),
                    }
                }
            })
            .collect();
        nets.push((net, source, sink_list));
    }

    let graph = Graph::new(placement);
    // Tracks that a net could not get a repeater past (crossings) get more expensive
    // for that net each time, until every signal survives.
    let mut penalties: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); nets.len()];
    let mut retries = 0;
    loop {
        let trees = pathfinder(&graph, &nets, &penalties)?;
        match realize_trees(netlist, placement, &nets, &trees) {
            Ok(routed) => return Ok(routed),
            Err(Failure::Strength { net_index, crossings }) if retries < MAX_STRENGTH_RETRIES => {
                retries += 1;
                for node in crossings {
                    let index = graph.index[&node];
                    *penalties[net_index].entry(index).or_insert(0.0) += 4.0;
                }
            }
            Err(Failure::Strength { .. }) => {
                return Err(
                    "a signal cannot be kept alive: every path it finds runs through too many crossings".to_string()
                );
            }
            Err(Failure::Other(e)) => return Err(e),
        }
    }
}

/// What each tile carries: per net, the edges it uses and the one the signal enters by.
type Tiles = BTreeMap<Cell, Vec<(NetId, BTreeSet<Edge>, Option<Edge>)>>;

/// Why a routing attempt could not be turned into tiles.
enum Failure {
    /// Repeater insertion failed for net `net_index`; `crossings` are the nodes it
    /// used in crossing tiles on the failing path.
    Strength {
        net_index: usize,
        crossings: Vec<Node>,
    },
    Other(String),
}

/// Turns routed trees into tiles and inserts repeaters.
fn realize_trees(
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

    let mut layout = Layout::default();
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

    // Repeaters: walk each net from its source with exact strengths.
    let mut repeater_counts: BTreeMap<NetId, Vec<u32>> = BTreeMap::new();
    let mut branches: BTreeMap<NetId, Vec<Vec<(Cell, Edge)>>> = BTreeMap::new();
    for (net_index, ((net, source, sinks), tree)) in nets.iter().zip(trees).enumerate() {
        // A constant 0 carries no signal, so nothing can die on the way.
        let counts = if source.strength == 0 {
            vec![0; sinks.len()]
        } else {
            match insert_repeaters(&mut layout, &tiles, *net, source, sinks, tree) {
                Ok(counts) => counts,
                Err(Dies { path }) => {
                    let crossings = path
                        .iter()
                        .filter(|cell| tiles[*cell].len() > 1)
                        .flat_map(|cell| tree.parent.keys().filter(|n| n.0 == *cell).copied().collect::<Vec<_>>())
                        .collect();
                    return Err(Failure::Strength { net_index, crossings });
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
    Ok(Routed { layout, repeaters: repeater_counts, branches })
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

/// Negotiated-congestion routing of all nets. `penalties` adds a per-net cost to
/// particular nodes.
fn pathfinder(
    graph: &Graph,
    nets: &[(NetId, Source, Vec<Sink>)],
    penalties: &[BTreeMap<usize, f64>],
) -> Result<Vec<Tree>, String> {
    let n = graph.nodes.len();
    let mut history = vec![0.0f64; n];
    let mut occupancy = vec![0u32; n];
    let mut trees: Vec<Vec<usize>> = vec![Vec::new(); nets.len()];
    let mut parents: Vec<BTreeMap<usize, Option<usize>>> = vec![BTreeMap::new(); nets.len()];
    let mut present_factor = 0.5;

    let lookup = |node: Node| -> Result<usize, String> {
        graph.index.get(&node).copied().ok_or_else(|| format!("no routing tile at {:?}", node.0))
    };

    for iteration in 0..MAX_ITERATIONS {
        for (k, (_, source, sinks)) in nets.iter().enumerate() {
            for &node in &trees[k] {
                occupancy[node] -= 1;
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
                let path = shortest_path(graph, &tree, target, |node| {
                    let overuse = occupancy[node] as f64; // this net's own nodes are not in occupancy now
                    let penalty = penalties[k].get(&node).copied().unwrap_or(0.0);
                    (1.0 + history[node] + penalty) * (1.0 + present_factor * overuse)
                })
                .ok_or_else(|| format!("net cannot reach the tile at {:?}", graph.nodes[target].0))?;
                for window in path.windows(2) {
                    if let std::collections::btree_map::Entry::Vacant(slot) = parent.entry(window[1]) {
                        slot.insert(Some(window[0]));
                        tree.push(window[1]);
                    }
                }
            }
            for &node in &tree {
                occupancy[node] += 1;
            }
            trees[k] = tree;
            parents[k] = parent;
        }
        let overused: Vec<usize> = (0..n).filter(|&i| occupancy[i] > 1).collect();
        if overused.is_empty() {
            return Ok(parents
                .iter()
                .map(|p| Tree {
                    parent: p.iter().map(|(&node, &par)| (graph.nodes[node], par.map(|q| graph.nodes[q]))).collect(),
                })
                .collect());
        }
        for &i in &overused {
            history[i] += f64::from(occupancy[i] - 1);
        }
        present_factor = (present_factor * 1.6).min(1e6);
        if iteration + 1 == MAX_ITERATIONS {
            return Err(format!(
                "routing did not converge: {} tracks are still wanted by several nets",
                overused.len()
            ));
        }
    }
    unreachable!()
}

/// Dijkstra from every node of `tree` (cost 0) to `target`. Returns the path
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
    let mut best = vec![f64::INFINITY; n];
    let mut from: Vec<Option<usize>> = vec![None; n];
    let mut heap = BinaryHeap::new();
    for &node in tree {
        best[node] = 0.0;
        heap.push(Item(0.0, node));
    }
    while let Some(Item(d, node)) = heap.pop() {
        if d > best[node] {
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
                heap.push(Item(nd, next));
            }
        }
    }
    None
}

/// Walks the net from its source, converting straight wire tiles into repeaters
/// where the signal would arrive too weak. Returns the repeater count on the path
/// to each sink.
fn insert_repeaters(
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
                let rotation = if matches!(entry, Edge::West | Edge::East) { 0 } else { 1 };
                let local = Edge::ALL
                    .into_iter()
                    .find(|e| e.rotated(rotation) == *entry)
                    .expect("an edge turns onto the entry");
                Placed::Cross { rotation, repeater_entry: Some(local), delay: 1 }
            }
            _ => Placed::Repeater { edges: edges.iter().copied().collect(), entry: *entry, delay: 1 },
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
            let out = if at_repeater { REPEATER_OUT } else { v.saturating_sub(HOP) };
            let mut needed = if at_repeater { REPEATER_NEEDS } else { 1 };
            let children = child_cells(cell);
            if !children.is_empty() && !at_repeater {
                needed = needed.max(1 + HOP);
            }
            for (i, sink) in sinks.iter().enumerate() {
                if sink.node.0 == cell {
                    if at_repeater {
                        // The pin reads the repeater's own output dust: always strong enough.
                    } else {
                        needed = needed.max(sink.needed_after + HOP);
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
                stack.push((child, out));
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
        // is not one already, including this tile itself.
        let mut candidate = Some(bad);
        while let Some(cell) = candidate {
            if can_repeat(cell, layout) && !repeaters.contains(&cell) {
                break;
            }
            candidate = parent_cell(cell);
        }
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
