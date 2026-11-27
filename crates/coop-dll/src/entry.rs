//! Windows entry point: start a background thread when the game loads the DLL.

use crate::bridge::{GowrBridge, TrackOnly};
use crate::config::{CoopConfig, Role};
use crate::log;
use crate::memory::{Memory, ProcessMemory};
use crate::offsets::Offsets;
use coop_net::UdpLink;
use coop_session::{GuestSession, HostConfig, HostSession, SessionEvent};
use std::ffi::c_void;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{BOOL, HMODULE, TRUE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

const FRAME: Duration = Duration::from_millis(16);

#[no_mangle]
#[allow(non_snake_case)]
extern "system" fn DllMain(module: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        let module = module as usize;
        // Do nothing heavy inside DllMain (loader lock); the thread starts after it returns.
        std::thread::spawn(move || {
            if let Err(e) = std::panic::catch_unwind(|| run(module)) {
                log!("gowr-coop crashed: {e:?}");
            }
        });
    }
    TRUE
}

fn dll_dir(module: usize) -> PathBuf {
    let mut buf = vec![0u16; 1024];
    // SAFETY: buf is valid for buf.len() u16s.
    let n = unsafe { GetModuleFileNameW(module as HMODULE, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    path.parent().map(PathBuf::from).unwrap_or_default()
}

/// Base address and size of the game's .exe, read from its PE headers.
fn game_image(mem: &ProcessMemory) -> Option<(usize, Vec<u8>)> {
    // SAFETY: a null name returns the handle of the process's own executable.
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let pe = base + mem.read_u32(base + 0x3C)? as usize;
    let size = mem.read_u32(pe + 0x18 + 0x38)? as usize; // OptionalHeader.SizeOfImage
    let mut image = vec![0u8; size];
    // Copy page by page; unreadable pages stay zero.
    for (i, page) in image.chunks_mut(0x1000).enumerate() {
        if !mem.read_into(base + i * 0x1000, page) {
            page.fill(0);
        }
    }
    Some((base, image))
}

fn run(module: usize) {
    let dir = dll_dir(module);
    log::init(&dir.join("gowr-coop.log"));
    log!("gowr-coop {} loaded from {}", env!("CARGO_PKG_VERSION"), dir.display());

    let cfg = match std::fs::read_to_string(dir.join("gowr-coop.toml")).map_err(|e| e.to_string()).and_then(|t| CoopConfig::parse(&t)) {
        Ok(c) => c,
        Err(e) => return log!("gowr-coop.toml: {e} — copy config/gowr-coop.example.toml next to the DLL"),
    };
    let offsets = match std::fs::read_to_string(dir.join("offsets.toml")).map_err(|e| e.to_string()).and_then(|t| Offsets::parse(&t)) {
        Ok(o) => o,
        Err(e) => return log!("offsets.toml: {e}"),
    };

    // Give the game time to unpack itself and reach the menu before scanning.
    std::thread::sleep(Duration::from_secs(10));
    let mem = ProcessMemory;
    let Some((base, image)) = game_image(&mem) else { return log!("could not read the game's executable") };
    log!("game image at {base:#x}, {} MB", image.len() / (1024 * 1024));
    let mut game = GowrBridge::new(ProcessMemory, &offsets, base, &image, TrackOnly::default());
    drop(image);
    for line in game.report() {
        log!("  {line}");
    }

    let start = Instant::now();
    let now = || start.elapsed().as_millis() as u64;
    let show = |events: Vec<SessionEvent>| {
        for e in events {
            log!("{e:?}");
        }
    };

    match cfg.role {
        Role::Off => log!("role = off: not networking"),
        Role::Host => {
            let link = match UdpLink::bind(SocketAddr::from(([0, 0, 0, 0], cfg.port))) {
                Ok(l) => l,
                Err(e) => return log!("cannot open UDP port {}: {e}", cfg.port),
            };
            log!("hosting on UDP port {}", cfg.port);
            let hc = HostConfig { name: cfg.name.clone(), max_players: cfg.max_players, ..HostConfig::default() };
            let mut session = HostSession::new(link, hc);
            loop {
                show(session.update(&mut game, now()));
                std::thread::sleep(FRAME);
            }
        }
        Role::Join => {
            let target = cfg.host_address.clone().unwrap_or_default();
            let Some(addr) = target.to_socket_addrs().ok().and_then(|mut a| a.next()) else {
                return log!("cannot resolve host_address {target:?}");
            };
            let link = match UdpLink::bind(SocketAddr::from(([0, 0, 0, 0], 0))) {
                Ok(l) => l,
                Err(e) => return log!("cannot open a UDP socket: {e}"),
            };
            log!("joining {addr} as {}", cfg.name);
            let mut session = GuestSession::connect(link, addr, &cfg.name, now());
            loop {
                show(session.update(&mut game, now()));
                if !matches!(session.status(), coop_session::GuestStatus::Connecting | coop_session::GuestStatus::Connected { .. }) {
                    return log!("session ended: {:?}", session.status());
                }
                std::thread::sleep(FRAME);
            }
        }
    }
}
