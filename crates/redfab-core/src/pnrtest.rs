//! Random netlists, to exercise placement and routing on shapes that no
//! hand-written design covers: wide fan-out, constants, inputs wired straight
//! to outputs, gates reading the same net twice, and flip-flops fed back from
//! the end of the logic.

use std::collections::BTreeMap;

use crate::build::{Built, build_netlist, simulate};
use crate::netlist::{Cell, Driver, NetId, Netlist, PortBit};
use crate::tiletest::Rng;

/// A netlist shaped like a ripple design: `wide` NOR2 gates read the inputs in
/// the first level, then a tail of `tail` levels of one NOT each, every one
/// reading the gate before it; the last NOT and the first level's other gates
/// drive the outputs. The tail is the run of near-empty gate columns a ripple
/// adder has (adder8's levels 15 to 19 hold 2, 3, 1, 2, 1 gates).
pub fn ripple_netlist(wide: usize, tail: usize) -> Netlist {
    let mut net_count = 0;
    let mut next = || {
        net_count += 1;
        net_count - 1
    };
    let inputs: Vec<PortBit> = (0..2 * wide).map(|i| PortBit { name: format!("i{i}"), net: next() }).collect();
    let mut cells = Vec::new();
    let mut outputs = Vec::new();
    for k in 0..wide {
        let output = next();
        cells.push(Cell {
            name: format!("n{k}"),
            label: format!("n{k}"),
            kind: "NOR2".to_string(),
            inputs: vec![inputs[2 * k].net, inputs[2 * k + 1].net],
            output,
            init: false,
        });
        if k > 0 {
            outputs.push(PortBit { name: format!("o{k}"), net: output });
        }
    }
    let mut last = cells[0].output;
    for t in 0..tail {
        let output = next();
        cells.push(Cell {
            name: format!("t{t}"),
            label: format!("t{t}"),
            kind: "NOT".to_string(),
            inputs: vec![last],
            output,
            init: false,
        });
        last = output;
    }
    outputs.push(PortBit { name: "o0".to_string(), net: last });
    Netlist { inputs, outputs, cells, net_count, constants: Vec::new() }
}

/// A random combinational netlist of `gates` NOT/NOR2/NOR3 cells and one to
/// six inputs.
pub fn random_netlist(rng: &mut Rng, gates: usize) -> Netlist {
    let inputs = 1 + rng.below(6);
    random_netlist_sized(rng, gates, inputs, 0)
}

/// A random netlist of `gates` NOT/NOR2/NOR3 cells, `inputs` inputs and
/// `flip_flops` flip-flops (with a clock input `clk` when there are any).
/// Every gate reads nets made before it, so there is no loop; what nothing
/// reads becomes an output. Flip-flops are clocked on the rising edge through
/// one inverter, as synthesis does, and their D inputs come from the end of
/// the logic, so their nets run back across every column.
pub fn random_netlist_sized(rng: &mut Rng, gates: usize, inputs: usize, flip_flops: usize) -> Netlist {
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
    let mut ports: Vec<PortBit> = Vec::new();
    let mut cells = Vec::new();
    let mut clock_nets = Vec::new();
    if flip_flops > 0 {
        let clk = next();
        ports.push(PortBit { name: "clk".to_string(), net: clk });
        let nclk = next();
        cells.push(Cell {
            name: "nclk".to_string(),
            label: "nclk".to_string(),
            kind: "NOT".to_string(),
            inputs: vec![clk],
            output: nclk,
            init: false,
        });
        clock_nets.push(nclk);
    }
    ports.extend((0..inputs.max(1)).map(|i| PortBit { name: format!("i{i}"), net: next() }));
    let inputs = ports;
    let q_nets: Vec<NetId> = (0..flip_flops).map(|_| next()).collect();

    // Nets a gate may read, in the order they were made; recent ones are favoured
    // so that the netlist gets deep as well as wide.
    let mut available: Vec<NetId> = constants
        .iter()
        .map(|c| c.0)
        .chain(inputs.iter().skip(usize::from(flip_flops > 0)).map(|p| p.net))
        .chain(q_nets.iter().copied())
        .collect();
    let first_gate = cells.len();
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
        cells.push(Cell {
            name: format!("g{g}"),
            label: format!("g{g}"),
            kind: kind.to_string(),
            inputs,
            output,
            init: false,
        });
        available.push(output);
    }
    // Flip-flops: D from one of the last nets made (or anything, now and then).
    for (f, &q) in q_nets.iter().enumerate() {
        let recent = available.len().min(6);
        let d = if rng.below(4) == 0 {
            available[rng.below(available.len())]
        } else {
            available[available.len() - 1 - rng.below(recent)]
        };
        cells.push(Cell {
            name: format!("ff{f}"),
            label: format!("ff{f}"),
            kind: "DFF".to_string(),
            inputs: vec![d, clock_nets[0]],
            output: q,
            init: rng.below(2) == 1,
        });
    }

    let mut outputs = Vec::new();
    let mut read: Vec<bool> = vec![false; net_count];
    for cell in &cells {
        for &n in &cell.inputs {
            read[n] = true;
        }
    }
    for cell in &cells[first_gate..] {
        if !read[cell.output] {
            outputs.push(cell.output);
        }
    }
    // Sometimes an input or a constant goes straight to an output, or a net is
    // an output as well as read by gates.
    let data_inputs = &inputs[usize::from(flip_flops > 0)..];
    if rng.below(4) == 0 {
        outputs.push(data_inputs[rng.below(data_inputs.len())].net);
    }
    if !constants.is_empty() && rng.below(4) == 0 {
        outputs.push(constants[rng.below(constants.len())].0);
    }
    if cells.len() > first_gate && rng.below(2) == 0 {
        outputs.push(cells[first_gate + rng.below(cells.len() - first_gate)].output);
    }
    if outputs.is_empty() {
        outputs.push(data_inputs[0].net);
    }
    let outputs = outputs.into_iter().enumerate().map(|(o, net)| PortBit { name: format!("o{o}"), net }).collect();
    Netlist { inputs, outputs, cells, net_count, constants }
}

/// The outputs of a combinational netlist for one input vector, by evaluating the gates.
pub fn evaluate(netlist: &Netlist, inputs: &[bool]) -> Vec<bool> {
    let state: Vec<bool> = netlist.cells.iter().filter(|c| c.is_dff()).map(|c| c.init).collect();
    evaluate_with_state(netlist, inputs, &state).0
}

/// The outputs of a netlist for one input vector with the flip-flops holding
/// `state` (in cell order), and what the flip-flops would capture next.
pub fn evaluate_with_state(netlist: &Netlist, inputs: &[bool], state: &[bool]) -> (Vec<bool>, Vec<bool>) {
    let drivers = netlist.drivers().expect("a generated netlist has one driver per net");
    let mut value: Vec<Option<bool>> = vec![None; netlist.net_count];
    let dff_index: BTreeMap<usize, usize> =
        netlist.cells.iter().enumerate().filter(|(_, c)| c.is_dff()).map(|(c, _)| c).zip(0..).collect();
    fn net_value(
        net: NetId,
        netlist: &Netlist,
        drivers: &[Driver],
        inputs: &[bool],
        state: &[bool],
        dff_index: &BTreeMap<usize, usize>,
        value: &mut [Option<bool>],
    ) -> bool {
        if let Some(v) = value[net] {
            return v;
        }
        let v = match drivers[net] {
            Driver::Const(v) => v,
            Driver::Input(i) => inputs[i],
            Driver::Cell(c) if netlist.cells[c].is_dff() => state[dff_index[&c]],
            Driver::Cell(c) => !netlist.cells[c]
                .inputs
                .iter()
                .any(|&n| net_value(n, netlist, drivers, inputs, state, dff_index, value)),
        };
        value[net] = Some(v);
        v
    }
    let outputs = netlist
        .outputs
        .iter()
        .map(|p| net_value(p.net, netlist, &drivers, inputs, state, &dff_index, &mut value))
        .collect();
    let next = netlist
        .cells
        .iter()
        .filter(|c| c.is_dff())
        .map(|c| net_value(c.inputs[0], netlist, &drivers, inputs, state, &dff_index, &mut value))
        .collect();
    (outputs, next)
}

/// How much of a build [`build_and_verify`] holds to account. The kept layout
/// is always checked and simulated; `Every` also refuses a build in which any
/// other candidate laid out failed its checks or any placement failed
/// ([`Built::rejected`]), so that a check asserts every layout it pays for,
/// not only the one the pick took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strict {
    Kept,
    Every,
}

/// Builds the netlist and simulates it on `vectors` input vectors (every
/// combination when there are no more than that), comparing with [`evaluate`].
/// A clocked netlist runs one vector per clock cycle, as the harness drives the
/// game: the edge first, then the inputs change, then the outputs are read.
/// Returns the build report on success.
pub fn build_and_verify(
    netlist: &Netlist,
    name: &str,
    vectors: usize,
    rng: &mut Rng,
    strict: Strict,
) -> Result<Built, String> {
    let built = build_netlist(netlist, name)?;
    verify_built(netlist, name, &built, vectors, rng, strict)?;
    Ok(built)
}

/// The checks [`build_and_verify`] makes of a finished build: under
/// [`Strict::Every`], that no laid-out candidate was rejected; then the
/// simulator against the netlist's own evaluation on `vectors` input vectors.
pub fn verify_built(
    netlist: &Netlist,
    name: &str,
    built: &Built,
    vectors: usize,
    rng: &mut Rng,
    strict: Strict,
) -> Result<(), String> {
    if strict == Strict::Every && !built.rejected.is_empty() {
        return Err(format!(
            "{name}: {} of the layouts laid out could not have been kept:\n  - {}",
            built.rejected.len(),
            built.rejected.join("\n  - ")
        ));
    }
    let clocked = netlist.cells.iter().any(|c| c.is_dff());
    let data: Vec<usize> = (0..netlist.inputs.len()).filter(|&i| !(clocked && i == 0)).collect();
    let n = data.len();
    let all: Vec<Vec<bool>> = if n <= 16 && (1usize << n) <= vectors && !clocked {
        (0..1u32 << n).map(|m| (0..n).map(|i| m >> i & 1 == 1).collect()).collect()
    } else {
        (0..vectors).map(|_| (0..n).map(|_| rng.below(2) == 1).collect()).collect()
    };
    let vectors_json: Vec<BTreeMap<String, u8>> = all
        .iter()
        .map(|v| data.iter().zip(v).map(|(&i, &b)| (netlist.inputs[i].name.clone(), u8::from(b))).collect())
        .collect();
    let vectors_json = serde_json::to_string(&vectors_json).map_err(|e| e.to_string())?;
    let got = simulate(&built.design.to_json(), &vectors_json)?;
    let got: Vec<BTreeMap<String, u8>> = serde_json::from_str(&got).map_err(|e| e.to_string())?;
    let full = |v: &[bool]| -> Vec<bool> {
        let mut inputs = vec![false; netlist.inputs.len()];
        for (&i, &b) in data.iter().zip(v) {
            inputs[i] = b;
        }
        inputs
    };
    let mut state: Vec<bool> = netlist.cells.iter().filter(|c| c.is_dff()).map(|c| c.init).collect();
    let mut previous = vec![false; netlist.inputs.len()];
    for (cycle, (vector, outputs)) in all.iter().zip(&got).enumerate() {
        let inputs = full(vector);
        let want = if clocked {
            // The edge captures what the previous inputs produced, then the inputs change.
            state = evaluate_with_state(netlist, &previous, &state).1;
            previous = inputs.clone();
            evaluate_with_state(netlist, &inputs, &state).0
        } else {
            evaluate(netlist, &inputs)
        };
        for (port, &w) in netlist.outputs.iter().zip(&want) {
            if outputs[&port.name] != u8::from(w) {
                return Err(format!(
                    "{name}: {} inputs {} the simulated {} is {} but the netlist says {}",
                    if clocked { format!("in cycle {cycle} with") } else { "with".to_string() },
                    vector.iter().map(|&b| u8::from(b).to_string()).collect::<Vec<_>>().join(""),
                    port.name,
                    outputs[&port.name],
                    u8::from(w)
                ));
            }
        }
    }
    Ok(())
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
        let mut netnames = serde_json::Map::new();
        for c in &self.cells {
            let mut connections = serde_json::Map::new();
            let pins: &[&str] = if c.is_dff() { &["D", "CLK"] } else { &["A", "B", "C"] };
            for (pin, &net) in pins.iter().zip(&c.inputs) {
                connections.insert(pin.to_string(), serde_json::json!([bit(net)]));
            }
            connections.insert(if c.is_dff() { "Q" } else { "Y" }.to_string(), serde_json::json!([bit(c.output)]));
            cells.insert(c.name.clone(), serde_json::json!({"type": c.kind, "connections": connections}));
            if c.is_dff() {
                netnames.insert(
                    c.label.clone(),
                    serde_json::json!({"hide_name": 0, "bits": [bit(c.output)], "attributes": {"init": if c.init { "1" } else { "0" }}}),
                );
            }
        }
        serde_json::json!({"modules": {top: {"ports": ports, "cells": cells, "netnames": netnames}}}).to_string()
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
                Cell {
                    name: "n1".into(),
                    label: "n1".into(),
                    kind: "NOT".into(),
                    inputs: vec![0],
                    output: 1,
                    init: false,
                },
                Cell {
                    name: "n2".into(),
                    label: "n2".into(),
                    kind: "NOR2".into(),
                    inputs: vec![0, 0],
                    output: 2,
                    init: false,
                },
                Cell {
                    name: "y".into(),
                    label: "y".into(),
                    kind: "NOR2".into(),
                    inputs: vec![1, 0],
                    output: 3,
                    init: false,
                },
                Cell {
                    name: "z".into(),
                    label: "z".into(),
                    kind: "NOR3".into(),
                    inputs: vec![2, 1, 0],
                    output: 4,
                    init: false,
                },
            ],
            net_count: 5,
            constants: vec![],
        };
        let mut rng = Rng::new(1);
        build_and_verify(&netlist, "twins", 16, &mut rng, Strict::Every).unwrap();
    }

    /// The strict check refuses a build with a rejected layout, naming it; the
    /// plain one keeps to the kept layout, as the build itself does.
    #[test]
    fn a_rejected_layout_fails_the_strict_check_and_passes_the_plain_one() {
        let mut rng = Rng::new(5);
        let netlist = random_netlist(&mut rng, 4);
        let mut built = build_netlist(&netlist, "small").unwrap();
        assert!(built.rejected.is_empty(), "{:?}", built.rejected);
        verify_built(&netlist, "small", &built, 16, &mut rng, Strict::Every).unwrap();
        built.rejected.push("stretched, seed 1: 1 problems: lamp y reads net 3, the netlist says 4".into());
        let err = verify_built(&netlist, "small", &built, 16, &mut rng, Strict::Every).unwrap_err();
        assert!(err.contains("small: 1 of the layouts laid out could not have been kept"), "{err}");
        assert!(err.contains("  - stretched, seed 1: 1 problems: lamp y reads net 3"), "{err}");
        verify_built(&netlist, "small", &built, 16, &mut rng, Strict::Kept).unwrap();
    }

    /// Everything a build lays out and cannot keep is in `rejected`, as the log
    /// tells it: a placement that failed, or a layout whose checks found problems.
    #[test]
    fn the_rejected_layouts_are_the_ones_the_log_names() {
        for seed in 0..8 {
            let mut rng = Rng::new(900 + seed);
            let (gates, inputs) = (2 + rng.below(7), 1 + rng.below(4));
            let netlist = random_netlist_sized(&mut rng, gates, inputs, 0);
            let mut lines: Vec<String> = Vec::new();
            let (built, problems) =
                crate::build::build_unchecked_logged(&netlist, "logged", crate::build::Break::None, &mut |l| {
                    lines.push(l.to_string())
                })
                .unwrap();
            assert!(problems.is_empty(), "{problems:?}");
            let failed = lines.iter().filter(|l| l.contains(" placement failed: ")).count();
            let faulty = lines.iter().filter(|l| l.starts_with("  ") && l.contains(" problems")).count();
            assert_eq!(
                built.rejected.len(),
                failed + faulty,
                "seed {seed}: {:?}\n{}",
                built.rejected,
                lines.join("\n")
            );
            for r in &built.rejected {
                assert!(r.contains("placement failed: ") || r.contains(" problems: "), "{r}");
            }
        }
    }

    #[test]
    fn clocked_random_netlists_build_and_cycle_like_the_netlist() {
        for seed in 0..6 {
            let mut rng = Rng::new(100 + seed);
            let (gates, inputs, flip_flops) = (2 + rng.below(6), 1 + rng.below(3), 1 + rng.below(3));
            let netlist = random_netlist_sized(&mut rng, gates, inputs, flip_flops);
            assert!(netlist.cells.iter().any(|c| c.is_dff()));
            build_and_verify(&netlist, &format!("clocked{seed}"), 12, &mut rng, Strict::Every).unwrap();
            // The Yosys JSON form keeps the flip-flops, their clock and their initial values.
            let library = Library::builtin().unwrap();
            let back = Netlist::from_yosys_json(&netlist.to_yosys_json("top"), "top", &library).unwrap();
            let inits = |n: &Netlist| {
                n.cells.iter().filter(|c| c.is_dff()).map(|c| (c.label.clone(), c.init)).collect::<Vec<_>>()
            };
            assert_eq!(inits(&back), inits(&netlist));
        }
    }

    #[test]
    fn a_wide_random_netlist_builds() {
        // More inputs than any shipped design, in one gap in front of the gates.
        let mut rng = Rng::new(42);
        let netlist = random_netlist_sized(&mut rng, 24, 12, 0);
        assert_eq!(netlist.inputs.len(), 12);
        build_and_verify(&netlist, "wide", 16, &mut rng, Strict::Every).unwrap();
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
