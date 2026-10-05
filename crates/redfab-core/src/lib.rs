//! The redfab compiler core: blocks, tiles, placement, routing, timing,
//! extraction and simulation.
//!
//! This crate is pure computation. It takes bytes and strings in and gives
//! bytes and strings out, so the same code runs natively (through
//! `redfab-cli`) and in a browser (as WebAssembly). `clippy.toml` bans the
//! APIs that would break that.

pub mod block;
pub mod geom;
pub mod nbt;
pub mod sim;
pub mod structure;
pub mod trace;
pub mod view;
pub mod world;
