//! Exploration: which .datc64 tables contain the given u32 values (little endian).
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let want: Vec<u32> = std::env::args().skip(1).map(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).unwrap()).collect();
    for p in files.list() {
        let lp = p.to_lowercase();
        if !lp.ends_with(".datc64") { continue; }
        let Ok(b) = files.read(&lp) else { continue };
        for &w in &want {
            let n = w.to_le_bytes();
            let mut hits = Vec::new();
            for (i, win) in b.windows(4).enumerate() { if win == n { hits.push(i); if hits.len() > 3 { break; } } }
            if !hits.is_empty() { println!("{lp} {w:#x} at {hits:?}"); }
        }
    }
    Ok(())
}
