# Scourge

A terminal survival roguelike, inspired by Cataclysm:DDA, Cataclysm:BN,
Cataclysm:TLG, and Cataclysm:LN — built independently in Rust, not forked
from or built on any of their code.

## Goals

- **Pure Rust**, runs in any terminal on any OS: Windows, macOS, Linux
  (regular terminals and bare tty), and Termux on Android. Controller
  support for touch/handheld play is a planned future addition.
- **Singleplayer and multiplayer from the same core**, with multiplayer
  designed (à la Cataclysm:LN) to let players in different timezones play
  async without one player's absence getting another killed. Sync model is
  still being designed — see `docs/ARCHITECTURE.md`.
- **Moddable like C:DDA/C:BN**: content (items, monsters, recipes, maps,
  dialogue) lives in JSON mods under `mods/`, not compiled code, so a broken
  mod fails to load cleanly instead of crashing the game.
- **Modular workspace**: each concern is its own crate (see below) so
  forks/tweaks can touch one piece without destabilizing the rest.
- Tile/graphical rendering and a real-time (vs. turn-based) mode are
  planned as far-future, opt-in additions — not in scope yet.
- Mature/NSFW content is planned as an optional mod. The engine and mod
  system support it; explicit content itself is authored separately by hand.

## Workspace layout

- `crates/scourge-core` — entities, turn engine, game state
- `crates/scourge-data` — mod/content loading (JSON)
- `crates/scourge-world` — map generation and world simulation
- `crates/scourge-net` — multiplayer networking and state sync
- `crates/scourge-input` — input abstraction (keyboard now, controller later)
- `crates/scourge-render` — backend-agnostic rendering traits
- `crates/scourge-render-term` — terminal rendering backend (crossterm)
- `crates/scourge` — main binary, wires everything together
- `mods/vanilla` — base content pack (empty for now)

## Status

Pre-alpha. The workspace compiles; no gameplay yet.
