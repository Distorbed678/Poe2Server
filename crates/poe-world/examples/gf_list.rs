//! Lists game files whose path contains every given (case-insensitive) substring: `gf_list <substr>...`.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let pats: Vec<String> = std::env::args().skip(1).map(|a| a.to_lowercase()).collect();
    for p in files.list() {
        let l = p.to_lowercase();
        if pats.iter().all(|s| l.contains(s)) {
            println!("{p}");
        }
    }
    Ok(())
}
