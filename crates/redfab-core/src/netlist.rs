//! The synthesized design: library cells joined by single-bit nets.
//! Read from the JSON netlist Yosys writes after mapping to the tile library.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::tile::Library;

pub type NetId = usize;

#[derive(Clone, Debug)]
pub struct PortBit {
    /// `name` for a one-bit port, `name[i]` for bit i of a wider one, with `i`
    /// the index the source declares (`[7:4]` gives `a[4]` to `a[7]`).
    pub name: String,
    pub net: NetId,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub name: String,
    /// How the user knows the cell: for a flip-flop the register bit its output
    /// is named after in the source (`count[2]`), otherwise the cell's name.
    pub label: String,
    /// Library cell name: `NOT`, `NOR2`, `NOR3`, `DFF`.
    pub kind: String,
    /// Input nets, in the library's pin order (`D`, `CLK` for a flip-flop).
    pub inputs: Vec<NetId>,
    pub output: NetId,
    /// A flip-flop's value before the first clock edge.
    pub init: bool,
}

impl Cell {
    pub fn is_dff(&self) -> bool {
        self.kind == "DFF"
    }
}

#[derive(Clone, Debug)]
pub struct Netlist {
    pub inputs: Vec<PortBit>,
    pub outputs: Vec<PortBit>,
    pub cells: Vec<Cell>,
    pub net_count: usize,
    /// Nets that are constant 0 or 1 (at most one of each).
    pub constants: Vec<(NetId, bool)>,
}

/// The name of bit `i` (counting from the least significant) of a port or net
/// of `width` bits, by the index the source declares: Yosys records the lowest
/// index as `offset` and `upto` when the range is written low to high.
pub fn bit_name(name: &str, entry: &Value, i: usize, width: usize) -> String {
    if width == 1 {
        return name.to_string();
    }
    let offset = entry["offset"].as_i64().unwrap_or(0);
    let upto = entry["upto"].as_i64().unwrap_or(0) != 0;
    let index = if upto { offset + width as i64 - 1 - i as i64 } else { offset + i as i64 };
    format!("{name}[{index}]")
}

/// What drives a net.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Driver {
    Input(usize),
    Cell(usize),
    Const(bool),
}

impl Netlist {
    /// Reads module `top` from a Yosys JSON netlist.
    ///
    /// Fails on anything the tile library cannot build: unknown cells, undriven
    /// or multiply driven nets. Buffers are dissolved into wire; constant bits
    /// become constant nets.
    pub fn from_yosys_json(json: &str, top: &str, library: &Library) -> Result<Netlist, String> {
        let root: Value = serde_json::from_str(json).map_err(|e| format!("netlist JSON: {e}"))?;
        let module = root["modules"].get(top).ok_or_else(|| format!("module {top:?} not found in the netlist"))?;
        let cells_of_library: BTreeMap<&str, Vec<&str>> = library.cells().into_iter().collect();

        // Yosys numbers nets from 2; the strings "0" and "1" are constants, which
        // get the private numbers 0 and 1 here.
        let bit = |value: &Value, what: &str| -> Result<u64, String> {
            match (value.as_u64(), value.as_str()) {
                (Some(n), _) => Ok(n),
                (_, Some("0")) => Ok(0),
                (_, Some("1")) => Ok(1),
                (_, Some("x")) => Err(format!(
                    "{what} is undefined (x): synthesis found nothing drives it, usually a register that is never assigned or a signal left unset; give it a value, a reset or an initial value"
                )),
                _ => Err(format!("{what} is tied to {value}, which the tile library cannot build")),
            }
        };

        let mut alias: BTreeMap<u64, u64> = BTreeMap::new(); // buffer output -> buffer input
        let mut raw_cells = Vec::new();
        let empty = serde_json::Map::new();
        for (name, cell) in module["cells"].as_object().unwrap_or(&empty) {
            let kind = cell["type"].as_str().unwrap_or_default();
            let pin = |pin: &str| -> Result<u64, String> {
                let bits = cell["connections"][pin].as_array().ok_or_else(|| format!("cell {name}: no pin {pin}"))?;
                match bits.as_slice() {
                    [one] => bit(one, &format!("cell {name} pin {pin}")),
                    _ => Err(format!("cell {name}: pin {pin} is not one bit")),
                }
            };
            if kind == "BUF" {
                alias.insert(pin("Y")?, pin("A")?);
                continue;
            }
            let pins = cells_of_library
                .get(kind)
                .ok_or_else(|| format!("cell {name} has type {kind}, which is not in the tile library"))?;
            let inputs = pins.iter().map(|p| pin(p)).collect::<Result<Vec<_>, _>>()?;
            let output = pin(if kind == "DFF" { "Q" } else { "Y" })?;
            raw_cells.push((name.clone(), kind.to_string(), inputs, output));
        }
        let resolve = |mut id: u64| {
            while let Some(&next) = alias.get(&id) {
                id = next;
            }
            id
        };

        let mut ids: BTreeMap<u64, NetId> = BTreeMap::new();
        let mut net = |raw: u64| -> NetId {
            let next = ids.len();
            *ids.entry(resolve(raw)).or_insert(next)
        };
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for (name, port) in module["ports"].as_object().unwrap_or(&empty) {
            let bits = port["bits"].as_array().ok_or_else(|| format!("port {name} has no bits"))?;
            for (i, value) in bits.iter().enumerate() {
                let bit_name = bit_name(name, port, i, bits.len());
                let entry = PortBit { net: net(bit(value, &format!("port {bit_name}"))?), name: bit_name };
                match port["direction"].as_str() {
                    Some("input") => inputs.push(entry),
                    Some("output") => outputs.push(entry),
                    other => {
                        return Err(format!(
                            "port {name} has direction {other:?}; only input and output are supported"
                        ));
                    }
                }
            }
        }
        // A flip-flop's initial value is an `init` attribute on a net name of its
        // output, and a public net name there (not hidden, not generated) is how the
        // user knows the flip-flop.
        let mut inits: BTreeMap<u64, bool> = BTreeMap::new();
        let mut public: BTreeMap<u64, String> = BTreeMap::new();
        for (name, entry) in module["netnames"].as_object().unwrap_or(&empty) {
            let init = entry["attributes"]["init"].as_str().unwrap_or_default();
            let bits = entry["bits"].as_array().cloned().unwrap_or_default();
            let hidden = entry["hide_name"].as_u64().unwrap_or(0) != 0 || name.starts_with('$');
            // Init strings are written most significant bit first.
            for (i, value) in bits.iter().enumerate() {
                if let Some(raw) = value.as_u64() {
                    if let Some(c) = init.chars().rev().nth(i)
                        && (c == '0' || c == '1')
                    {
                        inits.insert(raw, c == '1');
                    }
                    if !hidden {
                        let label = bit_name(name, entry, i, bits.len());
                        let slot = public.entry(raw).or_insert_with(|| label.clone());
                        if label < *slot {
                            *slot = label;
                        }
                    }
                }
            }
        }
        let mut labels: BTreeMap<String, usize> = BTreeMap::new();
        let cells: Vec<Cell> = raw_cells
            .into_iter()
            .map(|(name, kind, ins, out)| {
                let label = match public.get(&out) {
                    Some(l) if kind == "DFF" && !labels.contains_key(l) => l.clone(),
                    _ => name.clone(),
                };
                *labels.entry(label.clone()).or_default() += 1;
                Cell {
                    name,
                    label,
                    kind,
                    inputs: ins.into_iter().map(&mut net).collect(),
                    output: net(out),
                    init: inits.get(&out).copied().unwrap_or(false),
                }
            })
            .collect();

        let constants: Vec<(NetId, bool)> =
            ids.iter().filter(|(raw, _)| **raw < 2).map(|(raw, id)| (*id, *raw == 1)).collect();
        let netlist = Netlist { inputs, outputs, cells, net_count: ids.len(), constants };
        netlist.drivers()?; // every net has exactly one driver
        Ok(netlist)
    }

    /// The driver of `net`.
    pub fn driver_of(&self, net: NetId) -> Result<Driver, String> {
        Ok(self.drivers()?[net])
    }

    /// [`Netlist::sinks_of`] for every net at once, in one pass: `table[net]`
    /// is what `sinks_of(net)` returns, in the same order. For the placer, which
    /// asks for every net's sinks per gap per placement it scores.
    pub fn sink_table(&self) -> Vec<Vec<(usize, usize)>> {
        let mut table: Vec<Vec<(usize, usize)>> = vec![Vec::new(); self.net_count];
        for (c, cell) in self.cells.iter().enumerate() {
            for (pin, &n) in cell.inputs.iter().enumerate() {
                table[n].push((c, pin));
            }
        }
        for (o, port) in self.outputs.iter().enumerate() {
            table[port.net].push((usize::MAX, o));
        }
        table
    }

    /// Every pin that reads `net`: `(cell, pin)` pairs and `(usize::MAX, output index)` for outputs.
    pub fn sinks_of(&self, net: NetId) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (c, cell) in self.cells.iter().enumerate() {
            for (pin, &n) in cell.inputs.iter().enumerate() {
                if n == net {
                    out.push((c, pin));
                }
            }
        }
        for (o, port) in self.outputs.iter().enumerate() {
            if port.net == net {
                out.push((usize::MAX, o));
            }
        }
        out
    }

    /// The single driver of every net.
    pub fn drivers(&self) -> Result<Vec<Driver>, String> {
        let mut drivers: Vec<Option<Driver>> = vec![None; self.net_count];
        let mut claim = |net: NetId, driver: Driver, who: &str| -> Result<(), String> {
            if drivers[net].is_some() {
                return Err(format!("{who} drives a net that already has a driver"));
            }
            drivers[net] = Some(driver);
            Ok(())
        };
        for &(net, value) in &self.constants {
            claim(net, Driver::Const(value), &format!("constant {}", u8::from(value)))?;
        }
        for (i, port) in self.inputs.iter().enumerate() {
            claim(port.net, Driver::Input(i), &format!("input {}", port.name))?;
        }
        for (i, cell) in self.cells.iter().enumerate() {
            claim(cell.output, Driver::Cell(i), &format!("cell {}", cell.name))?;
        }
        drivers.into_iter().enumerate().map(|(net, d)| d.ok_or_else(|| format!("net {net} has no driver"))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOR2: &str = r#"{"modules":{"top":{
        "ports":{"a":{"direction":"input","bits":[2]},"b":{"direction":"input","bits":[3]},"y":{"direction":"output","bits":[4]}},
        "cells":{"g":{"type":"NOR2","connections":{"A":[2],"B":[3],"Y":[4]}}}}}}"#;

    /// The table built in one pass says what the per-net scan says, net by net
    /// and in the scan's order, over random netlists with fan-out, duplicate
    /// pins, constants, unread nets and flip-flops.
    #[test]
    fn the_sink_table_is_every_nets_sinks_in_the_scans_order() {
        for seed in 0..40 {
            let mut rng = crate::tiletest::Rng::new(1_300 + seed);
            let (gates, inputs, flip_flops) = (1 + rng.below(20), 1 + rng.below(8), rng.below(3));
            let netlist = crate::pnrtest::random_netlist_sized(&mut rng, gates, inputs, flip_flops);
            let table = netlist.sink_table();
            assert_eq!(table.len(), netlist.net_count);
            for net in 0..netlist.net_count {
                assert_eq!(table[net], netlist.sinks_of(net), "seed {seed}, net {net}");
            }
            assert!(table.iter().any(|sinks| sinks.len() > 1) || gates < 2, "seed {seed}: no fan-out at all");
        }
        let n = Netlist::from_yosys_json(NOR2, "top", &Library::builtin().unwrap()).unwrap();
        let table = n.sink_table();
        assert_eq!(table[n.inputs[0].net], vec![(0, 0)]);
        assert_eq!(table[n.outputs[0].net], vec![(usize::MAX, 0)]);
    }

    #[test]
    fn reads_a_mapped_nor() {
        let n = Netlist::from_yosys_json(NOR2, "top", &Library::builtin().unwrap()).unwrap();
        assert_eq!((n.inputs.len(), n.outputs.len(), n.cells.len(), n.net_count), (2, 1, 1, 3));
        assert_eq!(n.cells[0].inputs, vec![n.inputs[0].net, n.inputs[1].net]);
        assert_eq!(n.cells[0].output, n.outputs[0].net);
    }

    #[test]
    fn rejects_cells_outside_the_library_and_undefined_bits() {
        let lib = Library::builtin().unwrap();
        let unknown = NOR2.replace("NOR2", "$_AND_");
        assert!(Netlist::from_yosys_json(&unknown, "top", &lib).unwrap_err().contains("not in the tile library"));
        let undefined = NOR2.replace(r#""B":[3]"#, r#""B":["x"]"#);
        assert!(Netlist::from_yosys_json(&undefined, "top", &lib).is_err());
    }

    #[test]
    fn constant_bits_become_constant_nets() {
        let constant = NOR2.replace(r#""B":[3]"#, r#""B":["0"]"#);
        let n = Netlist::from_yosys_json(&constant, "top", &Library::builtin().unwrap()).unwrap();
        assert_eq!(n.constants.len(), 1);
        assert_eq!(n.driver_of(n.cells[0].inputs[1]).unwrap(), Driver::Const(false));
    }

    #[test]
    fn port_bits_are_named_by_the_declared_index() {
        // `input [7:4] a` and `output [0:3] y` with `assign y = a`: Yosys lists bits
        // least significant first, so a[4] is the first bit and y[3] is.
        let json = r#"{"modules":{"top":{
            "ports":{"a":{"direction":"input","offset":4,"bits":[2,3,4,5]},
                     "y":{"direction":"output","upto":1,"bits":[2,3,4,5]},
                     "s":{"direction":"input","bits":[6]}},
            "cells":{}}}}"#;
        let n = Netlist::from_yosys_json(json, "top", &Library::builtin().unwrap()).unwrap();
        let names = |ports: &[PortBit]| ports.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&n.inputs), vec!["a[4]", "a[5]", "a[6]", "a[7]", "s"]);
        assert_eq!(names(&n.outputs), vec!["y[3]", "y[2]", "y[1]", "y[0]"]);
        // a[4] and y[3] are the same net: the least significant bit of each.
        assert_eq!(n.inputs[0].net, n.outputs[0].net);
    }

    #[test]
    fn flip_flops_are_labelled_by_their_register_bit() {
        let json = r#"{"modules":{"top":{
            "ports":{"clk":{"direction":"input","bits":[2]},"q":{"direction":"output","bits":[3,5]}},
            "netnames":{"count":{"hide_name":0,"bits":[3,5],"attributes":{"init":"10"}},
                        "$abc$9$x":{"hide_name":1,"bits":[5],"attributes":{}}},
            "cells":{"$auto$ff.cc:337:slice$85":{"type":"DFF","connections":{"D":[5],"CLK":[2],"Q":[3]}},
                     "$auto$ff.cc:337:slice$86":{"type":"DFF","connections":{"D":[3],"CLK":[2],"Q":[5]}},
                     "g":{"type":"NOT","connections":{"A":[3],"Y":[4]}}}}}}"#;
        let n = Netlist::from_yosys_json(json, "top", &Library::builtin().unwrap()).unwrap();
        let labels: Vec<&str> = n.cells.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["count[0]", "count[1]", "g"]);
        assert_eq!(n.cells.iter().map(|c| c.init).collect::<Vec<_>>(), vec![false, true, false]);
    }

    #[test]
    fn an_undefined_port_bit_says_what_to_do() {
        let undefined =
            NOR2.replace(r#""y":{"direction":"output","bits":[4]}"#, r#""y":{"direction":"output","bits":["x"]}"#);
        let err = Netlist::from_yosys_json(&undefined, "top", &Library::builtin().unwrap()).unwrap_err();
        assert!(err.starts_with("port y is undefined (x)"), "{err}");
        assert!(err.contains("initial value"), "{err}");
    }

    #[test]
    fn a_buffer_becomes_wire() {
        let buffered = NOR2
            .replace(r#""cells":{"#, r#""cells":{"b":{"type":"BUF","connections":{"A":[4],"Y":[5]}},"#)
            .replace(r#""y":{"direction":"output","bits":[4]}"#, r#""y":{"direction":"output","bits":[5]}"#);
        let n = Netlist::from_yosys_json(&buffered, "top", &Library::builtin().unwrap()).unwrap();
        assert_eq!(n.cells.len(), 1);
        assert_eq!(n.cells[0].output, n.outputs[0].net);
    }
}
