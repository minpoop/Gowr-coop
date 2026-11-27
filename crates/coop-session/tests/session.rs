use coop_net::sim::{SimConfig, SimLink, SimNetwork};
use coop_net::Link;
use coop_protocol::{GameEvent, RejectReason, Vec3, HOST_PLAYER};
use coop_session::sim::SimGame;
use coop_session::{GuestSession, GuestStatus, HostConfig, HostSession, LeaveReason, SessionEvent};

const FRAME_MS: u64 = 16;

struct World {
    net: SimNetwork,
    host: HostSession<SimLink>,
    host_game: SimGame,
    guest: GuestSession<SimLink>,
    guest_game: SimGame,
    now: u64,
    events: Vec<SessionEvent>,
}

impl World {
    fn new(cfg: SimConfig) -> Self {
        let net = SimNetwork::new(cfg);
        let host_link = net.link(1);
        let host_addr = host_link.local_addr();
        let host = HostSession::new(host_link, HostConfig::default());
        let guest = GuestSession::connect(net.link(2), host_addr, "Atreus", 0);
        let mut w = World {
            net,
            host,
            host_game: SimGame::new(true),
            guest,
            guest_game: SimGame::new(false),
            now: 0,
            events: Vec::new(),
        };
        w.guest_game.player.pos = Vec3::new(-500.0, 0.0, 0.0); // somewhere else entirely
        w
    }

    fn run(&mut self, ms: u64, host_walk: Vec3, guest_walk: Vec3) {
        let end = self.now + ms;
        while self.now < end {
            self.now += FRAME_MS;
            let dt = FRAME_MS as f32 / 1000.0;
            self.host_game.step(dt, host_walk);
            self.guest_game.step(dt, guest_walk);
            self.events.extend(self.host.update(&mut self.host_game, self.now));
            self.events.extend(self.guest.update(&mut self.guest_game, self.now));
        }
    }
}

fn lossy() -> SimConfig {
    SimConfig { loss: 0.1, latency_ms: 50, jitter_ms: 20, seed: 42 }
}

#[test]
fn guest_joins_and_appears_beside_host() {
    let mut w = World::new(lossy());
    w.run(2_000, Vec3::ZERO, Vec3::ZERO);
    assert!(matches!(w.guest.status(), GuestStatus::Connected { you: 1 }));
    assert!(w.events.iter().any(|e| matches!(e, SessionEvent::PlayerJoined { id: 1, name } if name == "Atreus")));
    assert!(w.guest_game.player.pos.distance(w.host_game.player.pos) < 3.0, "guest was teleported next to host");
    assert!(w.host_game.remotes.contains_key(&1), "host sees the guest's puppet");
    assert!(w.guest_game.remotes.contains_key(&HOST_PLAYER), "guest sees the host's puppet");
    assert_eq!(w.guest_game.enemies.len(), 5, "guest received the host's enemies");
}

#[test]
fn movement_stays_in_sync_over_a_bad_connection() {
    let mut w = World::new(lossy());
    w.run(1_500, Vec3::ZERO, Vec3::ZERO);
    let walk = Vec3::new(4.0, 0.0, 1.0);
    w.run(5_000, walk, walk);
    // The guest draws the host ~latency + interpolation delay in the past.
    let seen = w.guest_game.remotes[&HOST_PLAYER].pos;
    let real = w.host_game.player.pos;
    let lag_m = seen.distance(real);
    assert!(lag_m < 1.5, "host puppet is {lag_m} m behind on the guest");
    let seen = w.host_game.remotes[&1].pos;
    let lag_m = seen.distance(w.guest_game.player.pos);
    assert!(lag_m < 1.5, "guest puppet is {lag_m} m behind on the host");
    // Enemies on the guest follow the host's AI.
    for (id, e) in &w.host_game.enemies {
        let g = w.guest_game.enemies[id];
        assert!(g.pos.distance(e.pos) < 1.5, "enemy {id} off by {}", g.pos.distance(e.pos));
    }
}

#[test]
fn guest_hits_count_on_the_host_and_kills_reach_the_guest() {
    let mut w = World::new(lossy());
    w.run(2_000, Vec3::ZERO, Vec3::ZERO);
    let target = *w.host_game.enemies.keys().next().unwrap();
    w.guest_game.player.pos = w.host_game.enemies[&target].pos; // walk up to it
    w.guest_game.guest_attack(target, 40.0, 0);
    w.guest_game.guest_attack(target, 40.0, 0);
    w.run(2_000, Vec3::ZERO, Vec3::ZERO);
    assert!(!w.host_game.enemies.contains_key(&target), "host's enemy died");
    assert!(!w.guest_game.enemies.contains_key(&target), "guest's copy died too");
    assert!(w.guest_game.applied.contains(&GameEvent::EnemyKilled { id: target, by: 1 }));
}

#[test]
fn host_kills_reach_the_guest() {
    let mut w = World::new(lossy());
    w.run(2_000, Vec3::ZERO, Vec3::ZERO);
    let target = *w.host_game.enemies.keys().next().unwrap();
    w.host_game.host_attack(target, 999.0);
    w.run(1_000, Vec3::ZERO, Vec3::ZERO);
    assert!(w.guest_game.applied.contains(&GameEvent::EnemyKilled { id: target, by: HOST_PLAYER }));
    assert!(!w.guest_game.enemies.contains_key(&target));
}

#[test]
fn cheaty_hits_are_ignored() {
    let mut w = World::new(SimConfig::default());
    w.run(1_000, Vec3::ZERO, Vec3::ZERO);
    let target = *w.host_game.enemies.keys().next().unwrap();
    w.guest_game.player.pos = Vec3::new(10_000.0, 0.0, 0.0); // far away
    w.guest_game.guest_attack(target, 999.0, 0);
    w.guest_game.player.pos = w.host_game.enemies[&target].pos;
    w.guest_game.guest_attack(target, f32::NAN, 0);
    w.guest_game.guest_attack(target, -50.0, 0);
    w.run(500, Vec3::ZERO, Vec3::ZERO);
    assert_eq!(w.host_game.enemies[&target].health, 60.0);
}

#[test]
fn area_change_and_cutscenes_follow_the_host() {
    let mut w = World::new(lossy());
    w.run(2_000, Vec3::ZERO, Vec3::ZERO);
    w.host_game.cutscene = Some(77);
    w.run(500, Vec3::ZERO, Vec3::ZERO);
    assert_eq!(w.guest_game.cutscene, Some(77));
    w.host_game.cutscene = None;
    w.host_game.area = 2;
    w.host_game.enemies.clear();
    w.host_game.spawn_camp(100, Vec3::new(5.0, 0.0, 5.0), 3);
    w.run(1_500, Vec3::ZERO, Vec3::ZERO);
    assert_eq!(w.guest_game.cutscene, None);
    assert_eq!(w.guest_game.area, 2);
    assert!(w.guest_game.enemies.keys().all(|id| *id >= 100), "old area's enemies are gone");
    assert_eq!(w.guest_game.enemies.len(), 3);
}

#[test]
fn guest_that_wanders_off_is_pulled_back() {
    let mut w = World::new(SimConfig::default());
    w.run(1_000, Vec3::ZERO, Vec3::ZERO);
    w.guest_game.player.pos = Vec3::new(400.0, 0.0, 0.0);
    w.run(500, Vec3::ZERO, Vec3::ZERO);
    assert!(w.guest_game.player.pos.distance(w.host_game.player.pos) < 3.0);
}

#[test]
fn many_enemies_are_split_across_packets() {
    let mut w = World::new(lossy());
    w.host_game.spawn_camp(1_000, Vec3::new(-20.0, 0.0, 0.0), 40);
    w.run(2_500, Vec3::ZERO, Vec3::ZERO);
    assert_eq!(w.guest_game.enemies.len(), 45);
}

#[test]
fn host_notices_a_guest_that_vanishes() {
    let mut w = World::new(SimConfig::default());
    w.run(1_000, Vec3::ZERO, Vec3::ZERO);
    w.net.set_config(SimConfig { loss: 1.0, ..SimConfig::default() }); // cable unplugged
    w.run(10_000, Vec3::ZERO, Vec3::ZERO);
    assert!(w.events.contains(&SessionEvent::PlayerLeft { id: 1, reason: LeaveReason::TimedOut }));
    assert!(w.host_game.remotes.is_empty());
    assert!(matches!(w.guest.status(), GuestStatus::Disconnected(_)));
    assert!(w.guest_game.remotes.is_empty());
}

#[test]
fn guest_leaving_is_clean() {
    let mut w = World::new(SimConfig::default());
    w.run(1_000, Vec3::ZERO, Vec3::ZERO);
    w.guest.leave(w.now);
    w.run(200, Vec3::ZERO, Vec3::ZERO);
    assert!(w.events.contains(&SessionEvent::PlayerLeft { id: 1, reason: LeaveReason::Left }));
    assert!(w.host_game.remotes.is_empty());
}

#[test]
fn full_session_rejects_extra_guests() {
    let mut w = World::new(SimConfig::default());
    w.run(1_000, Vec3::ZERO, Vec3::ZERO);
    let host_addr = w.host.local_addr();
    let mut third = GuestSession::connect(w.net.link(3), host_addr, "Freya", w.now);
    let mut third_game = SimGame::new(false);
    for _ in 0..60 {
        w.run(FRAME_MS, Vec3::ZERO, Vec3::ZERO);
        third.update(&mut third_game, w.now);
    }
    assert_eq!(third.status(), &GuestStatus::Rejected(RejectReason::SessionFull));
}

#[test]
fn chat_goes_both_ways() {
    let mut w = World::new(lossy());
    w.run(1_500, Vec3::ZERO, Vec3::ZERO);
    w.guest.send_chat("BOY!");
    w.run(1_000, Vec3::ZERO, Vec3::ZERO);
    let chats = w.events.iter().filter(|e| matches!(e, SessionEvent::Chat { from: 1, text } if text == "BOY!")).count();
    assert_eq!(chats, 2, "host log + echo back to the guest");
}
