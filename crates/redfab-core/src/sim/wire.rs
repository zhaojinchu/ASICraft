//! Redstone wire: which way it connects, and how a network settles its power.

use std::collections::VecDeque;

use super::{Mutation, Sim};
use crate::block::{Block, Side, Wire};
use crate::geom::{Dir, Pos};

impl Sim {
    // ------------------------------------------------------------ shape

    /// The game's `shouldConnectTo`. `dir` is `None` when asking about a block
    /// one level up or down, where only another wire counts.
    fn should_connect_to(block: Block, dir: Option<Dir>) -> bool {
        match block {
            Block::Wire(_) => true,
            other => other.is_signal_source() && dir.is_some(),
        }
    }

    /// The game's `getConnectingSide`.
    fn wire_connecting_side(&self, pos: Pos, dir: Dir, can_connect_up: bool) -> Side {
        let beside = pos.offset(dir);
        let beside_block = self.world.get(beside);
        if can_connect_up
            && beside_block.is_full_block()
            && Self::should_connect_to(self.world.get(beside.up()), None)
        {
            return Side::Up;
        }
        let steps_down = !self.conducts(beside_block)
            && self.mutation != Some(Mutation::NoStepDownConnection)
            && Self::should_connect_to(self.world.get(beside.down()), None);
        if Self::should_connect_to(beside_block, Some(dir)) || steps_down {
            Side::Side
        } else {
            Side::None
        }
    }

    fn wire_can_connect_up(&self, pos: Pos) -> bool {
        !self.conducts(self.world.get(pos.up()))
    }

    /// The game's `getConnectionState`: the shape this wire should have, given
    /// its surroundings and whether it is currently a dot.
    pub(super) fn wire_connection_state(&self, pos: Pos, wire: Wire) -> Wire {
        let was_dot = wire.is_dot();
        let can_up = self.wire_can_connect_up(pos);
        let mut shape = Wire {
            power: wire.power,
            ..Wire::DOT
        };
        for dir in Dir::HORIZONTAL {
            shape = shape.with_side(dir, self.wire_connecting_side(pos, dir, can_up));
        }
        if was_dot && shape.is_dot() {
            return shape;
        }
        let n = shape.north.is_connected();
        let s = shape.south.is_connected();
        let e = shape.east.is_connected();
        let w = shape.west.is_connected();
        let no_ns = !n && !s;
        let no_ew = !e && !w;
        // A wire with connections on one axis only runs straight through.
        if !w && no_ns {
            shape.west = Side::Side;
        }
        if !e && no_ns {
            shape.east = Side::Side;
        }
        if !n && no_ew {
            shape.north = Side::Side;
        }
        if !s && no_ew {
            shape.south = Side::Side;
        }
        shape
    }

    /// The game's `updateShape` for wire: its neighbour in direction `dir` changed.
    pub(super) fn wire_update_shape(&self, pos: Pos, wire: Wire, dir: Dir) -> Wire {
        match dir {
            Dir::Down => wire,
            Dir::Up => self.wire_connection_state(pos, wire),
            _ => {
                let side = self.wire_connecting_side(pos, dir, self.wire_can_connect_up(pos));
                if side.is_connected() == wire.side(dir).is_connected() && !wire.is_cross() {
                    wire.with_side(dir, side)
                } else {
                    let cross = Wire {
                        power: wire.power,
                        north: Side::Side,
                        east: Side::Side,
                        south: Side::Side,
                        west: Side::Side,
                    };
                    self.wire_connection_state(pos, cross.with_side(dir, side))
                }
            }
        }
    }

    /// The game's `updateIndirectNeighbourShapes`: wires one level up or down
    /// from each connected side re-check the side that faces back.
    pub(super) fn wire_update_indirect_shapes(&mut self, pos: Pos, wire: Wire) {
        for dir in Dir::HORIZONTAL {
            if wire.side(dir) == Side::None
                || matches!(self.world.get(pos.offset(dir)), Block::Wire(_))
            {
                continue;
            }
            for diagonal in [pos.offset(dir).down(), pos.offset(dir).up()] {
                if matches!(self.world.get(diagonal), Block::Wire(_)) {
                    self.neighbor_shape_changed(diagonal, dir.opposite());
                }
            }
        }
    }

    /// The game's `updateNeighborsOfNeighboringWires`.
    pub(super) fn update_neighbors_of_neighboring_wires(&mut self, pos: Pos) {
        for dir in Dir::HORIZONTAL {
            self.check_corner_change_at(pos.offset(dir));
        }
        for dir in Dir::HORIZONTAL {
            let beside = pos.offset(dir);
            if self.conducts(self.world.get(beside)) {
                self.check_corner_change_at(beside.up());
            } else {
                self.check_corner_change_at(beside.down());
            }
        }
    }

    fn check_corner_change_at(&mut self, pos: Pos) {
        if matches!(self.world.get(pos), Block::Wire(_)) {
            self.update_neighbors_at(pos);
            for dir in Dir::ALL {
                self.update_neighbors_at(pos.offset(dir));
            }
        }
    }

    // ------------------------------------------------------------ power

    /// Re-settles the wire network around `start` and tells the neighbours of
    /// every wire whose power changed. This is the game's experimental wire
    /// evaluator: all strengths are final before any neighbour is notified.
    pub(super) fn update_power(&mut self, start: Pos) {
        let mut net = Network {
            updated: Vec::new(),
            turn_off: VecDeque::new(),
            turn_on: VecDeque::new(),
        };

        if let Block::Wire(w) = self.world.get(start) {
            net.set_power(start, w.power);
            net.turn_off.push_back(start);
        } else {
            self.propagate(&mut net, start, 0, true);
        }
        while let Some(pos) = net.turn_off.pop_front() {
            let current = net.power_of(pos).unwrap_or(0);
            let from_blocks = self.block_signal_for_wire(pos);
            let incoming = from_blocks.max(self.incoming_wire_signal(&net, pos));
            let new = if incoming < current {
                if from_blocks > 0 && !net.turn_on.contains(&pos) {
                    net.turn_on.push_back(pos);
                }
                0
            } else {
                incoming
            };
            if new != current {
                net.set_power(pos, new);
            }
            self.propagate(&mut net, pos, new, current > incoming);
        }
        while let Some(pos) = net.turn_on.pop_front() {
            let current = net.power_of(pos).unwrap_or(0);
            let incoming = self
                .block_signal_for_wire(pos)
                .max(self.incoming_wire_signal(&net, pos));
            if incoming > current {
                net.set_power(pos, incoming);
            }
            self.propagate(&mut net, pos, incoming.max(current), false);
        }

        // Write every changed strength first (no updates), then notify.
        let mut changed: Vec<Pos> = Vec::new();
        for &(pos, power) in &net.updated {
            if let Block::Wire(w) = self.world.get(pos)
                && w.power != power
            {
                self.world.set(pos, Block::Wire(Wire { power, ..w }));
                changed.push(pos);
            }
        }
        for pos in changed {
            let Block::Wire(w) = self.world.get(pos) else {
                continue;
            };
            for dir in self.delivery_order(Dir::UPDATE_ORDER) {
                let connected = match dir {
                    Dir::Down => true,
                    Dir::Up => false,
                    _ => w.side(dir).is_connected(),
                };
                if !connected {
                    continue;
                }
                let beside = pos.offset(dir);
                self.neighbor_changed(beside);
                if self.conducts(self.world.get(beside)) {
                    for onward in self.delivery_order(Dir::UPDATE_ORDER) {
                        if onward != dir.opposite() {
                            self.neighbor_changed(beside.offset(onward));
                        }
                    }
                }
            }
        }
    }

    fn wire_signal(&self, net: &Network, pos: Pos) -> u8 {
        if let Some(power) = net.power_of(pos) {
            return power;
        }
        self.world.get(pos).as_wire().map_or(0, |w| w.power)
    }

    /// The strongest neighbouring wire, minus the loss of one block
    /// (`getIncomingWireSignal`). Wires one level up or down count when the
    /// blocks in between allow it.
    fn incoming_wire_signal(&self, net: &Network, pos: Pos) -> u8 {
        let above_conducts = self.conducts(self.world.get(pos.up()));
        let mut best = 0;
        for dir in Dir::HORIZONTAL {
            let beside = pos.offset(dir);
            let beside_conducts = self.conducts(self.world.get(beside));
            best = best.max(self.wire_signal(net, beside));
            if beside_conducts && !above_conducts {
                best = best.max(self.wire_signal(net, beside.up()));
            } else if !beside_conducts && self.mutation != Some(Mutation::NoStepDownConnection) {
                best = best.max(self.wire_signal(net, beside.down()));
            }
        }
        let loss = if self.mutation == Some(Mutation::DustLosesTwoPerBlock) {
            2
        } else {
            1
        };
        best.saturating_sub(loss)
    }

    /// The game's `propagateChangeToNeighbors`: queue the wires this one can reach.
    fn propagate(&self, net: &mut Network, pos: Pos, power: u8, can_turn_off: bool) {
        for dir in Dir::HORIZONTAL {
            self.enqueue(net, pos.offset(dir), power, can_turn_off);
        }
        let above_conducts = self.conducts(self.world.get(pos.up()));
        for dir in Dir::HORIZONTAL {
            if !above_conducts {
                self.enqueue(net, pos.up().offset(dir), power, can_turn_off);
            }
        }
        for dir in Dir::HORIZONTAL {
            if !self.conducts(self.world.get(pos.offset(dir))) {
                self.enqueue(net, pos.down().offset(dir), power, can_turn_off);
            }
        }
    }

    fn enqueue(&self, net: &mut Network, pos: Pos, power: u8, can_turn_off: bool) {
        if !matches!(self.world.get(pos), Block::Wire(_)) {
            return;
        }
        let theirs = self.wire_signal(net, pos);
        if u16::from(theirs) + 1 < u16::from(power) && !net.turn_on.contains(&pos) {
            net.turn_on.push_back(pos);
            net.set_power(pos, theirs);
        }
        if can_turn_off && theirs > power && !net.turn_off.contains(&pos) {
            net.turn_off.push_back(pos);
            net.set_power(pos, theirs);
        }
    }
}

/// Working state of one power update: the strengths decided so far, in the order
/// the wires were first reached, and the two work queues.
struct Network {
    updated: Vec<(Pos, u8)>,
    turn_off: VecDeque<Pos>,
    turn_on: VecDeque<Pos>,
}

impl Network {
    fn power_of(&self, pos: Pos) -> Option<u8> {
        self.updated
            .iter()
            .find(|(p, _)| *p == pos)
            .map(|&(_, power)| power)
    }

    fn set_power(&mut self, pos: Pos, power: u8) {
        match self.updated.iter_mut().find(|(p, _)| *p == pos) {
            Some(entry) => entry.1 = power,
            None => self.updated.push((pos, power)),
        }
    }
}
