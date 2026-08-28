# Architecture notes

## Crate boundaries

Rust compiles a workspace into one binary, so splitting into crates does not
give true crash isolation (a panic anywhere can still take down the process).
What it does give:

- Clear ownership: each crate has one job and a narrow public API.
- Compile-time enforcement of dependency direction (e.g. `scourge-core` can
  never accidentally depend on rendering or networking).
- Easier forking: someone can swap `scourge-render-term` for a tile backend,
  or replace `scourge-net`, without touching game logic.

Real resilience against bad content comes from **mods being data, not code**
(JSON, loaded and validated at runtime by `scourge-data`) — a malformed mod
produces a load error, not a panic.

## Rendering

`scourge-render` defines backend-agnostic traits; `scourge-render-term` is
the first implementation, via `crossterm` (portable across Windows, macOS,
Linux terminals and tty, and Termux). A future tile/graphical backend would
be a new crate implementing the same traits — game logic doesn't change.

## Input

Same pattern as rendering: `scourge-input` defines actions, not key codes.
A controller backend can map buttons to the same actions a keyboard backend
maps keys to, so game logic never branches on input device.

## Multiplayer (planned direction, not yet implemented)

Deferred until singleplayer is solid. `scourge-net` is a stub until then.
Design arrived at after extensive brainstorming (2026-08-27); revisit before
implementing, but this is the current plan:

- **Client-server, not P2P.** One authoritative server (possibly embedded
  in one player's client, "listen server" style) holds world state; all
  players connect as clients. Works the same on LAN and internet; only
  discovery/NAT traversal differs. P2P/decentralized play is a possible
  *future* addition on top of this, not a v1 requirement.
- **One persistent shared world, never per-player forked saves.** This is
  what prevents timeline paradoxes (e.g. two players experiencing
  contradictory "current states" of the same location) — there is exactly
  one canonical world state at all times, the same way reconnecting to a
  live Minecraft/Valheim server drops you into the world as it currently
  is, not into a frozen personal copy.
- **The world only advances while at least one player is connected and
  acting** — consistent with turn-based play being driven entirely by
  decisions, never a real-time clock ticking in the void.
- **Offline players are treated like DDA's existing "asleep" state** —
  present in the world, not removed, not AI-piloted, with the same
  realistic (if reduced) vulnerability that state already implies. No new
  mechanic; reuse what DDA already has. A well-secured location is safe: a
  bad one carries the same realistic risk it always would.
- **When two+ players are online and colocated (reality bubbles overlap),
  their turns lock together and resolve jointly** so it feels like real
  simultaneous co-op (bounded by human decision time, not distance/absence).
  When not colocated or not simultaneous, each plays solo-paced against the
  one shared world — no one is ever blocked waiting on someone else.
- **A "what happened while you were away" recap/journal on login** softens
  the async case — the goal is to make catching up feel like rejoining a
  shared story, not discovering unexplained changes.
- Accepted tradeoff: solo progress during a partner's absence is real and
  visible (e.g. a location can get looted before the other player logs in).
  This is treated as realistic and expected, not a bug, and is the cost of
  never forcing anyone to wait.
- Should scale from 2 players up through a larger dedicated server (e.g.
  ~16): most players won't be colocated at once, so this is many small,
  cheap, mostly-independent groups rather than one global lockstep.

## Settings UX (planned, not built yet)

DDA-inspired tiering for options, once there are enough real settings to
justify it: **basic** (common toggles, in-game menu), **advanced** (the
full option set, still in-game), **expert** (hand-edit the config file
directly — no UI needed for these). Not building Basic/Advanced menu
screens yet: right now the only real setting is keybindings, and splitting
one Settings screen into two empty-feeling tabs would be scaffolding with
nothing to differentiate. Revisit once there are enough independent
settings (display, gameplay, etc.) for the tiers to mean something.

## Mods

Content packs live under `mods/<mod-name>/` as JSON, loaded by
`scourge-data`. This is also the intended path for a future NSFW/mature
content mod — the engine and mod-loading support it, but explicit text and
descriptions are authored by hand, not generated.

**Scripted mods (planned, not built): WASM, not a bundled scripting
language.** Rather than embedding one interpreter (Lua, JS, Python) and
locking mod authors into it, host a WASM runtime (`wasmtime` is the
obvious pure-Rust choice) and define a stable host-function ABI. Any
language with a WASM target can then write mods: Rust, C, C++, Zig, and
AssemblyScript compile to WASM directly and natively; Lua/JS/Python are
still reachable by bundling a small interpreter *for that language*
compiled to WASM inside the mod itself (e.g. a Lua-in-WASM runtime, or
QuickJS-wasm) — genuinely "any language," but not all equally cheap: a
Rust mod's wasm binary is tiny, a Python-based one drags its whole
interpreter along and will be much larger and slower to start. WASM
sandboxing is also a real safety win for distributing untrusted community
mods — a mod can't touch the filesystem or network unless a host function
explicitly allows it.

The hard part is the ABI itself: what data crosses the boundary and how
(JSON/bincode blobs through linear memory vs. the still-maturing WASM
Component Model for typed interop). Deliberately not designed yet — there's
no real game data/systems for an API to expose until items, entities, and
actions actually exist. Revisit once there's something worth scripting.

Planned repo split (not set up yet): the main engine/game source stays on
GitHub, but the NSFW mod's content will likely live in a separate repo
hosted on GitGud.io or the user's own site, keeping explicit content off
GitHub entirely. Nothing to build for this yet - just a hosting decision
for whenever the NSFW mod actually exists.

## Future, explicitly out of scope for now

- Tile/graphical rendering backend
- Controller support
- Real-time (non-turn-based) mode as an opt-in mod
- Bare Linux tty sessions typically only have a small built-in console font
  (not full Unicode), so extended glyphs may not render there even though
  they work fine in a real terminal emulator. Possible future idea: a build
  flag that embeds a bitmap/PSF font with the needed glyphs directly in the
  binary, for the tty case specifically, instead of relying on whatever font
  the system currently has loaded. Not investigated yet - needs research
  into how (or whether) a userspace program can get the Linux console to use
  a custom font without root/`setfont`, and what framebuffer console
  limitations apply.
