//! Random combinational netlists, to exercise placement and routing on shapes
//! that no hand-written design covers: wide fan-out, constants, inputs wired
//! straight to outputs, gates reading the same net twice.

use std::collections::BTreeMap;

use crate::build::{Built, build_netlist, simulate};
use crate::netlist::{Cell, Driver, NetId, Netlist, PortBit};
use crate::tiletest::Rng;

/// A random netlist of `gates` NOT/NOR2/NOR3 cells. Every gate reads nets made
/// before it, so there is no loop; what nothing reads becomes an output.
pub fn random_netlist(rng: &mut Rng, gates: usize) -> Netlist {
    let mut net_count = 0;
    let mut next = || {
        net_count += 1;
        net_count - 1
    };
    let mut constants = Vec::new();
    for value in [false, true] {
        if rng.below(3) == 0 {
            constants.push((next(), value));
        }
    }
    let inputs: Vec<PortBit> = (0..1 + rng.below(6)).map(|i| PortBit { name: format!("i{i}"), net: next() }).collect();

    // Nets a gate may read, in the order they were made; recent ones are favoured
    // so that the netlist gets deep as well as wide.
    let mut available: Vec<NetId> = constants.iter().map(|c| c.0).chain(inputs.iter().map(|p| p.net)).collect();
    let mut cells = Vec::new();
    for g in 0..gates {
        let arity = 1 + rng.below(3);
        let kind = ["NOT", "NOR2", "NOR3"][arity - 1];
        let pick = |rng: &mut Rng, available: &[NetId]| -> NetId {
            let recent = available.len().min(4);
            if rng.below(2) == 0 {
                available[available.len() - 1 - rng.below(recent)]
            } else {
                available[rng.below(available.len())]
            }
        };
        let inputs: Vec<NetId> = (0..arity).map(|_| pick(rng, &available)).collect();
        let output = next();
        cells.push(Cell { name: format!("g{g}"), kind: kind.to_string(), inputs, output, init: false });
        available.push(output);
    }

    let mut outputs = Vec::new();
    let mut read: Vec<bool> = vec![false; net_count];
    for cell in &cells {
        for &n in &cell.inputs {
            read[n] = true;
        }
    }
    for cell in &cells {
        if !read[cell.output] {
            outputs.push(cell.output);
        }
    }
    // Sometimes an input or a constant goes straight to an output, or a net is
    // an output as well as read by gates.
    if rng.below(4) == 0 {
        outputs.push(inputs[rng.below(inputs.len())].net);
    }
    if !constants.is_empty() && rng.below(4) == 0 {
        outputs.push(constants[rng.below(constants.len())].0);
    }
    if !cells.is_empty() && rng.below(2) == 0 {
        outputs.push(cells[rng.below(cells.len())].output);
    }
    if outputs.is_empty() {
        outputs.push(inputs[0].net);
    }
    let outputs = outputs.into_iter().enumerate().map(|(o, net)| PortBit { name: format!("o{o}"), net }).collect();
    Netlist { inputs, outputs, cells, net_count, constants }
}

/// The outputs of a netlist for one input vector, by evaluating the gates.
pub fn evaluate(netlist: &Netlist, inputs: &[bool]) -> Vec<bool> {
    let drivers = netlist.drivers().expect("a generated netlist has one driver per net");
    let mut value: Vec<Option<bool>> = vec![None; netlist.net_count];
    fn net_value(
        net: NetId,
        netlist: &Netlist,
        drivers: &[Driver],
        inputs: &[bool],
        value: &mut [Option<bool>],
    ) -> bool {
        if let Some(v) = value[net] {
            return v;
        }
        let v = match drivers[net] {
            Driver::Const(v) => v,
            Driver::Input(i) => inputs[i],
            Driver::Cell(c) => !netlist.cells[c].inputs.iter().any(|&n| net_value(n, netlist, drivers, inputs, value)),
        };
        value[net] = Some(v);
        v
    }
    netlist.outputs.iter().map(|p| net_value(p.net, netlist, &drivers, inputs, &mut value)).collect()
}

/// Builds the netlist and simulates it on `vectors` input vectors (every
/// combination when there are no more than that), comparing with [`evaluate`].
/// Returns the build report on success.
pub fn build_and_verify(netlist: &Netlist, name: &str, vectors: usize, rng: &mut Rng) -> Result<Built, String> {
    let built = build_netlist(netlist, name)?;
    let n = netlist.inputs.len();
    let all: Vec<Vec<bool>> = if n <= 16 && (1usize << n) <= vectors {
        (0..1u32 << n).map(|m| (0..n).map(|i| m >> i & 1 == 1).collect()).collect()
    } else {
        (0..vectors).map(|_| (0..n).map(|_| rng.below(2) == 1).collect()).collect()
    };
    let vectors_json: Vec<BTreeMap<String, u8>> = all
        .iter()
        .map(|v| netlist.inputs.iter().zip(v).map(|(p, &b)| (p.name.clone(), u8::from(b))).collect())
        .collect();
    let vectors_json = serde_json::to_string(&vectors_json).map_err(|e| e.to_string())?;
    let got = simulate(&built.design.to_json(), &vectors_json)?;
    let got: Vec<BTreeMap<String, u8>> = serde_json::from_str(&got).map_err(|e| e.to_string())?;
    for (vector, outputs) in all.iter().zip(&got) {
        let want = evaluate(netlist, vector);
        for (port, &w) in netlist.outputs.iter().zip(&want) {
            if outputs[&port.name] != u8::from(w) {
                return Err(format!(
                    "{name}: with inputs {} the simulated {} is {} but the netlist says {}",
                    vector.iter().map(|&b| u8::from(b).to_string()).collect::<Vec<_>>().join(""),
                    port.name,
                    outputs[&port.name],
                    u8::from(w)
                ));
            }
        }
    }
    Ok(built)
}

impl Netlist {
    /// The netlist as Yosys would write it, so a failing random one can be
    /// built and drawn with the ordinary commands.
    pub fn to_yosys_json(&self, top: &str) -> String {
        let bit = |net: NetId| -> serde_json::Value {
            match self.constants.iter().find(|c| c.0 == net) {
                Some(&(_, value)) => serde_json::Value::String(u8::from(value).to_string()),
                None => serde_json::json!(net + 2),
            }
        };
        let mut ports = serde_json::Map::new();
        for (list, direction) in [(&self.inputs, "input"), (&self.outputs, "output")] {
            for p in list {
                ports.insert(p.name.clone(), serde_json::json!({"direction": direction, "bits": [bit(p.net)]}));
            }
        }
        let mut cells = serde_json::Map::new();
        for c in &self.cells {
            let mut connections = serde_json::Map::new();
            for (pin, &net) in ["A", "B", "C"].iter().zip(&c.inputs) {
                connections.insert(pin.to_string(), serde_json::json!([bit(net)]));
            }
            connections.insert("Y".to_string(), serde_json::json!([bit(c.output)]));
            cells.insert(c.name.clone(), serde_json::json!({"type": c.kind, "connections": connections}));
        }
        serde_json::json!({"modules": {top: {"ports": ports, "cells": cells}}}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::Library;

    #[test]
    fn random_netlists_round_trip_through_yosys_json() {
        let library = Library::builtin().unwrap();
        for seed in 0..20 {
            let mut rng = Rng::new(seed);
            let gates = 1 + rng.below(12);
            let netlist = random_netlist(&mut rng, gates);
            let back = Netlist::from_yosys_json(&netlist.to_yosys_json("top"), "top", &library).unwrap();
            assert_eq!(back.cells.len(), netlist.cells.len());
            assert_eq!(back.outputs.len(), netlist.outputs.len());
            // Yosys JSON orders ports by name, so compare by name.
            let by_name = |n: &Netlist, inputs: &[bool]| -> BTreeMap<String, bool> {
                n.outputs.iter().map(|p| p.name.clone()).zip(evaluate(n, inputs)).collect()
            };
            for _ in 0..8 {
                let inputs: Vec<bool> = (0..netlist.inputs.len()).map(|_| rng.below(2) == 1).collect();
                let back_inputs: Vec<bool> = back
                    .inputs
                    .iter()
                    .map(|p| inputs[netlist.inputs.iter().position(|q| q.name == p.name).unwrap()])
                    .collect();
                assert_eq!(by_name(&back, &back_inputs), by_name(&netlist, &inputs));
            }
        }
    }

    /// Two gates reading the same net compute the same thing; the checker must
    /// not care which torch is which.
    #[test]
    fn interchangeable_gates_build_and_pass_the_checks() {
        let netlist = Netlist {
            inputs: vec![PortBit { name: "a".into(), net: 0 }],
            outputs: vec![PortBit { name: "y".into(), net: 3 }, PortBit { name: "z".into(), net: 4 }],
            cells: vec![
                Cell { name: "n1".into(), kind: "NOT".into(), inputs: vec![0], output: 1, init: false },
                Cell { name: "n2".into(), kind: "NOR2".into(), inputs: vec![0, 0], output: 2, init: false },
                Cell { name: "y".into(), kind: "NOR2".into(), inputs: vec![1, 0], output: 3, init: false },
                Cell { name: "z".into(), kind: "NOR3".into(), inputs: vec![2, 1, 0], output: 4, init: false },
            ],
            net_count: 5,
            constants: vec![],
        };
        let mut rng = Rng::new(1);
        build_and_verify(&netlist, "twins", 16, &mut rng).unwrap();
    }

    #[test]
    fn evaluate_is_nor() {
        let mut rng = Rng::new(3);
        let netlist = random_netlist(&mut rng, 0);
        // No gates: outputs are inputs or constants passed through.
        for (o, p) in netlist.outputs.iter().enumerate() {
            match netlist.driver_of(p.net).unwrap() {
                Driver::Input(i) => {
                    let mut v = vec![false; netlist.inputs.len()];
                    v[i] = true;
                    assert!(evaluate(&netlist, &v)[o]);
                }
                Driver::Const(c) => assert_eq!(evaluate(&netlist, &vec![false; netlist.inputs.len()])[o], c),
                Driver::Cell(_) => unreachable!(),
            }
        }
    }
}
