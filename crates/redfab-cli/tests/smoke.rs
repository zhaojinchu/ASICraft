//! Every source under tests/smoke/ that carries an expectation is built with
//! the real `redfab build` (synthesis under Node, the proof, placement): a
//! refusal must be the exact message the source's header states, a build must
//! succeed and its report must contain the lines the header lists.
//!
//! Header lines:
//!   `// error: TEXT`   the build fails and prints exactly these lines after `error: `
//!   `// expect: ok`    the build succeeds
//!   `// report: TEXT`  report.txt contains this line (substring match)
//!   `// top: NAME`     passed as --top

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn smoke_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/smoke")
}

fn out_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("smoke").join(name);
    let _ = fs::remove_dir_all(&dir);
    dir
}

struct Expectation {
    top: String,
    errors: Vec<String>,
    ok: bool,
    report: Vec<String>,
}

fn expectation(source: &str) -> Option<Expectation> {
    let mut e = Expectation { top: "top".to_string(), errors: Vec::new(), ok: false, report: Vec::new() };
    let mut any = false;
    for line in source.lines().take_while(|l| l.starts_with("//")) {
        let line = line.trim_start_matches('/').trim_start();
        if let Some(text) = line.strip_prefix("error: ") {
            e.errors.push(text.to_string());
            any = true;
        } else if let Some(text) = line.strip_prefix("expect: ") {
            e.ok = text.trim() == "ok";
            any = true;
        } else if let Some(text) = line.strip_prefix("report: ") {
            e.report.push(text.to_string());
        } else if let Some(text) = line.strip_prefix("top: ") {
            e.top = text.trim().to_string();
        }
    }
    any.then_some(e)
}

fn build(source: &Path, top: &str, out: &Path) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_redfab"))
        .args(["build", source.to_str().unwrap(), "--top", top, "-o", out.to_str().unwrap()])
        .output()
        .expect("the redfab binary runs");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// The lines after `error: ` on stderr, without the progress lines and the log path.
fn error_lines(stderr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut started = false;
    for line in stderr.lines() {
        if let Some(rest) = line.strip_prefix("error: ") {
            started = true;
            out.push(rest.to_string());
        } else if started && !line.starts_with("(full log:") && !line.starts_with('[') {
            out.push(line.to_string());
        }
    }
    out
}

#[test]
fn every_smoke_source_builds_or_is_refused_as_its_header_says() {
    let mut sources: Vec<PathBuf> = fs::read_dir(smoke_dir())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "sv"))
        .collect();
    sources.sort();
    let mut checked = 0;
    let mut failures = Vec::new();
    for source in &sources {
        let name = source.file_stem().unwrap().to_str().unwrap().to_string();
        let Some(want) = expectation(&fs::read_to_string(source).unwrap()) else { continue };
        checked += 1;
        let out = out_dir(&name);
        let (ok, stdout, stderr) = build(source, &want.top, &out);
        if want.ok {
            if !ok {
                failures.push(format!("{name}: expected to build, got\n{stderr}"));
                continue;
            }
            let report = fs::read_to_string(out.join("report.txt")).unwrap_or_default();
            for line in &want.report {
                if !report.contains(line.as_str()) && !stdout.contains(line.as_str()) {
                    failures.push(format!("{name}: the report lacks {line:?}:\n{report}"));
                }
            }
        } else {
            if ok {
                failures.push(format!("{name}: expected a refusal, but it built:\n{stdout}"));
                continue;
            }
            let got = error_lines(&stderr);
            if got != want.errors {
                failures.push(format!("{name}: expected\n  {}\ngot\n  {}", want.errors.join("\n  "), got.join("\n  ")));
            }
        }
    }
    assert!(checked >= 18, "only {checked} smoke sources carry an expectation");
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Building a directory from its own mapped.json must not delete the input first.
#[test]
fn a_build_from_the_output_directorys_own_netlist_works() {
    let out = out_dir("rebuild");
    let (ok, _, stderr) = build(&smoke_dir().join("nor2.sv"), "nor2", &out);
    assert!(ok, "{stderr}");
    let first = fs::read_to_string(out.join("design.json")).unwrap();
    let (ok, _, stderr) = build(&out.join("mapped.json"), "nor2", &out);
    assert!(ok, "{stderr}");
    assert_eq!(fs::read_to_string(out.join("design.json")).unwrap(), first);
}

fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// Runs `redfab build` on nor2.sv with the given environment and working directory.
fn build_nor2_with(exe: &Path, cwd: &Path, out: &Path, env: &[(&str, &Path)], unset: &[&str]) -> (bool, String) {
    let mut command = Command::new(exe);
    command.current_dir(cwd).args([
        "build",
        smoke_dir().join("nor2.sv").to_str().unwrap(),
        "--top",
        "nor2",
        "-o",
        out.to_str().unwrap(),
    ]);
    for name in unset {
        command.env_remove(name);
    }
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().expect("the redfab binary runs");
    (output.status.success(), String::from_utf8_lossy(&output.stderr).to_string())
}

/// Synthesis finds its Yosys runner at $REDFAB_SYNTH or beside an ancestor of the
/// executable, and says in plain words what is missing otherwise: the file, Node,
/// or the node_modules the runner loads Yosys from.
#[test]
fn the_yosys_runner_is_found_by_env_var_or_beside_the_executable() {
    let exe = Path::new(env!("CARGO_BIN_EXE_redfab"));
    let runner = checkout().join("synth/yosys.mjs");
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("synth");
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();

    // REDFAB_SYNTH names the runner: an ordinary build.
    let (ok, stderr) = build_nor2_with(exe, &scratch, &scratch.join("env"), &[("REDFAB_SYNTH", &runner)], &[]);
    assert!(ok, "{stderr}");

    // REDFAB_SYNTH names nothing.
    let missing = scratch.join("nowhere/yosys.mjs");
    let (ok, stderr) = build_nor2_with(exe, &scratch, &scratch.join("missing"), &[("REDFAB_SYNTH", &missing)], &[]);
    assert!(!ok);
    assert!(stderr.contains(&format!("REDFAB_SYNTH is set to {}, which is not a file", missing.display())), "{stderr}");

    // No node on the PATH.
    let empty = scratch.join("empty-path");
    fs::create_dir_all(&empty).unwrap();
    let (ok, stderr) = build_nor2_with(exe, &scratch, &scratch.join("no-node"), &[("PATH", &empty)], &["REDFAB_SYNTH"]);
    assert!(!ok);
    assert!(stderr.contains("synthesis needs Node on the PATH"), "{stderr}");

    // A copy of the binary with a copy of synth/ beside it, outside the checkout
    // (the system temp directory, which no node_modules sits above): the lookup
    // finds that copy of the runner by walking up from the executable, and node
    // cannot load Yosys from there. The message names the copy, which proves it
    // was the exe-relative lookup and not the fallback to the checkout.
    let far = std::env::temp_dir().join(format!("redfab-smoke-{}", std::process::id()));
    let _ = fs::remove_dir_all(&far);
    fs::create_dir_all(far.join("bin")).unwrap();
    fs::create_dir_all(far.join("synth")).unwrap();
    let far_exe = far.join("bin").join(exe.file_name().unwrap());
    fs::copy(exe, &far_exe).unwrap();
    let far_runner = far.join("synth/yosys.mjs");
    fs::copy(&runner, &far_runner).unwrap();
    let (ok, stderr) = build_nor2_with(&far_exe, &far, &far.join("out"), &[], &["REDFAB_SYNTH"]);
    let _ = fs::remove_dir_all(&far);
    assert!(!ok);
    assert!(
        stderr.contains(&format!("the Yosys runner {} cannot load @yowasp/yosys", far_runner.display()))
            && stderr.contains("run npm ci"),
        "{stderr}"
    );
}

#[test]
fn help_lists_every_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_redfab")).output().unwrap();
    let usage = String::from_utf8_lossy(&output.stdout).to_string();
    for command in [
        "build",
        "simulate",
        "timing",
        "layout",
        "extract",
        "lvs",
        "stress",
        "timing-measure",
        "structure",
        "structure-pattern",
        "check-traces",
        "list-mutations",
        "replay",
        "check-fixtures",
        "tile-adjacency",
        "tile-fixtures",
        "characterize",
        "pnr-random",
        "help",
        "--version",
    ] {
        assert!(usage.lines().any(|l| l.trim_start().starts_with(command)), "usage lacks {command}:\n{usage}");
    }
    let help = Command::new(env!("CARGO_BIN_EXE_redfab")).arg("help").output().unwrap();
    assert!(help.status.success());
    assert_eq!(String::from_utf8_lossy(&help.stdout), usage);
}
