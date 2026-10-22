//! Generates a client export's job with the port and compares the layouts cell by cell.
//!
//! `cargo run --release -p poe-world --example layout_diff -- <export dir | run dir> [options]`
//!
//! - An export dir (has `meta.txt`, `layout.tsv`; e.g. `dumps/<v>/terrain/batch/fails1/exports/
//!   Abyss_Boss1_1280953652_1_0`): detail mode. The port generates the job named by `meta.txt`
//!   (area, seed, forced topology 1-based and orientation, as `forced_verify` does) and the
//!   layouts are compared: map size, number of differing cells, the first N differing cells in
//!   generation order (the port's piece index; cells the port left empty come last) with the
//!   tile `.tdt`, orientation, sub-tile and height on both sides, and the port's piece that
//!   covers the cell (corner, size). The port has no region/room records at this level; the
//!   piece index orders the pass.
//! - A run dir (`.../batch/fails1`, its `exports/` or any dir holding export dirs): one summary
//!   line per job (all exports, parallel).
//! - `--job NAME`: with a run dir, detail mode for that export (`<area>_<seed>_<topo>_<orient>`;
//!   a substring selects the first match).
//! - `--first N` (default 10): differing cells listed.
//! - `--map FILE`: write the diff map as text (`.` same, `#` differs, `+` only on the client's
//!   side outside the port's map, ` ` empty on both) to FILE; `--ppm FILE` writes it as a PPM
//!   image (green same, red differs, grey empty). Default dir for `--map auto`/`--ppm auto` is
//!   `runtime/layout_diff/<job>.{txt,ppm}`.
//! - `--threads N` (summary mode, default all cores); `POE_FORCED_BUDGET=<s>` (default 120)
//!   per-job time budget in summary mode.
//! - `--filter SUBSTR` (summary mode): only jobs whose name contains SUBSTR.
//!
//! Compared per cell: tile path (case-insensitive), sub-tile index, orientation, height and the
//! cell's offset in its tile (`skip` columns).

use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use poe_data::GameData;
use poe_world::{
    area::Forced,
    export::{AreaExport, ExportedTile},
    generate::{CachedFiles, TileMap},
    rng::TerrainRng,
};

struct Job {
    name: String,
    dir: PathBuf,
    area: String,
    seed: u32,
    topology: u32,
    orientation: u8,
}

fn export_dirs(root: &Path, out: &mut Vec<PathBuf>) {
    if root.join("meta.txt").is_file() {
        out.push(root.to_path_buf());
        return;
    }
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        if e.path().is_dir() {
            export_dirs(&e.path(), out);
        }
    }
}

fn job_of(dir: &Path, ex: &AreaExport) -> Result<Job, String> {
    let m = |k: &str| ex.meta(k).ok_or_else(|| format!("{}: no `{k}` in meta.txt", dir.display()));
    let num = |k: &str| m(k)?.parse::<u32>().map_err(|_| format!("{}: bad `{k}`", dir.display()));
    Ok(Job {
        name: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        dir: dir.to_path_buf(),
        area: m("area")?.to_string(),
        seed: num("seed")?,
        topology: num("topology")?,
        orientation: num("orientation")? as u8,
    })
}

enum Ours {
    Done(Box<poe_world::area::Generated>),
    Error(String),
    Panic,
    Timeout(u64),
}

fn generate(gd: &'static GameData, files: &'static CachedFiles<'static>, j: &Job, budget: u64) -> Ours {
    let (area, seed, topology, orientation) = (j.area.clone(), j.seed, j.topology, j.orientation);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut rng = TerrainRng::new(seed);
            let forced = Forced { topology: Some(topology), orientation: Some(orientation) };
            poe_world::area::generate_with_files(gd, files, &area, &mut rng, Some(forced))
        }));
        let _ = tx.send(match r {
            Ok(Ok(g)) => Ours::Done(Box::new(g)),
            Ok(Err(e)) => Ours::Error(e.to_string().replace(['\n', '\t'], " ")),
            Err(_) => Ours::Panic,
        });
    });
    rx.recv_timeout(Duration::from_secs(budget)).unwrap_or(Ours::Timeout(budget))
}

/// One cell, both sides.
struct Diff {
    x: u32,
    y: u32,
    /// Port piece index covering the cell (generation order), `usize::MAX` for none.
    order: usize,
    ours: String,
    theirs: String,
}

fn fmt_theirs(t: &ExportedTile) -> String {
    if t.path.is_empty() {
        return "(empty)".into();
    }
    format!("{} o{} s{} h{} off {},{}", t.path, t.orientation, t.sub, t.height, t.skip.0, t.skip.1)
}

fn fmt_ours(map: &TileMap, x: u32, y: u32) -> String {
    match map.get(x, y) {
        Some(c) if !c.path.is_empty() => format!("{} o{} s{} h{} off {},{}", c.path, c.orientation, c.sub, c.height, c.offset.0, c.offset.1),
        _ => "(empty)".into(),
    }
}

/// Differing cells (unsorted) of the common window; `grid` gets one char per client cell.
fn diff_cells(map: &TileMap, ex: &AreaExport, grid: &mut Vec<u8>) -> Vec<Diff> {
    let mut out = Vec::new();
    grid.clear();
    grid.resize((ex.width * ex.height) as usize, b' ');
    for y in 0..ex.height {
        for x in 0..ex.width {
            let Some(t) = ex.tile(x, y) else { continue };
            let idx = (y * ex.width + x) as usize;
            let ours = if x < map.width && y < map.height { map.get(x, y) } else { None };
            let (same, empty_both) = match (ours.filter(|c| !c.path.is_empty()), t.path.is_empty()) {
                (None, true) => (true, true),
                (Some(c), false) => (
                    c.path.eq_ignore_ascii_case(&t.path)
                        && c.sub == t.sub
                        && c.orientation == t.orientation
                        && c.height == t.height
                        && c.offset == t.skip,
                    false,
                ),
                _ => (false, false),
            };
            grid[idx] = if empty_both { b' ' } else if same { b'.' } else if x >= map.width || y >= map.height { b'+' } else { b'#' };
            if !same {
                let order = if x < map.width && y < map.height {
                    map.index(x as i32, y as i32).and_then(|i| map.piece_at.get(i).copied().flatten()).unwrap_or(usize::MAX)
                } else {
                    usize::MAX
                };
                out.push(Diff { x, y, order, ours: if x < map.width && y < map.height { fmt_ours(map, x, y) } else { "(outside map)".into() }, theirs: fmt_theirs(t) });
            }
        }
    }
    out
}

fn write_outputs(job: &Job, grid: &[u8], ex: &AreaExport, map_out: &Option<String>, ppm_out: &Option<String>) -> std::io::Result<()> {
    let target = |opt: &Option<String>, ext: &str| {
        opt.as_ref().map(|p| if p == "auto" { PathBuf::from(format!("runtime/layout_diff/{}.{ext}", job.name)) } else { PathBuf::from(p) })
    };
    if let Some(p) = target(map_out, "txt") {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut s = String::new();
        for row in grid.chunks(ex.width as usize) {
            s.push_str(&String::from_utf8_lossy(row));
            s.push('\n');
        }
        std::fs::write(&p, s)?;
        println!("diff map: {}", p.display());
    }
    if let Some(p) = target(ppm_out, "ppm") {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut bytes = format!("P6\n{} {}\n255\n", ex.width, ex.height).into_bytes();
        for &g in grid {
            bytes.extend_from_slice(match g {
                b'.' => &[60, 170, 60],
                b'#' => &[220, 40, 40],
                b'+' => &[230, 150, 30],
                _ => &[60, 60, 60],
            });
        }
        std::fs::write(&p, bytes)?;
        println!("diff image: {}", p.display());
    }
    Ok(())
}

fn bbox(d: &[Diff]) -> String {
    let (x0, x1) = (d.iter().map(|c| c.x).min().unwrap_or(0), d.iter().map(|c| c.x).max().unwrap_or(0));
    let (y0, y1) = (d.iter().map(|c| c.y).min().unwrap_or(0), d.iter().map(|c| c.y).max().unwrap_or(0));
    format!("x {x0}..{x1} y {y0}..{y1}")
}

fn detail(gd: &'static GameData, files: &'static CachedFiles<'static>, j: &Job, first: usize, map_out: &Option<String>, ppm_out: &Option<String>, budget: u64) -> Result<(), Box<dyn std::error::Error>> {
    let ex = AreaExport::load(&j.dir)?;
    println!("{}: area {} seed {} topology {} orientation {}, client map {}x{}, tile hash {}", j.name, j.area, j.seed, j.topology, j.orientation, ex.width, ex.height, ex.meta("tile_hash").unwrap_or("?"));
    let g = match generate(gd, files, j, budget) {
        Ours::Done(g) => g,
        Ours::Error(e) => return Ok(println!("port error: {e}")),
        Ours::Panic => return Ok(println!("port panic")),
        Ours::Timeout(s) => return Ok(println!("port timeout after {s}s")),
    };
    let map = &g.map;
    println!("port: graph {}, map {}x{}, {} pieces, tile hash {}", g.graph, map.width, map.height, map.pieces.len(), map.tile_hash());
    if (map.width, map.height) != (ex.width, ex.height) {
        println!("MAP SIZE differs: port {}x{} vs client {}x{} (cells compared in the common window)", map.width, map.height, ex.width, ex.height);
    }
    let mut grid = Vec::new();
    let mut diffs = diff_cells(map, &ex, &mut grid);
    let total = (ex.width * ex.height) as usize;
    println!("cells: {} of {total} differ{}", diffs.len(), if diffs.is_empty() { String::new() } else { format!(", bounding box {}", bbox(&diffs)) });
    diffs.sort_by_key(|d| (d.order, d.y, d.x));
    for d in diffs.iter().take(first) {
        let piece = map.pieces.get(d.order).map(|p| format!("port piece #{} at {},{} size {}x{} o{} {}", d.order, p.x, p.y, p.size.0, p.size.1, p.orientation, p.path)).unwrap_or_else(|| "no port piece".into());
        println!("  cell {},{}: {piece}\n      ours   {}\n      client {}", d.x, d.y, d.ours, d.theirs);
    }
    write_outputs(j, &grid, &ex, map_out, ppm_out)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut positional, mut job_sel, mut filter) = (Vec::new(), None, None);
    let (mut first, mut map_out, mut ppm_out) = (10usize, None, None);
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |what: &str| args.next().ok_or(format!("{what} needs a value"));
        match a.as_str() {
            "--job" => job_sel = Some(val("--job")?),
            "--filter" => filter = Some(val("--filter")?),
            "--first" => first = val("--first")?.parse()?,
            "--map" => map_out = Some(val("--map")?),
            "--ppm" => ppm_out = Some(val("--ppm")?),
            "--threads" => threads = val("--threads")?.parse()?,
            _ => positional.push(a),
        }
    }
    let root = PathBuf::from(positional.first().ok_or("usage: layout_diff <export dir | run dir> [--job NAME] [--first N] [--map FILE|auto] [--ppm FILE|auto] [--filter S] [--threads N]")?);
    let budget: u64 = std::env::var("POE_FORCED_BUDGET").ok().map(|s| s.parse()).transpose()?.unwrap_or(120);
    let mut dirs = Vec::new();
    export_dirs(&root, &mut dirs);
    dirs.sort();
    if dirs.is_empty() {
        return Err(format!("no export dirs (meta.txt) under {}", root.display()).into());
    }
    let gd: &'static GameData = Box::leak(Box::new(GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?));
    let files: &'static CachedFiles<'static> = Box::leak(Box::new(CachedFiles::new(gd.files())));
    std::panic::set_hook(Box::new(|_| {}));

    let single = dirs.len() == 1 && root.join("meta.txt").is_file();
    if single || job_sel.is_some() {
        let dir = match &job_sel {
            Some(s) => dirs.iter().find(|d| d.file_name().is_some_and(|n| n.to_string_lossy() == s.as_str())).or_else(|| dirs.iter().find(|d| d.to_string_lossy().contains(s.as_str()))).ok_or("no such job")?.clone(),
            None => dirs[0].clone(),
        };
        let ex = AreaExport::load(&dir)?;
        return detail(gd, files, &job_of(&dir, &ex)?, first, &map_out, &ppm_out, budget);
    }

    // Summary mode: one line per job.
    let jobs: Vec<Job> = dirs
        .iter()
        .filter(|d| filter.as_ref().is_none_or(|f| d.to_string_lossy().contains(f.as_str())))
        .filter_map(|d| match AreaExport::load(d).map_err(|e| e.to_string()).and_then(|ex| job_of(d, &ex)) {
            Ok(j) => Some(j),
            Err(e) => {
                println!("{}\tunreadable: {e}", d.display());
                None
            }
        })
        .collect();
    let next = AtomicUsize::new(0);
    let lines: Mutex<Vec<String>> = Mutex::new(vec![String::new(); jobs.len()]);
    let (matched, total) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(j) = jobs.get(i) else { break };
                    let line = match (AreaExport::load(&j.dir), generate(gd, files, j, budget)) {
                        (Err(e), _) => format!("unreadable: {e}"),
                        (_, Ours::Error(e)) => format!("error: {e}"),
                        (_, Ours::Panic) => "panic".into(),
                        (_, Ours::Timeout(s)) => format!("timeout after {s}s"),
                        (Ok(ex), Ours::Done(g)) => {
                            let mut grid = Vec::new();
                            let mut d = diff_cells(&g.map, &ex, &mut grid);
                            let size = if (g.map.width, g.map.height) == (ex.width, ex.height) { format!("{}x{}", ex.width, ex.height) } else { format!("SIZE {}x{} vs {}x{}", g.map.width, g.map.height, ex.width, ex.height) };
                            if d.is_empty() {
                                matched.fetch_add(1, Ordering::Relaxed);
                                format!("match\t{size}")
                            } else {
                                d.sort_by_key(|c| (c.order, c.y, c.x));
                                let f = &d[0];
                                format!("diff\t{size}\t{} of {} cells\t{}\tfirst {},{} ours [{}] client [{}]", d.len(), ex.width * ex.height, bbox(&d), f.x, f.y, f.ours, f.theirs)
                            }
                        }
                    };
                    total.fetch_add(1, Ordering::Relaxed);
                    lines.lock().unwrap()[i] = line;
                }
            });
        }
    });
    for (j, l) in jobs.iter().zip(lines.into_inner().unwrap()) {
        println!("{}\t{l}", j.name);
    }
    eprintln!("{} of {} jobs identical", matched.load(Ordering::Relaxed), total.load(Ordering::Relaxed));
    Ok(())
}
