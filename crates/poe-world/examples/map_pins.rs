//! Prints an act's world-map pins as the port lays them out: `map_pins <act> [<state seed>] [--generated] [--objects <text>] [--trace <generator_trace.tsv>]`
//! (`--generated`: from the generated world map instead of the client's graph walk; `--objects`:
//! also list the walked objects whose type contains the text)
//! (`index x y area`, like the client's `map_pins` trace lines). With `--trace` it compares with a
//! client trace of a town of that act (a traced `gen_batch` job, `docs/re/terrain/batch.md`) and
//! exits 1 on any difference.
use std::path::Path;

use poe_data::GameData;
use poe_world::{generate::CachedFiles, map_pins::{act_pins, act_pins_generated}};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let act: i64 = a[0].parse()?;
    let seed: u32 = a.get(1).filter(|s| !s.starts_with("--")).map_or(Ok(0), |s| s.parse())?;
    let gd = GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let files = CachedFiles::new(gd.files());
    let pins = if a.iter().any(|s| s == "--generated") { act_pins_generated(&gd, &files, act, seed)? } else { act_pins(&gd, &files, act, seed)? };
    if let Some(i) = a.iter().position(|s| s == "--objects") {
        // Every walked object whose type contains the text (case-insensitive), with its node.
        let want = a[i + 1].to_ascii_lowercase();
        let area = poe_world::map_pins::world_map_area(&gd, act)?.unwrap_or_default();
        let tops = poe_world::area::topology_entries(&gd, &area, 1)?;
        let path = &tops[poe_world::rng::lcg_index(seed, tops.len() as u32) as usize].0;
        let (mut objects, mut skipped) = (Vec::new(), Vec::new());
        poe_world::map_pins::graph_objects(&files, path, &mut poe_world::rng::TerrainRng::new(seed), (0, 0), &mut objects, &mut skipped, 0)?;
        for o in objects.iter().filter(|o| o.kind.to_ascii_lowercase().contains(&want)) {
            println!("obj {} {} {}", o.x, o.y, o.kind);
        }
        println!("# skipped {skipped:?}");
    }
    let ours: Vec<String> = pins.iter().enumerate().map(|(i, p)| format!("{i} {} {} {}", p.x, p.y, p.id)).collect();
    for l in &ours {
        println!("{l}");
    }
    if let Some(i) = a.iter().position(|s| s == "--trace") {
        // Trace lines: `seq  map_pins  (callers)  <index x y>  <area>`; the first one is `count N`.
        let text = std::fs::read_to_string(&a[i + 1])?;
        let theirs: Vec<String> = text
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                (f.get(1) == Some(&"map_pins") && !f.get(3)?.starts_with("count")).then(|| format!("{} {}", f[3], f.get(4).unwrap_or(&"")))
            })
            .collect();
        let same = ours == theirs;
        println!("# client {} pins, port {}: {}", theirs.len(), ours.len(), if same { "same" } else { "DIFFERENT" });
        if !same {
            for (o, t) in ours.iter().zip(theirs.iter()).filter(|(o, t)| o != t) {
                println!("# port {o} | client {t}");
            }
            std::process::exit(1);
        }
    }
    Ok(())
}
