//! Lists the `.tmd` objects of the tiles on an act's world map (`map_pins::world_map`) with their
//! tile's corner, size and orientation: `tile_objects <act> [<state seed>]`, one line per object:
//! `kind corner_x corner_y w h orientation ex ey`.
use std::path::Path;

use poe_data::GameData;
use poe_world::{generate::CachedFiles, map_pins};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let act: i64 = a[0].parse()?;
    let seed: u32 = a.get(1).map_or(Ok(0), |s| s.parse())?;
    let gd = GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let files = CachedFiles::new(gd.files());
    let Some(g) = map_pins::world_map(&gd, &files, act, seed)? else { return Ok(()) };
    let mut live: Vec<usize> = g.map.piece_at.iter().flatten().copied().collect();
    live.sort_unstable();
    live.dedup();
    for i in live {
        let p = &g.map.pieces[i];
        for (ex, ey, kind) in map_pins::tmd_objects(&files, &p.path) {
            println!("{kind}\t{}\t{}\t{}\t{}\t{}\t{ex}\t{ey}", p.x, p.y, p.size.0, p.size.1, p.orientation);
        }
    }
    Ok(())
}
