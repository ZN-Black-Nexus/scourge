//! World/map generation and simulation.

use rand::Rng;
use scourge_core::{Map, Tile};

/// Fixed placeholder world size - much larger than any terminal, so the
/// camera actually has room to pan. Chunk-based/effectively-infinite
/// generation is future work; this is just enough to make exploration real.
pub const MAP_WIDTH: i32 = 200;
pub const MAP_HEIGHT: i32 = 200;

#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn overlaps(&self, other: &Rect, padding: i32) -> bool {
        !(self.x + self.w + padding < other.x
            || other.x + other.w + padding < self.x
            || self.y + self.h + padding < other.y
            || other.y + other.h + padding < self.y)
    }
}

/// Generates whole rectangular buildings (complete wall perimeter, floor
/// interior, one door) scattered across the map, instead of loose wall
/// clutter. Placeholder for real mapgen - no multi-room buildings, furniture,
/// or loot yet.
pub fn generate(width: i32, height: i32) -> Map {
    let mut map = Map::new(width, height, Tile::Floor);
    border_walls(&mut map);

    if width > 12 && height > 12 {
        let mut rng = rand::thread_rng();
        let attempts = (width * height) / 300;
        let mut placed: Vec<Rect> = Vec::new();

        for _ in 0..attempts {
            let w = rng.gen_range(4..10);
            let h = rng.gen_range(4..8);
            let x = rng.gen_range(2..(width - w - 2).max(3));
            let y = rng.gen_range(2..(height - h - 2).max(3));
            let rect = Rect { x, y, w, h };

            if placed.iter().any(|p| p.overlaps(&rect, 1)) {
                continue;
            }
            place_building(&mut map, &rect, &mut rng);
            placed.push(rect);
        }

        // Keep the spawn area clear so the player never starts boxed in or inside a wall.
        let (cx, cy) = (width / 2, height / 2);
        for dy in -2..=2 {
            for dx in -2..=2 {
                map.set(cx + dx, cy + dy, Tile::Floor);
            }
        }
    }

    map
}

fn border_walls(map: &mut Map) {
    for x in 0..map.width {
        map.set(x, 0, Tile::Wall);
        map.set(x, map.height - 1, Tile::Wall);
    }
    for y in 0..map.height {
        map.set(0, y, Tile::Wall);
        map.set(map.width - 1, y, Tile::Wall);
    }
}

fn place_building(map: &mut Map, r: &Rect, rng: &mut impl Rng) {
    for x in r.x..r.x + r.w {
        map.set(x, r.y, Tile::Wall);
        map.set(x, r.y + r.h - 1, Tile::Wall);
    }
    for y in r.y..r.y + r.h {
        map.set(r.x, y, Tile::Wall);
        map.set(r.x + r.w - 1, y, Tile::Wall);
    }
    for y in r.y + 1..r.y + r.h - 1 {
        for x in r.x + 1..r.x + r.w - 1 {
            map.set(x, y, Tile::Floor);
        }
    }

    match rng.gen_range(0..4) {
        0 => {
            let x = rng.gen_range(r.x + 1..r.x + r.w - 1);
            map.set(x, r.y, Tile::Door);
        }
        1 => {
            let x = rng.gen_range(r.x + 1..r.x + r.w - 1);
            map.set(x, r.y + r.h - 1, Tile::Door);
        }
        2 => {
            let y = rng.gen_range(r.y + 1..r.y + r.h - 1);
            map.set(r.x, y, Tile::Door);
        }
        _ => {
            let y = rng.gen_range(r.y + 1..r.y + r.h - 1);
            map.set(r.x + r.w - 1, y, Tile::Door);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_enterable_buildings_and_a_clear_spawn() {
        let map = generate(MAP_WIDTH, MAP_HEIGHT);

        let door_count = (0..map.height)
            .flat_map(|y| (0..map.width).map(move |x| (x, y)))
            .filter(|&(x, y)| map.tile(x, y) == Some(Tile::Door))
            .count();
        assert!(door_count > 0, "expected at least one building with a door");

        let (cx, cy) = (map.width / 2, map.height / 2);
        for dy in -2..=2 {
            for dx in -2..=2 {
                assert_eq!(
                    map.tile(cx + dx, cy + dy),
                    Some(Tile::Floor),
                    "spawn area must stay walkable"
                );
            }
        }
    }
}
