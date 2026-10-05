//! A text picture of a world, one grid per height level, for debugging.
//!
//! North is up and east is right. Legend:
//! `.` air, `#` stone, `G` glass, `R` redstone block, `L`/`l` lamp lit/unlit,
//! `0`-`f` dust with that power, `T`/`t` standing torch lit/unlit, `P`/`/` lever on/off,
//! arrows for a lit wall torch pointing away from its block, `n e s w` for an unlit one.

use crate::block::Block;
use crate::geom::{Dir, Pos};
use crate::world::World;

pub fn symbol(block: Block) -> char {
    match block {
        Block::Air => '.',
        Block::Stone => '#',
        Block::Glass => 'G',
        Block::RedstoneBlock => 'R',
        Block::Lamp { lit } => {
            if lit {
                'L'
            } else {
                'l'
            }
        }
        Block::Wire(w) => char::from_digit(u32::from(w.power), 16).unwrap_or('?'),
        Block::Torch { lit } => {
            if lit {
                'T'
            } else {
                't'
            }
        }
        Block::Lever { powered, .. } => {
            if powered {
                'P'
            } else {
                '/'
            }
        }
        Block::WallTorch { facing, lit } => match (facing, lit) {
            (Dir::North, true) => '↑',
            (Dir::East, true) => '→',
            (Dir::South, true) => '↓',
            (Dir::West, true) => '←',
            (Dir::North, false) => 'n',
            (Dir::East, false) => 'e',
            (Dir::South, false) => 's',
            (Dir::West, false) => 'w',
            _ => '?',
        },
    }
}

/// All levels side by side, lowest on the left.
pub fn layers(world: &World) -> String {
    let (sx, sy, sz) = world.size();
    let mut out = String::new();
    for y in 0..sy {
        let label = format!("y={y}");
        out.push_str(&format!("{label:<width$}  ", width = sx as usize));
    }
    out.push('\n');
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                out.push(symbol(world.get(Pos::new(x, y, z))));
            }
            out.push_str("  ");
        }
        out.push('\n');
    }
    out
}
