//! Input abstraction. Keyboard today; a controller backend can implement the
//! same action types later without touching game logic.

use std::io;

use scourge_core::Action;

pub trait InputSource {
    /// Blocks until the next game action is ready.
    fn next_action(&mut self) -> io::Result<Action>;
}
