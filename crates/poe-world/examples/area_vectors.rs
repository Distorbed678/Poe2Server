//! Checks the generator against tile-hash vectors (`area level seed tile_hash doodad_hash build`,
//! from `scripts/re/clientlog_terrain.py`):
//!
//! `cargo run --release -p poe-world --example area_vectors -- <vectors.tsv> [build] [area]
//! [--natural] [--threads N] [--hints FILE] [--probe FILE] [--runnable-only]`
//!
//! - `build`: `all` (default), one build number, or `>=N` (builds at least N). The vectors of
//!   older builds are valid for matches (the hash is of the generated output) but a mismatch
//!   there is inconclusive.
//! - `area`: only that area.
//! - `--natural`: only the natural pick (`generate`), no forced search. A vector that does not
//!   match is `differ`/`error`/`panic` with no known combination.
//! - `--threads N` (default: all cores): the vectors of different (area, seed) pairs run on
//!   threads; with fewer pairs than threads the forced search of one pair runs on the spare
//!   threads.
//! - `--hints <area-vectors-results.txt>`: a previous result file; combinations that matched the
//!   same (area, seed) before are tried first, then those that matched the area. A rerun then
//!   finds its known matches at once (the regression check of a generator change).
//! - `--probe <probe-results.tsv>` (`area_probe` output): topologies whose probe ran (`ok`) are
//!   tried before the rest; with `--runnable-only` the rest are not tried at all (they cannot
//!   match unless the error depended on the seed or orientation, so a vector that matches only
//!   there is missed: use it for bulk runs).
//! - `POE_VECTOR_BUDGET=<seconds>`: time budget per (area, seed) pair for the forced search
//!   (checked between attempts; a single generate is not interrupted). A pair out of budget
//!   prints `skipped (budget)` for its unresolved vectors.
//!
//! Vectors with the same area and seed share one search (every layout is generated once and
//! compared with all their hashes), so a file with several builds per seed costs little more.
//! Prints one line per vector in input order and a summary per area. When the natural topology
//! choice (world-map pins, not ported) does not match, every forced topology and orientation is
//! tried: a match there is what the server reproduces by sending `map_force_topology` /
//! `map_force_orientation`.
//!
//! Result lines (`cargo xtask terrain-matrix` parses them; keep the format):
//! `area  seed  match (natural T:O)`, `area  seed  match (forced T:O)` (T the 1-based topology
//! index, O the orientation), `area  seed  differ  ...`, `area  seed  error  ...`,
//! `area  seed  panic`, `area  seed  skipped (budget)`, each followed by `build=N` (the vector's
//! client build, so the matrix can tell matches from older builds). Keep the output in
//! `dumps/<version>/terrain/vectors/area-vectors-results.txt`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use poe_data::GameData;
use poe_world::{area::Forced, generate::CachedFiles, rng::TerrainRng};

/// The `[build]` argument.
enum BuildFilter {
    All,
    Exact(String),
    AtLeast(u64),
}

impl BuildFilter {
    fn parse(s: Option<&str>) -> Result<Self, String> {
        Ok(match s {
            None | Some("all") | Some("") => BuildFilter::All,
            Some(s) => match s.strip_prefix(">=") {
                Some(n) => BuildFilter::AtLeast(n.trim().parse().map_err(|_| format!("bad build filter `{s}`"))?),
                None => BuildFilter::Exact(s.to_string()),
            },
        })
    }

    fn accepts(&self, build: &str) -> bool {
        match self {
            BuildFilter::All => true,
            BuildFilter::Exact(b) => b == build,
            BuildFilter::AtLeast(n) => build.parse::<u64>().is_ok_and(|b| b >= *n),
        }
    }
}

/// How one (area, seed) pair came out for one wanted hash.
#[derive(Clone)]
enum Outcome {
    Match(String),
    Differ { ours: String, graph: String },
    Error(String),
    Panic,
    Skipped,
}

impl Outcome {
    fn line(&self, area: &str, seed: u32) -> String {
        match self {
            Outcome::Match(how) => format!("{area}\t{seed}\tmatch{how}"),
            Outcome::Differ { ours, graph } => format!("{area}\t{seed}\tdiffer\tours {ours} client {{want}}\t{graph}"),
            Outcome::Error(e) => format!("{area}\t{seed}\terror\t{e}"),
            Outcome::Panic => format!("{area}\t{seed}\tpanic"),
            Outcome::Skipped => format!("{area}\t{seed}\tskipped (budget)"),
        }
    }
}

/// What orders the forced search (see the module comment).
#[derive(Default)]
struct Priors {
    /// `(area, seed)` -> combinations that matched it before.
    exact: HashMap<(String, u32), Vec<(u32, u8)>>,
    /// `area` -> combinations that matched any of its seeds before.
    area: HashMap<String, HashSet<(u32, u8)>>,
    /// `(area, topology)` whose probe attempts ran.
    runnable: Option<HashSet<(String, u32)>>,
    runnable_only: bool,
}

impl Priors {
    fn load_hints(&mut self, text: &str) {
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let (Some(area), Some(seed), Some(how)) = (f.first(), f.get(1).and_then(|s| s.parse::<u32>().ok()), f.get(2)) else { continue };
            let Some(combo) = how.strip_prefix("match (").and_then(|r| r.split_once(' ')).and_then(|(_, c)| c.trim_end_matches(')').split_once(':')) else { continue };
            let (Ok(t), Ok(o)) = (combo.0.parse::<u32>(), combo.1.parse::<u8>()) else { continue };
            self.exact.entry((area.to_string(), seed)).or_default().push((t, o));
            self.area.entry(area.to_string()).or_default().insert((t, o));
        }
    }

    fn load_probe(&mut self, text: &str) {
        let set = self.runnable.get_or_insert_with(HashSet::new);
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() >= 6
                && f[5] == "ok"
                && let Ok(t) = f[1].parse()
            {
                set.insert((f[0].to_string(), t));
            }
        }
    }

    /// Search tier of a combination (lower first); `None` = not tried.
    fn tier(&self, area: &str, seed: u32, c: (u32, u8)) -> Option<u8> {
        if self.exact.get(&(area.to_string(), seed)).is_some_and(|v| v.contains(&c)) {
            return Some(0);
        }
        if self.area.get(area).is_some_and(|v| v.contains(&c)) {
            return Some(1);
        }
        match &self.runnable {
            Some(r) if r.contains(&(area.to_string(), c.0)) => Some(2),
            Some(_) if self.runnable_only => None,
            _ => Some(3),
        }
    }
}

type Natural = std::thread::Result<Result<poe_world::area::Generated, poe_world::generate::GenError>>;

/// Tries every topology and orientation (in index order) for the hashes in `wants` that are
/// still unresolved; returns the combination `T:O` that reproduced each, `None` where none did.
/// `complete` is false when the budget ran out first.
#[allow(clippy::too_many_arguments)]
fn forced_search(gd: &GameData, files: &CachedFiles, area: &str, seed: u32, wants: &[&str], deadline: Option<Instant>, threads: usize, priors: &Priors) -> (Vec<Option<String>>, bool) {
    let Ok(entries) = poe_world::area::topology_entries(gd, area, 1) else { return (vec![None; wants.len()], true) };
    let mut combos: Vec<(u32, u8)> = Vec::new();
    for (t, (_, mask)) in (1..).zip(&entries) {
        combos.extend((0..8u8).filter(|o| mask & (1 << o) != 0).map(|o| (t, o)));
    }
    // Hinted and runnable combinations first (stable: index order within a tier).
    let mut tiered: Vec<(u8, (u32, u8))> = combos.iter().filter_map(|&c| priors.tier(area, seed, c).map(|t| (t, c))).collect();
    tiered.sort_by_key(|(t, _)| *t);
    let combos: Vec<(u32, u8)> = tiered.into_iter().map(|(_, c)| c).collect();
    let next = AtomicUsize::new(0);
    // Per want: the lowest combination index that matched.
    let found: Mutex<Vec<Option<usize>>> = Mutex::new(vec![None; wants.len()]);
    let timed_out = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= combos.len() {
                        break;
                    }
                    {
                        // Everything resolved and nothing earlier can still win: stop.
                        let f = found.lock().unwrap();
                        if f.iter().all(Option::is_some) && f.iter().flatten().all(|&j| i >= j) {
                            break;
                        }
                    }
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        timed_out.store(true, Ordering::Relaxed);
                        break;
                    }
                    let (t, o) = combos[i];
                    let forced = Forced { topology: Some(t), orientation: Some(o) };
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut rng = TerrainRng::new(seed);
                        poe_world::area::generate_with_files(gd, files, area, &mut rng, Some(forced))
                    }));
                    if let Ok(Ok(g)) = r {
                        let h = g.map.tile_hash().to_string();
                        let mut f = found.lock().unwrap();
                        for (w, slot) in wants.iter().zip(f.iter_mut()) {
                            if *w == h && slot.is_none_or(|j| i < j) {
                                *slot = Some(i);
                            }
                        }
                    }
                }
            });
        }
    });
    let found = found.into_inner().unwrap();
    let unresolved = found.iter().any(Option::is_none);
    let complete = !(timed_out.load(Ordering::Relaxed) && unresolved);
    (found.into_iter().map(|j| j.map(|j| format!("{}:{}", combos[j].0, combos[j].1))).collect(), complete)
}

/// One (area, seed) pair against all its wanted hashes.
struct Settings<'a> {
    natural_only: bool,
    budget: Option<Duration>,
    /// Threads for the forced search of one pair.
    threads: usize,
    priors: &'a Priors,
    files: &'a CachedFiles<'a>,
}

fn check_pair(gd: &GameData, area: &str, seed: u32, wants: &[&str], cfg: &Settings) -> Vec<Outcome> {
    let Settings { natural_only, budget, threads, priors, files } = *cfg;
    let natural: Natural = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poe_world::area::generate_with_files(gd, files, area, &mut TerrainRng::new(seed), None)));
    let natural_hash = match &natural {
        Ok(Ok(g)) => Some(g.map.tile_hash().to_string()),
        _ => None,
    };
    let mut out: Vec<Option<Outcome>> = wants
        .iter()
        .map(|w| {
            let (Some(h), Ok(Ok(g))) = (&natural_hash, &natural) else { return None };
            if h != w {
                return None;
            }
            let entries = poe_world::area::topology_entries(gd, area, 1).unwrap_or_default();
            let natural = entries.iter().position(|(p, _)| *p == g.graph).map(|i| {
                let mask = entries[i].1;
                let o = if mask == 0 || mask == 0xff { 0 } else { mask.trailing_zeros() };
                format!(" (natural {}:{o})", i + 1)
            });
            Some(Outcome::Match(natural.unwrap_or_default()))
        })
        .collect();
    let mut skipped = false;
    let pending: Vec<usize> = (0..wants.len()).filter(|&i| out[i].is_none()).collect();
    if !natural_only && !pending.is_empty() {
        let deadline = budget.map(|b| Instant::now() + b);
        let ws: Vec<&str> = pending.iter().map(|&i| wants[i]).collect();
        let (combos, complete) = forced_search(gd, files, area, seed, &ws, deadline, threads, priors);
        for (&i, c) in pending.iter().zip(combos) {
            if let Some(c) = c {
                out[i] = Some(Outcome::Match(format!(" (forced {c})")));
            }
        }
        skipped = !complete;
    }
    out.into_iter()
        .map(|o| {
            o.unwrap_or_else(|| {
                if skipped {
                    return Outcome::Skipped;
                }
                match &natural {
                    Ok(Ok(g)) => Outcome::Differ { ours: g.map.tile_hash().to_string(), graph: g.graph.clone() },
                    Ok(Err(e)) => Outcome::Error(e.to_string()),
                    Err(_) => Outcome::Panic,
                }
            })
        })
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut positional = Vec::new();
    let mut natural_only = false;
    let mut priors = Priors::default();
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--natural" => natural_only = true,
            "--threads" => threads = args.next().ok_or("--threads needs a number")?.parse()?,
            "--hints" => priors.load_hints(&std::fs::read_to_string(args.next().ok_or("--hints needs a file")?)?),
            "--probe" => priors.load_probe(&std::fs::read_to_string(args.next().ok_or("--probe needs a file")?)?),
            "--runnable-only" => priors.runnable_only = true,
            _ => positional.push(a),
        }
    }
    let usage = "usage: area_vectors <vectors.tsv> [all|build|>=N] [area] [--natural] [--threads N] [--hints FILE] [--probe FILE] [--runnable-only]";
    let path = positional.first().ok_or(usage)?;
    let build = BuildFilter::parse(positional.get(1).map(String::as_str))?;
    let area_filter = positional.get(2).filter(|a| !a.is_empty());
    let budget = std::env::var("POE_VECTOR_BUDGET").ok().map(|s| s.parse::<u64>().map(Duration::from_secs)).transpose()?;
    let gd = GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let files = CachedFiles::new(gd.files());
    std::panic::set_hook(Box::new(|_| {}));

    // The vectors in input order; pairs share one search.
    let mut vectors: Vec<(String, u32, String, String)> = Vec::new();
    for line in std::fs::read_to_string(path)?.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 6 || !build.accepts(f[5]) || area_filter.is_some_and(|a| a != f[0]) {
            continue;
        }
        vectors.push((f[0].to_string(), f[2].parse()?, f[3].to_string(), f[5].to_string()));
    }
    let mut pairs: BTreeMap<(String, u32), Vec<usize>> = BTreeMap::new();
    for (i, (area, seed, _, _)) in vectors.iter().enumerate() {
        pairs.entry((area.clone(), *seed)).or_default().push(i);
    }
    let pairs: Vec<((String, u32), Vec<usize>)> = pairs.into_iter().collect();
    let outer = threads.min(pairs.len()).max(1);
    let inner = (threads / outer).max(1);
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Outcome>>> = Mutex::new(vec![None; vectors.len()]);
    std::thread::scope(|s| {
        for _ in 0..outer {
            s.spawn(|| {
                loop {
                    let p = next.fetch_add(1, Ordering::Relaxed);
                    let Some(((area, seed), idx)) = pairs.get(p) else { break };
                    let wants: Vec<&str> = idx.iter().map(|&i| vectors[i].2.as_str()).collect();
                    let outcomes = check_pair(&gd, area, *seed, &wants, &Settings { natural_only, budget, threads: inner, priors: &priors, files: &files });
                    let mut r = results.lock().unwrap();
                    for (&i, o) in idx.iter().zip(outcomes) {
                        r[i] = Some(o);
                    }
                    eprintln!("[{}/{}] {area} {seed}", p + 1, pairs.len());
                }
            });
        }
    });
    let results = results.into_inner().unwrap();
    // (match, differ, error, skipped, last error)
    let mut per_area: BTreeMap<String, (u32, u32, u32, u32, String)> = BTreeMap::new();
    for ((area, seed, want, build), r) in vectors.iter().zip(&results) {
        let r = r.clone().unwrap_or(Outcome::Skipped);
        let e = per_area.entry(area.clone()).or_default();
        match &r {
            Outcome::Match(_) => e.0 += 1,
            Outcome::Differ { .. } => e.1 += 1,
            Outcome::Error(m) => {
                e.2 += 1;
                e.4 = m.clone();
            }
            Outcome::Panic => {
                e.2 += 1;
                e.4 = "panic".into();
            }
            Outcome::Skipped => e.3 += 1,
        }
        println!("{}\tbuild={build}", r.line(area, *seed).replace("{want}", want));
    }
    println!("--- per area (match, differ, error, skipped, last error)");
    for (a, (m, d, e, sk, last)) in &per_area {
        println!("{a}\t{m}\t{d}\t{e}\t{sk}\t{last}");
    }
    Ok(())
}
