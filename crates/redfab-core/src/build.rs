//! From a mapped netlist to a finished, checked block world.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::bake::bake;
use crate::block::Block;
use crate::extract::{extract, weak_signals};
use crate::geom::Pos;
use crate::lvs::lvs;
use crate::netlist::Netlist;
use crate::place::place_single_gate;
use crate::sim::Sim;
use crate::structure::structure_file;
use crate::tile::Library;
use crate::world::World;

pub const DESIGN_FORMAT: &str = "redfab-design/1";
/// A lamp goes out this many game ticks after it loses power.
const LAMP_OFF_DELAY: u32 = 4;

/// A built design: its blocks in their settled all-inputs-off state, and how to
/// drive and read it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Design {
    pub format: String,
    pub name: String,
    pub size: [i32; 3],
    pub inputs: Vec<InputPin>,
    pub outputs: Vec<OutputPin>,
    /// Game ticks after an input change by which every output is final.
    pub settle_ticks: u32,
    /// Position (`x,y,z`) to full block state.
    pub blocks: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InputPin {
    pub name: String,
    /// The block to swap: `off` for 0, `on` for 1.
    pub probe: String,
    pub on: String,
    pub off: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OutputPin {
    pub name: String,
    pub lamp: String,
}

pub struct Built {
    pub design: Design,
    /// The same blocks as a vanilla structure file.
    pub structure: Vec<u8>,
    pub report: String,
}

impl Design {
    pub fn from_json(json: &str) -> Result<Design, String> {
        let design: Design = serde_json::from_str(json).map_err(|e| format!("design file: {e}"))?;
        if design.format != DESIGN_FORMAT {
            return Err(format!("unexpected design format {:?}", design.format));
        }
        Ok(design)
    }

    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a design always serializes");
        text.push('\n');
        text
    }

    pub fn world(&self) -> Result<World, String> {
        let mut world = World::new((self.size[0], self.size[1], self.size[2]));
        for (pos, state) in &self.blocks {
            world.set(pos.parse()?, state.parse()?);
        }
        Ok(world)
    }

    pub fn probes(&self) -> Result<Vec<Pos>, String> {
        self.inputs.iter().map(|i| i.probe.parse()).collect()
    }

    pub fn lamps(&self) -> Result<Vec<Pos>, String> {
        self.outputs.iter().map(|o| o.lamp.parse()).collect()
    }
}

/// Places, bakes and checks the design in a Yosys JSON netlist that has been
/// mapped to the tile library. Fails if the layout does not implement the netlist.
pub fn build(mapped_json: &str, top: &str, name: &str) -> Result<Built, String> {
    let library = Library::builtin()?;
    let netlist = Netlist::from_yosys_json(mapped_json, top, &library)?;
    let layout = place_single_gate(&netlist)?;
    let realized = layout.realize(&library, &netlist)?;
    let world = bake(realized.world)?;

    let problems = check_world(&world, &netlist, &realized.probes, &realized.lamps);
    if !problems.is_empty() {
        return Err(format!(
            "the layout does not implement the netlist:\n  - {}",
            problems.join("\n  - ")
        ));
    }

    let (sx, sy, sz) = world.size();
    let design = Design {
        format: DESIGN_FORMAT.to_string(),
        name: name.to_string(),
        size: [sx, sy, sz],
        inputs: netlist
            .inputs
            .iter()
            .zip(&realized.probes)
            .map(|(port, probe)| InputPin {
                name: port.name.clone(),
                probe: probe.to_string(),
                on: Block::RedstoneBlock.to_string(),
                off: Block::Stone.to_string(),
            })
            .collect(),
        outputs: netlist
            .outputs
            .iter()
            .zip(&realized.lamps)
            .map(|(port, lamp)| OutputPin {
                name: port.name.clone(),
                lamp: lamp.to_string(),
            })
            .collect(),
        settle_ticks: library.nor.delay + LAMP_OFF_DELAY,
        blocks: world
            .iter()
            .map(|(pos, block)| (pos.to_string(), block.to_string()))
            .collect(),
    };
    let (cols, rows) = layout.size();
    let report = format!(
        "design {name}\n  cells: {}\n  tiles: {} ({cols} columns x {rows} rows)\n  blocks: {} in {sx} x {sy} x {sz}\n  settles within {} game ticks of an input change\n  layout matches the netlist\n",
        netlist.cells.len(),
        layout.tiles.len(),
        design.blocks.len(),
        design.settle_ticks,
    );
    Ok(Built {
        structure: structure_file(&world),
        design,
        report,
    })
}

/// The layout-versus-netlist check and the signal-strength check on a block world.
pub fn check_world(world: &World, netlist: &Netlist, probes: &[Pos], lamps: &[Pos]) -> Vec<String> {
    let circuit = extract(world, probes);
    let mut problems = lvs(netlist, &circuit, probes, lamps);
    problems.extend(weak_signals(world, probes, &circuit));
    problems
}

/// Re-checks a design file against a netlist. Used to prove that broken layouts are caught.
pub fn check_design(
    design_json: &str,
    mapped_json: &str,
    top: &str,
) -> Result<Vec<String>, String> {
    let design = Design::from_json(design_json)?;
    let netlist = Netlist::from_yosys_json(mapped_json, top, &Library::builtin()?)?;
    Ok(check_world(
        &design.world()?,
        &netlist,
        &design.probes()?,
        &design.lamps()?,
    ))
}

/// Applies input vectors to a design in the simulator, one after another, and
/// returns the outputs after each has settled.
///
/// `vectors` is a JSON list of objects mapping input names to 0 or 1; the
/// result is a JSON list of objects mapping output names to 0 or 1.
pub fn simulate(design_json: &str, vectors_json: &str) -> Result<String, String> {
    let design = Design::from_json(design_json)?;
    let vectors: Vec<BTreeMap<String, u8>> =
        serde_json::from_str(vectors_json).map_err(|e| format!("vectors: {e}"))?;
    let probes = design.probes()?;
    let lamps = design.lamps()?;
    let mut sim = Sim::new(design.world()?);
    let mut results = Vec::new();
    for vector in &vectors {
        let mut changes = Vec::new();
        for (input, &probe) in design.inputs.iter().zip(&probes) {
            let value = *vector
                .get(&input.name)
                .ok_or_else(|| format!("vector is missing input {}", input.name))?;
            let block: Block = if value != 0 {
                input.on.parse()?
            } else {
                input.off.parse()?
            };
            changes.push((probe, block));
        }
        // Same order as the trace format gives stimuli that share a tick.
        changes.sort_by_key(|(pos, _)| crate::trace::event_order(*pos));
        for (probe, block) in changes {
            sim.set_block(probe, block, false)?;
        }
        for _ in 0..design.settle_ticks {
            sim.step();
        }
        if !sim.pending().is_empty() {
            return Err(format!(
                "the design is still active {} ticks after an input change",
                design.settle_ticks
            ));
        }
        let mut outputs = BTreeMap::new();
        for (output, &lamp) in design.outputs.iter().zip(&lamps) {
            outputs.insert(
                output.name.clone(),
                u8::from(sim.world().get(lamp) == Block::Lamp { lit: true }),
            );
        }
        results.push(outputs);
    }
    serde_json::to_string(&results).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOR2: &str = r#"{"modules":{"top":{
        "ports":{"a":{"direction":"input","bits":[2]},"b":{"direction":"input","bits":[3]},"y":{"direction":"output","bits":[4]}},
        "cells":{"g":{"type":"NOR2","connections":{"A":[2],"B":[3],"Y":[4]}}}}}}"#;

    fn outputs(design: &Design, vectors: &str) -> String {
        simulate(&design.to_json(), vectors).unwrap()
    }

    #[test]
    fn a_nor_gate_builds_and_computes_nor() {
        let built = build(NOR2, "top", "nor2").unwrap();
        let vectors = r#"[{"a":0,"b":0},{"a":0,"b":1},{"a":1,"b":0},{"a":1,"b":1}]"#;
        assert_eq!(
            outputs(&built.design, vectors),
            r#"[{"y":1},{"y":0},{"y":0},{"y":0}]"#
        );
    }

    #[test]
    fn the_built_state_is_already_settled() {
        let built = build(NOR2, "top", "nor2").unwrap();
        let world = built.design.world().unwrap();
        let mut sim = Sim::new(world.clone());
        sim.poke_all();
        assert!(sim.pending().is_empty());
        assert_eq!(sim.world(), &world);
    }

    #[test]
    fn removing_the_torch_is_caught() {
        let mut design = build(NOR2, "top", "nor2").unwrap().design;
        let torch = design
            .blocks
            .iter()
            .find(|(_, s)| s.contains("wall_torch"))
            .map(|(p, _)| p.clone())
            .unwrap();
        design.blocks.remove(&torch);
        let problems = check_design(&design.to_json(), NOR2, "top").unwrap();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("no torch implements cell g")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_wrong_netlist_is_caught() {
        // The layout is a NOR of a and b; a netlist that says NOT a must not match it.
        let design = build(NOR2, "top", "nor2").unwrap().design;
        let not_a = NOR2.replace(
            r#""type":"NOR2","connections":{"A":[2],"B":[3],"Y":[4]}"#,
            r#""type":"NOT","connections":{"A":[2],"Y":[4]}"#,
        );
        let problems = check_design(&design.to_json(), &not_a, "top").unwrap();
        assert!(!problems.is_empty());
    }

    #[test]
    fn designs_needing_routing_are_refused_for_now() {
        let two_cells = NOR2
            .replace(
                r#""cells":{"#,
                r#""cells":{"h":{"type":"NOT","connections":{"A":[4],"Y":[5]}},"#,
            )
            .replace(
                r#""y":{"direction":"output","bits":[4]}"#,
                r#""y":{"direction":"output","bits":[5]}"#,
            );
        let err = build(&two_cells, "top", "x").err().unwrap();
        assert!(err.contains("placement and routing"), "{err}");
    }
}
