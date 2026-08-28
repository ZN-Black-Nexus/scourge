//! World/map generation and simulation.

use rand::Rng;
use scourge_core::{Map, Tile};

/// Fixed placeholder world size - much larger than any terminal, so the
/// camera actually has room to scroll. Chunk-based/effectively-infinite
/// generation is future work; this is just enough to make scrolling real.
pub const MAP_WIDTH: i32 = 200;
pub const MAP_HEIGHT: i32 = 200;

/// Generates a simple bordered area with scattered interior obstacles.
/// Placeholder for real mapgen - just enough to make walkable/blocked terrain real.
pub fn generate(width: i32, height: i32) -> Map {
    let mut map = Map::new(width, height, Tile::Floor);

    for x in 0..map.width {
        map.set(x, 0, Tile::Wall);
        map.set(x, map.height - 1, Tile::Wall);
    }
    for y in 0..map.height {
        map.set(0, y, Tile::Wall);
        map.set(map.width - 1, y, Tile::Wall);
    }

    if map.width > 6 && map.height > 6 {
        let mut rng = rand::thread_rng();
        let obstacle_count = (map.width * map.height) / 40;
        for _ in 0..obstacle_count {
            let x = rng.gen_range(2..map.width - 2);
            let y = rng.gen_range(2..map.height - 2);
            map.set(x, y, Tile::Wall);
        }

        // Keep the spawn area clear so the player never starts boxed in.
        let (cx, cy) = (map.width / 2, map.height / 2);
        for dy in -1..=1 {
            for dx in -1..=1 {
                map.set(cx + dx, cy + dy, Tile::Floor);
            }
        }
    }

    map
}
