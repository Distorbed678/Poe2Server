//! Prints the corner lattice of an area export's job around a tile range: per corner the
//! ground index (letters, legend below), flags in hex, and the `.gt` flags of each ground.
//!
//! `cargo run --release -p poe-world --example lattice_probe -- <export dir> <x0> <y0> <x1> <y1>`
use std::path::Path;

use poe_data::GameData;
use poe_world::{area::Forced, generate::CachedFiles, rng::TerrainRng};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(&a[0]);
    let n = |i: usize| a[i].parse::<i32>().unwrap();
    let meta = std::fs::read_to_string(dir.join("meta.txt"))?;
    let get = |k: &str| meta.lines().find_map(|l| l.strip_prefix(k).and_then(|x| x.strip_prefix('\t'))).unwrap().to_string();
    let gd = GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let files = CachedFiles::new(gd.files());
    let mut rng = TerrainRng::new(get("seed").parse()?);
    let forced = Forced { topology: Some(get("topology").parse()?), orientation: Some(get("orientation").parse()?) };
    let g = poe_world::area::generate_with_files(&gd, &files, &get("area"), &mut rng, Some(forced))?;
    let m = &g.map;
    let mut legend: Vec<String> = Vec::new();
    for y in n(2)..=n(4) {
        let mut row = String::new();
        for x in n(1)..=n(3) {
            let Some(i) = m.point_index(x, y) else { row.push_str(" .. "); continue };
            let p = &m.lattice[i];
            let c = match &p.ground {
                None => '-',
                Some(g) => {
                    let k = legend.iter().position(|l| l == g).unwrap_or_else(|| { legend.push(g.clone()); legend.len() - 1 });
                    (b'a' + k as u8) as char
                }
            };
            row.push_str(&format!(" {c}{:x} ", p.flags));
        }
        println!("{y:4} {row}");
    }
    for (k, g) in legend.iter().enumerate() {
        println!("{} = {g} gt flags {:#x}", (b'a' + k as u8) as char, m.gt_flags(g));
    }
    Ok(())
}
