//! UDP transport for GoWR co-op.
//!
//! One [`Connection`] holds the reliability state for one peer. It does no I/O itself:
//! you feed it packets with [`Connection::receive`] and send whatever
//! [`Connection::flush`] returns through a [`Link`]. That keeps it testable with
//! a simulated, lossy network ([`sim::SimNetwork`]).
//!
//! * Unreliable messages are sent once (player state, snapshots — only the newest matters).
//! * Reliable messages are resent until acknowledged and delivered in order
//!   (hits, deaths, area changes, cutscenes).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::SocketAddr;

pub mod sim;

/// First four bytes of every packet: "GWCO".
pub const MAGIC: u32 = 0x4757_434f;
/// Stay under typical internet MTU so packets are not fragmented.
pub const MAX_PACKET_BYTES: usize = 1200;
/// Largest single message. Bigger payloads must be split by the caller.
pub const MAX_MESSAGE_BYTES: usize = MAX_PACKET_BYTES - 64;
/// A peer we have not heard from in this long is gone.
pub const TIMEOUT_MS: u64 = 8_000;
/// Send something at least this often so the peer knows we are alive.
pub const KEEPALIVE_MS: u64 = 250;
const MIN_RESEND_MS: u64 = 50;
const MAX_RESEND_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Frame {
    Unreliable(Vec<u8>),
    Reliable { id: u32, data: Vec<u8> },
    Ack(Vec<u32>),
    KeepAlive,
}

#[derive(Debug, Serialize, Deserialize)]
struct Packet {
    magic: u32,
    frames: Vec<Frame>,
}

fn frame_size(f: &Frame) -> usize {
    bincode::serialized_size(f).unwrap_or(u64::MAX) as usize
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    TooLarge { len: usize },
}

struct Pending {
    data: Vec<u8>,
    first_sent_ms: Option<u64>,
    last_sent_ms: Option<u64>,
    sends: u32,
}

/// Reliability state for one peer.
pub struct Connection {
    peer: SocketAddr,
    next_id: u32,
    pending: BTreeMap<u32, Pending>,
    unreliable_out: Vec<Vec<u8>>,
    acks_out: Vec<u32>,
    recv_next: u32,
    recv_buffer: BTreeMap<u32, Vec<u8>>,
    inbox: Vec<Vec<u8>>,
    last_recv_ms: u64,
    last_send_ms: u64,
    rtt_ms: f32,
    stats: Stats,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub resends: u64,
    pub bad_packets: u64,
}

impl Connection {
    pub fn new(peer: SocketAddr, now_ms: u64) -> Self {
        Self {
            peer,
            next_id: 0,
            pending: BTreeMap::new(),
            unreliable_out: Vec::new(),
            acks_out: Vec::new(),
            recv_next: 0,
            recv_buffer: BTreeMap::new(),
            inbox: Vec::new(),
            last_recv_ms: now_ms,
            last_send_ms: 0,
            rtt_ms: 100.0,
            stats: Stats::default(),
        }
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Smoothed round-trip time, measured from reliable acks.
    pub fn rtt_ms(&self) -> f32 {
        self.rtt_ms
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Reliable messages sent but not yet acknowledged.
    pub fn unacked(&self) -> usize {
        self.pending.len()
    }

    pub fn timed_out(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_recv_ms) > TIMEOUT_MS
    }

    pub fn send_unreliable(&mut self, data: Vec<u8>) -> Result<(), SendError> {
        check_len(&data)?;
        self.unreliable_out.push(data);
        Ok(())
    }

    pub fn send_reliable(&mut self, data: Vec<u8>) -> Result<(), SendError> {
        check_len(&data)?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.pending.insert(id, Pending { data, first_sent_ms: None, last_sent_ms: None, sends: 0 });
        Ok(())
    }

    /// Feed one packet that arrived from this peer. Returns false if it was not ours.
    pub fn receive(&mut self, bytes: &[u8], now_ms: u64) -> bool {
        let packet: Packet = match bincode::deserialize(bytes) {
            Ok(p) => p,
            Err(_) => {
                self.stats.bad_packets += 1;
                return false;
            }
        };
        if packet.magic != MAGIC {
            self.stats.bad_packets += 1;
            return false;
        }
        self.stats.packets_received += 1;
        self.last_recv_ms = now_ms;
        for frame in packet.frames {
            match frame {
                Frame::Unreliable(data) => self.inbox.push(data),
                Frame::Reliable { id, data } => {
                    // Always ack, even duplicates: our earlier ack may have been lost.
                    self.acks_out.push(id);
                    let ahead = id.wrapping_sub(self.recv_next);
                    if ahead < u32::MAX / 2 {
                        self.recv_buffer.entry(id).or_insert(data);
                    }
                    while let Some(d) = self.recv_buffer.remove(&self.recv_next) {
                        self.inbox.push(d);
                        self.recv_next = self.recv_next.wrapping_add(1);
                    }
                }
                Frame::Ack(ids) => {
                    for id in ids {
                        if let Some(p) = self.pending.remove(&id) {
                            // Karn's rule: only time messages sent once.
                            if let (1, Some(t)) = (p.sends, p.first_sent_ms) {
                                let sample = now_ms.saturating_sub(t) as f32;
                                self.rtt_ms = self.rtt_ms * 0.875 + sample * 0.125;
                            }
                        }
                    }
                }
                Frame::KeepAlive => {}
            }
        }
        true
    }

    /// Messages received since the last call, reliable ones in order.
    pub fn drain_received(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.inbox)
    }

    fn resend_ms(&self) -> u64 {
        ((self.rtt_ms * 1.5) as u64).clamp(MIN_RESEND_MS, MAX_RESEND_MS)
    }

    /// Build the packets to send now. Call once per tick.
    pub fn flush(&mut self, now_ms: u64) -> Vec<Vec<u8>> {
        let mut frames: Vec<Frame> = Vec::new();
        self.acks_out.sort_unstable();
        self.acks_out.dedup();
        for chunk in self.acks_out.chunks(64) {
            frames.push(Frame::Ack(chunk.to_vec()));
        }
        self.acks_out.clear();

        let resend = self.resend_ms();
        for (&id, p) in self.pending.iter_mut() {
            let due = match p.last_sent_ms {
                None => true,
                Some(t) => now_ms.saturating_sub(t) >= resend,
            };
            if due {
                if p.sends > 0 {
                    self.stats.resends += 1;
                }
                p.sends += 1;
                p.first_sent_ms.get_or_insert(now_ms);
                p.last_sent_ms = Some(now_ms);
                frames.push(Frame::Reliable { id, data: p.data.clone() });
            }
        }
        for data in self.unreliable_out.drain(..) {
            frames.push(Frame::Unreliable(data));
        }

        if frames.is_empty() && now_ms.saturating_sub(self.last_send_ms) >= KEEPALIVE_MS {
            frames.push(Frame::KeepAlive);
        }
        if frames.is_empty() {
            return Vec::new();
        }

        // Pack frames greedily into MTU-sized packets.
        let header = 4 + 8; // magic + vec length
        let mut packets = Vec::new();
        let mut current: Vec<Frame> = Vec::new();
        let mut size = header;
        for f in frames {
            let fs = frame_size(&f);
            if !current.is_empty() && size + fs > MAX_PACKET_BYTES {
                packets.push(std::mem::take(&mut current));
                size = header;
            }
            size += fs;
            current.push(f);
        }
        if !current.is_empty() {
            packets.push(current);
        }
        self.last_send_ms = now_ms;
        self.stats.packets_sent += packets.len() as u64;
        packets
            .into_iter()
            .map(|frames| bincode::serialize(&Packet { magic: MAGIC, frames }).expect("serialize packet"))
            .collect()
    }
}

fn check_len(data: &[u8]) -> Result<(), SendError> {
    if data.len() > MAX_MESSAGE_BYTES {
        Err(SendError::TooLarge { len: data.len() })
    } else {
        Ok(())
    }
}

/// Something that can move datagrams: a real UDP socket or the simulated network.
pub trait Link {
    fn local_addr(&self) -> SocketAddr;
    fn send_to(&mut self, now_ms: u64, to: SocketAddr, bytes: &[u8]);
    fn recv_from(&mut self, now_ms: u64) -> Option<(SocketAddr, Vec<u8>)>;
}

/// A non-blocking UDP socket.
pub struct UdpLink {
    socket: std::net::UdpSocket,
    buf: Vec<u8>,
}

impl UdpLink {
    pub fn bind(addr: SocketAddr) -> std::io::Result<Self> {
        let socket = std::net::UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(Self { socket, buf: vec![0; 2048] })
    }
}

impl Link for UdpLink {
    fn local_addr(&self) -> SocketAddr {
        self.socket.local_addr().expect("bound socket has an address")
    }
    fn send_to(&mut self, _now_ms: u64, to: SocketAddr, bytes: &[u8]) {
        // UDP is lossy anyway; a failed send is treated like a dropped packet.
        let _ = self.socket.send_to(bytes, to);
    }
    fn recv_from(&mut self, _now_ms: u64) -> Option<(SocketAddr, Vec<u8>)> {
        loop {
            match self.socket.recv_from(&mut self.buf) {
                Ok((n, from)) => return Some((from, self.buf[..n].to_vec())),
                // Windows reports ICMP "port unreachable" as a recv error; skip it.
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
                Err(_) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sim::{SimConfig, SimNetwork};
    use super::*;

    fn pair(cfg: SimConfig) -> (SimNetwork, sim::SimLink, sim::SimLink) {
        let net = SimNetwork::new(cfg);
        let a = net.link(1);
        let b = net.link(2);
        (net, a, b)
    }

    fn step(link: &mut sim::SimLink, conn: &mut Connection, now: u64) {
        for p in conn.flush(now) {
            link.send_to(now, conn.peer(), &p);
        }
        while let Some((_, bytes)) = link.recv_from(now) {
            conn.receive(&bytes, now);
        }
    }

    #[test]
    fn reliable_messages_arrive_once_and_in_order_over_lossy_network() {
        let (_net, mut la, mut lb) = pair(SimConfig { loss: 0.3, latency_ms: 40, jitter_ms: 30, seed: 7 });
        let mut a = Connection::new(lb.local_addr(), 0);
        let mut b = Connection::new(la.local_addr(), 0);
        for i in 0u32..200 {
            a.send_reliable(i.to_le_bytes().to_vec()).unwrap();
        }
        let mut got = Vec::new();
        for t in (0..20_000).step_by(16) {
            step(&mut la, &mut a, t);
            step(&mut lb, &mut b, t);
            got.extend(b.drain_received());
        }
        let got: Vec<u32> = got.iter().map(|d| u32::from_le_bytes(d[..4].try_into().unwrap())).collect();
        assert_eq!(got, (0..200).collect::<Vec<_>>());
        assert_eq!(a.unacked(), 0);
        assert!(a.stats().resends > 0, "loss should have forced resends");
    }

    #[test]
    fn unreliable_messages_are_not_resent() {
        let (_net, mut la, mut lb) = pair(SimConfig { loss: 1.0, ..SimConfig::default() });
        let mut a = Connection::new(lb.local_addr(), 0);
        let mut b = Connection::new(la.local_addr(), 0);
        a.send_unreliable(vec![1, 2, 3]).unwrap();
        step(&mut la, &mut a, 0);
        let sent_after_first = a.stats().packets_sent;
        step(&mut la, &mut a, 10);
        step(&mut lb, &mut b, 10);
        assert_eq!(a.stats().packets_sent, sent_after_first);
        assert!(b.drain_received().is_empty());
    }

    #[test]
    fn oversized_message_is_refused() {
        let mut c = Connection::new("127.0.0.1:1".parse().unwrap(), 0);
        assert!(matches!(c.send_reliable(vec![0; MAX_MESSAGE_BYTES + 1]), Err(SendError::TooLarge { .. })));
    }

    #[test]
    fn packets_stay_under_mtu() {
        let mut c = Connection::new("127.0.0.1:1".parse().unwrap(), 0);
        for _ in 0..20 {
            c.send_reliable(vec![7; 500]).unwrap();
            c.send_unreliable(vec![9; 300]).unwrap();
        }
        let packets = c.flush(0);
        assert!(packets.len() > 1);
        assert!(packets.iter().all(|p| p.len() <= MAX_PACKET_BYTES), "{:?}", packets.iter().map(|p| p.len()).collect::<Vec<_>>());
    }

    #[test]
    fn junk_is_ignored_and_silence_times_out() {
        let mut c = Connection::new("127.0.0.1:1".parse().unwrap(), 0);
        assert!(!c.receive(b"hello", 10));
        assert!(!c.timed_out(TIMEOUT_MS));
        assert!(c.timed_out(TIMEOUT_MS + 1));
    }

    #[test]
    fn keepalive_when_idle() {
        let mut c = Connection::new("127.0.0.1:1".parse().unwrap(), 0);
        assert_eq!(c.flush(1_000).len(), 1);
        assert!(c.flush(1_010).is_empty());
        assert_eq!(c.flush(1_000 + KEEPALIVE_MS).len(), 1);
    }

    #[test]
    fn real_udp_loopback() {
        let mut la = UdpLink::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut lb = UdpLink::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut a = Connection::new(lb.local_addr(), 0);
        let mut b = Connection::new(la.local_addr(), 0);
        a.send_reliable(b"axe".to_vec()).unwrap();
        for p in a.flush(0) {
            la.send_to(0, a.peer(), &p);
        }
        let mut got = Vec::new();
        for _ in 0..200 {
            while let Some((from, bytes)) = lb.recv_from(0) {
                assert_eq!(from, b.peer());
                b.receive(&bytes, 0);
            }
            got.extend(b.drain_received());
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got, vec![b"axe".to_vec()]);
    }
}
