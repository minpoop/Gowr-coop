//! A simulated network with latency, jitter and packet loss, for tests and the
//! `coop-sim` tool. Deterministic for a given seed.

use crate::Link;
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

#[derive(Debug, Clone, Copy)]
pub struct SimConfig {
    /// Chance (0..=1) that a packet is dropped.
    pub loss: f32,
    pub latency_ms: u64,
    /// Extra random delay 0..jitter_ms per packet (can reorder packets).
    pub jitter_ms: u64,
    pub seed: u64,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self { loss: 0.0, latency_ms: 0, jitter_ms: 0, seed: 1 }
    }
}

struct InFlight {
    deliver_at: u64,
    from: SocketAddr,
    bytes: Vec<u8>,
}

struct Inner {
    cfg: SimConfig,
    rng: u64,
    queues: HashMap<SocketAddr, Vec<InFlight>>,
}

impl Inner {
    fn next_f32(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let v = self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (v >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[derive(Clone)]
pub struct SimNetwork(Rc<RefCell<Inner>>);

impl SimNetwork {
    pub fn new(cfg: SimConfig) -> Self {
        Self(Rc::new(RefCell::new(Inner { cfg, rng: cfg.seed.max(1), queues: HashMap::new() })))
    }

    /// A link at 10.0.0.`host`:7777.
    pub fn link(&self, host: u8) -> SimLink {
        let addr = SocketAddr::from((Ipv4Addr::new(10, 0, 0, host), 7777));
        self.0.borrow_mut().queues.entry(addr).or_default();
        SimLink { net: self.clone(), addr }
    }

    pub fn set_config(&self, cfg: SimConfig) {
        self.0.borrow_mut().cfg = cfg;
    }
}

pub struct SimLink {
    net: SimNetwork,
    addr: SocketAddr,
}

impl Link for SimLink {
    fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    fn send_to(&mut self, now_ms: u64, to: SocketAddr, bytes: &[u8]) {
        let mut n = self.net.0.borrow_mut();
        if n.next_f32() < n.cfg.loss {
            return;
        }
        let jitter = if n.cfg.jitter_ms > 0 { (n.next_f32() * n.cfg.jitter_ms as f32) as u64 } else { 0 };
        let deliver_at = now_ms + n.cfg.latency_ms + jitter;
        if let Some(q) = n.queues.get_mut(&to) {
            q.push(InFlight { deliver_at, from: self.addr, bytes: bytes.to_vec() });
        }
    }

    fn recv_from(&mut self, now_ms: u64) -> Option<(SocketAddr, Vec<u8>)> {
        let mut n = self.net.0.borrow_mut();
        let q = n.queues.get_mut(&self.addr)?;
        let idx = q
            .iter()
            .enumerate()
            .filter(|(_, p)| p.deliver_at <= now_ms)
            .min_by_key(|(_, p)| p.deliver_at)
            .map(|(i, _)| i)?;
        let p = q.remove(idx);
        Some((p.from, p.bytes))
    }
}
