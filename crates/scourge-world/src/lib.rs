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

/// A region of the map with its own vegetation character - the first,
/// smallest possible step toward real biomes: no distinct ground tile or
/// city placement yet, just "how much and what kind of plant life grows
/// here" varying across the map instead of one uniform density everywhere.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Biome {
    Forest,
    Plains,
}

/// Which biome owns each tile - a cheap Voronoi-style partition (nearest
/// seed point wins) rather than DDA's noise-function approach, since that
/// needs no new dependency and already gives natural, non-rectangular
/// blobs. Seeds alternate Forest/Plains by index instead of a coin flip
/// per seed, so even a small map can't randomly end up all one biome.
struct BiomeMap {
    width: i32,
    cells: Vec<Biome>,
}

impl BiomeMap {
    fn generate(width: i32, height: i32, rng: &mut impl Rng) -> Self {
        let seed_count = ((width * height) / 3000).max(2);
        let seeds: Vec<(i32, i32, Biome)> = (0..seed_count)
            .map(|i| {
                let biome = if i % 2 == 0 { Biome::Forest } else { Biome::Plains };
                (rng.gen_range(0..width), rng.gen_range(0..height), biome)
            })
            .collect();

        let mut cells = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let nearest = seeds
                    .iter()
                    .min_by_key(|&&(sx, sy, _)| (sx - x).pow(2) + (sy - y).pow(2))
                    .expect("seed_count is always at least 2");
                cells.push(nearest.2);
            }
        }
        BiomeMap { width, cells }
    }

    fn at(&self, x: i32, y: i32) -> Biome {
        self.cells[(y * self.width + x) as usize]
    }
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

        let biomes = BiomeMap::generate(width, height, &mut rng);
        scatter_vegetation(&mut map, &mut rng, &biomes);
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
            // Gray for now, matching real DDA's plainest zombie - once
            // there's more than one kind, each gets its own color here.
            npcs.push(Npc::new("Zombie", Pos { x, y }, Attitude::Hostile).with_glyph('Z', scourge_core::TileColor::Gray));
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

/// Scatters ground vegetation across open terrain, never inside a building.
/// Density and mix now depend on `biomes`: Forest is heavy tree/shrub
/// cover with only the occasional grassy clearing, Plains is mostly open
/// with wide tallgrass swaths and just a scattering of trees - the same
/// tile types either way, just very different proportions.
fn scatter_vegetation(map: &mut Map, rng: &mut impl Rng, biomes: &BiomeMap) {
    scatter_tallgrass(map, rng, biomes);

    // Bushes are biome-agnostic for now - a scarce foraging feature
    // wherever there's open ground, not tied to a particular biome yet.
    let bush_count = (map.width * map.height) / 250;
    for _ in 0..bush_count {
        let x = rng.gen_range(1..map.width - 1);
        let y = rng.gen_range(1..map.height - 1);
        if is_open_ground(map.tile(x, y)) {
            map.set(x, y, Tile::BlueberryBush);
        }
    }

    let area = map.width * map.height;
    scatter_trees_and_shrubs(map, rng, biomes, Biome::Forest, area / 60, area / 150);
    scatter_trees_and_shrubs(map, rng, biomes, Biome::Plains, area / 700, area / 900);
}

/// Tallgrass patches, biased by biome: wide swaths in Plains (the classic
/// open-field look), small occasional clearings in Forest.
fn scatter_tallgrass(map: &mut Map, rng: &mut impl Rng, biomes: &BiomeMap) {
    let patch_count = (map.width * map.height) / 900;
    for _ in 0..patch_count {
        let (cx, cy) = (rng.gen_range(1..map.width - 1), rng.gen_range(1..map.height - 1));
        let radius = match biomes.at(cx, cy) {
            Biome::Plains => rng.gen_range(2..6),
            Biome::Forest => rng.gen_range(1..3),
        };
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
}

/// Places `tree_count` trees (mostly mature, some young) and `shrub_count`
/// shrubs, restricted to tiles belonging to `biome`.
fn scatter_trees_and_shrubs(
    map: &mut Map,
    rng: &mut impl Rng,
    biomes: &BiomeMap,
    biome: Biome,
    tree_count: i32,
    shrub_count: i32,
) {
    for _ in 0..tree_count {
        if let Some((x, y)) = find_spot_in_biome(map, rng, biomes, biome) {
            let tile = if rng.gen_bool(0.3) { Tile::YoungTree } else { Tile::Tree };
            map.set(x, y, tile);
        }
    }
    for _ in 0..shrub_count {
        if let Some((x, y)) = find_spot_in_biome(map, rng, biomes, biome) {
            map.set(x, y, Tile::Shrub);
        }
    }
}

/// A handful of random attempts at an open-ground tile inside `biome` - a
/// biome region is typically much bigger than any individual placement
/// budget, so a few retries reliably finds one without scanning the whole
/// map for candidates. `None` on repeated bad luck (a small or absent
/// biome region) just means slightly fewer plants there, not a bug.
fn find_spot_in_biome(map: &Map, rng: &mut impl Rng, biomes: &BiomeMap, biome: Biome) -> Option<(i32, i32)> {
    for _ in 0..8 {
        let x = rng.gen_range(1..map.width - 1);
        let y = rng.gen_range(1..map.height - 1);
        if biomes.at(x, y) == biome && is_open_ground(map.tile(x, y)) {
            return Some((x, y));
        }
    }
    None
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
                // Blue '*' - the bush itself is the green 'b', these are
                // what it drops.
                Item::food("Blueberries", 20).with_glyph('*', scourge_core::TileColor::Blue)
            } else {
                // A gray '*' reads more like a rock than a generic '?', and
                // a rock is a real (if crude) improvised weapon over fists.
                Item::new("Rock").with_glyph('*', scourge_core::TileColor::Gray).with_melee_damage(14)
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

    #[test]
    fn biome_map_always_includes_both_biomes_even_on_a_small_map() {
        let mut rng = rand::thread_rng();
        // Small enough that the area-based seed count would floor to 0
        // without the `.max(2)` floor - this is exactly the case that
        // guards against.
        let biomes = BiomeMap::generate(10, 10, &mut rng);

        let forest = (0..10).flat_map(|y| (0..10).map(move |x| (x, y))).any(|(x, y)| biomes.at(x, y) == Biome::Forest);
        let plains = (0..10).flat_map(|y| (0..10).map(move |x| (x, y))).any(|(x, y)| biomes.at(x, y) == Biome::Plains);
        assert!(forest, "expected at least one Forest tile");
        assert!(plains, "expected at least one Plains tile");
    }

    #[test]
    fn biome_map_forms_contiguous_regions_not_scattered_noise() {
        // A tile's biome should usually match its immediate neighbor's -
        // nearest-seed partitioning gives contiguous blobs, not a
        // salt-and-pepper mix, on a map big enough to have real regions.
        let mut rng = rand::thread_rng();
        let biomes = BiomeMap::generate(60, 60, &mut rng);

        let mut matching_neighbors = 0;
        let mut total = 0;
        for y in 0..59 {
            for x in 0..59 {
                total += 2;
                if biomes.at(x, y) == biomes.at(x + 1, y) {
                    matching_neighbors += 1;
                }
                if biomes.at(x, y) == biomes.at(x, y + 1) {
                    matching_neighbors += 1;
                }
            }
        }
        let ratio = matching_neighbors as f64 / total as f64;
        assert!(ratio > 0.9, "expected mostly-contiguous biome regions, got {ratio:.3} neighbor agreement");
    }

    #[test]
    fn forest_biome_gets_denser_vegetation_than_plains_biome() {
        use rand::SeedableRng;

        // A hand-built, cleanly-split biome map (left half Forest, right
        // half Plains) rather than `generate()`'s random Voronoi seeds -
        // this way the density comparison doesn't depend on where those
        // seeds happen to land, and a seeded RNG keeps the whole test
        // deterministic.
        let (width, height) = (40, 40);
        let mut map = Map::new(width, height, Tile::Grass);
        let cells = (0..height)
            .flat_map(|_| (0..width).map(|x| if x < width / 2 { Biome::Forest } else { Biome::Plains }))
            .collect();
        let biomes = BiomeMap { width, cells };

        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        scatter_vegetation(&mut map, &mut rng, &biomes);

        let is_woody = |t: Tile| matches!(t, Tile::Tree | Tile::YoungTree | Tile::Shrub);
        let count = |x_range: std::ops::Range<i32>| {
            (0..height)
                .flat_map(|y| x_range.clone().map(move |x| (x, y)))
                .filter(|&(x, y)| map.tile(x, y).is_some_and(is_woody))
                .count()
        };
        let forest_count = count(0..width / 2);
        let plains_count = count(width / 2..width);
        assert!(
            forest_count > plains_count * 3,
            "forest half should be much denser than plains: forest={forest_count} plains={plains_count}"
        );
    }
}
