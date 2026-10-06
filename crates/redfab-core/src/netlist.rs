//! The synthesized design: library cells joined by single-bit nets.
//! Read from the JSON netlist Yosys writes after mapping to the tile library.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::tile::Library;

pub type NetId = usize;

#[derive(Clone, Debug)]
pub struct PortBit {
    /// `name` for a one-bit port, `name[i]` for bit i of a wider one.
    pub name: String,
    pub net: NetId,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub name: String,
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
                let bit_name = if bits.len() == 1 { name.clone() } else { format!("{name}[{i}]") };
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
        // A flip-flop's initial value is an `init` attribute on a net name of its output.
        let mut inits: BTreeMap<u64, bool> = BTreeMap::new();
        for (_, entry) in module["netnames"].as_object().unwrap_or(&empty) {
            let init = entry["attributes"]["init"].as_str().unwrap_or_default();
            let bits = entry["bits"].as_array().cloned().unwrap_or_default();
            // Init strings are written most significant bit first.
            for (i, value) in bits.iter().enumerate() {
                if let Some(raw) = value.as_u64()
                    && let Some(c) = init.chars().rev().nth(i)
                    && (c == '0' || c == '1')
                {
                    inits.insert(raw, c == '1');
                }
            }
        }
        let cells: Vec<Cell> = raw_cells
            .into_iter()
            .map(|(name, kind, ins, out)| Cell {
                name,
                kind,
                inputs: ins.into_iter().map(&mut net).collect(),
                output: net(out),
                init: inits.get(&out).copied().unwrap_or(false),
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
    fn a_buffer_becomes_wire() {
        let buffered = NOR2
            .replace(r#""cells":{"#, r#""cells":{"b":{"type":"BUF","connections":{"A":[4],"Y":[5]}},"#)
            .replace(r#""y":{"direction":"output","bits":[4]}"#, r#""y":{"direction":"output","bits":[5]}"#);
        let n = Netlist::from_yosys_json(&buffered, "top", &Library::builtin().unwrap()).unwrap();
        assert_eq!(n.cells.len(), 1);
        assert_eq!(n.cells[0].output, n.outputs[0].net);
    }
}
