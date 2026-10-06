//! A block-level redstone simulator that follows the game's own rules.
//!
//! The functions here mirror the game's (`neighborChanged`, `updateShape`,
//! `getSignal`, scheduled ticks with priorities) for the blocks in
//! [`crate::block::Block`], so a simulated circuit can be compared with a
//! recording from the real game tick for tick. Wire power follows the game's
//! "redstone experiments" evaluator: a whole wire network settles to its new
//! strengths before any neighbour hears about it. `docs/mechanics.md` lists the
//! measurements these rules were checked against.

mod repeater;
mod signal;
mod wire;

use crate::block::Block;
use crate::geom::{Dir, Pos};
use crate::world::World;

/// A deliberately wrong rule, used to prove that the test corpus can tell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mutation {
    DustLosesTwoPerBlock,
    TorchDelayOneTick,
    LampOffDelayTwoTicks,
    WeakPowerReachesDust,
    GlassConducts,
    NoStepDownConnection,
    TorchPowersBlockBelow,
    BurnoutOnNinthToggle,
    DotPowersSides,
    RepeaterDelayPlusTwoTicks,
    RepeaterIgnoresLock,
    RepeaterNoPulseStretch,
    RepeaterOnePriority,
    RepeaterWeakFront,
}

impl Mutation {
    pub const ALL: [Mutation; 14] = [
        Mutation::DustLosesTwoPerBlock,
        Mutation::TorchDelayOneTick,
        Mutation::LampOffDelayTwoTicks,
        Mutation::WeakPowerReachesDust,
        Mutation::GlassConducts,
        Mutation::NoStepDownConnection,
        Mutation::TorchPowersBlockBelow,
        Mutation::BurnoutOnNinthToggle,
        Mutation::DotPowersSides,
        Mutation::RepeaterDelayPlusTwoTicks,
        Mutation::RepeaterIgnoresLock,
        Mutation::RepeaterNoPulseStretch,
        Mutation::RepeaterOnePriority,
        Mutation::RepeaterWeakFront,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mutation::DustLosesTwoPerBlock => "dust-loses-two-per-block",
            Mutation::TorchDelayOneTick => "torch-delay-one-tick",
            Mutation::LampOffDelayTwoTicks => "lamp-off-delay-two-ticks",
            Mutation::WeakPowerReachesDust => "weak-power-reaches-dust",
            Mutation::GlassConducts => "glass-conducts",
            Mutation::NoStepDownConnection => "no-step-down-connection",
            Mutation::TorchPowersBlockBelow => "torch-powers-block-below",
            Mutation::BurnoutOnNinthToggle => "burnout-on-ninth-toggle",
            Mutation::DotPowersSides => "dot-powers-sides",
            Mutation::RepeaterDelayPlusTwoTicks => "repeater-delay-plus-two-ticks",
            Mutation::RepeaterIgnoresLock => "repeater-ignores-lock",
            Mutation::RepeaterNoPulseStretch => "repeater-no-pulse-stretch",
            Mutation::RepeaterOnePriority => "repeater-one-priority",
            Mutation::RepeaterWeakFront => "repeater-weak-front",
        }
    }

    pub fn from_name(name: &str) -> Option<Mutation> {
        Mutation::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// A scheduled block tick that has not run yet.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Pending {
    pub pos: Pos,
    pub block: String,
    pub delay: u64,
    pub priority: i8,
}

#[derive(Clone, Debug)]
struct Scheduled {
    trigger: u64,
    priority: i8,
    seq: u64,
    pos: Pos,
    block: &'static str,
}

/// What a block change tells the rest of the world, as in the game's `setBlock` flags.
#[derive(Clone, Copy)]
struct Flags {
    /// Send block updates to the six neighbours (flag 1).
    neighbors: bool,
    /// Let neighbours update their shape (absence of flag 16).
    shapes: bool,
}

const FLAGS_ALL: Flags = Flags { neighbors: true, shapes: true };
const FLAGS_NO_NEIGHBORS: Flags = Flags { neighbors: false, shapes: true };

const TORCH_DELAY: u64 = 2;
const LAMP_OFF_DELAY: u64 = 4;
const BURNOUT_WINDOW: u64 = 60;
const BURNOUT_TOGGLES: usize = 8;
const BURNOUT_RETRY: u64 = 160;

#[derive(Clone)]
pub struct Sim {
    world: World,
    time: u64,
    queue: Vec<Scheduled>,
    next_seq: u64,
    /// Ticks collected for the tick being processed and not yet run.
    running: Vec<(Pos, &'static str)>,
    /// Recent torch turn-offs, oldest first (the game keeps one list for the whole level).
    toggles: Vec<(Pos, u64)>,
    mutation: Option<Mutation>,
    /// When set, every block update is delivered to its targets in a pseudo-random
    /// order drawn from this state instead of the fixed default order.
    delivery_rng: Option<u64>,
}

impl Sim {
    pub fn new(world: World) -> Sim {
        Sim {
            world,
            time: 0,
            queue: Vec::new(),
            next_seq: 0,
            running: Vec::new(),
            toggles: Vec::new(),
            mutation: None,
            delivery_rng: None,
        }
    }

    /// Delivers block updates in a shuffled order from now on (`None` restores the
    /// default). The game's own delivery order depends on position in the default
    /// redstone mode and is partly random in experiments mode, so a circuit whose
    /// result changes with this setting has a race.
    pub fn shuffle_delivery(&mut self, seed: Option<u64>) {
        self.delivery_rng = seed.map(|s| s.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    }

    /// Returns `items` in delivery order: unchanged by default, shuffled if requested.
    fn delivery_order<const N: usize>(&mut self, mut items: [Dir; N]) -> [Dir; N] {
        if let Some(state) = self.delivery_rng.as_mut() {
            for i in (1..N).rev() {
                // xorshift64
                *state ^= *state << 13;
                *state ^= *state >> 7;
                *state ^= *state << 17;
                items.swap(i, (*state % (i as u64 + 1)) as usize);
            }
        }
        items
    }

    /// The game destroys wire and torches that lose their support. redfab never
    /// builds those, so the simulator refuses them instead of modelling the break.
    fn check_supported(&self, pos: Pos, block: Block) -> Result<(), String> {
        let support = match block {
            Block::Wire(_) | Block::Torch { .. } | Block::Repeater { .. } => pos.down(),
            Block::WallTorch { facing, .. } => pos.offset(facing.opposite()),
            Block::Lever { .. } => pos.offset(block.lever_mount().expect("levers have a mount")),
            _ => return Ok(()),
        };
        if self.world.get(support).is_full_block() {
            Ok(())
        } else {
            Err(format!("unsupported: {} at {pos} has no supporting block", block.name()))
        }
    }

    // ------------------------------------------------------------ whole-world operations

    pub fn into_world(self) -> World {
        self.world
    }

    /// Gives every wire the shape the game would give it from its surroundings
    /// (lines and corners; a wire with no connections becomes a cross).
    pub fn fix_wire_shapes(&mut self) {
        let wires: Vec<(Pos, Block)> = self.world.iter().filter(|(_, b)| matches!(b, Block::Wire(_))).collect();
        for (pos, block) in wires {
            let Block::Wire(w) = block else { continue };
            // Start from a cross so the "stays a dot" rule does not apply.
            let cross = crate::block::Wire {
                power: w.power,
                north: crate::block::Side::Side,
                east: crate::block::Side::Side,
                south: crate::block::Side::Side,
                west: crate::block::Side::Side,
            };
            self.world.set(pos, Block::Wire(self.wire_connection_state(pos, cross)));
        }
    }

    /// Sends a block update to every block, as if each had just noticed its surroundings.
    pub fn poke_all(&mut self) {
        let positions: Vec<Pos> = self.world.iter().map(|(pos, _)| pos).collect();
        for pos in positions {
            self.neighbor_changed(pos);
        }
    }

    /// Sends block updates to `pos` and to everything within two blocks of it,
    /// which is what the game does around a component that changed.
    pub fn wake(&mut self, pos: Pos) {
        self.neighbor_changed(pos);
        self.update_neighbors_at(pos);
        for dir in Dir::ALL {
            self.update_neighbors_at(pos.offset(dir));
        }
    }

    /// Steps until no scheduled tick is left. Returns the number of ticks run, or
    /// an error if the circuit is still active after `max_ticks`.
    pub fn settle(&mut self, max_ticks: u32) -> Result<u32, String> {
        for tick in 0..=max_ticks {
            if self.queue.is_empty() {
                return Ok(tick);
            }
            self.step();
        }
        Err(format!("the circuit has not settled after {max_ticks} ticks; it oscillates or has burned out a torch"))
    }

    // ------------------------------------------------------------ queries

    /// Is the torch at `pos` receiving power (so that it is, or will turn, off)?
    pub fn torch_is_powered(&self, pos: Pos) -> bool {
        self.torch_has_signal(pos, self.world.get(pos))
    }

    /// Is anything powering the block at `pos`? This is what a lamp looks at.
    pub fn is_powered(&self, pos: Pos) -> bool {
        self.has_neighbor_signal(pos)
    }

    /// Power the wire at `pos` receives from blocks other than wire.
    pub fn wire_block_signal(&self, pos: Pos) -> u8 {
        self.block_signal_for_wire(pos)
    }

    /// Checks that every wire and torch in the world is supported.
    pub fn check_all_supported(&self) -> Result<(), String> {
        self.world.iter().try_for_each(|(pos, block)| self.check_supported(pos, block))
    }

    pub fn with_mutation(mut self, mutation: Option<Mutation>) -> Sim {
        self.mutation = mutation;
        self
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// How often each torch has turned off within the last sixty ticks, for the
    /// ones that have at all. The game burns a torch out on its eighth.
    pub fn torch_turn_offs(&self) -> std::collections::BTreeMap<Pos, usize> {
        let now = self.time;
        let mut out = std::collections::BTreeMap::new();
        for &(pos, when) in &self.toggles {
            if now - when <= BURNOUT_WINDOW {
                *out.entry(pos).or_insert(0) += 1;
            }
        }
        out
    }

    /// Scheduled ticks that have not run, sorted by position.
    pub fn pending(&self) -> Vec<Pending> {
        let mut out: Vec<Pending> = self
            .queue
            .iter()
            .map(|s| Pending {
                pos: s.pos,
                block: s.block.to_string(),
                delay: s.trigger - self.time,
                priority: s.priority,
            })
            .collect();
        out.sort();
        out
    }

    /// Adds a pending tick, for starting from a recorded state that has some.
    pub fn seed_pending(&mut self, pos: Pos, delay: u64, priority: i8) -> Result<(), String> {
        let block = self.world.get(pos).name();
        if self.world.get(pos).is_air() {
            return Err(format!("pending tick at {pos} but the block there is air"));
        }
        self.schedule_with_priority(pos, block, delay, priority);
        Ok(())
    }

    /// Places a block the way the `/setblock` command does.
    ///
    /// `strict` writes the state as given and tells nobody. Otherwise the placed
    /// block first takes its shape from its neighbours, neighbours update their
    /// shapes, and then the six neighbours receive a block update.
    /// Returns `Ok(false)` if the block was already in that state.
    pub fn set_block(&mut self, pos: Pos, block: Block, strict: bool) -> Result<bool, String> {
        if !self.world.contains(pos) {
            return Err(format!("{pos} is outside the simulated box"));
        }
        if strict {
            let changed = self.world.get(pos) != block;
            self.world.set(pos, block);
            return Ok(changed);
        }
        self.check_supported(pos, block)?;
        for dir in Dir::ALL {
            // Removing or replacing a support would break what stands on it.
            let beside = pos.offset(dir);
            let rests_here = match self.world.get(beside) {
                Block::Wire(_) | Block::Torch { .. } | Block::Repeater { .. } => dir == Dir::Up,
                Block::WallTorch { facing, .. } => facing == dir,
                lever @ Block::Lever { .. } => lever.lever_mount() == Some(dir.opposite()),
                _ => false,
            };
            if rests_here && !block.is_full_block() {
                return Err(format!("unsupported: placing {} at {pos} removes the support of {beside}", block.name()));
            }
        }
        let block = self.update_from_neighbour_shapes(block, pos);
        let old = self.world.get(pos);
        if old == block {
            return Ok(false);
        }
        self.change(pos, block, FLAGS_NO_NEIGHBORS);
        // The command then does what the game calls updateNeighboursOnBlockSet.
        if old.name() != self.world.get(pos).name() {
            self.after_removal(pos, old);
        }
        self.update_neighbors_at(pos);
        Ok(true)
    }

    /// Runs one game tick: every scheduled tick that is due, in priority order.
    /// Ticks with the same due time and priority run in the order they were scheduled.
    pub fn step(&mut self) {
        self.step_in_order(&[]);
    }

    /// How many scheduled ticks fall due in the next step, in groups that share a
    /// due time and priority. Only the order inside a group is a free choice.
    pub fn due_groups(&self) -> Vec<usize> {
        let now = self.time + 1;
        let mut keys: Vec<(u64, i8)> =
            self.queue.iter().filter(|s| s.trigger <= now).map(|s| (s.trigger, s.priority)).collect();
        keys.sort_unstable();
        let mut groups: Vec<usize> = Vec::new();
        let mut last = None;
        for key in keys {
            if last == Some(key) {
                *groups.last_mut().expect("a group exists once a key repeats") += 1;
            } else {
                groups.push(1);
            }
            last = Some(key);
        }
        groups
    }

    /// Like [`Sim::step`], but `order` rearranges the due ticks: it lists, for each
    /// position in the run order, which of the default-ordered ticks goes there.
    /// It must be a permutation that keeps every tick inside its group (see
    /// [`Sim::due_groups`]); an empty slice means the default order.
    pub fn step_in_order(&mut self, order: &[usize]) {
        self.time += 1;
        let now = self.time;
        let mut due: Vec<Scheduled> = Vec::new();
        self.queue.retain(|s| {
            if s.trigger <= now {
                due.push(s.clone());
                false
            } else {
                true
            }
        });
        due.sort_by_key(|s| (s.trigger, s.priority, s.seq));
        if !order.is_empty() {
            assert_eq!(order.len(), due.len(), "order must cover every due tick");
            due = order.iter().map(|&i| due[i].clone()).collect();
        }
        self.running = due.iter().map(|s| (s.pos, s.block)).collect();
        for tick in due {
            if let Some(i) = self.running.iter().position(|r| *r == (tick.pos, tick.block)) {
                self.running.remove(i);
            }
            let block = self.world.get(tick.pos);
            if block.name() != tick.block {
                continue; // the block was replaced since the tick was scheduled
            }
            match block {
                Block::Torch { lit } | Block::WallTorch { lit, .. } => self.torch_tick(tick.pos, block, lit),
                Block::Lamp { lit } => self.lamp_tick(tick.pos, lit),
                Block::Repeater { .. } => self.repeater_tick(tick.pos, block),
                _ => {}
            }
        }
        self.running.clear();
    }

    // ------------------------------------------------------------ block changes

    /// The game's `Level.setBlock` for a state that differs from the current one.
    fn change(&mut self, pos: Pos, new: Block, flags: Flags) {
        let old = self.world.get(pos);
        if old == new {
            return;
        }
        self.world.set(pos, new);
        if flags.neighbors && old.name() != new.name() {
            self.after_removal(pos, old);
        }
        self.on_place(pos, new, old);
        if flags.neighbors {
            self.update_neighbors_at(pos);
        }
        if flags.shapes {
            if let Block::Wire(w) = old {
                self.wire_update_indirect_shapes(pos, w);
            }
            for dir in Dir::SHAPE_ORDER {
                self.neighbor_shape_changed(pos.offset(dir), dir.opposite());
            }
            if let Block::Wire(w) = new {
                self.wire_update_indirect_shapes(pos, w);
            }
        }
    }

    fn on_place(&mut self, pos: Pos, new: Block, old: Block) {
        match new {
            Block::Wire(_) if old.name() != new.name() => {
                self.update_power(pos);
                self.update_neighbors_at(pos.up());
                self.update_neighbors_at(pos.down());
                self.update_neighbors_of_neighboring_wires(pos);
            }
            Block::Torch { .. } | Block::WallTorch { .. } => self.torch_notify_neighbors(pos),
            // Any change of a repeater's state, not only its placement.
            Block::Repeater { facing, .. } => self.repeater_update_front(pos, facing),
            _ => {}
        }
    }

    fn after_removal(&mut self, pos: Pos, old: Block) {
        match old {
            Block::Wire(_) => {
                for dir in Dir::ALL {
                    self.update_neighbors_at(pos.offset(dir));
                }
                self.update_power(pos);
                self.update_neighbors_of_neighboring_wires(pos);
            }
            Block::Torch { .. } | Block::WallTorch { .. } => self.torch_notify_neighbors(pos),
            Block::Repeater { facing, .. } => self.repeater_update_front(pos, facing),
            _ => {}
        }
    }

    /// Sends a block update to the six neighbours of `pos`.
    fn update_neighbors_at(&mut self, pos: Pos) {
        for dir in self.delivery_order(Dir::UPDATE_ORDER) {
            self.neighbor_changed(pos.offset(dir));
        }
    }

    fn neighbor_changed(&mut self, pos: Pos) {
        match self.world.get(pos) {
            Block::Wire(_) => self.update_power(pos),
            block @ (Block::Torch { lit } | Block::WallTorch { lit, .. }) => {
                if lit == self.torch_has_signal(pos, block) && !self.will_tick_this_tick(pos, block.name()) {
                    let delay = if self.mutation == Some(Mutation::TorchDelayOneTick) { 1 } else { TORCH_DELAY };
                    self.schedule(pos, block.name(), delay);
                }
            }
            block @ Block::Repeater { .. } => self.repeater_neighbor_changed(pos, block),
            Block::Lamp { lit } if lit != self.has_neighbor_signal(pos) => {
                if lit {
                    let delay = if self.mutation == Some(Mutation::LampOffDelayTwoTicks) { 2 } else { LAMP_OFF_DELAY };
                    self.schedule(pos, "minecraft:redstone_lamp", delay);
                } else {
                    self.change(pos, Block::Lamp { lit: true }, FLAGS_NO_NEIGHBORS);
                }
            }
            _ => {}
        }
    }

    /// Asks the block at `pos` to update its shape because its neighbour in
    /// direction `dir` changed.
    fn neighbor_shape_changed(&mut self, pos: Pos, dir: Dir) {
        let block = self.world.get(pos);
        let updated = match block {
            Block::Wire(w) => Block::Wire(self.wire_update_shape(pos, w, dir)),
            Block::Repeater { .. } => self.repeater_update_shape(pos, block, dir),
            _ => return,
        };
        if updated != block {
            // Shape-only changes are written without block updates; this is why a
            // reshaped wire does not wake the components around it.
            self.change(pos, updated, FLAGS_NO_NEIGHBORS);
        }
    }

    fn update_from_neighbour_shapes(&self, block: Block, pos: Pos) -> Block {
        match block {
            Block::Wire(mut w) => {
                for dir in Dir::SHAPE_ORDER {
                    w = self.wire_update_shape(pos, w, dir);
                }
                Block::Wire(w)
            }
            Block::Repeater { .. } => {
                Dir::SHAPE_ORDER.into_iter().fold(block, |b, dir| self.repeater_update_shape(pos, b, dir))
            }
            other => other,
        }
    }

    // ------------------------------------------------------------ torches and lamps

    fn torch_has_signal(&self, pos: Pos, torch: Block) -> bool {
        match torch {
            Block::Torch { .. } => self.has_signal(pos.down(), Dir::Down),
            Block::WallTorch { facing, .. } => {
                let toward_wall = facing.opposite();
                self.has_signal(pos.offset(toward_wall), toward_wall)
            }
            _ => false,
        }
    }

    fn torch_notify_neighbors(&mut self, pos: Pos) {
        for dir in self.delivery_order(Dir::ALL) {
            self.update_neighbors_at(pos.offset(dir));
        }
    }

    fn torch_tick(&mut self, pos: Pos, torch: Block, lit: bool) {
        let now = self.time;
        while self.toggles.first().is_some_and(|&(_, when)| now - when > BURNOUT_WINDOW) {
            self.toggles.remove(0);
        }
        let powered = self.torch_has_signal(pos, torch);
        let with_lit = |lit| match torch {
            Block::WallTorch { facing, .. } => Block::WallTorch { facing, lit },
            _ => Block::Torch { lit },
        };
        if lit {
            if powered {
                self.change(pos, with_lit(false), FLAGS_ALL);
                if self.toggled_too_often(pos, true) {
                    self.schedule(pos, torch.name(), BURNOUT_RETRY);
                }
            }
        } else if !powered && !self.toggled_too_often(pos, false) {
            self.change(pos, with_lit(true), FLAGS_ALL);
        }
    }

    fn toggled_too_often(&mut self, pos: Pos, log: bool) -> bool {
        if log {
            self.toggles.push((pos, self.time));
        }
        let limit =
            if self.mutation == Some(Mutation::BurnoutOnNinthToggle) { BURNOUT_TOGGLES + 1 } else { BURNOUT_TOGGLES };
        self.toggles.iter().filter(|(p, _)| *p == pos).count() >= limit
    }

    fn lamp_tick(&mut self, pos: Pos, lit: bool) {
        if lit && !self.has_neighbor_signal(pos) {
            self.change(pos, Block::Lamp { lit: false }, FLAGS_NO_NEIGHBORS);
        }
    }

    // ------------------------------------------------------------ scheduled ticks

    fn schedule(&mut self, pos: Pos, block: &'static str, delay: u64) {
        self.schedule_with_priority(pos, block, delay, 0);
    }

    fn schedule_with_priority(&mut self, pos: Pos, block: &'static str, delay: u64, priority: i8) {
        // The game keeps at most one pending tick per position and block.
        if self.queue.iter().any(|s| s.pos == pos && s.block == block) {
            return;
        }
        self.queue.push(Scheduled { trigger: self.time + delay, priority, seq: self.next_seq, pos, block });
        self.next_seq += 1;
    }

    fn will_tick_this_tick(&self, pos: Pos, block: &'static str) -> bool {
        self.running.contains(&(pos, block))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{Side, Wire};

    /// Builds a world from `(x, y, z, state)` entries.
    fn world(size: (i32, i32, i32), blocks: &[(i32, i32, i32, &str)]) -> World {
        let mut world = World::new(size);
        for &(x, y, z, state) in blocks {
            world.set(Pos::new(x, y, z), state.parse().unwrap());
        }
        world
    }

    fn power(sim: &Sim, x: i32, y: i32, z: i32) -> u8 {
        sim.world().get(Pos::new(x, y, z)).as_wire().expect("a wire").power
    }

    #[test]
    fn dust_loses_one_power_per_block_and_dies_at_sixteen() {
        let mut blocks = vec![(0, 0, 0, "stone")];
        for x in 1..=16 {
            blocks.push((x, 0, 0, "stone"));
            blocks.push((x, 1, 0, "redstone_wire[east=side,west=side]"));
        }
        let mut sim = Sim::new(world((17, 2, 1), &blocks));
        sim.set_block(Pos::new(0, 1, 0), Block::RedstoneBlock, false).unwrap();
        for x in 1..=16 {
            assert_eq!(power(&sim, x, 1, 0), (16 - x) as u8, "dust {x} blocks from the source");
        }
    }

    #[test]
    fn inverter_reacts_two_ticks_after_its_input() {
        // source slot, dust, dust pointing into a block, torch on top of the block
        let blocks = [
            (0, 0, 0, "stone"),
            (1, 0, 0, "stone"),
            (2, 0, 0, "stone"),
            (3, 0, 0, "stone"),
            (1, 1, 0, "redstone_wire[east=side,west=side]"),
            (2, 1, 0, "redstone_wire[east=side,west=side]"),
            (3, 1, 0, "stone"),
            (3, 2, 0, "redstone_torch[lit=true]"),
        ];
        let torch = Pos::new(3, 2, 0);
        let mut sim = Sim::new(world((4, 3, 1), &blocks));
        sim.set_block(Pos::new(0, 1, 0), Block::RedstoneBlock, false).unwrap();
        assert_eq!(sim.pending().len(), 1, "the torch schedules a tick as soon as its block is powered");
        sim.step();
        assert_eq!(sim.world().get(torch), Block::Torch { lit: true });
        sim.step();
        assert_eq!(sim.world().get(torch), Block::Torch { lit: false });
        assert!(sim.pending().is_empty());
    }

    #[test]
    fn lamp_lights_at_once_and_goes_out_four_ticks_later() {
        let blocks = [
            (0, 0, 0, "stone"),
            (1, 0, 0, "stone"),
            (1, 1, 0, "redstone_wire[east=side,west=side]"),
            (2, 1, 0, "redstone_lamp"),
        ];
        let lamp = Pos::new(2, 1, 0);
        let mut sim = Sim::new(world((3, 2, 1), &blocks));
        sim.set_block(Pos::new(0, 1, 0), Block::RedstoneBlock, false).unwrap();
        assert_eq!(sim.world().get(lamp), Block::Lamp { lit: true });
        sim.set_block(Pos::new(0, 1, 0), Block::Air, false).unwrap();
        for _ in 0..3 {
            sim.step();
            assert_eq!(sim.world().get(lamp), Block::Lamp { lit: true });
        }
        sim.step();
        assert_eq!(sim.world().get(lamp), Block::Lamp { lit: false });
    }

    #[test]
    fn a_weakly_powered_block_does_not_power_dust() {
        let blocks = [
            (0, 0, 0, "stone"),
            (1, 0, 0, "stone"),
            (3, 0, 0, "stone"),
            (1, 1, 0, "redstone_wire[east=side,west=side]"),
            (2, 1, 0, "stone"),
            (3, 1, 0, "redstone_wire[east=side,west=side]"),
        ];
        let mut sim = Sim::new(world((4, 2, 1), &blocks));
        sim.set_block(Pos::new(0, 1, 0), Block::RedstoneBlock, false).unwrap();
        assert_eq!(power(&sim, 1, 1, 0), 15);
        assert_eq!(power(&sim, 3, 1, 0), 0);
    }

    #[test]
    fn command_placed_dust_takes_its_shape_from_its_neighbours() {
        let mut sim =
            Sim::new(world((3, 2, 1), &[(0, 0, 0, "stone"), (1, 0, 0, "stone"), (0, 1, 0, "redstone_block")]));
        sim.set_block(Pos::new(1, 1, 0), Block::Wire(Wire::DOT), false).unwrap();
        let wire = sim.world().get(Pos::new(1, 1, 0)).as_wire().unwrap();
        assert_eq!((wire.west, wire.east, wire.north, wire.south), (Side::Side, Side::Side, Side::None, Side::None));
        assert_eq!(wire.power, 15);
    }

    #[test]
    fn lone_command_placed_dust_stays_a_dot() {
        let mut sim = Sim::new(world((1, 2, 1), &[(0, 0, 0, "stone")]));
        sim.set_block(Pos::new(0, 1, 0), Block::Wire(Wire::DOT), false).unwrap();
        assert!(sim.world().get(Pos::new(0, 1, 0)).as_wire().unwrap().is_dot());
    }

    /// source slot at x=0, dust, a repeater facing west with the given delay, dust
    fn repeater_line(delay: u8) -> Sim {
        let repeater = format!("repeater[facing=west,delay={delay}]");
        let blocks = [
            (0, 0, 0, "glass"),
            (1, 0, 0, "glass"),
            (2, 0, 0, "glass"),
            (3, 0, 0, "glass"),
            (1, 1, 0, "redstone_wire[east=side,west=side]"),
            (2, 1, 0, repeater.as_str()),
            (3, 1, 0, "redstone_wire[east=side,west=side]"),
        ];
        Sim::new(world((4, 2, 3), &blocks))
    }

    #[test]
    fn a_repeater_delays_by_twice_its_setting() {
        for delay in 1..=4 {
            let mut sim = repeater_line(delay);
            sim.set_block(Pos::new(0, 1, 0), Block::RedstoneBlock, false).unwrap();
            for _ in 0..(2 * delay - 1) {
                sim.step();
                assert_eq!(power(&sim, 3, 1, 0), 0, "delay {delay}: too early");
            }
            sim.step();
            assert_eq!(power(&sim, 3, 1, 0), 15, "delay {delay}: output after {} ticks", 2 * delay);
        }
    }

    #[test]
    fn a_short_pulse_is_stretched_to_the_repeaters_delay() {
        let mut sim = repeater_line(3);
        let source = Pos::new(0, 1, 0);
        sim.set_block(source, Block::RedstoneBlock, false).unwrap();
        sim.step();
        sim.step();
        sim.set_block(source, Block::Air, false).unwrap(); // a 2-tick pulse
        let mut on_ticks = 0;
        for _ in 0..20 {
            sim.step();
            on_ticks += u32::from(power(&sim, 3, 1, 0) == 15);
        }
        assert_eq!(on_ticks, 6);
    }

    #[test]
    fn a_locked_repeater_ignores_its_input() {
        let mut sim = repeater_line(1);
        // A powered repeater pointing north into the data repeater's south side.
        sim.set_block(Pos::new(2, 0, 1), Block::Glass, true).unwrap();
        sim.set_block(Pos::new(2, 1, 2), Block::RedstoneBlock, true).unwrap();
        sim.set_block(Pos::new(2, 1, 1), "repeater[facing=south,delay=1]".parse().unwrap(), false).unwrap();
        sim.wake(Pos::new(2, 1, 1));
        sim.settle(10).unwrap();
        assert_eq!(
            sim.world().get(Pos::new(2, 1, 0)).to_string(),
            "minecraft:repeater[delay=1,facing=west,locked=true,powered=false]"
        );
        sim.set_block(Pos::new(0, 1, 0), Block::RedstoneBlock, false).unwrap();
        sim.settle(10).unwrap();
        assert_eq!(power(&sim, 3, 1, 0), 0, "a locked repeater must not pass its input on");
        // Release the lock: the repeater follows its input after its delay.
        sim.set_block(Pos::new(2, 1, 2), Block::Air, false).unwrap();
        sim.settle(10).unwrap();
        assert_eq!(power(&sim, 3, 1, 0), 15);
    }

    #[test]
    fn floating_dust_is_refused() {
        let mut sim = Sim::new(World::new((1, 2, 1)));
        let err = sim.set_block(Pos::new(0, 1, 0), Block::Wire(Wire::DOT), false).unwrap_err();
        assert!(err.starts_with("unsupported"), "{err}");
    }
}
