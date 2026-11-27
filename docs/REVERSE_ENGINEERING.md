# Reverse-engineering checklist

This happens on **your PC, with your copy of the game**. Each step ends with entries in
`offsets.toml`. Run the game with `role = "off"` and read `gowr-coop.log` to check them.

**Tools:** [Cheat Engine](https://www.cheatengine.org/) (finding values, pointer scans),
[x64dbg](https://x64dbg.com/) (breakpoints), [Ghidra](https://ghidra-sco.re/) or IDA Free
(reading code), [ReClass.NET](https://github.com/ReClassNET/ReClass.NET) (mapping structures).

**Safety:** play offline, back up your saves, and don't share game files.

---

### 1. Get set up
- [ ] Note the game version (Steam → Properties → Updates) and put it in `game_version`.
- [ ] Attach Cheat Engine to `GoWR.exe`. If the game closes, try again from the main menu.

### 2. The local Kratos: `[player]`, `[player_fields]`
- [ ] **Health:** search *Unknown initial value, Float*, take damage → *Decreased*, heal →
      *Increased*. Repeat until a few addresses are left. Freeze one: if health stops changing,
      that's it.
- [ ] **Position:** stand still → *Unchanged*; walk forward → *Changed*. Jumping changes the
      height (Y or Z). Position is three floats in a row. Find which one is height.
- [ ] Right-click the health address → *Find out what accesses this address*. The register
      plus offset (e.g. `[rbx+1A0]`) gives the **object** and the **field offset**
      (`health = 0x1A0`). Position should be in the same object (`pos = 0x…`).
- [ ] **Pointer scan** for the object address. Restart the game twice and keep only the pointers
      that still work. That gives `chain`.
- [ ] **Signature:** in x64dbg, find the instruction that loads the static pointer (usually
      `mov rax, [rip+…]`). Copy its bytes with the 4 address bytes as `??`, and check it is
      unique (Cheat Engine *Memory View → Search → AOB*). That gives
      `locate = { pattern = "...", disp_at = 3, insn_len = 7 }`.
- [ ] Rotation: four floats near position that change when you turn on the spot. The squares
      add up to 1.
- [ ] Rage, max health, animation id (a u32 that changes per attack/state), animation time.

### 3. Area and cutscene: `[area]`, `[cutscene]`
- [ ] **Area:** an integer that changes when you change realm or region and stays put
      otherwise. Search *Changed/Unchanged* across a few trips.
- [ ] **Cutscene:** a value that is non-zero only during cutscenes. Search across a few.

### 4. Enemies: `[enemies]`, `[enemy_fields]`
- [ ] Find one enemy's health like step 2 (hit it → decreased).
- [ ] *Find out what accesses* → the enemy object and its field offsets. Same layout as the
      player? Character classes are often shared, so player offsets may carry over.
- [ ] Find what holds that object: a list or array of entity pointers with a count. Look for a
      pointer to the enemy object, then scan up from it for the start of an array.
- [ ] Fill `array_offset` and `count_offset`. Check the log shows the right number of enemies
      in a camp.

### 5. An enemy id that's the same on both PCs: `enemy_fields.id`
- [ ] Look in the enemy object for an integer that **stays the same after reloading the area**
      and is **the same on a second PC** (or after a full game restart). Candidates: a
      spawn/encounter id, a hash of the spawner's name, a level-object id.
- [ ] Without this, the guest's enemies can't be matched to the host's. The log will say
      `host enemies have no match here`.

### 6. A second Kratos (puppet)
Pick the first option that works:
- [ ] **Companion takeover:** find the companion's (Atreus/Freya) object like step 2. Writing its
      position/rotation every frame and swapping its model to Kratos would give a puppet
      quickly.
- [ ] **Spawn function:** break on the code that runs when enemies spawn at a camp, and find the
      function that takes a character type and position. Call it with Kratos's type.
- [ ] Write down the function's address/signature and arguments in `docs/notes/` for the code.

### 7. The damage function (hit hook)
- [ ] From the enemy-health *what writes to this address* result, walk back up the call stack
      to the function that takes (attacker, target, damage/attack data).
- [ ] Find the signature and arguments. The mod will hook it: on the guest, report hits instead
      of applying them; on the host, call it for guest hits so stagger and death play
      properly.

### 8. Pausing enemy AI on the guest
- [ ] Find the per-enemy AI update (it runs every frame for each enemy and writes its
      position). Hooking it to skip on the guest leaves enemies fully driven by the host.

---

When an entry is done, send the values (and the game version) and the code side can be
written against them.
