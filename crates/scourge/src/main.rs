fn main() -> std::io::Result<()> {
    let map = scourge_world::generate(scourge_world::MAP_WIDTH, scourge_world::MAP_HEIGHT);
    let world = scourge_core::World::new(map);
    scourge_render_term::run(world)
}
