//! Routing: wires between the placed tiles, then repeaters where the signal
//! would otherwise die.
//!
//! Every routing tile has two wire levels (cells/README.md), and each level two
//! tracks, west-east and north-south. On the lower level a straight uses one
//! track and a turn or a fan-out both, and two straights on different tracks
//! share the tile as a crossing, as they always have. The upper level is one
//! net's: any shape, above whatever wiring runs below (a stack), except a
//! crossing, whose bridge stands where the upper level would. A ramp takes a
//! net from one level to the other and holds its whole tile. So the resources
//! of a tile are three slots, the lower west-east and north-south tracks and the
//! upper level. The router is PathFinder: each net is routed by shortest path,
//! and slots that several nets want become more expensive each round until
//! every one has one net. A run whose count of contended slots stops falling is
//! given up, and reports which are still wanted so that the gap they lie in can
//! be widened; the history costs it built up carry over to the next attempt.
//!
//! Signal strength is exact on the tile grid: dust loses one level per block,
//! and each tile says what a hop through it costs, what its own driver hands
//! on and what its pins need ([`Tile::dust_steps`] and the methods beside it,
//! read off the tile files). From that, repeaters are inserted wherever a
//! signal would arrive too weak: each as late as it can go, then, per net, the
//! fewest the tree allows without any sink getting more on its path.

use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use crate::layout::{Layout, Placed};
use crate::netlist::{Driver, NetId, Netlist};
use crate::place::{Cell, Column, Placement};
use crate::tile::{Edge, Kind, Library, PITCH, Tile};

/// One of the two wire levels.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Lower,
    Upper,
}

/// What a tile offers the router: the two tracks of the lower level, which two
/// nets may hold as a crossing, and the upper level, which is one net's.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Slot {
    LowerX = 0,
    LowerZ = 1,
    Upper = 2,
}

/// A track in a routing tile: west-east or north-south on either level, or a
/// ramp, named by the edge its lower port is on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Track {
    X,
    Z,
    UpperX,
    UpperZ,
    Ramp(Edge),
}

impl Track {
    const ALL: [Track; 8] = [
        Track::X,
        Track::Z,
        Track::UpperX,
        Track::UpperZ,
        Track::Ramp(Edge::West),
        Track::Ramp(Edge::East),
        Track::Ramp(Edge::North),
        Track::Ramp(Edge::South),
    ];

    /// The slots of its tile the track holds: one, or all three for a ramp.
    fn slots(self) -> &'static [Slot] {
        match self {
            Track::X => &[Slot::LowerX],
            Track::Z => &[Slot::LowerZ],
            Track::UpperX | Track::UpperZ => &[Slot::Upper],
            Track::Ramp(_) => &[Slot::LowerX, Slot::LowerZ, Slot::Upper],
        }
    }

    /// The same track on the upper level.
    fn upper(self) -> Track {
        match self {
            Track::X => Track::UpperX,
            Track::Z => Track::UpperZ,
            other => other,
        }
    }

    /// Where in its tile a net on this track is.
    fn at(self) -> At {
        match self {
            Track::X | Track::Z => At::Level(Level::Lower),
            Track::UpperX | Track::UpperZ => At::Level(Level::Upper),
            Track::Ramp(lower) => At::Ramp(lower),
        }
    }
}

/// Where in a tile a net runs: on one wire level, or on a ramp between them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum At {
    Level(Level),
    Ramp(Edge),
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

/// What a repeater costs in the exact repeater placement: far more than the one
/// more a repeater in a crossing costs, so that the count decides first.
const REPEATER_COST: u32 = 1000;

/// What a hop costs the router that makes a crossing that cannot turn (in a
/// narrow column or a shallow row), against one for a plain hop and two for a
/// ramp: the crossing's climbing net can take no repeater there, so a run
/// across many dies, and is cheaper on the upper level. Measured over adder8, adder4, counter4 and
/// mem2 at 2, 3, 4, 6, 8 and 100: 4 gave the least footprint in all but counter4,
/// and above it the router stalls more often than it gains.
const FIXED_CROSSING_COST: f64 = 4.0;

/// The most crossings in a row a net may climb where the climbing arm can take
/// no repeater (in a narrow column, or along a shallow row): twelve after a
/// repeater in a straight, five lost in each crossing, and the tile after them
/// needs at least one (pinned against the tile files by a test). The
/// negotiation counts a longer run as contended.
const MAX_CLIMBED: usize = 2;

/// How much the A* search weighs the distance still to go against the cost so
/// far: above one it explores less of the grid and may miss the cheapest path
/// by a little, which the negotiation's rounds make up for. Measured with rows
/// three deep over the 23 shipped designs and the 5 capacity ones at 1, 1.2 and
/// 1.5: 1.5 built them in 582 s against 668 (adder8 28 s against 47), with
/// 2.7 % less area shipped and 2 % more on the capacity designs; 1.2 sent alu8
/// to the four-row pitch.
const ASTAR_WEIGHT: f64 = 1.5;

/// The most rounds of negotiation in one run.
const MAX_ITERATIONS: usize = 200;
/// Rounds without the number of contended tracks falling below its best so far
/// before a run is given up as stuck.
const STALL_ROUNDS: usize = 12;
/// How many times routing is redone after a signal could not be kept alive.
const MAX_STRENGTH_RETRIES: usize = 100;
/// After this many, if the signals that keep dying run through shallow rows,
/// routing gives up and names those rows, so that the build can make them a
/// cell deep (where turns and crossings take repeaters) instead of searching on.
const RETRIES_BEFORE_DEEPENING: usize = 20;

/// The tiles from a net's driver to one of its sinks, each with the edge the
/// signal enters it by and the level it runs on.
pub type Branch = Vec<(Cell, Edge, Level)>;

/// The routed design.
#[derive(Clone)]
pub struct Routed {
    pub layout: Layout,
    /// Per net, per sink (in `Netlist::sinks_of` order): repeaters on the path from the driver.
    pub repeaters: BTreeMap<NetId, Vec<u32>>,
    /// Per net, per sink: the tiles from the driver to the sink, each with the
    /// edge the signal enters it by and the level it runs on (the lower one for
    /// a ramp). The timing fixes lengthen a path here.
    pub branches: BTreeMap<NetId, Vec<Branch>>,
    /// How many rounds of negotiation the final run took.
    pub rounds: usize,
}

/// PathFinder's history costs, named by column kind, row and slot so that they
/// survive a gap being widened (the tiles keep their names).
#[derive(Clone, Debug, Default)]
pub struct History {
    pub costs: BTreeMap<(Column, i32, Slot), f64>,
}

/// Why routing failed.
#[derive(Debug)]
pub enum RouteFailure {
    /// The negotiation got stuck: these tracks are still wanted by several nets
    /// (`users[i]` names the nets at `tracks[i]`).
    Congested { tracks: Vec<Node>, users: Vec<Vec<NetId>>, rounds: usize, history: History },
    /// Signals keep dying in shallow rows, where only a straight takes a
    /// repeater in a routing column: per row, how many tiles that could not
    /// take one the dying signals ran through, most first.
    Dying { rows: Vec<(i32, usize)>, history: History },
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
            RouteFailure::Dying { rows, .. } => {
                write!(f, "signals keep dying in {} shallow rows: no repeater fits where they run", rows.len())
            }
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
    route_with_seed(library, netlist, placement, history, 0, log)
}

/// [`route`] with `seed` breaking the ties between equal-cost paths: seed 0 is
/// [`route`] itself, another seed another trajectory through the negotiation,
/// and so, often, another layout of the same placement.
pub fn route_with_seed(
    library: &Library,
    netlist: &Netlist,
    placement: &Placement,
    history: Option<&History>,
    seed: u64,
    log: &mut dyn FnMut(&str),
) -> Result<Routed, RouteFailure> {
    let drivers = netlist.drivers().map_err(RouteFailure::Other)?;
    // What a pin needs at the port of the tile in front of it.
    let needs = |tile: &Tile, pin: Edge| -> Result<u8, RouteFailure> {
        tile.port_needs(pin)
            .ok_or_else(|| RouteFailure::Other(format!("the {} tile has no {} input", tile.name, pin.letter())))
    };
    // The tile a part at `col`, `row` uses: the full cell's, or the one sized to
    // its column's width and its row's depth (narrow in the sources' and the
    // outputs' columns, shallow in a row no flip-flop sits in).
    let in_cell = |kind: Kind, col: i32, row: i32| -> &Tile {
        let width = placement.col_width.get(col as usize).copied().unwrap_or(PITCH);
        let depth = placement.row_depth.get(row as usize).copied().unwrap_or(PITCH);
        library.get(kind.sized(width, depth).unwrap_or(kind))
    };
    let gate_tile = |c: usize, (col, row): (i32, i32)| {
        if netlist.cells[c].is_dff() { &library.dff } else { in_cell(Kind::Nor, col, row) }
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
                Source {
                    node: ((col + placement.widths[c], row), Track::X),
                    entry: Edge::West,
                    strength: gate_tile(c, (col, row)).source_out(Edge::East),
                }
            }
            Driver::Input(i) => {
                let (col, row) = placement.inputs[i];
                Source {
                    node: ((col + 1, row), Track::X),
                    entry: Edge::West,
                    strength: in_cell(Kind::Input, col, row).source_out(Edge::East),
                }
            }
            Driver::Const(v) => {
                let (_, _, (col, row)) = *placement
                    .ties
                    .iter()
                    .find(|t| t.0 == net)
                    .ok_or_else(|| RouteFailure::Other("constant without a tie".to_string()))?;
                let tile = in_cell(if v { Kind::TieHigh } else { Kind::TieLow }, col, row);
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
                    needed_after: needs(in_cell(Kind::Output, col, row), Edge::West)?,
                }
            } else {
                let (col, row) = placement.gates[c];
                let edge = placement.pin_edges[c][pin];
                let needed_after = needs(gate_tile(c, (col, row)), edge)?;
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
    let key = |(cell, slot): (Cell, Slot)| (placement.column(cell.0), cell.1, slot);
    let start: Vec<f64> =
        graph.slot_of.iter().map(|&s| history.and_then(|h| h.costs.get(&key(s)).copied()).unwrap_or(0.0)).collect();
    let mut negotiation = Negotiation::new(&graph, &nets, start, seed);
    // Tiles that a net could not get a repeater past (ramps) get more expensive
    // for that net each time, until every signal survives. The negotiation picks
    // up where it was: only that net has to move, and whatever it then collides with.
    let learned = |negotiation: &Negotiation| History {
        costs: graph
            .slot_of
            .iter()
            .zip(&negotiation.history)
            .filter(|(_, c)| **c > 0.0)
            .map(|(&s, &c)| (key(s), c))
            .collect(),
    };
    let shallow = |row: i32| placement.row_depth.get(row as usize).is_some_and(|&d| d < PITCH);
    let mut retries = 0;
    // Per row, the tiles that could not take a repeater on the way of a dying signal.
    let mut dying_rows: BTreeMap<i32, usize> = BTreeMap::new();
    loop {
        let (trees, rounds) = match negotiation.run(log) {
            Ok(done) => done,
            Err(Stuck::Unroutable(e)) => return Err(RouteFailure::Other(e)),
            Err(Stuck::Congested { overused, rounds }) => {
                let history = learned(&negotiation);
                // Each contended slot, named by a track that holds it.
                let tracks = overused
                    .iter()
                    .map(|&i| {
                        let (cell, slot) = graph.slot_of[i];
                        let track = match slot {
                            Slot::LowerX => Track::X,
                            Slot::LowerZ => Track::Z,
                            Slot::Upper => Track::UpperX,
                        };
                        (cell, track)
                    })
                    .collect();
                // The nets that want each: its holders, and for an upper slot above a
                // crossing the crossing's nets too.
                let users = overused
                    .iter()
                    .map(|&i| {
                        let base = i - graph.slot_of[i].1 as usize;
                        let mut wanting: BTreeSet<usize> = negotiation.holders[i].iter().copied().collect();
                        if crossed(&negotiation.holders, base) {
                            wanting.extend(negotiation.holders[base..base + 2].iter().flatten().copied());
                        }
                        wanting.into_iter().map(|k| nets[k].0).collect()
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
            Err(failure @ (Failure::Strength(_) | Failure::Twice(_))) if retries < MAX_STRENGTH_RETRIES => {
                retries += 1;
                let failed = match failure {
                    Failure::Strength(failed) => {
                        for (cell, _) in failed.iter().flat_map(|(_, nodes)| nodes) {
                            *dying_rows.entry(cell.1).or_default() += 1;
                        }
                        log(&format!("  round {rounds}: a signal dies in {} nets; routing them again", failed.len()));
                        failed
                    }
                    Failure::Twice(failed) => {
                        log(&format!(
                            "  round {rounds}: {} nets come back into a tile; routing them again",
                            failed.len()
                        ));
                        failed
                    }
                    Failure::Other(_) => unreachable!("matched above"),
                };
                if retries >= RETRIES_BEFORE_DEEPENING && dying_rows.keys().any(|&row| shallow(row)) {
                    let mut rows: Vec<(i32, usize)> =
                        dying_rows.iter().filter(|(row, _)| shallow(**row)).map(|(&r, &n)| (r, n)).collect();
                    rows.sort_by_key(|&(row, n)| (std::cmp::Reverse(n), row));
                    return Err(RouteFailure::Dying { rows, history: learned(&negotiation) });
                }
                // Heavier each time, so that the nets leave their runs of ramps
                // rather than shuffle along them.
                let weight = 4.0 * f64::from(retries.min(16) as u8);
                for (net_index, ramps) in failed {
                    for node in ramps {
                        let index = graph.index[&node];
                        *negotiation.penalties[net_index].entry(index).or_insert(0.0) += weight;
                    }
                    negotiation.dirty.push(net_index);
                }
                // Let the nets push others aside again rather than keep their paths.
                negotiation.present_factor = 0.5;
            }
            Err(Failure::Strength(_) | Failure::Twice(_)) => {
                return Err(RouteFailure::Other(
                    "a signal cannot be kept alive: every path it finds runs through too many ramps".to_string(),
                ));
            }
            Err(Failure::Other(e)) => return Err(RouteFailure::Other(e)),
        }
    }
}

/// What each tile carries, per level or ramp: per net, the edges it uses and
/// the one the signal enters by.
type Tiles = BTreeMap<(Cell, At), Vec<(NetId, BTreeSet<Edge>, Option<Edge>)>>;

/// One net's tiles: the edges it uses in each, the edge it enters by, where in
/// the tile it runs (a net holds one level or a ramp of a tile), and whether it
/// crosses another net there.
type NetTiles = BTreeMap<Cell, (BTreeSet<Edge>, Edge, At, bool)>;

/// Why a routing attempt could not be turned into tiles.
enum Failure {
    /// These nets (by index into the net list) must be routed again, each with
    /// the nodes that failed it made more expensive for it: the tiles a dying
    /// signal ran through that could not take a repeater.
    Strength(Vec<(usize, Vec<Node>)>),
    /// The same for a path that came back into a tile it had passed.
    Twice(Vec<(usize, Vec<Node>)>),
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
    // A net holds one place in a tile, a level or a ramp; a path that came back
    // into a tile it had passed is routed again with the nodes it holds there
    // made more expensive for it.
    let mut twice: Vec<(usize, Vec<Node>)> = Vec::new();
    for (net_index, tree) in trees.iter().enumerate() {
        let mut at_of: BTreeMap<Cell, BTreeSet<At>> = BTreeMap::new();
        for &(cell, track) in tree.parent.keys() {
            at_of.entry(cell).or_default().insert(track.at());
        }
        let nodes: Vec<Node> = tree.parent.keys().filter(|(cell, _)| at_of[cell].len() > 1).copied().collect();
        if !nodes.is_empty() {
            twice.push((net_index, nodes));
        }
    }
    if !twice.is_empty() {
        return Err(Failure::Twice(twice));
    }
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
            // Both tracks of a level, if this net uses them, mean a turn: all the
            // edges of the level belong to one wire; record under the level once.
            let list = tiles.entry((cell, track.at())).or_default();
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
        upper: BTreeMap::new(),
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
    for ((cell, at), list) in &tiles {
        let straight = |e: &BTreeSet<Edge>| {
            *e == BTreeSet::from([Edge::West, Edge::East]) || *e == BTreeSet::from([Edge::North, Edge::South])
        };
        let placed = match (at, list.as_slice()) {
            (At::Ramp(lower), [_]) => Placed::Ramp { lower: *lower },
            (_, [(_, edges, _)]) => Placed::Wire { edges: edges.iter().copied().collect() },
            (At::Level(Level::Lower), [a, b]) if straight(&a.1) && straight(&b.1) && a.1 != b.1 => {
                Placed::Cross { rotation: 0, repeater_entry: None, delay: 1 }
            }
            _ => return Err(Failure::Other(format!("nets meet in tile {cell:?} other than as a crossing or a stack"))),
        };
        let earlier = match at {
            At::Level(Level::Lower) => layout.tiles.insert(*cell, placed),
            At::Level(Level::Upper) => layout.upper.insert(*cell, placed),
            At::Ramp(_) => layout.tiles.insert(*cell, placed).or(layout.upper.get(cell).cloned()),
        };
        let above_a_part =
            matches!(at, At::Level(Level::Upper)) && layout.tiles.get(cell).is_some_and(|p| !p.may_be_covered());
        if earlier.is_some() || above_a_part {
            return Err(Failure::Other(format!(
                "a ramp shares tile {cell:?}, or wiring runs above a gate or a crossing"
            )));
        }
    }
    // Each net's tiles: its edges, the edge it enters by and where it runs.
    let mut by_net: BTreeMap<NetId, NetTiles> = BTreeMap::new();
    for ((cell, at), list) in &tiles {
        for (net, edges, entry) in list {
            let entry = entry.expect("every routed tile of a net has an entry");
            by_net.entry(*net).or_default().insert(*cell, (edges.clone(), entry, *at, list.len() > 1));
        }
    }

    // Repeaters: walk each net from its source with exact strengths. Every net
    // whose signal dies is reported, so that one re-route moves them all.
    let mut repeater_counts: BTreeMap<NetId, Vec<u32>> = BTreeMap::new();
    let mut branches: BTreeMap<NetId, Vec<Branch>> = BTreeMap::new();
    let mut died: Vec<(usize, Vec<Node>)> = Vec::new();
    for (net_index, ((net, source, sinks), tree)) in nets.iter().zip(trees).enumerate() {
        // A constant 0 carries no signal, so nothing can die on the way.
        let counts = if source.strength == 0 {
            vec![0; sinks.len()]
        } else {
            match insert_repeaters(library, &mut layout, &by_net[net], source, sinks, tree) {
                Ok(counts) => counts,
                Err(Dies { stuck }) => {
                    // The nodes it used in the tiles on the way that could not take a repeater.
                    let stuck = stuck
                        .iter()
                        .flat_map(|cell| tree.parent.keys().filter(|n| n.0 == *cell).copied().collect::<Vec<_>>())
                        .collect();
                    died.push((net_index, stuck));
                    continue;
                }
            }
        };
        repeater_counts.insert(*net, counts);
        // The branch to each sink: from the sink's tile back to the source's.
        let found = |cell: Cell| -> Option<(Edge, Level)> {
            let (_, entry, at, _) = by_net.get(net)?.get(&cell)?;
            Some((*entry, if *at == At::Level(Level::Upper) { Level::Upper } else { Level::Lower }))
        };
        let parent_cell = |cell: Cell| -> Option<Cell> {
            let node = tree.parent.keys().find(|n| n.0 == cell && tree.parent[n].is_none_or(|p| p.0 != cell))?;
            tree.parent[node].map(|p| p.0)
        };
        let mut per_sink = Vec::new();
        for sink in sinks {
            let mut path = Vec::new();
            let mut cell = Some(sink.node.0);
            while let Some(c) = cell {
                let (entry, level) = found(c).unwrap_or((source.entry, Level::Lower));
                path.push((c, entry, level));
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

/// A signal that could not be kept alive: the tiles on the way from the source
/// to where it died that could not take a repeater (crossings taken, ramps, and
/// where an arm is the centre, its fan-outs, and turns into an arm that is too).
struct Dies {
    stuck: Vec<Cell>,
}

/// A text picture of a layout: one character per tile. A tile with wiring on
/// both levels is a heavy cross, upper wiring alone is drawn in heavy lines,
/// and a ramp is `^`.
pub fn render(layout: &Layout) -> String {
    let (cols, rows) = layout.size();
    let mut out = String::new();
    for row in 0..rows {
        for col in 0..cols {
            let upper = layout.upper.get(&(col, row));
            let c = match (layout.tiles.get(&(col, row)), upper) {
                (Some(_), Some(_)) => '╋',
                (None, Some(Placed::Wire { edges } | Placed::Repeater { edges, .. })) => heavy(wire_char(edges, false)),
                (_, Some(_)) => '?',
                (below, None) => match below {
                    None => '.',
                    Some(Placed::Ramp { .. }) => '^',
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
                },
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

/// The heavy form of a light box-drawing character.
fn heavy(c: char) -> char {
    match c {
        '─' => '━',
        '│' => '┃',
        '┌' => '┏',
        '┐' => '┓',
        '└' => '┗',
        '┘' => '┛',
        '├' => '┣',
        '┤' => '┫',
        '┬' => '┳',
        '┴' => '┻',
        '┼' => '╋',
        other => other,
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

/// The routing graph: every track of every free tile on both wire levels, joined
/// to the same track of the neighbouring tiles and to the other track of its own
/// tile on its level, and the four ways a ramp can stand in the tile, each joined
/// to the lower track of the neighbour across its lower edge and the upper track
/// of the neighbour across the opposite one. Every node holds one slot of its
/// tile, or a ramp all three.
struct Graph {
    nodes: Vec<Node>,
    index: BTreeMap<Node, usize>,
    neighbours: Vec<Vec<usize>>,
    /// Per node, the slots it holds: `3 * cell + slot`, and its tile's first slot.
    slots: Vec<Vec<usize>>,
    base: Vec<usize>,
    /// Per slot, its cell and which slot of the tile it is.
    slot_of: Vec<(Cell, Slot)>,
    /// Per slot, whether its tile can hold a crossing: one arm climbs over the
    /// other, which takes five blocks along it, so a tile three by three cannot.
    crossable: Vec<bool>,
    /// Per slot, the lower track whose net climbs a crossing in its tile and can
    /// never take a repeater there, the tile being unable to turn: north-south
    /// in a narrow column, west-east in a shallow row. None in a full cell,
    /// whose crossing turns to give either net the flat arm.
    climbs: Vec<Option<Slot>>,
}

impl Graph {
    fn new(placement: &Placement) -> Graph {
        let mut nodes = Vec::new();
        let mut index = BTreeMap::new();
        let mut slot_of = Vec::new();
        let mut slots = Vec::new();
        let mut base = Vec::new();
        let mut crossable = Vec::new();
        let mut climbs = Vec::new();
        for row in 0..placement.rows {
            for col in 0..placement.cols {
                if !placement.is_free((col, row)) {
                    continue;
                }
                let first = slot_of.len();
                let narrow = placement.col_width.get(col as usize).is_some_and(|&w| w < PITCH);
                let shallow = placement.row_depth.get(row as usize).is_some_and(|&d| d < PITCH);
                for slot in [Slot::LowerX, Slot::LowerZ, Slot::Upper] {
                    slot_of.push(((col, row), slot));
                    crossable.push(!(narrow && shallow));
                    climbs.push(match (narrow, shallow) {
                        (true, false) => Some(Slot::LowerZ),
                        (false, true) => Some(Slot::LowerX),
                        _ => None,
                    });
                }
                for track in Track::ALL {
                    index.insert(((col, row), track), nodes.len());
                    nodes.push(((col, row), track));
                    slots.push(track.slots().iter().map(|&s| first + s as usize).collect());
                    base.push(first);
                }
            }
        }
        let mut neighbours = vec![Vec::new(); nodes.len()];
        for (i, &(cell, track)) in nodes.iter().enumerate() {
            let at = |edge: Edge, track: Track| -> Node {
                let (dc, dr) = edge.step();
                ((cell.0 + dc, cell.1 + dr), track)
            };
            // A ramp across `edge` whose lower port faces this cell, and one whose upper port does.
            let ramp_lower_facing = |edge: Edge| at(edge, Track::Ramp(edge.opposite()));
            let ramp_upper_facing = |edge: Edge| at(edge, Track::Ramp(edge));
            let candidates: Vec<Node> = match track {
                Track::X => vec![
                    at(Edge::West, Track::X),
                    at(Edge::East, Track::X),
                    (cell, Track::Z),
                    ramp_lower_facing(Edge::West),
                    ramp_lower_facing(Edge::East),
                ],
                Track::Z => vec![
                    at(Edge::North, Track::Z),
                    at(Edge::South, Track::Z),
                    (cell, Track::X),
                    ramp_lower_facing(Edge::North),
                    ramp_lower_facing(Edge::South),
                ],
                Track::UpperX => vec![
                    at(Edge::West, Track::UpperX),
                    at(Edge::East, Track::UpperX),
                    (cell, Track::UpperZ),
                    ramp_upper_facing(Edge::West),
                    ramp_upper_facing(Edge::East),
                ],
                Track::UpperZ => vec![
                    at(Edge::North, Track::UpperZ),
                    at(Edge::South, Track::UpperZ),
                    (cell, Track::UpperX),
                    ramp_upper_facing(Edge::North),
                    ramp_upper_facing(Edge::South),
                ],
                Track::Ramp(lower) => {
                    let along = if matches!(lower, Edge::West | Edge::East) { Track::X } else { Track::Z };
                    vec![at(lower, along), at(lower.opposite(), along.upper())]
                }
            };
            neighbours[i] = candidates.into_iter().filter_map(|n| index.get(&n).copied()).collect();
        }
        Graph { nodes, index, neighbours, slots, base, slot_of, crossable, climbs }
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
/// every slot to one net, the caller may penalise nodes for a net and run
/// again, and only that net and whatever it then collides with move. A slot is
/// one net's; and a crossing (the two lower tracks of a tile held by two nets)
/// leaves no room for an upper wire, so that too counts against the upper slot.
struct Negotiation<'a> {
    graph: &'a Graph,
    nets: &'a [(NetId, Source, Vec<Sink>)],
    /// Per slot: the nets that hold it.
    holders: Vec<Vec<usize>>,
    /// Per net: its nodes, each node's parent, and the slots they hold.
    trees: Vec<Vec<usize>>,
    parents: Vec<BTreeMap<usize, Option<usize>>>,
    held: Vec<BTreeSet<usize>>,
    /// Per slot: the history cost, seeded by the caller.
    history: Vec<f64>,
    /// Per net: extra cost on particular nodes.
    penalties: Vec<BTreeMap<usize, f64>>,
    /// Nets that must be routed again whatever their congestion.
    dirty: Vec<usize>,
    present_factor: f64,
    /// Seeds the tie-breaker between equal-cost paths (`jitter`): another seed,
    /// another trajectory through the same placement.
    seed: u64,
    rounds: usize,
    search: Search,
}

/// Whether the two lower tracks of the tile whose first slot is `base` are
/// held by different nets (a crossing).
fn crossed(holders: &[Vec<usize>], base: usize) -> bool {
    let x = &holders[base + Slot::LowerX as usize];
    let z = &holders[base + Slot::LowerZ as usize];
    // Both held, by two nets at least: then some holder of one differs from some
    // holder of the other (no set is built; this runs for every node A* weighs).
    x.iter().any(|a| z.iter().any(|b| a != b))
}

impl<'a> Negotiation<'a> {
    fn new(graph: &'a Graph, nets: &'a [(NetId, Source, Vec<Sink>)], history: Vec<f64>, seed: u64) -> Negotiation<'a> {
        Negotiation {
            graph,
            nets,
            holders: vec![Vec::new(); graph.slot_of.len()],
            trees: vec![Vec::new(); nets.len()],
            parents: vec![BTreeMap::new(); nets.len()],
            held: vec![BTreeSet::new(); nets.len()],
            history,
            penalties: vec![BTreeMap::new(); nets.len()],
            dirty: (0..nets.len()).collect(),
            present_factor: 0.5,
            seed,
            rounds: 0,
            search: Search::new(graph),
        }
    }

    /// The slots wanted by more than one net: held by several, the upper slots
    /// above a crossing, the lower tracks of a tile crossed that cannot hold a
    /// crossing, and both lower tracks of every tile of a run of crossings that
    /// cannot turn longer than one net can climb.
    fn overused(&self) -> Vec<usize> {
        let mut out: BTreeSet<usize> = (0..self.holders.len()).filter(|&i| self.holders[i].len() > 1).collect();
        for (i, &(_, slot)) in self.graph.slot_of.iter().enumerate() {
            if slot != Slot::Upper || !crossed(&self.holders, i - Slot::Upper as usize) {
                continue;
            }
            if !self.holders[i].is_empty() {
                out.insert(i);
            }
            if !self.graph.crossable[i] {
                out.extend([i - 2, i - 1]);
            }
            let base = i - Slot::Upper as usize;
            if let Some(climbing) = self.graph.climbs[base]
                && let [climber] = self.holders[base + climbing as usize][..]
                && climbed(self.graph, &self.holders, base, climber) > MAX_CLIMBED
            {
                out.extend([base + Slot::LowerX as usize, base + Slot::LowerZ as usize]);
            }
        }
        out.into_iter().collect()
    }

    /// Rounds of rip-up and re-route until no slot is overused. Each round
    /// re-routes the nets that hold an overused slot (and the dirty ones); the
    /// first round routes everything.
    fn run(&mut self, log: &mut dyn FnMut(&str)) -> Result<(Vec<Tree>, usize), Stuck> {
        let graph = self.graph;
        let mut best = usize::MAX;
        let mut stalled = 0;
        let lookup = |node: Node| -> Result<usize, Stuck> {
            graph.index.get(&node).copied().ok_or_else(|| Stuck::Unroutable(format!("no routing tile at {:?}", node.0)))
        };
        for round in 0..MAX_ITERATIONS {
            let overused: BTreeSet<usize> = self.overused().into_iter().collect();
            let mut to_route: Vec<usize> = std::mem::take(&mut self.dirty);
            to_route.extend((0..self.nets.len()).filter(|&k| self.held[k].iter().any(|slot| overused.contains(slot))));
            to_route.sort_unstable();
            to_route.dedup();
            for k in to_route {
                let (_, source, sinks) = &self.nets[k];
                for &slot in &self.held[k] {
                    self.holders[slot].retain(|&n| n != k);
                }
                let start = lookup(source.node)?;
                let mut targets: Vec<usize> = sinks.iter().map(|s| lookup(s.node)).collect::<Result<_, _>>()?;
                // The pins are on the lower level, so the net stays off the upper
                // level of its own sinks' tiles, or a branch passing over one would
                // leave no way down into it.
                let sink_cells: BTreeSet<Cell> = sinks.iter().map(|s| s.node.0).collect();
                // Nearest sinks first.
                let dist = |a: usize, b: usize| {
                    let (ca, cb) = (graph.nodes[a].0, graph.nodes[b].0);
                    (ca.0 - cb.0).abs() + (ca.1 - cb.1).abs()
                };
                targets.sort_by_key(|&t| dist(start, t));
                let (holders, history, penalties, present_factor, seed) =
                    (&self.holders, &self.history, &self.penalties[k], self.present_factor, self.seed);
                let cost = |node: usize| {
                    let (cell, track) = graph.nodes[node];
                    if track.slots().contains(&Slot::Upper) && sink_cells.contains(&cell) {
                        return f64::INFINITY;
                    }
                    // This net's own slots are not among the holders now.
                    let slots = &graph.slots[node];
                    let base = graph.base[node];
                    let held = |slot: Slot| !holders[base + slot as usize].is_empty();
                    let mut overuse = slots.iter().map(|&s| holders[s].len()).max().unwrap_or(0);
                    // A crossing under an upper wire: taking the upper level above a
                    // crossing, or a lower track that makes one under an upper wire.
                    let crossable = graph.crossable[base];
                    let crossing_under = match track {
                        Track::UpperX | Track::UpperZ => crossed(holders, base),
                        Track::X => (held(Slot::Upper) || !crossable) && held(Slot::LowerZ),
                        Track::Z => (held(Slot::Upper) || !crossable) && held(Slot::LowerX),
                        Track::Ramp(_) => false,
                    };
                    if crossing_under {
                        overuse = overuse.max(1);
                    }
                    // The flat track across a net that climbs here (north-south in a
                    // narrow column, west-east in a shallow row) where that would make
                    // it climb more crossings in a row than it can.
                    let climbing = graph.climbs[base];
                    let flat = match climbing {
                        Some(Slot::LowerZ) => Some(Track::X),
                        Some(Slot::LowerX) => Some(Track::Z),
                        _ => None,
                    };
                    if flat == Some(track)
                        && let Some(climbing) = climbing
                        && let [climber] = holders[base + climbing as usize][..]
                        && climbed(graph, holders, base, climber) > MAX_CLIMBED
                    {
                        overuse = overuse.max(1);
                    }
                    let past: f64 = slots.iter().map(|&s| history[s]).sum();
                    let penalty = penalties.get(&node).copied().unwrap_or(0.0);
                    // A ramp holds its whole tile, and costs a tile more; a crossing
                    // that cannot turn, whose climbing net can take no repeater there,
                    // costs more again, so that runs across many take the upper level.
                    let fixed_crossing = climbing.is_some()
                        && match track {
                            Track::X => held(Slot::LowerZ),
                            Track::Z => held(Slot::LowerX),
                            _ => false,
                        };
                    let cost = if matches!(track, Track::Ramp(_)) {
                        2.0
                    } else if fixed_crossing {
                        FIXED_CROSSING_COST
                    } else {
                        1.0
                    };
                    (cost + past + penalty + jitter(seed, k, node)) * (1.0 + present_factor * overuse as f64)
                };
                let (tree, parent) = grow_tree(graph, start, targets, cost, &mut self.search).map_err(|target| {
                    Stuck::Unroutable(format!("net cannot reach the tile at {:?}", graph.nodes[target].0))
                })?;
                let held: BTreeSet<usize> = tree.iter().flat_map(|&node| graph.slots[node].iter().copied()).collect();
                for &slot in &held {
                    self.holders[slot].push(k);
                }
                self.trees[k] = tree;
                self.parents[k] = parent;
                self.held[k] = held;
            }
            self.rounds += 1;
            let overused = self.overused();
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
                self.history[i] += self.holders[i].len().saturating_sub(1).max(1) as f64;
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

/// How many crossings in a row net `climber` climbs through the tile whose
/// first slot is `base` (one whose crossing cannot turn), counting that tile as
/// one: it and the tiles before and after it along the climbing track (up and
/// down its column for a north-south climber, along its row for a west-east
/// one) where `climber` climbs and another net holds the flat track.
fn climbed(graph: &Graph, holders: &[Vec<usize>], base: usize, climber: usize) -> usize {
    let Some(climbing) = graph.climbs[base] else { return 0 };
    let flat = if climbing == Slot::LowerZ { Slot::LowerX } else { Slot::LowerZ };
    let (col, row) = graph.slot_of[base].0;
    let climbed_at = |cell: Cell| {
        graph.index.get(&(cell, Track::X)).is_some_and(|&node| {
            let b = graph.base[node];
            let other = &holders[b + flat as usize];
            graph.climbs[b] == Some(climbing)
                && holders[b + climbing as usize] == [climber]
                && !other.is_empty()
                && !other.contains(&climber)
        })
    };
    let mut run = 1;
    for step in [-1, 1] {
        let next = |n: i32| if climbing == Slot::LowerZ { (col, row + step * n) } else { (col + step * n, row) };
        let mut n = 1;
        while climbed_at(next(n)) {
            run += 1;
            n += 1;
        }
    }
    run
}

/// A tiny cost that differs per net and node, so that nets with several
/// equally short paths (every staircase between two rows is one) do not all
/// pick the same one and then have to be pried apart. Deterministic, so the
/// same netlist always routes the same way.
/// A tiny deterministic cost that breaks ties between equal paths, hashed from
/// the net, the node and the routing seed (seed 0 adds nothing to the hash, so
/// it is the router as it was before seeds).
fn jitter(seed: u64, net: usize, node: usize) -> f64 {
    let mut h = (net as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (node as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ seed.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    h ^= h >> 31;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    (h >> 40) as f64 / (1u64 << 24) as f64 * 1e-3
}

/// A net's tree as it grows: its nodes, and each node with the node it was reached from.
type Grown = (Vec<usize>, BTreeMap<usize, Option<usize>>);

/// A net's tree: from `start` to each of `targets` in turn along the cheapest
/// path by `cost`, every node with the node it was reached from. A branch can
/// wall in a later target (the net never enters a tile it holds, and an upper
/// wire has no way down in its own tile); then the tree starts again with that
/// target first, as many times as there are targets. Fails with a target no
/// path reaches.
fn grow_tree(
    graph: &Graph,
    start: usize,
    mut targets: Vec<usize>,
    cost: impl Fn(usize) -> f64,
    search: &mut Search,
) -> Result<Grown, usize> {
    let mut restarts = 0;
    'grow: loop {
        let mut tree = vec![start];
        let mut parent: BTreeMap<usize, Option<usize>> = BTreeMap::from([(start, None)]);
        for i in 0..targets.len() {
            let target = targets[i];
            if tree.contains(&target) {
                continue;
            }
            let Some(path) = shortest_path(graph, &tree, target, &cost, search) else {
                if i == 0 || restarts == targets.len() {
                    return Err(target);
                }
                restarts += 1;
                targets[..=i].rotate_right(1);
                continue 'grow;
            };
            for window in path.windows(2) {
                if let std::collections::btree_map::Entry::Vacant(slot) = parent.entry(window[1]) {
                    slot.insert(Some(window[0]));
                    tree.push(window[1]);
                }
            }
        }
        return Ok((tree, parent));
    }
}

/// The working arrays of the A* searches, kept from one search to the next: a
/// search clears only what the one before it touched, so it costs what it
/// explores rather than the size of the graph.
struct Search {
    best: Vec<f64>,
    from: Vec<Option<usize>>,
    touched: Vec<usize>,
    /// Per slot, the search whose tree holds that slot's tile (by its first slot).
    held: Vec<u32>,
    generation: u32,
}

impl Search {
    fn new(graph: &Graph) -> Search {
        let n = graph.nodes.len();
        Search {
            best: vec![f64::INFINITY; n],
            from: vec![None; n],
            touched: Vec::new(),
            held: vec![0; graph.slot_of.len()],
            generation: 0,
        }
    }

    /// Ready for a new search: the last one's entries cleared, a new generation.
    fn start(&mut self) -> u32 {
        for &i in &self.touched {
            self.best[i] = f64::INFINITY;
            self.from[i] = None;
        }
        self.touched.clear();
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.held.fill(0);
            self.generation = 1;
        }
        self.generation
    }
}

/// A* from every node of `tree` (cost 0) to `target`, with the distance in
/// tiles as the estimate, weighed by `ASTAR_WEIGHT`. Returns the path
/// from the tree to the target, starting with a tree node.
///
/// A net may use both tracks of a level only as a turn or fan-out, where the
/// two are joined inside the tile, and never two levels of one tile but by a
/// ramp. So a path may not enter a tile from outside if the net already holds
/// any of that tile's nodes.
fn shortest_path(
    graph: &Graph,
    tree: &[usize],
    target: usize,
    cost: impl Fn(usize) -> f64,
    search: &mut Search,
) -> Option<Vec<usize>> {
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
    let goal = graph.nodes[target].0;
    let estimate = |node: usize| {
        let cell = graph.nodes[node].0;
        ASTAR_WEIGHT * f64::from((cell.0 - goal.0).abs() + (cell.1 - goal.1).abs())
    };
    let generation = search.start();
    for &i in tree {
        search.held[graph.base[i]] = generation;
    }
    let Search { best, from, touched, held, .. } = search;
    let mut heap = BinaryHeap::new();
    for &node in tree {
        best[node] = 0.0;
        touched.push(node);
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
            let cell = graph.nodes[next].0;
            if graph.nodes[node].0 != cell && held[graph.base[next]] == generation {
                continue;
            }
            // Nor straight back into the tile it came from: a path that ramps up
            // and turns round would hold two places in one tile.
            if from[node].is_some_and(|p| graph.nodes[p].0 == cell && graph.nodes[node].0 != cell) {
                continue;
            }
            let nd = d + cost(next);
            if nd < best[next] {
                if best[next] == f64::INFINITY {
                    touched.push(next);
                }
                best[next] = nd;
                from[next] = Some(node);
                heap.push(Item(nd + estimate(next), next));
            }
        }
    }
    None
}

/// Walks the net from its source, converting wire tiles into repeaters where
/// the signal would arrive too weak. Returns the repeater count on the path to
/// each sink.
///
/// The walk places each repeater at the latest tile before the signal dies, so
/// every path from the source has as few repeaters as it can have on its own;
/// but below a fork that puts one in each branch where one before the fork
/// might do for both. So the tree is then solved exactly: the fewest
/// repeaters such that no sink has more on its path than the walk gave it (no
/// path gets slower), and the walk's choice is kept unless that is fewer.
fn insert_repeaters(
    library: &Library,
    layout: &mut Layout,
    cells: &NetTiles,
    source: &Source,
    sinks: &[Sink],
    tree: &Tree,
) -> Result<Vec<u32>, Dies> {
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
    // A tile of this net's alone can carry a repeater, and so can a crossing
    // whose flat arm no other net has taken for one; a ramp has no room for one.
    let crossing = |cell: Cell| cells[&cell].3;
    // The cell's size in blocks, west-east and north-south.
    let size = |cell: Cell| {
        let width = layout.col_width.get(cell.0 as usize).copied().unwrap_or(PITCH);
        (width, layout.row_depth.get(cell.1 as usize).copied().unwrap_or(PITCH))
    };
    let can_repeat = |cell: Cell| match cells[&cell].2 {
        At::Ramp(_) => false,
        _ if crossing(cell) => {
            // The repeater goes in the flat arm, which a full cell turns to give
            // either net; in a narrow column it is the west-east one, in a shallow
            // row the north-south one.
            let north_south = cross_turn_for(cells[&cell].1) == 1;
            let flat = match size(cell) {
                (width, _) if width < PITCH => !north_south,
                (_, depth) if depth < PITCH => north_south,
                _ => true,
            };
            matches!(layout.tiles.get(&cell), Some(Placed::Cross { repeater_entry: None, .. })) && flat
        }
        // Where an arm is the centre, a straight or a turn only.
        _ => {
            let (edges, entry, _, _) = &cells[&cell];
            let (width, depth) = size(cell);
            crate::layout::can_carry_repeater(width, depth, &edges.iter().copied().collect::<Vec<_>>(), *entry)
        }
    };
    let repeater_tile = |cell: Cell| -> Placed {
        let (edges, entry, _, crossed) = &cells[&cell];
        if *crossed {
            // The repeater goes in the flat west-east arm: turn the tile when this
            // net runs north-south, and name the entry before the turn.
            let rotation = cross_turn_for(*entry);
            Placed::Cross { rotation, repeater_entry: Some(unturned(*entry, rotation)), delay: 1 }
        } else {
            Placed::Repeater { edges: edges.iter().copied().collect(), entry: *entry, delay: 1 }
        }
    };
    // The tile at a cell of this net's as the file describes it, and how it is
    // turned: a crossing is turned a quarter when the repeater it carries, or is
    // about to carry, belongs to a net running north-south. Repeaters of this
    // net are placed after the walk, so the one it is about to carry is read
    // from `at_repeater`.
    let tile_at = |cell: Cell, at_repeater: bool| -> (&Tile, u8) {
        let (width, depth) = size(cell);
        let full = width >= PITCH && depth >= PITCH;
        let sized = |kind: Kind| {
            library.get(
                kind.sized(width, depth).expect("wiring comes in every size, and no crossing is left three by three"),
            )
        };
        match cells[&cell].2 {
            // Only a full cell's ramp turns a quarter: the others have a file for
            // each axis and turn half round.
            At::Ramp(lower) if !full => {
                let along_x = matches!(lower, Edge::West | Edge::East);
                let half = if matches!(lower, Edge::West | Edge::North) { 0 } else { 2 };
                (library.get(Kind::ramp_sized(width, depth, along_x)), half)
            }
            At::Ramp(lower) => (&library.ramp, crate::layout::ramp_turn(lower)),
            At::Level(Level::Upper) => (sized(Kind::WireUp), 0),
            At::Level(Level::Lower) if crossing(cell) && !full => (sized(Kind::Cross), 0),
            At::Level(Level::Lower) if crossing(cell) => match (at_repeater, layout.tiles.get(&cell)) {
                (true, Some(Placed::Cross { repeater_entry: None, .. })) => {
                    (&library.cross, cross_turn_for(cells[&cell].1))
                }
                (_, Some(Placed::Cross { rotation, .. })) => (&library.cross, *rotation),
                _ => (&library.cross, 0),
            },
            At::Level(Level::Lower) => (sized(Kind::Wire), 0),
        }
    };
    let parent_cell = |cell: Cell| -> Option<Cell> {
        let node = tree.parent.keys().find(|n| n.0 == cell && tree.parent[n].is_none_or(|p| p.0 != cell))?;
        tree.parent[node].map(|p| p.0)
    };
    // What a hop through `cell` costs from its entry to the port beyond `exit`,
    // and what a repeater in its entry arm hands on there.
    let steps = |cell: Cell, exit: Edge| -> u8 {
        let (tile, rotation) = tile_at(cell, false);
        tile.dust_steps(unturned(cells[&cell].1, rotation), unturned(exit, rotation))
            .expect("the tile's dust joins the entry to every edge the net leaves by")
    };
    let repeater_out = |cell: Cell, exit: Edge| -> u8 {
        let (tile, rotation) = tile_at(cell, true);
        tile.repeater_out(unturned(cells[&cell].1, rotation), unturned(exit, rotation))
            .expect("a repeater the tile can carry reaches every exit")
    };
    // What the entry port must carry for a repeater here to switch: any power
    // where it reads the port's own dust, more where it stands further in (a
    // narrow turn's, in the arm it leaves by).
    let repeater_needs = |cell: Cell, exits: &mut dyn Iterator<Item = Edge>| -> u8 {
        let (tile, rotation) = tile_at(cell, true);
        let entry = unturned(cells[&cell].1, rotation);
        exits
            .map(|exit| {
                tile.repeater_needs(entry, unturned(exit, rotation))
                    .expect("a repeater the tile can carry reaches every exit")
            })
            .fold(1, u8::max)
    };
    let root = source.node.0;

    // The strength at each cell's entry, in DFS order, with `repeaters` placed:
    // the repeater count on the path to each sink, or the first cell where the
    // signal arrives too weak.
    let walk = |repeaters: &BTreeSet<Cell>| -> Result<Vec<u32>, Cell> {
        let mut stack = vec![(root, source.strength)];
        let mut sink_counts = vec![0u32; sinks.len()];
        while let Some((cell, v)) = stack.pop() {
            let at_repeater = repeaters.contains(&cell);
            let out = |exit: Edge| {
                if at_repeater { repeater_out(cell, exit) } else { v.saturating_sub(steps(cell, exit)) }
            };
            let children = child_cells(cell);
            let mut needed = if at_repeater {
                let exits = children.iter().map(|&c| edge_between(cell, c));
                let sinks_here = sinks.iter().filter(|s| s.node.0 == cell).map(|s| s.exit);
                repeater_needs(cell, &mut exits.chain(sinks_here))
            } else {
                1
            };
            if !at_repeater {
                // Every child's port must still see one level.
                for child in &children {
                    needed = needed.max(1 + steps(cell, edge_between(cell, *child)));
                }
            }
            for (i, sink) in sinks.iter().enumerate() {
                if sink.node.0 == cell {
                    if !at_repeater {
                        // At a repeater the pin reads the repeater's own output dust,
                        // which is always strong enough.
                        needed = needed.max(sink.needed_after + steps(cell, sink.exit));
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
                return Err(cell);
            }
            for child in children {
                stack.push((child, out(edge_between(cell, child))));
            }
        }
        Ok(sink_counts)
    };

    // The exact solution over the tree. For each cell, the strength at its entry
    // and the repeaters already on the path above it: the least cost of the
    // subtree (a repeater costs `REPEATER_COST`), when no
    // sink may have more repeaters on its path than `budget` gives it. Every leaf
    // of the tree is a sink, so a path with more repeaters than the largest
    // budget can be cut short.
    let fewest_repeaters = |budget: &[u32]| -> Option<(BTreeSet<Cell>, u32)> {
        let most = budget.iter().copied().max().unwrap_or(0) as usize;
        let width = most + 1;
        // Cells, every one after its parent.
        let mut order = vec![root];
        let mut i = 0;
        while i < order.len() {
            order.extend(child_cells(order[i]));
            i += 1;
        }
        struct Hop {
            children: Vec<Cell>,
            /// Per child: the dust steps to its port without a repeater, and
            /// what a repeater in this cell's entry arm hands on there.
            plain: Vec<u8>,
            repeated: Vec<u8>,
            /// Least strength at the entry without a repeater.
            needed: u8,
            /// The sinks here.
            sinks: Vec<usize>,
            /// The cost of a repeater here, if one can go here, and what the
            /// entry must carry for it.
            repeater: Option<u32>,
            repeater_needs: u8,
        }
        let mut hops: BTreeMap<Cell, Hop> = BTreeMap::new();
        for &cell in &order {
            let children = child_cells(cell);
            let plain: Vec<u8> = children.iter().map(|&c| steps(cell, edge_between(cell, c))).collect();
            let here: Vec<usize> = (0..sinks.len()).filter(|&i| sinks[i].node.0 == cell).collect();
            let needed = plain
                .iter()
                .map(|s| 1 + s)
                .chain(here.iter().map(|&i| sinks[i].needed_after + steps(cell, sinks[i].exit)))
                .fold(1, u8::max);
            let repeater = can_repeat(cell).then(|| REPEATER_COST + u32::from(crossing(cell)));
            let (repeated, repeater_needs) = if repeater.is_some() {
                let exits = children.iter().map(|&c| edge_between(cell, c));
                let needs = repeater_needs(cell, &mut exits.clone().chain(here.iter().map(|&i| sinks[i].exit)));
                (exits.map(|exit| repeater_out(cell, exit)).collect(), needs)
            } else {
                (Vec::new(), 1)
            };
            hops.insert(cell, Hop { children, plain, repeated, needed, sinks: here, repeater, repeater_needs });
        }
        const INFEASIBLE: u32 = u32::MAX;
        let at = |v: u8, r: usize| usize::from(v) * width + r;
        let mut best: BTreeMap<Cell, Vec<u32>> = BTreeMap::new();
        // The cost of each choice at a cell: without a repeater, and with one.
        let choices = |hop: &Hop, best: &BTreeMap<Cell, Vec<u32>>, v: u8, r: usize| -> (u32, u32) {
            let sum = |strengths: &[u8], r: usize, base: u32| {
                hop.children.iter().zip(strengths).try_fold(base, |total, (child, &out)| {
                    let cost = best[child][at(out, r)];
                    (cost != INFEASIBLE).then(|| total.saturating_add(cost))
                })
            };
            let within = |r: usize| hop.sinks.iter().all(|&i| r <= budget[i] as usize);
            let plain = if v >= hop.needed && within(r) {
                let after: Vec<u8> = hop.plain.iter().map(|s| v - s).collect();
                sum(&after, r, 0).unwrap_or(INFEASIBLE)
            } else {
                INFEASIBLE
            };
            let repeated = match hop.repeater {
                Some(cost) if v >= hop.repeater_needs && r < most && within(r + 1) => {
                    sum(&hop.repeated, r + 1, cost).unwrap_or(INFEASIBLE)
                }
                _ => INFEASIBLE,
            };
            (plain, repeated)
        };
        for &cell in order.iter().rev() {
            let hop = &hops[&cell];
            let mut table = vec![INFEASIBLE; 16 * width];
            for v in 0..16u8 {
                for r in 0..width {
                    let (plain, repeated) = choices(hop, &best, v, r);
                    table[at(v, r)] = plain.min(repeated);
                }
            }
            best.insert(cell, table);
        }
        let total = best[&root][at(source.strength.min(15), 0)];
        if total == INFEASIBLE {
            return None;
        }
        // Down from the root, without a repeater wherever that costs no more.
        let mut chosen = BTreeSet::new();
        let mut stack = vec![(root, source.strength.min(15), 0usize)];
        while let Some((cell, v, r)) = stack.pop() {
            let hop = &hops[&cell];
            let (plain, repeated) = choices(hop, &best, v, r);
            if plain <= repeated {
                for (&child, s) in hop.children.iter().zip(&hop.plain) {
                    stack.push((child, v - s, r));
                }
            } else {
                chosen.insert(cell);
                for (&child, &out) in hop.children.iter().zip(&hop.repeated) {
                    stack.push((child, out, r + 1));
                }
            }
        }
        Some((chosen, total))
    };

    let mut repeaters: BTreeSet<Cell> = BTreeSet::new();
    let counts = loop {
        let bad = match walk(&repeaters) {
            Ok(counts) => break counts,
            Err(bad) => bad,
        };
        // Place a repeater at the latest tile on the way here that can take one and
        // is not one already, including this tile itself; but a tile of this net's
        // own before a crossing, so that the crossing's one slot stays free for the
        // net that has no other choice there.
        let mut candidate = Some(bad);
        let mut at_crossing: Option<Cell> = None;
        let mut chosen: Option<Cell> = None;
        while let Some(cell) = candidate {
            if repeaters.contains(&cell) {
                break;
            }
            if can_repeat(cell) {
                if !crossing(cell) {
                    chosen = Some(cell);
                    break;
                }
                at_crossing.get_or_insert(cell);
            }
            candidate = parent_cell(cell);
        }
        let candidate = chosen.or(at_crossing).or_else(|| {
            // Nothing since the last repeater: anywhere earlier that can take one.
            let mut candidate = Some(bad);
            while let Some(cell) = candidate {
                if can_repeat(cell) && !repeaters.contains(&cell) {
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
                return Err(Dies { stuck: path.into_iter().filter(|&c| !can_repeat(c)).collect() });
            }
        }
    };

    // The exact solution, kept when it costs less than the walk's (fewer
    // repeaters, or as many in fewer crossings) and the walk confirms it.
    let walked = repeaters.iter().map(|&c| REPEATER_COST + u32::from(crossing(c))).sum::<u32>();
    let (repeaters, counts) = match fewest_repeaters(&counts) {
        Some((fewer, cost)) if cost < walked => match walk(&fewer) {
            Ok(fewer_counts) => (fewer, fewer_counts),
            Err(_) => (repeaters, counts),
        },
        _ => (repeaters, counts),
    };
    let placed: Vec<(Cell, Placed)> = repeaters.iter().map(|&cell| (cell, repeater_tile(cell))).collect();
    for (cell, placed) in placed {
        match cells[&cell].2 {
            At::Level(Level::Upper) => layout.upper.insert(cell, placed),
            _ => layout.tiles.insert(cell, placed),
        };
    }
    Ok(counts)
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

    /// A net never enters a tile it already holds, and an upper wire has no way
    /// down in its own tile, so an upper branch to the nearer sink can wall in a
    /// farther one: here the cheap way from (0, 1) to the sink at (0, 3) climbs
    /// at (1, 1), runs round on the upper level through (2, 1) and (2, 3), and
    /// comes down at (1, 3); the gates at (1, 2) and (3, 2) close the other
    /// sides of the sink at (2, 2), whose upper level its own net may not take.
    /// The net then routes again with the walled-in sink first, and reaches
    /// both; a sink that no path reaches even first is still named.
    #[test]
    fn a_branch_that_walls_in_a_later_sink_is_routed_again_with_that_sink_first() {
        let placement = Placement {
            cols: 5,
            rows: 5,
            col_width: vec![PITCH; 5],
            row_depth: vec![PITCH; 5],
            gates: vec![(1, 2), (3, 2)],
            widths: vec![1, 1],
            pin_edges: vec![vec![], vec![]],
            inputs: vec![],
            outputs: vec![],
            ties: vec![],
            routing: vec![5],
            columns: (0..5).map(|offset| Column::Routing { gap: 0, offset }).collect(),
            stages: vec![1, 1],
        };
        let graph = Graph::new(&placement);
        let node = |cell: Cell, track: Track| graph.index[&(cell, track)];
        let ring: BTreeSet<Cell> = [(2, 1), (3, 1), (4, 1), (4, 2), (4, 3), (3, 3), (2, 3)].into_iter().collect();
        let cost = |n: usize| {
            let (cell, track) = graph.nodes[n];
            match track {
                _ if cell == (2, 2) => {
                    if track.slots().contains(&Slot::Upper) {
                        f64::INFINITY
                    } else {
                        1.0
                    }
                }
                Track::Ramp(Edge::West) if cell == (1, 1) || cell == (1, 3) => 2.0,
                Track::Ramp(_) => f64::INFINITY,
                Track::UpperX | Track::UpperZ if ring.contains(&cell) => 1.0,
                Track::UpperX | Track::UpperZ => f64::INFINITY,
                _ if cell == (0, 1) || cell == (0, 3) => 1.0,
                _ => 100.0,
            }
        };
        let (start, near, walled) = (node((0, 1), Track::X), node((0, 3), Track::X), node((2, 2), Track::X));
        let mut search = Search::new(&graph);
        let (tree, parent) =
            grow_tree(&graph, start, vec![near, walled], cost, &mut search).expect("both sinks reached");
        for sink in [near, walled] {
            assert!(tree.contains(&sink), "{:?} is in the tree", graph.nodes[sink]);
            let mut at = sink;
            while let Some(Some(up)) = parent.get(&at) {
                at = *up;
            }
            assert_eq!(at, start, "{:?} leads back to the source", graph.nodes[sink]);
        }
        let shut = node((4, 4), Track::X);
        let walls =
            |n: usize| if graph.nodes[n].0 == (4, 3) || graph.nodes[n].0 == (3, 4) { f64::INFINITY } else { 1.0 };
        assert_eq!(grow_tree(&graph, start, vec![near, shut], walls, &mut search), Err(shut));
    }

    /// A crossing's climbing net can take no repeater in a narrow column, and
    /// loses five levels in each, so a net that climbs three in a row has nowhere
    /// to be repeated (twelve after a repeater, seven, two, and the third needs
    /// six). Such a run counts as contended, both tracks of each of its tiles, so
    /// that the negotiation moves a net off it; two in a row do not, nor three
    /// with a gap, nor three in a column five blocks wide, whose crossings turn
    /// to give the climbing net the flat arm.
    #[test]
    fn three_narrow_crossings_climbed_in_a_row_by_one_net_are_contended() {
        let placement = |width: i32| Placement {
            cols: 3,
            rows: 6,
            col_width: vec![width; 3],
            row_depth: vec![PITCH; 6],
            gates: vec![],
            widths: vec![],
            pin_edges: vec![],
            inputs: vec![],
            outputs: vec![],
            ties: vec![],
            routing: vec![3],
            columns: (0..3).map(|offset| Column::Routing { gap: 0, offset }).collect(),
            stages: vec![],
        };
        let nets: Vec<(NetId, Source, Vec<Sink>)> = (0..5)
            .map(|k| (k, Source { node: ((0, k as i32), Track::X), entry: Edge::West, strength: 15 }, vec![]))
            .collect();
        let narrow = crate::tile::NARROW;
        for (width, crossed, contended) in [
            (narrow, vec![1, 2, 3], true),
            (narrow, vec![2, 3, 4], true),
            (narrow, vec![1, 2], false),
            (narrow, vec![1, 2, 4], false),
            (PITCH, vec![1, 2, 3], false),
        ] {
            let placement = placement(width);
            let graph = Graph::new(&placement);
            let mut negotiation = Negotiation::new(&graph, &nets, vec![0.0; graph.slot_of.len()], 0);
            let base = |row: i32| graph.base[graph.index[&((1, row), Track::X)]];
            // Net 0 runs down column 1 from row 0 to row 5; nets 1 on cross it in the rows given.
            for row in 0..6 {
                negotiation.holders[base(row) + Slot::LowerZ as usize].push(0);
            }
            for (k, &row) in crossed.iter().enumerate() {
                negotiation.holders[base(row) + Slot::LowerX as usize].push(k + 1);
            }
            let overused: BTreeSet<usize> = negotiation.overused().into_iter().collect();
            let expected: BTreeSet<usize> = if contended {
                crossed
                    .iter()
                    .flat_map(|&row| [base(row) + Slot::LowerX as usize, base(row) + Slot::LowerZ as usize])
                    .collect()
            } else {
                BTreeSet::new()
            };
            assert_eq!(overused, expected, "width {width}, crossings in rows {crossed:?}");
        }
    }

    /// How many crossings a net may climb in a row where they cannot turn
    /// follows from the tiles: what a repeater hands on in a straight, or in a
    /// turn into the climbing axis, less the one the tile after the run needs,
    /// in crossings; down a narrow column and along a shallow row alike.
    #[test]
    fn the_crossings_a_net_may_climb_follow_from_the_narrow_and_shallow_tiles() {
        let lib = Library::builtin().unwrap();
        use Edge::{East, North, South, West};
        for (wire, cross, along, turn_in) in [
            (&lib.wire_n, &lib.cross_n, (North, South), (West, North)),
            (&lib.wire_s, &lib.cross_s, (West, East), (North, West)),
        ] {
            let crossing = cross.dust_steps(along.0, along.1).unwrap();
            let needed_after = wire.repeater_needs(along.0, along.1).unwrap();
            for after in
                [wire.repeater_out(along.0, along.1).unwrap(), wire.repeater_out(turn_in.0, turn_in.1).unwrap()]
            {
                let climbable = usize::from((after - needed_after) / crossing);
                assert_eq!(climbable, MAX_CLIMBED, "{} and {}: {after} after a repeater", wire.name, cross.name);
            }
        }
    }

    #[test]
    fn a_gap_too_narrow_is_given_up_after_stalling_and_named() {
        let netlist = braid(5);
        let gaps = gap_count(&netlist, Style::Pitch4).unwrap();
        // Rows a cell deep, as the braid was measured with: in shallow rows no
        // crossing fits a routing column, and every gap is too narrow.
        let mut narrow = place(&netlist, &vec![1; gaps], Style::Pitch4).unwrap();
        narrow.row_depth = vec![PITCH; narrow.rows as usize];
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
        // Every contended track names the nets that want it.
        assert!(!tracks.is_empty());
        assert_eq!(tracks.len(), users.len());
        assert!(users.iter().all(|u| u.len() >= 2), "{users:?}");
        let mut per_gap = vec![0usize; gaps];
        // The free tiles of the outputs' column are routing room too, in no gap,
        // which the widening passes over.
        for (cell, _) in &tracks {
            if let Some(gap) = narrow.gap_of(cell.0) {
                per_gap[gap] += 1;
            }
        }
        // The narrow gap holds a fair share of them (a quarter of the most or
        // more), so the build widens it; the nets it turns away crowd the next.
        let most = *per_gap.iter().max().unwrap();
        assert!(per_gap[0] > 0 && per_gap[0] * 4 >= most, "{per_gap:?}");
        // The history carries what was learned, named by column so that it
        // survives the widening, and only where there was contention.
        assert!(history.costs.values().all(|&c| c > 0.0));
        assert!(history.costs.keys().any(|(column, _, _)| matches!(column, Column::Routing { gap: 0, .. })));
        // Widened as the build widens it (from the estimate, a gap at a time,
        // each attempt seeded with what the last learned), the placement routes.
        let (built, widths) = crate::build::place_and_route(&library, &netlist, Style::Pitch4).unwrap();
        assert!(widths.iter().zip(&estimate).all(|(w, e)| w >= e), "{widths:?} against {estimate:?}");
        assert!(built.rounds > 0);
    }

    /// The negotiation breaks ties between equal-cost paths with a hash of the
    /// net and the node (`jitter`); seeding that hash gives another trajectory
    /// through the same placement, and so, often, another layout of the same
    /// width, which is as valid as the first (the build routes every candidate
    /// of a small design with two seeds and keeps the smaller). Seed 0 is the
    /// unseeded router, byte for byte.
    #[test]
    fn another_seed_routes_another_valid_layout_sometimes() {
        let library = Library::builtin().unwrap();
        let (mut routed_too, mut differs) = (0, 0);
        for seed in 400..412u64 {
            let mut rng = crate::tiletest::Rng::new(seed);
            let gates = 4 + rng.below(8);
            let netlist = crate::pnrtest::random_netlist(&mut rng, gates);
            let (_, routing) = crate::build::place_and_route(&library, &netlist, Style::Packed).unwrap();
            let placement = place(&netlist, &routing, Style::Packed).unwrap();
            let mut log = |_: &str| {};
            // Cold on the final widths (the build's last attempt carried a history):
            // seed 0 is the plain router, byte for byte.
            let plain = route(&library, &netlist, &placement, None, &mut log);
            let again = route_with_seed(&library, &netlist, &placement, None, 0, &mut log);
            let first = match (plain, again) {
                (Ok(plain), Ok(again)) => {
                    assert_eq!(again.layout, plain.layout, "seed {seed}: routing seed 0 is the plain router");
                    plain
                }
                (Err(_), Err(_)) => continue,
                (plain, again) => {
                    panic!("seed {seed}: seed 0 routed {} and the plain router {}", again.is_ok(), plain.is_ok())
                }
            };
            // The second seed may need other widths; where it routes, it routes a valid layout.
            if let Ok(other) = route_with_seed(&library, &netlist, &placement, None, 1, &mut log) {
                other.layout.realize(&library, &netlist).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
                routed_too += 1;
                if other.layout != first.layout {
                    differs += 1;
                }
            }
        }
        assert!(routed_too >= 4, "the second seed routed only {routed_too} of 12 placements");
        assert!(differs >= 1, "the second seed never laid a net differently ({routed_too} routed)");
    }

    #[test]
    fn a_seeded_history_steers_nets_off_the_costly_tracks() {
        let netlist = braid(5);
        let gaps = gap_count(&netlist, Style::Pitch4).unwrap();
        let narrow = place(&netlist, &vec![1; gaps], Style::Pitch4).unwrap();
        // Rows a cell deep, as the braid was measured with (in shallow rows the
        // other columns of the gap, where no crossing fits, cannot take it all).
        let mut wide = place(&netlist, &estimate_routing(&netlist, &narrow), Style::Pitch4).unwrap();
        wide.row_depth = vec![PITCH; wide.rows as usize];
        let first = first_routing_column(&wide);
        let library = Library::builtin().unwrap();
        let mut log = |_: &str| {};
        let cold = route(&library, &netlist, &wide, None, &mut log).unwrap();
        assert!(runs_north_south_in(&cold.layout, first), "cold:\n{}", render(&cold.layout));
        // A history that makes that column prohibitive on both levels moves every
        // row change to the other columns.
        let mut steer = History::default();
        for row in 0..wide.rows {
            for slot in [Slot::LowerX, Slot::LowerZ, Slot::Upper] {
                steer.costs.insert((Column::Routing { gap: 0, offset: 0 }, row, slot), 1e6);
            }
        }
        let steered = route(&library, &netlist, &wide, Some(&steer), &mut log).unwrap();
        assert!(!runs_north_south_in(&steered.layout, first), "steered:\n{}", render(&steered.layout));
    }

    /// A net as a list of tiles, each with the tile it is reached from (`None`
    /// for the first) and the edges it uses, laid out as plain wire: the tiles,
    /// the layout and the tree `insert_repeaters` walks.
    fn wired(hops: &[(Cell, Option<Cell>, &[Edge])]) -> (NetTiles, Layout, Tree) {
        let mut tiles: NetTiles = BTreeMap::new();
        let mut layout = Layout::default();
        let mut parent: BTreeMap<Node, Option<Node>> = BTreeMap::new();
        for &(cell, from, edges) in hops {
            let entry = match from {
                Some(p) => edge_between(cell, p),
                None => Edge::West,
            };
            tiles.insert(cell, (edges.iter().copied().collect(), entry, At::Level(Level::Lower), false));
            layout.tiles.insert(cell, Placed::Wire { edges: edges.to_vec() });
            // The track the signal arrives on, and the other one when it turns or forks.
            let arrives = if matches!(entry, Edge::West | Edge::East) { Track::X } else { Track::Z };
            let parent_node = from.map(|p| {
                let leaves = if matches!(edge_between(p, cell), Edge::West | Edge::East) { Track::X } else { Track::Z };
                (p, leaves)
            });
            parent.insert((cell, arrives), parent_node);
            if edges.iter().any(|&e| e != entry && matches!(e, Edge::West | Edge::East) != (arrives == Track::X)) {
                let other = if arrives == Track::X { Track::Z } else { Track::X };
                parent.insert((cell, other), Some((cell, arrives)));
            }
        }
        (tiles, layout, Tree { parent })
    }

    /// A signal that would die in both branches just below a fork gets one
    /// repeater in the fork, which reaches both sinks, not one in each branch;
    /// and no sink gets more repeaters on its path than the walk alone gives it.
    #[test]
    fn a_fork_whose_branches_both_die_gets_one_repeater_before_it() {
        let library = Library::builtin().unwrap();
        let wire = &library.wire;
        let step = wire.dust_steps(Edge::West, Edge::East).unwrap();
        assert_eq!(step, 5, "the arithmetic below assumes five steps a tile");
        // West to east into a fork at (1, 1), which sends the net north and south;
        // each branch turns east and ends one tile on.
        use Edge::{East, North, South, West};
        let (tiles, mut layout, tree) = wired(&[
            ((0, 1), None, &[West, East]),
            ((1, 1), Some((0, 1)), &[West, North, South]),
            ((1, 0), Some((1, 1)), &[South, East]),
            ((2, 0), Some((1, 0)), &[West, East]),
            ((1, 2), Some((1, 1)), &[North, East]),
            ((2, 2), Some((1, 2)), &[West, East]),
        ]);
        // Fourteen at the start: nine at the fork, four at the turns, too weak to
        // reach the sinks a tile further on; a repeater in the fork reaches both.
        let source = Source { node: ((0, 1), Track::X), entry: West, strength: 14 };
        let sinks = vec![
            Sink { node: ((2, 0), Track::X), exit: East, needed_after: 1 },
            Sink { node: ((2, 2), Track::X), exit: East, needed_after: 1 },
        ];
        let counts = insert_repeaters(&library, &mut layout, &tiles, &source, &sinks, &tree).ok().unwrap();
        let repeaters: Vec<Cell> =
            layout.tiles.iter().filter(|(_, p)| matches!(p, Placed::Repeater { .. })).map(|(&c, _)| c).collect();
        assert_eq!(repeaters, vec![(1, 1)], "{:?}", layout.tiles);
        assert_eq!(counts, vec![1, 1]);
        // A single branch keeps its repeater as late as it can go.
        let (tiles, mut layout, tree) = wired(&[
            ((0, 1), None, &[West, East]),
            ((1, 1), Some((0, 1)), &[West, East]),
            ((2, 1), Some((1, 1)), &[West, East]),
            ((3, 1), Some((2, 1)), &[West, East]),
        ]);
        let sinks = vec![Sink { node: ((3, 1), Track::X), exit: East, needed_after: 1 }];
        let counts = insert_repeaters(&library, &mut layout, &tiles, &source, &sinks, &tree).ok().unwrap();
        let repeaters: Vec<Cell> =
            layout.tiles.iter().filter(|(_, p)| matches!(p, Placed::Repeater { .. })).map(|(&c, _)| c).collect();
        assert_eq!(repeaters, vec![(2, 1)]);
        assert_eq!(counts, vec![1]);
    }

    /// Two wire levels: where a tile can hold a crossing the router still uses
    /// one (cheaper than two ramps), so the braid needs no more routing columns
    /// than before and still matches its netlist; where no tile can (a routing
    /// column three blocks wide in a row three deep), nets cross as stacks, one
    /// on each level with ramps between, and every port still meets its match.
    #[test]
    fn crossing_nets_take_the_upper_level_where_no_tile_can_cross() {
        let netlist = braid(5);
        let library = Library::builtin().unwrap();
        let (routed, routing) = crate::build::place_and_route(&library, &netlist, Style::Pitch4).unwrap();
        // With crossing tiles alone the braid routed at 4 and 1 routing columns.
        assert!(routing.iter().sum::<i32>() <= 5, "{routing:?}");
        let built = crate::build::build_netlist(&netlist, "braid").unwrap();
        assert!(built.report.contains("layout matches the netlist"));
        let mut shallow = place(&netlist, &routing, Style::Pitch4).unwrap();
        shallow.row_depth = vec![3; shallow.rows as usize];
        let mut log = |_: &str| {};
        let stacked = route(&library, &netlist, &shallow, None, &mut log).unwrap();
        let layout = &stacked.layout;
        // No crossing fits a routing column three by three; the gate, source and
        // output columns, five wide, take the shallow crossing.
        let crossings_in_narrow = layout
            .tiles
            .iter()
            .any(|(&(col, _), p)| matches!(p, Placed::Cross { .. }) && layout.col_width[col as usize] < PITCH);
        assert!(!crossings_in_narrow, "{}", render(layout));
        assert!(!layout.upper.is_empty() && layout.tiles.values().any(|p| matches!(p, Placed::Ramp { .. })));
        layout.check_ports().unwrap();
        assert!(routed.layout.cells_used() > 0);
    }
}
