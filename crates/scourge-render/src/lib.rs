//! Rendering trait definitions. Backends (terminal now, tiles later) implement
//! these without game logic depending on any specific backend.

use std::io;

use scourge_core::World;

pub trait Renderer {
    fn render(&mut self, world: &World) -> io::Result<()>;
}
