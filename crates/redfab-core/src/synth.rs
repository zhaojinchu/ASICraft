//! The Yosys scripts that turn SystemVerilog into a netlist of the tile
//! library's cells and prove the result equals the source. The command line
//! runs them under Node, the playground in a browser worker; `{top}` is the
//! top module's name.

/// Synthesis: read, flatten, turn memories into registers and muxes, give
/// every register an initial value (0 unless the source says otherwise),
/// write the netlist for the lint (`pre.json`, see [`crate::lint`]), then map
/// flip-flops to the falling-edge cell (with an inverter on the clock for
/// rising-edge ones) and gates to NOT/NOR2/NOR3.
pub const MAP_SCRIPT: &str = "\
read_slang --threads 1 --top {top} top.sv
proc
memory
write_json pre.json
setundef -init -zero
opt
techmap
opt
dfflegalize -cell $_DFF_N_ 01
dfflibmap -liberty cells.lib
abc -liberty cells.lib
opt_clean
write_json mapped.json
write_verilog -noattr mapped.v
";

/// The equivalence proof between the source and the mapped netlist. The proof
/// ignores initial values; `gold.json` is written so that they can be compared
/// with the mapped netlist's (see [`crate::lint::check_inits`]).
pub const EQUIV_SCRIPT: &str = "\
read_slang --threads 1 --top {top} top.sv
proc
memory
setundef -init -zero
opt_clean
write_json gold.json
rename {top} gold
design -stash gold
read_verilog mapped.v
read_verilog cells.v
hierarchy -top {top}
flatten
proc
opt_clean
rename {top} gate
design -stash gate
design -copy-from gold -as gold gold
design -copy-from gate -as gate gate
equiv_make gold gate equiv
hierarchy -top equiv
equiv_simple
equiv_induct
equiv_status -assert
";
