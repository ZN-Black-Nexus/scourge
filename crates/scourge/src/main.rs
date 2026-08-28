fn main() -> std::io::Result<()> {
    let world = scourge_core::World::new(scourge_world::generate(1, 1));
    scourge_render_term::run(world)
}
