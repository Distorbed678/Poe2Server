//! Generates a `.tgr` graph on its own (as a main graph): `tgr_solo <path.tgr> <seed>...`.
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: tgr_solo <path.tgr> <seed>...")?;
    let files = poe_data::GameFiles::open(Path::new("GameFiles"))?;
    let graph = poe_world::graph::Graph::parse_bytes(&poe_world::generate::Files::read(&files, &path.to_lowercase())?).map_err(|e| format!("{e:?}"))?;
    for seed in args {
        let seed: u32 = seed.parse()?;
        let mut rng = poe_world::rng::TerrainRng::new(seed);
        match poe_world::area::generate_graph(&files, &graph, &mut rng) {
            Ok(m) => println!("{seed}\t{}\t{}x{}", m.tile_hash(), m.width, m.height),
            Err(e) => println!("{seed}\terror\t{e}"),
        }
    }
    Ok(())
}
