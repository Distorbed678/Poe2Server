//! Compares the port's doodads with the client's `doodads.tsv` of DLL area exports (WP7.11):
//!
//! `cargo run --release -p poe-world --example check_doodads -- <export dir | run dir>...
//! [--only forced.tsv] [--threads N] [--kinds] [--verbose]`
//!
//! A run dir is a folder holding `exports/` (for example `dumps/<version>/terrain/batch/a1`); an
//! export dir holds `meta.txt` and `doodads.tsv`. Each export is regenerated with the port using
//! the forced topology and orientation of its `meta.txt`. `--only F` keeps only the exports whose
//! (area, seed, topology, orientation) are `match` rows of the tab-separated file F (for example
//! `runtime/forced-ab.tsv`): those are the ones whose tile map is right, so the doodads can be
//! checked. The port's doodads are compared as sets of `(x, y, path)` and their hash with the
//! export's `doodad_hash`. `--kinds` prints, per object type, how many generated doodads the client
//! counted and how many it did not. `--verbose` prints the first differences of each failing job.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Mutex, atomic::{AtomicUsize, Ordering}},
};

use poe_data::GameData;
use poe_world::{area::Forced, generate::CachedFiles, rng::TerrainRng};

struct Job {
    dir: PathBuf,
    area: String,
    seed: u32,
    topology: u32,
    orientation: u8,
    doodad_hash: u32,
    expected: Vec<(i32, i32, f32, String)>,
}

fn load(dir: &Path) -> Option<Job> {
    let meta = std::fs::read_to_string(dir.join("meta.txt")).ok()?;
    let get = |k: &str| meta.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('\t'))).map(str::to_string);
    let mut expected = Vec::new();
    for l in std::fs::read_to_string(dir.join("doodads.tsv")).ok()?.lines() {
        if l.starts_with('#') || l.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = l.split('\t').collect();
        expected.push((f[0].parse().ok()?, f[1].parse().ok()?, f[2].parse().ok()?, f[4].to_string()));
    }
    Some(Job {
        dir: dir.to_path_buf(),
        area: get("area")?,
        seed: get("seed")?.parse().ok()?,
        topology: get("topology")?.parse().ok()?,
        orientation: get("orientation")?.parse().ok()?,
        doodad_hash: get("doodad_hash")?.parse().ok()?,
        expected,
    })
}

/// The bounded draws `(n, result)` of the doodad pass in an export's `generator_trace.tsv`
/// (callers inside `0x141fe7520`, the choice and scale draws), when it has one.
fn client_draws(dir: &Path) -> Option<(Vec<(u32, u32)>, bool)> {
    let text = std::fs::read_to_string(dir.join("generator_trace.tsv")).ok()?;
    let lines: Vec<Vec<&str>> = text.lines().map(|l| l.split('\t').collect()).collect();
    let mut out = Vec::new();
    let mut in_doodads = false;
    for (i, f) in lines.iter().enumerate() {
        if f.len() > 3 && f[1] == "phase" {
            in_doodads |= f[3].starts_with("doodads ");
        }
        if f.len() > 3 && f[1] == "below" && in_doodads {
            let caller = u64::from_str_radix(f[2].split(',').next().unwrap_or(""), 16).unwrap_or(0);
            // The server-only scatter branch (out of scope) starts with its position draws.
            if (0x141ff6000..0x141ff7000).contains(&caller) {
                return Some((out, true));
            }
            if (0x141fe7520..0x141fe78d0).contains(&caller) {
                let n: u32 = f[3].parse().ok()?;
                let v = lines.get(i + 2).filter(|l| l.len() > 4 && l[1] == "uniform").and_then(|l| l[4].parse().ok()).unwrap_or(0);
                out.push((n, v));
            }
        }
    }
    Some((out, false))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut dirs = Vec::new();
    let (mut only, mut kinds, mut verbose) = (None::<String>, false, false);
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--only" => only = Some(args.next().ok_or("--only needs a file")?),
            "--threads" => threads = args.next().ok_or("--threads needs a number")?.parse()?,
            "--kinds" => kinds = true,
            "--verbose" => verbose = true,
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
    let mut jobs = Vec::new();
    for d in &dirs {
        let roots: Vec<PathBuf> = if d.join("meta.txt").exists() {
            vec![d.clone()]
        } else {
            let e = if d.join("exports").is_dir() { d.join("exports") } else { d.clone() };
            let mut v: Vec<PathBuf> = std::fs::read_dir(e)?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("meta.txt").exists()).collect();
            v.sort();
            v
        };
        for r in roots {
            if let Some(j) = load(&r) {
                if allowed.as_ref().is_none_or(|a| a.contains(&(j.area.clone(), j.seed, j.topology, j.orientation))) {
                    jobs.push(j);
                }
            }
        }
    }
    eprintln!("{} exports", jobs.len());
    let gd: &'static GameData = Box::leak(Box::new(GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?));
    let files: &'static CachedFiles<'static> = Box::leak(Box::new(CachedFiles::new(gd.files())));
    std::panic::set_hook(Box::new(|_| {}));
    let next = AtomicUsize::new(0);
    // (job index, status line)
    let out: Mutex<Vec<(usize, String, bool)>> = Mutex::new(Vec::new());
    let stats: Mutex<BTreeMap<String, (u32, u32, u32)>> = Mutex::new(BTreeMap::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(j) = jobs.get(i) else { break };
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut rng = TerrainRng::new(j.seed);
                    rng.log = Some(Vec::new());
                    let forced = Forced { topology: Some(j.topology), orientation: Some(j.orientation) };
                    poe_world::area::generate_with_files(gd, files, &j.area, &mut rng, Some(forced))
                }));
                let (line, ok) = match r {
                    Ok(Ok(g)) => {
                        let mut exp: HashMap<(i32, i32, String), u32> = HashMap::new();
                        for (x, y, _, p) in &j.expected {
                            *exp.entry((*x, *y, p.clone())).or_default() += 1;
                        }
                        let mut got: HashMap<(i32, i32, String), (u32, String)> = HashMap::new();
                        for d in &g.doodads {
                            got.entry((d.x, d.y, d.path.clone())).or_insert((0, d.kind.clone())).0 += 1;
                        }
                        {
                            let mut st = stats.lock().unwrap();
                            for (k, (n, kind)) in &got {
                                let e = st.entry(kind.clone()).or_default();
                                let c = exp.get(k).copied().unwrap_or(0).min(*n);
                                e.0 += c;
                                e.1 += n - c;
                            }
                        }
                        let counted: Vec<_> = g.doodads.iter().filter(|d| d.counted).collect();
                        let hash = g.doodad_hash();
                        let mut gmap: HashMap<(i32, i32, String), u32> = HashMap::new();
                        for d in &counted {
                            *gmap.entry((d.x, d.y, d.path.clone())).or_default() += 1;
                        }
                        let missing: Vec<_> = exp.iter().filter(|(k, n)| gmap.get(*k).copied().unwrap_or(0) < **n).map(|(k, _)| k.clone()).collect();
                        let extra: Vec<_> = gmap.iter().filter(|(k, n)| exp.get(*k).copied().unwrap_or(0) < **n).map(|(k, _)| k.clone()).collect();
                        let mut trace_note = String::new();
                        if let Some((client, truncated)) = client_draws(&j.dir) {
                            let mut ours: Vec<u32> = g.doodad_draws.iter().map(|d| d.0).collect();
                            let mut theirs: Vec<u32> = client.iter().map(|d| d.0).collect();
                            // The client's later draws may belong to the scatter branch (out of scope).
                            if truncated {
                                ours.truncate(theirs.len());
                            } else {
                                theirs.truncate(ours.len());
                            }
                            if ours != theirs {
                                let at = ours.iter().zip(&theirs).position(|(a, b)| a != b).unwrap_or(ours.len().min(theirs.len()));
                                trace_note = format!(" DRAWS differ at {at} (ours {} client {}; ours {:?} client {:?})", ours.len(), theirs.len(), ours.get(at), theirs.get(at));
                            } else if g.doodad_draws.iter().zip(&client).any(|(a, b)| a.1 != b.1) {
                                trace_note = " DRAW VALUES differ".into();
                            }
                        }
                        // Angles: every client angle has one of ours at its cell and path (mod 2π).
                        let mut ang: HashMap<(i32, i32, String), Vec<f32>> = HashMap::new();
                        for d in &counted {
                            ang.entry((d.x, d.y, d.path.clone())).or_default().push(d.angle);
                        }
                        let mut bad_angle = 0;
                        for (x, y, a, p) in &j.expected {
                            let near = |b: f32| {
                                let d = (a - b).rem_euclid(std::f32::consts::TAU);
                                d.min(std::f32::consts::TAU - d) < 0.01
                            };
                            if !ang.get(&(*x, *y, p.clone())).is_some_and(|v| v.iter().any(|&b| near(b))) {
                                bad_angle += 1;
                                if verbose && bad_angle <= 4 {
                                    trace_note += &format!("\n    angle at ({x}, {y}) client {a:.4} ours {:?}", ang.get(&(*x, *y, p.clone())));
                                }
                            }
                        }
                        if bad_angle > 0 {
                            trace_note += &format!(" ANGLES differ: {bad_angle}");
                        }
                        let ok = missing.is_empty() && extra.is_empty() && hash == j.doodad_hash && trace_note.is_empty();
                        let mut l = format!("{} {} doodads {} (client {}) missing {} extra {} hash {}", if ok { "ok  " } else { "FAIL" }, j.dir.file_name().unwrap().to_string_lossy(), counted.len(), j.expected.len(), missing.len(), extra.len(), if hash == j.doodad_hash { "match" } else { "differs" }) + &trace_note;
                        if verbose && !ok {
                            for m in missing.iter().take(3) {
                                l += &format!("\n    missing {m:?}");
                            }
                            for m in extra.iter().take(3) {
                                l += &format!("\n    extra {m:?} kind {} (ours {} client {})", got.get(m).map(|g| g.1.as_str()).unwrap_or("?"), gmap[m], exp.get(m).copied().unwrap_or(0));
                            }
                        }
                        (l, ok)
                    }
                    Ok(Err(e)) => (format!("ERR  {} {e}", j.dir.file_name().unwrap().to_string_lossy()), false),
                    Err(_) => (format!("PANIC {}", j.dir.file_name().unwrap().to_string_lossy()), false),
                };
                out.lock().unwrap().push((i, line, ok));
            });
        }
    });
    let mut out = out.into_inner().unwrap();
    out.sort();
    let ok = out.iter().filter(|o| o.2).count();
    for (_, l, ok) in &out {
        if !ok || verbose {
            println!("{l}");
        }
    }
    println!("doodads match in {ok} of {} exports", out.len());
    if kinds {
        for (k, (a, b, _)) in stats.into_inner().unwrap() {
            println!("kind {k}\tclient-counted {a}\tnot {b}");
        }
    }
    Ok(())
}
