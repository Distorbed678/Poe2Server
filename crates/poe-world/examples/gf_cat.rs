//! Prints a game file as text (UTF-16 with BOM or UTF-8): `gf_cat <path>...`; `--grep <substr> <path>` filters lines.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut grep = None;
    if args.first().map(|a| a.as_str()) == Some("--grep") {
        args.remove(0);
        grep = Some(args.remove(0).to_lowercase());
    }
    for p in args {
        let bytes = files.read(&p.to_lowercase())?;
        let text = if bytes.starts_with(&[0xff, 0xfe]) {
            let u: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&u)
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };
        for l in text.lines() {
            if grep.as_ref().is_none_or(|g| l.to_lowercase().contains(g)) {
                println!("{l}");
            }
        }
    }
    Ok(())
}
