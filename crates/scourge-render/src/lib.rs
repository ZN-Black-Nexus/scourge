//! Rendering trait definitions. Backends (terminal now, tiles later) implement
//! these without game logic depending on any specific backend.

use std::io;

use scourge_core::{Pos, World};

pub trait Renderer {
    fn render(&mut self, world: &World) -> io::Result<()>;
}

/// A viewport into a larger map. Only shifts once the player nears its edge
/// (DDA-style scrolling), instead of recentering on every single move.
pub struct Camera {
    pub x: i32,
    pub y: i32,
}

impl Camera {
    pub fn new() -> Self {
        Camera { x: 0, y: 0 }
    }

    pub fn update(
        &mut self,
        player: Pos,
        viewport_w: i32,
        viewport_h: i32,
        map_w: i32,
        map_h: i32,
        margin: i32,
    ) {
        let margin = margin.max(0);

        if player.x - self.x < margin {
            self.x = player.x - margin;
        } else if self.x + viewport_w - player.x <= margin {
            self.x = player.x - viewport_w + margin + 1;
        }

        if player.y - self.y < margin {
            self.y = player.y - margin;
        } else if self.y + viewport_h - player.y <= margin {
            self.y = player.y - viewport_h + margin + 1;
        }

        self.x = self.x.clamp(0, (map_w - viewport_w).max(0));
        self.y = self.y.clamp(0, (map_h - viewport_h).max(0));
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}
