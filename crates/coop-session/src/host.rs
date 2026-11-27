use crate::{Game, LeaveReason, SessionEvent, TICK_MS};
use coop_net::{Connection, Link};
use coop_protocol::{
    decode, encode, AreaId, ClientMsg, GameEvent, HitRequest, PlayerId, PlayerState, RejectReason, ServerMsg,
    Snapshot, Tick, Vec3, HOST_PLAYER, PROTOCOL_VERSION,
};
use std::collections::HashMap;
use std::net::SocketAddr;

/// Enemies per snapshot packet; larger groups are split across several packets.
pub const ENEMIES_PER_CHUNK: usize = 12;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub name: String,
    /// Including the host.
    pub max_players: usize,
    /// A guest further than this from the host (metres) is teleported back.
    pub leash_distance: f32,
    /// Hits from further away than this are ignored (generous: the Axe can be thrown).
    pub max_hit_range: f32,
    /// Damage per hit is clamped to this.
    pub max_hit_damage: f32,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self { name: "Host".into(), max_players: 2, leash_distance: 80.0, max_hit_range: 60.0, max_hit_damage: 500.0 }
    }
}

struct Peer {
    id: PlayerId,
    name: String,
    conn: Connection,
    state: Option<(Tick, PlayerState)>,
}

pub struct HostSession<L: Link> {
    link: L,
    cfg: HostConfig,
    peers: HashMap<SocketAddr, Peer>,
    /// Connections we are only telling "no" to; dropped after a while.
    rejecting: Vec<(Connection, u64)>,
    tick: Tick,
    tick_accum: f64,
    last_ms: Option<u64>,
    last_area: Option<AreaId>,
    last_cutscene: Option<u32>,
}

impl<L: Link> HostSession<L> {
    pub fn new(link: L, cfg: HostConfig) -> Self {
        Self {
            link,
            cfg,
            peers: HashMap::new(),
            rejecting: Vec::new(),
            tick: 0,
            tick_accum: 0.0,
            last_ms: None,
            last_area: None,
            last_cutscene: None,
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.link.local_addr()
    }

    pub fn players(&self) -> Vec<(PlayerId, String, f32)> {
        self.peers.values().map(|p| (p.id, p.name.clone(), p.conn.rtt_ms())).collect()
    }

    pub fn tick(&self) -> Tick {
        self.tick
    }

    pub fn kick(&mut self, id: PlayerId, game: &mut impl Game, now_ms: u64) -> Option<SessionEvent> {
        let addr = *self.peers.iter().find(|(_, p)| p.id == id)?.0;
        let mut peer = self.peers.remove(&addr)?;
        game.remove_remote_player(id);
        let _ = peer.conn.send_reliable(encode(&ServerMsg::Kick { reason: "kicked by host".into() }));
        self.rejecting.push((peer.conn, now_ms));
        Some(SessionEvent::PlayerLeft { id, reason: LeaveReason::Kicked })
    }

    /// Call every frame.
    pub fn update(&mut self, game: &mut impl Game, now_ms: u64) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        let dt = self.last_ms.map_or(0, |l| now_ms.saturating_sub(l));
        self.last_ms = Some(now_ms);

        self.receive(game, now_ms, &mut out);

        // Drop silent guests.
        let gone: Vec<SocketAddr> =
            self.peers.iter().filter(|(_, p)| p.conn.timed_out(now_ms)).map(|(a, _)| *a).collect();
        for addr in gone {
            let p = self.peers.remove(&addr).unwrap();
            game.remove_remote_player(p.id);
            out.push(SessionEvent::PlayerLeft { id: p.id, reason: LeaveReason::TimedOut });
        }

        self.tick_accum += dt as f64;
        let mut ticked = self.last_area.is_none();
        while self.tick_accum >= TICK_MS {
            self.tick_accum -= TICK_MS;
            ticked = true;
        }
        if ticked {
            self.tick = self.tick.wrapping_add(1);
            self.on_tick(game);
        }

        for p in self.peers.values_mut() {
            for pkt in p.conn.flush(now_ms) {
                self.link.send_to(now_ms, p.conn.peer(), &pkt);
            }
        }
        self.rejecting.retain(|(_, since)| now_ms.saturating_sub(*since) < 3_000);
        for (c, _) in self.rejecting.iter_mut() {
            for pkt in c.flush(now_ms) {
                self.link.send_to(now_ms, c.peer(), &pkt);
            }
        }
        out
    }

    fn broadcast(&mut self, ev: GameEvent) {
        let bytes = encode(&ServerMsg::Event(ev));
        for p in self.peers.values_mut() {
            let _ = p.conn.send_reliable(bytes.clone());
        }
    }

    fn next_id(&self) -> Option<PlayerId> {
        (1..=u8::MAX).find(|id| !self.peers.values().any(|p| p.id == *id))
    }

    fn receive(&mut self, game: &mut impl Game, now_ms: u64, out: &mut Vec<SessionEvent>) {
        while let Some((from, bytes)) = self.link.recv_from(now_ms) {
            if let Some(peer) = self.peers.get_mut(&from) {
                peer.conn.receive(&bytes, now_ms);
                continue;
            }
            if let Some((c, _)) = self.rejecting.iter_mut().find(|(c, _)| c.peer() == from) {
                c.receive(&bytes, now_ms);
                continue;
            }
            // A stranger: only a Hello lets them in.
            let mut conn = Connection::new(from, now_ms);
            if !conn.receive(&bytes, now_ms) {
                continue;
            }
            let msgs = conn.drain_received();
            let Some(ClientMsg::Hello { version, name }) = msgs.first().and_then(|m| decode::<ClientMsg>(m).ok())
            else {
                continue;
            };
            let reject = if version != PROTOCOL_VERSION {
                Some(RejectReason::VersionMismatch { host: PROTOCOL_VERSION })
            } else if self.peers.len() + 1 >= self.cfg.max_players {
                Some(RejectReason::SessionFull)
            } else {
                None
            };
            if let Some(reason) = reject {
                let _ = conn.send_reliable(encode(&ServerMsg::Reject(reason)));
                self.rejecting.push((conn, now_ms));
                continue;
            }
            let Some(id) = self.next_id() else { continue };
            let host = game.local_player();
            let welcome = ServerMsg::Welcome {
                you: id,
                host_name: self.cfg.name.clone(),
                area: game.area(),
                spawn: beside(host.pos),
                tick: self.tick,
            };
            let _ = conn.send_reliable(encode(&welcome));
            let name = sanitize_name(&name);
            out.push(SessionEvent::PlayerJoined { id, name: name.clone() });
            self.peers.insert(from, Peer { id, name, conn, state: None });
        }

        // Handle everything the guests sent.
        let addrs: Vec<SocketAddr> = self.peers.keys().copied().collect();
        for addr in addrs {
            let msgs = self.peers.get_mut(&addr).unwrap().conn.drain_received();
            for m in msgs {
                let Ok(msg) = decode::<ClientMsg>(&m) else { continue };
                self.handle(addr, msg, game, out);
                if !self.peers.contains_key(&addr) {
                    break;
                }
            }
        }
    }

    fn handle(&mut self, addr: SocketAddr, msg: ClientMsg, game: &mut impl Game, out: &mut Vec<SessionEvent>) {
        let id = self.peers[&addr].id;
        match msg {
            ClientMsg::Hello { .. } => {} // duplicate
            ClientMsg::State { tick, state } => {
                let peer = self.peers.get_mut(&addr).unwrap();
                let newer = match peer.state {
                    None => true,
                    Some((t, _)) => tick.wrapping_sub(t) < u32::MAX / 2 && tick != t,
                };
                if newer {
                    peer.state = Some((tick, state));
                    game.update_remote_player(id, &state);
                }
            }
            ClientMsg::Hit(hit) => self.handle_hit(id, hit, game),
            ClientMsg::TookDamage { amount, .. } => {
                let amount = amount.clamp(0.0, self.cfg.max_hit_damage);
                self.broadcast(GameEvent::PlayerDamaged { player: id, amount });
            }
            ClientMsg::Chat { text } => {
                let text: String = text.chars().take(200).collect();
                out.push(SessionEvent::Chat { from: id, text: text.clone() });
                self.broadcast(GameEvent::Chat { from: id, text });
            }
            ClientMsg::Ping { sent_ms } => {
                let peer = self.peers.get_mut(&addr).unwrap();
                let _ = peer.conn.send_unreliable(encode(&ServerMsg::Pong { sent_ms }));
            }
            ClientMsg::Leave => {
                self.peers.remove(&addr);
                game.remove_remote_player(id);
                out.push(SessionEvent::PlayerLeft { id, reason: LeaveReason::Left });
            }
        }
    }

    fn handle_hit(&mut self, by: PlayerId, hit: HitRequest, game: &mut impl Game) {
        if !hit.damage.is_finite() || hit.damage <= 0.0 {
            return;
        }
        let Some(enemy) = game.enemies().into_iter().find(|e| e.id == hit.target) else { return };
        if enemy.pos.distance(hit.from) > self.cfg.max_hit_range {
            return;
        }
        let amount = hit.damage.min(self.cfg.max_hit_damage);
        let Some(left) = game.damage_enemy(hit.target, amount, by) else { return };
        self.broadcast(GameEvent::EnemyDamaged { id: hit.target, amount, by });
        if left <= 0.0 {
            self.broadcast(GameEvent::EnemyKilled { id: hit.target, by });
        }
    }

    fn on_tick(&mut self, game: &mut impl Game) {
        let host = game.local_player();
        let area = game.area();

        if self.last_area.is_some() && self.last_area != Some(area) {
            self.broadcast(GameEvent::AreaChanged { area, spawn: beside(host.pos) });
            for p in self.peers.values_mut() {
                p.state = None; // their old position is in the old area
            }
        }
        self.last_area = Some(area);

        let cut = game.cutscene();
        if cut != self.last_cutscene {
            if let Some(id) = self.last_cutscene {
                self.broadcast(GameEvent::CutsceneEnded { id });
            }
            if let Some(id) = cut {
                self.broadcast(GameEvent::CutsceneStarted { id });
            }
            self.last_cutscene = cut;
        }

        for ev in game.take_host_events() {
            self.broadcast(ev);
        }

        // Pull stragglers back (not during cutscenes; the game moves everyone then).
        if cut.is_none() {
            let leash = self.cfg.leash_distance;
            let mut pulled = Vec::new();
            for p in self.peers.values_mut() {
                if let Some((_, s)) = p.state {
                    if s.pos.distance(host.pos) > leash {
                        pulled.push(p.conn.peer());
                        p.state = None;
                    }
                }
            }
            for addr in pulled {
                let msg = encode(&ServerMsg::Event(GameEvent::Teleport { pos: beside(host.pos) }));
                let _ = self.peers.get_mut(&addr).unwrap().conn.send_reliable(msg);
            }
        }

        // Snapshots: every player except the receiver, enemies split into chunks.
        let enemies = game.enemies();
        let mut all_players: Vec<(PlayerId, PlayerState)> = vec![(HOST_PLAYER, host)];
        all_players.extend(self.peers.values().filter_map(|p| p.state.map(|(_, s)| (p.id, s))));
        for p in self.peers.values_mut() {
            let players: Vec<_> = all_players.iter().filter(|(id, _)| *id != p.id).copied().collect();
            let mut chunks: Vec<&[coop_protocol::EnemyState]> = enemies.chunks(ENEMIES_PER_CHUNK).collect();
            if chunks.is_empty() {
                chunks.push(&[]);
            }
            for chunk in chunks {
                let snap = Snapshot { tick: self.tick, area, players: players.clone(), enemies: chunk.to_vec() };
                let _ = p.conn.send_unreliable(encode(&ServerMsg::Snapshot(snap)));
            }
        }
    }
}

/// A spot next to `pos` for a guest to appear at.
fn beside(pos: Vec3) -> Vec3 {
    Vec3::new(pos.x + 1.5, pos.y, pos.z)
}

fn sanitize_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control()).take(24).collect();
    if n.trim().is_empty() {
        "Guest".into()
    } else {
        n
    }
}
