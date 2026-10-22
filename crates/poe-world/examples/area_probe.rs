//! Tries to generate every WorldAreas row that has topologies, one attempt per topology,
//! orientation and seed, and prints one TSV row per attempt:
//! `area  topology(1-based)  orientation  graph  seconds  ok|error|panic|timeout  detail  seed`
//! (`detail` is the tile hash and draw count, or the error). It checks no hash: it shows what the
//! generator can run at all, which `cargo xtask terrain-matrix` reads as the probe column. Rows
//! come out in job order (area, topology, orientation, seed) whatever the thread timing.
//!
//! `cargo run --release -p poe-world --example area_probe -- [seed 1001] [area filter | all]
//! [timeout seconds 30] [--seeds 1001,1002,1003 (then no leading seed)] [--orientations first|all|N] [--threads N]
//! > dumps/<version>/terrain/vectors/probe-results.tsv`
//!
//! - `--seeds` replaces the positional seed (a comma list).
//! - `--orientations`: `first` (default: the first allowed orientation of each topology), `all`
//!   (every allowed one) or a number (that orientation, for the topologies that allow it).
//! - `--threads N` (default: all cores): attempts run on a pool; an attempt that exceeds the
//!   timeout is abandoned (its thread keeps spinning); after eight the run stops.
//!
//! Files from before the seed column (seven columns) are still read by the matrix.

use std::{
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use poe_data::GameData;
use poe_world::{area::Forced, generate::CachedFiles, rng::TerrainRng};

enum Orientations {
    First,
    All,
    One(u8),
}

/// The orientations a topology allows (bit per orientation; 0 = no list: unturned).
fn allowed(mask: u8) -> Vec<u8> {
    if mask == 0 {
        return vec![0];
    }
    (0..8).filter(|o| mask & (1 << o) != 0).collect()
}

struct Job {
    area: String,
    topology: u32,
    orientation: u8,
    graph: String,
    seed: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut positional = Vec::new();
    let mut seeds: Option<Vec<u32>> = None;
    let mut orientations = Orientations::First;
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--seeds" => seeds = Some(args.next().ok_or("--seeds needs a list")?.split(',').map(|s| s.trim().parse()).collect::<Result<_, _>>()?),
            "--orientations" => {
                orientations = match args.next().ok_or("--orientations needs first|all|N")?.as_str() {
                    "first" => Orientations::First,
                    "all" => Orientations::All,
                    n => Orientations::One(n.parse()?),
                }
            }
            "--threads" => threads = args.next().ok_or("--threads needs a number")?.parse()?,
            _ => positional.push(a),
        }
    }
    // Positional: [seed] [area | all] [timeout]; with --seeds the leading seed is left out.
    let seeds = match seeds {
        Some(s) => s,
        None => vec![if positional.is_empty() { 1001 } else { positional.remove(0).parse()? }],
    };
    let filter = positional.first().filter(|a| *a != "all").cloned();
    let timeout = Duration::from_secs(positional.get(1).map(|s| s.parse()).transpose()?.unwrap_or(30));
    let gd: &'static GameData = Box::leak(Box::new(GameData::open(
        Path::new("GameFiles"),
        Path::new("third_party/dat-schema/schema-poe2.min.json"),
    )?));
    let files: &'static CachedFiles<'static> = Box::leak(Box::new(CachedFiles::new(gd.files())));
    std::panic::set_hook(Box::new(|_| {}));
    let ids: Vec<String> = gd.table("WorldAreas")?.rows().filter_map(|r| r.str("Id").ok()).collect();
    let mut jobs: Vec<Job> = Vec::new();
    for id in ids {
        if filter.as_deref().is_some_and(|f| f != id) {
            continue;
        }
        let Ok(entries) = poe_world::area::topology_entries(gd, &id, 1) else { continue };
        for (t, (path, mask)) in (1u32..).zip(&entries) {
            let list = allowed(*mask);
            let chosen: Vec<u8> = match orientations {
                Orientations::First => vec![list[0]],
                Orientations::All => list,
                Orientations::One(n) => list.into_iter().filter(|o| *o == n).collect(),
            };
            for o in chosen {
                for &seed in &seeds {
                    jobs.push(Job { area: id.clone(), topology: t, orientation: o, graph: path.clone(), seed });
                }
            }
        }
    }
    eprintln!("{} attempts on {threads} threads", jobs.len());
    let next = AtomicUsize::new(0);
    let abandoned = AtomicUsize::new(0);
    // Finished lines by job index; printed in order as the prefix completes.
    let done: Mutex<(Vec<Option<String>>, usize)> = Mutex::new((vec![None; jobs.len()], 0));
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    let (tx, rx) = mpsc::channel();
                    let (area, seed, t, o) = (job.area.clone(), job.seed, job.topology, job.orientation);
                    let start = std::time::Instant::now();
                    std::thread::spawn(move || {
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let mut rng = TerrainRng::new(seed);
                            let forced = Forced { topology: Some(t), orientation: Some(o) };
                            poe_world::area::generate_with_files(gd, files, &area, &mut rng, Some(forced)).map(|g| (g.map.tile_hash(), g.draws))
                        }));
                        let _ = tx.send(match r {
                            Ok(Ok((h, d))) => ("ok", format!("hash {h} draws {d}")),
                            Ok(Err(e)) => ("error", e.to_string().replace(['\n', '\t'], " ")),
                            Err(_) => ("panic", String::new()),
                        });
                    });
                    let (status, detail) = rx.recv_timeout(timeout).unwrap_or_else(|_| {
                        abandoned.fetch_add(1, Ordering::Relaxed);
                        ("timeout", String::new())
                    });
                    let line = format!("{}\t{}\t{}\t{}\t{:.2}\t{status}\t{detail}\t{}", job.area, job.topology, job.orientation, job.graph, start.elapsed().as_secs_f32(), job.seed);
                    let mut guard = done.lock().unwrap();
                    let (lines, printed) = &mut *guard;
                    lines[i] = Some(line);
                    while *printed < lines.len() {
                        let Some(l) = lines[*printed].take() else { break };
                        println!("{l}");
                        *printed += 1;
                    }
                    drop(guard);
                    if abandoned.load(Ordering::Relaxed) >= 8 {
                        eprintln!("eight attempts hung; stopping");
                        std::process::exit(2);
                    }
                }
            });
        }
    });
    Ok(())
}
