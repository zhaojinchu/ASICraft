//! What a design may not contain, said in plain words before any placing is
//! done: the lint of the netlist Yosys writes right after `proc` and `memory`
//! (`pre.json` in [`crate::synth::MAP_SCRIPT`]), the translation of a Yosys
//! failure into one line, and the comparison of flip-flop initial values
//! between the source and the mapped netlist, which the equivalence proof
//! does not cover.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::netlist::bit_name;

/// A module of a Yosys JSON netlist, with the names its bits go by.
struct Module {
    cells: serde_json::Map<String, Value>,
    ports: serde_json::Map<String, Value>,
    /// Bit number to the best public name for it (`count[2]`).
    names: BTreeMap<u64, String>,
}

impl Module {
    fn read(json: &Value, top: &str) -> Result<Module, String> {
        let module = json["modules"].get(top).ok_or_else(|| format!("module {top:?} not found in the netlist"))?;
        let empty = serde_json::Map::new();
        let cells = module["cells"].as_object().unwrap_or(&empty).clone();
        let ports = module["ports"].as_object().unwrap_or(&empty).clone();
        let mut names: BTreeMap<u64, String> = BTreeMap::new();
        let mut consider = |name: &str, entry: &Value, hidden: bool| {
            if hidden || name.starts_with('$') {
                return;
            }
            let bits = entry["bits"].as_array().cloned().unwrap_or_default();
            for (i, value) in bits.iter().enumerate() {
                if let Some(raw) = value.as_u64() {
                    let label = bit_name(name, entry, i, bits.len());
                    let slot = names.entry(raw).or_insert_with(|| label.clone());
                    if label.len() < slot.len() || (label.len() == slot.len() && label < *slot) {
                        *slot = label;
                    }
                }
            }
        };
        for (name, entry) in module["netnames"].as_object().unwrap_or(&empty) {
            consider(name, entry, entry["hide_name"].as_u64().unwrap_or(0) != 0);
        }
        for (name, entry) in &ports {
            consider(name, entry, false);
        }
        Ok(Module { cells, ports, names })
    }

    /// The name of a bit, or how Yosys refers to it.
    fn name(&self, bit: &Value) -> String {
        match bit.as_u64() {
            Some(raw) => self.names.get(&raw).cloned().unwrap_or_else(|| format!("net {raw}")),
            None => bit.as_str().unwrap_or("?").to_string(),
        }
    }

    /// The first bit of a cell's pin.
    fn pin<'b>(&self, cell: &'b Value, pin: &str) -> Option<&'b Value> {
        cell["connections"][pin].as_array().and_then(|bits| bits.first())
    }

    /// Where the source declares the cell, for the message.
    fn src(&self, cell: &Value) -> String {
        match cell["attributes"]["src"].as_str() {
            Some(src) => {
                // "top.sv:4.9-4.28|top.sv:2.5": the first place, as file:line.
                let first = src.split('|').next().unwrap_or(src);
                let (file, rest) = first.rsplit_once(':').unwrap_or((first, ""));
                let line = rest.split('.').next().unwrap_or(rest);
                format!(" (at {file}:{line})")
            }
            None => String::new(),
        }
    }

    fn input_bits(&self) -> BTreeSet<u64> {
        let mut bits = BTreeSet::new();
        for port in self.ports.values() {
            if port["direction"].as_str() == Some("input") {
                bits.extend(port["bits"].as_array().into_iter().flatten().filter_map(Value::as_u64));
            }
        }
        bits
    }
}

/// Cell types that hold state on a clock edge (what the flow can build).
fn is_clocked_ff(kind: &str) -> bool {
    matches!(kind, "$dff" | "$dffe" | "$sdff" | "$sdffe" | "$sdffce" | "$adff" | "$adffe" | "$aldff" | "$aldffe")
        || matches!(kind, "$dffsr" | "$dffsre")
}

/// Checks the netlist of the top module as it is right after `proc` and
/// `memory`, before anything is optimised or mapped, and says in one sentence
/// per problem what the design uses that cannot be built, and what to write
/// instead: latches, asynchronous resets, tristate and inout, `$display`,
/// assertions, memories that did not become registers, several clocks, derived
/// clocks, and a clock used as data.
pub fn lint_pre_netlist(pre_json: &str, top: &str) -> Result<(), String> {
    let json: Value = serde_json::from_str(pre_json).map_err(|e| format!("netlist JSON: {e}"))?;
    let m = Module::read(&json, top)?;
    let mut problems: Vec<String> = Vec::new();

    for (name, port) in &m.ports {
        if port["direction"].as_str() == Some("inout") {
            problems.push(format!(
                "port `{name}` is inout: inout and tristate ports are not supported; use separate input and output ports"
            ));
        }
    }

    // Per clock bit: the registers it clocks, as (name, where declared).
    let mut clocks: BTreeMap<u64, Vec<(String, String)>> = BTreeMap::new();
    for cell in m.cells.values() {
        let kind = cell["type"].as_str().unwrap_or_default();
        let src = m.src(cell);
        let q = || m.pin(cell, "Q").map(|b| m.name(b)).unwrap_or_default();
        match kind {
            "$tribuf" => {
                let y = m.pin(cell, "Y").map(|b| m.name(b)).unwrap_or_default();
                problems.push(format!(
                    "`{y}` is driven with 'z, a tristate{src}: tristate drivers are not supported; drive every signal with 0 or 1"
                ));
            }
            "$dlatch" | "$adlatch" | "$dlatchsr" | "$sr" => {
                let q = q();
                problems.push(format!(
                    "`{q}` is a latch{src}: latches are not supported; assign `{q}` on every path of its always block, or make it a register with always_ff @(posedge clk)"
                ));
            }
            "$adff" | "$adffe" | "$aldff" | "$aldffe" | "$dffsr" | "$dffsre" => {
                let q = q();
                problems.push(format!(
                    "register `{q}` has an asynchronous reset{src}: asynchronous resets are not supported; write a synchronous one: always_ff @(posedge clk) if (rst) {q} <= 0; else ..."
                ));
            }
            "$print" => problems.push(format!(
                "$display and $write are not supported{src}: remove them; the design becomes a circuit, which cannot print"
            )),
            "$check" | "$assert" | "$assume" | "$cover" | "$live" | "$fair" => {
                problems.push(format!("assertions are not supported{src}: remove the assert"));
            }
            "$ff" => {
                let q = q();
                problems.push(format!(
                    "register `{q}` has no clock{src}: clock it with always_ff @(posedge clk)"
                ));
            }
            _ if kind.starts_with("$mem") => {
                let memid = cell["parameters"]["MEMID"].as_str().unwrap_or("?").trim_start_matches('\\');
                problems.push(format!(
                    "memory `{memid}` could not be turned into registers{src}: write it as an array written in one always_ff block and read with a plain index"
                ));
            }
            _ => {}
        }
        if is_clocked_ff(kind)
            && let Some(clk) = m.pin(cell, "CLK").and_then(Value::as_u64)
        {
            clocks.entry(clk).or_default().push((q(), src.clone()));
        }
    }

    // Clocks: one, an input, used for nothing else.
    let inputs = m.input_bits();
    if clocks.len() > 1 && clocks.keys().all(|c| inputs.contains(c)) {
        let names: Vec<String> = clocks.keys().map(|&c| m.name(&Value::from(c))).collect();
        problems.push(format!(
            "registers are clocked by {} different signals ({}): one clock only; clock every always_ff block from the same input",
            clocks.len(),
            names.join(", ")
        ));
    }
    for (&clk, registers) in &clocks {
        let clk_name = m.name(&Value::from(clk));
        if !inputs.contains(&clk) {
            let (register, src) = &registers[0];
            problems.push(format!(
                "register `{register}` is clocked by `{clk_name}`, which is not an input{src}: derived and gated clocks are not supported; clock every register straight from the clock input"
            ));
            continue;
        }
        let mut data_uses: Vec<String> = Vec::new();
        for cell in m.cells.values() {
            let kind = cell["type"].as_str().unwrap_or_default();
            if matches!(kind, "$print" | "$check" | "$assert" | "$assume" | "$cover" | "$live" | "$fair") {
                continue; // reported above; they trigger on the clock by nature
            }
            let directions = cell["port_directions"].as_object();
            for (pin, bits) in cell["connections"].as_object().into_iter().flatten() {
                if is_clocked_ff(kind) && pin == "CLK" {
                    continue;
                }
                let is_input = directions.is_none_or(|d| d[pin].as_str() != Some("output"));
                if is_input && bits.as_array().into_iter().flatten().any(|b| b.as_u64() == Some(clk)) {
                    let what = if is_clocked_ff(kind) {
                        format!("the {pin} of register `{}`", m.pin(cell, "Q").map(|b| m.name(b)).unwrap_or_default())
                    } else {
                        format!("{} logic{}", kind.trim_start_matches('$'), m.src(cell))
                    };
                    data_uses.push(what);
                }
            }
        }
        for (name, port) in &m.ports {
            if port["direction"].as_str() == Some("output")
                && port["bits"].as_array().into_iter().flatten().any(|b| b.as_u64() == Some(clk))
            {
                data_uses.push(format!("output `{name}`"));
            }
        }
        if !data_uses.is_empty() {
            problems.push(format!(
                "the clock `{clk_name}` is also used as data by {}: the clock input may only clock registers; use another input for data",
                data_uses.join(", ")
            ));
        }
    }

    // Combinational loops: a latch written as an always_comb that does not assign
    // on every path comes out as a mux feeding itself.
    if let Some(problem) = combinational_loop(&m) {
        problems.push(problem);
    }

    if problems.is_empty() { Ok(()) } else { Err(problems.join("\n")) }
}

/// A cycle through cells that hold no state, named by a public net on it.
fn combinational_loop(m: &Module) -> Option<String> {
    // Which cell drives each bit, for cells without state.
    let mut driver: BTreeMap<u64, &str> = BTreeMap::new();
    let mut inputs_of: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for (name, cell) in &m.cells {
        let kind = cell["type"].as_str().unwrap_or_default();
        if is_clocked_ff(kind)
            || kind.starts_with("$mem")
            || matches!(kind, "$dlatch" | "$adlatch" | "$dlatchsr" | "$sr" | "$ff" | "$print" | "$check")
        {
            continue;
        }
        let directions = cell["port_directions"].as_object();
        for (pin, bits) in cell["connections"].as_object().into_iter().flatten() {
            let output = directions.map_or(pin == "Y" || pin == "Q", |d| d[pin].as_str() == Some("output"));
            for bit in bits.as_array().into_iter().flatten().filter_map(Value::as_u64) {
                if output {
                    driver.insert(bit, name);
                } else {
                    inputs_of.entry(name).or_default().push(bit);
                }
            }
        }
    }
    // Depth-first search for a cell reachable from itself.
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Active,
        Done,
    }
    let mut marks: BTreeMap<&str, Mark> = BTreeMap::new();
    fn visit<'a>(
        cell: &'a str,
        driver: &BTreeMap<u64, &'a str>,
        inputs_of: &BTreeMap<&'a str, Vec<u64>>,
        marks: &mut BTreeMap<&'a str, Mark>,
        path: &mut Vec<u64>,
    ) -> Option<Vec<u64>> {
        marks.insert(cell, Mark::Active);
        for &bit in inputs_of.get(cell).into_iter().flatten() {
            if let Some(&next) = driver.get(&bit) {
                match marks.get(next) {
                    Some(Mark::Done) => continue,
                    Some(Mark::Active) => {
                        path.push(bit);
                        return Some(path.clone());
                    }
                    None => {
                        path.push(bit);
                        if let Some(cycle) = visit(next, driver, inputs_of, marks, path) {
                            return Some(cycle);
                        }
                        path.pop();
                    }
                }
            }
        }
        marks.insert(cell, Mark::Done);
        None
    }
    let cells: Vec<&str> = inputs_of.keys().copied().collect();
    for cell in cells {
        if marks.contains_key(cell) {
            continue;
        }
        let mut path = Vec::new();
        if let Some(cycle) = visit(cell, &driver, &inputs_of, &mut marks, &mut path) {
            let name = cycle
                .iter()
                .find_map(|bit| m.names.get(bit).cloned())
                .unwrap_or_else(|| m.name(&Value::from(cycle[0])));
            let src = cycle.iter().find_map(|bit| driver.get(bit)).map(|c| m.src(&m.cells[*c])).unwrap_or_default();
            return Some(format!(
                "`{name}` depends on itself through logic with no register in the loop{src}: an always_comb that leaves `{name}` unassigned on some path makes a latch, which is not supported; assign it on every path (an else branch or a default), or make it a register"
            ));
        }
    }
    None
}

/// Removes terminal colour codes.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// One line that says why a Yosys script failed, from its log: the first error
/// the frontend or a pass reported, in plain words where the error is a known
/// refusal.
pub fn yosys_failure(log: &str, script: &str) -> String {
    let clean = strip_ansi(log);
    let line = clean
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("ERROR:") || l.contains(": error: ") || l.starts_with("error: "));
    let Some(line) = line else {
        return format!("{script} failed; see its log");
    };
    if let Some(rest) = line.strip_prefix("error: ").or_else(|| line.strip_prefix("ERROR: ")) {
        if let Some(name) = rest.strip_suffix("' is not a valid top-level module").and_then(|r| r.strip_prefix('\'')) {
            return format!("no module named `{name}` in the source: name the module top or pass --top NAME");
        }
        if rest.contains("dffs with async set or reset are not supported") {
            return "asynchronous resets are not supported; write a synchronous one: always_ff @(posedge clk) if (rst) ..."
                .to_string();
        }
        if rest.contains("D latches are not supported") {
            return "latches are not supported; assign every signal on every path of its always block, or use always_ff"
                .to_string();
        }
        if rest.contains("contains memories or processes") {
            return "memories in this form are not supported; write them as arrays written in one always_ff block"
                .to_string();
        }
        if rest.contains("$tribuf") {
            return "tristate drivers ('z) are not supported; drive every signal with 0 or 1".to_string();
        }
        if let Some(task) = rest.strip_prefix("Can't resolve task name `\\").and_then(|r| r.strip_suffix("'.")) {
            return format!("`{task}` is not supported; remove it");
        }
        if rest == "Design elaboration failed; see full log for details" {
            return format!("{script} failed; see its log");
        }
        return rest.to_string();
    }
    line.replace(": error: ", ": ")
}

/// A flip-flop's initial value in a Yosys JSON module, by its public name bit.
fn initial_values(m: &Module, is_ff: &dyn Fn(&str) -> bool, module: &Value) -> BTreeMap<String, bool> {
    let mut inits: BTreeMap<u64, bool> = BTreeMap::new();
    let empty = serde_json::Map::new();
    for entry in module["netnames"].as_object().unwrap_or(&empty).values() {
        let init = entry["attributes"]["init"].as_str().unwrap_or_default();
        let bits = entry["bits"].as_array().cloned().unwrap_or_default();
        for (i, value) in bits.iter().enumerate() {
            if let Some(raw) = value.as_u64()
                && let Some(c) = init.chars().rev().nth(i)
                && (c == '0' || c == '1')
            {
                inits.insert(raw, c == '1');
            }
        }
    }
    let mut out = BTreeMap::new();
    for cell in m.cells.values() {
        let kind = cell["type"].as_str().unwrap_or_default();
        if !is_ff(kind) {
            continue;
        }
        for bit in cell["connections"]["Q"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
            if let Some(name) = m.names.get(&bit) {
                out.insert(name.clone(), inits.get(&bit).copied().unwrap_or(false));
            }
        }
    }
    out
}

/// The equivalence proof ignores initial values, so they are compared here:
/// every flip-flop of the mapped netlist must start at the value the source
/// gives the register bit it implements (`gold.json` is the source after
/// `proc`, `memory` and `setundef -init -zero`, so every register has one).
pub fn check_inits(gold_json: &str, mapped_json: &str, top: &str) -> Result<(), String> {
    let gold: Value = serde_json::from_str(gold_json).map_err(|e| format!("gold netlist JSON: {e}"))?;
    let mapped: Value = serde_json::from_str(mapped_json).map_err(|e| format!("mapped netlist JSON: {e}"))?;
    let gm = Module::read(&gold, top)?;
    let mm = Module::read(&mapped, top)?;
    let gold_inits = initial_values(&gm, &is_clocked_ff, &gold["modules"][top]);
    let mapped_inits = initial_values(&mm, &|k| k == "DFF", &mapped["modules"][top]);
    let mut problems = Vec::new();
    for (name, &init) in &mapped_inits {
        match gold_inits.get(name) {
            Some(&want) if want != init => problems.push(format!(
                "flip-flop {name} starts at {} in the mapped netlist but at {} in the source",
                u8::from(init),
                u8::from(want)
            )),
            Some(_) => {}
            None => problems.push(format!("flip-flop {name} of the mapped netlist is no register of the source")),
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(cells: &str, ports: &str, netnames: &str) -> String {
        format!(r#"{{"modules":{{"top":{{"ports":{{{ports}}},"cells":{{{cells}}},"netnames":{{{netnames}}}}}}}}}"#)
    }

    const CLK_D_Q: &str = r#""clk":{"direction":"input","bits":[2]},"d":{"direction":"input","bits":[3]},"q":{"direction":"output","bits":[4]}"#;

    #[test]
    fn a_clean_register_passes() {
        let json = module(
            r#""$f":{"type":"$dff","port_directions":{"CLK":"input","D":"input","Q":"output"},"connections":{"CLK":[2],"D":[3],"Q":[4]}}"#,
            CLK_D_Q,
            r#""q":{"hide_name":0,"bits":[4],"attributes":{"init":"0"}}"#,
        );
        assert_eq!(lint_pre_netlist(&json, "top"), Ok(()));
    }

    #[test]
    fn each_refusal_says_what_to_write_instead() {
        let cases: Vec<(String, &str)> = vec![
            (
                module(
                    r#""$f":{"type":"$adff","attributes":{"src":"top.sv:2.5"},"connections":{"ARST":[3],"CLK":[2],"D":[3],"Q":[4]}}"#,
                    CLK_D_Q,
                    r#""q":{"hide_name":0,"bits":[4]}"#,
                ),
                "register `q` has an asynchronous reset (at top.sv:2): asynchronous resets are not supported; write a synchronous one: always_ff @(posedge clk) if (rst) q <= 0; else ...",
            ),
            (
                module(
                    r#""$l":{"type":"$dlatch","connections":{"EN":[2],"D":[3],"Q":[4]}}"#,
                    CLK_D_Q,
                    r#""q":{"hide_name":0,"bits":[4]}"#,
                ),
                "`q` is a latch: latches are not supported; assign `q` on every path of its always block, or make it a register with always_ff @(posedge clk)",
            ),
            (
                module(
                    r#""$m":{"type":"$mux","port_directions":{"A":"input","B":"input","S":"input","Y":"output"},"connections":{"A":[4],"B":[3],"S":[2],"Y":[4]}}"#,
                    CLK_D_Q,
                    r#""q":{"hide_name":0,"bits":[4]}"#,
                ),
                "`q` depends on itself through logic with no register in the loop: an always_comb that leaves `q` unassigned on some path makes a latch, which is not supported; assign it on every path (an else branch or a default), or make it a register",
            ),
            (
                module(
                    r#""$t":{"type":"$tribuf","connections":{"A":[3],"EN":[2],"Y":[4]}}"#,
                    r#""oe":{"direction":"input","bits":[2]},"d":{"direction":"input","bits":[3]},"bus":{"direction":"inout","bits":[4]}"#,
                    "",
                ),
                "port `bus` is inout: inout and tristate ports are not supported; use separate input and output ports\n`bus` is driven with 'z, a tristate: tristate drivers are not supported; drive every signal with 0 or 1",
            ),
            (
                module(
                    r#""$p":{"type":"$print","attributes":{"src":"top.sv:4.9-4.28"},"connections":{"TRG":[2],"EN":["1"],"ARGS":[4]}}"#,
                    CLK_D_Q,
                    "",
                ),
                "$display and $write are not supported (at top.sv:4): remove them; the design becomes a circuit, which cannot print",
            ),
            (
                module(
                    r#""$c":{"type":"$check","attributes":{"src":"top.sv:4.9-4.25"},"connections":{"TRG":[2],"EN":["1"],"A":[3],"ARGS":[]}}"#,
                    CLK_D_Q,
                    "",
                ),
                "assertions are not supported (at top.sv:4): remove the assert",
            ),
            (
                module(
                    r#""$f1":{"type":"$dff","connections":{"CLK":[2],"D":[3],"Q":[4]}},"$f2":{"type":"$dff","connections":{"CLK":[5],"D":[3],"Q":[6]}}"#,
                    r#""clk1":{"direction":"input","bits":[2]},"clk2":{"direction":"input","bits":[5]},"d":{"direction":"input","bits":[3]},"q1":{"direction":"output","bits":[4]},"q2":{"direction":"output","bits":[6]}"#,
                    "",
                ),
                "registers are clocked by 2 different signals (clk1, clk2): one clock only; clock every always_ff block from the same input",
            ),
            (
                module(
                    r#""$f1":{"type":"$dff","connections":{"CLK":[2],"D":[5],"Q":[4]}},"$f2":{"type":"$dff","connections":{"CLK":[4],"D":[6],"Q":[3]}}"#,
                    r#""clk":{"direction":"input","bits":[2]},"q":{"direction":"output","bits":[3]}"#,
                    r#""half":{"hide_name":0,"bits":[4]}"#,
                ),
                "register `q` is clocked by `half`, which is not an input: derived and gated clocks are not supported; clock every register straight from the clock input",
            ),
            (
                module(
                    r#""$f":{"type":"$dff","connections":{"CLK":[2],"D":[3],"Q":[4]}},"$a":{"type":"$and","attributes":{"src":"top.sv:3.16-3.23"},"port_directions":{"A":"input","B":"input","Y":"output"},"connections":{"A":[2],"B":[3],"Y":[5]}}"#,
                    r#""clk":{"direction":"input","bits":[2]},"d":{"direction":"input","bits":[3]},"q":{"direction":"output","bits":[4]},"y":{"direction":"output","bits":[5]}"#,
                    "",
                ),
                "the clock `clk` is also used as data by and logic (at top.sv:3): the clock input may only clock registers; use another input for data",
            ),
            (
                module(r#""$m":{"type":"$mem_v2","parameters":{"MEMID":"\\mem"},"connections":{}}"#, CLK_D_Q, ""),
                "memory `mem` could not be turned into registers: write it as an array written in one always_ff block and read with a plain index",
            ),
        ];
        for (json, want) in cases {
            assert_eq!(lint_pre_netlist(&json, "top").unwrap_err(), want);
        }
    }

    #[test]
    fn yosys_failures_become_one_plain_line() {
        let log = "\u{1b}[91merror: \u{1b}[0m\u{1b}[1m'top' is not a valid top-level module\u{1b}[0m\nBuild failed: 1 error\nERROR: Design elaboration failed; see full log for details\n";
        assert_eq!(
            yosys_failure(log, "map.ys"),
            "no module named `top` in the source: name the module top or pass --top NAME"
        );
        let log = "\u{1b}[36mtop.sv\u{1b}[0m:\u{1b}[96m2\u{1b}[0m\u{1b}[96m:18\u{1b}[0m: \u{1b}[91merror: \u{1b}[0m\u{1b}[1mexpected ';'\u{1b}[0m\n";
        assert_eq!(yosys_failure(log, "map.ys"), "top.sv:2:18: expected ';'");
        assert_eq!(
            yosys_failure(
                "6. Executing DFFLEGALIZE pass\nERROR: FF top.$x (type $_DFF_PP0_) cannot be legalized: dffs with async set or reset are not supported\n",
                "map.ys"
            ),
            "asynchronous resets are not supported; write a synchronous one: always_ff @(posedge clk) if (rst) ..."
        );
        assert_eq!(yosys_failure("all fine\n", "equiv.ys"), "equiv.ys failed; see its log");
        assert_eq!(strip_ansi("a\u{1b}[1mb\u{1b}[0mc"), "abc");
    }

    #[test]
    fn initial_values_are_compared_by_register_bit() {
        let gold = module(
            r#""$f":{"type":"$dff","connections":{"CLK":[2],"D":[3],"Q":[4]}}"#,
            CLK_D_Q,
            r#""q":{"hide_name":0,"bits":[4],"attributes":{"init":"1"}}"#,
        );
        let mapped = |init: &str| {
            module(
                r#""$auto$ff.cc:337:slice$9":{"type":"DFF","connections":{"CLK":[2],"D":[3],"Q":[4]}}"#,
                CLK_D_Q,
                &format!(r#""q":{{"hide_name":0,"bits":[4],"attributes":{{"init":"{init}"}}}}"#),
            )
        };
        assert_eq!(check_inits(&gold, &mapped("1"), "top"), Ok(()));
        assert_eq!(
            check_inits(&gold, &mapped("0"), "top").unwrap_err(),
            "flip-flop q starts at 0 in the mapped netlist but at 1 in the source"
        );
    }
}
