fn main() -> std::io::Result<()> {
    let world = scourge_core::World::new(60, 20);
    scourge_render_term::run(world)
}
