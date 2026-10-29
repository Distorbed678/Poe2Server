//! The natural topology choice of an area (`poe_world::map_dirs`): its direction list, the
//! candidates and, per seed, the pick:
//! `cargo run --release -p poe-world --example map_dirs -- <area id> [seed ...]` (from the hub
//! root). `POE_STATE_SEED=<n>` sets the generation state's seed (default 0).

use std::path::Path;

use poe_world::{
    area::topologies,
    map_dirs::{PinState, natural_candidates},
    rng::{TerrainRng, lcg_index},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let area = args.next().ok_or("usage: map_dirs <area id> [seed ...]")?;
    let seeds: Vec<u32> = args.map(|s| s.parse()).collect::<Result<_, _>>()?;
    let gd = poe_data::GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let files = poe_world::generate::CachedFiles::new(gd.files());
    let areas = gd.table("WorldAreas")?;
    let pins = gd.table("MapPins")?;
    let row = areas.find_by_str("Id", &area)?.ok_or("no such area")?.index();
    let seed = std::env::var("POE_STATE_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut state = PinState { seed, dirs: Vec::new() };
    let cands = natural_candidates(&gd, &files, row, &mut state, 1, 8)?;
    for d in &state.dirs {
        println!("dir\t{}\t{}", d.dir, pins.row(d.pin).map(|r| r.str("Id").unwrap_or_default()).unwrap_or_default());
    }
    let names = topologies(&gd, &area)?;
    for c in &cands {
        println!("candidate\t{}:{}\t{}", c.topology + 1, c.orientation, names[c.topology]);
    }
    for s in seeds {
        // The league rolls come first (`phase_cad0`), then the sub-seed.
        let mut rng = TerrainRng::new(s);
        poe_world::league::roll_area(&gd, &area, &mut rng);
        let sub = rng.below(u32::MAX);
        if cands.is_empty() {
            println!("seed\t{s}\tno candidates");
        } else {
            let c = cands[lcg_index(sub, cands.len() as u32) as usize];
            println!("seed\t{s}\t{}:{}", c.topology + 1, c.orientation);
        }
    }
    Ok(())
}
