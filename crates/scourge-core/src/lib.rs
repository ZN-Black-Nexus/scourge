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
    Quit,
}

/// A single terrain type. More kinds (rubble, water...) land here as they're needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    Floor,
    Wall,
    Door,
}

impl Tile {
    pub fn is_walkable(self) -> bool {
        matches!(self, Tile::Floor | Tile::Door)
    }

    pub fn glyph(self) -> char {
        match self {
            Tile::Floor => '.',
            Tile::Wall => '#',
            Tile::Door => '+',
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
        !matches!(self.tile(x, y), Some(Tile::Wall))
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

    fn wall_glyph(&self, x: i32, y: i32) -> char {
        let is_wall = |dx: i32, dy: i32| self.tile(x + dx, y + dy) == Some(Tile::Wall);
        let (n, s, e, w) = (is_wall(0, -1), is_wall(0, 1), is_wall(1, 0), is_wall(-1, 0));
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

pub struct Player {
    pub pos: Pos,
}

pub struct World {
    pub map: Map,
    pub player: Player,
    pub turn: u64,
}

impl World {
    pub fn new(map: Map) -> Self {
        let mut world = World {
            map,
            player: Player {
                pos: Pos { x: 0, y: 0 },
            },
            turn: 0,
        };
        world.place_player_at_center();
        world
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
    /// impassable tile costs no turn, matching how DDA handles a blocked move.
    pub fn step(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => return false,
            Action::Move(dir) => {
                let (dx, dy) = direction_delta(dir);
                let (nx, ny) = (self.player.pos.x + dx, self.player.pos.y + dy);
                if self.map.tile(nx, ny).is_some_and(Tile::is_walkable) {
                    self.player.pos = Pos { x: nx, y: ny };
                    self.turn += 1;
                }
            }
        }
        true
    }
}
