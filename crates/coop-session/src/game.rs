//! The boundary between the co-op session and a game.
//!
//! The real implementation lives in `coop-dll` and reads/writes God of War Ragnarök's
//! memory. [`crate::sim::SimGame`] is a stand-in used by tests and `coop-sim`.

use coop_protocol::{AreaId, EnemyState, EntityId, GameEvent, HitRequest, PlayerId, PlayerState};

pub trait Game {
    /// This machine's own Kratos.
    fn local_player(&mut self) -> PlayerState;

    /// The area this machine is in.
    fn area(&mut self) -> AreaId;

    /// The cutscene playing right now, if any.
    fn cutscene(&mut self) -> Option<u32>;

    /// Host: enemies worth syncing (near any player).
    fn enemies(&mut self) -> Vec<EnemyState>;

    /// Show and drive another player's character (a "puppet") from network state.
    fn update_remote_player(&mut self, id: PlayerId, state: &PlayerState);

    /// That player left: remove their puppet.
    fn remove_remote_player(&mut self, id: PlayerId);

    /// Host: apply damage a guest dealt. Returns the enemy's health afterwards,
    /// or `None` if there is no such enemy (already dead, despawned, wrong area).
    fn damage_enemy(&mut self, id: EntityId, amount: f32, by: PlayerId) -> Option<f32>;

    /// Guest: make local enemies match the host's (AI is off on the guest).
    fn apply_enemies(&mut self, enemies: &[EnemyState]);

    /// Guest: something happened in the host's world (death, area change, teleport...).
    fn apply_event(&mut self, event: &GameEvent);

    /// Guest: hits this machine's Kratos landed since the last call.
    fn take_local_hits(&mut self) -> Vec<HitRequest>;

    /// Guest: damage this machine's Kratos took since the last call, as (attacker, amount).
    fn take_local_damage(&mut self) -> Vec<(EntityId, f32)>;

    /// Host: things that happened in the host's own world to tell guests about
    /// (an enemy the host killed, the host going down, Spartan Rage...).
    fn take_host_events(&mut self) -> Vec<GameEvent>;
}
