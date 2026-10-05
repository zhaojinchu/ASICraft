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
    /// Library cell name: `NOT`, `NOR2`, `NOR3`.
    pub kind: String,
    /// Input nets, in the library's pin order.
    pub inputs: Vec<NetId>,
    pub output: NetId,
}

#[derive(Clone, Debug)]
pub struct Netlist {
    pub inputs: Vec<PortBit>,
    pub outputs: Vec<PortBit>,
    pub cells: Vec<Cell>,
    pub net_count: usize,
}

/// What drives a net.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Driver {
    Input(usize),
    Cell(usize),
}

impl Netlist {
    /// Reads module `top` from a Yosys JSON netlist.
    ///
    /// Fails on anything the tile library cannot build: unknown cells, constant
    /// nets, undriven or multiply driven nets. Buffers are dissolved into wire.
    pub fn from_yosys_json(json: &str, top: &str, library: &Library) -> Result<Netlist, String> {
        let root: Value = serde_json::from_str(json).map_err(|e| format!("netlist JSON: {e}"))?;
        let module = root["modules"]
            .get(top)
            .ok_or_else(|| format!("module {top:?} not found in the netlist"))?;
        let cells_of_library: BTreeMap<&str, Vec<&str>> = library.cells().into_iter().collect();

        // Yosys numbers nets from 2; strings are constants.
        let bit = |value: &Value, what: &str| -> Result<u64, String> {
            value.as_u64().ok_or_else(|| {
                format!("{what} is tied to constant {value}; constants are not supported yet")
            })
        };

        let mut alias: BTreeMap<u64, u64> = BTreeMap::new(); // buffer output -> buffer input
        let mut raw_cells = Vec::new();
        let empty = serde_json::Map::new();
        for (name, cell) in module["cells"].as_object().unwrap_or(&empty) {
            let kind = cell["type"].as_str().unwrap_or_default();
            let pin = |pin: &str| -> Result<u64, String> {
                let bits = cell["connections"][pin]
                    .as_array()
                    .ok_or_else(|| format!("cell {name}: no pin {pin}"))?;
                match bits.as_slice() {
                    [one] => bit(one, &format!("cell {name} pin {pin}")),
                    _ => Err(format!("cell {name}: pin {pin} is not one bit")),
                }
            };
            if kind == "BUF" {
                alias.insert(pin("Y")?, pin("A")?);
                continue;
            }
            let pins = cells_of_library.get(kind).ok_or_else(|| {
                format!("cell {name} has type {kind}, which is not in the tile library")
            })?;
            let inputs = pins.iter().map(|p| pin(p)).collect::<Result<Vec<_>, _>>()?;
            raw_cells.push((name.clone(), kind.to_string(), inputs, pin("Y")?));
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
            let bits = port["bits"]
                .as_array()
                .ok_or_else(|| format!("port {name} has no bits"))?;
            for (i, value) in bits.iter().enumerate() {
                let bit_name = if bits.len() == 1 {
                    name.clone()
                } else {
                    format!("{name}[{i}]")
                };
                let entry = PortBit {
                    net: net(bit(value, &format!("port {bit_name}"))?),
                    name: bit_name,
                };
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
        let cells: Vec<Cell> = raw_cells
            .into_iter()
            .map(|(name, kind, ins, out)| Cell {
                name,
                kind,
                inputs: ins.into_iter().map(&mut net).collect(),
                output: net(out),
            })
            .collect();

        let netlist = Netlist {
            inputs,
            outputs,
            cells,
            net_count: ids.len(),
        };
        netlist.drivers()?; // every net has exactly one driver
        Ok(netlist)
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
        for (i, port) in self.inputs.iter().enumerate() {
            claim(port.net, Driver::Input(i), &format!("input {}", port.name))?;
        }
        for (i, cell) in self.cells.iter().enumerate() {
            claim(cell.output, Driver::Cell(i), &format!("cell {}", cell.name))?;
        }
        drivers
            .into_iter()
            .enumerate()
            .map(|(net, d)| d.ok_or_else(|| format!("net {net} has no driver")))
            .collect()
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
        assert_eq!(
            (n.inputs.len(), n.outputs.len(), n.cells.len(), n.net_count),
            (2, 1, 1, 3)
        );
        assert_eq!(n.cells[0].inputs, vec![n.inputs[0].net, n.inputs[1].net]);
        assert_eq!(n.cells[0].output, n.outputs[0].net);
    }

    #[test]
    fn rejects_cells_outside_the_library_and_constants() {
        let lib = Library::builtin().unwrap();
        let unknown = NOR2.replace("NOR2", "$_AND_");
        assert!(
            Netlist::from_yosys_json(&unknown, "top", &lib)
                .unwrap_err()
                .contains("not in the tile library")
        );
        let constant = NOR2.replace(r#""B":[3]"#, r#""B":["0"]"#);
        assert!(
            Netlist::from_yosys_json(&constant, "top", &lib)
                .unwrap_err()
                .contains("constant")
        );
    }

    #[test]
    fn a_buffer_becomes_wire() {
        let buffered = NOR2
            .replace(
                r#""cells":{"#,
                r#""cells":{"b":{"type":"BUF","connections":{"A":[4],"Y":[5]}},"#,
            )
            .replace(
                r#""y":{"direction":"output","bits":[4]}"#,
                r#""y":{"direction":"output","bits":[5]}"#,
            );
        let n = Netlist::from_yosys_json(&buffered, "top", &Library::builtin().unwrap()).unwrap();
        assert_eq!(n.cells.len(), 1);
        assert_eq!(n.cells[0].output, n.outputs[0].net);
    }
}
