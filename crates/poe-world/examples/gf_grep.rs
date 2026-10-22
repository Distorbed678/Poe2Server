//! Lists game files with a path suffix that contain a text (UTF-8 or UTF-16): `gf_grep <suffix> <needle>`.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (suffix, needle) = (a[0].to_lowercase(), a[1].to_lowercase());
    let n16: Vec<u8> = needle.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    for p in files.list() {
        if !p.to_lowercase().ends_with(&suffix) {
            continue;
        }
        let Ok(b) = files.read(&p.to_lowercase()) else { continue };
        let low: Vec<u8> = b.iter().map(|c| c.to_ascii_lowercase()).collect();
        let has = |n: &[u8]| !n.is_empty() && low.windows(n.len()).any(|w| w == n);
        if has(needle.as_bytes()) || has(&n16) {
            println!("{p}");
        }
    }
    Ok(())
}
