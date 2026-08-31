//! Core game state: entities, turn engine, stats. No I/O, no rendering, no networking.

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Direction {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

/// The 8 cell offsets surrounding a point, used by pickup/open/close to
/// scan a player's immediate surroundings.
const ADJACENT_OFFSETS: [(i32, i32); 8] = [
    (0, -1), (0, 1), (1, 0), (-1, 0),
    (1, -1), (-1, -1), (1, 1), (-1, 1),
];

/// Buckets a displacement into one of the 8 compass directions - used by
/// the sidebar compass to say which way a visible creature is, not just
/// that one exists. `dy` follows this codebase's convention of increasing
/// southward (see `direction_delta`), so it's negated before the angle
/// calculation to get a normal compass bearing.
pub fn compass_direction(dx: i32, dy: i32) -> Direction {
    let angle = (dx as f64).atan2(-dy as f64).to_degrees();
    let angle = if angle < 0.0 { angle + 360.0 } else { angle };
    match ((angle + 22.5) / 45.0) as i32 % 8 {
        0 => Direction::North,
        1 => Direction::NorthEast,
        2 => Direction::East,
        3 => Direction::SouthEast,
        4 => Direction::South,
        5 => Direction::SouthWest,
        6 => Direction::West,
        _ => Direction::NorthWest,
    }
}

/// One turn is one second, matching real BN's clock - a fixed in-game
/// start of Day 1, 08:00:00 (also BN's default), advanced purely by the
/// turn counter rather than any separate clock state.
pub fn format_clock(turn: u64) -> String {
    const START_OF_DAY_SECONDS: u64 = 8 * 3600;
    let total = turn + START_OF_DAY_SECONDS;
    let day = total / 86400 + 1;
    let seconds_today = total % 86400;
    let (h, m, s) = (seconds_today / 3600, (seconds_today % 3600) / 60, seconds_today % 60);
    format!("Day {day}, {h:02}:{m:02}:{s:02}")
}

pub fn direction_delta(dir: Direction) -> (i32, i32) {
    match dir {
        Direction::North => (0, -1),
        Direction::South => (0, 1),
        Direction::East => (1, 0),
        Direction::West => (-1, 0),
        Direction::NorthEast => (1, -1),
        Direction::NorthWest => (-1, -1),
        Direction::SouthEast => (1, 1),
        Direction::SouthWest => (-1, 1),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Action {
    Move(Direction),
    /// Opens the first adjacent openable thing: a closed door, a closed
    /// window sash, or (if the sash is already open) its curtain.
    Open,
    /// The reverse of `Open`: closes an open door, closes an open window
    /// sash, or (if the sash is already closed) closes its curtain.
    Close,
    Quit,
    /// Picks up the topmost item on the player's current tile, if any.
    PickUp,
    /// UI-only: opens the wield-selection screen. Never actually reaches
    /// `World::step` in normal play (the render layer intercepts it), but
    /// still routed through the same preset-aware key resolution as every
    /// other action, since which key triggers it differs by preset ('w'
    /// for vi, 'h' for wasd - see KEYBINDS.md).
    OpenWieldMenu,
    /// Wields inventory item at this index, unwielding (back to inventory)
    /// whatever was wielded before.
    Wield(usize),
    /// Empties your hands, returning the wielded item to inventory.
    Unwield,
    /// Passes the turn without doing anything else - real DDA's `|`.
    Wait,
    /// UI-only, same pattern as `OpenWieldMenu`: opens the drop-selection
    /// screen. 'd' on vi (real DDA), 'p' ("put down") on wasd since 'd' is
    /// movement there.
    OpenDropMenu,
    /// Drops inventory item at this index onto the player's current tile.
    Drop(usize),
    /// UI-only: shows a description of the player's own current tile - a
    /// quick, cursor-free version of the free-roam 'x' look. 'e' on vi
    /// (real DDA), 'l' ("look") on wasd since 'e' is movement there.
    OpenExamine,
    /// UI-only, same pattern as `OpenWieldMenu`/`OpenDropMenu`: opens the
    /// eat-selection screen. Shift+E, real DDA - free on both presets since
    /// neither movement scheme uses the shifted letter, only lowercase.
    OpenEatMenu,
    /// Eats inventory item at this index if it's food; no-ops otherwise
    /// (the eat-select screen only lists food, so this shouldn't normally
    /// be reachable with a non-food index, but step() stays defensive).
    Eat(usize),
    /// Trades positions with the NPC at this index in `World.npcs` - real
    /// DDA lets you do this with a friendly ally so it doesn't permanently
    /// block a doorway or corridor. No-ops (with a message) if the NPC
    /// isn't actually adjacent by the time this runs.
    SwapPlaces(usize),
    /// A single canned greeting from the NPC at this index - a placeholder
    /// for real dialogue trees, which don't exist yet. No-ops (with a
    /// message) if the NPC isn't actually adjacent.
    Talk(usize),
    /// Melee-attacks the NPC at this index - reached by walking into a
    /// hostile one, real DDA's own bump-to-attack convention, not a
    /// separate menu. Deliberately just a flat guaranteed hit for now (a
    /// wielded weapon's own damage, or bare fists otherwise - see
    /// `World::player_attack_damage`): no accuracy/dodge system yet,
    /// matching "simple content first" - that's a real DDA-depth addition
    /// for later, once combat existing at all is confirmed to work.
    Attack(usize),
}

/// Backend-agnostic terrain color name. `scourge-core` has no rendering
/// dependency, so this is just a palette name - a renderer maps it to
/// whatever color representation it actually supports, at whatever
/// richness it can (truecolor, 256-color, or the basic ANSI 16 as a last
/// resort for things like a bare Linux tty console).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TileColor {
    Default,
    White,
    Gray,
    Brown,
    Cyan,
    Green,
    DarkGreen,
    Blue,
    Magenta,
    Yellow,
    Red,
}

/// A single terrain type. More kinds (rubble, water...) land here as they're needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tile {
    Floor,
    /// Outdoor ground, distinct from indoor `Floor` (same glyph, different
    /// color - real DDA often distinguishes similar terrain this way too).
    Dirt,
    /// The dominant outdoor ground cover; Dirt appears in patches on top of it.
    Grass,
    /// Walkable, doesn't block sight - just a taller/denser look than Grass.
    TallGrass,
    Wall,
    DoorClosed,
    DoorOpen,
    /// `open` (the sash/glass) governs walkability; `curtain_open` governs
    /// whether you can see through it. The two are intentionally
    /// independent - closing curtains doesn't stop you walking through an
    /// open window, and an open window doesn't imply the curtain's aside.
    Window { open: bool, curtain_open: bool },
    /// Blocks both movement and sight, like real dense underbrush. 'b',
    /// a shade of green - the berries themselves are the blue '*' item
    /// they drop, not the bush's own look.
    BlueberryBush,
    /// Non-tree woody vegetation - denser than TallGrass, same "blocks
    /// movement and sight" class as the bushes. DDA/BN's 's'.
    Shrub,
    /// A young tree - blocks movement and sight like a mature one, but
    /// reads visually distinct (lighter green, lowercase 't') so a forest
    /// isn't visually uniform. DDA/BN's 't'.
    YoungTree,
    /// A mature tree - the bigger obstacle. DDA/BN's 'T'.
    Tree,
}

impl Tile {
    pub fn is_walkable(self) -> bool {
        matches!(
            self,
            Tile::Floor
                | Tile::Dirt
                | Tile::Grass
                | Tile::TallGrass
                | Tile::DoorOpen
                | Tile::Window { open: true, .. }
        )
    }

    pub fn is_transparent(self) -> bool {
        !matches!(
            self,
            Tile::Wall
                | Tile::DoorClosed
                | Tile::Window { curtain_open: false, .. }
                | Tile::BlueberryBush
                | Tile::Shrub
                | Tile::YoungTree
                | Tile::Tree
        )
    }

    pub fn glyph(self) -> char {
        match self {
            Tile::Floor | Tile::Dirt | Tile::Grass => '.',
            Tile::TallGrass => ',',
            Tile::Wall => '#',
            Tile::DoorClosed => '+',
            Tile::DoorOpen => '\'',
            // Same glyph as an open door - real BN's convention too - but
            // never confused for one since doors and windows are different
            // colors (see `color()`).
            Tile::Window { open: true, .. } => '\'',
            Tile::Window { open: false, .. } => '"',
            Tile::BlueberryBush => 'b',
            Tile::Shrub => 's',
            Tile::YoungTree => 't',
            Tile::Tree => 'T',
        }
    }

    pub fn color(self) -> TileColor {
        match self {
            Tile::Floor => TileColor::Default,
            Tile::Dirt => TileColor::Brown,
            Tile::Grass => TileColor::Green,
            Tile::TallGrass => TileColor::DarkGreen,
            Tile::Wall => TileColor::Gray,
            Tile::DoorClosed | Tile::DoorOpen => TileColor::Brown,
            Tile::Window { .. } => TileColor::Cyan,
            Tile::BlueberryBush => TileColor::DarkGreen,
            Tile::Shrub => TileColor::DarkGreen,
            Tile::YoungTree => TileColor::Green,
            Tile::Tree => TileColor::DarkGreen,
        }
    }
}

/// A grid of tiles. Generation itself lives in `scourge-world`; this is just the data.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Map {
    pub width: i32,
    pub height: i32,
    tiles: Vec<Tile>,
}

impl Map {
    pub fn new(width: i32, height: i32, fill: Tile) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Map {
            width,
            height,
            tiles: vec![fill; (width * height) as usize],
        }
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            None
        } else {
            Some((y * self.width + x) as usize)
        }
    }

    pub fn tile(&self, x: i32, y: i32) -> Option<Tile> {
        self.index(x, y).map(|i| self.tiles[i])
    }

    pub fn set(&mut self, x: i32, y: i32, tile: Tile) {
        if let Some(i) = self.index(x, y) {
            self.tiles[i] = tile;
        }
    }

    pub fn transparent(&self, x: i32, y: i32) -> bool {
        self.tile(x, y).map(Tile::is_transparent).unwrap_or(true)
    }

    /// Shortest walkable path from `from` to `to`, 8-directional. Every
    /// step costs the same, so breadth-first search already finds the
    /// shortest path - no need for A*'s extra heuristic bookkeeping.
    /// Capped at `max_nodes` tiles explored so a distant or truly
    /// unreachable target can't make this expensive: enough headroom to
    /// route around a whole building, nowhere near enough to scan a
    /// meaningful fraction of a real map.
    pub fn find_path(&self, from: Pos, to: Pos, max_nodes: usize) -> Option<Vec<Pos>> {
        use std::collections::{HashMap, HashSet, VecDeque};

        if from == to {
            return Some(vec![from]);
        }
        let mut visited: HashSet<(i32, i32)> = HashSet::new();
        let mut came_from: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
        let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
        visited.insert((from.x, from.y));
        queue.push_back((from.x, from.y));
        let mut explored = 0;

        while let Some((cx, cy)) = queue.pop_front() {
            explored += 1;
            if explored > max_nodes {
                return None;
            }
            for (dx, dy) in ADJACENT_OFFSETS {
                let (nx, ny) = (cx + dx, cy + dy);
                if visited.contains(&(nx, ny)) || !self.tile(nx, ny).is_some_and(Tile::is_walkable) {
                    continue;
                }
                visited.insert((nx, ny));
                came_from.insert((nx, ny), (cx, cy));
                if (nx, ny) == (to.x, to.y) {
                    let mut path = vec![(nx, ny)];
                    let mut cur = (nx, ny);
                    while let Some(&prev) = came_from.get(&cur) {
                        path.push(prev);
                        cur = prev;
                    }
                    path.reverse();
                    return Some(path.into_iter().map(|(x, y)| Pos { x, y }).collect());
                }
                queue.push_back((nx, ny));
            }
        }
        None
    }

    /// The glyph to draw at (x, y). Walls pick a Unicode box-drawing
    /// character based on which neighbors are also walls, so a building's
    /// perimeter reads as a continuous outline instead of a field of '#'.
    pub fn glyph_at(&self, x: i32, y: i32) -> char {
        match self.tile(x, y) {
            Some(Tile::Wall) => self.wall_glyph(x, y),
            Some(other) => other.glyph(),
            None => ' ',
        }
    }

    pub fn color_at(&self, x: i32, y: i32) -> TileColor {
        self.tile(x, y).map(Tile::color).unwrap_or(TileColor::Default)
    }

    fn wall_glyph(&self, x: i32, y: i32) -> char {
        // Doors and windows count as "wall-like" for corner shape purposes
        // even though they render their own glyph - a corner sitting right
        // next to a window is still structurally a corner, not a dead end.
        let connects = |dx: i32, dy: i32| {
            matches!(
                self.tile(x + dx, y + dy),
                Some(Tile::Wall) | Some(Tile::DoorClosed) | Some(Tile::DoorOpen) | Some(Tile::Window { .. })
            )
        };
        let (n, s, e, w) = (connects(0, -1), connects(0, 1), connects(1, 0), connects(-1, 0));
        match (n, s, e, w) {
            (false, false, false, false) => '#',
            (false, false, false, true) => '─',
            (false, false, true, false) => '─',
            (false, false, true, true) => '─',
            (false, true, false, false) => '│',
            (false, true, false, true) => '┐',
            (false, true, true, false) => '┌',
            (false, true, true, true) => '┬',
            (true, false, false, false) => '│',
            (true, false, false, true) => '┘',
            (true, false, true, false) => '└',
            (true, false, true, true) => '┴',
            (true, true, false, false) => '│',
            (true, true, false, true) => '┤',
            (true, true, true, false) => '├',
            (true, true, true, true) => '┼',
        }
    }

    /// Tiles visible from `origin` within `radius`, accounting for walls
    /// blocking line of sight. Approximate: traces a line to every tile in
    /// range and stops it at the first opaque tile, rather than full
    /// recursive shadowcasting - cheap and good enough for now, swappable
    /// later without changing callers.
    pub fn visible_from(&self, origin: Pos, radius: i32) -> VisibleSet {
        let mut set = VisibleSet::new(origin, radius);
        set.mark(origin.x, origin.y);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
                for (x, y) in bresenham_line(origin.x, origin.y, origin.x + dx, origin.y + dy) {
                    set.mark(x, y);
                    if !self.transparent(x, y) {
                        break;
                    }
                }
            }
        }
        set
    }
}

/// The result of `Map::visible_from`: which tiles are currently visible
/// from a given point. A plain array indexed by offset from the origin,
/// not a hash set - this gets recomputed often, so avoiding hashing matters.
pub struct VisibleSet {
    origin: Pos,
    radius: i32,
    side: i32,
    cells: Vec<bool>,
}

impl VisibleSet {
    fn new(origin: Pos, radius: i32) -> Self {
        let side = 2 * radius + 1;
        VisibleSet {
            origin,
            radius,
            side,
            cells: vec![false; (side * side) as usize],
        }
    }

    fn local_index(&self, x: i32, y: i32) -> Option<usize> {
        let (lx, ly) = (x - self.origin.x + self.radius, y - self.origin.y + self.radius);
        if lx < 0 || ly < 0 || lx >= self.side || ly >= self.side {
            None
        } else {
            Some((ly * self.side + lx) as usize)
        }
    }

    fn mark(&mut self, x: i32, y: i32) {
        if let Some(i) = self.local_index(x, y) {
            self.cells[i] = true;
        }
    }

    pub fn is_visible(&self, x: i32, y: i32) -> bool {
        self.local_index(x, y).is_some_and(|i| self.cells[i])
    }
}

fn bresenham_line(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
    let mut points = Vec::new();
    let (mut x, mut y) = (x0, y0);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        points.push((x, y));
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
    points
}

/// What an item actually does, if anything. Deliberately minimal - real
/// items (weight/volume/containers/weapons, matching DDA's actual model)
/// are future work once there's a mod-data pipeline to define them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ItemKind {
    Generic,
    Food { hunger_restored: i32 },
}

/// A minimal carryable/ground item. `glyph`/`color` are what shows on the
/// map when it's lying on the ground - `?` in a generic marker color by
/// default, overridable per item (`with_glyph`) once a specific item earns
/// its own look, same "simple content first, detail per-item later"
/// pattern as `Npc::with_glyph`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Item {
    pub name: String,
    pub kind: ItemKind,
    pub glyph: char,
    pub color: TileColor,
    /// `None` means "not a weapon" - wielding it is no better than bare
    /// fists in combat. `Some(n)` replaces (not adds to) the unarmed
    /// baseline while wielded, matching real DDA: a weapon has its own
    /// damage, it doesn't stack on top of your fists.
    pub melee_damage: Option<i32>,
}

impl Item {
    pub fn new(name: impl Into<String>) -> Self {
        Item { name: name.into(), kind: ItemKind::Generic, glyph: '?', color: TileColor::Magenta, melee_damage: None }
    }

    pub fn food(name: impl Into<String>, hunger_restored: i32) -> Self {
        Item {
            name: name.into(),
            kind: ItemKind::Food { hunger_restored },
            glyph: '?',
            color: TileColor::Magenta,
            melee_damage: None,
        }
    }

    pub fn with_glyph(mut self, glyph: char, color: TileColor) -> Self {
        self.glyph = glyph;
        self.color = color;
        self
    }

    pub fn with_melee_damage(mut self, damage: i32) -> Self {
        self.melee_damage = Some(damage);
        self
    }

    pub fn is_food(&self) -> bool {
        matches!(self.kind, ItemKind::Food { .. })
    }
}

/// The four primary attributes, DDA-style. Deliberately just flat numbers
/// with no derived skills/combat math yet - only `strength` currently does
/// anything (feeds `Health::from_strength`). The others are groundwork for
/// when carry capacity, hit chance, etc. get built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stats {
    pub strength: i32,
    pub dexterity: i32,
    pub intelligence: i32,
    pub perception: i32,
}

impl Default for Stats {
    /// 8 in each stat - an unremarkable, average starting character, same
    /// baseline DDA uses before any traits/points are spent.
    fn default() -> Self {
        Stats { strength: 8, dexterity: 8, intelligence: 8, perception: 8 }
    }
}

/// The six main body parts, matching real DDA (which also splits out
/// hands/feet/eyes/mouth as separate parts - left out here since nothing
/// yet needs that finer granularity, e.g. no called-shots or encumbrance
/// system; easy to add later without disturbing this).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BodyPart {
    Head,
    Torso,
    LeftArm,
    RightArm,
    LeftLeg,
    RightLeg,
}

impl BodyPart {
    pub const ALL: [BodyPart; 6] = [
        BodyPart::Head,
        BodyPart::Torso,
        BodyPart::LeftArm,
        BodyPart::RightArm,
        BodyPart::LeftLeg,
        BodyPart::RightLeg,
    ];

    /// Real DDA: losing your head or torso kills you; losing a limb just
    /// disables it.
    pub fn is_vital(self) -> bool {
        matches!(self, BodyPart::Head | BodyPart::Torso)
    }

    pub fn label(self) -> &'static str {
        match self {
            BodyPart::Head => "Head",
            BodyPart::Torso => "Torso",
            BodyPart::LeftArm => "L Arm",
            BodyPart::RightArm => "R Arm",
            BodyPart::LeftLeg => "L Leg",
            BodyPart::RightLeg => "R Leg",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PartHealth {
    pub current: i32,
    pub max: i32,
}

impl PartHealth {
    fn new(max: i32) -> Self {
        PartHealth { current: max, max }
    }

    pub fn is_broken(self) -> bool {
        self.current <= 0
    }
}

/// Per-body-part health, matching real DDA/BN instead of a single HP pool -
/// losing an arm doesn't kill you, losing your torso does. No damage
/// sources or regeneration exist yet (nothing currently calls `damage` or
/// `heal`), so every part just sits at max until combat or anything else
/// is built to change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Health {
    pub head: PartHealth,
    pub torso: PartHealth,
    pub left_arm: PartHealth,
    pub right_arm: PartHealth,
    pub left_leg: PartHealth,
    pub right_leg: PartHealth,
}

impl Health {
    /// Real DDA's base_hp is a flat 60 per part at average (8) strength,
    /// scaling from there - the exact per-part formula in the real game is
    /// more involved than this; easy to retune once there's actual damage
    /// to balance against.
    pub fn from_strength(strength: i32) -> Self {
        let max = (60 + (strength - 8) * 3).max(20);
        let part = PartHealth::new(max);
        Health {
            head: part,
            torso: part,
            left_arm: part,
            right_arm: part,
            left_leg: part,
            right_leg: part,
        }
    }

    pub fn part(&self, bp: BodyPart) -> PartHealth {
        match bp {
            BodyPart::Head => self.head,
            BodyPart::Torso => self.torso,
            BodyPart::LeftArm => self.left_arm,
            BodyPart::RightArm => self.right_arm,
            BodyPart::LeftLeg => self.left_leg,
            BodyPart::RightLeg => self.right_leg,
        }
    }

    fn part_mut(&mut self, bp: BodyPart) -> &mut PartHealth {
        match bp {
            BodyPart::Head => &mut self.head,
            BodyPart::Torso => &mut self.torso,
            BodyPart::LeftArm => &mut self.left_arm,
            BodyPart::RightArm => &mut self.right_arm,
            BodyPart::LeftLeg => &mut self.left_leg,
            BodyPart::RightLeg => &mut self.right_leg,
        }
    }

    /// Applies damage to one part, clamped at 0. Returns true if this was a
    /// killing blow (a vital part reaching 0) - matching DDA, where a
    /// broken limb disables it but only head/torso loss is lethal.
    pub fn damage(&mut self, bp: BodyPart, amount: i32) -> bool {
        let part = self.part_mut(bp);
        part.current = (part.current - amount).max(0);
        part.current == 0 && bp.is_vital()
    }

    pub fn heal(&mut self, bp: BodyPart, amount: i32) {
        let part = self.part_mut(bp);
        part.current = (part.current + amount).min(part.max);
    }

    pub fn is_dead(&self) -> bool {
        self.head.is_broken() || self.torso.is_broken()
    }

    pub fn parts(&self) -> [(BodyPart, PartHealth); 6] {
        BodyPart::ALL.map(|bp| (bp, self.part(bp)))
    }

    /// A single aggregate (current, max) summed across all parts - handy
    /// for a compact status-line number when the full per-part breakdown
    /// would be too wide to show at a glance.
    pub fn total(&self) -> (i32, i32) {
        self.parts().iter().fold((0, 0), |(c, m), (_, p)| (c + p.current, m + p.max))
    }
}

/// Real DDA hides exact HP numbers by default, showing a 5-character bar
/// instead (`|` filled, `:` empty, `-----` for a fully destroyed part) -
/// only the Self-Aware trait reveals precise numbers. An approximation of
/// the real formula (couldn't verify the exact pip-rounding rule from
/// source), but the visual language matches: an idea of your condition,
/// not a precise readout, unless you have that trait.
/// Real BN's HP bar doesn't just drop straight from a full pip ('|') to an
/// empty one (':') - a pip passes through a "worn" state ('\') first, so a
/// part that's about to lose a pip reads as visually declining rather than
/// jumping. Modeled as 10 half-steps spread across the 5 pips.
pub fn hp_bar(current: i32, max: i32) -> String {
    if current <= 0 {
        return "-----".to_string();
    }
    let ratio = if max > 0 { (current as f64 / max as f64).clamp(0.0, 1.0) } else { 0.0 };
    let tenths = ((ratio * 10.0).round() as i32).clamp(1, 10);
    (0..5)
        .map(|i| match tenths - i * 2 {
            slot if slot >= 2 => '|',
            1 => '\\',
            _ => ':',
        })
        .collect()
}

/// The 27 real DDA skills (a 28th, "weapon", is an internal contextual
/// skill in the real game with no meaning to show here). All start at 0 -
/// nothing yet trains or checks them (no combat/crafting/social systems
/// exist), same "logic before the systems that use it" situation as
/// hunger's threshold and health's damage sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Skill {
    Speech,
    Computer,
    FirstAid,
    Mechanics,
    Traps,
    Driving,
    Swimming,
    Fabrication,
    Cooking,
    Tailoring,
    Survival,
    Electronics,
    Archery,
    Gun,
    Launcher,
    Pistol,
    Rifle,
    Shotgun,
    Smg,
    Throw,
    Melee,
    Bashing,
    Cutting,
    Dodge,
    Stabbing,
    Unarmed,
    Chemistry,
}

impl Skill {
    pub const ALL: [Skill; 27] = [
        Skill::Speech,
        Skill::Computer,
        Skill::FirstAid,
        Skill::Mechanics,
        Skill::Traps,
        Skill::Driving,
        Skill::Swimming,
        Skill::Fabrication,
        Skill::Cooking,
        Skill::Tailoring,
        Skill::Survival,
        Skill::Electronics,
        Skill::Archery,
        Skill::Gun,
        Skill::Launcher,
        Skill::Pistol,
        Skill::Rifle,
        Skill::Shotgun,
        Skill::Smg,
        Skill::Throw,
        Skill::Melee,
        Skill::Bashing,
        Skill::Cutting,
        Skill::Dodge,
        Skill::Stabbing,
        Skill::Unarmed,
        Skill::Chemistry,
    ];

    /// Matches real DDA's display names (data/json/skills.json).
    pub fn label(self) -> &'static str {
        match self {
            Skill::Speech => "Speech",
            Skill::Computer => "Computers",
            Skill::FirstAid => "First Aid",
            Skill::Mechanics => "Mechanics",
            Skill::Traps => "Traps",
            Skill::Driving => "Driving",
            Skill::Swimming => "Swimming",
            Skill::Fabrication => "Fabrication",
            Skill::Cooking => "Cooking",
            Skill::Tailoring => "Tailoring",
            Skill::Survival => "Survival",
            Skill::Electronics => "Electronics",
            Skill::Archery => "Archery",
            Skill::Gun => "Marksmanship",
            Skill::Launcher => "Launchers",
            Skill::Pistol => "Handguns",
            Skill::Rifle => "Rifles",
            Skill::Shotgun => "Shotguns",
            Skill::Smg => "Submachine Guns",
            Skill::Throw => "Throwing",
            Skill::Melee => "Melee",
            Skill::Bashing => "Bashing",
            Skill::Cutting => "Cutting",
            Skill::Dodge => "Dodge",
            Skill::Stabbing => "Stabbing",
            Skill::Unarmed => "Unarmed",
            Skill::Chemistry => "Chemistry",
        }
    }
}

/// A survivor's skill levels, 0-10 like real DDA. Sparse - an untrained
/// skill is just absent rather than stored as an explicit 0, since a fresh
/// character starts with nearly all of them untrained.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Skills(std::collections::HashMap<Skill, u8>);

impl Skills {
    pub fn new() -> Self {
        Skills(std::collections::HashMap::new())
    }

    pub fn level(&self, skill: Skill) -> u8 {
        self.0.get(&skill).copied().unwrap_or(0)
    }

    pub fn set_level(&mut self, skill: Skill, level: u8) {
        self.0.insert(skill, level.min(10));
    }

    pub fn train(&mut self, skill: Skill, amount: u8) {
        let level = self.level(skill).saturating_add(amount).min(10);
        self.set_level(skill, level);
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Player {
    pub name: String,
    pub pos: Pos,
    pub inventory: Vec<Item>,
    /// The one item currently held in hand, distinct from carried inventory
    /// - matches DDA's model of wielding being separate from just carrying.
    pub wielded: Option<Item>,
    pub stats: Stats,
    pub health: Health,
    pub skills: Skills,
    /// 0 (fully fed) to 100 (starving). Ticks up slowly over time (see
    /// `World::tick`); nothing happens yet at 100 - that's a balance
    /// decision for later, once there's a reason to tune it carefully.
    pub hunger: i32,
    /// Real DDA's Self-Aware trait reveals exact HP numbers instead of the
    /// abstracted bar; always false for now since there's no traits/
    /// character-creation system yet to ever set it true.
    pub self_aware: bool,
}

/// Deliberately just two states, not DDA/BN's full faction-reputation
/// system - `Friendly` is the only one anything acts on so far. `Hostile`
/// exists now because enemy AI is the very next thing being built on top of
/// this same `Npc` type, not because of any other planned use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Attitude {
    Friendly,
    Hostile,
}

/// A non-player character. Reuses `Health`/`Item` rather than inventing
/// NPC-specific versions, since an NPC is mechanically just another
/// creature on the map - the only new thing it needs is `attitude` to
/// decide what its AI does each turn.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Npc {
    pub name: String,
    pub pos: Pos,
    pub health: Health,
    pub inventory: Vec<Item>,
    pub attitude: Attitude,
    /// A human-like NPC (ally, and eventually neutral/hostile people) is
    /// always '@', same as the player - only its color differs, by
    /// attitude. A creature (a zombie, and eventually other species) gets
    /// its own glyph and a color tied to *what it is*, not just whether
    /// it's hostile - real DDA colors different zombie kinds differently
    /// even though they're all hostile. Set via `with_glyph`.
    pub glyph: char,
    pub color: TileColor,
}

impl Npc {
    pub fn new(name: impl Into<String>, pos: Pos, attitude: Attitude) -> Self {
        let color = match attitude {
            Attitude::Friendly => TileColor::Green,
            Attitude::Hostile => TileColor::Red,
        };
        Npc {
            name: name.into(),
            pos,
            health: Health::from_strength(8),
            inventory: Vec::new(),
            attitude,
            glyph: '@',
            color,
        }
    }

    /// Overrides the default '@' + attitude-color look for a creature type
    /// with its own visual identity.
    pub fn with_glyph(mut self, glyph: char, color: TileColor) -> Self {
        self.glyph = glyph;
        self.color = color;
        self
    }
}

/// A friendly NPC keeps this close to the player before it stops closing
/// the distance, so it reads as "following alongside" rather than either
/// crowding onto the player's own tile or trailing far behind out of sight.
const FOLLOW_DISTANCE: i32 = 3;

/// How many tiles `Map::find_path` may explore while routing a friendly
/// NPC toward the player - comfortably enough to route around a whole
/// building, far short of scanning a meaningful fraction of a real map.
const NPC_PATHFIND_BUDGET: usize = 2000;

/// How far away a hostile notices the player and starts closing in - no
/// line-of-sight/stealth check yet, just distance, matching "simple first".
const HOSTILE_DETECTION_RADIUS: i32 = 15;

/// Bare-fists damage - a wielded weapon with its own `melee_damage`
/// replaces this (see `World::player_attack_damage`), otherwise this is
/// what every attack does. Still flat/guaranteed either way - no
/// accuracy/dodge system yet. Always lands on the torso for the same
/// reason: simplest thing that makes combat exist at all.
const UNARMED_ATTACK_DAMAGE: i32 = 10;
const HOSTILE_ATTACK_DAMAGE: i32 = 5;

pub struct World {
    pub map: Map,
    pub player: Player,
    pub npcs: Vec<Npc>,
    pub turn: u64,
    ground_items: std::collections::HashMap<(i32, i32), Vec<Item>>,
    /// What the last action actually did, in plain words - e.g. "Picked up
    /// Rock." or "Nothing here to pick up." Without this, an action that
    /// silently fails (nothing adjacent to interact with, out of range,
    /// etc.) gives the player no way to tell "it didn't work" apart from
    /// "it worked but I misjudged where".
    pub last_message: Option<String>,
}

/// A serializable snapshot of a `World` - `World` itself isn't directly
/// (de)serializable because `ground_items` keys on a raw `(i32, i32)` tuple,
/// which JSON has no notion of as an object key. Flattening it to a plain
/// `Vec<(Pos, Item)>` here sidesteps that; `World::to_save`/`World::from_save`
/// convert between the two.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SaveData {
    pub map: Map,
    pub player: Player,
    pub npcs: Vec<Npc>,
    pub turn: u64,
    pub ground_items: Vec<(Pos, Item)>,
    pub last_message: Option<String>,
}

impl World {
    /// Save-and-quit: everything needed to resume exactly where the player
    /// left off, at the granularity that already exists (no partial-turn
    /// state to worry about since actions only ever complete a whole turn).
    pub fn to_save(&self) -> SaveData {
        let mut ground_items = Vec::new();
        for (&(x, y), stack) in &self.ground_items {
            for item in stack {
                ground_items.push((Pos { x, y }, item.clone()));
            }
        }
        SaveData {
            map: self.map.clone(),
            player: self.player.clone(),
            npcs: self.npcs.clone(),
            turn: self.turn,
            ground_items,
            last_message: self.last_message.clone(),
        }
    }

    pub fn from_save(data: SaveData) -> Self {
        let mut ground_items: std::collections::HashMap<(i32, i32), Vec<Item>> = std::collections::HashMap::new();
        for (pos, item) in data.ground_items {
            ground_items.entry((pos.x, pos.y)).or_default().push(item);
        }
        World {
            map: data.map,
            player: data.player,
            npcs: data.npcs,
            turn: data.turn,
            ground_items,
            last_message: data.last_message,
        }
    }

    pub fn new(map: Map) -> Self {
        let stats = Stats::default();
        let mut world = World {
            map,
            player: Player {
                name: "Survivor".to_string(),
                pos: Pos { x: 0, y: 0 },
                inventory: Vec::new(),
                wielded: None,
                health: Health::from_strength(stats.strength),
                stats,
                skills: Skills::new(),
                hunger: 0,
                self_aware: false,
            },
            npcs: Vec::new(),
            turn: 0,
            ground_items: std::collections::HashMap::new(),
            last_message: None,
        };
        world.place_player_at_center();
        world
    }

    pub fn place_item(&mut self, x: i32, y: i32, item: Item) {
        self.ground_items.entry((x, y)).or_default().push(item);
    }

    pub fn items_at(&self, x: i32, y: i32) -> &[Item] {
        self.ground_items.get(&(x, y)).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Every ground item currently within `visible`, nearest to the player
    /// first (then by name, for a stable order when several tie on
    /// distance) - the 'V' view screen's item list.
    pub fn visible_items(&self, visible: &VisibleSet) -> Vec<(Pos, &Item)> {
        let mut items: Vec<(Pos, &Item)> = self
            .ground_items
            .iter()
            .filter(|((x, y), _)| visible.is_visible(*x, *y))
            .flat_map(|(&(x, y), stack)| stack.iter().map(move |item| (Pos { x, y }, item)))
            .collect();
        items.sort_by_key(|(pos, item)| {
            let dx = pos.x - self.player.pos.x;
            let dy = pos.y - self.player.pos.y;
            (dx * dx + dy * dy, item.name.clone())
        });
        items
    }

    pub fn npc_at(&self, x: i32, y: i32) -> Option<&Npc> {
        self.npcs.iter().find(|n| n.pos.x == x && n.pos.y == y)
    }

    /// Same lookup as `npc_at`, but returns the index into `npcs` instead
    /// of the NPC itself - needed wherever the caller has to name the NPC
    /// again later (e.g. `Action::Talk`/`SwapPlaces` take an index, not a
    /// borrow, since they mutate `World`).
    pub fn npc_index_at(&self, x: i32, y: i32) -> Option<usize> {
        self.npcs.iter().position(|n| n.pos.x == x && n.pos.y == y)
    }

    /// True if the player or any NPC already occupies this tile - checked
    /// before letting an NPC step onto it, so followers don't pile onto the
    /// player or onto each other.
    fn actor_at(&self, x: i32, y: i32, exclude_npc: usize) -> bool {
        (self.player.pos.x == x && self.player.pos.y == y)
            || self.npcs.iter().enumerate().any(|(i, n)| i != exclude_npc && n.pos.x == x && n.pos.y == y)
    }

    /// Swaps in a freshly generated map (e.g. on terminal resize) and repositions the player.
    pub fn set_map(&mut self, map: Map) {
        self.map = map;
        self.place_player_at_center();
    }

    fn place_player_at_center(&mut self) {
        let cx = self.map.width / 2;
        let cy = self.map.height / 2;
        self.player.pos = self.nearest_walkable(cx, cy);
    }

    fn nearest_walkable(&self, x: i32, y: i32) -> Pos {
        if self.map.tile(x, y).is_some_and(Tile::is_walkable) {
            return Pos { x, y };
        }
        let max_radius = self.map.width.max(self.map.height);
        for radius in 1..=max_radius {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let (nx, ny) = (x + dx, y + dy);
                    if self.map.tile(nx, ny).is_some_and(Tile::is_walkable) {
                        return Pos { x: nx, y: ny };
                    }
                }
            }
        }
        Pos { x, y }
    }

    /// Applies one action. Returns false when the game should end. Bumping into an
    /// impassable tile costs no turn, matching how DDA handles a blocked move -
    /// except a closed door or window sash, which auto-opens instead (also
    /// matching DDA), costing a turn but not moving you onto it yet.
    pub fn step(&mut self, action: Action) -> bool {
        // Cleared before every action, not just left stale, so the sidebar
        // only ever shows feedback for what just happened.
        self.last_message = None;
        match action {
            Action::Quit => return false,
            Action::Move(dir) => {
                let (dx, dy) = direction_delta(dir);
                let (nx, ny) = (self.player.pos.x + dx, self.player.pos.y + dy);
                if let Some(index) = self.npc_index_at(nx, ny) {
                    if self.npcs[index].attitude == Attitude::Hostile {
                        self.attack_npc(index);
                    } else {
                        self.last_message = Some(format!("{} is in the way.", self.npcs[index].name));
                    }
                    return true;
                }
                match self.map.tile(nx, ny) {
                    Some(Tile::DoorClosed) => {
                        self.map.set(nx, ny, Tile::DoorOpen);
                        self.tick();
                    }
                    Some(Tile::Window { open: false, curtain_open }) => {
                        self.map.set(nx, ny, Tile::Window { open: true, curtain_open });
                        self.tick();
                    }
                    Some(t) if t.is_walkable() => {
                        self.player.pos = Pos { x: nx, y: ny };
                        self.tick();
                    }
                    _ => {}
                }
            }
            Action::Open => {
                self.toggle_nearby("Opened.", "Nothing nearby to open.", |t| match t {
                    Tile::DoorClosed => Some(Tile::DoorOpen),
                    Tile::Window { open: false, curtain_open } => {
                        Some(Tile::Window { open: true, curtain_open })
                    }
                    Tile::Window { open: true, curtain_open: false } => {
                        Some(Tile::Window { open: true, curtain_open: true })
                    }
                    _ => None,
                });
            }
            Action::Close => {
                self.toggle_nearby("Closed.", "Nothing nearby to close.", |t| match t {
                    Tile::DoorOpen => Some(Tile::DoorClosed),
                    Tile::Window { open: true, curtain_open } => {
                        Some(Tile::Window { open: false, curtain_open })
                    }
                    Tile::Window { open: false, curtain_open: true } => {
                        Some(Tile::Window { open: false, curtain_open: false })
                    }
                    _ => None,
                });
            }
            // Checks the player's own tile first, then the 8 adjacent ones -
            // matches real DDA, which doesn't require standing exactly on
            // an item to grab it.
            Action::PickUp => {
                let mut found = false;
                for (dx, dy) in std::iter::once((0, 0)).chain(ADJACENT_OFFSETS) {
                    let key = (self.player.pos.x + dx, self.player.pos.y + dy);
                    let popped = self.ground_items.get_mut(&key).and_then(Vec::pop);
                    if let Some(item) = popped {
                        self.last_message = Some(format!("Picked up {}.", item.name));
                        self.player.inventory.push(item);
                        self.tick();
                        if self.ground_items.get(&key).is_some_and(Vec::is_empty) {
                            self.ground_items.remove(&key);
                        }
                        found = true;
                        break;
                    }
                }
                if !found {
                    self.last_message = Some("Nothing here to pick up.".to_string());
                }
            }
            // Never actually reaches here in normal play - the render layer
            // intercepts it to open a UI screen instead. Harmless no-op if
            // it ever does slip through.
            Action::OpenWieldMenu => {}
            Action::Wield(index) => {
                if index < self.player.inventory.len() {
                    let new_wielded = self.player.inventory.remove(index);
                    self.last_message = Some(format!("Wielding {}.", new_wielded.name));
                    if let Some(old) = self.player.wielded.replace(new_wielded) {
                        self.player.inventory.push(old);
                    }
                    self.tick();
                } else {
                    self.last_message = Some("Nothing to wield there.".to_string());
                }
            }
            Action::Unwield => {
                if let Some(old) = self.player.wielded.take() {
                    self.last_message = Some(format!("Put away {}.", old.name));
                    self.player.inventory.push(old);
                    self.tick();
                } else {
                    self.last_message = Some("You have nothing wielded.".to_string());
                }
            }
            Action::Wait => self.tick(),
            // Never actually reaches here in normal play - the render layer
            // intercepts these to open UI screens instead.
            Action::OpenDropMenu | Action::OpenExamine => {}
            Action::Drop(index) => {
                if index < self.player.inventory.len() {
                    let item = self.player.inventory.remove(index);
                    self.last_message = Some(format!("Dropped {}.", item.name));
                    self.place_item(self.player.pos.x, self.player.pos.y, item);
                    self.tick();
                } else {
                    self.last_message = Some("Nothing to drop there.".to_string());
                }
            }
            Action::OpenEatMenu => {}
            Action::Eat(index) => match self.player.inventory.get(index).map(|i| i.kind) {
                Some(ItemKind::Food { hunger_restored }) => {
                    let name = self.player.inventory[index].name.clone();
                    self.player.hunger = (self.player.hunger - hunger_restored).max(0);
                    self.player.inventory.remove(index);
                    self.last_message = Some(format!("Ate {name}."));
                    self.tick();
                }
                _ => self.last_message = Some("You can't eat that.".to_string()),
            },
            Action::SwapPlaces(index) => {
                let adjacent = self.npcs.get(index).is_some_and(|n| {
                    (n.pos.x - self.player.pos.x).abs() <= 1 && (n.pos.y - self.player.pos.y).abs() <= 1
                });
                if adjacent {
                    let npc_pos = self.npcs[index].pos;
                    self.npcs[index].pos = self.player.pos;
                    self.last_message = Some(format!("Swapped places with {}.", self.npcs[index].name));
                    self.player.pos = npc_pos;
                    self.tick();
                } else {
                    self.last_message = Some("They're not close enough for that.".to_string());
                }
            }
            Action::Talk(index) => {
                let adjacent = self.npcs.get(index).is_some_and(|n| {
                    (n.pos.x - self.player.pos.x).abs() <= 1 && (n.pos.y - self.player.pos.y).abs() <= 1
                });
                self.last_message = if adjacent {
                    Some(format!("\"Hey there.\" - {}", self.npcs[index].name))
                } else {
                    Some("They're not close enough for that.".to_string())
                };
            }
            Action::Attack(index) => {
                if index < self.npcs.len() {
                    self.attack_npc(index);
                } else {
                    self.last_message = Some("There's nothing there to attack.".to_string());
                }
            }
        }
        true
    }

    /// Applies `transition` to the first adjacent tile it has an answer for
    /// (fixed scan order, no way to pick among several candidates yet),
    /// setting `last_message` to `success` or `failure` accordingly.
    fn toggle_nearby(&mut self, success: &str, failure: &str, transition: impl Fn(Tile) -> Option<Tile>) {
        for (dx, dy) in ADJACENT_OFFSETS {
            let (x, y) = (self.player.pos.x + dx, self.player.pos.y + dy);
            if let Some(tile) = self.map.tile(x, y).and_then(&transition) {
                self.map.set(x, y, tile);
                self.tick();
                self.last_message = Some(success.to_string());
                return;
            }
        }
        self.last_message = Some(failure.to_string());
    }

    /// Advances the turn counter and anything that passively changes over
    /// time - currently just hunger, ticking up very slowly. Every
    /// turn-costing action should go through this rather than touching
    /// `self.turn` directly, so time-based effects stay centralized.
    fn tick(&mut self) {
        self.turn += 1;
        if self.turn % 100 == 0 {
            self.player.hunger = (self.player.hunger + 1).min(100);
        }
        self.update_npcs();
    }

    /// Every NPC gets one simple action per turn, run right alongside the
    /// player's own turn-costing actions - this is deliberately far simpler
    /// than DDA/BN's real NPC AI. A friendly NPC just closes the distance
    /// to the player when it's wandered off past `FOLLOW_DISTANCE`, one
    /// tile at a time; a hostile one does the same but all the way to
    /// adjacent (no holding distance) once the player comes within
    /// `HOSTILE_DETECTION_RADIUS`, and attacks instead of moving once it's
    /// there. Both route around obstacles via `Map::find_path` rather than
    /// a naive straight-line step (which used to get an ally stuck on the
    /// first corner it walked into - a real bug report, not a
    /// hypothetical). If no path turns up within the search budget (out of
    /// the NPC's own doing, or genuinely unreachable), it just holds
    /// position for that turn rather than erroring.
    fn update_npcs(&mut self) {
        let mut attackers = Vec::new();
        for i in 0..self.npcs.len() {
            let pos = self.npcs[i].pos;
            let (dx, dy) = (self.player.pos.x - pos.x, self.player.pos.y - pos.y);
            let dist_sq = dx * dx + dy * dy;

            match self.npcs[i].attitude {
                Attitude::Friendly if dist_sq <= FOLLOW_DISTANCE * FOLLOW_DISTANCE => continue,
                Attitude::Friendly => {}
                Attitude::Hostile if dist_sq <= 2 => {
                    attackers.push(i);
                    continue;
                }
                Attitude::Hostile if dist_sq > HOSTILE_DETECTION_RADIUS * HOSTILE_DETECTION_RADIUS => continue,
                Attitude::Hostile => {}
            }

            let next = self.map.find_path(pos, self.player.pos, NPC_PATHFIND_BUDGET).and_then(|path| path.get(1).copied());
            if let Some(next) = next {
                if !self.actor_at(next.x, next.y, i) {
                    self.npcs[i].pos = next;
                }
            }
        }
        for i in attackers {
            self.hostile_attacks_player(i);
        }
    }

    /// A wielded weapon's own damage, or bare-fists damage if unwielded
    /// (or wielding something with no `melee_damage` set, e.g. food) -
    /// still no accuracy/dodge system, but wielding something now actually
    /// changes the outcome instead of every hit being identical either way.
    fn player_attack_damage(&self) -> i32 {
        self.player.wielded.as_ref().and_then(|item| item.melee_damage).unwrap_or(UNARMED_ATTACK_DAMAGE)
    }

    /// The player attacking an NPC - reached either by bumping into a
    /// hostile one or via `Action::Attack` from the examine menu. Flat
    /// guaranteed damage to the torso, no accuracy/dodge system yet (see
    /// `Action::Attack`'s doc comment). A kill leaves a corpse item behind
    /// instead of the NPC just vanishing - same glyph slot as the ground
    /// items rendering already handles, colored the same as the NPC was.
    fn attack_npc(&mut self, index: usize) {
        let name = self.npcs[index].name.clone();
        let damage = self.player_attack_damage();
        let lethal = self.npcs[index].health.damage(BodyPart::Torso, damage);
        if lethal {
            let npc = self.npcs.remove(index);
            self.place_item(npc.pos.x, npc.pos.y, Item::new(format!("{name} corpse")).with_glyph('%', npc.color));
            self.last_message = Some(format!("You defeated the {name}."));
        } else {
            self.last_message = Some(format!("You hit the {name} for {damage} damage."));
        }
        self.tick();
    }

    /// A hostile NPC attacking the player, called from `update_npcs` once
    /// it's adjacent - same flat-damage simplicity as `attack_npc`.
    fn hostile_attacks_player(&mut self, index: usize) {
        let name = self.npcs[index].name.clone();
        self.player.health.damage(BodyPart::Torso, HOSTILE_ATTACK_DAMAGE);
        self.last_message = Some(format!("The {name} hits you for {HOSTILE_ATTACK_DAMAGE} damage."));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> Item {
        Item::new(name)
    }

    #[test]
    fn wielding_swaps_with_whatever_was_already_wielded() {
        let map = Map::new(3, 3, Tile::Floor);
        let mut world = World::new(map);
        world.player.inventory = vec![item("Rock"), item("Stick")];

        world.step(Action::Wield(0));
        assert_eq!(world.player.wielded, Some(item("Rock")));
        assert_eq!(world.player.inventory, vec![item("Stick")]);
        assert_eq!(world.turn, 1);

        // Wielding the stick puts the rock back in inventory.
        world.step(Action::Wield(0));
        assert_eq!(world.player.wielded, Some(item("Stick")));
        assert_eq!(world.player.inventory, vec![item("Rock")]);
        assert_eq!(world.turn, 2);

        world.step(Action::Unwield);
        assert_eq!(world.player.wielded, None);
        assert!(world.player.inventory.contains(&item("Stick")));
        assert_eq!(world.turn, 3);
    }

    #[test]
    fn waiting_costs_a_turn_and_nothing_else() {
        let map = Map::new(3, 3, Tile::Floor);
        let mut world = World::new(map);
        let pos_before = world.player.pos;

        assert!(world.step(Action::Wait));
        assert_eq!(world.player.pos, pos_before);
        assert_eq!(world.turn, 1);
    }

    #[test]
    fn hunger_ticks_up_slowly_and_eating_reduces_it() {
        let map = Map::new(3, 3, Tile::Floor);
        let mut world = World::new(map);
        assert_eq!(world.player.hunger, 0);

        for _ in 0..99 {
            world.step(Action::Wait);
        }
        assert_eq!(world.player.hunger, 0, "not yet at the 100-turn tick");
        world.step(Action::Wait);
        assert_eq!(world.turn, 100);
        assert_eq!(world.player.hunger, 1, "one tick every 100 turns");

        world.player.inventory = vec![Item::food("Blueberries", 20)];
        world.step(Action::Eat(0));
        assert_eq!(world.player.hunger, 0, "clamped at 0, not negative");
        assert!(world.player.inventory.is_empty());
    }

    #[test]
    fn cannot_eat_a_non_food_item() {
        let map = Map::new(3, 3, Tile::Floor);
        let mut world = World::new(map);
        world.player.inventory = vec![item("Rock")];

        world.step(Action::Eat(0));
        assert_eq!(world.player.inventory, vec![item("Rock")], "rock is not food, nothing happens");
        assert_eq!(world.turn, 0, "no turn cost for a no-op");
    }

    #[test]
    fn default_strength_gives_sixty_hp_per_part_matching_real_dda() {
        let health = Health::from_strength(8);
        for (_, part) in health.parts() {
            assert_eq!(part, PartHealth { current: 60, max: 60 });
        }
    }

    #[test]
    fn losing_a_limb_does_not_kill_but_losing_torso_or_head_does() {
        let mut health = Health::from_strength(8);

        let killed = health.damage(BodyPart::LeftArm, 60);
        assert!(!killed, "a broken arm is not lethal");
        assert!(health.left_arm.is_broken());
        assert!(!health.is_dead());

        let killed = health.damage(BodyPart::Torso, 60);
        assert!(killed, "torso reaching 0 is a killing blow");
        assert!(health.is_dead());
    }

    #[test]
    fn damage_and_heal_clamp_correctly() {
        let mut health = Health::from_strength(8);

        health.damage(BodyPart::Head, 1000);
        assert_eq!(health.head.current, 0, "clamped at 0, not negative");

        health.heal(BodyPart::Head, 1000);
        assert_eq!(health.head.current, health.head.max, "clamped at max, not over-healed");
    }

    #[test]
    fn total_sums_current_and_max_across_all_parts() {
        let mut health = Health::from_strength(8);
        health.damage(BodyPart::LeftLeg, 10);
        let (current, max) = health.total();
        assert_eq!(max, 60 * 6);
        assert_eq!(current, 60 * 6 - 10);
    }

    #[test]
    fn hp_bar_shows_an_idea_not_a_number() {
        assert_eq!(hp_bar(60, 60), "|||||");
        assert_eq!(hp_bar(30, 60), "||\\::", "half health shows a worn pip before it fully drops");
        assert_eq!(hp_bar(1, 60), "\\::::", "barely alive shows a sliver, not a full pip");
        assert_eq!(hp_bar(0, 60), "-----", "fully destroyed part");
    }

    #[test]
    fn skills_default_to_zero_and_train_up_clamped_at_ten() {
        let mut skills = Skills::new();
        assert_eq!(skills.level(Skill::Survival), 0, "untrained skills default to 0");

        skills.train(Skill::Survival, 3);
        assert_eq!(skills.level(Skill::Survival), 3);

        skills.set_level(Skill::Survival, 255);
        assert_eq!(skills.level(Skill::Survival), 10, "clamped at the real DDA cap");
    }

    #[test]
    fn dropping_places_the_item_on_the_players_own_tile() {
        let map = Map::new(3, 3, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 1, y: 1 };
        world.player.inventory = vec![item("Rock"), item("Stick")];

        world.step(Action::Drop(0));
        assert_eq!(world.player.inventory, vec![item("Stick")], "only the dropped item leaves inventory");
        assert_eq!(world.items_at(1, 1), &[item("Rock")]);
        assert_eq!(world.turn, 1);

        // Dropping with an out-of-range index is a harmless no-op.
        world.step(Action::Drop(99));
        assert_eq!(world.turn, 1);
    }

    #[test]
    fn blueberry_bushes_block_both_sight_and_movement() {
        let mut map = Map::new(3, 3, Tile::Grass);
        map.set(2, 1, Tile::BlueberryBush);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 1, y: 1 };

        assert!(world.step(Action::Move(Direction::East)));
        assert_eq!(world.player.pos, Pos { x: 1, y: 1 }, "bush blocks the step");
        assert_eq!(world.turn, 0, "blocked bump costs no turn");
        assert!(!world.map.transparent(2, 1), "bush also blocks sight");
    }

    #[test]
    fn picking_up_moves_an_item_from_ground_to_inventory() {
        let map = Map::new(3, 3, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 1, y: 1 };
        world.place_item(1, 1, Item::new("Rock"));

        assert_eq!(world.items_at(1, 1).len(), 1);
        world.step(Action::PickUp);
        assert_eq!(world.player.inventory, vec![Item::new("Rock")]);
        assert!(world.items_at(1, 1).is_empty());
        assert_eq!(world.turn, 1);

        // Picking up again with nothing there costs no turn.
        world.step(Action::PickUp);
        assert_eq!(world.turn, 1);
    }

    #[test]
    fn visible_items_lists_only_what_is_in_sight_nearest_first() {
        let map = Map::new(20, 20, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 10, y: 10 };
        world.place_item(12, 10, Item::new("Far Rock"));
        world.place_item(11, 10, Item::new("Near Rock"));
        world.place_item(2, 2, Item::new("Out Of Sight Rock")); // far outside the radius below

        let visible = world.map.visible_from(world.player.pos, 5);
        let items = world.visible_items(&visible);
        let names: Vec<&str> = items.iter().map(|(_, i)| i.name.as_str()).collect();
        assert_eq!(names, vec!["Near Rock", "Far Rock"], "nearest first, and the out-of-range one excluded");
    }

    #[test]
    fn picking_up_also_works_from_an_adjacent_tile() {
        let map = Map::new(3, 3, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 1, y: 1 };
        world.place_item(2, 1, Item::new("Rock")); // adjacent, not underfoot

        world.step(Action::PickUp);
        assert_eq!(world.player.inventory, vec![Item::new("Rock")]);
        assert!(world.items_at(2, 1).is_empty());
        assert_eq!(world.turn, 1);
    }

    #[test]
    fn actions_report_clear_feedback_including_silent_failures() {
        let map = Map::new(3, 3, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 1, y: 1 };

        // Nothing nearby: pickup, open, and close should all say so, not
        // silently do nothing with no way to tell it failed.
        world.step(Action::PickUp);
        assert_eq!(world.last_message.as_deref(), Some("Nothing here to pick up."));

        world.step(Action::Open);
        assert_eq!(world.last_message.as_deref(), Some("Nothing nearby to open."));

        world.step(Action::Close);
        assert_eq!(world.last_message.as_deref(), Some("Nothing nearby to close."));

        // A real pickup reports what was picked up.
        world.place_item(1, 1, Item::new("Rock"));
        world.step(Action::PickUp);
        assert_eq!(world.last_message.as_deref(), Some("Picked up Rock."));

        // Move doesn't set a message, and the old one doesn't linger.
        world.step(Action::Move(Direction::East));
        assert_eq!(world.last_message, None);
    }

    #[test]
    fn find_path_takes_the_straight_line_when_nothing_blocks_it() {
        let map = Map::new(10, 10, Tile::Floor);
        let path = map.find_path(Pos { x: 1, y: 1 }, Pos { x: 4, y: 1 }, 1000).unwrap();
        assert_eq!(path.first(), Some(&Pos { x: 1, y: 1 }));
        assert_eq!(path.last(), Some(&Pos { x: 4, y: 1 }));
        // Diagonal movement is allowed, so 3 tiles over in a straight line
        // is reachable in exactly 3 steps (4 positions including start).
        assert_eq!(path.len(), 4);
    }

    #[test]
    fn find_path_routes_around_a_wall() {
        let mut map = Map::new(12, 12, Tile::Floor);
        for y in 0..9 {
            map.set(5, y, Tile::Wall);
        }
        let path = map.find_path(Pos { x: 2, y: 4 }, Pos { x: 8, y: 4 }, 2000).unwrap();
        assert_eq!(path.last(), Some(&Pos { x: 8, y: 4 }));
        assert!(path.iter().all(|p| map.tile(p.x, p.y).unwrap().is_walkable()));
    }

    #[test]
    fn find_path_gives_up_within_budget_when_truly_unreachable() {
        let mut map = Map::new(10, 10, Tile::Floor);
        // Fully enclose (5,5) in a 1-tile box of walls.
        for (dx, dy) in ADJACENT_OFFSETS {
            map.set(5 + dx, 5 + dy, Tile::Wall);
        }
        assert!(map.find_path(Pos { x: 0, y: 0 }, Pos { x: 5, y: 5 }, 500).is_none());
    }

    #[test]
    fn a_corner_next_to_a_window_still_renders_as_a_corner() {
        // 3x3 building perimeter with a window replacing the top wall's
        // east neighbor of the top-left corner.
        let mut map = Map::new(5, 5, Tile::Floor);
        for x in 1..4 {
            map.set(x, 1, Tile::Wall);
            map.set(x, 3, Tile::Wall);
        }
        for y in 1..4 {
            map.set(1, y, Tile::Wall);
            map.set(3, y, Tile::Wall);
        }
        map.set(2, 1, Tile::Window { open: false, curtain_open: true });

        assert_eq!(map.glyph_at(1, 1), '┌', "top-left corner should stay a corner");
    }

    #[test]
    fn bumping_a_closed_door_opens_it_but_does_not_move_you() {
        let mut map = Map::new(3, 3, Tile::Floor);
        map.set(2, 1, Tile::DoorClosed);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 1, y: 1 };

        assert!(world.step(Action::Move(Direction::East)));
        assert_eq!(world.player.pos, Pos { x: 1, y: 1 }, "should not step onto the door yet");
        assert_eq!(world.map.tile(2, 1), Some(Tile::DoorOpen));
        assert_eq!(world.turn, 1, "opening the door still costs a turn");

        assert!(world.step(Action::Move(Direction::East)));
        assert_eq!(world.player.pos, Pos { x: 2, y: 1 }, "now walks through the open door");
        assert_eq!(world.turn, 2);
    }

    #[test]
    fn open_then_close_a_window_cycles_sash_and_curtain() {
        let mut map = Map::new(3, 3, Tile::Floor);
        map.set(2, 1, Tile::Window { open: false, curtain_open: true });
        let mut world = World::new(map);
        world.player.pos = Pos { x: 1, y: 1 };

        world.step(Action::Open);
        assert_eq!(
            world.map.tile(2, 1),
            Some(Tile::Window { open: true, curtain_open: true }),
            "opens the sash first"
        );

        world.step(Action::Close);
        assert_eq!(
            world.map.tile(2, 1),
            Some(Tile::Window { open: false, curtain_open: true }),
            "closes the sash back, leaving the curtain untouched"
        );

        world.step(Action::Close);
        assert_eq!(
            world.map.tile(2, 1),
            Some(Tile::Window { open: false, curtain_open: false }),
            "sash already closed, so this closes the curtain instead - matches DDA"
        );
        assert_eq!(world.turn, 3);
    }

    #[test]
    fn closed_window_blocks_sight_only_when_curtain_is_closed() {
        let mut map = Map::new(5, 1, Tile::Floor);
        map.set(2, 0, Tile::Window { open: false, curtain_open: true });
        assert!(map.transparent(2, 0));

        map.set(2, 0, Tile::Window { open: false, curtain_open: false });
        assert!(!map.transparent(2, 0));
        assert!(!map.tile(2, 0).unwrap().is_walkable(), "closed sash still blocks movement");
    }

    #[test]
    fn friendly_npc_closes_the_distance_when_far_from_the_player() {
        let map = Map::new(30, 30, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 15, y: 15 };
        world.npcs.push(Npc::new("Ally", Pos { x: 5, y: 15 }, Attitude::Friendly));

        let start_dist = (world.player.pos.x - world.npcs[0].pos.x).abs();
        world.step(Action::Wait);
        let after_dist = (world.player.pos.x - world.npcs[0].pos.x).abs();
        assert!(after_dist < start_dist, "should have stepped one tile closer");
    }

    #[test]
    fn friendly_npc_routes_around_a_wall_corner_instead_of_getting_stuck() {
        // A wall spanning most of the map's height, with a single gap near
        // the bottom - exactly the shape of a building corner that used to
        // trap the ally with the old straight-line-only follow logic.
        let mut map = Map::new(12, 12, Tile::Floor);
        for y in 0..9 {
            map.set(5, y, Tile::Wall);
        }
        let mut world = World::new(map);
        world.player.pos = Pos { x: 8, y: 4 };
        world.npcs.push(Npc::new("Ally", Pos { x: 2, y: 4 }, Attitude::Friendly));

        for _ in 0..30 {
            world.step(Action::Wait);
        }

        let (dx, dy) = (world.player.pos.x - world.npcs[0].pos.x, world.player.pos.y - world.npcs[0].pos.y);
        assert!(
            dx * dx + dy * dy <= FOLLOW_DISTANCE * FOLLOW_DISTANCE,
            "ally should have routed around the wall and caught up, ended at {:?}",
            world.npcs[0].pos
        );
    }

    #[test]
    fn friendly_npc_stays_put_once_within_follow_distance() {
        let map = Map::new(10, 10, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 5, y: 5 };
        world.npcs.push(Npc::new("Ally", Pos { x: 6, y: 5 }, Attitude::Friendly));

        world.step(Action::Wait);
        assert_eq!(world.npcs[0].pos, Pos { x: 6, y: 5 }, "already within FOLLOW_DISTANCE, shouldn't move");
    }

    #[test]
    fn friendly_npc_never_steps_onto_the_players_tile() {
        let map = Map::new(6, 3, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 4, y: 1 };
        world.npcs.push(Npc::new("Ally", Pos { x: 0, y: 1 }, Attitude::Friendly));

        for _ in 0..20 {
            world.step(Action::Wait);
            assert_ne!(world.npcs[0].pos, world.player.pos, "NPC must never occupy the player's tile");
        }
    }

    #[test]
    fn bumping_into_a_friendly_npc_blocks_the_move_without_costing_a_turn() {
        let map = Map::new(5, 5, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 2, y: 2 };
        world.npcs.push(Npc::new("Ally", Pos { x: 3, y: 2 }, Attitude::Friendly));

        let turn_before = world.turn;
        assert!(world.step(Action::Move(Direction::East)));
        assert_eq!(world.player.pos, Pos { x: 2, y: 2 }, "blocked, didn't move onto the NPC");
        assert_eq!(world.turn, turn_before, "a blocked bump costs no turn, same as bumping a wall");
        assert_eq!(world.last_message.as_deref(), Some("Ally is in the way."));
    }

    #[test]
    fn compass_direction_buckets_all_eight_ways_correctly() {
        assert_eq!(compass_direction(0, -5), Direction::North);
        assert_eq!(compass_direction(5, -5), Direction::NorthEast);
        assert_eq!(compass_direction(5, 0), Direction::East);
        assert_eq!(compass_direction(5, 5), Direction::SouthEast);
        assert_eq!(compass_direction(0, 5), Direction::South);
        assert_eq!(compass_direction(-5, 5), Direction::SouthWest);
        assert_eq!(compass_direction(-5, 0), Direction::West);
        assert_eq!(compass_direction(-5, -5), Direction::NorthWest);
    }

    #[test]
    fn clock_starts_at_day_one_eight_am_and_advances_one_second_per_turn() {
        assert_eq!(format_clock(0), "Day 1, 08:00:00");
        assert_eq!(format_clock(3600), "Day 1, 09:00:00");
        // 16 hours after 08:00 rolls into day 2 at midnight.
        assert_eq!(format_clock(16 * 3600), "Day 2, 00:00:00");
    }

    #[test]
    fn swapping_places_trades_positions_with_an_adjacent_npc() {
        let map = Map::new(5, 5, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 2, y: 2 };
        world.npcs.push(Npc::new("Ally", Pos { x: 3, y: 2 }, Attitude::Friendly));

        assert!(world.step(Action::SwapPlaces(0)));
        assert_eq!(world.player.pos, Pos { x: 3, y: 2 });
        assert_eq!(world.npcs[0].pos, Pos { x: 2, y: 2 });
        assert_eq!(world.last_message.as_deref(), Some("Swapped places with Ally."));
    }

    #[test]
    fn swapping_places_fails_gracefully_when_the_npc_is_not_adjacent() {
        let map = Map::new(10, 10, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 2, y: 2 };
        world.npcs.push(Npc::new("Ally", Pos { x: 8, y: 8 }, Attitude::Friendly));

        let turn_before = world.turn;
        world.step(Action::SwapPlaces(0));
        assert_eq!(world.player.pos, Pos { x: 2, y: 2 });
        assert_eq!(world.turn, turn_before);
        assert_eq!(world.last_message.as_deref(), Some("They're not close enough for that."));
    }

    #[test]
    fn talking_to_an_adjacent_npc_gives_a_canned_greeting() {
        let map = Map::new(5, 5, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 2, y: 2 };
        world.npcs.push(Npc::new("Ally", Pos { x: 3, y: 2 }, Attitude::Friendly));

        world.step(Action::Talk(0));
        assert_eq!(world.last_message.as_deref(), Some("\"Hey there.\" - Ally"));
    }

    #[test]
    fn npc_index_at_finds_the_right_npc() {
        let map = Map::new(5, 5, Tile::Floor);
        let mut world = World::new(map);
        world.npcs.push(Npc::new("Ally", Pos { x: 3, y: 2 }, Attitude::Friendly));

        assert_eq!(world.npc_index_at(3, 2), Some(0));
        assert_eq!(world.npc_index_at(0, 0), None);
    }

    #[test]
    fn hostile_npc_closes_in_once_within_detection_radius() {
        let map = Map::new(30, 30, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 15, y: 15 };
        world.npcs.push(Npc::new("Zombie", Pos { x: 5, y: 15 }, Attitude::Hostile));

        world.step(Action::Wait);
        assert_eq!(world.npcs[0].pos, Pos { x: 6, y: 15 }, "within detection radius, should step toward the player");
    }

    #[test]
    fn hostile_npc_ignores_the_player_outside_detection_radius() {
        let map = Map::new(60, 60, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 30, y: 30 };
        world.npcs.push(Npc::new("Zombie", Pos { x: 0, y: 30 }, Attitude::Hostile));

        world.step(Action::Wait);
        assert_eq!(world.npcs[0].pos, Pos { x: 0, y: 30 }, "far outside detection radius, should stay put");
    }

    #[test]
    fn adjacent_hostile_attacks_the_player_instead_of_moving() {
        let map = Map::new(10, 10, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 5, y: 5 };
        world.npcs.push(Npc::new("Zombie", Pos { x: 6, y: 5 }, Attitude::Hostile));

        let hp_before = world.player.health.part(BodyPart::Torso).current;
        world.step(Action::Wait);
        assert_eq!(world.npcs[0].pos, Pos { x: 6, y: 5 }, "attacks in place, doesn't move onto the player");
        assert_eq!(world.player.health.part(BodyPart::Torso).current, hp_before - 5);
        assert!(world.last_message.as_deref().unwrap().contains("Zombie hits you"));
    }

    #[test]
    fn bumping_into_a_hostile_npc_attacks_it_instead_of_blocking() {
        let map = Map::new(5, 5, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 2, y: 2 };
        world.npcs.push(Npc::new("Zombie", Pos { x: 3, y: 2 }, Attitude::Hostile));

        world.step(Action::Move(Direction::East));
        assert_eq!(world.player.pos, Pos { x: 2, y: 2 }, "attacking doesn't move you onto its tile");
        assert_eq!(world.npcs[0].health.part(BodyPart::Torso).current, 50, "60 max - 10 damage");
        // The zombie survives and, still adjacent, gets a same-turn
        // counterattack via the tick() the player's own attack triggers -
        // its message is what's left standing, not the player's swing.
        assert_eq!(world.last_message.as_deref(), Some("The Zombie hits you for 5 damage."));
    }

    #[test]
    fn killing_a_hostile_npc_removes_it_and_reports_the_kill() {
        let map = Map::new(5, 5, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 2, y: 2 };
        let mut zombie = Npc::new("Zombie", Pos { x: 3, y: 2 }, Attitude::Hostile);
        zombie.health.damage(BodyPart::Torso, 55); // leave it at 5 HP, one hit from dead
        world.npcs.push(zombie);

        world.step(Action::Move(Direction::East));
        assert!(world.npcs.is_empty(), "defeated NPCs are removed from the world");
        assert_eq!(world.last_message.as_deref(), Some("You defeated the Zombie."));

        let corpse = &world.items_at(3, 2)[0];
        assert_eq!(corpse.name, "Zombie corpse");
        assert_eq!(corpse.glyph, '%');
        assert_eq!(corpse.color, TileColor::Red, "corpse matches the zombie's own color, not a fixed one");
    }

    #[test]
    fn wielding_a_weapon_replaces_bare_fists_damage() {
        let map = Map::new(5, 5, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 2, y: 2 };
        world.player.wielded = Some(Item::new("Sharp Stick").with_melee_damage(25));
        world.npcs.push(Npc::new("Zombie", Pos { x: 3, y: 2 }, Attitude::Hostile));

        world.step(Action::Move(Direction::East));
        assert_eq!(world.npcs[0].health.part(BodyPart::Torso).current, 35, "60 max - 25 weapon damage, not the 10 unarmed baseline");
    }

    #[test]
    fn wielding_something_with_no_melee_damage_falls_back_to_bare_fists() {
        let map = Map::new(5, 5, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 2, y: 2 };
        world.player.wielded = Some(item("Rock")); // the plain test helper - no melee_damage set
        world.npcs.push(Npc::new("Zombie", Pos { x: 3, y: 2 }, Attitude::Hostile));

        world.step(Action::Move(Direction::East));
        assert_eq!(world.npcs[0].health.part(BodyPart::Torso).current, 50, "60 max - 10 unarmed damage, same as wielding nothing");
    }

    #[test]
    fn world_round_trips_through_save_data() {
        let map = Map::new(5, 5, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 2, y: 2 };
        world.player.name = "Runner".to_string();
        world.turn = 7;
        world.npcs.push(Npc::new("Ally", Pos { x: 3, y: 2 }, Attitude::Friendly));
        world.place_item(1, 1, item("Rock"));
        world.place_item(1, 1, item("Stick"));

        let reloaded = World::from_save(world.to_save());
        assert_eq!(reloaded.player.name, "Runner");
        assert_eq!(reloaded.player.pos, Pos { x: 2, y: 2 });
        assert_eq!(reloaded.turn, 7);
        assert_eq!(reloaded.npcs.len(), 1);
        assert_eq!(reloaded.npcs[0].name, "Ally");
        assert_eq!(reloaded.items_at(1, 1), &[item("Rock"), item("Stick")], "ground items survive the tuple-key flattening");
    }
}
