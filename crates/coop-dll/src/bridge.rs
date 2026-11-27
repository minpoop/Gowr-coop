//! Implements the session's [`Game`] trait on God of War Ragnarök's memory.
//!
//! What works depends on which offsets are filled in. Anything still missing is listed
//! by [`GowrBridge::report`] and simply does nothing, so the mod can be tried step by
//! step while the reverse engineering in `docs/REVERSE_ENGINEERING.md` progresses.

use crate::memory::Memory;
use crate::offsets::{resolve, EnemyFields, Offsets, PlayerFields, Resolved};
use coop_protocol::{
    AnimState, AreaId, EnemyState, EntityId, GameEvent, HitRequest, PlayerId, PlayerState, HOST_PLAYER,
};
use coop_session::Game;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Shows other players in the world. The real one (spawning a second Kratos and driving
/// its animation) is phase 2 of the roadmap; until then remote players are only tracked.
pub trait Puppets {
    fn update(&mut self, id: PlayerId, state: &PlayerState);
    fn remove(&mut self, id: PlayerId);
}

/// Placeholder: remembers where remote players are (for the overlay) without spawning anything.
#[derive(Default)]
pub struct TrackOnly {
    pub players: BTreeMap<PlayerId, PlayerState>,
}

impl Puppets for TrackOnly {
    fn update(&mut self, id: PlayerId, state: &PlayerState) {
        self.players.insert(id, *state);
    }
    fn remove(&mut self, id: PlayerId) {
        self.players.remove(&id);
    }
}

struct Located {
    player: Option<Resolved>,
    area: Option<Resolved>,
    cutscene: Option<Resolved>,
    enemies: Option<(Resolved, u64, u64, u32)>,
}

pub struct GowrBridge<M: Memory, P: Puppets> {
    mem: M,
    loc: Located,
    pf: PlayerFields,
    ef: EnemyFields,
    pub puppets: P,
    report: Vec<String>,
    /// Host: enemy health last tick, to notice kills by the host's own Kratos.
    last_alive: HashMap<EntityId, f32>,
    killed_by_guest: HashSet<EntityId>,
    warned: HashSet<&'static str>,
}

impl<M: Memory, P: Puppets> GowrBridge<M, P> {
    /// `base` is where the game's executable is loaded; `image` a copy of its bytes for
    /// signature scanning.
    pub fn new(mem: M, offsets: &Offsets, base: usize, image: &[u8], puppets: P) -> Self {
        let mut report = vec![format!("offsets for game version: {:?}", offsets.game_version)];
        let mut find = |name: &str, p: &Option<crate::offsets::Pointer>| -> Option<Resolved> {
            match p {
                None => {
                    report.push(format!("MISSING  {name}: not in offsets.toml yet"));
                    None
                }
                Some(p) => match resolve(p, base, image) {
                    Ok(r) => {
                        report.push(format!("ok       {name}: static at {:#x}", r.addr));
                        Some(r)
                    }
                    Err(e) => {
                        report.push(format!("BROKEN   {name}: {e}"));
                        None
                    }
                },
            }
        };
        let player = find("player", &offsets.player);
        let area = find("area", &offsets.area);
        let cutscene = find("cutscene", &offsets.cutscene);
        let enemies = offsets.enemies.as_ref().and_then(|e| {
            find("enemies", &Some(e.pointer.clone())).map(|r| (r, e.array_offset, e.count_offset, e.max_count))
        });
        if offsets.enemies.is_none() {
            report.push("MISSING  enemies: not in offsets.toml yet".into());
        }
        let pf = &offsets.player_fields;
        for (name, v) in [("player.pos", pf.pos), ("player.rot", pf.rot), ("player.health", pf.health)] {
            if v.is_none() {
                report.push(format!("MISSING  {name}"));
            }
        }
        let ef = &offsets.enemy_fields;
        for (name, v) in [("enemy.id", ef.id), ("enemy.pos", ef.pos), ("enemy.health", ef.health)] {
            if v.is_none() {
                report.push(format!("MISSING  {name}"));
            }
        }
        report.push("TODO     second Kratos (puppet) spawning — remote players are tracked only".into());
        report.push("TODO     hit hook — guest hits are not reported to the host yet".into());
        report.push("TODO     guest enemy AI off — guest enemies are moved but still think".into());
        Self {
            mem,
            loc: Located { player, area, cutscene, enemies },
            pf: offsets.player_fields.clone(),
            ef: offsets.enemy_fields.clone(),
            puppets,
            report,
            last_alive: HashMap::new(),
            killed_by_guest: HashSet::new(),
            warned: HashSet::new(),
        }
    }

    /// What was found and what is still missing, one line each.
    pub fn report(&self) -> &[String] {
        &self.report
    }

    fn warn_once(&mut self, key: &'static str, msg: &str) {
        if self.warned.insert(key) {
            crate::log!("{msg}");
        }
    }

    fn player_obj(&self) -> Option<usize> {
        self.loc.player.as_ref()?.get(&self.mem)
    }

    fn enemy_ptrs(&self) -> Vec<usize> {
        let Some((list, array_off, count_off, max)) = &self.loc.enemies else { return Vec::new() };
        let Some(obj) = list.get(&self.mem) else { return Vec::new() };
        let Some(count) = self.mem.read_u32(obj + *count_off as usize) else { return Vec::new() };
        let Some(array) = self.mem.read_ptr(obj + *array_off as usize) else { return Vec::new() };
        (0..count.min(*max)).filter_map(|i| self.mem.read_ptr(array + i as usize * 8)).collect()
    }

    fn read_enemy(&self, e: usize) -> Option<EnemyState> {
        let f = &self.ef;
        let at = |o: Option<u64>| o.map(|o| e + o as usize);
        Some(EnemyState {
            id: self.mem.read_u32(at(f.id)?)?,
            kind: at(f.kind).and_then(|a| self.mem.read_u32(a)).unwrap_or(0),
            pos: self.mem.read_vec3(at(f.pos)?)?,
            rot: at(f.rot).and_then(|a| self.mem.read_quat(a)).unwrap_or_default(),
            anim: AnimState { id: at(f.anim_id).and_then(|a| self.mem.read_u32(a)).unwrap_or(0), time: 0.0 },
            health: self.mem.read_f32(at(f.health)?)?,
            max_health: at(f.max_health).and_then(|a| self.mem.read_f32(a)).unwrap_or(0.0),
            target: None,
        })
    }

    fn find_enemy(&self, id: EntityId) -> Option<usize> {
        let id_off = self.ef.id? as usize;
        self.enemy_ptrs().into_iter().find(|&e| self.mem.read_u32(e + id_off) == Some(id))
    }
}

impl<M: Memory, P: Puppets> Game for GowrBridge<M, P> {
    fn local_player(&mut self) -> PlayerState {
        let Some(p) = self.player_obj() else { return PlayerState::default() };
        let f = &self.pf;
        let m = &self.mem;
        let at = |o: Option<u64>| o.map(|o| p + o as usize);
        PlayerState {
            pos: at(f.pos).and_then(|a| m.read_vec3(a)).unwrap_or_default(),
            rot: at(f.rot).and_then(|a| m.read_quat(a)).unwrap_or_default(),
            vel: at(f.vel).and_then(|a| m.read_vec3(a)).unwrap_or_default(),
            anim: AnimState {
                id: at(f.anim_id).and_then(|a| m.read_u32(a)).unwrap_or(0),
                time: at(f.anim_time).and_then(|a| m.read_f32(a)).unwrap_or(0.0),
            },
            weapon: Default::default(),
            health: at(f.health).and_then(|a| m.read_f32(a)).unwrap_or(0.0),
            max_health: at(f.max_health).and_then(|a| m.read_f32(a)).unwrap_or(0.0),
            rage: at(f.rage).and_then(|a| m.read_f32(a)).unwrap_or(0.0),
            downed: false,
        }
    }

    fn area(&mut self) -> AreaId {
        self.loc.area.as_ref().and_then(|r| r.get(&self.mem)).and_then(|a| self.mem.read_u32(a)).unwrap_or(0)
    }

    fn cutscene(&mut self) -> Option<u32> {
        let a = self.loc.cutscene.as_ref()?.get(&self.mem)?;
        self.mem.read_u32(a).filter(|&id| id != 0)
    }

    fn enemies(&mut self) -> Vec<EnemyState> {
        self.enemy_ptrs().into_iter().filter_map(|e| self.read_enemy(e)).filter(|e| e.health > 0.0).collect()
    }

    fn update_remote_player(&mut self, id: PlayerId, state: &PlayerState) {
        self.puppets.update(id, state);
    }

    fn remove_remote_player(&mut self, id: PlayerId) {
        self.puppets.remove(id);
    }

    fn damage_enemy(&mut self, id: EntityId, amount: f32, _by: PlayerId) -> Option<f32> {
        // Writing health directly skips the game's hit reaction and death handling; the
        // proper fix is calling the game's own damage function (REVERSE_ENGINEERING.md, step 7).
        let e = self.find_enemy(id)?;
        let h = e + self.ef.health? as usize;
        let left = self.mem.read_f32(h)? - amount;
        self.mem.write_f32(h, left.max(0.0));
        if left <= 0.0 {
            self.killed_by_guest.insert(id);
        }
        Some(left)
    }

    fn apply_enemies(&mut self, enemies: &[EnemyState]) {
        let (Some(pos_off), Some(id_off)) = (self.ef.pos, self.ef.id) else { return };
        let mine: HashMap<u32, usize> = self
            .enemy_ptrs()
            .into_iter()
            .filter_map(|e| Some((self.mem.read_u32(e + id_off as usize)?, e)))
            .collect();
        let mut unmatched = 0;
        for s in enemies {
            let Some(&e) = mine.get(&s.id) else {
                unmatched += 1;
                continue;
            };
            self.mem.write_vec3(e + pos_off as usize, s.pos);
            if let Some(r) = self.ef.rot {
                self.mem.write_quat(e + r as usize, s.rot);
            }
            if let Some(h) = self.ef.health {
                self.mem.write_f32(e + h as usize, s.health);
            }
        }
        if unmatched > 0 {
            self.warn_once(
                "unmatched",
                &format!("{unmatched} host enemies have no match here: enemy ids differ between PCs (see step 5)"),
            );
        }
    }

    fn apply_event(&mut self, event: &GameEvent) {
        match event {
            GameEvent::Teleport { pos } | GameEvent::AreaChanged { spawn: pos, .. } => {
                if let GameEvent::AreaChanged { area, .. } = event {
                    if *area != self.area() {
                        self.warn_once("travel", "host changed area; travelling the guest is not implemented yet");
                        return;
                    }
                }
                if let (Some(p), Some(off)) = (self.player_obj(), self.pf.pos) {
                    self.mem.write_vec3(p + off as usize, *pos);
                }
            }
            GameEvent::EnemyKilled { id, .. } => {
                if let (Some(e), Some(h)) = (self.find_enemy(*id), self.ef.health) {
                    self.mem.write_f32(e + h as usize, 0.0);
                }
            }
            _ => {}
        }
    }

    fn take_local_hits(&mut self) -> Vec<HitRequest> {
        Vec::new() // needs the hit hook
    }

    fn take_local_damage(&mut self) -> Vec<(EntityId, f32)> {
        Vec::new() // needs the hit hook
    }

    fn take_host_events(&mut self) -> Vec<GameEvent> {
        // Enemies that were alive last tick and are now dead or gone, and were not killed
        // by a guest, were killed by the host.
        let now: HashMap<EntityId, f32> = self.enemies().into_iter().map(|e| (e.id, e.health)).collect();
        let mut events = Vec::new();
        for id in self.last_alive.keys() {
            if !now.contains_key(id) && !self.killed_by_guest.remove(id) {
                events.push(GameEvent::EnemyKilled { id: *id, by: HOST_PLAYER });
            }
        }
        self.last_alive = now;
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FakeMemory;
    use coop_protocol::Vec3;

    const BASE: usize = 0x14000_0000;
    const PLAYER: usize = 0x2000_0000;
    const LIST: usize = 0x3000_0000;
    const ARRAY: usize = 0x3100_0000;
    const ENEMY: usize = 0x3200_0000;

    fn put(m: &FakeMemory, at: usize, bytes: &[u8]) {
        m.write_from(at, bytes);
    }

    /// A fake game: an executable with `mov rax,[rip+X]` pointing at a static player
    /// pointer, a player object, and a list of two enemies.
    fn fake() -> (FakeMemory, Vec<u8>, Offsets) {
        let m = FakeMemory::default();
        let mut image = vec![0x90u8; 0x2000];
        // At 0x100: 48 8B 05 <disp> 48 85 C0, pointing at 0x1000.
        image[0x100..0x103].copy_from_slice(&[0x48, 0x8B, 0x05]);
        let disp = (0x1000i32 - (0x100 + 7)) as i32;
        image[0x103..0x107].copy_from_slice(&disp.to_le_bytes());
        image[0x107..0x10A].copy_from_slice(&[0x48, 0x85, 0xC0]);
        image[0x1000..0x1008].copy_from_slice(&(PLAYER as u64).to_le_bytes());
        image[0x1100..0x1108].copy_from_slice(&(LIST as u64).to_le_bytes());
        m.map(BASE, image.clone());

        m.map(PLAYER, vec![0; 0x200]);
        put(&m, PLAYER + 0x80, &[0; 12]);
        m.write_vec3(PLAYER + 0x80, Vec3::new(1.0, 2.0, 3.0));
        m.write_f32(PLAYER + 0x1A0, 87.5);

        m.map(LIST, vec![0; 0x20]);
        put(&m, LIST + 0x8, &(ARRAY as u64).to_le_bytes());
        put(&m, LIST + 0x10, &2u32.to_le_bytes());
        m.map(ARRAY, vec![0; 16]);
        put(&m, ARRAY, &(ENEMY as u64).to_le_bytes());
        put(&m, ARRAY + 8, &((ENEMY + 0x1000) as u64).to_le_bytes());
        m.map(ENEMY, vec![0; 0x2000]);
        for (i, id) in [(0usize, 501u32), (1, 502)] {
            let e = ENEMY + i * 0x1000;
            put(&m, e + 0x10, &id.to_le_bytes());
            m.write_vec3(e + 0x40, Vec3::new(10.0 * (i + 1) as f32, 0.0, 0.0));
            m.write_f32(e + 0x90, 60.0);
        }

        let offsets = Offsets::parse(
            r#"
            [player]
            locate = { pattern = "48 8B 05 ?? ?? ?? ?? 48 85 C0", disp_at = 3, insn_len = 7 }
            chain = [0x0]
            [player_fields]
            pos = 0x80
            health = 0x1A0
            [enemies]
            pointer = { locate = { rva = 0x1100 }, chain = [0x0] }
            array_offset = 0x8
            count_offset = 0x10
            [enemy_fields]
            id = 0x10
            pos = 0x40
            health = 0x90
            "#,
        )
        .unwrap();
        (m, image, offsets)
    }

    #[test]
    fn reads_the_player_through_a_signature_and_pointer() {
        let (m, image, o) = fake();
        let mut b = GowrBridge::new(m, &o, BASE, &image, TrackOnly::default());
        let p = b.local_player();
        assert_eq!(p.pos, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(p.health, 87.5);
        assert!(b.report().iter().any(|l| l.starts_with("ok       player")));
        assert!(b.report().iter().any(|l| l.starts_with("MISSING  area")));
    }

    #[test]
    fn reads_enemies_and_applies_damage() {
        let (m, image, o) = fake();
        let mut b = GowrBridge::new(m, &o, BASE, &image, TrackOnly::default());
        let es = b.enemies();
        assert_eq!(es.iter().map(|e| e.id).collect::<Vec<_>>(), vec![501, 502]);
        assert_eq!(b.damage_enemy(502, 25.0, 1), Some(35.0));
        assert_eq!(b.damage_enemy(999, 25.0, 1), None);
    }

    #[test]
    fn notices_host_kills_but_not_guest_kills() {
        let (m, image, o) = fake();
        let mut b = GowrBridge::new(m, &o, BASE, &image, TrackOnly::default());
        assert!(b.take_host_events().is_empty());
        b.damage_enemy(501, 100.0, 1); // guest kill
        b.mem.write_f32(ENEMY + 0x1000 + 0x90, 0.0); // host's Kratos kills 502
        let ev = b.take_host_events();
        assert_eq!(ev, vec![GameEvent::EnemyKilled { id: 502, by: HOST_PLAYER }]);
    }

    #[test]
    fn guest_follows_host_enemies_and_teleports() {
        let (m, image, o) = fake();
        let mut b = GowrBridge::new(m, &o, BASE, &image, TrackOnly::default());
        let mut host_view = b.enemies();
        host_view[0].pos = Vec3::new(-4.0, 1.0, 0.0);
        b.apply_enemies(&host_view);
        assert_eq!(b.enemies()[0].pos, Vec3::new(-4.0, 1.0, 0.0));
        b.apply_event(&GameEvent::Teleport { pos: Vec3::new(7.0, 0.0, 7.0) });
        assert_eq!(b.local_player().pos, Vec3::new(7.0, 0.0, 7.0));
    }

    #[test]
    fn nothing_filled_in_is_harmless() {
        let m = FakeMemory::default();
        let mut b = GowrBridge::new(m, &Offsets::default(), BASE, &[], TrackOnly::default());
        assert_eq!(b.local_player(), PlayerState::default());
        assert!(b.enemies().is_empty());
        assert_eq!(b.cutscene(), None);
        b.apply_event(&GameEvent::Teleport { pos: Vec3::ZERO });
    }
}
