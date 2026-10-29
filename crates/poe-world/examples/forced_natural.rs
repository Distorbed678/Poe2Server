//! Checks the server's forced layout (WP7.13) against unforced client generations: for each
//! vector (`area level seed tile_hash ...`, `area_vectors` format), [`choose_layout`] gives the
//! topology and orientation the instance server sends, the area is generated with them forced,
//! and the tile hash is compared with the client's natural one:
//! `cargo run --release -p poe-world --example forced_natural -- <vectors.tsv> [--threads N]`
//! (from the hub root). Prints `area seed T:O match|differ|error ...` and a summary.

use std::{path::Path, sync::Mutex};

use poe_world::{
    area::{choose_layout, generate_with_files},
    generate::CachedFiles,
    rng::TerrainRng,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let input = args.first().ok_or("usage: forced_natural <vectors.tsv> [--threads N]")?;
    let threads = args.iter().position(|a| a == "--threads").and_then(|i| args.get(i + 1)).and_then(|t| t.parse().ok()).unwrap_or(4usize);
    let jobs: Vec<(String, u32, u32)> = std::fs::read_to_string(input)?
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Some((f.first()?.to_string(), f.get(2)?.parse().ok()?, f.get(3)?.parse().ok()?))
        })
        .collect();
    let next = Mutex::new(0usize);
    let counts = Mutex::new([0usize; 3]);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let gd = poe_data::GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json")).expect("game data");
                let files = CachedFiles::new(gd.files());
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        *n += 1;
                        *n - 1
                    };
                    let Some((area, seed, client)) = jobs.get(i) else { break };
                    let (line, k) = match choose_layout(&gd, &files, area, *seed, Default::default()) {
                        Ok(f) => {
                            let to = format!("{}:{}", f.topology.unwrap_or(0), f.orientation.unwrap_or(8));
                            match generate_with_files(&gd, &files, area, &mut TerrainRng::new(*seed), Some(f)) {
                                Ok(g) if g.map.tile_hash() == *client => (format!("{area}\t{seed}\t{to}\tmatch"), 0),
                                Ok(g) => (format!("{area}\t{seed}\t{to}\tdiffer\tours {} client {client}", g.map.tile_hash()), 1),
                                Err(e) => (format!("{area}\t{seed}\t{to}\terror\t{e}"), 2),
                            }
                        }
                        Err(e) => (format!("{area}\t{seed}\t-\terror\t{e}"), 2),
                    };
                    println!("{line}");
                    counts.lock().unwrap()[k] += 1;
                }
            });
        }
    });
    let c = counts.into_inner().unwrap();
    println!("--- match {} differ {} error {}", c[0], c[1], c[2]);
    Ok(())
}
