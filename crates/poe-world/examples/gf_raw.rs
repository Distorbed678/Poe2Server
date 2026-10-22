//! Writes a game file's raw bytes: `gf_raw <game path> <out file>`.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let a: Vec<String> = std::env::args().skip(1).collect();
    std::fs::write(&a[1], files.read(&a[0].to_lowercase())?)?;
    Ok(())
}
