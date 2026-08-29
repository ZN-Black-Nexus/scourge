fn main() -> std::io::Result<()> {
    let generated = scourge_world::generate(scourge_world::MAP_WIDTH, scourge_world::MAP_HEIGHT);
    let mut world = scourge_core::World::new(generated.map);
    for (pos, item) in generated.items {
        world.place_item(pos.x, pos.y, item);
    }
    world.npcs = generated.npcs;
    scourge_render_term::run(world)
}
