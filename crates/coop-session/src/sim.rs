//! A tiny stand-in for God of War Ragnarök so the session and netcode can be tested
//! without the game. Enemies chase the nearest player; Kratos walks a scripted path.

use crate::Game;
use coop_protocol::{
    AreaId, EnemyState, EntityId, GameEvent, HitRequest, PlayerId, PlayerState, Tick, Vec3, HOST_PLAYER,
};
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub struct SimGame {
    pub is_host: bool,
    pub player: PlayerState,
    pub area: AreaId,
    pub cutscene: Option<u32>,
    pub enemies: BTreeMap<EntityId, EnemyState>,
    pub remotes: BTreeMap<PlayerId, PlayerState>,
    /// Hits/damage queued by the test or script, drained by the session.
    pub hits: Vec<HitRequest>,
    pub damage: Vec<(EntityId, f32)>,
    pub host_events: Vec<GameEvent>,
    /// Every event the session applied, for assertions.
    pub applied: Vec<GameEvent>,
    time: f32,
}

impl SimGame {
    pub fn new(is_host: bool) -> Self {
        let mut g = SimGame { is_host, area: 1, ..Default::default() };
        g.player.health = 100.0;
        g.player.max_health = 100.0;
        if is_host {
            g.spawn_camp(1, Vec3::new(20.0, 0.0, 0.0), 5);
        }
        g
    }

    /// Host only: place a group of `count` enemies around `at`.
    pub fn spawn_camp(&mut self, first_id: EntityId, at: Vec3, count: u32) {
        for i in 0..count {
            let id = first_id + i;
            self.enemies.insert(
                id,
                EnemyState {
                    id,
                    kind: 1,
                    pos: Vec3::new(at.x + i as f32 * 2.0, at.y, at.z),
                    health: 60.0,
                    max_health: 60.0,
                    ..Default::default()
                },
            );
        }
    }

    /// Advance the stand-in game by `dt` seconds. `walk` is a scripted direction for Kratos.
    pub fn step(&mut self, dt: f32, walk: Vec3) {
        self.time += dt;
        self.player.vel = walk;
        self.player.pos = Vec3::new(
            self.player.pos.x + walk.x * dt,
            self.player.pos.y + walk.y * dt,
            self.player.pos.z + walk.z * dt,
        );
        self.player.anim.time += dt;
        if !self.is_host {
            return; // the guest's enemies only move when the host says so
        }
        let mut targets: Vec<(PlayerId, Vec3)> = vec![(HOST_PLAYER, self.player.pos)];
        targets.extend(self.remotes.iter().map(|(id, s)| (*id, s.pos)));
        for e in self.enemies.values_mut() {
            let (tid, tpos) = *targets
                .iter()
                .min_by(|a, b| e.pos.distance(a.1).total_cmp(&e.pos.distance(b.1)))
                .unwrap();
            e.target = Some(tid);
            let d = tpos.sub(e.pos);
            let len = d.length();
            if len > 1.5 {
                let speed = 3.0 * dt / len;
                e.pos = Vec3::new(e.pos.x + d.x * speed, e.pos.y + d.y * speed, e.pos.z + d.z * speed);
            }
        }
    }

    /// The host's own Kratos hits an enemy.
    pub fn host_attack(&mut self, id: EntityId, amount: f32) {
        if let Some(e) = self.enemies.get_mut(&id) {
            e.health -= amount;
            self.host_events.push(GameEvent::EnemyDamaged { id, amount, by: HOST_PLAYER });
            if e.health <= 0.0 {
                self.enemies.remove(&id);
                self.host_events.push(GameEvent::EnemyKilled { id, by: HOST_PLAYER });
            }
        }
    }

    /// The guest's Kratos swings at an enemy it sees locally.
    pub fn guest_attack(&mut self, id: EntityId, amount: f32, tick: Tick) {
        self.hits.push(HitRequest { target: id, attack: 1, damage: amount, from: self.player.pos, tick });
    }
}

impl Game for SimGame {
    fn local_player(&mut self) -> PlayerState {
        self.player
    }
    fn area(&mut self) -> AreaId {
        self.area
    }
    fn cutscene(&mut self) -> Option<u32> {
        self.cutscene
    }
    fn enemies(&mut self) -> Vec<EnemyState> {
        self.enemies.values().copied().collect()
    }
    fn update_remote_player(&mut self, id: PlayerId, state: &PlayerState) {
        self.remotes.insert(id, *state);
    }
    fn remove_remote_player(&mut self, id: PlayerId) {
        self.remotes.remove(&id);
    }
    fn damage_enemy(&mut self, id: EntityId, amount: f32, _by: PlayerId) -> Option<f32> {
        let e = self.enemies.get_mut(&id)?;
        e.health -= amount;
        let left = e.health;
        if left <= 0.0 {
            self.enemies.remove(&id);
        }
        Some(left)
    }
    fn apply_enemies(&mut self, enemies: &[EnemyState]) {
        for e in enemies {
            self.enemies.insert(e.id, *e);
        }
    }
    fn apply_event(&mut self, event: &GameEvent) {
        match event {
            GameEvent::EnemyKilled { id, .. } => {
                self.enemies.remove(id);
            }
            GameEvent::AreaChanged { area, spawn } => {
                self.area = *area;
                self.player.pos = *spawn;
                self.enemies.clear();
            }
            GameEvent::Teleport { pos } => self.player.pos = *pos,
            GameEvent::PlayerDamaged { player: _, amount: _ } => {}
            GameEvent::CutsceneStarted { id } => self.cutscene = Some(*id),
            GameEvent::CutsceneEnded { .. } => self.cutscene = None,
            _ => {}
        }
        self.applied.push(event.clone());
    }
    fn take_local_hits(&mut self) -> Vec<HitRequest> {
        std::mem::take(&mut self.hits)
    }
    fn take_local_damage(&mut self) -> Vec<(EntityId, f32)> {
        std::mem::take(&mut self.damage)
    }
    fn take_host_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.host_events)
    }
}
