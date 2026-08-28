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

/// A minimal carryable/ground item - just a name for now. Real items (with
/// weight/volume/containers, matching DDA's actual inventory model) are
/// future work once there's a mod-data pipeline to define them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
}

pub struct Player {
    pub pos: Pos,
    pub inventory: Vec<Item>,
    /// The one item currently held in hand, distinct from carried inventory
    /// - matches DDA's model of wielding being separate from just carrying.
    pub wielded: Option<Item>,
}

pub struct World {
    pub map: Map,
    pub player: Player,
    pub turn: u64,
    ground_items: std::collections::HashMap<(i32, i32), Vec<Item>>,
}

impl World {
    pub fn new(map: Map) -> Self {
        let mut world = World {
            map,
            player: Player {
                pos: Pos { x: 0, y: 0 },
                inventory: Vec::new(),
                wielded: None,
            },
            turn: 0,
            ground_items: std::collections::HashMap::new(),
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
        match action {
            Action::Quit => return false,
            Action::Move(dir) => {
                let (dx, dy) = direction_delta(dir);
                let (nx, ny) = (self.player.pos.x + dx, self.player.pos.y + dy);
                match self.map.tile(nx, ny) {
                    Some(Tile::DoorClosed) => {
                        self.map.set(nx, ny, Tile::DoorOpen);
                        self.turn += 1;
                    }
                    Some(Tile::Window { open: false, curtain_open }) => {
                        self.map.set(nx, ny, Tile::Window { open: true, curtain_open });
                        self.turn += 1;
                    }
                    Some(t) if t.is_walkable() => {
                        self.player.pos = Pos { x: nx, y: ny };
                        self.turn += 1;
                    }
                    _ => {}
                }
            }
            Action::Open => self.toggle_nearby(|t| match t {
                Tile::DoorClosed => Some(Tile::DoorOpen),
                Tile::Window { open: false, curtain_open } => {
                    Some(Tile::Window { open: true, curtain_open })
                }
                Tile::Window { open: true, curtain_open: false } => {
                    Some(Tile::Window { open: true, curtain_open: true })
                }
                _ => None,
            }),
            Action::Close => self.toggle_nearby(|t| match t {
                Tile::DoorOpen => Some(Tile::DoorClosed),
                Tile::Window { open: true, curtain_open } => {
                    Some(Tile::Window { open: false, curtain_open })
                }
                Tile::Window { open: false, curtain_open: true } => {
                    Some(Tile::Window { open: false, curtain_open: false })
                }
                _ => None,
            }),
            Action::PickUp => {
                let key = (self.player.pos.x, self.player.pos.y);
                if let Some(items) = self.ground_items.get_mut(&key) {
                    if let Some(item) = items.pop() {
                        self.player.inventory.push(item);
                        self.turn += 1;
                    }
                    if items.is_empty() {
                        self.ground_items.remove(&key);
                    }
                }
            }
            // Never actually reaches here in normal play - the render layer
            // intercepts it to open a UI screen instead. Harmless no-op if
            // it ever does slip through.
            Action::OpenWieldMenu => {}
            Action::Wield(index) => {
                if index < self.player.inventory.len() {
                    let new_wielded = self.player.inventory.remove(index);
                    if let Some(old) = self.player.wielded.replace(new_wielded) {
                        self.player.inventory.push(old);
                    }
                    self.turn += 1;
                }
            }
            Action::Unwield => {
                if let Some(old) = self.player.wielded.take() {
                    self.player.inventory.push(old);
                    self.turn += 1;
                }
            }
        }
        true
    }

    /// Applies `transition` to the first adjacent tile it has an answer for
    /// (fixed scan order, no way to pick among several candidates yet).
    fn toggle_nearby(&mut self, transition: impl Fn(Tile) -> Option<Tile>) {
        const OFFSETS: [(i32, i32); 8] = [
            (0, -1), (0, 1), (1, 0), (-1, 0),
            (1, -1), (-1, -1), (1, 1), (-1, 1),
        ];
        for (dx, dy) in OFFSETS {
            let (x, y) = (self.player.pos.x + dx, self.player.pos.y + dy);
            if let Some(tile) = self.map.tile(x, y).and_then(&transition) {
                self.map.set(x, y, tile);
                self.turn += 1;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> Item {
        Item { name: name.to_string() }
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
        world.place_item(1, 1, Item { name: "Rock".to_string() });

        assert_eq!(world.items_at(1, 1).len(), 1);
        world.step(Action::PickUp);
        assert_eq!(world.player.inventory, vec![Item { name: "Rock".to_string() }]);
        assert!(world.items_at(1, 1).is_empty());
        assert_eq!(world.turn, 1);

        // Picking up again with nothing there costs no turn.
        world.step(Action::PickUp);
        assert_eq!(world.turn, 1);
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
}
