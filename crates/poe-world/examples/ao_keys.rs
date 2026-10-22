//! Exploration: the keys of the `AOSet` blocks of all `.ao`/`.ot` files.
use std::collections::BTreeMap;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let mut keys: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut n = 0;
    for p in files.list() {
        let l = p.to_lowercase();
        if !(l.starts_with("metadata/") && (l.ends_with(".ao") || l.ends_with(".ot"))) { continue; }
        let Ok(b) = files.read(&l) else { continue };
        let t = poe_world::text_utf16(&b);
        let mut inb = false; let mut depth = 0; let block = std::env::args().nth(1).unwrap_or("AOSet".into()); let block = block.as_str();
        let mut it = t.lines().peekable();
        while let Some(line) = it.next() {
            let s = line.trim();
            if depth == 0 && s == block { inb = true; }
            if s == "{" { depth += 1; continue; }
            if s == "}" { depth -= 1; if depth == 0 { inb = false; } continue; }
            if inb && depth == 1 {
                if let Some((k, v)) = s.split_once('=') {
                    let e = keys.entry(k.trim().to_string()).or_insert((0, format!("{p}: {}", v.trim())));
                    e.0 += 1;
                }
            }
        }
        n += 1;
    }
    println!("files {n}");
    for (k, (c, ex)) in keys { println!("{c}\t{k}\t{ex}"); }
    Ok(())
}
