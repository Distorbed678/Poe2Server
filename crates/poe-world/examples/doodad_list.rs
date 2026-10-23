//! Prints the doodads the port places for one layout: `doodad_list <area> <seed> <topology> <orientation>`
//! (topology 1-based, orientation 0-7), one line per doodad: `x y angle counted solid kind path leaf`,
//! then the doodad hash. For comparing with a client `doodads.tsv` by hand.
use std::path::Path;

use poe_data::GameData;
use poe_world::{area::Forced, generate::CachedFiles, rng::TerrainRng};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let gd = GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let files = CachedFiles::new(gd.files());
    let mut rng = TerrainRng::new(a[1].parse()?);
    let forced = Forced { topology: Some(a[2].parse()?), orientation: Some(a[3].parse()?) };
    let g = poe_world::area::generate_with_files(&gd, &files, &a[0], &mut rng, Some(forced))?;
    for d in &g.doodads {
        println!("{}\t{}\t{:.4}\t{}\t{}\t{}\t{}\t{}", d.x, d.y, d.angle, d.counted, d.solid, d.kind, d.path, d.leaf);
    }
    println!("# doodad hash {}", g.doodad_hash());
    Ok(())
}
