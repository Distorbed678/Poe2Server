//! Lists the tiles of a `.tst` (including the master's tile set) whose edges contain all the given substrings:
//! `tile_scan <tst path> <substring>...`
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut a = std::env::args().skip(1);
    let tst = a.next().ok_or("usage: tile_scan <tst> <substr>...")?;
    let subs: Vec<String> = a.map(|s| s.to_lowercase()).collect();
    let files = poe_data::GameFiles::open(Path::new("GameFiles"))?;
    let text = poe_world::text_utf16(files.read(&tst.to_lowercase())?.as_ref());
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with("//") || !l.starts_with('"') {
            continue;
        }
        let p = l.split('"').nth(1).unwrap_or("");
        let Ok(b) = files.read(&p.to_lowercase()) else { continue };
        let Ok(t) = poe_world::tdt::Tdt::parse(&b) else { continue };
        let edges: Vec<String> = t.edges.iter().map(|e| e.as_deref().unwrap_or("-").rsplit('/').next().unwrap_or("").to_lowercase()).collect();
        let joined = edges.join(" ");
        if subs.iter().all(|s| joined.contains(s)) {
            println!("{p} size {:?} edges {:?} grounds {:?} feature {:?}", t.size, edges, t.grounds.iter().map(|e| e.as_deref().unwrap_or("-").rsplit('/').next().unwrap_or("").to_string()).collect::<Vec<_>>(), t.feature);
        }
    }
    Ok(())
}
