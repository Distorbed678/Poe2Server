//! Compares the port's collision grid with the client's `grids.bin` of DLL area exports (WP7.12):
//!
//! `cargo run --release -p poe-world --example check_grids -- <export dir | run dir>...
//! [--only forced.tsv] [--threads N] [--verbose] [--dump DIR]`
//! (`--dump` writes the port's `PWG1` bytes as `DIR/<export name>.bin`)
//!
//! Same inputs as `check_doodads`. Per export: layers that differ with the number of differing
//! cells (`L0:n L1:n ...`) and the bounding box of the first layer that differs.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Mutex, atomic::{AtomicUsize, Ordering}},
};

use poe_data::GameData;
use poe_world::{area::Forced, generate::CachedFiles, rng::TerrainRng};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut dirs = Vec::new();
    let (mut only, mut verbose) = (None::<String>, false);
    let mut dump: Option<PathBuf> = None;
    let mut blame = false;
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--only" => only = Some(args.next().ok_or("--only needs a file")?),
            "--threads" => threads = args.next().ok_or("--threads needs a number")?.parse()?,
            "--verbose" => verbose = true,
            "--blame" => blame = true,
            "--dump" => dump = Some(PathBuf::from(args.next().ok_or("--dump needs a dir")?)),
            _ => dirs.push(PathBuf::from(a)),
        }
    }
    let allowed: Option<HashSet<(String, u32, u32, u8)>> = only.map(|f| {
        std::fs::read_to_string(f)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                (f.len() >= 5 && f[4].starts_with("match")).then(|| Some((f[0].to_string(), f[1].parse().ok()?, f[2].parse().ok()?, f[3].parse().ok()?)))?
            })
            .collect()
    });
    // (dir, area, seed, topology, orientation)
    let mut jobs: Vec<(PathBuf, String, u32, u32, u8)> = Vec::new();
    for d in &dirs {
        let roots: Vec<PathBuf> = if d.join("meta.txt").exists() {
            vec![d.clone()]
        } else {
            let e = if d.join("exports").is_dir() { d.join("exports") } else { d.clone() };
            let mut v: Vec<PathBuf> = std::fs::read_dir(e)?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("meta.txt").exists() && p.join("grids.bin").exists()).collect();
            v.sort();
            v
        };
        for r in roots {
            let meta = std::fs::read_to_string(r.join("meta.txt"))?;
            let get = |k: &str| meta.lines().find_map(|l| l.strip_prefix(k).and_then(|x| x.strip_prefix('\t'))).map(str::to_string);
            let (Some(area), Some(seed), Some(t), Some(o)) = (get("area"), get("seed"), get("topology"), get("orientation")) else { continue };
            let (Ok(seed), Ok(t), Ok(o)) = (seed.parse::<u32>(), t.parse::<u32>(), o.parse::<u8>()) else { continue };
            if allowed.as_ref().is_none_or(|a| a.contains(&(area.clone(), seed, t, o))) {
                jobs.push((r, area, seed, t, o));
            }
        }
    }
    eprintln!("{} exports", jobs.len());
    let gd: &'static GameData = Box::leak(Box::new(GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?));
    let files: &'static CachedFiles<'static> = Box::leak(Box::new(CachedFiles::new(gd.files())));
    std::panic::set_hook(Box::new(|_| {}));
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<(usize, String, bool)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((dir, area, seed, t, o)) = jobs.get(i) else { break };
                let name = dir.file_name().unwrap().to_string_lossy().to_string();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut rng = TerrainRng::new(*seed);
                    let forced = Forced { topology: Some(*t), orientation: Some(*o) };
                    let g = poe_world::area::generate_with_files(gd, files, area, &mut rng, Some(forced))?;
                    let grid = poe_world::collision::build(&g.map, files)?;
                    Ok::<_, poe_world::generate::GenError>((grid, g.map))
                }));
                let (line, ok) = match r {
                    Ok(Ok((grid, map))) => {
                        let ours = grid.pwg1();
                        if let Some(d) = &dump {
                            std::fs::create_dir_all(d).ok();
                            std::fs::write(d.join(format!("{name}.bin")), &ours).ok();
                        }
                        let theirs = std::fs::read(dir.join("grids.bin")).unwrap_or_default();
                        if ours == theirs {
                            (format!("ok   {name}"), true)
                        } else if ours.len() != theirs.len() || ours[..20] != theirs[..20] {
                            (format!("FAIL {name} size/header differ (ours {} client {})", ours.len(), theirs.len()), false)
                        } else {
                            let bpr = grid.cells_x.div_ceil(2);
                            let size = bpr * grid.cells_y;
                            let mut parts = Vec::new();
                            let mut first = None;
                            for l in 0..4 {
                                let (a, b) = (&ours[20 + l * size..20 + (l + 1) * size], &theirs[20 + l * size..20 + (l + 1) * size]);
                                let mut n = 0;
                                for (i, (x, y)) in a.iter().zip(b).enumerate() {
                                    for half in 0..2 {
                                        let cx = (i % bpr) * 2 + half;
                                        if cx < grid.cells_x && (x >> (4 * half)) & 15 != (y >> (4 * half)) & 15 {
                                            n += 1;
                                            first.get_or_insert((l, cx, i / bpr, (x >> (4 * half)) & 15, (y >> (4 * half)) & 15));
                                        }
                                    }
                                }
                                parts.push(format!("L{l}:{n}"));
                            }
                            if blame {
                                // Tiles (by owner of the cell) with a differing cell, keyed by sub-tile flags.
                                let theirs_g = |l: usize, x: usize, y: usize| (theirs[20 + l * size + y * bpr + x / 2] >> (4 * (x & 1))) & 15;
                                let mut tiles: std::collections::BTreeMap<(u32, u32), usize> = Default::default();
                                for l in 0..2 {
                                    for y in 0..grid.cells_y {
                                        for x in 0..grid.cells_x {
                                            if grid.get(l, x, y) != theirs_g(l, x, y) {
                                                *tiles.entry(((x / 23) as u32, (y / 23) as u32)).or_default() += 1;
                                            }
                                        }
                                    }
                                }
                                let mut hist: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
                                for ty in 0..map.height {
                                    for tx in 0..map.width {
                                        let key = match map.get(tx, ty) {
                                            Some(c) => match poe_world::generate::load_tdt(files, &c.path).ok().and_then(|t| t.subtiles.get(c.sub as usize).cloned()) {
                                                Some(st) => format!("flags {:#04x} b1 {} o{} list{}", st.flags, st.b1, c.orientation, if st.list.is_empty() { 0 } else { 1 }),
                                                None => "nosub".to_string(),
                                            },
                                            None => "empty".to_string(),
                                        };
                                        let e = hist.entry(key).or_default();
                                        e.0 += 1;
                                        e.1 += tiles.contains_key(&(tx, ty)) as usize;
                                    }
                                }
                                for (k, (n, bad)) in hist {
                                    if bad > 0 {
                                        eprintln!("  BLAME {name} {k}: {bad}/{n} tiles");
                                    }
                                }
                            }
                            let f = first.map(|(l, x, y, a, b)| format!(" first L{l} ({x},{y}) ours {a} client {b}")).unwrap_or_default();
                            (format!("FAIL {name} cells differ {}{f}", parts.join(" ")), false)
                        }
                    }
                    Ok(Err(e)) => (format!("ERR  {name} {e}"), false),
                    Err(_) => (format!("PANIC {name}"), false),
                };
                out.lock().unwrap().push((i, line, ok));
            });
        }
    });
    let mut out = out.into_inner().unwrap();
    out.sort();
    for (_, l, ok) in &out {
        if !ok || verbose {
            println!("{l}");
        }
    }
    println!("grids match in {} of {} exports", out.iter().filter(|o| o.2).count(), out.len());
    Ok(())
}
