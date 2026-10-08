//! Writes a world as a vanilla structure file (`.nbt`), the format `/place template`
//! and structure blocks load. Whether other tools (Litematica) read it is untested.

use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;

use crate::block::Block;
use crate::nbt::{Tag, write_root};
use crate::world::World;

/// The Minecraft version this crate's block states and file formats target.
pub const MC_VERSION: &str = "26.3";
/// That version's world data version, written into every file.
pub const DATA_VERSION: i32 = 5023;

/// The gzip-compressed structure file for every non-air block of `world`.
///
/// Air is left out, so placing the structure does not clear the space it covers.
pub fn structure_file(world: &World) -> Vec<u8> {
    let mut palette: Vec<Block> = Vec::new();
    let mut blocks: Vec<Tag> = Vec::new();
    for (pos, block) in world.iter() {
        let state = match palette.iter().position(|b| *b == block) {
            Some(i) => i,
            None => {
                palette.push(block);
                palette.len() - 1
            }
        };
        blocks.push(Tag::compound(vec![("pos", Tag::ints([pos.x, pos.y, pos.z])), ("state", Tag::Int(state as i32))]));
    }
    let palette: Vec<Tag> = palette
        .iter()
        .map(|block| {
            let mut entry = vec![("id", Tag::Str(block.name().to_string()))];
            let props = block.properties();
            if !props.is_empty() {
                entry.push((
                    "properties",
                    Tag::Compound(props.into_iter().map(|(k, v)| (k.to_string(), Tag::Str(v))).collect()),
                ));
            }
            Tag::compound(entry)
        })
        .collect();
    let (sx, sy, sz) = world.size();
    let root = Tag::compound(vec![
        ("DataVersion", Tag::Int(DATA_VERSION)),
        ("size", Tag::ints([sx, sy, sz])),
        ("palette", Tag::List(palette)),
        ("blocks", Tag::List(blocks)),
        ("entities", Tag::List(Vec::new())),
    ]);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&write_root(&root)).expect("writing to a Vec cannot fail");
    encoder.finish().expect("writing to a Vec cannot fail")
}
