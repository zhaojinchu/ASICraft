//! The compiler core as a WebAssembly module, for the browser playground and
//! for checking that the WebAssembly build gives the same output as the native
//! one.
//!
//! The interface is one function and JSON text, so no binding generator is
//! needed: `redfab_call(ptr, len)` takes a request `{"op": ..., ...}` and
//! returns a pointer to the reply, four bytes of little-endian length then
//! that many bytes of JSON. Memory for the request comes from
//! `redfab_alloc(len)`; both buffers go back through `redfab_free(ptr, len)`.
//! A failure is `{"ok": false, "error": ...}`; `redfab_build` is the old name
//! of the `build` operation.
//!
//! Operations:
//!
//! - `build` `{mapped, top, name}`: a Yosys JSON netlist to a design: `{ok,
//!   design, structure (hex .nbt), report}`.
//! - `library`: what synthesis needs: `{liberty, models, map, equiv}`.
//! - `sim_new` `{design}`: a simulator on a design file: `{handle}`.
//! - `sim_set` `{handle, input, on}`: flips input number `input`, as the last
//!   event of the current tick: `{changes: [[pos, state]]}`, what changed at once.
//! - `sim_step` `{handle, ticks}`: runs ticks: `{tick, changes: [[pos, state]],
//!   lamps: [0|1]}`.
//! - `sim_trace` `{handle}`: everything so far as a fixture and its trace, in
//!   the trace format, so it can be compared with the native simulator.
//! - `sim_free` `{handle}`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;

use redfab_core::block::Block;
use redfab_core::build::Design;
use redfab_core::geom::Pos;
use redfab_core::sim::Sim;
use redfab_core::tile::Library;
use redfab_core::trace::Recorder;
use serde_json::{Value, json};

/// A running simulation with what the checks need to replay it natively.
struct Running {
    sim: Sim,
    design: Design,
    probes: Vec<Pos>,
    lamps: Vec<Pos>,
    recorder: Recorder,
}

thread_local! {
    static RUNNING: RefCell<Vec<Option<Running>>> = const { RefCell::new(Vec::new()) };
}

fn with_running<T>(handle: usize, f: impl FnOnce(&mut Running) -> Result<T, String>) -> Result<T, String> {
    RUNNING.with(|cell| {
        let mut list = cell.borrow_mut();
        let running = list.get_mut(handle).and_then(Option::as_mut).ok_or("no simulation with that handle")?;
        f(running)
    })
}

fn handle(request: &Value) -> Result<Value, String> {
    let text = |key: &str| request[key].as_str().ok_or_else(|| format!("request is missing {key:?}"));
    match request["op"].as_str().unwrap_or("build") {
        "build" => {
            let built = redfab_core::build::build(text("mapped")?, text("top")?, text("name")?)?;
            let mut hex = String::with_capacity(built.structure.len() * 2);
            for byte in &built.structure {
                write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
            }
            Ok(json!({"ok": true, "design": built.design.to_json(), "structure": hex, "report": built.report}))
        }
        "library" => {
            let library = Library::builtin()?;
            Ok(json!({
                "ok": true,
                "liberty": library.liberty(),
                "models": library.verilog_models(),
                "map": redfab_core::synth::MAP_SCRIPT,
                "equiv": redfab_core::synth::EQUIV_SCRIPT,
            }))
        }
        "sim_new" => {
            let design = Design::from_json(text("design")?)?;
            let probes = design.probes()?;
            let lamps = design.lamps()?;
            let sim = Sim::new(design.world()?);
            let recorder = Recorder::start(&design.name, design.size, &sim);
            let running = Running { sim, design, probes, lamps, recorder };
            let handle = RUNNING.with(|cell| {
                let mut list = cell.borrow_mut();
                list.push(Some(running));
                list.len() - 1
            });
            Ok(json!({"ok": true, "handle": handle}))
        }
        "sim_set" => {
            let h = request["handle"].as_u64().ok_or("request is missing handle")? as usize;
            let input = request["input"].as_u64().ok_or("request is missing input")? as usize;
            let on = request["on"].as_bool().ok_or("request is missing on")?;
            with_running(h, |r| {
                let pin = r.design.inputs.get(input).ok_or("no such input")?;
                let block: Block = if on { pin.on.parse()? } else { pin.off.parse()? };
                let probe = r.probes[input];
                let before: BTreeMap<Pos, Block> = r.sim.world().iter().collect();
                r.sim.set_block(probe, block, false)?;
                r.recorder.event(&r.sim, probe, block);
                // What changed at once: the probe, and what dust and lamps did in response.
                let after: BTreeMap<Pos, Block> = r.sim.world().iter().collect();
                let changes: Vec<(String, String)> =
                    Recorder::diff(&before, &after).into_iter().map(|(p, b)| (p.to_string(), b.to_string())).collect();
                Ok(json!({"ok": true, "changes": changes}))
            })
        }
        "sim_step" => {
            let h = request["handle"].as_u64().ok_or("request is missing handle")? as usize;
            let ticks = request["ticks"].as_u64().unwrap_or(1).min(100_000) as u32;
            with_running(h, |r| {
                let mut changes: BTreeMap<Pos, Block> = BTreeMap::new();
                for _ in 0..ticks {
                    r.sim.step();
                    for (pos, block) in r.recorder.record(&r.sim) {
                        changes.insert(pos, block);
                    }
                }
                let lamps: Vec<u8> =
                    r.lamps.iter().map(|&l| u8::from(r.sim.world().get(l) == Block::Lamp { lit: true })).collect();
                let changes: Vec<(String, String)> =
                    changes.into_iter().map(|(p, b)| (p.to_string(), b.to_string())).collect();
                Ok(json!({"ok": true, "tick": r.recorder.tick(), "changes": changes, "lamps": lamps}))
            })
        }
        "sim_trace" => {
            let h = request["handle"].as_u64().ok_or("request is missing handle")? as usize;
            with_running(h, |r| {
                let (fixture, trace) = r.recorder.finish(&r.design.blocks);
                Ok(json!({"ok": true, "fixture": fixture, "trace": trace}))
            })
        }
        "sim_free" => {
            let h = request["handle"].as_u64().ok_or("request is missing handle")? as usize;
            RUNNING.with(|cell| {
                let mut list = cell.borrow_mut();
                if let Some(slot) = list.get_mut(h) {
                    *slot = None;
                }
            });
            Ok(json!({"ok": true}))
        }
        other => Err(format!("unknown op {other:?}")),
    }
}

fn respond(request: &[u8]) -> String {
    let result = serde_json::from_slice::<Value>(request)
        .map_err(|e| format!("request: {e}"))
        .and_then(|request| handle(&request));
    result.unwrap_or_else(|error| json!({"ok": false, "error": error})).to_string()
}

#[unsafe(no_mangle)]
pub extern "C" fn redfab_alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(len);
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// # Safety
/// `ptr` and `len` must come from `redfab_alloc(len)`, or from `redfab_call`
/// (whose reply is `4 + length` bytes long), and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn redfab_free(ptr: *mut u8, len: usize) {
    drop(unsafe { Vec::from_raw_parts(ptr, 0, len) });
}

/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn redfab_call(ptr: *const u8, len: usize) -> *mut u8 {
    let request = unsafe { std::slice::from_raw_parts(ptr, len) };
    let reply = respond(request);
    let mut out = Vec::with_capacity(4 + reply.len());
    out.extend_from_slice(&(reply.len() as u32).to_le_bytes());
    out.extend_from_slice(reply.as_bytes());
    out.shrink_to_fit();
    let ptr = out.as_mut_ptr();
    std::mem::forget(out);
    ptr
}

/// The `build` operation under its old name.
///
/// # Safety
/// As for [`redfab_call`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn redfab_build(ptr: *const u8, len: usize) -> *mut u8 {
    unsafe { redfab_call(ptr, len) }
}
