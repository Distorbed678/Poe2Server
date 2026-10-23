//! Lists data files that contain an `include` line: `list_includes <extension>`.
fn main() {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles")).unwrap();
    let ext = std::env::args().nth(1).unwrap();
    for p in files.list() {
        if !p.ends_with(&ext) { continue; }
        let Ok(b) = files.read(&p) else { continue };
        let t = { let u: Vec<u16> = b.chunks_exact(2).skip(1).map(|c| u16::from_le_bytes([c[0], c[1]])).collect(); String::from_utf16_lossy(&u) };
        let inc: Vec<&str> = t.lines().filter(|l| l.trim_start().to_ascii_lowercase().starts_with("include")).collect();
        if !inc.is_empty() { println!("{p}\t{:?}", inc); }
    }
}
