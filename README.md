# GoWR Co-op

Online co-op for **God of War Ragnarök** (PC). Two players, two copies of the game, one world:
the host's game runs the enemies, story and progress, and the guest's Kratos fights beside them.

> **Status: early groundwork.** The netcode is built and tested; hooking it into the game is
> not done yet. See [What works today](#what-works-today).

## How it works
* **The host is in charge.** Enemies, AI, damage, deaths, area changes and cutscenes are
  decided in the host's game.
* **Each player sends their own Kratos** (position, facing, animation, health) 30 times a second.
  The other player's game shows them as a *puppet*.
* **Hits are requests.** When the guest hits an enemy, the host checks it (range, sane damage)
  and applies it, then tells everyone.
* **The guest is kept close.** If the guest wanders too far they are pulled back to the host,
  and area changes and cutscenes follow the host.
* **Networking** is a small UDP layer: unreliable messages for constant updates, reliable
  in-order messages for events. It handles packet loss, reordering and timeouts.

More detail: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## What works today
| Part | State |
|---|---|
| UDP transport (reliable + unreliable, acks, resends, timeouts) | ✅ done, tested |
| Host/guest session: join, reject, leave, timeouts, chat | ✅ done, tested |
| Snapshot sync with interpolation (smooth over 120 ms ping / 10% loss) | ✅ done, tested |
| Host-checked hits, kills, area changes, cutscenes, leash teleport | ✅ done, tested with a stand-in game |
| `coop-sim`: test the netcode between two PCs without the game | ✅ done |
| DLL: config, signature scanning, crash-safe memory access, offsets file | ✅ done, tested on fake memory |
| **Finding Ragnarök's player/enemy/area data in memory** | ⏳ you, with [the checklist](docs/REVERSE_ENGINEERING.md) |
| Spawning a second Kratos as a puppet | ⏳ not started |
| Hit hook, guest enemy AI off, travel between areas | ⏳ not started |
| In-game overlay (join/host UI) | ⏳ not started |

Roadmap: [docs/ROADMAP.md](docs/ROADMAP.md).

## Try the netcode now
```
cargo run -p coop-sim -- local --latency 120 --loss 0.1     # simulated bad connection
cargo run -p coop-sim -- host --port 27015                  # PC 1
cargo run -p coop-sim -- join <PC1-public-ip>:27015         # PC 2 (a friend)
```
If `join` connects, your port forwarding works and the real mod will too.

## Install (once offsets exist)
1. Install [Ultimate ASI Loader](https://github.com/ThirteenAG/Ultimate-ASI-Loader) in the game
   folder (next to `GoWR.exe`) as `dinput8.dll` or `winmm.dll`.
2. From the CI build (`gowr-coop-windows` artifact) copy `gowr-coop.asi`, `gowr-coop.toml` and
   `offsets.toml` into the game folder.
3. Edit `gowr-coop.toml`: `role = "host"` or `role = "join"` with `host_address`.
4. Start the game. `gowr-coop.log` shows what was found and who connected.

Play **offline, on a backup save**. The mod never sends game files anywhere.

## Layout
| Path | What |
|---|---|
| `crates/coop-protocol` | Messages between host and guest |
| `crates/coop-net` | UDP transport + a simulated lossy network |
| `crates/coop-session` | Host and guest logic, the `Game` trait, interpolation, stand-in game |
| `crates/coop-sim` | Command-line tool to try the netcode |
| `crates/coop-dll` | `gowr-coop.asi`: the part that lives in the game |
| `config/` | Example settings and the offsets template |
| `docs/` | Architecture, roadmap, reverse-engineering checklist |

## Build
```
cargo test --workspace
cargo build --release -p coop-dll     # on Windows, or let GitHub Actions do it
```

## Legal
Unofficial fan project, not affiliated with Santa Monica Studio or Sony. Contains no game files
or code. You need your own copy of God of War Ragnarök. MIT licence for this project's code.
