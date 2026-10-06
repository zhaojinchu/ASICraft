//! Placement: which tile each gate, input, output and constant occupies.
//!
//! Gates go into columns by logic depth, with routing columns between. Within a
//! column, gates are ordered by the average row of what feeds them (a few
//! barycenter sweeps), which keeps most wires short and uncrossed. Each gate
//! takes four rows: the tile above it feeds its north input, the tile below
//! its south input, its west input comes straight in from the left, and the
//! fourth row is left free so that other nets can pass the gate column.

use std::collections::BTreeMap;

use crate::netlist::{Driver, NetId, Netlist};
use crate::tile::Edge;

/// A tile position: (column, row). Columns run east, rows run south.
pub type Cell = (i32, i32);

#[derive(Clone, Debug)]
pub struct Placement {
    pub cols: i32,
    pub rows: i32,
    /// Per netlist cell.
    pub gates: Vec<Cell>,
    /// Per netlist cell, per input pin: the edge the pin's net enters by.
    pub pin_edges: Vec<Vec<Edge>>,
    /// Per design input.
    pub inputs: Vec<Cell>,
    /// Per design output.
    pub outputs: Vec<Cell>,
    /// Constant nets that are used, with their value and tile.
    pub ties: Vec<(NetId, bool, Cell)>,
}

impl Placement {
    pub fn is_free(&self, cell: Cell) -> bool {
        cell.0 >= 0
            && cell.0 < self.cols
            && cell.1 >= 0
            && cell.1 < self.rows
            && !self.gates.contains(&cell)
            && !self.inputs.contains(&cell)
            && !self.outputs.contains(&cell)
            && !self.ties.iter().any(|t| t.2 == cell)
    }
}

/// Logic depth of every cell: 1 for a gate fed only by inputs and constants.
fn levels(netlist: &Netlist, drivers: &[Driver]) -> Vec<usize> {
    let mut level = vec![0usize; netlist.cells.len()];
    let mut changed = true;
    let mut rounds = 0;
    while changed {
        changed = false;
        rounds += 1;
        assert!(rounds <= netlist.cells.len() + 1, "combinational loop in the netlist");
        for (c, cell) in netlist.cells.iter().enumerate() {
            let depth = cell
                .inputs
                .iter()
                .map(|&n| match drivers[n] {
                    Driver::Cell(d) => level[d] + 1,
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
    level
}

/// Places the netlist with `routing_cols` routing columns between gate columns.
pub fn place(netlist: &Netlist, routing_cols: i32) -> Result<Placement, String> {
    let drivers = netlist.drivers()?;
    let level = levels(netlist, &drivers);
    let depth = level.iter().copied().max().unwrap_or(0);
    let used_constants: Vec<(NetId, bool)> =
        netlist.constants.iter().copied().filter(|(net, _)| !netlist.sinks_of(*net).is_empty()).collect();

    // Columns: sources, routing, gates, routing, ..., gates, routing, outputs.
    let gate_col = |l: usize| 1 + routing_cols + (l as i32 - 1) * (routing_cols + 1);
    let cols = if depth == 0 { 2 + routing_cols } else { gate_col(depth) + 1 + routing_cols + 1 };

    let mut by_level: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (c, &l) in level.iter().enumerate() {
        by_level.entry(l).or_default().push(c);
    }
    let gates_per_col = by_level.values().map(Vec::len).max().unwrap_or(0) as i32;
    let sources = (netlist.inputs.len() + used_constants.len()) as i32;
    // A net can only pass a gate column through a row that holds neither a gate nor
    // one of its feeds, so every column needs as many free rows as there are nets
    // crossing it: nets driven before the column with a sink after it.
    let sink_level = |net: NetId| -> usize {
        netlist
            .sinks_of(net)
            .iter()
            .map(|&(c, _)| if c == usize::MAX { depth + 1 } else { level[c] })
            .max()
            .unwrap_or(0)
    };
    let driver_level = |net: NetId| -> usize {
        match drivers[net] {
            Driver::Cell(d) => level[d],
            _ => 0,
        }
    };
    let crossing_rows = (1..=depth)
        .map(|l| {
            let crossing = (0..netlist.net_count).filter(|&n| driver_level(n) < l && sink_level(n) > l).count() as i32;
            3 * by_level.get(&l).map_or(0, Vec::len) as i32 + crossing
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
    for cells in by_level.values() {
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
        for cells in by_level.values() {
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
        // Outputs: by the row of their driver, keeping the order stable.
        let mut output_order: Vec<(f64, usize)> = netlist
            .outputs
            .iter()
            .enumerate()
            .map(|(o, p)| (driver_row(p.net, &row_of_cell, &row_of_input, &row_of_tie), o))
            .collect();
        output_order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
        for (row, &(_, o)) in output_order.iter().enumerate() {
            row_of_output[o] = row as i32;
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
        for (row, &(_, i, is_tie)) in order.iter().enumerate() {
            if is_tie {
                row_of_tie[i] = row as i32;
            } else {
                row_of_input[i] = row as i32;
            }
        }
    }
    let gates: Vec<Cell> = (0..netlist.cells.len()).map(|c| (gate_col(level[c]), row_of_cell[c])).collect();
    let inputs: Vec<Cell> = row_of_input.iter().map(|&r| (0, r)).collect();
    let ties: Vec<(NetId, bool, Cell)> =
        used_constants.iter().zip(&row_of_tie).map(|(&(net, value), &r)| (net, value, (0, r))).collect();
    let outputs: Vec<Cell> = row_of_output.iter().map(|&r| (cols - 1, r)).collect();

    // Pin edges: give each input net the side closest to where it comes from.
    let mut pin_edges = Vec::new();
    for (c, cell) in netlist.cells.iter().enumerate() {
        let row = f64::from(gates[c].1);
        let from: Vec<f64> =
            cell.inputs.iter().map(|&n| driver_row(n, &row_of_cell, &row_of_input, &row_of_tie)).collect();
        pin_edges.push(assign_sides(&from, row));
    }

    Ok(Placement { cols, rows, gates, pin_edges, inputs, outputs, ties })
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
