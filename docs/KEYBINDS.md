# Keybind design

## Real DDA reference (2026-08-28, via the community wiki mirror)

Movement: cardinals `hjkl`/numpad/arrows, diagonals `yubn`. Core actions:

| Action | Key | Action | Key |
|---|---|---|---|
| Inventory | `i` | Apply/use item | `a` / `A` (wielded) |
| Wield item | `w` | Eat/drink/consume | `E` |
| Wear item | `W` | Read | `R` |
| Take off worn item | `T` | Throw | `t` |
| Drop | `d` | Fire weapon | `f` |
| Drop to adjacent tile | `D` | Reload | `r` |
| Open door/window | `o` | Close door/window | `c` |
| Examine | `e` | Pick up | `,` or `g` |
| Look around (free cursor) | `x` or `;` | Sleep | `$` |
| Wait | `.` | Running/crouch toggle | `"` |

Sources: [CDDA wiki mirror - Controls](https://srgnis.github.io/cdda-wiki/cdda_wiki/Controls), [DefKey CDDA shortcuts](https://defkey.com/cdda-cataclysm-dark-days-ahead-shortcuts)

**Correction (2026-08-28):** the wiki-mirror fetch above originally said
`|` for wait. The user corrected this from direct DDA experience - it's
`.`, a very standard "wait a turn" key across roguelikes generally. Fixed
in code and in the table above; trust direct player experience over a
secondhand source summary when they conflict.

Note several are same-letter shift pairs for related actions: wield/wear
(`w`/`W`), drop/drop-to-tile (`d`/`D`), apply/apply-wielded (`a`/`A`).

## The collision problem

DDA's real letters assume hjkl/yubn own movement. Our WASD preset uses
`w a s d q e z c` for movement instead, which collides directly with DDA's
`w` (wield), `a` (apply), `d` (drop), `e` (examine), `c` (close) - five of
its most-used action keys.

**Resolution:**
- **Vi-keys preset: copy DDA's real bindings verbatim** (table above). Zero
  collisions, since hjkl/yubn never overlaps DDA's action letters - and it
  means muscle memory actually transfers between this game and real DDA,
  which was the whole point.
- **WASD preset: keep whatever doesn't collide, re-letter what does.**
  `i, t, r, f, o, g, x` (and `m` for the map screen) aren't used by either
  preset's movement, so they stay identical across both presets - no need
  to relearn inventory, throw, reload, fire, open, pick-up, or look-around
  just because you're on WASD.
- For the five that collide (wield, wear, apply, drop, examine, close),
  the wasd preset's own unused letters are exactly vi's movement letters:
  `h j k l y u b n` (plus `v`/`p`, unused by DDA entirely) - 10 free
  letters for 5 actions, so pairs are affordable. Pick synonyms that land
  on a free letter, keeping DDA's same-letter-shift-pair convention:
  - **wield -> "hold": `h`** — implemented. Vi preset keeps DDA's real `w`.
    Wear (as a separate action from wield) not built yet.
  - apply -> **use**: `u` — reserved, not implemented (no use-effects
    system yet, nothing for "apply" to actually do).
  - **close -> "bar" (as in bar the door): `b`** — implemented. Vi preset
    keeps DDA's real `c`.
  - **drop -> "put down": `p`** — implemented. Vi preset keeps DDA's real `d`.
  - **examine -> "look": `l`** — implemented. Vi preset keeps DDA's real `e`.
    A quick cursor-free "what's on my own tile" query, distinct from the
    free-roam `x` look-around.
- **`.` (wait)** is shared by both presets too, like `o`/`g` - real DDA key,
  not used by either movement scheme. Just passes the turn.
- **Shift+`E` (eat)** is also shared - movement only binds lowercase `e` in
  wasd, so uppercase `E` is free on both presets without needing a synonym.
  Implemented; only lists food items to eat.
- This is a *default*, not a constraint - full custom rebinding already
  exists (`scourge-render-term`'s Settings screen), so a player can always
  override any of this.

## Noted for later, not built

- **Minecraft-style preset** (`e` inventory, `q` drop, etc.) as a third
  preset option. Raises its own problem: Minecraft's control scheme has no
  8-directional movement to begin with (WASD + mouse-look, not a
  grid-roguelike), so "diagonal movement" has no natural Minecraft
  equivalent - unresolved, deliberately deferred.
- **Onboarding despite a dense keybind surface**: DDA's real learning curve
  is famously steep precisely because of how many single-letter commands
  exist. Copying its keys doesn't have to copy that problem - a searchable
  in-game command reference / contextual hints could make discovery much
  faster than DDA's own docs. Design later, once there are enough real
  commands for a reference to be worth building.
