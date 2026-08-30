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

fn default_sidebar_width_percent() -> u16 {
    25
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DisplaySettings {
    pub sidebar_on_left: bool,
    pub sidebar_order: Vec<SidebarSection>,
    /// Compass diagonals (NE/NW/SE/SW) are always abbreviated - there's no
    /// shorter form worth having. This only affects the 4 cardinals
    /// (North/South/East/West), and defaults to the full word.
    /// `serde(default)` so a settings.json saved before this field existed
    /// still loads instead of falling back to every other default too.
    #[serde(default)]
    pub compass_abbreviate_cardinals: bool,
    /// A percentage of the terminal's width, not a fixed column count, so
    /// the sidebar scales with the window instead of staying visually
    /// thinner in a wider terminal - clamped to
    /// `[MIN_SIDEBAR_PERCENT, MAX_SIDEBAR_PERCENT]` wherever it's read.
    #[serde(default = "default_sidebar_width_percent")]
    pub sidebar_width_percent: u16,
    /// Real DDA's wider default sidebar shows limbs in 2 rows of 3, not a
    /// single column of 6 - true here means "try that, auto-collapsing to
    /// one column if the sidebar's too narrow to fit 3 across"; false
    /// forces the single column regardless of width.
    #[serde(default = "default_true")]
    pub limbs_two_rows: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        DisplaySettings {
            sidebar_on_left: false,
            sidebar_order: SidebarSection::ALL.to_vec(),
            compass_abbreviate_cardinals: false,
            sidebar_width_percent: default_sidebar_width_percent(),
            limbs_two_rows: true,
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
