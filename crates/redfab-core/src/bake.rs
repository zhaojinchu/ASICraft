//! Computes the state a freshly stamped layout should be saved in.
//!
//! Tiles are written without dust shapes, power levels or torch states. Pasting
//! blocks into a world does not make the game work those out (a paste triggers
//! no updates), so the build does it here: with every input off, give each wire
//! its shape, let the circuit run until nothing is scheduled, and keep the result.

use crate::sim::Sim;
use crate::world::World;

/// The longest a design may take to come to rest from its template state.
const MAX_SETTLE_TICKS: u32 = 2000;

/// The settled, self-consistent state of `world` with all inputs off.
pub fn bake(world: World) -> Result<World, String> {
    let mut sim = Sim::new(world);
    sim.check_all_supported()?;
    sim.fix_wire_shapes();
    sim.poke_all();
    sim.settle(MAX_SETTLE_TICKS)?;

    // The result must be a fixed point: updating everything again changes nothing.
    let settled = sim.world().clone();
    sim.poke_all();
    if !sim.pending().is_empty() || sim.world() != &settled {
        return Err("the settled state is not stable: another round of updates changed it".into());
    }
    Ok(settled)
}
