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
        Some("tile-adjacency") => tile_adjacency_command(),
        Some("tile-fixtures") => tile_fixtures_command(&args[1..]),
        Some("check-fixtures") => check_fixtures_command(&args[1..]),
        Some("build") => build_command(&args[1..]),
        Some("layout") => layout_command(&args[1..]),
        Some("pnr-random") => pnr_random_command(&args[1..]),
        Some("lvs") => lvs_command(&args[1..]),
        Some("extract") => extract_command(&args[1..]),
        Some("timing") => timing_command(&args[1..]),
        Some("characterize") => characterize_command(&args[1..]),
        Some("stress") => stress_command(&args[1..]),
        Some("timing-measure") => timing_measure_command(&args[1..]),
        Some("simulate") => simulate_command(&args[1..]),
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

/// `redfab check-traces [--mutate NAME] [--verbose] [--no-skips] DIR`
///
/// Replays every recorded trace in DIR in the simulator. Succeeds only if every
/// replayed trace matches and at least one was replayed. With `--no-skips`, a
/// trace that uses a block the simulator lacks is a failure too.
fn check_traces(args: &[String]) -> Result<bool, String> {
    let mut mutation = None;
    let mut verbose = false;
    let mut no_skips = false;
    let mut dir = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--mutate" => {
                let name = it.next().ok_or("--mutate needs a name")?;
                mutation = Some(Mutation::from_name(name).ok_or_else(|| format!("unknown mutation {name:?}"))?);
            }
            "--verbose" => verbose = true,
            "--no-skips" => no_skips = true,
            other => dir = Some(PathBuf::from(other)),
        }
    }
    let dir = dir.ok_or("usage: redfab check-traces [--mutate NAME] [--verbose] DIR")?;

    let mut fixtures: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".fixture.json")))
        .collect();
    fixtures.sort();

    let (mut matched, mut mismatched, mut unsupported, mut racy) = (0, 0, 0, 0);
    for fixture_path in &fixtures {
        let name =
            fixture_path.file_name().and_then(|n| n.to_str()).unwrap_or_default().trim_end_matches(".fixture.json");
        let fixture_json = read(fixture_path)?;
        for mode in TRACE_MODES {
            let trace_path = dir.join(format!("{name}.{mode}.trace.json"));
            if !trace_path.exists() {
                continue;
            }
            match replay(&fixture_json, &read(&trace_path)?, mutation).map_err(|e| format!("{name} [{mode}]: {e}"))? {
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
    Ok(mismatched == 0 && matched > 0 && !(no_skips && unsupported > 0))
}

/// `redfab replay FIXTURE TRACE [TICKS]`: prints the game's recording beside the
/// simulator's result, tick by tick, as text layers.
fn replay_report_command(args: &[String]) -> Result<bool, String> {
    let [fixture, trace, rest @ ..] = args else {
        return Err("usage: redfab replay FIXTURE.json TRACE.json [TICKS]".into());
    };
    let until = rest.first().map_or(Ok(10), |t| t.parse::<u32>().map_err(|_| format!("bad tick count {t:?}")))?;
    print!("{}", redfab_core::trace::replay_report(&read(Path::new(fixture))?, &read(Path::new(trace))?, until)?);
    Ok(true)
}

/// `redfab structure TRACE.json OUT.nbt`: writes the trace's tick-0 state as a
/// vanilla structure file.
fn structure_command(args: &[String]) -> Result<bool, String> {
    let [trace, out] = args else {
        return Err("usage: redfab structure TRACE.json OUT.nbt".into());
    };
    let world = redfab_core::trace::initial_world(&read(Path::new(trace))?)?;
    fs::write(out, redfab_core::structure::structure_file(&world)).map_err(|e| format!("{out}: {e}"))?;
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
    fs::write(out, redfab_core::structure::structure_file(&world)).map_err(|e| format!("{out}: {e}"))?;
    Ok(true)
}

/// `redfab tile-adjacency`: every pair of tile variants side by side, and 400
/// random 2×2 groups, must interact only through matched ports.
fn tile_adjacency_command() -> Result<bool, String> {
    let library = redfab_core::tile::Library::builtin()?;
    let (checked, problems) = redfab_core::tiletest::check_all_adjacency(&library, 400, 1);
    for problem in &problems {
        println!("{problem}");
    }
    println!("{checked} tile groups checked, {} problems", problems.len());
    Ok(problems.is_empty())
}

/// `redfab tile-fixtures DIR [--groups N] [--seed S]`: writes a fixture for every
/// tile variant, plus N random tile groups (pairs and 2×2) for the game.
fn tile_fixtures_command(args: &[String]) -> Result<bool, String> {
    let mut dir = None;
    let mut groups = 24;
    let mut seed = 1;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--groups" => groups = it.next().and_then(|n| n.parse().ok()).ok_or("--groups needs a number")?,
            "--seed" => seed = it.next().and_then(|n| n.parse().ok()).ok_or("--seed needs a number")?,
            other => dir = Some(PathBuf::from(other)),
        }
    }
    let dir = dir.ok_or("usage: redfab tile-fixtures DIR [--groups N] [--seed S]")?;
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let library = redfab_core::tile::Library::builtin()?;
    let mut count = 0;
    for (name, fixture) in redfab_core::tiletest::all_variant_fixtures(&library) {
        fs::write(
            dir.join(format!("{name}.fixture.json")),
            serde_json::to_string(&fixture).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        count += 1;
    }
    let mut rng = redfab_core::tiletest::Rng::new(seed);
    for i in 0..groups {
        let (cols, rows) = [(2, 1), (1, 2), (2, 2)][i % 3];
        let name = format!("group_{seed}_{i:02}");
        let fixture = redfab_core::tiletest::group_fixture(&name, cols, rows, &mut rng, &library);
        fs::write(
            dir.join(format!("{name}.fixture.json")),
            serde_json::to_string(&fixture).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        count += 1;
    }
    println!("wrote {count} fixtures to {}", dir.display());
    Ok(true)
}

/// `redfab check-fixtures DIR`: runs every fixture in DIR in the simulator and
/// checks its expectations. Writes the simulated traces next to the fixtures.
fn check_fixtures_command(args: &[String]) -> Result<bool, String> {
    let [dir] = args else {
        return Err("usage: redfab check-fixtures DIR".into());
    };
    let dir = Path::new(dir);
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".fixture.json"))
        .collect();
    paths.sort();
    let (mut passed, mut failed) = (0, 0);
    for path in &paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy().replace(".fixture.json", "");
        let run = redfab_core::trace::run_fixture(&read(path)?).map_err(|e| format!("{name}: {e}"))?;
        fs::write(
            dir.join(format!("{name}.sim.trace.json")),
            serde_json::to_string(&run.trace).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if run.failures.is_empty() {
            passed += 1;
        } else {
            failed += 1;
            println!("  FAIL  {name}: {}", run.failures.join("; "));
        }
    }
    println!("{}: {passed} fixtures meet their expectations in the simulator, {failed} do not", dir.display());
    Ok(failed == 0 && passed > 0)
}

/// `redfab pnr-random [--count N] [--seed S] [--dump DIR]`
///
/// Generates random combinational netlists and builds every one: placement,
/// routing, the layout-versus-netlist and strength checks, and the simulator
/// against a direct evaluation of the netlist on up to 16 input vectors. With
/// `--dump`, the netlists that fail are written there as Yosys JSON.
fn pnr_random_command(args: &[String]) -> Result<bool, String> {
    let mut count = 20usize;
    let mut seed = 1u64;
    let mut dump: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--count" => count = it.next().and_then(|n| n.parse().ok()).ok_or("--count needs a number")?,
            "--seed" => seed = it.next().and_then(|n| n.parse().ok()).ok_or("--seed needs a number")?,
            "--dump" => dump = Some(PathBuf::from(it.next().ok_or("--dump needs a directory")?)),
            other => {
                return Err(format!("usage: redfab pnr-random [--count N] [--seed S] [--dump DIR] (got {other:?})"));
            }
        }
    }
    if let Some(dir) = &dump {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut failures = 0;
    let mut cells = 0;
    let mut tiles = 0;
    for i in 0..count {
        let mut rng = redfab_core::tiletest::Rng::new(seed * 100_000 + i as u64);
        // Mostly small, every fifth one larger.
        let gates = if i % 5 == 4 { 20 + rng.below(21) } else { 1 + rng.below(16) };
        let netlist = redfab_core::pnrtest::random_netlist(&mut rng, gates);
        let name = format!("random_{seed}_{i:02}");
        match redfab_core::pnrtest::build_and_verify(&netlist, &name, 16, &mut rng) {
            Ok(built) => {
                let line = built.report.lines().find(|l| l.contains("tiles:")).unwrap_or_default().trim().to_string();
                println!("  ok    {name}: {} cells, {line}", netlist.cells.len());
                cells += netlist.cells.len();
                tiles += built.design.blocks.len();
            }
            Err(e) => {
                failures += 1;
                println!("  FAIL  {name}: {e}");
                if let Some(dir) = &dump {
                    let path = dir.join(format!("{name}.json"));
                    fs::write(&path, netlist.to_yosys_json("top")).map_err(|e| format!("{}: {e}", path.display()))?;
                    println!("        netlist written to {}", path.display());
                }
            }
        }
    }
    println!(
        "{} of {count} random netlists build and compute what they should ({cells} cells, {tiles} blocks)",
        count - failures
    );
    Ok(failures == 0)
}

/// `redfab layout MAPPED.json [--top NAME]`: places and routes a mapped netlist
/// and prints the layout as one character per tile (I input, O output, G gate,
/// 1/0 constants, box drawing for wires, double lines for repeaters, ╬ crossing).
fn layout_command(args: &[String]) -> Result<bool, String> {
    let (mapped, top) = match args {
        [mapped] => (mapped, "top"),
        [mapped, flag, top] if flag == "--top" => (mapped, top.as_str()),
        _ => return Err("usage: redfab layout MAPPED.json [--top NAME]".into()),
    };
    let library = redfab_core::tile::Library::builtin()?;
    let netlist = redfab_core::netlist::Netlist::from_yosys_json(&read(Path::new(mapped))?, top, &library)?;
    match redfab_core::build::place_and_route(&netlist) {
        Ok((routed, cols)) => {
            print!("{}", redfab_core::route::render(&routed.layout));
            println!("({cols} routing columns between gate columns)");
            Ok(true)
        }
        Err(e) => {
            // Show the placement alone, with one routing column.
            let placement = redfab_core::place::place(&netlist, 1)?;
            let mut layout = redfab_core::layout::Layout::default();
            for (c, &cell) in placement.gates.iter().enumerate() {
                layout.tiles.insert(
                    cell,
                    redfab_core::layout::Placed::Gate { cell: c, pin_edges: placement.pin_edges[c].clone() },
                );
            }
            for (i, &cell) in placement.inputs.iter().enumerate() {
                layout.tiles.insert(cell, redfab_core::layout::Placed::Input { port: i, read: true });
            }
            for (o, &cell) in placement.outputs.iter().enumerate() {
                layout.tiles.insert(cell, redfab_core::layout::Placed::Output { port: o });
            }
            for &(net, value, cell) in &placement.ties {
                let placed = if value {
                    redfab_core::layout::Placed::TieHigh { net }
                } else {
                    redfab_core::layout::Placed::TieLow { net }
                };
                layout.tiles.insert(cell, placed);
            }
            print!("{}", redfab_core::route::render(&layout));
            println!("routing failed: {e}");
            Ok(false)
        }
    }
}

/// `redfab build SOURCE.sv [--top NAME] -o DIR`
///
/// Synthesizes the design to the tile library, proves the mapped netlist equals
/// the source, lays it out, computes its settled state and checks the layout
/// against the netlist. Writes `design.json`, `world.nbt` and `report.txt` to DIR.
fn build_command(args: &[String]) -> Result<bool, String> {
    let mut source = None;
    let mut top = "top".to_string();
    let mut out = None;
    let mut fault = redfab_core::build::Break::None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--top" => top = it.next().ok_or("--top needs a module name")?.clone(),
            "-o" => out = Some(PathBuf::from(it.next().ok_or("-o needs a directory")?)),
            // A fault built in on purpose: the design is written out and the build fails.
            "--break" => {
                fault = match it.next().map(String::as_str) {
                    Some("hold") => redfab_core::build::Break::Hold,
                    _ => return Err("--break needs hold".into()),
                }
            }
            other => source = Some(PathBuf::from(other)),
        }
    }
    let (Some(source), Some(out)) = (source, out) else {
        return Err("usage: redfab build SOURCE.sv|MAPPED.json [--top NAME] [--break hold] -o DIR".into());
    };
    let name = out.file_name().and_then(|n| n.to_str()).unwrap_or("design").to_string();

    let library = redfab_core::tile::Library::builtin()?;
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let write = |file: &str, data: &[u8]| {
        fs::write(out.join(file), data).map_err(|e| format!("{}: {e}", out.join(file).display()))
    };
    for stale in ["mapped.json", "mapped.v", "design.json", "world.nbt", "report.txt"] {
        let _ = fs::remove_file(out.join(stale));
    }
    // A mapped netlist (as `pnr-random --dump` writes) skips synthesis and the proof.
    // With one, or with a fault built in, the layout is written even when it fails
    // the checks, so it can be examined or run.
    let from_json = source.extension().is_some_and(|e| e == "json");
    if from_json {
        write("mapped.json", read(&source)?.as_bytes())?;
    } else {
        write("top.sv", read(&source)?.as_bytes())?;
        write("cells.lib", library.liberty().as_bytes())?;
        write("cells.v", library.verilog_models().as_bytes())?;
        write("map.ys", redfab_core::synth::MAP_SCRIPT.replace("{top}", &top).as_bytes())?;
        write("equiv.ys", redfab_core::synth::EQUIV_SCRIPT.replace("{top}", &top).as_bytes())?;
        run_yosys(&out, "map.ys").map_err(|e| format!("synthesis failed: {e}"))?;
        run_yosys(&out, "equiv.ys")
            .map_err(|e| format!("the mapped netlist is NOT proven equal to the source: {e}"))?;
    }
    if from_json || fault != redfab_core::build::Break::None {
        let mapped = read(&out.join("mapped.json"))?;
        let netlist = redfab_core::netlist::Netlist::from_yosys_json(&mapped, &top, &library)?;
        let (built, problems) = redfab_core::build::build_unchecked(&netlist, &name, fault)?;
        write("design.json", built.design.to_json().as_bytes())?;
        write("world.nbt", &built.structure)?;
        print!("{}", built.report);
        for problem in &problems {
            println!("  problem: {problem}");
        }
        return Ok(problems.is_empty());
    }

    let built = redfab_core::build::build(&read(&out.join("mapped.json"))?, &top, &name)?;
    write("design.json", built.design.to_json().as_bytes())?;
    write("world.nbt", &built.structure)?;
    let report = format!("{}  mapped netlist proven equal to the source\n", built.report);
    write("report.txt", report.as_bytes())?;
    print!("{report}");
    Ok(true)
}

/// Runs the WebAssembly build of Yosys (under Node) on the files in `dir`.
fn run_yosys(dir: &Path, script: &str) -> Result<(), String> {
    let runner = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../synth/yosys.mjs");
    let output = std::process::Command::new("node")
        .arg(&runner)
        .arg(dir)
        .arg(script)
        .output()
        .map_err(|e| format!("could not run node: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}\n(full log: {})",
            String::from_utf8_lossy(&output.stderr).trim(),
            dir.join(format!("{script}.log")).display()
        ))
    }
}

/// `redfab characterize [DIR] [--shuffles N] [--margin M] [--chosen NAME] [--tables]`
///
/// Characterizes every flip-flop tile: replays its sweep fixtures in the
/// simulator, and N more times (20 unless told otherwise) under shuffled
/// same-tick orders so that a trial decided by ordering counts as neither
/// captured nor missed; derives setup, hold,
/// clock-to-output and the minimum clock high and low times, and compares them
/// with the tile's `timing` line. With DIR, the fixtures there must be the ones
/// generated here (so the game ran the same sweeps). With `--chosen`, that tile
/// must also pass the margin rule.
fn characterize_command(args: &[String]) -> Result<bool, String> {
    let mut dir: Option<PathBuf> = None;
    let mut shuffles = 20u32;
    let mut margin = 2i32;
    let mut chosen: Option<String> = None;
    let mut tables = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--shuffles" => shuffles = it.next().and_then(|n| n.parse().ok()).ok_or("--shuffles needs a number")?,
            "--margin" => margin = it.next().and_then(|n| n.parse().ok()).ok_or("--margin needs a number")?,
            "--chosen" => chosen = Some(it.next().ok_or("--chosen needs a tile name")?.clone()),
            "--tables" => tables = true,
            other => dir = Some(PathBuf::from(other)),
        }
    }
    let library = redfab_core::tile::Library::builtin()?;
    let mut ok = true;
    for variant in redfab_core::tile::Variant::all() {
        if !matches!(variant.kind, redfab_core::tile::Kind::Dff | redfab_core::tile::Kind::DffAlt) {
            continue;
        }
        if let Some(dir) = &dir {
            for fixture in redfab_core::dfftest::fixtures(&variant, &library) {
                let path = dir.join(format!("{}.fixture.json", fixture.name));
                let on_disk: serde_json::Value =
                    serde_json::from_str(&read(&path)?).map_err(|e| format!("{}: {e}", path.display()))?;
                if on_disk != fixture.json {
                    println!("  FAIL  {}: the fixture in {} is not the one generated now", fixture.name, dir.display());
                    ok = false;
                }
            }
        }
        let c = redfab_core::dfftest::characterize(&variant, &library, shuffles)?;
        let is_chosen = chosen.as_deref() == Some(variant.kind.name());
        let problems = redfab_core::dfftest::check(&c, &variant, &library, is_chosen, margin);
        println!("{} ({} edge):", c.name, if c.falling { "falling" } else { "rising" });
        if tables {
            for table in &c.tables {
                println!("{table}");
            }
        }
        let timing: Vec<String> = c.timing().iter().map(|(k, v)| format!("{k} {v}")).collect();
        println!(
            "  measured: {}; capture boundary between offsets {:?} and {:?}; ordering-dependent offsets {:?}",
            timing.join(", "),
            c.captured_until,
            c.ignored_from,
            c.ordering_zone
        );
        for f in &c.findings {
            println!("  note  {f}");
        }
        for d in &c.defects {
            println!("  defect  {d}");
        }
        for p in &problems {
            println!("  FAIL  {p}");
        }
        ok &= problems.is_empty();
    }
    Ok(ok)
}

/// `redfab stress DESIGN.json [--spacing N]`
///
/// Toggles each input of a combinational design on its own, sixteen times with N
/// ticks between changes, and reports the torch that turned off most often
/// within sixty ticks. The game burns a torch out on its eighth.
fn stress_command(args: &[String]) -> Result<bool, String> {
    let mut design = None;
    let mut spacing = 8u32;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--spacing" => spacing = it.next().and_then(|n| n.parse().ok()).ok_or("--spacing needs a number")?,
            other => design = Some(PathBuf::from(other)),
        }
    }
    let path = design.ok_or("usage: redfab stress DESIGN.json [--spacing N]")?;
    let design = redfab_core::build::Design::from_json(&read(&path)?)?;
    let (worst, pos, walk) = redfab_core::timing::worst_stress(&design, spacing)?;
    println!(
        "{}: with changes every {spacing} ticks, the torch at {} turns off {worst} times within sixty ticks ({walk}); the game burns out at 8, the budget is {}",
        design.name,
        pos.map_or("-".to_string(), |p| p.to_string()),
        redfab_core::timing::BURNOUT_BUDGET
    );
    Ok(worst <= redfab_core::timing::BURNOUT_BUDGET)
}

/// `redfab timing DESIGN.json`
///
/// Prints the timing report of a built design: for each output, the earliest
/// and latest game tick after an input change at which its lamp can change.
fn timing_command(args: &[String]) -> Result<bool, String> {
    let (design, paths) = match args {
        [design] => (design, false),
        [design, flag] if flag == "--paths" => (design, true),
        _ => return Err("usage: redfab timing DESIGN.json [--paths]".into()),
    };
    let design = redfab_core::build::Design::from_json(&read(Path::new(design))?)?;
    print!("{}", timing_report(&design));
    if paths {
        print!("{}", worst_paths(&design));
    }
    Ok(true)
}

fn timing_report(design: &redfab_core::build::Design) -> String {
    let timing = &design.timing;
    if let Some(seq) = &timing.sequential {
        let mut out = format!(
            "design {}: clock {} ({} edge), period {} game ticks, high {}; inputs change {} ticks after the edge (allowed {} to {}); outputs read at {}\n",
            design.name,
            seq.clock,
            seq.active,
            seq.period,
            seq.high,
            seq.input_change_at,
            seq.input_change_from,
            seq.input_change_by,
            seq.sample_at
        );
        out += "  flip-flop                         clock at   D from flip-flops   D from inputs   hold slack   setup slack\n";
        for f in &seq.flip_flops {
            let opt = |v: Option<i32>| v.map_or("-".to_string(), |v| v.to_string());
            let range = |r: Option<(u32, u32)>| r.map_or("-".to_string(), |(a, b)| format!("{a} to {b}"));
            out += &format!(
                "  {:<32} {:>8}   {:>17}   {:>13}   {:>10}   {:>11}\n",
                f.cell.chars().rev().take(30).collect::<String>().chars().rev().collect::<String>(),
                f.clock_arrival,
                range(f.d_from_flip_flops),
                range(f.d_from_inputs),
                opt(f.hold_slack),
                opt(f.setup_slack)
            );
        }
        for (name, t) in &timing.outputs {
            out += &format!("  output {name:<12} {:>4} to {:>4} gt after the edge\n", t.min, t.max);
        }
        if !seq.hold_violations.is_empty() {
            out += &format!("  HOLD VIOLATED at {}\n", seq.hold_violations.join(", "));
        }
        return out;
    }
    let mut out = format!(
        "design {}: outputs settle within {} game ticks of an input change (bound by path{})\n",
        design.name,
        design.settle_ticks,
        if timing.exhaustive { "; exact figures over every pair of input combinations" } else { "" }
    );
    out += "  output        by path          exact\n";
    for (name, t) in &timing.outputs {
        let exact = match t.exact {
            Some((lo, hi)) => format!("{lo:>4} to {hi:>4} gt"),
            None if timing.exhaustive => "never changes".to_string(),
            None => "-".to_string(),
        };
        out += &format!("  {name:<12} {:>4} to {:>4} gt   {exact}\n", t.min, t.max);
    }
    out
}

fn worst_paths(design: &redfab_core::build::Design) -> String {
    let mut out = String::new();
    for (name, t) in &design.timing.outputs {
        out += &format!("{name}: worst case {} gt\n", t.max);
        for step in &t.path {
            out += &format!("    {step}\n");
        }
    }
    out
}

/// `redfab timing-measure DESIGN.json [--multi N] [--seed S]`
///
/// Runs the design in the simulator through every single-bit input change from
/// every input combination (a sample when there are more than ten inputs) and N
/// random changes of several bits, and compares when the lamps changed with the
/// design's timing report. Fails if the report is not tight.
fn timing_measure_command(args: &[String]) -> Result<bool, String> {
    let mut design = None;
    let mut multi = 256usize;
    let mut seed = 1u64;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--multi" => multi = it.next().and_then(|n| n.parse().ok()).ok_or("--multi needs a number")?,
            "--seed" => seed = it.next().and_then(|n| n.parse().ok()).ok_or("--seed needs a number")?,
            other => design = Some(PathBuf::from(other)),
        }
    }
    let path = design.ok_or("usage: redfab timing-measure DESIGN.json [--multi N] [--seed S]")?;
    let design = redfab_core::build::Design::from_json(&read(&path)?)?;
    let measurement = redfab_core::timing::measure(&design, 10, multi, seed)?;
    print!("{}", timing_report(&design));
    for (name, seen) in &measurement.seen {
        match seen {
            Some((lo, hi)) => println!("  {name:<12} simulator shows {lo:>4} to {hi:>4} gt"),
            None => println!("  {name:<12} simulator shows no change"),
        }
    }
    for p in &measurement.problems {
        println!("  FAIL  {p}");
    }
    println!(
        "{}: the timed model and the report {} the block simulator over {} input changes",
        design.name,
        if measurement.problems.is_empty() { "agree with" } else { "do NOT agree with" },
        measurement.changes
    );
    Ok(measurement.problems.is_empty())
}

/// `redfab extract DESIGN.json`
///
/// Prints the circuit read back from a design's blocks: every net with its
/// tiles, and every torch, repeater, lamp and source with what it reads and drives.
fn extract_command(args: &[String]) -> Result<bool, String> {
    let [design] = args else { return Err("usage: redfab extract DESIGN.json".into()) };
    let design = redfab_core::build::Design::from_json(&read(Path::new(design))?)?;
    let circuit = redfab_core::extract::extract(&design.world()?, &design.probes()?);
    let tile =
        |p: &redfab_core::geom::Pos| format!("({},{})", p.x / redfab_core::tile::PITCH, p.z / redfab_core::tile::PITCH);
    for (i, net) in circuit.nets.iter().enumerate() {
        let mut tiles: Vec<String> = net.iter().map(tile).collect();
        tiles.dedup();
        println!("net {i}: {} dust in tiles {}", net.len(), tiles.join(" "));
    }
    let list = |what: &str, items: &[redfab_core::extract::Component]| {
        for c in items {
            println!("{what} at {} in tile {}: reads {:?}, drives {:?}", c.pos, tile(&c.pos), c.reads, c.drives);
        }
    };
    list("torch", &circuit.torches);
    list("repeater", &circuit.repeaters);
    list("lamp", &circuit.lamps);
    list("source", &circuit.sources);
    Ok(true)
}

/// `redfab lvs DESIGN.json MAPPED.json [--top NAME]`: checks a design file's
/// blocks against a netlist. Prints each mismatch and fails if there is any.
fn lvs_command(args: &[String]) -> Result<bool, String> {
    let (design, mapped, top) = match args {
        [design, mapped] => (design, mapped, "top"),
        [design, mapped, flag, top] if flag == "--top" => (design, mapped, top.as_str()),
        _ => return Err("usage: redfab lvs DESIGN.json MAPPED.json [--top NAME]".into()),
    };
    let problems = redfab_core::build::check_design(&read(Path::new(design))?, &read(Path::new(mapped))?, top)?;
    for problem in &problems {
        println!("{problem}");
    }
    if problems.is_empty() {
        println!("layout matches the netlist");
    }
    Ok(problems.is_empty())
}

/// `redfab simulate DESIGN.json VECTORS.json`: applies input vectors in the
/// simulator and prints the outputs as JSON.
/// `redfab simulate DESIGN.json VECTORS.json [--shuffle SEED]`
///
/// Applies the vectors in the simulator and prints the outputs after each. For a
/// clocked design each vector is one clock cycle; `--shuffle` runs the ticks that
/// fall due together in a random order within each priority.
fn simulate_command(args: &[String]) -> Result<bool, String> {
    let (design, vectors, shuffle) = match args {
        [design, vectors] => (design, vectors, None),
        [design, vectors, flag, seed] if flag == "--shuffle" => {
            (design, vectors, Some(seed.parse::<u64>().map_err(|_| "--shuffle needs a number")?))
        }
        _ => return Err("usage: redfab simulate DESIGN.json VECTORS.json [--shuffle SEED]".into()),
    };
    println!("{}", redfab_core::build::simulate_with(&read(Path::new(design))?, &read(Path::new(vectors))?, shuffle)?);
    Ok(true)
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}
