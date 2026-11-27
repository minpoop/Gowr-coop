//! `offsets.toml`: where the game keeps things, found by reverse engineering.
//! Everything is optional; the mod reports what is still missing at startup.

use crate::memory::Memory;
use crate::pattern::{rip_relative, Pattern};
use serde::Deserialize;

/// How to find a static address in the game's executable.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged, deny_unknown_fields)]
pub enum Locate {
    /// A byte signature of an instruction that references the address RIP-relatively.
    /// Survives most game patches.
    Pattern {
        pattern: String,
        /// Byte offset of the 32-bit displacement inside the instruction.
        disp_at: usize,
        /// Length of the instruction.
        insn_len: usize,
        /// Which match to use if the pattern matches more than once.
        #[serde(default)]
        nth: usize,
    },
    /// Offset from the start of the executable (`GoWR.exe + rva`). Breaks on every patch.
    Rva { rva: u64 },
}

/// A static pointer plus the chain of offsets leading from it to an object.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pointer {
    pub locate: Locate,
    #[serde(default)]
    pub chain: Vec<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlayerFields {
    pub pos: Option<u64>,
    pub rot: Option<u64>,
    pub vel: Option<u64>,
    pub health: Option<u64>,
    pub max_health: Option<u64>,
    pub rage: Option<u64>,
    pub anim_id: Option<u64>,
    pub anim_time: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnemyFields {
    /// Should be the same on both PCs: an encounter/spawn id from level data, not a pointer.
    pub id: Option<u64>,
    pub kind: Option<u64>,
    pub pos: Option<u64>,
    pub rot: Option<u64>,
    pub health: Option<u64>,
    pub max_health: Option<u64>,
    pub anim_id: Option<u64>,
}

/// A list of enemies: `pointer` leads to the list object, which holds a pointer to an
/// array of entity pointers and a count.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnemyList {
    pub pointer: Pointer,
    pub array_offset: u64,
    pub count_offset: u64,
    #[serde(default = "default_max")]
    pub max_count: u32,
}

fn default_max() -> u32 {
    256
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Offsets {
    /// The game build these were found on, shown in the log.
    #[serde(default)]
    pub game_version: String,
    pub player: Option<Pointer>,
    #[serde(default)]
    pub player_fields: PlayerFields,
    /// A u32 that changes when you change area/realm.
    pub area: Option<Pointer>,
    /// A u32: non-zero cutscene id while one plays.
    pub cutscene: Option<Pointer>,
    pub enemies: Option<EnemyList>,
    #[serde(default)]
    pub enemy_fields: EnemyFields,
}

impl Offsets {
    pub fn parse(text: &str) -> Result<Self, String> {
        let o: Offsets = toml::from_str(text).map_err(|e| e.to_string())?;
        for p in [&o.player, &o.area, &o.cutscene].into_iter().flatten() {
            p.locate.check()?;
        }
        if let Some(e) = &o.enemies {
            e.pointer.locate.check()?;
        }
        Ok(o)
    }
}

impl Locate {
    fn check(&self) -> Result<(), String> {
        if let Locate::Pattern { pattern, .. } = self {
            Pattern::parse(pattern)?;
        }
        Ok(())
    }

    /// Absolute address, given the executable's base and a copy of its bytes.
    pub fn resolve(&self, base: usize, image: &[u8]) -> Result<usize, String> {
        match self {
            Locate::Rva { rva } => Ok(base + *rva as usize),
            Locate::Pattern { pattern, disp_at, insn_len, nth } => {
                let p = Pattern::parse(pattern)?;
                let hits = p.find_all(image);
                let at = *hits.get(*nth).ok_or_else(|| {
                    format!("signature not found ({} matches, wanted #{nth}): `{pattern}`", hits.len())
                })?;
                let rva = rip_relative(image, at, *disp_at, *insn_len).ok_or("signature at end of image")?;
                Ok(base + rva)
            }
        }
    }
}

/// A pointer whose static part has been located; following the chain happens every read
/// because the game moves objects around (loading, respawning).
#[derive(Debug, Clone)]
pub struct Resolved {
    pub addr: usize,
    pub chain: Vec<u64>,
}

impl Resolved {
    pub fn get(&self, mem: &impl Memory) -> Option<usize> {
        mem.follow(self.addr, &self.chain)
    }
}

pub fn resolve(p: &Pointer, base: usize, image: &[u8]) -> Result<Resolved, String> {
    Ok(Resolved { addr: p.locate.resolve(base, image)?, chain: p.chain.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_and_is_empty() {
        let o = Offsets::parse(include_str!("../../../config/offsets.template.toml")).unwrap();
        assert!(o.player.is_none(), "template ships with nothing filled in");
    }

    #[test]
    fn filled_in_example_parses() {
        let o = Offsets::parse(
            r#"
            game_version = "test"
            [player]
            locate = { pattern = "48 8B 05 ?? ?? ?? ?? 48 85 C0", disp_at = 3, insn_len = 7 }
            chain = [0x10, 0x28]
            [player_fields]
            pos = 0x80
            health = 0x1A0
            [area]
            locate = { rva = 0x4000 }
            [enemies]
            pointer = { locate = { rva = 0x5000 }, chain = [0x0] }
            array_offset = 0x8
            count_offset = 0x10
            "#,
        )
        .unwrap();
        assert_eq!(o.player.unwrap().chain, vec![0x10, 0x28]);
        assert_eq!(o.player_fields.health, Some(0x1A0));
        assert_eq!(o.area.unwrap().locate, Locate::Rva { rva: 0x4000 });
    }

    #[test]
    fn bad_signature_is_caught_at_load() {
        let e = Offsets::parse("[player]\nlocate = { pattern = \"ZZ\", disp_at = 3, insn_len = 7 }").unwrap_err();
        assert!(e.contains("bad byte"), "{e}");
    }
}
