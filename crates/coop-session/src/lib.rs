//! Co-op session logic: who is connected, what gets sent each tick, and how the guest's
//! world is kept in line with the host's. No game memory is touched here; that goes
//! through the [`Game`] trait.

pub mod game;
pub mod guest;
pub mod host;
pub mod interp;
pub mod sim;

pub use game::Game;
pub use guest::{GuestSession, GuestStatus};
pub use host::{HostConfig, HostSession};

use coop_protocol::PlayerId;

/// Things the overlay/log should show.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    PlayerJoined { id: PlayerId, name: String },
    PlayerLeft { id: PlayerId, reason: LeaveReason },
    Chat { from: PlayerId, text: String },
    Connected { you: PlayerId, host_name: String },
    Rejected(coop_protocol::RejectReason),
    Disconnected { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LeaveReason {
    Left,
    TimedOut,
    Kicked,
}

pub(crate) const TICK_MS: f64 = 1000.0 / coop_protocol::TICK_RATE as f64;
