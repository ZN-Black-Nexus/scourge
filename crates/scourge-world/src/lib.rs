//! World/map generation and simulation.

use rand::Rng;
use scourge_core::{Attitude, Item, Map, Npc, Pos, Tile};

/// Fixed placeholder world size - much larger than any terminal, so the
/// camera actually has room to pan. Chunk-based/effectively-infinite
/// generation is future work; this is just enough to make exploration real.
pub const MAP_WIDTH: i32 = 200;
pub const MAP_HEIGHT: i32 = 200;

/// A generated map plus whatever ground items were scattered on it -
/// bundled together since item placement needs to know the terrain.
pub struct Generated {
    pub map: Map,
    pub items: Vec<(Pos, Item)>,
    pub npcs: Vec<Npc>,
}

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
pub fn generate(width: i32, height: i32) -> Generated {
    // Grass is the dominant outdoor ground cover; Dirt appears in patches on
    // top of it for a more natural look, and building interiors get carved
    // to Floor below - all three stay visually distinct from each other.
    let mut map = Map::new(width, height, Tile::Grass);
    border_walls(&mut map);

    let mut items = Vec::new();
    let mut npcs = Vec::new();

    if width > 12 && height > 12 {
        let mut rng = rand::thread_rng();
        scatter_dirt_patches(&mut map, &mut rng);

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

        scatter_vegetation(&mut map, &mut rng);
        items = scatter_items(&map, &mut rng);

        // Placed a few tiles off the player's own spawn point (which is the
        // exact map center - see `World::place_player_at_center`) so the
        // ally starts out visible nearby rather than exactly underfoot,
        // without yet knowing where the player actually landed.
        let ally_pos = find_walkable_near(&map, width / 2 + 3, height / 2);
        npcs.push(Npc::new("Ally", ally_pos, Attitude::Friendly));

        scatter_zombies(&map, &mut rng, width, height, &mut npcs);
    }

    Generated { map, items, npcs }
}

/// A handful of hostile creatures scattered across the map, far enough
/// from the player's own spawn point to not be an instant unfair ambush -
/// just one creature type for now ("simple content first"), real variety
/// is a later, targeted pass.
fn scatter_zombies(map: &Map, rng: &mut impl Rng, width: i32, height: i32, npcs: &mut Vec<Npc>) {
    let count = (width * height) / 4000;
    let (cx, cy) = (width / 2, height / 2);
    let min_spawn_distance = 20;
    for _ in 0..count {
        let x = rng.gen_range(1..width - 1);
        let y = rng.gen_range(1..height - 1);
        let far_enough = (x - cx).abs().max((y - cy).abs()) > min_spawn_distance;
        if far_enough && (is_open_ground(map.tile(x, y)) || map.tile(x, y) == Some(Tile::Floor)) {
            npcs.push(Npc::new("Zombie", Pos { x, y }, Attitude::Hostile));
        }
    }
}

/// Scans outward in expanding rings from `(x, y)` for the nearest walkable
/// tile - a small local search, distinct from (but mirroring) `World`'s own
/// private `nearest_walkable`, since generation happens before there's a
/// `World` to ask.
fn find_walkable_near(map: &Map, x: i32, y: i32) -> Pos {
    if map.tile(x, y).is_some_and(Tile::is_walkable) {
        return Pos { x, y };
    }
    let max_radius = map.width.max(map.height);
    for radius in 1..=max_radius {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let (nx, ny) = (x + dx, y + dy);
                if map.tile(nx, ny).is_some_and(Tile::is_walkable) {
                    return Pos { x: nx, y: ny };
                }
            }
        }
    }
    Pos { x, y }
}

fn is_open_ground(tile: Option<Tile>) -> bool {
    matches!(tile, Some(Tile::Grass) | Some(Tile::Dirt) | Some(Tile::TallGrass))
}

/// Natural-looking dirt patches (small filled circles, not per-tile noise)
/// scattered across the grass.
fn scatter_dirt_patches(map: &mut Map, rng: &mut impl Rng) {
    let patch_count = (map.width * map.height) / 500;
    for _ in 0..patch_count {
        let (cx, cy) = (rng.gen_range(1..map.width - 1), rng.gen_range(1..map.height - 1));
        let radius = rng.gen_range(2..6);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy <= radius * radius && rng.gen_bool(0.6) {
                    let (x, y) = (cx + dx, cy + dy);
                    if map.tile(x, y) == Some(Tile::Grass) {
                        map.set(x, y, Tile::Dirt);
                    }
                }
            }
        }
    }
}

/// Scatters ground vegetation across open terrain, never inside a building:
/// tall grass patches, blueberry bushes, and trees.
fn scatter_vegetation(map: &mut Map, rng: &mut impl Rng) {
    let grass_patches = (map.width * map.height) / 900;
    for _ in 0..grass_patches {
        let (cx, cy) = (rng.gen_range(1..map.width - 1), rng.gen_range(1..map.height - 1));
        let radius = rng.gen_range(2..6);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy <= radius * radius && rng.gen_bool(0.5) {
                    let (x, y) = (cx + dx, cy + dy);
                    if is_open_ground(map.tile(x, y)) {
                        map.set(x, y, Tile::TallGrass);
                    }
                }
            }
        }
    }

    let bush_count = (map.width * map.height) / 250;
    for _ in 0..bush_count {
        let x = rng.gen_range(1..map.width - 1);
        let y = rng.gen_range(1..map.height - 1);
        if is_open_ground(map.tile(x, y)) {
            map.set(x, y, Tile::BlueberryBush);
        }
    }

    let tree_count = (map.width * map.height) / 180;
    for _ in 0..tree_count {
        let x = rng.gen_range(1..map.width - 1);
        let y = rng.gen_range(1..map.height - 1);
        if is_open_ground(map.tile(x, y)) {
            map.set(x, y, Tile::Tree);
        }
    }
}

/// A handful of placeholder ground items to make the inventory/eat
/// mechanics testable. Real loot tables are future work.
fn scatter_items(map: &Map, rng: &mut impl Rng) -> Vec<(Pos, Item)> {
    let count = (map.width * map.height) / 600;
    let mut items = Vec::new();
    for _ in 0..count {
        let x = rng.gen_range(1..map.width - 1);
        let y = rng.gen_range(1..map.height - 1);
        if is_open_ground(map.tile(x, y)) || map.tile(x, y) == Some(Tile::Floor) {
            let item = if rng.gen_bool(0.4) {
                Item::food("Blueberries", 20)
            } else {
                Item::new("Rock")
            };
            items.push((Pos { x, y }, item));
        }
    }
    items
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

    // Doors generate closed - matches DDA, and lets the auto-open-on-bump
    // behavior actually mean something instead of already being open.
    let door = wall_point(r, rng);
    map.set(door.0, door.1, Tile::DoorClosed);

    if rng.gen_bool(0.7) {
        place_window(map, r, rng, door);
    }
}

/// A random point on the building's perimeter (not a corner).
fn wall_point(r: &Rect, rng: &mut impl Rng) -> (i32, i32) {
    match rng.gen_range(0..4) {
        0 => (rng.gen_range(r.x + 1..r.x + r.w - 1), r.y),
        1 => (rng.gen_range(r.x + 1..r.x + r.w - 1), r.y + r.h - 1),
        2 => (r.x, rng.gen_range(r.y + 1..r.y + r.h - 1)),
        _ => (r.x + r.w - 1, rng.gen_range(r.y + 1..r.y + r.h - 1)),
    }
}

/// Places a window (sash closed, curtains open) on a wall cell that isn't
/// the door, retrying a few times rather than guaranteeing one.
fn place_window(map: &mut Map, r: &Rect, rng: &mut impl Rng, avoid: (i32, i32)) {
    for _ in 0..10 {
        let point = wall_point(r, rng);
        if point != avoid {
            map.set(
                point.0,
                point.1,
                Tile::Window { open: false, curtain_open: true },
            );
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_enterable_buildings_and_a_clear_spawn() {
        let generated = generate(MAP_WIDTH, MAP_HEIGHT);
        let map = &generated.map;

        let door_count = (0..map.height)
            .flat_map(|y| (0..map.width).map(move |x| (x, y)))
            .filter(|&(x, y)| map.tile(x, y) == Some(Tile::DoorClosed))
            .count();
        assert!(door_count > 0, "expected at least one building with a closed door");

        let floor_count = (0..map.height)
            .flat_map(|y| (0..map.width).map(move |x| (x, y)))
            .filter(|&(x, y)| map.tile(x, y) == Some(Tile::Floor))
            .count();
        assert!(floor_count > 0, "building interiors should be Floor, distinct from outdoor ground");

        // Spawn safety is World's job (nearest_walkable), not generate()'s -
        // generate() no longer force-clears the map center, since doing so
        // used to be able to punch a hole in a building that landed there.
        let world = scourge_core::World::new(generated.map.clone());
        assert!(
            world.map.tile(world.player.pos.x, world.player.pos.y).unwrap().is_walkable(),
            "player must always spawn on a walkable tile"
        );

        assert!(!generated.items.is_empty(), "expected some scattered ground items");

        let allies: Vec<_> = generated.npcs.iter().filter(|n| n.attitude == scourge_core::Attitude::Friendly).collect();
        assert_eq!(allies.len(), 1, "expected the one guaranteed starter ally");
        assert!(
            world.map.tile(allies[0].pos.x, allies[0].pos.y).unwrap().is_walkable(),
            "the ally must spawn on a walkable tile too"
        );

        let zombies: Vec<_> = generated.npcs.iter().filter(|n| n.attitude == scourge_core::Attitude::Hostile).collect();
        assert!(!zombies.is_empty(), "expected some scattered hostiles on a map this size");
        for zombie in &zombies {
            assert!(
                world.map.tile(zombie.pos.x, zombie.pos.y).unwrap().is_walkable(),
                "every zombie must spawn on a walkable tile"
            );
        }
    }
}
