//! Display settings that aren't keybindings - currently just sidebar
//! position. Persisted separately from `config/keybindings.json` since
//! they're a different concern, mirroring the same save/load pattern.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct DisplaySettings {
    pub sidebar_on_left: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        DisplaySettings { sidebar_on_left: false }
    }
}

impl DisplaySettings {
    pub fn load(path: &Path) -> io::Result<Self> {
        let data = std::fs::read_to_string(path)?;
        serde_json::from_str(&data).map_err(io::Error::other)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(path, json)
    }
}
