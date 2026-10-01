//! The `redfab` command. All file and process access lives here, not in
//! `redfab-core`.

fn main() {
    println!("redfab {}", env!("CARGO_PKG_VERSION"));
}
