//! Prints the rows of a forced_verify input (stdin or file arg) whose league rolls give a stone
//! circle (mechanic 9): `league_filter <input.tsv>`; keeps the header.
use std::io::BufRead;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: league_filter <input.tsv>")?;
    let gd = poe_data::GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    for line in std::io::BufReader::new(std::fs::File::open(path)?).lines() {
        let line = line?;
        if line.starts_with('#') || line.trim().is_empty() {
            println!("{line}");
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let Ok(seed) = f[1].parse::<u32>() else { continue };
        let mut rng = poe_world::rng::TerrainRng::new(seed);
        if poe_world::league::roll_area(&gd, f[0], &mut rng).stone_circles() > 0 {
            println!("{line}");
        }
    }
    Ok(())
}
