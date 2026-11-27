//! `gowr-coop.dll`: loaded into God of War Ragnarök (PC) by an ASI loader.
//!
//! * [`config`] — `gowr-coop.toml`: host or join, address, name.
//! * [`offsets`] — `offsets.toml`: where things live in the game's memory, found by
//!   reverse engineering (see `docs/REVERSE_ENGINEERING.md`). Byte signatures are
//!   preferred over raw addresses so a game patch does not break everything.
//! * [`pattern`] — byte-signature scanning.
//! * [`memory`] — crash-safe reads and writes of the game's memory.
//! * [`bridge`] — implements the session's `Game` trait on top of the game's memory.
//! * `entry` (Windows only) — DllMain, logging and the session thread.

pub mod bridge;
pub mod config;
pub mod log;
pub mod memory;
pub mod offsets;
pub mod pattern;

#[cfg(windows)]
mod entry;
