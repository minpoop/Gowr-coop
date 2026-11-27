//! `coop-sim` — try the co-op netcode without the game.
//!
//!   coop-sim local [--loss 0.1] [--latency 80] [--seconds 20]
//!       Host and guest in one process over a simulated bad network; prints how far apart
//!       the two copies of the world drift.
//!
//!   coop-sim host [--port 27015] [--name Kratos]
//!   coop-sim join <host-ip:port> [--name Atreus]
//!       Two PCs over real UDP with a stand-in game: checks your port forwarding and ping
//!       before the real mod is ready.

use coop_net::sim::{SimConfig, SimNetwork};
use coop_net::{Link, UdpLink};
use coop_protocol::{Vec3, HOST_PLAYER};
use coop_session::sim::SimGame;
use coop_session::{GuestSession, GuestStatus, HostConfig, HostSession, SessionEvent};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const FRAME_MS: u64 = 16;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("local") => local(&args[1..]),
        Some("host") => host(&args[1..]),
        Some("join") => join(&args[1..]),
        _ => Err(usage()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(2);
    }
}

fn usage() -> String {
    "usage:\n  coop-sim local [--loss 0.1] [--latency 80] [--seconds 20]\n  coop-sim host [--port 27015] [--name Kratos]\n  coop-sim join <host-ip:port> [--name Atreus]".into()
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn parse<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T, String> {
    match flag(args, name) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|_| format!("bad value for {name}: {v}")),
    }
}

/// Kratos walks in a slow circle so there is something to sync.
fn circle(t: f32, speed: f32) -> Vec3 {
    Vec3::new((t * 0.3).cos() * speed, 0.0, (t * 0.3).sin() * speed)
}

fn print_events(who: &str, events: Vec<SessionEvent>) {
    for e in events {
        println!("[{who}] {e:?}");
    }
}

fn local(args: &[String]) -> Result<(), String> {
    let loss: f32 = parse(args, "--loss", 0.05)?;
    let latency: u64 = parse(args, "--latency", 60)?;
    let seconds: u64 = parse(args, "--seconds", 20)?;
    let net = SimNetwork::new(SimConfig { loss, latency_ms: latency / 2, jitter_ms: latency / 4, seed: 1 });
    let host_link = net.link(1);
    let addr = host_link.local_addr();
    let mut host = HostSession::new(host_link, HostConfig::default());
    let mut guest = GuestSession::connect(net.link(2), addr, "Atreus", 0);
    let (mut hg, mut gg) = (SimGame::new(true), SimGame::new(false));
    println!("simulating {seconds}s, {latency} ms ping, {:.0}% loss", loss * 100.0);
    let mut worst: f32 = 0.0;
    let mut now = 0;
    while now < seconds * 1000 {
        now += FRAME_MS;
        let t = now as f32 / 1000.0;
        hg.step(FRAME_MS as f32 / 1000.0, circle(t, 5.0));
        gg.step(FRAME_MS as f32 / 1000.0, circle(t + 1.0, 5.0));
        print_events("host", host.update(&mut hg, now));
        print_events("guest", guest.update(&mut gg, now));
        if now % 1000 == 0 {
            if let Some(seen) = gg.remotes.get(&HOST_PLAYER) {
                let drift = seen.pos.distance(hg.player.pos);
                worst = worst.max(drift);
                println!(
                    "t={:>3}s  host-on-guest drift {:.2} m  enemies on guest {}/{}",
                    now / 1000,
                    drift,
                    gg.enemies.len(),
                    hg.enemies.len()
                );
            }
        }
    }
    println!("worst drift {worst:.2} m (Kratos runs ~5 m/s, so 0.5 m is about 100 ms behind)");
    Ok(())
}

fn host(args: &[String]) -> Result<(), String> {
    let port: u16 = parse(args, "--port", 27015)?;
    let name = flag(args, "--name").unwrap_or("Kratos").to_string();
    let link = UdpLink::bind(SocketAddr::from(([0, 0, 0, 0], port))).map_err(|e| format!("cannot open port {port}: {e}"))?;
    println!("hosting on UDP port {port}. Your friend runs: coop-sim join <your-public-ip>:{port}");
    let mut session = HostSession::new(link, HostConfig { name, ..HostConfig::default() });
    let mut game = SimGame::new(true);
    run_loop(|now, t| {
        game.step(FRAME_MS as f32 / 1000.0, circle(t, 4.0));
        print_events("host", session.update(&mut game, now));
        if now % 2000 < FRAME_MS {
            for (id, name, rtt) in session.players() {
                let pos = game.remotes.get(&id).map(|s| s.pos);
                println!("  player {id} {name}: rtt {rtt:.0} ms, at {pos:?}");
            }
        }
        true
    });
    Ok(())
}

fn join(args: &[String]) -> Result<(), String> {
    let host: SocketAddr = args
        .first()
        .ok_or_else(usage)?
        .parse()
        .map_err(|_| "host must look like 203.0.113.5:27015".to_string())?;
    let name = flag(args, "--name").unwrap_or("Atreus").to_string();
    let link = UdpLink::bind(SocketAddr::from(([0, 0, 0, 0], 0))).map_err(|e| e.to_string())?;
    let mut session = GuestSession::connect(link, host, &name, 0);
    let mut game = SimGame::new(false);
    println!("joining {host} as {name}...");
    run_loop(|now, t| {
        game.step(FRAME_MS as f32 / 1000.0, circle(t + 2.0, 4.0));
        print_events("guest", session.update(&mut game, now));
        match session.status() {
            GuestStatus::Rejected(_) | GuestStatus::Disconnected(_) => return false,
            GuestStatus::Connecting => {}
            GuestStatus::Connected { .. } => {
                if now % 2000 < FRAME_MS {
                    println!(
                        "  ping {:?} ms, host at {:?}, {} enemies",
                        session.ping_ms().map(|p| p.round()),
                        game.remotes.get(&HOST_PLAYER).map(|s| s.pos),
                        game.enemies.len()
                    );
                }
            }
        }
        true
    });
    Ok(())
}

fn run_loop(mut frame: impl FnMut(u64, f32) -> bool) {
    let start = Instant::now();
    loop {
        let now = start.elapsed().as_millis() as u64;
        if !frame(now, now as f32 / 1000.0) {
            break;
        }
        std::thread::sleep(Duration::from_millis(FRAME_MS));
    }
}
