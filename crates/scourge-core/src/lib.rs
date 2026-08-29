//! Core game state: entities, turn engine, stats. No I/O, no rendering, no networking.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

/// Backend-agnostic terrain color name. `scourge-core` has no rendering
/// dependency, so this is just a palette name - a renderer maps it to
/// whatever color representation it actually supports, at whatever
/// richness it can (truecolor, 256-color, or the basic ANSI 16 as a last
/// resort for things like a bare Linux tty console).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

/// A single terrain type. More kinds (rubble, water...) land here as they're needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// Blocks both movement and sight, like real dense underbrush.
    BlueberryBush,
    /// Blocks both movement and sight - a bigger obstacle than a bush.
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
            Tile::Window { open: true, .. } => '_',
            Tile::Window { open: false, .. } => '"',
            Tile::BlueberryBush => '*',
            Tile::Tree => 't',
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
            Tile::BlueberryBush => TileColor::Blue,
            Tile::Tree => TileColor::DarkGreen,
        }
    }
}

/// A grid of tiles. Generation itself lives in `scourge-world`; this is just the data.
#[derive(Clone)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Generic,
    Food { hunger_restored: i32 },
}

/// A minimal carryable/ground item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub kind: ItemKind,
}

impl Item {
    pub fn new(name: impl Into<String>) -> Self {
        Item { name: name.into(), kind: ItemKind::Generic }
    }

    pub fn food(name: impl Into<String>, hunger_restored: i32) -> Self {
        Item { name: name.into(), kind: ItemKind::Food { hunger_restored } }
    }

    pub fn is_food(&self) -> bool {
        matches!(self.kind, ItemKind::Food { .. })
    }
}

/// The four primary attributes, DDA-style. Deliberately just flat numbers
/// with no derived skills/combat math yet - only `strength` currently does
/// anything (feeds `Health::from_strength`). The others are groundwork for
/// when carry capacity, hit chance, etc. get built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
pub fn hp_bar(current: i32, max: i32) -> String {
    if current <= 0 {
        return "-----".to_string();
    }
    let ratio = if max > 0 { (current as f64 / max as f64).clamp(0.0, 1.0) } else { 0.0 };
    let filled = ((ratio * 5.0).round() as usize).clamp(1, 5);
    format!("{}{}", "|".repeat(filled), ":".repeat(5 - filled))
}

/// The 27 real DDA skills (a 28th, "weapon", is an internal contextual
/// skill in the real game with no meaning to show here). All start at 0 -
/// nothing yet trains or checks them (no combat/crafting/social systems
/// exist), same "logic before the systems that use it" situation as
/// hunger's threshold and health's damage sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
#[derive(Clone, Debug, Default)]
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

pub struct Player {
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attitude {
    Friendly,
    Hostile,
}

/// A non-player character. Reuses `Health`/`Item` rather than inventing
/// NPC-specific versions, since an NPC is mechanically just another
/// creature on the map - the only new thing it needs is `attitude` to
/// decide what its AI does each turn.
pub struct Npc {
    pub name: String,
    pub pos: Pos,
    pub health: Health,
    pub inventory: Vec<Item>,
    pub attitude: Attitude,
}

impl Npc {
    pub fn new(name: impl Into<String>, pos: Pos, attitude: Attitude) -> Self {
        Npc {
            name: name.into(),
            pos,
            health: Health::from_strength(8),
            inventory: Vec::new(),
            attitude,
        }
    }
}

/// A friendly NPC keeps this close to the player before it stops closing
/// the distance, so it reads as "following alongside" rather than either
/// crowding onto the player's own tile or trailing far behind out of sight.
const FOLLOW_DISTANCE: i32 = 3;

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

impl World {
    pub fn new(map: Map) -> Self {
        let stats = Stats::default();
        let mut world = World {
            map,
            player: Player {
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

    pub fn npc_at(&self, x: i32, y: i32) -> Option<&Npc> {
        self.npcs.iter().find(|n| n.pos.x == x && n.pos.y == y)
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
                if let Some(npc) = self.npc_at(nx, ny) {
                    self.last_message = Some(format!("{} is in the way.", npc.name));
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
    /// than DDA/BN's real NPC AI: a friendly NPC just closes the distance
    /// to the player when it's wandered off past `FOLLOW_DISTANCE`, one
    /// tile at a time, and does nothing else (no pathfinding around
    /// obstacles - it can get stuck behind them, which is an accepted
    /// limitation of this first pass). Hostile NPCs don't act yet; that's
    /// the enemy-AI phase this is groundwork for.
    fn update_npcs(&mut self) {
        for i in 0..self.npcs.len() {
            if self.npcs[i].attitude != Attitude::Friendly {
                continue;
            }
            let pos = self.npcs[i].pos;
            let (dx, dy) = (self.player.pos.x - pos.x, self.player.pos.y - pos.y);
            if dx * dx + dy * dy <= FOLLOW_DISTANCE * FOLLOW_DISTANCE {
                continue;
            }
            let (nx, ny) = (pos.x + dx.signum(), pos.y + dy.signum());
            if self.map.tile(nx, ny).is_some_and(Tile::is_walkable) && !self.actor_at(nx, ny, i) {
                self.npcs[i].pos = Pos { x: nx, y: ny };
            }
        }
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
        assert_eq!(hp_bar(30, 60), "|||::", "half health rounds to 3 of 5 pips");
        assert_eq!(hp_bar(1, 60), "|::::", "still shows at least one pip while alive");
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
    fn hostile_npcs_do_not_move_yet() {
        let map = Map::new(30, 30, Tile::Floor);
        let mut world = World::new(map);
        world.player.pos = Pos { x: 15, y: 15 };
        world.npcs.push(Npc::new("Raider", Pos { x: 5, y: 15 }, Attitude::Hostile));

        world.step(Action::Wait);
        assert_eq!(world.npcs[0].pos, Pos { x: 5, y: 15 }, "enemy AI isn't built yet - hostiles are inert");
    }
}
