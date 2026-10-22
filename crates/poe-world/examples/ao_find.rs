//! Exploration: AOSet files that set a key: `ao_find <key>`.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let key = std::env::args().nth(1).unwrap();
    for p in files.list() {
        let l = p.to_lowercase();
        if !(l.starts_with("metadata/") && l.ends_with(".ao")) { continue; }
        let Ok(b) = files.read(&l) else { continue };
        let t = poe_world::text_utf16(&b);
        if t.lines().any(|x| x.trim().starts_with(&key)) { println!("{l}"); }
    }
    Ok(())
}
