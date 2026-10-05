//! Placement. For now only the simplest shape of design: one gate fed directly
//! by the design's inputs and driving one output. Real placement and routing
//! arrive in a later stage and replace this.

use crate::layout::{Layout, Placed};
use crate::netlist::{Driver, Netlist};
use crate::tile::Edge;

/// Lays out a single-gate design:
///
/// ```text
///            col 0     col 1        col 2
/// row 0      input  -> turn (W,S)
/// row 1      input  -> gate      -> output
/// row 2      input  -> turn (W,N)
/// ```
///
/// The gate's first input pin enters from the west, the second from the north,
/// the third from the south. With one input everything sits on a single row.
pub fn place_single_gate(netlist: &Netlist) -> Result<Layout, String> {
    const LATER: &str = "this design needs placement and routing, which redfab does not have yet";
    let [cell] = netlist.cells.as_slice() else {
        return Err(format!("{LATER} (it has {} cells; only single-gate designs work so far)", netlist.cells.len()));
    };
    let [output] = netlist.outputs.as_slice() else {
        return Err(format!("{LATER} (it has {} outputs)", netlist.outputs.len()));
    };
    if output.net != cell.output {
        return Err(format!("{LATER} (the output is not driven by the gate)"));
    }
    let drivers = netlist.drivers()?;
    let mut pin_ports = Vec::new();
    for &net in &cell.inputs {
        match drivers[net] {
            Driver::Input(port) if !pin_ports.contains(&port) => pin_ports.push(port),
            _ => {
                return Err(format!("{LATER} (a gate input is not its own design input)"));
            }
        }
    }
    if pin_ports.len() != netlist.inputs.len() {
        return Err(format!("{LATER} (an input is unused)"));
    }

    let mut layout = Layout::default();
    let gate_row = if pin_ports.len() >= 2 { 1 } else { 0 };
    let pin_edges: Vec<Edge> = [Edge::West, Edge::North, Edge::South][..pin_ports.len()].to_vec();
    for (pin, (&port, edge)) in pin_ports.iter().zip(&pin_edges).enumerate() {
        let row = match edge {
            Edge::West => gate_row,
            Edge::North => gate_row - 1,
            _ => gate_row + 1,
        };
        layout.tiles.insert((0, row), Placed::Input { port });
        if pin > 0 {
            // Turn from the input's row into the gate's north or south edge.
            layout.tiles.insert((1, row), Placed::Wire { edges: vec![Edge::West, edge.opposite()] });
        }
    }
    layout.tiles.insert((1, gate_row), Placed::Gate { cell: 0, pin_edges });
    layout.tiles.insert((2, gate_row), Placed::Output { port: 0 });
    Ok(layout)
}
