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

fn direction_delta(dir: Direction) -> (i32, i32) {
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

/// A single terrain type. More kinds (doors, rubble, water...) land here as they're needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    Floor,
    Wall,
}

impl Tile {
    pub fn is_walkable(self) -> bool {
        matches!(self, Tile::Floor)
    }

    pub fn glyph(self) -> char {
        match self {
            Tile::Floor => '.',
            Tile::Wall => '#',
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
