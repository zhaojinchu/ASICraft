//! Redstone signal queries, mirroring the game's `Level.getSignal` family.
//!
//! Direction convention, as in the game: `dir` points from the block that asks
//! to the block that is asked. So `signal(pos, Dir::Down)` is what the block at
//! `pos` gives to the block directly above it.

use super::{Mutation, Sim};
use crate::block::Block;
use crate::geom::{Dir, Pos};

impl Sim {
    /// The game's `Level.getSignal`. With `wires` false, redstone wire answers 0,
    /// which is how the game keeps wire from powering wire through a block.
    pub(super) fn signal(&self, pos: Pos, dir: Dir, wires: bool) -> u8 {
        let block = self.world.get(pos);
        let own = self.own_signal(pos, block, dir, wires);
        if self.conducts(block) { own.max(self.direct_signal_to(pos, wires)) } else { own }
    }

    pub(super) fn conducts(&self, block: Block) -> bool {
        block.is_conductor() || (self.mutation == Some(Mutation::GlassConducts) && block == Block::Glass)
    }

    /// What the block itself emits toward the asker (`BlockState.getSignal`).
    fn own_signal(&self, pos: Pos, block: Block, dir: Dir, wires: bool) -> u8 {
        match block {
            Block::RedstoneBlock => 15,
            // A repeater powers only the block it points away from.
            Block::Repeater { facing, powered, .. } => {
                if powered && facing == dir {
                    15
                } else {
                    0
                }
            }
            Block::Lever { powered, .. } => {
                if powered {
                    15
                } else {
                    0
                }
            }
            Block::Torch { lit } => {
                let blocked = dir == Dir::Up && self.mutation != Some(Mutation::TorchPowersBlockBelow);
                if lit && !blocked { 15 } else { 0 }
            }
            Block::WallTorch { facing, lit } => {
                if lit && facing != dir {
                    15
                } else {
                    0
                }
            }
            Block::Wire(w) => {
                if !wires || dir == Dir::Down || w.power == 0 {
                    0
                } else if dir == Dir::Up {
                    w.power
                } else {
                    // The game recomputes the wire's connections here instead of
                    // trusting the stored shape.
                    let shape = self.wire_connection_state(pos, w);
                    let toward_asker = dir.opposite();
                    if shape.side(toward_asker).is_connected() || self.mutation == Some(Mutation::DotPowersSides) {
                        w.power
                    } else {
                        0
                    }
                }
            }
            _ => 0,
        }
    }

    /// Strong power the block at `pos` emits toward the asker (`getDirectSignal`).
    pub(super) fn direct_signal(&self, pos: Pos, dir: Dir, wires: bool) -> u8 {
        let block = self.world.get(pos);
        match block {
            Block::Torch { .. } | Block::WallTorch { .. } => {
                if dir == Dir::Down || (dir == Dir::Up && self.mutation == Some(Mutation::TorchPowersBlockBelow)) {
                    self.own_signal(pos, block, dir, wires)
                } else {
                    0
                }
            }
            Block::Wire(_) => self.own_signal(pos, block, dir, wires),
            Block::Repeater { .. } if self.mutation != Some(Mutation::RepeaterWeakFront) => {
                self.own_signal(pos, block, dir, wires)
            }
            // A lever strongly powers only the block it is mounted on.
            Block::Lever { powered: true, .. } if block.lever_mount() == Some(dir.opposite()) => 15,
            _ => 0,
        }
    }

    /// Strong power arriving at `pos` from its six neighbours (`getDirectSignalTo`).
    fn direct_signal_to(&self, pos: Pos, wires: bool) -> u8 {
        let wires = wires || self.mutation == Some(Mutation::WeakPowerReachesDust);
        Dir::ALL.into_iter().map(|d| self.direct_signal(pos.offset(d), d, wires)).max().unwrap_or(0)
    }

    /// The game's `Level.hasSignal`.
    pub(super) fn has_signal(&self, pos: Pos, dir: Dir) -> bool {
        self.signal(pos, dir, true) > 0
    }

    /// The game's `Level.hasNeighborSignal`: is anything around `pos` powering it?
    pub(super) fn has_neighbor_signal(&self, pos: Pos) -> bool {
        Dir::ALL.into_iter().any(|d| self.signal(pos.offset(d), d, true) > 0)
    }

    /// Power a wire at `pos` receives from non-wire neighbours (`getBlockSignal`).
    pub(super) fn block_signal_for_wire(&self, pos: Pos) -> u8 {
        Dir::ALL.into_iter().map(|d| self.signal(pos.offset(d), d, false)).max().unwrap_or(0)
    }
}
