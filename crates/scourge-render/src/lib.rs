//! Rendering trait definitions. Backends (terminal now, tiles later) implement
//! these without game logic depending on any specific backend.

use std::io;

use scourge_core::{Pos, World};

pub trait Renderer {
    fn render(&mut self, world: &World) -> io::Result<()>;
}

/// A viewport into a larger map, always centered on the player - real DDA
/// behavior: the view pans every single move, it doesn't wait for you to
/// approach an edge.
pub struct Camera {
    pub x: i32,
    pub y: i32,
}

impl Camera {
    pub fn new() -> Self {
        Camera { x: 0, y: 0 }
    }

    pub fn center_on(&mut self, player: Pos, viewport_w: i32, viewport_h: i32, map_w: i32, map_h: i32) {
        self.x = (player.x - viewport_w / 2).clamp(0, (map_w - viewport_w).max(0));
        self.y = (player.y - viewport_h / 2).clamp(0, (map_h - viewport_h).max(0));
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracks which map tiles a viewer has ever seen, for a DDA-style "remembered"
/// state once a tile leaves current view. Deliberately per-viewer, not part of
/// `scourge_core::Map` - the shared world has one truth, but what a given
/// player has personally explored is a client-side concept (relevant once
/// multiplayer means different players have seen different things).
pub struct Explored {
    width: i32,
    height: i32,
    seen: Vec<bool>,
}

impl Explored {
    pub fn new(width: i32, height: i32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Explored {
            width,
            height,
            seen: vec![false; (width * height) as usize],
        }
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            None
        } else {
            Some((y * self.width + x) as usize)
        }
    }

    pub fn is_seen(&self, x: i32, y: i32) -> bool {
        self.index(x, y).map(|i| self.seen[i]).unwrap_or(false)
    }

    fn mark_seen(&mut self, x: i32, y: i32) {
        if let Some(i) = self.index(x, y) {
            self.seen[i] = true;
        }
    }

    /// Marks every tile within `radius` of `center` as seen (circular, not square).
    pub fn reveal(&mut self, center: Pos, radius: i32) {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy <= radius * radius {
                    self.mark_seen(center.x + dx, center.y + dy);
                }
            }
        }
    }
}

pub fn within_radius(a: Pos, b: Pos, radius: i32) -> bool {
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    dx * dx + dy * dy <= radius * radius
}
