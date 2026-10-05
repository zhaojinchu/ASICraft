//! A box of blocks. Everything outside the box is air.

use crate::block::Block;
use crate::geom::Pos;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct World {
    size: (i32, i32, i32),
    blocks: Vec<Block>,
}

impl World {
    /// An all-air world covering `0..size` on each axis.
    pub fn new(size: (i32, i32, i32)) -> World {
        assert!(size.0 > 0 && size.1 > 0 && size.2 > 0, "world size must be positive");
        World { size, blocks: vec![Block::Air; (size.0 * size.1 * size.2) as usize] }
    }

    pub fn size(&self) -> (i32, i32, i32) {
        self.size
    }

    pub fn contains(&self, pos: Pos) -> bool {
        (0..self.size.0).contains(&pos.x) && (0..self.size.1).contains(&pos.y) && (0..self.size.2).contains(&pos.z)
    }

    // Same cell order as Litematica and vanilla sections: y, then z, then x.
    fn index(&self, pos: Pos) -> usize {
        ((pos.y * self.size.2 + pos.z) * self.size.0 + pos.x) as usize
    }

    pub fn get(&self, pos: Pos) -> Block {
        if self.contains(pos) { self.blocks[self.index(pos)] } else { Block::Air }
    }

    /// Panics if `pos` is outside the box; callers decide what that means for them.
    pub fn set(&mut self, pos: Pos, block: Block) {
        assert!(self.contains(pos), "{pos} is outside the world");
        let i = self.index(pos);
        self.blocks[i] = block;
    }

    /// Every non-air block with its position, in y, z, x order.
    pub fn iter(&self) -> impl Iterator<Item = (Pos, Block)> + '_ {
        let (sx, sy, sz) = self.size;
        (0..sy)
            .flat_map(move |y| (0..sz).flat_map(move |z| (0..sx).map(move |x| Pos::new(x, y, z))))
            .map(|pos| (pos, self.get(pos)))
            .filter(|(_, block)| !block.is_air())
    }
}
