//! The `redfab` command. All file and process access lives here, not in
//! `redfab-core`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use redfab_core::sim::Mutation;
use redfab_core::trace::{Replay, replay};

/// File-name suffixes of recorded traces, by redstone mode.
const TRACE_MODES: [&str; 2] = ["vanilla", "vanilla-rx"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("check-traces") => check_traces(&args[1..]),
        Some("replay") => replay_report_command(&args[1..]),
        Some("structure") => structure_command(&args[1..]),
        Some("structure-pattern") => structure_pattern_command(&args[1..]),
        Some("list-mutations") => {
            for m in Mutation::ALL {
                println!("{}", m.name());
            }
            Ok(true)
        }
        Some("--version") | None => {
            println!(
                "redfab {} (Minecraft {}, data version {})",
                env!("CARGO_PKG_VERSION"),
                redfab_core::structure::MC_VERSION,
                redfab_core::structure::DATA_VERSION
            );
            Ok(true)
        }
        Some(other) => Err(format!("unknown command {other:?}")),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

/// `redfab check-traces [--mutate NAME] [--verbose] DIR`
///
/// Replays every recorded trace in DIR in the simulator. Succeeds only if every
/// replayed trace matches and at least one was replayed.
fn check_traces(args: &[String]) -> Result<bool, String> {
    let mut mutation = None;
    let mut verbose = false;
    let mut dir = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--mutate" => {
                let name = it.next().ok_or("--mutate needs a name")?;
                mutation = Some(
                    Mutation::from_name(name)
                        .ok_or_else(|| format!("unknown mutation {name:?}"))?,
                );
            }
            "--verbose" => verbose = true,
            other => dir = Some(PathBuf::from(other)),
        }
    }
    let dir = dir.ok_or("usage: redfab check-traces [--mutate NAME] [--verbose] DIR")?;

    let mut fixtures: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".fixture.json"))
        })
        .collect();
    fixtures.sort();

    let (mut matched, mut mismatched, mut unsupported, mut racy) = (0, 0, 0, 0);
    for fixture_path in &fixtures {
        let name = fixture_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .trim_end_matches(".fixture.json");
        let fixture_json = read(fixture_path)?;
        for mode in TRACE_MODES {
            let trace_path = dir.join(format!("{name}.{mode}.trace.json"));
            if !trace_path.exists() {
                continue;
            }
            match replay(&fixture_json, &read(&trace_path)?, mutation)
                .map_err(|e| format!("{name} [{mode}]: {e}"))?
            {
                Replay::Match { reordered_ticks } => {
                    matched += 1;
                    if reordered_ticks > 0 {
                        racy += 1;
                    }
                    if verbose {
                        let note = if reordered_ticks > 0 {
                            format!(" (race: {reordered_ticks} tick(s) needed another order)")
                        } else {
                            String::new()
                        };
                        println!("  ok           {name} [{mode}]{note}");
                    }
                }
                Replay::Mismatch(why) => {
                    mismatched += 1;
                    println!("  MISMATCH     {name} [{mode}]: {why}");
                }
                Replay::Unsupported(why) => {
                    unsupported += 1;
                    if verbose {
                        println!("  unsupported  {name} [{mode}]: {why}");
                    }
                }
            }
        }
    }
    println!(
        "{}: {matched} matched ({racy} of them only with a different order of same-tick events), {mismatched} mismatched, {unsupported} skipped (unsupported blocks)",
        dir.display()
    );
    Ok(mismatched == 0 && matched > 0)
}

/// `redfab replay FIXTURE TRACE [TICKS]`: prints the game's recording beside the
/// simulator's result, tick by tick, as text layers.
fn replay_report_command(args: &[String]) -> Result<bool, String> {
    let [fixture, trace, rest @ ..] = args else {
        return Err("usage: redfab replay FIXTURE.json TRACE.json [TICKS]".into());
    };
    let until = rest.first().map_or(Ok(10), |t| {
        t.parse::<u32>()
            .map_err(|_| format!("bad tick count {t:?}"))
    })?;
    print!(
        "{}",
        redfab_core::trace::replay_report(
            &read(Path::new(fixture))?,
            &read(Path::new(trace))?,
            until
        )?
    );
    Ok(true)
}

/// `redfab structure TRACE.json OUT.nbt`: writes the trace's tick-0 state as a
/// vanilla structure file.
fn structure_command(args: &[String]) -> Result<bool, String> {
    let [trace, out] = args else {
        return Err("usage: redfab structure TRACE.json OUT.nbt".into());
    };
    let world = redfab_core::trace::initial_world(&read(Path::new(trace))?)?;
    fs::write(out, redfab_core::structure::structure_file(&world))
        .map_err(|e| format!("{out}: {e}"))?;
    Ok(true)
}

/// `redfab structure-pattern SX SY SZ OUT.nbt`: a test structure of the given
/// size, stone where x+y+z is even and glass elsewhere.
fn structure_pattern_command(args: &[String]) -> Result<bool, String> {
    let [sx, sy, sz, out] = args else {
        return Err("usage: redfab structure-pattern SX SY SZ OUT.nbt".into());
    };
    let dim = |s: &String| s.parse::<i32>().map_err(|_| format!("bad size {s:?}"));
    let size = (dim(sx)?, dim(sy)?, dim(sz)?);
    let mut world = redfab_core::world::World::new(size);
    for y in 0..size.1 {
        for z in 0..size.2 {
            for x in 0..size.0 {
                let block = if (x + y + z) % 2 == 0 {
                    redfab_core::block::Block::Stone
                } else {
                    redfab_core::block::Block::Glass
                };
                world.set(redfab_core::geom::Pos::new(x, y, z), block);
            }
        }
    }
    fs::write(out, redfab_core::structure::structure_file(&world))
        .map_err(|e| format!("{out}: {e}"))?;
    Ok(true)
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}
