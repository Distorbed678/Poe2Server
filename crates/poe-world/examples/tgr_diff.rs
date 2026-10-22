//! Compares the port's layout with client exports of failing batch jobs (`<exports dir>/<area>_<seed>_<topology>_<orientation>/layout.tsv`).
//!
//! `cargo run --release -p poe-world --example tgr_diff -- <exports dir> <area> [--max N] [--seed S] [--show K]`
//!
//! Per job: error or the number of differing cells, their bounding box and the first differing
//! cell in row-major order (y outer) with both tiles. A summary of "first differing cell"
//! tile pairs over all jobs follows.

use std::{collections::BTreeMap, path::Path};

use poe_world::{area::Forced, rng::TerrainRng};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root = args.next().ok_or("usage: tgr_diff <exports dir> <area> [--max N] [--seed S] [--show K]")?;
    let area = args.next().ok_or("area")?;
    let (mut max, mut only_seed, mut show) = (20usize, None::<u32>, 3usize);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--max" => max = args.next().unwrap().parse()?,
            "--seed" => only_seed = Some(args.next().unwrap().parse()?),
            "--show" => show = args.next().unwrap().parse()?,
            _ => {}
        }
    }
    let gd = poe_data::GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let mut dirs: Vec<_> = std::fs::read_dir(&root)?.flatten().map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{area}_"))).collect();
    dirs.sort();
    let mut first_pairs: BTreeMap<String, usize> = BTreeMap::new();
    let mut n = 0;
    for d in dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rest = &name[area.len() + 1..];
        let p: Vec<&str> = rest.split('_').collect();
        if p.len() != 3 || name[area.len() + 1..].contains(|c: char| c.is_alphabetic()) {
            continue;
        }
        let (seed, topo, orient): (u32, u32, u8) = (p[0].parse()?, p[1].parse()?, p[2].parse()?);
        if only_seed.is_some_and(|s| s != seed) {
            continue;
        }
        n += 1;
        if n > max {
            break;
        }
        let mut rng = TerrainRng::new(seed);
        let r = poe_world::area::generate_with(&gd, &area, &mut rng, Some(Forced { topology: Some(topo), orientation: Some(orient) }));
        let g = match r {
            Ok(g) => g,
            Err(e) => {
                println!("{name}: error {e}");
                continue;
            }
        };
        let meta = std::fs::read_to_string(d.join("meta.txt"))?;
        let size: Vec<u32> = meta.lines().find(|l| l.starts_with("size_tiles")).unwrap().split('\t').skip(1).map(|v| v.parse().unwrap()).collect();
        let text = std::fs::read_to_string(d.join("layout.tsv"))?;
        let mut cells: Vec<(u32, u32, String)> = Vec::new();
        for l in text.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 7 {
                continue;
            }
            let (x, y): (u32, u32) = (f[0].parse()?, f[1].parse()?);
            cells.push((x, y, format!("{} s{} h{} o{} {}", f[2].to_ascii_lowercase(), f[3], f[4], f[5], f[6])));
        }
        cells.sort_by_key(|c| (c.1, c.0));
        if let Ok(out) = std::env::var("POE_DUMP_OURS") {
            let mut t = String::new();
            for (x, y, _) in &cells {
                if let Some(c) = g.map.get(*x, *y) {
                    t += &format!("{x}\t{y}\t{}\t{}\t{}\t{}\t{},{}\n", c.path, c.sub, c.height, c.orientation, c.offset.0, c.offset.1);
                }
            }
            std::fs::create_dir_all(&out)?;
            std::fs::write(Path::new(&out).join(format!("{name}.tsv")), t)?;
        }
        let mut diffs = Vec::new();
        for (x, y, theirs) in &cells {
            let ours = g.map.get(*x, *y).map(|c| format!("{} s{} h{} o{} {},{}", c.path.to_ascii_lowercase(), c.sub, c.height, c.orientation, c.offset.0, c.offset.1)).unwrap_or_else(|| " s0 h0 o0 0,0".into());
            if &ours != theirs {
                diffs.push((*x, *y, ours, theirs.clone()));
            }
        }
        let sz = format!("{}x{} vs {}x{}", g.map.width, g.map.height, size[0], size[1]);
        if diffs.is_empty() {
            println!("{name}: layout same ({sz}) hash {}", g.map.tile_hash());
            continue;
        }
        let (x0, x1) = (diffs.iter().map(|d| d.0).min().unwrap(), diffs.iter().map(|d| d.0).max().unwrap());
        let (y0, y1) = (diffs.iter().map(|d| d.1).min().unwrap(), diffs.iter().map(|d| d.1).max().unwrap());
        println!("{name}: {} of {} cells differ, bbox {x0},{y0}-{x1},{y1}, size {sz}", diffs.len(), cells.len());
        for dd in diffs.iter().take(show) {
            println!("   {},{}: ours {}\n         client {}", dd.0, dd.1, dd.2, dd.3);
        }
        // Pieces (offset 0,0 cells) that exist on one side only.
        let piece = |s: &str| {
            let f: Vec<&str> = s.split(' ').collect();
            format!("{} {} {}", f[0].rsplit('/').next().unwrap(), f[3], f[2])
        };
        let mut only_ours: Vec<String> = Vec::new();
        let mut only_client: Vec<String> = Vec::new();
        for (x, y, ours, theirs) in &diffs {
            if ours.ends_with(" 0,0") {
                only_ours.push(format!("{x},{y} {}", piece(ours)));
            }
            if theirs.ends_with(" 0,0") {
                only_client.push(format!("{x},{y} {}", piece(theirs)));
            }
        }
        let k = std::env::var("POE_PIECES").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        if k > 0 {
            let mut i = 0;
            println!("   pieces only ours: {} only client: {}", only_ours.len(), only_client.len());
            for (o, c) in only_ours.iter().zip(&only_client) {
                println!("     ours {o}  | client {c}");
                i += 1;
                if i >= k {
                    break;
                }
            }
        }
        let short = |s: &str| s.split(' ').next().unwrap().rsplit('/').next().unwrap().to_string();
        *first_pairs.entry(format!("{} <- {}", short(&diffs[0].2), short(&diffs[0].3))).or_default() += 1;
    }
    println!("first differing cell tile pairs (ours <- client):");
    let mut v: Vec<_> = first_pairs.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (k, c) in v.iter().take(15) {
        println!("  {c} {k}");
    }
    Ok(())
}
