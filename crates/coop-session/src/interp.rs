//! Guest-side snapshot buffer. Draws the world slightly in the past so movement is smooth
//! even when packets arrive late or out of order.

use coop_protocol::{EnemyState, PlayerId, PlayerState, Snapshot, Tick, TICK_RATE};
use std::collections::BTreeMap;

/// How far behind the newest snapshot we draw, in ticks (~66 ms at 30 Hz).
pub const DELAY_TICKS: f64 = 2.0;
const KEEP: usize = 32;

#[derive(Default)]
pub struct Interpolator {
    snaps: BTreeMap<Tick, Snapshot>,
    render_tick: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    pub players: Vec<(PlayerId, PlayerState)>,
    pub enemies: Vec<EnemyState>,
}

impl Interpolator {
    /// Add a snapshot (or one chunk of it; chunks with the same tick are merged).
    pub fn push(&mut self, snap: Snapshot) {
        if let Some((&oldest, _)) = self.snaps.iter().next() {
            if self.snaps.len() >= KEEP && snap.tick < oldest {
                return;
            }
        }
        match self.snaps.get_mut(&snap.tick) {
            Some(existing) => {
                for e in snap.enemies {
                    match existing.enemies.iter_mut().find(|x| x.id == e.id) {
                        Some(x) => *x = e,
                        None => existing.enemies.push(e),
                    }
                }
                existing.players = snap.players;
            }
            None => {
                self.snaps.insert(snap.tick, snap);
            }
        }
        while self.snaps.len() > KEEP {
            let first = *self.snaps.keys().next().unwrap();
            self.snaps.remove(&first);
        }
    }

    pub fn newest_tick(&self) -> Option<Tick> {
        self.snaps.keys().next_back().copied()
    }

    /// Advance the render clock by `dt_ms` and return the interpolated world.
    pub fn sample(&mut self, dt_ms: f64) -> Option<Sample> {
        let newest = self.newest_tick()? as f64;
        let target = newest - DELAY_TICKS;
        let t = match self.render_tick {
            None => target,
            Some(t) => {
                let t = t + dt_ms * TICK_RATE as f64 / 1000.0;
                if (t - target).abs() > 4.0 {
                    target // too far off (hitch, new session): snap
                } else {
                    t + (target - t) * 0.1 // gently drift toward the target delay
                }
            }
        };
        self.render_tick = Some(t);
        Some(self.at(t))
    }

    fn at(&self, t: f64) -> Sample {
        let before = self.snaps.range(..=(t.floor().max(0.0) as Tick)).next_back();
        let after = self.snaps.range((t.floor().max(0.0) as Tick + 1)..).next();
        match (before, after) {
            (Some((&ta, a)), Some((&tb, b))) => {
                let f = ((t - ta as f64) / (tb - ta) as f64).clamp(0.0, 1.0) as f32;
                lerp_snap(a, b, f)
            }
            (Some((_, a)), None) => Sample { players: a.players.clone(), enemies: a.enemies.clone() },
            (None, Some((_, b))) => Sample { players: b.players.clone(), enemies: b.enemies.clone() },
            (None, None) => Sample::default(),
        }
    }
}

fn lerp_snap(a: &Snapshot, b: &Snapshot, f: f32) -> Sample {
    let players = b
        .players
        .iter()
        .map(|(id, pb)| match a.players.iter().find(|(ia, _)| ia == id) {
            Some((_, pa)) => (*id, lerp_player(pa, pb, f)),
            None => (*id, *pb),
        })
        .collect();
    let enemies = b
        .enemies
        .iter()
        .map(|eb| match a.enemies.iter().find(|ea| ea.id == eb.id) {
            Some(ea) => {
                let mut e = *eb;
                e.pos = ea.pos.lerp(eb.pos, f);
                e.rot = ea.rot.nlerp(eb.rot, f);
                e
            }
            None => *eb,
        })
        .collect();
    Sample { players, enemies }
}

fn lerp_player(a: &PlayerState, b: &PlayerState, f: f32) -> PlayerState {
    let mut p = *b;
    p.pos = a.pos.lerp(b.pos, f);
    p.rot = a.rot.nlerp(b.rot, f);
    p.vel = a.vel.lerp(b.vel, f);
    if a.anim.id == b.anim.id {
        p.anim.time = a.anim.time + (b.anim.time - a.anim.time) * f;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use coop_protocol::Vec3;

    fn snap(tick: Tick, x: f32) -> Snapshot {
        Snapshot {
            tick,
            players: vec![(0, PlayerState { pos: Vec3::new(x, 0.0, 0.0), ..Default::default() })],
            ..Default::default()
        }
    }

    #[test]
    fn draws_between_snapshots() {
        let mut i = Interpolator::default();
        for t in 0..10 {
            i.push(snap(t, t as f32));
        }
        let s = i.sample(0.0).unwrap();
        assert!((s.players[0].1.pos.x - 7.0).abs() < 1e-4); // newest (9) minus delay (2)
        let s = i.sample(1000.0 / 30.0 / 2.0).unwrap(); // half a tick later
        assert!(s.players[0].1.pos.x > 7.0 && s.players[0].1.pos.x < 8.0);
    }

    #[test]
    fn out_of_order_and_chunked_snapshots_merge() {
        let mut i = Interpolator::default();
        i.push(snap(5, 5.0));
        i.push(snap(3, 3.0));
        let mut chunk = Snapshot { tick: 5, ..Default::default() };
        chunk.players = snap(5, 5.0).players;
        chunk.enemies = vec![EnemyState { id: 1, ..Default::default() }];
        i.push(chunk.clone());
        chunk.enemies = vec![EnemyState { id: 2, ..Default::default() }];
        i.push(chunk);
        assert_eq!(i.newest_tick(), Some(5));
        assert_eq!(i.snaps[&5].enemies.len(), 2);
    }
}
