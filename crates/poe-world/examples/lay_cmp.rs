//! Compares the port's layout with a client export (agent A's cell-by-cell tool).
//!
//! `lay_cmp <export dir>... [--map] [--n N]`: each dir is `.../exports/<area>_<seed>_<topology>_<orientation>/`
//! (layout.tsv + meta.txt, written by the client batch for jobs in export.txt). The port generates
//! the same forced job; the output per job is the size, the number of differing cells (path, height
//! or orientation of a hashed cell: the tile corner, offset (0,0)) and the clusters of differences
//! (4-connected, with bounding box and the first cell's ours/client). `--map` prints a character map
//! (`.` equal, `X` different, `-` both empty). `--n N` lists N clusters (default 4).

use std::path::Path;

use poe_data::GameData;
use poe_world::{area::Forced, export::AreaExport, generate::CachedFiles, rng::TerrainRng};

fn short(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut dirs = Vec::new();
    let (mut map, mut n) = (false, 4usize);
    let mut at: Option<(i32, i32, i32)> = None;
    let mut margin: Option<i32> = None;
    let mut dy = 0i32;
    let mut lat: Option<Vec<i32>> = None;
    let mut units: Option<(i32, i32, i32)> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--map" => map = true,
            "--at" => {
                let v: Vec<i32> = args.next().ok_or("--at x,y,r")?.split(',').map(|v| v.parse().unwrap_or(0)).collect();
                at = Some((v[0], v[1], v[2]));
            }
            "--units" => {
                let v: Vec<i32> = args.next().ok_or("--units margin,step,size")?.split(',').map(|v| v.parse().unwrap_or(0)).collect();
                units = Some((v[0], v[1], v[2]));
            }
            "--lat" => lat = Some(args.next().ok_or("--lat")?.split(',').map(|v| v.parse().unwrap_or(0)).collect()),
            "--dy" => dy = args.next().ok_or("--dy")?.parse()?,
            "--margin" => margin = Some(args.next().ok_or("--margin M")?.parse()?),
            "--n" => n = args.next().ok_or("--n needs a number")?.parse()?,
            _ => dirs.push(a),
        }
    }
    let gd: &'static GameData = Box::leak(Box::new(GameData::open(
        Path::new("GameFiles"),
        Path::new("third_party/dat-schema/schema-poe2.min.json"),
    )?));
    let files = CachedFiles::new(gd.files());
    for dir in dirs {
        let ex = AreaExport::load(Path::new(&dir))?;
        let m = |k: &str| ex.meta(k).unwrap_or("").to_string();
        let (area, seed, topo, orient) = (m("area"), m("seed").parse::<u32>()?, m("topology").parse::<u32>()?, m("orientation").parse::<u8>()?);
        println!("== {area} seed {seed} topology {topo} orientation {orient}: client {}x{} tile hash {}", ex.width, ex.height, m("tile_hash"));
        let mut rng = TerrainRng::new(seed);
        let forced = Forced { topology: Some(topo), orientation: Some(orient) };
        let g = match poe_world::area::generate_with_files(gd, &files, &area, &mut rng, Some(forced)) {
            Ok(g) => g,
            Err(e) => {
                println!("  port error: {e}");
                continue;
            }
        };
        let tm = &g.map;
        println!("  graph {}  port {}x{} hash {} draws {}", g.graph, tm.width, tm.height, tm.tile_hash(), g.draws);
        let (w, h) = (ex.width.max(tm.width), ex.height.max(tm.height));
        let mut diff = vec![false; (w * h) as usize];
        let mut total = 0;
        let desc_c = |x: u32, y: u32| {
            if x >= ex.width || y >= ex.height {
                return "-".to_string();
            }
            let t = ex.tile(x, y).unwrap();
            if t.path.is_empty() {
                "-".to_string()
            } else {
                format!("{} s{} h{} o{} skip{},{}", short(&t.path), t.sub, t.height, t.orientation, t.skip.0, t.skip.1)
            }
        };
        let desc_o = |x: u32, y: u32| match tm.get(x, y) {
            Some(c) if x < tm.width && y < tm.height => format!("{} s{} h{} o{} skip{},{}", short(&c.path), c.sub, c.height, c.orientation, c.offset.0, c.offset.1),
            _ => "-".to_string(),
        };
        for y in 0..h {
            for x in 0..w {
                if desc_c(x, y).to_ascii_lowercase() != desc_o(x, y).to_ascii_lowercase() {
                    diff[(y * w + x) as usize] = true;
                    total += 1;
                }
            }
        }
        println!("  differing cells: {total} of {}", w * h);
        // Clusters.
        let mut seen = vec![false; diff.len()];
        let mut clusters = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if !diff[i] || seen[i] {
                    continue;
                }
                let mut stack = vec![(x, y)];
                seen[i] = true;
                let (mut x0, mut y0, mut x1, mut y1, mut cnt) = (x, y, x, y, 0);
                while let Some((cx, cy)) = stack.pop() {
                    cnt += 1;
                    x0 = x0.min(cx);
                    x1 = x1.max(cx);
                    y0 = y0.min(cy);
                    y1 = y1.max(cy);
                    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                        let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                            continue;
                        }
                        let j = (ny as u32 * w + nx as u32) as usize;
                        if diff[j] && !seen[j] {
                            seen[j] = true;
                            stack.push((nx as u32, ny as u32));
                        }
                    }
                }
                clusters.push((cnt, x0, y0, x1, y1, x, y));
            }
        }
        println!("  clusters: {}", clusters.len());
        for (cnt, x0, y0, x1, y1, fx, fy) in clusters.iter().take(n) {
            println!("   {cnt} cells in ({x0},{y0})-({x1},{y1}); first ({fx},{fy}): ours {} | client {}", desc_o(*fx, *fy), desc_c(*fx, *fy));
        }
        // Placement order against the client trace's `place` lines (when the export has a trace).
        if let Ok(trace) = std::fs::read_to_string(Path::new(&dir).join("generator_trace.tsv")) {
            let theirs: Vec<(String, String)> = trace
                .lines()
                .filter_map(|l| {
                    let f: Vec<&str> = l.split('\t').collect();
                    (f.len() > 4 && f[1] == "place").then(|| {
                        let a: Vec<&str> = f[3].split(' ').collect();
                        (format!("{} {} {} {}", a[0], a[1], a[2], f[4].to_ascii_lowercase()), f[2].to_string())
                    })
                })
                .collect();
            // The trace's coordinates are region-local; the margin is the outer buffer (inferred from the first pair unless --margin).
            let margin = margin.unwrap_or_else(|| {
                let a: Vec<i32> = theirs.first().map(|t| t.0.split(' ').take(2).map(|v| v.parse().unwrap_or(0)).collect()).unwrap_or_default();
                match (tm.pieces.first(), a.first()) {
                    (Some(p), Some(x)) => p.x - x,
                    _ => 0,
                }
            });
            let ours: Vec<String> = tm.pieces.iter().map(|p| format!("{} {} o{} {}", p.x - margin, p.y - margin - dy, p.orientation, p.path.to_ascii_lowercase())).collect();
            let same = ours.iter().zip(&theirs).take_while(|(a, b)| **a == b.0).count();
            println!("  placements: ours {}, client {}, first {same} identical", ours.len(), theirs.len());
            if std::env::var("POE_PLACE_SETS").is_ok() {
                let k1 = theirs.iter().take_while(|t| t.1 == "142098dcc" || t.1 == "14209f9a7" || std::env::var("POE_ALLPLACE").is_ok() && t.1 != "x").count();
                println!("  pass A placements of the client: {k1}");
                let a: std::collections::BTreeSet<&String> = ours.iter().filter(|o| !o.starts_with('-') && o.split(' ').nth(1).and_then(|v| v.parse::<i32>().ok()).is_some_and(|y| y >= 0)).take(k1).collect();
                let b: std::collections::BTreeSet<&String> = theirs.iter().take(k1).map(|t| &t.0).collect();
                let key = |s: &&String| {
                    let f: Vec<&str> = s.split(' ').collect();
                    (f[1].parse::<i32>().unwrap_or(0), f[0].parse::<i32>().unwrap_or(0))
                };
                let mut only_o: Vec<_> = a.difference(&b).copied().collect();
                let mut only_c: Vec<_> = b.difference(&a).copied().collect();
                only_o.sort_by_key(key);
                only_c.sort_by_key(key);
                println!("  only ours ({}):", only_o.len());
                for s in only_o.iter().take(40) {
                    println!("    {s}");
                }
                println!("  only client ({}):", only_c.len());
                for s in only_c.iter().take(40) {
                    println!("    {s}");
                }
            }
            for k in same..(same + 3).min(ours.len().max(theirs.len())) {
                println!("    {k}: ours {:?}\n        client {:?}", ours.get(k), theirs.get(k));
            }
        }
        if let Some(l) = &lat {
            println!("  lattice heights (storeys; ! = no height flag) x {}..{}", l[0], l[2]);
            for y in l[1]..=l[3] {
                let row: String = (l[0]..=l[2]).map(|x| match tm.point_index(x, y) { Some(i) => { let p = &tm.lattice[i]; format!("{:>3}{}", p.height, if p.flags & 2 != 0 { ' ' } else { '!' }) } None => "  ? ".into() }).collect();
                println!("  y{y:4} {row}");
            }
        }
        if let Some((m, step, size)) = units {
            // One cell per grid unit: the tile at its centre (name, orientation); `*` marks a differing unit.
            let nx = (w as i32 - 2 * m - 1) / step + 1;
            let ny = (h as i32 - 2 * m - 1) / step + 1;
            println!("  units {nx}x{ny} (margin {m}, step {step}): ours | client");
            for v in 0..ny {
                let (mut a, mut b) = (String::new(), String::new());
                for u in 0..nx {
                    let (cx, cy) = ((m + u * step + size / 2) as u32, (m + v * step + size / 2) as u32);
                    let f = |d: String| {
                        let mut it = d.split(' ');
                        let n = it.next().unwrap_or("").to_string();
                        let o = it.nth(2).unwrap_or("");
                        format!("{}{}", &n[..n.len().min(9)], o)
                    };
                    let (x, y) = (f(desc_o(cx, cy)), f(desc_c(cx, cy)));
                    let star = if x.to_ascii_lowercase() != y.to_ascii_lowercase() { '*' } else { ' ' };
                    a += &format!("{x:<12}{star}");
                    b += &format!("{y:<12}{star}");
                }
                println!("  v{v:2} {a}\n      {b}");
            }
        }
        if let Some((ax, ay, r)) = at {
            // Window around (ax, ay), one line per row: ours | client, tile corners only (offset 0,0) as name:o:h.
            for y in (ay - r).max(0)..=(ay + r).min(h as i32 - 1) {
                let cell = |o: bool, x: i32| -> String {
                    if x < 0 || x >= w as i32 {
                        return String::new();
                    }
                    let d = if o { desc_o(x as u32, y as u32) } else { desc_c(x as u32, y as u32) };
                    let mut it = d.split(' ');
                    let (name, sub, hh, oo) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
                    format!("{}{}{}{}", &name[..name.len().min(10)], sub, hh, oo)
                };
                let (mut a, mut b) = (String::new(), String::new());
                for x in (ax - r).max(0)..=(ax + r).min(w as i32 - 1) {
                    a += &format!("{:<22}", cell(true, x));
                    b += &format!("{:<22}", cell(false, x));
                }
                println!("  y{y:3} ours  {a}\n       client {b}");
            }
        }
        if map {
            for y in 0..h {
                let line: String = (0..w)
                    .map(|x| {
                        if diff[(y * w + x) as usize] {
                            'X'
                        } else if desc_c(x, y) == "-" {
                            '-'
                        } else {
                            '.'
                        }
                    })
                    .collect();
                println!("  {line}");
            }
        }
    }
    Ok(())
}
