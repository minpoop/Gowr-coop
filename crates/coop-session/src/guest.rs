use crate::interp::Interpolator;
use crate::{Game, SessionEvent, TICK_MS};
use coop_net::{Connection, Link};
use coop_protocol::{decode, encode, ClientMsg, EntityId, GameEvent, PlayerId, RejectReason, ServerMsg, Tick, PROTOCOL_VERSION};
use std::collections::HashSet;
use std::net::SocketAddr;

/// Give up connecting after this long.
pub const CONNECT_TIMEOUT_MS: u64 = 10_000;
const PING_EVERY_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq)]
pub enum GuestStatus {
    Connecting,
    Connected { you: PlayerId },
    Rejected(RejectReason),
    Disconnected(String),
}

pub struct GuestSession<L: Link> {
    link: L,
    conn: Connection,
    name: String,
    status: GuestStatus,
    started_ms: u64,
    last_ms: Option<u64>,
    tick: Tick,
    tick_accum: f64,
    interp: Interpolator,
    remotes: HashSet<PlayerId>,
    /// Enemies the host said are dead. Delayed snapshots must not bring them back.
    dead: HashSet<EntityId>,
    in_cutscene: bool,
    last_ping_ms: u64,
    ping_ms: Option<f32>,
}

impl<L: Link> GuestSession<L> {
    pub fn connect(link: L, host: SocketAddr, name: &str, now_ms: u64) -> Self {
        let mut conn = Connection::new(host, now_ms);
        let _ = conn.send_reliable(encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, name: name.into() }));
        Self {
            link,
            conn,
            name: name.into(),
            status: GuestStatus::Connecting,
            started_ms: now_ms,
            last_ms: None,
            tick: 0,
            tick_accum: 0.0,
            interp: Interpolator::default(),
            remotes: HashSet::new(),
            dead: HashSet::new(),
            in_cutscene: false,
            last_ping_ms: 0,
            ping_ms: None,
        }
    }

    pub fn status(&self) -> &GuestStatus {
        &self.status
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Round trip to the host, from pings.
    pub fn ping_ms(&self) -> Option<f32> {
        self.ping_ms
    }

    pub fn send_chat(&mut self, text: &str) {
        let _ = self.conn.send_reliable(encode(&ClientMsg::Chat { text: text.chars().take(200).collect() }));
    }

    /// Tell the host we are leaving and send it right away.
    pub fn leave(&mut self, now_ms: u64) {
        let _ = self.conn.send_reliable(encode(&ClientMsg::Leave));
        for pkt in self.conn.flush(now_ms) {
            self.link.send_to(now_ms, self.conn.peer(), &pkt);
        }
        self.status = GuestStatus::Disconnected("left".into());
    }

    /// Call every frame.
    pub fn update(&mut self, game: &mut impl Game, now_ms: u64) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        let dt = self.last_ms.map_or(0, |l| now_ms.saturating_sub(l));
        self.last_ms = Some(now_ms);

        if matches!(self.status, GuestStatus::Rejected(_) | GuestStatus::Disconnected(_)) {
            return out;
        }

        while let Some((from, bytes)) = self.link.recv_from(now_ms) {
            if from == self.conn.peer() {
                self.conn.receive(&bytes, now_ms);
            }
        }
        for m in self.conn.drain_received() {
            let Ok(msg) = decode::<ServerMsg>(&m) else { continue };
            self.handle(msg, game, now_ms, &mut out);
        }

        match self.status {
            GuestStatus::Connecting if now_ms.saturating_sub(self.started_ms) > CONNECT_TIMEOUT_MS => {
                self.status = GuestStatus::Disconnected("host did not answer".into());
                out.push(SessionEvent::Disconnected { reason: "host did not answer".into() });
                return out;
            }
            GuestStatus::Connected { .. } if self.conn.timed_out(now_ms) => {
                self.drop_remotes(game);
                self.status = GuestStatus::Disconnected("lost connection to host".into());
                out.push(SessionEvent::Disconnected { reason: "lost connection to host".into() });
                return out;
            }
            _ => {}
        }

        if let GuestStatus::Connected { .. } = self.status {
            // Draw the host's world as of slightly in the past.
            if let Some(mut sample) = self.interp.sample(dt as f64) {
                sample.enemies.retain(|e| !self.dead.contains(&e.id));
                for (id, s) in &sample.players {
                    self.remotes.insert(*id);
                    game.update_remote_player(*id, s);
                }
                game.apply_enemies(&sample.enemies);
            }

            self.tick_accum += dt as f64;
            let mut ticked = false;
            while self.tick_accum >= TICK_MS {
                self.tick_accum -= TICK_MS;
                ticked = true;
            }
            if ticked {
                self.tick = self.tick.wrapping_add(1);
                if !self.in_cutscene {
                    let state = game.local_player();
                    let _ = self.conn.send_unreliable(encode(&ClientMsg::State { tick: self.tick, state }));
                }
                for hit in game.take_local_hits() {
                    let _ = self.conn.send_reliable(encode(&ClientMsg::Hit(hit)));
                }
                for (by, amount) in game.take_local_damage() {
                    let _ = self.conn.send_reliable(encode(&ClientMsg::TookDamage { by, amount }));
                }
            }
            if now_ms.saturating_sub(self.last_ping_ms) >= PING_EVERY_MS {
                self.last_ping_ms = now_ms;
                let _ = self.conn.send_unreliable(encode(&ClientMsg::Ping { sent_ms: now_ms }));
            }
        }

        for pkt in self.conn.flush(now_ms) {
            self.link.send_to(now_ms, self.conn.peer(), &pkt);
        }
        out
    }

    fn drop_remotes(&mut self, game: &mut impl Game) {
        for id in self.remotes.drain() {
            game.remove_remote_player(id);
        }
    }

    fn handle(&mut self, msg: ServerMsg, game: &mut impl Game, now_ms: u64, out: &mut Vec<SessionEvent>) {
        match msg {
            ServerMsg::Welcome { you, host_name, area, spawn, tick } => {
                if self.status != GuestStatus::Connecting {
                    return;
                }
                self.status = GuestStatus::Connected { you };
                self.tick = tick;
                if game.area() != area {
                    game.apply_event(&GameEvent::AreaChanged { area, spawn });
                } else {
                    game.apply_event(&GameEvent::Teleport { pos: spawn });
                }
                out.push(SessionEvent::Connected { you, host_name });
            }
            ServerMsg::Reject(reason) => {
                self.status = GuestStatus::Rejected(reason.clone());
                out.push(SessionEvent::Rejected(reason));
            }
            ServerMsg::Kick { reason } => {
                self.drop_remotes(game);
                self.status = GuestStatus::Disconnected(reason.clone());
                out.push(SessionEvent::Disconnected { reason });
            }
            ServerMsg::Snapshot(snap) => {
                if matches!(self.status, GuestStatus::Connected { .. }) && snap.area == game.area() {
                    self.interp.push(snap);
                }
            }
            ServerMsg::Event(ev) => {
                match &ev {
                    GameEvent::AreaChanged { .. } => {
                        self.interp = Interpolator::default(); // old area's snapshots are useless
                        self.dead.clear();
                    }
                    GameEvent::EnemyKilled { id, .. } => {
                        self.dead.insert(*id);
                    }
                    GameEvent::EnemySpawned { state } => {
                        self.dead.remove(&state.id); // ids can be reused when a camp respawns
                    }
                    GameEvent::CutsceneStarted { .. } => self.in_cutscene = true,
                    GameEvent::CutsceneEnded { .. } => self.in_cutscene = false,
                    GameEvent::Chat { from, text } => out.push(SessionEvent::Chat { from: *from, text: text.clone() }),
                    _ => {}
                }
                game.apply_event(&ev);
            }
            ServerMsg::Pong { sent_ms } => {
                let sample = now_ms.saturating_sub(sent_ms) as f32;
                self.ping_ms = Some(match self.ping_ms {
                    None => sample,
                    Some(p) => p * 0.8 + sample * 0.2,
                });
            }
        }
    }
}
