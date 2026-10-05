//! Redstone repeaters: delay, pulse stretching, locking and tick priority.
//!
//! A repeater's `facing` points at its input. It reads the block there, and
//! `delay` × 2 game ticks later drives the block on its other side.

use super::{FLAGS_NO_NEIGHBORS, Mutation, Sim};
use crate::block::Block;
use crate::geom::{Dir, Pos};

const REPEATER: &str = "minecraft:repeater";

// Scheduled-tick priorities, by the game's names. Lower runs first; torches and lamps use 0.
const EXTREMELY_HIGH: i8 = -3;
const VERY_HIGH: i8 = -2;
const HIGH: i8 = -1;

impl Sim {
    fn repeater_delay(&self, delay: u8) -> u64 {
        let extra = if self.mutation == Some(Mutation::RepeaterDelayPlusTwoTicks) { 2 } else { 0 };
        u64::from(delay) * 2 + extra
    }

    /// Power arriving at the repeater's input (`getInputSignal`). Dust behind a
    /// repeater feeds it whatever shape the dust has.
    fn repeater_input(&self, pos: Pos, facing: Dir) -> u8 {
        let behind = pos.offset(facing);
        let dust = self.world.get(behind).as_wire().map_or(0, |w| w.power);
        self.signal(behind, facing, true).max(dust)
    }

    /// `isLocked`: a powered repeater points into one of this repeater's sides.
    pub(super) fn repeater_is_locked(&self, pos: Pos, facing: Dir) -> bool {
        if self.mutation == Some(Mutation::RepeaterIgnoresLock) {
            return false;
        }
        [facing.clockwise(), facing.clockwise().opposite()].into_iter().any(|side| {
            let beside = pos.offset(side);
            matches!(self.world.get(beside), Block::Repeater { .. }) && self.direct_signal(beside, side, true) > 0
        })
    }

    /// `shouldPrioritize`: the output goes into the back or side of another repeater.
    fn repeater_feeds_repeater(&self, pos: Pos, facing: Dir) -> bool {
        let out = facing.opposite();
        matches!(self.world.get(pos.offset(out)), Block::Repeater { facing: other, .. } if other != out)
    }

    /// `checkTickOnNeighbor`: schedule a change if the input no longer matches the output.
    pub(super) fn repeater_neighbor_changed(&mut self, pos: Pos, block: Block) {
        let Block::Repeater { facing, delay, powered, .. } = block else { return };
        if self.repeater_is_locked(pos, facing) {
            return;
        }
        let should_be_on = self.repeater_input(pos, facing) > 0;
        if powered != should_be_on && !self.will_tick_this_tick(pos, REPEATER) {
            let priority = if self.mutation == Some(Mutation::RepeaterOnePriority) {
                HIGH
            } else if self.repeater_feeds_repeater(pos, facing) {
                EXTREMELY_HIGH
            } else if powered {
                VERY_HIGH
            } else {
                HIGH
            };
            self.schedule_with_priority(pos, REPEATER, self.repeater_delay(delay), priority);
        }
    }

    /// The repeater's scheduled tick. Turning on does not re-check the input: a
    /// pulse shorter than the delay still comes out, stretched to the delay.
    pub(super) fn repeater_tick(&mut self, pos: Pos, block: Block) {
        let Block::Repeater { facing, delay, locked, powered } = block else { return };
        if self.repeater_is_locked(pos, facing) {
            return;
        }
        let should_be_on = self.repeater_input(pos, facing) > 0;
        if powered && !should_be_on {
            self.change(pos, Block::Repeater { facing, delay, locked, powered: false }, FLAGS_NO_NEIGHBORS);
        } else if !powered {
            if !should_be_on && self.mutation == Some(Mutation::RepeaterNoPulseStretch) {
                return;
            }
            self.change(pos, Block::Repeater { facing, delay, locked, powered: true }, FLAGS_NO_NEIGHBORS);
            if !should_be_on {
                self.schedule_with_priority(pos, REPEATER, self.repeater_delay(delay), VERY_HIGH);
            }
        }
    }

    /// `updateNeighborsInFront`: when a repeater changes, the block it drives and
    /// that block's other neighbours hear about it.
    pub(super) fn repeater_update_front(&mut self, pos: Pos, facing: Dir) {
        let front = pos.offset(facing.opposite());
        self.neighbor_changed(front);
        for dir in self.delivery_order(Dir::UPDATE_ORDER) {
            if dir != facing {
                self.neighbor_changed(front.offset(dir));
            }
        }
    }

    /// The repeater's `updateShape`: a change beside it refreshes the `locked` property.
    pub(super) fn repeater_update_shape(&self, pos: Pos, block: Block, dir: Dir) -> Block {
        let Block::Repeater { facing, delay, powered, .. } = block else { return block };
        if dir == facing || dir == facing.opposite() || dir == Dir::Down {
            return block;
        }
        Block::Repeater { facing, delay, powered, locked: self.repeater_is_locked(pos, facing) }
    }
}
