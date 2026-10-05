//! Layout versus netlist: does the circuit read back from the blocks have
//! exactly the cells and connections the synthesized netlist asks for?
//!
//! Matching starts from the design inputs, whose positions are known, and works
//! forward: a torch whose mounting block is powered by exactly the nets of a
//! cell's inputs is that cell, and the net it drives is the cell's output.

use std::collections::BTreeMap;

use crate::extract::{Circuit, Component};
use crate::geom::Pos;
use crate::netlist::Netlist;

/// Returns one message per mismatch; an empty list means the layout is correct.
pub fn lvs(netlist: &Netlist, circuit: &Circuit, probes: &[Pos], lamps: &[Pos]) -> Vec<String> {
    let mut problems = Vec::new();
    let at = |net: usize| {
        circuit.nets[net]
            .first()
            .map_or_else(|| "?".to_string(), Pos::to_string)
    };

    for (from, to) in &circuit.one_way {
        problems.push(format!(
            "power passes from the wire at {from} to the wire at {to} but not back"
        ));
    }

    // Every net needs exactly one driver.
    let mut drivers: BTreeMap<usize, Vec<Pos>> = BTreeMap::new();
    for component in circuit.torches.iter().chain(&circuit.sources) {
        for &net in &component.drives {
            drivers.entry(net).or_default().push(component.pos);
        }
    }
    for net in 0..circuit.nets.len() {
        match drivers.get(&net).map_or(&[][..], Vec::as_slice) {
            [] => problems.push(format!(
                "the wires at {} are connected to no driver",
                at(net)
            )),
            [_] => {}
            many => {
                let list: Vec<String> = many.iter().map(Pos::to_string).collect();
                problems.push(format!(
                    "the wires at {} are driven from {} places ({}): two nets are shorted",
                    at(net),
                    many.len(),
                    list.join(" and ")
                ));
            }
        }
    }
    for component in circuit.torches.iter().chain(&circuit.sources) {
        for other in &component.direct {
            problems.push(format!("the block at {} powers the component at {other} directly, with no wire between them", component.pos));
        }
    }

    // Inputs anchor the matching.
    let mut net_map: Vec<Option<usize>> = vec![None; netlist.net_count];
    for (i, port) in netlist.inputs.iter().enumerate() {
        match circuit
            .sources
            .iter()
            .find(|s| s.pos == probes[i])
            .map(|s| s.drives.as_slice())
        {
            Some([net]) => net_map[port.net] = Some(*net),
            Some([]) | None => problems.push(format!(
                "input {} at {} drives no wire",
                port.name, probes[i]
            )),
            Some(_) => problems.push(format!(
                "input {} at {} drives more than one net",
                port.name, probes[i]
            )),
        }
    }
    for source in circuit.sources.iter().filter(|s| !probes.contains(&s.pos)) {
        problems.push(format!(
            "there is a redstone block at {} that is not a design input",
            source.pos
        ));
    }

    // Cells: match forward from mapped nets.
    let mut torch_of_cell: Vec<Option<usize>> = vec![None; netlist.cells.len()];
    let mut used_torch = vec![false; circuit.torches.len()];
    loop {
        let mut progress = false;
        for (c, cell) in netlist.cells.iter().enumerate() {
            if torch_of_cell[c].is_some() {
                continue;
            }
            let Some(mut want) = cell
                .inputs
                .iter()
                .map(|&n| net_map[n])
                .collect::<Option<Vec<usize>>>()
            else {
                continue;
            };
            want.sort_unstable();
            want.dedup();
            let found = circuit
                .torches
                .iter()
                .enumerate()
                .position(|(t, torch)| !used_torch[t] && sorted(&torch.reads) == want);
            if let Some(t) = found {
                torch_of_cell[c] = Some(t);
                used_torch[t] = true;
                progress = true;
                match circuit.torches[t].drives.as_slice() {
                    [net] => net_map[cell.output] = Some(*net),
                    [] => problems.push(format!(
                        "the torch at {} (cell {}) drives no wire",
                        circuit.torches[t].pos, cell.name
                    )),
                    _ => problems.push(format!(
                        "the torch at {} (cell {}) drives more than one net",
                        circuit.torches[t].pos, cell.name
                    )),
                }
            }
        }
        if !progress {
            break;
        }
    }
    for (c, cell) in netlist.cells.iter().enumerate() {
        if torch_of_cell[c].is_none() {
            problems.push(format!(
                "no torch implements cell {} ({}): none is powered by exactly its input nets",
                cell.name, cell.kind
            ));
        }
    }
    for (t, torch) in circuit.torches.iter().enumerate() {
        if !used_torch[t] {
            problems.push(format!(
                "the torch at {} is not in the netlist ({})",
                torch.pos,
                describe_reads(torch, circuit)
            ));
        }
    }

    // Outputs.
    for (j, port) in netlist.outputs.iter().enumerate() {
        let Some(lamp) = circuit.lamps.iter().find(|l| l.pos == lamps[j]) else {
            problems.push(format!(
                "output {}: there is no lamp at {}",
                port.name, lamps[j]
            ));
            continue;
        };
        match (net_map[port.net], lamp.reads.as_slice()) {
            (Some(want), [got]) if want == *got => {}
            (_, []) => problems.push(format!(
                "output {}: nothing powers the lamp at {}",
                port.name, lamp.pos
            )),
            _ => problems.push(format!(
                "output {}: the lamp at {} is not powered by exactly its own net",
                port.name, lamp.pos
            )),
        }
    }
    for lamp in circuit.lamps.iter().filter(|l| !lamps.contains(&l.pos)) {
        problems.push(format!(
            "there is a lamp at {} that is not a design output",
            lamp.pos
        ));
    }

    // Nothing left over.
    for net in 0..circuit.nets.len() {
        if !net_map.contains(&Some(net)) && drivers.get(&net).is_some_and(|d| d.len() == 1) {
            problems.push(format!(
                "the wires at {} belong to no net of the design",
                at(net)
            ));
        }
    }
    problems
}

fn sorted(values: &[usize]) -> Vec<usize> {
    let mut out = values.to_vec();
    out.sort_unstable();
    out.dedup();
    out
}

fn describe_reads(torch: &Component, circuit: &Circuit) -> String {
    if torch.reads.is_empty() {
        return "nothing powers it".to_string();
    }
    let nets: Vec<String> = torch
        .reads
        .iter()
        .map(|&n| {
            circuit.nets[n]
                .first()
                .map_or_else(|| "?".to_string(), Pos::to_string)
        })
        .collect();
    format!("powered by the nets at {}", nets.join(", "))
}
