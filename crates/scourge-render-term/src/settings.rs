//! Display settings that aren't keybindings - currently just sidebar
//! position. Persisted separately from `config/keybindings.json` since
//! they're a different concern, mirroring the same save/load pattern.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One reorderable block of the sidebar - real BN's '}' sidebar-layout
/// screen lets you rearrange these (e.g. limbs below stats); the header
/// (clock + last message) stays pinned above them all, unreorderable, same
/// as BN's own fixed top area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SidebarSection {
    Health,
    Hunger,
    Stats,
    Wielded,
    Compass,
}

impl SidebarSection {
    pub const ALL: [SidebarSection; 5] = [
        SidebarSection::Health,
        SidebarSection::Hunger,
        SidebarSection::Stats,
        SidebarSection::Wielded,
        SidebarSection::Compass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SidebarSection::Health => "Health",
            SidebarSection::Hunger => "Hunger",
            SidebarSection::Stats => "Stats",
            SidebarSection::Wielded => "Wielded item",
            SidebarSection::Compass => "Compass",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DisplaySettings {
    pub sidebar_on_left: bool,
    pub sidebar_order: Vec<SidebarSection>,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        DisplaySettings {
            sidebar_on_left: false,
            sidebar_order: SidebarSection::ALL.to_vec(),
        }
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
