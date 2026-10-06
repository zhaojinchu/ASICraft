//! The redfab compiler core: blocks, tiles, placement, routing, timing,
//! extraction and simulation.
//!
//! This crate is pure computation. It takes bytes and strings in and gives
//! bytes and strings out, so the same code runs natively (through
//! `redfab-cli`) and in a browser (as WebAssembly). `clippy.toml` bans the
//! APIs that would break that.

pub mod bake;
pub mod block;
pub mod build;
pub mod extract;
pub mod geom;
pub mod layout;
pub mod lvs;
pub mod nbt;
pub mod netlist;
pub mod place;
pub mod pnrtest;
pub mod route;
pub mod sim;
pub mod structure;
pub mod tile;
pub mod tiletest;
pub mod trace;
pub mod view;
pub mod world;
