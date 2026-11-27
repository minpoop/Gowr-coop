# Roadmap

## Phase 0: groundwork ✅
Netcode, session logic, tests, `coop-sim`, DLL skeleton, offsets format.

## Phase 1: see each other
* Find the local Kratos: position, rotation, health (RE steps 1–2).
* Fill `offsets.toml`, run with `role = "off"`, check `gowr-coop.log` reports them `ok`.
* Draw the other player as a marker with an ImGui overlay (hudhook).
* **Done when:** two PCs connect and each sees a dot where the other Kratos is.

## Phase 2: a second Kratos
* Find how the game spawns characters, or take over the companion (RE step 6).
* Drive the puppet's position, rotation and animation from network state.
* **Done when:** the guest's Kratos runs and swings around the host's world.

## Phase 3: shared enemies
* Enemy list and stable ids (RE steps 4–5).
* Turn enemy AI off on the guest; drive enemies from snapshots (RE step 8).
* Enemies target the nearest player on the host.
* **Done when:** both players see the same draugr in the same place.

## Phase 4: shared combat
* Hook the game's damage function (RE step 7): guest hits → host, damage to the guest → health.
* Apply guest hits through the game's own function so stagger, death and loot work.
* Downed/revive between players instead of game over.

## Phase 5: shared progress
* Area travel follows the host (realm travel, doors, loading).
* Cutscenes play or skip together.
* Guest XP/hacksilver mirrored from the host's rewards, on the guest's own save.

## Phase 6: polish
* Overlay UI: host/join, player list, ping, chat.
* Relay or Steam networking so no port forwarding is needed.
* Up to 4 players.
* Smooth guest movement on the host with interpolation, as the guest already has.
