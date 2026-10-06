//! Layout versus netlist: does the circuit read back from the blocks have
//! exactly the cells and connections the synthesized netlist asks for?
//!
//! Dust networks joined by repeaters count as one net (a repeater is wire with
//! a delay). Matching starts from the design inputs and constants, whose
//! positions are known, and works forward: a torch whose mounting block is
//! powered by exactly the nets of a cell's inputs is that cell, and the net it
//! drives is the cell's output.

use std::collections::{BTreeMap, BTreeSet};

use crate::extract::{Circuit, Component};
use crate::geom::Pos;
use crate::netlist::{Cell, Netlist};

/// Extracted nets grouped into logical nets, with everything resolved to the
/// group number.
struct Logical {
    /// Extracted net -> logical net.
    group: Vec<usize>,
    count: usize,
}

impl Logical {
    fn of(&self, net: usize) -> usize {
        self.group[net]
    }

    fn mapped(&self, nets: &[usize]) -> Vec<usize> {
        let mut out: Vec<usize> = nets.iter().map(|&n| self.group[n]).collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Returns one message per mismatch; an empty list means the layout is correct.
pub fn lvs(
    netlist: &Netlist,
    circuit: &Circuit,
    probes: &[Pos],
    lamps: &[Pos],
    ties: &[(usize, bool, Pos)],
) -> Vec<String> {
    let mut problems = Vec::new();
    let first = |net: usize| circuit.nets[net].first().map_or_else(|| "?".to_string(), Pos::to_string);

    for (from, to) in &circuit.one_way {
        problems.push(format!("power passes from the wire at {from} to the wire at {to} but not back"));
    }
    for component in circuit.torches.iter().chain(&circuit.repeaters).chain(&circuit.sources) {
        for other in &component.direct {
            problems.push(format!(
                "the block at {} powers the component at {other} directly, with no wire between them",
                component.pos
            ));
        }
    }

    // Repeaters join nets. Each must read one net and drive one net.
    let mut group: Vec<usize> = (0..circuit.nets.len()).collect();
    fn find(group: &mut [usize], mut n: usize) -> usize {
        while group[n] != n {
            group[n] = group[group[n]];
            n = group[n];
        }
        n
    }
    for repeater in &circuit.repeaters {
        match (repeater.reads.as_slice(), repeater.drives.as_slice()) {
            ([input], [output]) => {
                let (a, b) = (find(&mut group, *input), find(&mut group, *output));
                group[a.max(b)] = a.min(b);
            }
            ([], _) => problems.push(format!("the repeater at {} has nothing at its input", repeater.pos)),
            (_, []) => problems.push(format!("the repeater at {} drives no wire", repeater.pos)),
            _ => problems.push(format!("the repeater at {} touches more than one net on a side", repeater.pos)),
        }
    }
    let roots: Vec<usize> = (0..circuit.nets.len()).map(|n| find(&mut group, n)).collect();
    let mut numbering: BTreeMap<usize, usize> = BTreeMap::new();
    for &root in &roots {
        let next = numbering.len();
        numbering.entry(root).or_insert(next);
    }
    let logical = Logical { group: roots.iter().map(|r| numbering[r]).collect(), count: numbering.len() };
    let wires_of = |l: usize| -> String {
        (0..circuit.nets.len()).find(|&n| logical.of(n) == l).map_or_else(|| "?".to_string(), first)
    };

    // Every logical net needs exactly one driver, except a constant 0, which has none.
    let mut drivers: Vec<Vec<Pos>> = vec![Vec::new(); logical.count];
    for component in circuit.torches.iter().chain(&circuit.sources) {
        for net in logical.mapped(&component.drives) {
            drivers[net].push(component.pos);
        }
    }
    let zero_anchors: BTreeSet<usize> =
        ties.iter().filter(|t| !t.1).filter_map(|t| circuit.net_of(t.2)).map(|n| logical.of(n)).collect();
    for (net, list) in drivers.iter().enumerate() {
        match list.as_slice() {
            [] if zero_anchors.contains(&net) => {}
            [] => problems.push(format!("the wires at {} are connected to no driver", wires_of(net))),
            [_] if zero_anchors.contains(&net) => {
                problems.push(format!("the constant-0 wires at {} are driven from {}", wires_of(net), list[0]))
            }
            [_] => {}
            many => {
                let places: Vec<String> = many.iter().map(Pos::to_string).collect();
                problems.push(format!(
                    "the wires at {} are driven from {} places ({}): two nets are shorted",
                    wires_of(net),
                    many.len(),
                    places.join(" and ")
                ));
            }
        }
    }

    // Through repeaters, power must flow forward from the driver to every part of the net.
    let mut reached: BTreeSet<usize> = BTreeSet::new();
    for component in circuit.torches.iter().chain(&circuit.sources) {
        reached.extend(component.drives.iter().copied());
    }
    for &(_, value, anchor) in ties {
        if !value && let Some(net) = circuit.net_of(anchor) {
            reached.insert(net);
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for repeater in &circuit.repeaters {
            if let ([input], [output]) = (repeater.reads.as_slice(), repeater.drives.as_slice())
                && reached.contains(input)
                && reached.insert(*output)
            {
                changed = true;
            }
        }
    }
    for net in 0..circuit.nets.len() {
        if !reached.contains(&net) && drivers[logical.of(net)].len() == 1 {
            problems.push(format!(
                "no power reaches the wires at {}: a repeater on the way points the wrong way",
                first(net)
            ));
        }
    }

    // Inputs and constants anchor the matching.
    let mut net_map: Vec<Option<usize>> = vec![None; netlist.net_count];
    for (i, port) in netlist.inputs.iter().enumerate() {
        let driven = circuit.sources.iter().find(|s| s.pos == probes[i]).map(|s| logical.mapped(&s.drives));
        match driven.as_deref() {
            Some([net]) => net_map[port.net] = Some(*net),
            // An input nothing reads has no wire to drive.
            Some([]) | None if netlist.sinks_of(port.net).is_empty() => {}
            Some([]) | None => problems.push(format!("input {} at {} drives no wire", port.name, probes[i])),
            Some(_) => problems.push(format!("input {} at {} drives more than one net", port.name, probes[i])),
        }
    }
    for source in circuit.sources.iter().filter(|s| !probes.contains(&s.pos)) {
        problems.push(format!("there is a redstone block at {} that is not a design input", source.pos));
    }
    let mut tie_torches: Vec<Pos> = Vec::new();
    for &(net, value, anchor) in ties {
        if value {
            match circuit.torches.iter().find(|t| t.pos == anchor) {
                Some(torch) if torch.reads.is_empty() && torch.drives.len() == 1 => {
                    net_map[net] = Some(logical.of(torch.drives[0]));
                    tie_torches.push(anchor);
                }
                Some(_) => problems
                    .push(format!("the constant-1 torch at {anchor} is not a free-running torch driving one net")),
                None => problems.push(format!("constant 1: there is no torch at {anchor}")),
            }
        } else {
            match circuit.net_of(anchor) {
                Some(extracted) => net_map[net] = Some(logical.of(extracted)),
                None => problems.push(format!("constant 0: there is no wire at {anchor}")),
            }
        }
    }

    // Cells: match forward from mapped nets. Cells with the same inputs compute
    // the same thing, so their torches cannot be told apart and need not be: they
    // are matched as a group, and their output nets become one class that reads
    // downstream treat as a single net.
    let mut class: Vec<usize> = (0..logical.count).collect();
    fn canon(class: &mut [usize], nets: &[usize]) -> Vec<usize> {
        let mut out: Vec<usize> = nets.iter().map(|&n| find(class, n)).collect();
        out.sort_unstable();
        out.dedup();
        out
    }
    let mut torch_of_cell: Vec<Option<usize>> = vec![None; netlist.cells.len()];
    let mut used_torch: Vec<bool> = circuit.torches.iter().map(|t| tie_torches.contains(&t.pos)).collect();
    loop {
        let mut progress = false;
        for c in 0..netlist.cells.len() {
            if torch_of_cell[c].is_some() {
                continue;
            }
            let mapped_inputs = |cell: &Cell, net_map: &[Option<usize>]| -> Option<Vec<usize>> {
                cell.inputs.iter().map(|&n| net_map[n]).collect()
            };
            let Some(inputs) = mapped_inputs(&netlist.cells[c], &net_map) else {
                continue;
            };
            let want = canon(&mut class, &inputs);
            let peers: Vec<usize> = (0..netlist.cells.len())
                .filter(|&p| {
                    torch_of_cell[p].is_none()
                        && mapped_inputs(&netlist.cells[p], &net_map).is_some_and(|i| canon(&mut class, &i) == want)
                })
                .collect();
            let found: Vec<usize> = (0..circuit.torches.len())
                .filter(|&t| !used_torch[t] && canon(&mut class, &logical.mapped(&circuit.torches[t].reads)) == want)
                .collect();
            if found.len() != peers.len() {
                continue; // reported below, cell by cell and torch by torch
            }
            let mut outputs = Vec::new();
            for (&p, &t) in peers.iter().zip(&found) {
                let cell = &netlist.cells[p];
                torch_of_cell[p] = Some(t);
                used_torch[t] = true;
                match logical.mapped(&circuit.torches[t].drives).as_slice() {
                    [net] => {
                        net_map[cell.output] = Some(*net);
                        outputs.push(*net);
                    }
                    [] => problems
                        .push(format!("the torch at {} (cell {}) drives no wire", circuit.torches[t].pos, cell.name)),
                    _ => problems.push(format!(
                        "the torch at {} (cell {}) drives more than one net",
                        circuit.torches[t].pos, cell.name
                    )),
                }
            }
            for pair in outputs.windows(2) {
                let (a, b) = (find(&mut class, pair[0]), find(&mut class, pair[1]));
                class[a] = b;
            }
            progress = true;
        }
        if !progress {
            break;
        }
    }
    for (c, cell) in netlist.cells.iter().enumerate() {
        if torch_of_cell[c].is_none() {
            // Name the wanted nets by one position each, so the layout can be looked at.
            let wanted: Vec<String> = cell
                .inputs
                .iter()
                .map(|&n| match net_map[n] {
                    Some(l) => {
                        let extracted = (0..circuit.nets.len()).find(|&e| logical.of(e) == l).unwrap_or(0);
                        format!("the net at {}", first(extracted))
                    }
                    None => "a net the layout does not have".to_string(),
                })
                .collect();
            problems.push(format!(
                "no torch implements cell {} ({}): none is powered by exactly its input nets ({})",
                cell.name,
                cell.kind,
                wanted.join(", ")
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
            problems.push(format!("output {}: there is no lamp at {}", port.name, lamps[j]));
            continue;
        };
        match (net_map[port.net], logical.mapped(&lamp.reads).as_slice()) {
            (Some(want), [got]) if find(&mut class, want) == find(&mut class, *got) => {}
            (_, []) if net_map[port.net].is_some_and(|n| zero_anchors.contains(&n)) => {
                // A constant-0 output: the lamp must simply be unpowered by anything.
            }
            (_, []) => problems.push(format!("output {}: nothing powers the lamp at {}", port.name, lamp.pos)),
            _ => problems
                .push(format!("output {}: the lamp at {} is not powered by exactly its own net", port.name, lamp.pos)),
        }
    }
    for lamp in circuit.lamps.iter().filter(|l| !lamps.contains(&l.pos)) {
        problems.push(format!("there is a lamp at {} that is not a design output", lamp.pos));
    }

    // Nothing left over.
    for (net, list) in drivers.iter().enumerate().take(logical.count) {
        if !net_map.contains(&Some(net)) && list.len() == 1 {
            problems.push(format!("the wires at {} belong to no net of the design", wires_of(net)));
        }
    }
    problems
}

fn describe_reads(torch: &Component, circuit: &Circuit) -> String {
    if torch.reads.is_empty() {
        return "nothing powers it".to_string();
    }
    let nets: Vec<String> =
        torch.reads.iter().map(|&n| circuit.nets[n].first().map_or_else(|| "?".to_string(), Pos::to_string)).collect();
    format!("powered by the nets at {}", nets.join(", "))
}
