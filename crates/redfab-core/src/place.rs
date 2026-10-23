//! Placement: which tile each gate, input, output and constant occupies.
//!
//! Gates go into columns by logic depth, with routing columns between. Within a
//! column, gates are ordered by the average row of what feeds them (a few
//! barycenter sweeps), which keeps most wires short and uncrossed. The tile
//! above a gate feeds its north input, the tile below its south input, and its
//! west input comes straight in from the left. How many rows a gate takes is
//! the placement's [`Style`]: four at the four-row pitch (north feed, gate,
//! south feed, a free row for the nets that must pass the column), or, packed,
//! only the rows its pins use, with a free row after every few gates. Packed,
//! a pin pays for a feed row, so a gate reads on the west whenever that is
//! nearly as short, and two neighbouring gates that would each take the same
//! net on a feed side take it on their facing sides and share the feed tile
//! between them. The build tries the styles and keeps the layout that
//! measures best.
//!
//! Flip-flops form a column of their own, two cells wide, before the first
//! gates: their outputs are sources like the inputs, and their D inputs are
//! sinks fed back from wherever the logic ends.
//!
//! The columns form stages: the sources (inputs and constants) are stage 0,
//! the gate columns stages 1 and up in level order (the flip-flop column first
//! when there is one), and the outputs the last stage. Between every two
//! stages lies a gap of routing columns, and each gap has its own width, so
//! that a design with many inputs gets room in front of its first gates
//! without paying for it everywhere else.

use std::collections::BTreeMap;

use crate::netlist::{Driver, NetId, Netlist};
use crate::tile::{Edge, NARROW, PITCH, SHALLOW};

/// A tile position: (column, row). Columns run east, rows run south.
pub type Cell = (i32, i32);

/// How the gates of a column take their rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// Four rows per gate: north feed, gate, south feed, a free row.
    Pitch4,
    /// A row for each feed side a gate uses and one for the gate, with a free
    /// row after every few gates for the nets that pass the column.
    Packed,
    /// Packed, then every column shorter than the tallest is stretched over
    /// the tallest's rows, so that the rows it does not fill stay free for the
    /// nets that pass it (a clocked design's feedback and clock).
    Stretched,
}

impl Style {
    /// Every style, in the order the build tries them.
    pub const ALL: [Style; 3] = [Style::Pitch4, Style::Packed, Style::Stretched];

    /// The name a design file records.
    pub fn name(self) -> &'static str {
        match self {
            Style::Pitch4 => "pitch4",
            Style::Packed => "packed",
            Style::Stretched => "stretched",
        }
    }

    /// Whether gates take only the rows their pins use.
    pub fn packs(self) -> bool {
        self != Style::Pitch4
    }

    /// The style a design file names, if any.
    pub fn from_name(name: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// What a north or south side costs a packed gate on top of the wire to it: the
/// feed row it takes from the column. Measured the same for one, two and three.
const FEED_ROW_COST: f64 = 2.0;

/// What a column of the grid is, named so that a tile keeps its name when a
/// gap is widened: the new routing column goes at the east end of the gap.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Column {
    /// The inputs and constants.
    Sources,
    /// Routing column `offset` of gap `gap`.
    Routing {
        gap: usize,
        offset: i32,
    },
    /// Cell `offset` of gate column `index` (a flip-flop column is two wide).
    Gates {
        index: usize,
        offset: i32,
    },
    Outputs,
}

#[derive(Clone, Debug)]
pub struct Placement {
    pub cols: i32,
    pub rows: i32,
    /// Blocks per column and per row: `PITCH` for every one.
    pub col_width: Vec<i32>,
    pub row_depth: Vec<i32>,
    /// Per netlist cell: the cell of its first (west) tile.
    pub gates: Vec<Cell>,
    /// Per netlist cell: how many cells wide it is (two for a flip-flop).
    pub widths: Vec<i32>,
    /// Per netlist cell, per input pin: the edge the pin's net enters by.
    pub pin_edges: Vec<Vec<Edge>>,
    /// Per design input.
    pub inputs: Vec<Cell>,
    /// Per design output.
    pub outputs: Vec<Cell>,
    /// Constant nets that are used, with their value and tile.
    pub ties: Vec<(NetId, bool, Cell)>,
    /// Routing columns in each gap, west to east.
    pub routing: Vec<i32>,
    /// What each column is.
    pub columns: Vec<Column>,
    /// Per netlist cell: its stage (1 and up).
    pub stages: Vec<usize>,
}

impl Placement {
    pub fn is_free(&self, cell: Cell) -> bool {
        cell.0 >= 0
            && cell.0 < self.cols
            && cell.1 >= 0
            && cell.1 < self.rows
            && !self.gates.iter().zip(&self.widths).any(|(g, w)| g.1 == cell.1 && (g.0..g.0 + w).contains(&cell.0))
            && !self.inputs.contains(&cell)
            && !self.outputs.contains(&cell)
            && !self.ties.iter().any(|t| t.2 == cell)
    }

    /// What column `col` is.
    pub fn column(&self, col: i32) -> Column {
        self.columns[col as usize]
    }

    /// The gap a column belongs to, for a routing column; for any other column
    /// the gap to its east (the outputs' column has none).
    pub fn gap_of(&self, col: i32) -> Option<usize> {
        match self.column(col) {
            Column::Sources => Some(0),
            Column::Routing { gap, .. } => Some(gap),
            Column::Gates { index, .. } => Some(index + 1),
            Column::Outputs => None,
        }
    }

    /// The stage of every tile column: sources 0, gate columns 1 and up, outputs
    /// last; a routing column in gap `g` counts as `g` (the stage to its west).
    pub fn stage_count(&self) -> usize {
        self.routing.len() + 1
    }
}

/// Logic depth of every cell: 0 for a flip-flop, 1 for a gate fed only by
/// inputs, constants and flip-flops. Fails on a combinational loop.
pub fn levels(netlist: &Netlist, drivers: &[Driver]) -> Result<Vec<usize>, String> {
    let mut level = vec![0usize; netlist.cells.len()];
    let mut changed = true;
    let mut rounds = 0;
    while changed {
        changed = false;
        rounds += 1;
        if rounds > netlist.cells.len() + 1 {
            return Err("the netlist has a combinational loop".to_string());
        }
        for (c, cell) in netlist.cells.iter().enumerate() {
            if cell.is_dff() {
                continue;
            }
            let depth = cell
                .inputs
                .iter()
                .map(|&n| match drivers[n] {
                    Driver::Cell(d) if !netlist.cells[d].is_dff() => level[d] + 1,
                    _ => 1,
                })
                .max()
                .unwrap_or(1);
            if depth > level[c] {
                level[c] = depth;
                changed = true;
            }
        }
    }
    Ok(level)
}

/// The stage of every cell (1 and up, the flip-flop column being stage 1 when
/// there is one), and how many gaps the placement has. In the packed styles
/// the clock inverters go into the flip-flop column, not by their depth, so
/// that the clock reaches every flip-flop from beside it.
fn stages(netlist: &Netlist, level: &[usize], drivers: &[Driver], style: Style) -> (Vec<usize>, usize) {
    let has_dff = netlist.cells.iter().any(|c| c.is_dff());
    let first = if has_dff { 0 } else { 1 };
    let mut stage: Vec<usize> = level.iter().map(|&l| l + 1 - first).collect();
    if style.packs() {
        for c in clock_inverters(netlist, drivers) {
            stage[c] = 1;
        }
    }
    let gate_columns = stage.iter().copied().max().unwrap_or(0);
    (stage, gate_columns + 1)
}

/// The inverters between the clock input and the flip-flops' clock pins (the
/// one `dfflegalize` adds for a rising-edge clock): a NOT fed by a design
/// input whose every sink is a flip-flop's clock pin.
pub fn clock_inverters(netlist: &Netlist, drivers: &[Driver]) -> Vec<usize> {
    netlist
        .cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| {
            cell.kind == "NOT"
                && matches!(drivers[cell.inputs[0]], Driver::Input(_))
                && netlist
                    .sinks_of(cell.output)
                    .iter()
                    .all(|&(d, pin)| d != usize::MAX && netlist.cells[d].is_dff() && pin == 1)
        })
        .map(|(c, _)| c)
        .collect()
}

/// How many gaps of routing columns the placement of `netlist` in `style` has:
/// one more than its gate columns.
pub fn gap_count(netlist: &Netlist, style: Style) -> Result<usize, String> {
    let drivers = netlist.drivers()?;
    let level = levels(netlist, &drivers)?;
    Ok(stages(netlist, &level, &drivers, style).1)
}

/// Places the netlist with `routing[g]` routing columns in gap `g`, the rows
/// taken as `style` says.
pub fn place(netlist: &Netlist, routing: &[i32], style: Style) -> Result<Placement, String> {
    let drivers = netlist.drivers()?;
    let level = levels(netlist, &drivers)?;
    let has_dff = netlist.cells.iter().any(|c| c.is_dff());
    let (stage, gaps) = stages(netlist, &level, &drivers, style);
    let inverters = if style.packs() { clock_inverters(netlist, &drivers) } else { Vec::new() };
    if routing.len() != gaps {
        return Err(format!("the placement has {gaps} gaps but {} routing widths were given", routing.len()));
    }
    if routing.iter().any(|&r| r < 1) {
        return Err("every gap needs at least one routing column".to_string());
    }
    let used_constants: Vec<(NetId, bool)> =
        netlist.constants.iter().copied().filter(|(net, _)| !netlist.sinks_of(*net).is_empty()).collect();

    // Columns: sources, routing, [flip-flops (two cells), routing,] gates, routing,
    // ..., gates, routing, outputs.
    let mut columns = vec![Column::Sources];
    let mut gate_col_of_stage = vec![0i32; gaps];
    let mut by_stage: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (c, &s) in stage.iter().enumerate() {
        by_stage.entry(s).or_default().push(c);
    }
    for (gap, &width) in routing.iter().enumerate() {
        for offset in 0..width {
            columns.push(Column::Routing { gap, offset });
        }
        let index = gap + 1;
        if index < gaps {
            gate_col_of_stage[index] = columns.len() as i32;
            let wide = has_dff && index == 1;
            for offset in 0..if wide { 2 } else { 1 } {
                columns.push(Column::Gates { index: index - 1, offset });
            }
        }
    }
    columns.push(Column::Outputs);
    let cols = columns.len() as i32;

    let gates_per_col = by_stage.values().map(Vec::len).max().unwrap_or(0) as i32;
    let sources = (netlist.inputs.len() + used_constants.len()) as i32;
    // A net can only pass a gate column through a row that holds neither a gate nor
    // one of its feeds, so every column needs as many free rows as there are nets
    // crossing it: nets that span it, in either direction (a flip-flop's D input
    // is fed back from the last level to the first column).
    let span = |net: NetId| -> (i32, i32) {
        let driver = match drivers[net] {
            Driver::Cell(d) => stage[d] as i32,
            _ => 0,
        };
        let sinks =
            netlist.sinks_of(net).into_iter().map(|(c, _)| if c == usize::MAX { gaps as i32 } else { stage[c] as i32 });
        sinks.chain(std::iter::once(driver)).fold((i32::MAX, i32::MIN), |(lo, hi), l| (lo.min(l), hi.max(l)))
    };
    let crossing_of_stage: Vec<i32> = (0..gaps)
        .map(|s| {
            (0..netlist.net_count)
                .filter(|&n| {
                    let (lo, hi) = span(n);
                    lo < s as i32 && (s as i32) < hi
                })
                .count() as i32
        })
        .collect();
    // The most rows any stage can take: three per gate (north feed, gate, south
    // feed) and one per crossing net; the packed sweeps need fewer.
    let crossing_rows =
        (1..gaps).map(|s| 3 * by_stage.get(&s).map_or(0, Vec::len) as i32 + crossing_of_stage[s]).max().unwrap_or(0);
    // One spare row at the bottom: two nets that have to swap rows need a third row to pass.
    let mut rows = if style.packs() { crossing_rows + 1 } else { (4 * gates_per_col).max(crossing_rows + 1) }
        .max(sources + 1)
        .max(netlist.outputs.len() as i32 + 1)
        .max(2);

    // Rows: start in index order, then sweep by barycenter a few times.
    let mut row_of_cell = vec![0i32; netlist.cells.len()];
    // Packed: the sides each gate's pins were given when its rows were reserved.
    let mut sides_of_cell: Vec<Vec<Edge>> = vec![Vec::new(); netlist.cells.len()];
    let mut row_of_input: Vec<i32> = (0..netlist.inputs.len() as i32).collect();
    let mut row_of_tie: Vec<i32> = (0..used_constants.len() as i32).map(|i| i + netlist.inputs.len() as i32).collect();
    let mut row_of_output: Vec<i32> = (0..netlist.outputs.len() as i32).collect();
    for cells in by_stage.values() {
        for (i, &c) in cells.iter().enumerate() {
            row_of_cell[c] = 1 + 4 * i as i32;
        }
    }
    let driver_row = |net: NetId, row_of_cell: &[i32], row_of_input: &[i32], row_of_tie: &[i32]| -> f64 {
        match drivers[net] {
            Driver::Cell(d) => f64::from(row_of_cell[d]),
            Driver::Input(i) => f64::from(row_of_input[i]),
            Driver::Const(v) => {
                let i = used_constants.iter().position(|&(_, value)| value == v).unwrap_or(0);
                f64::from(row_of_tie.get(i).copied().unwrap_or(0))
            }
        }
    };
    for _ in 0..4 {
        // Gates: by the average row of their drivers, the clock inverters in the
        // middle of the flip-flop column.
        // Packed, the first row below each stage.
        let mut bottoms: Vec<(usize, i32)> = Vec::new();
        for (&s, cells) in &by_stage {
            let mut order: Vec<(f64, usize)> = cells
                .iter()
                .map(|&c| {
                    let rows: Vec<f64> = netlist.cells[c]
                        .inputs
                        .iter()
                        .map(|&n| driver_row(n, &row_of_cell, &row_of_input, &row_of_tie))
                        .collect();
                    (rows.iter().sum::<f64>() / rows.len().max(1) as f64, c)
                })
                .collect();
            order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
            let mut order: Vec<usize> = order.iter().map(|(_, c)| *c).collect();
            in_the_middle(&mut order, &inverters);
            if style.packs() {
                let from: Vec<Vec<f64>> = order
                    .iter()
                    .map(|&c| {
                        netlist.cells[c]
                            .inputs
                            .iter()
                            .map(|&n| driver_row(n, &row_of_cell, &row_of_input, &row_of_tie))
                            .collect()
                    })
                    .collect();
                let crossing = crossing_of_stage[s];
                let every = if crossing > 0 { (order.len() as i32 + crossing - 1) / crossing } else { i32::MAX };
                let bottom = pack_stage(netlist, &order, &from, every, &mut row_of_cell, &mut sides_of_cell);
                bottoms.push((s, bottom));
                rows = rows.max(bottom + 1);
            } else {
                for (i, &c) in order.iter().enumerate() {
                    row_of_cell[c] = 1 + 4 * i as i32;
                }
            }
        }
        if style == Style::Stretched {
            // Every stage shorter than the tallest is stretched over the
            // tallest's rows, uniformly: differences between rows only grow,
            // so no gate lands on another's row or feed.
            let tallest = bottoms.iter().map(|&(_, bottom)| bottom).max().unwrap_or(0);
            for &(s, bottom) in bottoms.iter().filter(|&&(_, bottom)| bottom > 1 && bottom < tallest) {
                for &c in &by_stage[&s] {
                    row_of_cell[c] = row_of_cell[c] * (tallest - 1) / (bottom - 1);
                }
            }
        }
        // Outputs: in the order of their drivers' rows and each as near its
        // driver's row as the ones above allow, so that most output nets run
        // straight instead of all converging on the top rows.
        let mut output_order: Vec<(f64, usize)> = netlist
            .outputs
            .iter()
            .enumerate()
            .map(|(o, p)| (driver_row(p.net, &row_of_cell, &row_of_input, &row_of_tie), o))
            .collect();
        output_order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
        let targets: Vec<f64> = output_order.iter().map(|(t, _)| *t).collect();
        for (row, &(_, o)) in spread(&targets, rows).into_iter().zip(&output_order) {
            row_of_output[o] = row;
        }
        // Inputs and constants: by the average row of what they feed.
        let sink_row = |net: NetId| -> f64 {
            let sinks = netlist.sinks_of(net);
            let rows: Vec<f64> = sinks
                .iter()
                .map(|&(c, o)| if c == usize::MAX { f64::from(row_of_output[o]) } else { f64::from(row_of_cell[c]) })
                .collect();
            rows.iter().sum::<f64>() / rows.len().max(1) as f64
        };
        let mut order: Vec<(f64, usize, bool)> =
            netlist.inputs.iter().enumerate().map(|(i, p)| (sink_row(p.net), i, false)).collect();
        order.extend(used_constants.iter().enumerate().map(|(i, &(net, _))| (sink_row(net), i, true)));
        order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
        let targets: Vec<f64> = order.iter().map(|(t, _, _)| *t).collect();
        for (row, &(_, i, is_tie)) in spread(&targets, rows).into_iter().zip(&order) {
            if is_tie {
                row_of_tie[i] = row;
            } else {
                row_of_input[i] = row;
            }
        }
    }
    let gates: Vec<Cell> = (0..netlist.cells.len()).map(|c| (gate_col_of_stage[stage[c]], row_of_cell[c])).collect();
    let widths: Vec<i32> = netlist.cells.iter().map(|c| if c.is_dff() { 2 } else { 1 }).collect();
    let inputs: Vec<Cell> = row_of_input.iter().map(|&r| (0, r)).collect();
    let ties: Vec<(NetId, bool, Cell)> =
        used_constants.iter().zip(&row_of_tie).map(|(&(net, value), &r)| (net, value, (0, r))).collect();
    let outputs: Vec<Cell> = row_of_output.iter().map(|&r| (cols - 1, r)).collect();

    // Pin edges: give each input net the side closest to where it comes from
    // (packed, the sides its rows were reserved for). A flip-flop's sides are
    // fixed: D on the west, the clock on the north.
    let mut pin_edges = Vec::new();
    for (c, cell) in netlist.cells.iter().enumerate() {
        if cell.is_dff() {
            pin_edges.push(vec![Edge::West, Edge::North]);
            continue;
        }
        if style.packs() {
            pin_edges.push(sides_of_cell[c].clone());
            continue;
        }
        let row = f64::from(gates[c].1);
        let from: Vec<f64> =
            cell.inputs.iter().map(|&n| driver_row(n, &row_of_cell, &row_of_input, &row_of_tie)).collect();
        pin_edges.push(assign_sides(&from, row, row, 0.0, &vec![None; from.len()]));
    }

    Ok(Placement {
        cols,
        rows,
        // Routing columns take the narrow wiring tiles; gates, sources and outputs a full cell.
        col_width: columns.iter().map(|c| if matches!(c, Column::Routing { .. }) { NARROW } else { PITCH }).collect(),
        // Rows take the shallow tiles, but a row a flip-flop sits in, which is a cell deep.
        row_depth: (0..rows)
            .map(|r| {
                let flip_flop = netlist.cells.iter().zip(&gates).any(|(cell, &(_, row))| cell.is_dff() && row == r);
                if flip_flop { PITCH } else { SHALLOW }
            })
            .collect(),
        gates,
        widths,
        pin_edges,
        inputs,
        outputs,
        ties,
        routing: routing.to_vec(),
        columns,
        stages: stage,
    })
}

/// A first guess at the routing columns each gap needs: in a gap, every net
/// that must change rows on its way to the next stage takes the north-south
/// track of one routing column over the rows it spans, and two nets whose
/// spans overlap cannot share a column. The most spans over any one row is
/// therefore a lower bound on the columns, the channel density of the gap.
pub fn estimate_routing(netlist: &Netlist, placement: &Placement) -> Vec<i32> {
    let gaps = placement.routing.len();
    let drivers = netlist.drivers().unwrap_or_default();
    let mut estimate = vec![1i32; gaps];
    for (gap, slot) in estimate.iter_mut().enumerate() {
        let mut spans: Vec<(i32, i32)> = Vec::new();
        for (net, &driver) in drivers.iter().enumerate() {
            let (driver_stage, driver_row) = match driver {
                Driver::Cell(c) => (placement.stages[c], placement.gates[c].1),
                Driver::Input(i) => (0, placement.inputs[i].1),
                Driver::Const(_) => match placement.ties.iter().find(|t| t.0 == net) {
                    Some(t) => (0, t.2.1),
                    None => continue,
                },
            };
            // The rows this net arrives at in the stage east of the gap; a net
            // whose sink is in its driver's own stage (a clock inverter's) turns
            // round in the gap east of that stage.
            let mut rows: Vec<i32> = Vec::new();
            for (c, pin) in netlist.sinks_of(net) {
                let (sink_stage, row) = if c == usize::MAX {
                    (gaps, placement.outputs[pin].1)
                } else {
                    let (_, row) = placement.gates[c];
                    let row = match placement.pin_edges[c][pin] {
                        Edge::North => row - 1,
                        Edge::South => row + 1,
                        _ => row,
                    };
                    (placement.stages[c], row)
                };
                let through = if sink_stage == driver_stage { sink_stage } else { sink_stage - 1 };
                if through == gap {
                    rows.push(row);
                }
            }
            if rows.is_empty() {
                continue;
            }
            // A net driven after this gap (the D feedback of a flip-flop) comes back
            // along some row and must reach its sink's row somewhere: count it here.
            rows.push(driver_row);
            let lo = *rows.iter().min().expect("non-empty");
            let hi = *rows.iter().max().expect("non-empty");
            if lo < hi {
                spans.push((lo, hi));
            }
        }
        let mut density = 0;
        for row in 0..placement.rows {
            let here = spans.iter().filter(|(lo, hi)| *lo <= row && row <= *hi).count() as i32;
            density = density.max(here);
        }
        *slot = density.max(1);
    }
    estimate
}

/// Moves the cells of `middle` that are in `order` to its middle, keeping the
/// order of the rest: a clock inverter sits halfway down the flip-flop column,
/// as near to every flip-flop as one place can be.
fn in_the_middle(order: &mut Vec<usize>, middle: &[usize]) {
    let moved: Vec<usize> = order.iter().copied().filter(|c| middle.contains(c)).collect();
    if moved.is_empty() {
        return;
    }
    order.retain(|c| !middle.contains(c));
    let at = order.len() / 2;
    order.splice(at..at, moved);
}

/// Distinct rows for things listed in the order of their target rows (which
/// must not decrease): each as near its target as the ones before it allow,
/// all within `rows`, which must be more than there are things.
fn spread(targets: &[f64], rows: i32) -> Vec<i32> {
    let n = targets.len() as i32;
    let mut out: Vec<i32> = Vec::with_capacity(targets.len());
    for (i, &t) in targets.iter().enumerate() {
        let want = (t.round() as i32).max(0);
        let above = if i == 0 { -1 } else { out[i - 1] };
        out.push(want.max(above + 1));
    }
    // Those that ran off the bottom move up, keeping one row each.
    for (i, row) in out.iter_mut().enumerate() {
        *row = (*row).min(rows - n + i as i32);
    }
    out
}

/// Lays the gates of one stage, `order` from the top down, packed: a gate takes
/// a row for its north feed when a pin reads on the north, its own row, and a
/// row for its south feed when a pin reads on the south (`from[i]` holds the
/// rows gate `order[i]`'s nets come from). Two gates in a row that would each
/// take the same net on a feed side take it on their facing sides, the upper on
/// the south and the lower on the north, and share the one feed tile between
/// them, a row fewer than two feeds. A free row is left
/// after every `every` gates for the nets that pass the column. A flip-flop
/// reads D on the west and the clock on the north. Records each gate's row and
/// sides; returns the first row below the stage.
fn pack_stage(
    netlist: &Netlist,
    order: &[usize],
    from: &[Vec<f64>],
    every: i32,
    row_of_cell: &mut [i32],
    sides_of_cell: &mut [Vec<Edge>],
) -> i32 {
    let reads_once = |c: usize, net: NetId| netlist.cells[c].inputs.iter().filter(|&&m| m == net).count() == 1;
    let mut top = 0i32;
    let mut since_free = 0i32;
    // The net shared with the gate above, which this gate reads on the north.
    let mut shared_above: Option<NetId> = None;
    for (i, &c) in order.iter().enumerate() {
        let cell = &netlist.cells[c];
        let (sides, shared_below) = if cell.is_dff() {
            (vec![Edge::West, Edge::North], None)
        } else {
            let forced_above: Vec<Option<Edge>> =
                cell.inputs.iter().map(|&net| (Some(net) == shared_above).then_some(Edge::North)).collect();
            // Reading on the north costs a row above the gate, unless that row is
            // the feed shared with the gate above.
            let row_with_north = if shared_above.is_some() { top } else { top + 1 };
            let own = assign_sides(&from[i], f64::from(top), f64::from(row_with_north), FEED_ROW_COST, &forced_above);
            // A net the gate below also reads, that both would take on a feed side
            // of their own accord (so sharing the tile saves a row); neither reads it
            // twice, and it is not the one shared above. The lowest such net.
            let below = order.get(i + 1).copied().filter(|&b| !netlist.cells[b].is_dff());
            let shared_below = below.and_then(|b| {
                let next = &netlist.cells[b];
                let unforced = vec![None; next.inputs.len()];
                let own_below =
                    assign_sides(&from[i + 1], f64::from(top + 2), f64::from(top + 3), FEED_ROW_COST, &unforced);
                let on_feed = |sides: &[Edge], inputs: &[NetId], net: NetId| {
                    inputs.iter().zip(sides).any(|(&m, &side)| m == net && side != Edge::West)
                };
                cell.inputs
                    .iter()
                    .copied()
                    .filter(|&net| {
                        Some(net) != shared_above
                            && reads_once(c, net)
                            && reads_once(b, net)
                            && on_feed(&own, &cell.inputs, net)
                            && on_feed(&own_below, &next.inputs, net)
                    })
                    .min()
            });
            match shared_below {
                None => (own, None),
                Some(shared) => {
                    let forced: Vec<Option<Edge>> = cell
                        .inputs
                        .iter()
                        .zip(&forced_above)
                        .map(|(&net, &above)| if net == shared { Some(Edge::South) } else { above })
                        .collect();
                    let sides =
                        assign_sides(&from[i], f64::from(top), f64::from(row_with_north), FEED_ROW_COST, &forced);
                    (sides, shared_below)
                }
            }
        };
        let uses_north = sides.contains(&Edge::North);
        let uses_south = sides.contains(&Edge::South);
        let row = top + i32::from(uses_north && shared_above.is_none());
        row_of_cell[c] = row;
        sides_of_cell[c] = sides;
        top = row + 1 + i32::from(uses_south);
        since_free += 1;
        shared_above = shared_below;
        if shared_below.is_none() && since_free >= every && i + 1 < order.len() {
            top += 1;
            since_free = 0;
        }
    }
    top
}

/// Distinct sides for up to three input pins, minimising how far each net has
/// to travel vertically to reach its side (west: the gate's row; north: above;
/// south: below) plus `feed_cost` for every north or south side. The gate sits
/// at `row`, or at `row_with_north` when a pin reads on the north. A pin with a
/// side in `forced` gets that side.
fn assign_sides(
    from_rows: &[f64],
    row: f64,
    row_with_north: f64,
    feed_cost: f64,
    forced: &[Option<Edge>],
) -> Vec<Edge> {
    let sides = [Edge::West, Edge::North, Edge::South];
    let n = from_rows.len();
    let mut best: Option<(f64, Vec<Edge>)> = None;
    // Every ordered choice of n distinct sides out of three.
    let mut choices: Vec<Vec<Edge>> = vec![Vec::new()];
    for _ in 0..n {
        let mut longer = Vec::new();
        for prefix in &choices {
            for &side in sides.iter().filter(|s| !prefix.contains(s)) {
                let mut next = prefix.clone();
                next.push(side);
                longer.push(next);
            }
        }
        choices = longer;
    }
    for choice in choices {
        if choice.iter().zip(forced).any(|(&e, f)| f.is_some_and(|f| f != e)) {
            continue;
        }
        let gate_row = if choice.contains(&Edge::North) { row_with_north } else { row };
        let target = |e: Edge| match e {
            Edge::North => gate_row - 1.0,
            Edge::South => gate_row + 1.0,
            _ => gate_row,
        };
        let cost: f64 = choice
            .iter()
            .zip(from_rows)
            .map(|(&e, &r)| (r - target(e)).abs() + if e == Edge::West { 0.0 } else { feed_cost })
            .sum();
        if best.as_ref().is_none_or(|(c, _)| cost < *c) {
            best = Some((cost, choice));
        }
    }
    best.map(|(_, c)| c).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::Library;

    const TWO_LEVELS: &str = r#"{"modules":{"top":{
        "ports":{"a":{"direction":"input","bits":[2]},"b":{"direction":"input","bits":[3]},"y":{"direction":"output","bits":[5]}},
        "cells":{"g":{"type":"NOR2","connections":{"A":[2],"B":[3],"Y":[4]}},
                 "h":{"type":"NOT","connections":{"A":[4],"Y":[5]}}}}}}"#;

    #[test]
    fn columns_are_named_stably_and_gaps_have_their_own_width() {
        let netlist = Netlist::from_yosys_json(TWO_LEVELS, "top", &Library::builtin().unwrap()).unwrap();
        assert_eq!(gap_count(&netlist, Style::Pitch4).unwrap(), 3);
        let p = place(&netlist, &[2, 1, 3], Style::Pitch4).unwrap();
        // sources, 2 routing, gate, 1 routing, gate, 3 routing, outputs
        assert_eq!(p.cols, 1 + 2 + 1 + 1 + 1 + 3 + 1);
        assert_eq!(p.column(0), Column::Sources);
        assert_eq!(p.column(1), Column::Routing { gap: 0, offset: 0 });
        assert_eq!(p.column(3), Column::Gates { index: 0, offset: 0 });
        assert_eq!(p.column(4), Column::Routing { gap: 1, offset: 0 });
        assert_eq!(p.column(5), Column::Gates { index: 1, offset: 0 });
        assert_eq!(p.column(8), Column::Routing { gap: 2, offset: 2 });
        assert_eq!(p.column(9), Column::Outputs);
        assert_eq!(p.gates[0].0, 3);
        assert_eq!(p.gates[1].0, 5);
        assert_eq!(p.outputs[0].0, 9);
        // A routing column is narrow; every other column is a cell wide, and every
        // row shallow (no flip-flop sits in any).
        let widths: Vec<i32> =
            p.columns.iter().map(|c| if matches!(c, Column::Routing { .. }) { NARROW } else { PITCH }).collect();
        assert_eq!(p.col_width, widths);
        assert_eq!(p.col_width[..4], [PITCH, NARROW, NARROW, PITCH]);
        assert_eq!(p.row_depth, vec![SHALLOW; p.rows as usize]);
        // Widening the middle gap moves the later columns but keeps their names.
        let q = place(&netlist, &[2, 2, 3], Style::Pitch4).unwrap();
        assert_eq!(q.column(6), Column::Gates { index: 1, offset: 0 });
        assert_eq!(q.column(5), Column::Routing { gap: 1, offset: 1 });
        assert!(place(&netlist, &[1, 1], Style::Pitch4).is_err());
    }

    #[test]
    fn rows_spread_towards_their_targets_and_stay_distinct() {
        assert_eq!(spread(&[0.0, 0.4, 5.0, 5.2], 10), vec![0, 1, 5, 6]);
        // Not enough room below: the last ones move up, still one row each.
        assert_eq!(spread(&[7.0, 8.0, 9.0], 10), vec![7, 8, 9]);
        assert_eq!(spread(&[8.0, 9.0, 9.0], 10), vec![7, 8, 9]);
        assert_eq!(spread(&[-3.0, 2.0], 3), vec![0, 2]);
    }

    #[test]
    fn a_feed_side_costs_a_packed_gate_a_row_so_the_west_wins_when_it_is_near() {
        let free = |n: usize| vec![None; n];
        // At the four-row pitch (no cost) a driver one row up takes the north side.
        assert_eq!(assign_sides(&[4.0], 5.0, 5.0, 0.0, &free(1)), vec![Edge::North]);
        // Packed, the gate moves down a row to make room for a north feed, so
        // the west port is as near, and the feed row costs on top: west.
        assert_eq!(assign_sides(&[4.0], 5.0, 6.0, FEED_ROW_COST, &free(1)), vec![Edge::West]);
        // Three nets from below, above and the gate's own row: the one at the
        // gate's row takes the west, the one below the south, the one above the north.
        assert_eq!(
            assign_sides(&[9.0, 2.0, 5.0], 5.0, 6.0, FEED_ROW_COST, &free(3)),
            vec![Edge::South, Edge::West, Edge::North]
        );
        // A forced side is kept whatever it costs.
        assert_eq!(
            assign_sides(&[9.0, 2.0, 5.0], 5.0, 6.0, FEED_ROW_COST, &[None, Some(Edge::South), None]),
            vec![Edge::West, Edge::South, Edge::North]
        );
    }

    /// Three NOR2 gates reading nets (b, a), (c, a) and (a, d).
    const SHARE: &str = r#"{"modules":{"top":{
        "ports":{"a":{"direction":"input","bits":[2]},"b":{"direction":"input","bits":[3]},"c":{"direction":"input","bits":[4]},
                 "d":{"direction":"input","bits":[5]},"y":{"direction":"output","bits":[6]},"z":{"direction":"output","bits":[7]},
                 "w":{"direction":"output","bits":[8]}},
        "cells":{"g0":{"type":"NOR2","connections":{"A":[3],"B":[2],"Y":[6]}},
                 "g1":{"type":"NOR2","connections":{"A":[4],"B":[2],"Y":[7]}},
                 "g2":{"type":"NOR2","connections":{"A":[2],"B":[5],"Y":[8]}}}}}}"#;

    #[test]
    fn packed_neighbours_share_a_feed_tile_only_when_both_would_take_the_net_on_a_feed_side() {
        let netlist = Netlist::from_yosys_json(SHARE, "top", &Library::builtin().unwrap()).unwrap();
        let pack = |order: &[usize], from: &[Vec<f64>], every: i32| {
            let mut rows = vec![0; 3];
            let mut sides = vec![Vec::new(); 3];
            let bottom = pack_stage(&netlist, order, from, every, &mut rows, &mut sides);
            (rows, sides, bottom)
        };
        // g0 reads b from above on the west and a from below on the south; g1
        // reads c from far below on the west and a on a feed side: a is shared,
        // g0 south, g1 north, and the pair takes three rows instead of four.
        let (rows, sides, bottom) = pack(&[0, 1], &[vec![0.0, 6.0], vec![10.0, 6.0]], i32::MAX);
        assert_eq!((rows[0], rows[1]), (0, 2));
        assert_eq!((&sides[0], &sides[1]), (&vec![Edge::West, Edge::South], &vec![Edge::West, Edge::North]));
        assert_eq!(bottom, 3);
        // g2 reads a on the west of its own accord: no sharing, g2 one row lower
        // with its north feed between.
        let (rows, sides, bottom) = pack(&[0, 2], &[vec![0.0, 6.0], vec![6.0, 0.0]], i32::MAX);
        assert_eq!((rows[0], rows[2]), (0, 3));
        assert_eq!(sides[2], vec![Edge::West, Edge::North]);
        assert_eq!(bottom, 4);
        // A free row after every gate when nets pass the column.
        let (rows, _, bottom) = pack(&[2, 0], &[vec![6.0, 0.0], vec![0.0, 6.0]], 1);
        assert_eq!((rows[2], rows[0]), (1, 3));
        assert_eq!(bottom, 5);
    }

    /// Packed, no two gates of a column want the same feed tile unless they
    /// share it for one net on facing sides, every feed tile is free, and the
    /// columns are no taller than at the four-row pitch.
    #[test]
    fn packed_feed_tiles_never_collide_and_the_columns_are_no_taller() {
        for seed in 0..12u64 {
            let mut rng = crate::tiletest::Rng::new(seed);
            let gates = 4 + rng.below(12);
            let inputs = 2 + rng.below(4);
            let netlist = crate::pnrtest::random_netlist_sized(&mut rng, gates, inputs, 0);
            let gaps = gap_count(&netlist, Style::Packed).unwrap();
            assert_eq!(gaps, gap_count(&netlist, Style::Pitch4).unwrap(), "no clock inverter to move");
            let packed = place(&netlist, &vec![1; gaps], Style::Packed).unwrap();
            let pitch4 = place(&netlist, &vec![1; gaps], Style::Pitch4).unwrap();
            let tallest = |p: &Placement| p.gates.iter().map(|g| g.1).max().unwrap_or(0);
            assert!(tallest(&packed) <= tallest(&pitch4) && packed.rows <= pitch4.rows, "seed {seed}");
            let mut feeds: BTreeMap<Cell, Vec<(usize, Edge)>> = BTreeMap::new();
            for (c, cell) in netlist.cells.iter().enumerate() {
                let (col, row) = packed.gates[c];
                assert!(row >= 0 && row < packed.rows, "seed {seed}: gate {c} at row {row} of {}", packed.rows);
                for (&net, &edge) in cell.inputs.iter().zip(&packed.pin_edges[c]) {
                    let feed = match edge {
                        Edge::North => (col, row - 1),
                        Edge::South => (col, row + 1),
                        _ => continue,
                    };
                    assert!(packed.is_free(feed), "seed {seed}: the feed tile {feed:?} of gate {c} is taken");
                    feeds.entry(feed).or_default().push((net, edge));
                }
            }
            for (feed, users) in feeds {
                match users.as_slice() {
                    [_] => {}
                    [(a, ea), (b, eb)] => assert!(a == b && ea != eb, "seed {seed}: {feed:?} fed by {users:?}"),
                    _ => panic!("seed {seed}: {feed:?} fed by {users:?}"),
                }
            }
        }
    }

    /// Stretched, every stage is drawn out over the rows of the tallest: the
    /// random netlists need no more rows than at the four-row pitch (a short
    /// stage may reach lower than the four-row pitch puts any gate, but never
    /// below the tallest stage's own rows), their feed tiles stay free and
    /// distinct, and a short stage's gate sits lower than packed.
    #[test]
    fn stretched_stages_span_the_tallest_and_keep_their_feeds_clear() {
        for seed in 0..12u64 {
            let mut rng = crate::tiletest::Rng::new(seed);
            let gates = 4 + rng.below(12);
            let inputs = 2 + rng.below(4);
            let netlist = crate::pnrtest::random_netlist_sized(&mut rng, gates, inputs, 0);
            let gaps = gap_count(&netlist, Style::Stretched).unwrap();
            let stretched = place(&netlist, &vec![1; gaps], Style::Stretched).unwrap();
            let pitch4 = place(&netlist, &vec![1; gaps], Style::Pitch4).unwrap();
            assert!(stretched.rows <= pitch4.rows, "seed {seed}: {} rows against {}", stretched.rows, pitch4.rows);
            let mut feeds: BTreeMap<Cell, Vec<(usize, Edge)>> = BTreeMap::new();
            for (c, cell) in netlist.cells.iter().enumerate() {
                let (col, row) = stretched.gates[c];
                assert!(row >= 0 && row < stretched.rows, "seed {seed}: gate {c} at row {row}");
                for (&net, &edge) in cell.inputs.iter().zip(&stretched.pin_edges[c]) {
                    let feed = match edge {
                        Edge::North => (col, row - 1),
                        Edge::South => (col, row + 1),
                        _ => continue,
                    };
                    assert!(stretched.is_free(feed), "seed {seed}: the feed tile {feed:?} of gate {c} is taken");
                    feeds.entry(feed).or_default().push((net, edge));
                }
            }
            for (feed, users) in feeds {
                match users.as_slice() {
                    [_] => {}
                    [(a, ea), (b, eb)] => assert!(a == b && ea != eb, "seed {seed}: {feed:?} fed by {users:?}"),
                    _ => panic!("seed {seed}: {feed:?} fed by {users:?}"),
                }
            }
        }
        // Four inverters in the first stage, one NOR of the last two in the
        // second: packed, the NOR takes a row near the top with a feed above
        // it; stretched, the second stage is drawn out over the first's rows,
        // so it sits lower.
        let json = r#"{"modules":{"top":{
            "ports":{"a":{"direction":"input","bits":[2]},"b":{"direction":"input","bits":[3]},
                     "c":{"direction":"input","bits":[4]},"d":{"direction":"input","bits":[5]},
                     "y":{"direction":"output","bits":[10]},"v":{"direction":"output","bits":[6]},
                     "w":{"direction":"output","bits":[7]}},
            "cells":{"n0":{"type":"NOT","connections":{"A":[2],"Y":[6]}},
                     "n1":{"type":"NOT","connections":{"A":[3],"Y":[7]}},
                     "n2":{"type":"NOT","connections":{"A":[4],"Y":[8]}},
                     "n3":{"type":"NOT","connections":{"A":[5],"Y":[9]}},
                     "g":{"type":"NOR2","connections":{"A":[9],"B":[8],"Y":[10]}}}}}}"#;
        let netlist = Netlist::from_yosys_json(json, "top", &Library::builtin().unwrap()).unwrap();
        let packed = place(&netlist, &[1, 1, 1], Style::Packed).unwrap();
        let stretched = place(&netlist, &[1, 1, 1], Style::Stretched).unwrap();
        let g = netlist.cells.iter().position(|c| c.kind == "NOR2").unwrap();
        assert!(stretched.gates[g].1 > packed.gates[g].1, "{:?} vs {:?}", stretched.gates, packed.gates);
        assert_eq!(stretched.rows, packed.rows);
    }

    /// The clock inverter goes into the flip-flop column in the packed styles,
    /// halfway down it, and stays a gate column of its own at the four-row pitch.
    #[test]
    fn the_clock_inverter_sits_beside_the_flip_flops_in_the_packed_styles() {
        let toggle = r#"{"modules":{"top":{
            "ports":{"clk":{"direction":"input","bits":[2]},"q":{"direction":"output","bits":[3]}},
            "cells":{"inv":{"type":"NOT","connections":{"A":[3],"Y":[4]}},
                     "nclk":{"type":"NOT","connections":{"A":[2],"Y":[5]}},
                     "ff":{"type":"DFF","connections":{"D":[4],"CLK":[5],"Q":[3]}}}}}}"#;
        let netlist = Netlist::from_yosys_json(toggle, "top", &Library::builtin().unwrap()).unwrap();
        let drivers = netlist.drivers().unwrap();
        let nclk = netlist.cells.iter().position(|c| c.name == "nclk").unwrap();
        let ff = netlist.cells.iter().position(|c| c.is_dff()).unwrap();
        assert_eq!(clock_inverters(&netlist, &drivers), vec![nclk]);
        assert_eq!(gap_count(&netlist, Style::Pitch4).unwrap(), 3);
        let pitch4 = place(&netlist, &[1, 1, 1], Style::Pitch4).unwrap();
        assert_ne!(pitch4.gates[nclk].0, pitch4.gates[ff].0);
        assert_eq!(pitch4.stages[nclk], 2);
        for style in [Style::Packed, Style::Stretched] {
            assert_eq!(gap_count(&netlist, style).unwrap(), 3, "{}", style.name());
            let p = place(&netlist, &[1, 1, 1], style).unwrap();
            assert_eq!(p.gates[nclk].0, p.gates[ff].0, "{}", style.name());
            assert_eq!(p.stages[nclk], 1);
            assert_eq!(p.pin_edges[nclk], vec![Edge::West]);
            // Its net turns round in the gap east of the column: the estimate counts it there.
            assert!(estimate_routing(&netlist, &p)[1] >= 1);
        }
        // Halfway down: among four flip-flops the inverter comes after the second.
        let mut order = vec![10, 11, 12, 13, 7];
        in_the_middle(&mut order, &[7]);
        assert_eq!(order, vec![10, 11, 7, 12, 13]);
        let mut order = vec![10, 7];
        in_the_middle(&mut order, &[7, 8]);
        assert_eq!(order, vec![7, 10]);
        let mut untouched = vec![1, 2, 3];
        in_the_middle(&mut untouched, &[9]);
        assert_eq!(untouched, vec![1, 2, 3]);
    }

    #[test]
    fn the_estimate_counts_overlapping_row_changes() {
        // Two inputs that each feed the same two gates in the first column: both
        // nets span the rows between the gates, so the first gap needs two columns.
        // The outputs sit in their gates' rows, so their nets run straight: one.
        let json = r#"{"modules":{"top":{
            "ports":{"a":{"direction":"input","bits":[2]},"b":{"direction":"input","bits":[3]},
                     "y":{"direction":"output","bits":[4]},"z":{"direction":"output","bits":[5]}},
            "cells":{"g":{"type":"NOR2","connections":{"A":[2],"B":[3],"Y":[4]}},
                     "h":{"type":"NOR2","connections":{"A":[3],"B":[2],"Y":[5]}}}}}}"#;
        let netlist = Netlist::from_yosys_json(json, "top", &Library::builtin().unwrap()).unwrap();
        let p = place(&netlist, &[1, 1], Style::Pitch4).unwrap();
        let estimate = estimate_routing(&netlist, &p);
        assert_eq!(estimate, vec![2, 1]);
    }
}
