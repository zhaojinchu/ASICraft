//! Placement: which tile each gate, input, output and constant occupies.
//!
//! Gates go into columns by logic depth, with routing columns between. Within a
//! column, gates are ordered by the average row of what feeds them (a few
//! barycenter sweeps), which keeps most wires short and uncrossed. Each gate
//! takes four rows: the tile above it feeds its north input, the tile below
//! its south input, its west input comes straight in from the left, and the
//! fourth row is left free so that other nets can pass the gate column.
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
use crate::tile::{Edge, PITCH};

/// A tile position: (column, row). Columns run east, rows run south.
pub type Cell = (i32, i32);

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
/// there is one), and how many gaps the placement has.
fn stages(netlist: &Netlist, level: &[usize]) -> (Vec<usize>, usize) {
    let has_dff = netlist.cells.iter().any(|c| c.is_dff());
    let first = if has_dff { 0 } else { 1 };
    let stage: Vec<usize> = level.iter().map(|&l| l + 1 - first).collect();
    let gate_columns = stage.iter().copied().max().unwrap_or(0);
    (stage, gate_columns + 1)
}

/// How many gaps of routing columns the placement of `netlist` has: one more
/// than its gate columns.
pub fn gap_count(netlist: &Netlist) -> Result<usize, String> {
    let level = levels(netlist, &netlist.drivers()?)?;
    Ok(stages(netlist, &level).1)
}

/// Places the netlist with `routing[g]` routing columns in gap `g`.
pub fn place(netlist: &Netlist, routing: &[i32]) -> Result<Placement, String> {
    let drivers = netlist.drivers()?;
    let level = levels(netlist, &drivers)?;
    let has_dff = netlist.cells.iter().any(|c| c.is_dff());
    let (stage, gaps) = stages(netlist, &level);
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
    let crossing_rows = (1..gaps)
        .map(|s| {
            let crossing = (0..netlist.net_count)
                .filter(|&n| {
                    let (lo, hi) = span(n);
                    lo < s as i32 && (s as i32) < hi
                })
                .count() as i32;
            3 * by_stage.get(&s).map_or(0, Vec::len) as i32 + crossing
        })
        .max()
        .unwrap_or(0);
    // One spare row at the bottom: two nets that have to swap rows need a third row to pass.
    let rows = (4 * gates_per_col).max(crossing_rows + 1).max(sources + 1).max(netlist.outputs.len() as i32 + 1).max(2);

    // Rows: start in index order, then sweep by barycenter a few times.
    let mut row_of_cell = vec![0i32; netlist.cells.len()];
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
        // Gates: by the average row of their drivers.
        for cells in by_stage.values() {
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
            for (i, (_, c)) in order.iter().enumerate() {
                row_of_cell[*c] = 1 + 4 * i as i32;
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

    // Pin edges: give each input net the side closest to where it comes from. A
    // flip-flop's sides are fixed: D on the west, the clock on the north.
    let mut pin_edges = Vec::new();
    for (c, cell) in netlist.cells.iter().enumerate() {
        if cell.is_dff() {
            pin_edges.push(vec![Edge::West, Edge::North]);
            continue;
        }
        let row = f64::from(gates[c].1);
        let from: Vec<f64> =
            cell.inputs.iter().map(|&n| driver_row(n, &row_of_cell, &row_of_input, &row_of_tie)).collect();
        pin_edges.push(assign_sides(&from, row));
    }

    Ok(Placement {
        cols,
        rows,
        col_width: vec![PITCH; cols as usize],
        row_depth: vec![PITCH; rows as usize],
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
            let driver_row = match driver {
                Driver::Cell(c) => placement.gates[c].1,
                Driver::Input(i) => placement.inputs[i].1,
                Driver::Const(_) => match placement.ties.iter().find(|t| t.0 == net) {
                    Some(t) => t.2.1,
                    None => continue,
                },
            };
            // The rows this net arrives at in the stage east of the gap.
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
                if sink_stage == gap + 1 {
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

/// Distinct sides for up to three input pins, minimising how far each net has
/// to travel vertically to reach its side (west: the gate's row; north: above; south: below).
fn assign_sides(from_rows: &[f64], gate_row: f64) -> Vec<Edge> {
    let sides = [Edge::West, Edge::North, Edge::South];
    let target = |e: Edge| match e {
        Edge::North => gate_row - 1.0,
        Edge::South => gate_row + 1.0,
        _ => gate_row,
    };
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
        let cost: f64 = choice.iter().zip(from_rows).map(|(&e, &r)| (r - target(e)).abs()).sum();
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
        assert_eq!(gap_count(&netlist).unwrap(), 3);
        let p = place(&netlist, &[2, 1, 3]).unwrap();
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
        // Every column is a cell wide and every row a cell deep.
        assert_eq!(p.col_width, vec![PITCH; p.cols as usize]);
        assert_eq!(p.row_depth, vec![PITCH; p.rows as usize]);
        // Widening the middle gap moves the later columns but keeps their names.
        let q = place(&netlist, &[2, 2, 3]).unwrap();
        assert_eq!(q.column(6), Column::Gates { index: 1, offset: 0 });
        assert_eq!(q.column(5), Column::Routing { gap: 1, offset: 1 });
        assert!(place(&netlist, &[1, 1]).is_err());
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
        let p = place(&netlist, &[1, 1]).unwrap();
        let estimate = estimate_routing(&netlist, &p);
        assert_eq!(estimate, vec![2, 1]);
    }
}
