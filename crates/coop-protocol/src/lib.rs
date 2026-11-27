//! Wire protocol for GoWR co-op.
//!
//! The host's game is the authority: it owns enemies, damage, story progress and the
//! current area. The guest sends its own player state and requests (hits it landed,
//! interactions); the host sends back snapshots and events.
//!
//! Everything here is plain data so it can be tested without the game.

use serde::{Deserialize, Serialize};

/// Bumped whenever a message layout changes. Host and guest must match.
pub const PROTOCOL_VERSION: u16 = 1;

/// Identifies a player in the session. The host is always `HOST_PLAYER`.
pub type PlayerId = u8;
pub const HOST_PLAYER: PlayerId = 0;

/// The game's own id for an enemy or object, as read from its memory.
pub type EntityId = u32;

/// An area (realm + region) as identified by the game.
pub type AreaId = u32;

/// Simulation tick counter, incremented by the host at `TICK_RATE`.
pub type Tick = u32;

/// Snapshots per second sent by the host.
pub const TICK_RATE: u32 = 30;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
    pub fn distance(self, o: Vec3) -> f32 {
        self.sub(o).length()
    }
    pub fn lerp(self, o: Vec3, t: f32) -> Vec3 {
        Vec3::new(
            self.x + (o.x - self.x) * t,
            self.y + (o.y - self.y) * t,
            self.z + (o.z - self.z) * t,
        )
    }
}

/// Facing as a unit quaternion (x, y, z, w), the form most engines store.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Quat::IDENTITY
    }
}

impl Quat {
    pub const IDENTITY: Quat = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    /// Normalised linear interpolation along the shorter arc. Good enough at 30 Hz.
    pub fn nlerp(self, o: Quat, t: f32) -> Quat {
        let dot = self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w;
        let s = if dot < 0.0 { -1.0 } else { 1.0 };
        let q = Quat {
            x: self.x + (o.x * s - self.x) * t,
            y: self.y + (o.y * s - self.y) * t,
            z: self.z + (o.z * s - self.z) * t,
            w: self.w + (o.w * s - self.w) * t,
        };
        let len = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
        if len < 1e-6 {
            return o;
        }
        Quat { x: q.x / len, y: q.y / len, z: q.z / len, w: q.w / len }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Weapon {
    #[default]
    Unarmed,
    Axe,
    Blades,
    Spear,
}

/// Which animation a character is playing and how far into it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimState {
    /// The game's animation/state id, as read from memory.
    pub id: u32,
    /// Seconds into the animation.
    pub time: f32,
}

/// Everything needed to draw and drive one player's Kratos on the other machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    pub anim: AnimState,
    pub weapon: Weapon,
    pub health: f32,
    pub max_health: f32,
    pub rage: f32,
    pub downed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EnemyState {
    pub id: EntityId,
    /// The game's enemy type id (draugr, wolf, ...).
    pub kind: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub anim: AnimState,
    pub health: f32,
    pub max_health: f32,
    /// Which player the enemy is targeting, if known.
    pub target: Option<PlayerId>,
}

/// A hit the guest's Kratos landed in the guest's copy of the world.
/// The host decides whether it counts and for how much.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HitRequest {
    pub target: EntityId,
    /// The game's attack/move id, used by the host to look up damage.
    pub attack: u32,
    /// Damage as the guest's game computed it. The host may clamp it.
    pub damage: f32,
    /// Where the guest's Kratos was when the hit landed, for range checks.
    pub from: Vec3,
    pub tick: Tick,
}

/// One-off things that happened in the host's world. Always sent reliably.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GameEvent {
    EnemyDamaged { id: EntityId, amount: f32, by: PlayerId },
    EnemyKilled { id: EntityId, by: PlayerId },
    EnemySpawned { state: EnemyState },
    PlayerDamaged { player: PlayerId, amount: f32 },
    PlayerDowned { player: PlayerId },
    PlayerRevived { player: PlayerId },
    SpartanRage { player: PlayerId, active: bool },
    CutsceneStarted { id: u32 },
    CutsceneEnded { id: u32 },
    /// Host moved to a new area; the guest should load it and appear at `spawn`.
    AreaChanged { area: AreaId, spawn: Vec3 },
    /// Guest wandered too far from the host and is pulled back.
    Teleport { pos: Vec3 },
    Chat { from: PlayerId, text: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RejectReason {
    VersionMismatch { host: u16 },
    SessionFull,
    Banned,
}

/// Guest → host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello { version: u16, name: String },
    /// Sent every tick, unreliably. Only the newest matters.
    State { tick: Tick, state: PlayerState },
    /// Reliable.
    Hit(HitRequest),
    /// Reliable. The guest's Kratos was hit by `by` in the guest's world.
    TookDamage { by: EntityId, amount: f32 },
    Chat { text: String },
    Ping { sent_ms: u64 },
    Leave,
}

/// Host → guest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome { you: PlayerId, host_name: String, area: AreaId, spawn: Vec3, tick: Tick },
    Reject(RejectReason),
    /// Sent every tick, unreliably.
    Snapshot(Snapshot),
    /// Reliable.
    Event(GameEvent),
    Pong { sent_ms: u64 },
    Kick { reason: String },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub tick: Tick,
    pub area: AreaId,
    /// Every player other than the receiver, including the host.
    pub players: Vec<(PlayerId, PlayerState)>,
    /// Enemies near any player.
    pub enemies: Vec<EnemyState>,
}

#[derive(Debug)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "decode error: {}", self.0)
    }
}
impl std::error::Error for DecodeError {}

/// Upper bound on a decoded message, so a bad packet cannot make us allocate gigabytes.
pub const MAX_MESSAGE_BYTES: u64 = 64 * 1024;

fn opts() -> impl bincode::Options {
    use bincode::Options;
    bincode::DefaultOptions::new().with_limit(MAX_MESSAGE_BYTES)
}

pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    use bincode::Options;
    opts().serialize(msg).expect("protocol messages always serialize")
}

pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, DecodeError> {
    use bincode::Options;
    opts().deserialize(bytes).map_err(|e| DecodeError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let snap = ServerMsg::Snapshot(Snapshot {
            tick: 42,
            area: 7,
            players: vec![(HOST_PLAYER, PlayerState { health: 100.0, ..Default::default() })],
            enemies: vec![EnemyState { id: 9, health: 50.0, ..Default::default() }],
        });
        assert_eq!(decode::<ServerMsg>(&encode(&snap)).unwrap(), snap);

        let hit = ClientMsg::Hit(HitRequest {
            target: 9,
            attack: 3,
            damage: 12.5,
            from: Vec3::new(1.0, 2.0, 3.0),
            tick: 41,
        });
        assert_eq!(decode::<ClientMsg>(&encode(&hit)).unwrap(), hit);
    }

    #[test]
    fn garbage_is_rejected_not_panicked() {
        assert!(decode::<ServerMsg>(&[0xff; 16]).is_err());
        assert!(decode::<ClientMsg>(&[]).is_err());
    }

    #[test]
    fn quat_nlerp_takes_short_arc() {
        let a = Quat::IDENTITY;
        let b = Quat { x: 0.0, y: 0.0, z: 0.0, w: -1.0 }; // same rotation, flipped sign
        let m = a.nlerp(b, 0.5);
        assert!((m.w.abs() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn snapshot_with_many_enemies_fits_in_limit() {
        let snap = ServerMsg::Snapshot(Snapshot {
            enemies: vec![EnemyState::default(); 64],
            players: vec![(0, PlayerState::default()); 4],
            ..Default::default()
        });
        assert!((encode(&snap).len() as u64) < MAX_MESSAGE_BYTES);
    }
}
