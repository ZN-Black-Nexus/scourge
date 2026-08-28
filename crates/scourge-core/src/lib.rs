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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Action {
    Move(Direction),
    Quit,
}

pub struct Player {
    pub pos: Pos,
}

pub struct World {
    pub width: i32,
    pub height: i32,
    pub player: Player,
    pub turn: u64,
}

impl World {
    pub fn new(width: i32, height: i32) -> Self {
        World {
            width,
            height,
            player: Player {
                pos: Pos {
                    x: width / 2,
                    y: height / 2,
                },
            },
            turn: 0,
        }
    }

    /// Applies one action and advances the turn counter. Returns false when the game should end.
    pub fn step(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => return false,
            Action::Move(dir) => {
                let (dx, dy) = match dir {
                    Direction::North => (0, -1),
                    Direction::South => (0, 1),
                    Direction::East => (1, 0),
                    Direction::West => (-1, 0),
                    Direction::NorthEast => (1, -1),
                    Direction::NorthWest => (-1, -1),
                    Direction::SouthEast => (1, 1),
                    Direction::SouthWest => (-1, 1),
                };
                self.player.pos.x = (self.player.pos.x + dx).clamp(0, self.width - 1);
                self.player.pos.y = (self.player.pos.y + dy).clamp(0, self.height - 1);
            }
        }
        self.turn += 1;
        true
    }

    /// Updates the playable area to a new size (e.g. on terminal resize), clamping the player back inside it.
    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.player.pos.x = self.player.pos.x.clamp(0, self.width - 1);
        self.player.pos.y = self.player.pos.y.clamp(0, self.height - 1);
    }
}
