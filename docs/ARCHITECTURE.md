# Architecture

```
 Host PC                                           Guest PC
┌──────────────────────────┐                     ┌──────────────────────────┐
│ God of War Ragnarök      │                     │ God of War Ragnarök      │
│  └ gowr-coop.asi         │                     │  └ gowr-coop.asi         │
│     GowrBridge (memory)  │                     │     GowrBridge (memory)  │
│        ▲  Game trait     │                     │        ▲  Game trait     │
│     HostSession          │  UDP 30 Hz + events │     GuestSession         │
│        └ coop-net ───────┼─────────────────────┼──────── coop-net        │
└──────────────────────────┘                     └──────────────────────────┘
```

## Layers
1. **`coop-protocol`**: plain data. `ClientMsg` (guest → host), `ServerMsg` (host → guest),
   `GameEvent`, `PlayerState`, `EnemyState`. Encoded with bincode, size-limited on decode.
2. **`coop-net`**: one `Connection` per peer. No I/O inside: feed packets in, send what
   `flush()` returns. Packets stay under 1200 bytes. Reliable messages are resent after
   ~1.5× RTT until acked and delivered in order. Silence for 8 s means the peer is gone.
   `sim::SimNetwork` adds latency, jitter and loss for tests.
3. **`coop-session`**: the rules of co-op, written against the `Game` trait, not the real game.
   * `HostSession`: accepts `Hello`, rejects wrong versions and full sessions, sends snapshots
     (enemies split 12 per packet), checks guest hits, broadcasts events, detects area and
     cutscene changes, and teleports guests who stray past 80 m.
   * `GuestSession`: connects, sends its Kratos every tick, sends hits and damage reliably,
     draws the host's world ~2 ticks in the past through `Interpolator`, and ignores enemies
     the host has declared dead.
   * `sim::SimGame`: a stand-in world used by tests and `coop-sim`.
4. **`coop-dll`**: `GowrBridge` implements `Game` over Ragnarök's memory using `offsets.toml`.
   Every missing offset turns its feature off instead of crashing, and is listed in the log.

## Why the host is in charge
Two copies of a single-player game will drift apart: enemy AI is not deterministic and has a
different player to chase on each PC. Letting one game decide and the other follow is
the only reliable approach. The cost is that the guest sees enemies about half a ping late, so
the guest's hits are checked generously (60 m range, damage clamp).

## Known hard problems
* **Enemy identity.** Both PCs must agree which enemy is which. A pointer address is no good.
  The id must come from level data (spawn/encounter id) that is the same on both PCs.
  Reverse-engineering step 5.
* **A second Kratos.** The game expects one player. Options in order of effort: reuse the
  companion slot (Atreus/Freya), clone an NPC with Kratos's model, or call the game's spawn
  function for a second player character. Step 6.
* **Guest enemies thinking for themselves.** On the guest, enemy AI must be paused and enemies
  driven from snapshots, or they will fight the network. Step 8.
* **Story scripting.** Cutscenes, doors, puzzles and boss phases are scripted per player. First
  version: the guest is a spectator-fighter, and the host does all interactions.
* **Thread safety.** The session runs on its own thread. Reads are fine; writes (moving puppets
  and enemies) should move into a hook on the game's update function to avoid tearing.
