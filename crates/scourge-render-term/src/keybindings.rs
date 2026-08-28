//! Rebindable letter-key movement bindings, saved as a flat list of
//! (key, action) pairs (not a JSON map, since map keys must be strings and
//! `KeyCode` isn't one). Arrow keys are handled separately in `lib.rs` and
//! are always on, regardless of preset or custom binds.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use crossterm::event::KeyCode;
use scourge_core::{Action, Direction};
use serde::{Deserialize, Serialize};

pub const DIRECTIONS: [Direction; 8] = [
    Direction::North,
    Direction::South,
    Direction::East,
    Direction::West,
    Direction::NorthEast,
    Direction::NorthWest,
    Direction::SouthEast,
    Direction::SouthWest,
];

#[derive(Clone)]
pub struct KeyBindings {
    bindings: HashMap<KeyCode, Action>,
}

#[derive(Serialize, Deserialize)]
struct KeyBindingsFile(Vec<(KeyCode, Action)>);

impl KeyBindings {
    /// DDA/vi convention: hjkl are cardinal, yubn are the diagonals
    /// (spatially matching numpad 7/9/1/3).
    pub fn preset_vi() -> Self {
        use Direction::*;
        let pairs = [
            (KeyCode::Char('h'), West),
            (KeyCode::Char('j'), South),
            (KeyCode::Char('k'), North),
            (KeyCode::Char('l'), East),
            (KeyCode::Char('y'), NorthWest),
            (KeyCode::Char('u'), NorthEast),
            (KeyCode::Char('b'), SouthWest),
            (KeyCode::Char('n'), SouthEast),
        ];
        Self::from_pairs(&pairs)
    }

    /// wasd cardinals, with qweazxc for diagonals (spatially: q/e above,
    /// z/c below, matching the wasd block).
    pub fn preset_wasd() -> Self {
        use Direction::*;
        let pairs = [
            (KeyCode::Char('w'), North),
            (KeyCode::Char('a'), West),
            (KeyCode::Char('s'), South),
            (KeyCode::Char('d'), East),
            (KeyCode::Char('q'), NorthWest),
            (KeyCode::Char('e'), NorthEast),
            (KeyCode::Char('z'), SouthWest),
            (KeyCode::Char('c'), SouthEast),
        ];
        Self::from_pairs(&pairs)
    }

    fn from_pairs(pairs: &[(KeyCode, Direction)]) -> Self {
        let mut bindings: HashMap<KeyCode, Action> =
            pairs.iter().map(|(k, d)| (*k, Action::Move(*d))).collect();
        // 'o' (open/interact) is shared by both presets - it's not a letter
        // either movement scheme uses, and matches real DDA's own binding.
        bindings.insert(KeyCode::Char('o'), Action::Interact);
        KeyBindings { bindings }
    }

    pub fn resolve(&self, code: KeyCode) -> Option<Action> {
        self.bindings.get(&code).copied()
    }

    pub fn key_for(&self, direction: Direction) -> Option<KeyCode> {
        self.bindings
            .iter()
            .find(|(_, action)| **action == Action::Move(direction))
            .map(|(key, _)| *key)
    }

    /// Rebinds `direction` to `key`, removing any prior key bound to it so
    /// each direction has exactly one custom letter binding.
    pub fn bind(&mut self, key: KeyCode, direction: Direction) {
        let action = Action::Move(direction);
        self.bindings.retain(|_, a| *a != action);
        self.bindings.insert(key, action);
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = KeyBindingsFile(self.bindings.iter().map(|(k, a)| (*k, *a)).collect());
        let json = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
        std::fs::write(path, json)
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let data = std::fs::read_to_string(path)?;
        let file: KeyBindingsFile = serde_json::from_str(&data).map_err(io::Error::other)?;
        Ok(KeyBindings {
            bindings: file.0.into_iter().collect(),
        })
    }
}

pub fn describe_key(code: KeyCode) -> String {
    match code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        other => format!("{other:?}"),
    }
}

/// Arrow keys always move (baseline, not part of the rebindable letter map).
pub fn arrow_action(code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Up => Some(Action::Move(Direction::North)),
        KeyCode::Down => Some(Action::Move(Direction::South)),
        KeyCode::Left => Some(Action::Move(Direction::West)),
        KeyCode::Right => Some(Action::Move(Direction::East)),
        _ => None,
    }
}
