//! Prints a game file (UTF-16 text decoded): `file_cat <path>...`
use std::path::Path;


fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gd = poe_data::GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    for p in std::env::args().skip(1) {
        let b = gd.files().read(&p)?;
        println!("== {p}\n{}", { let u: Vec<u16> = b.chunks_exact(2).skip(1).map(|c| u16::from_le_bytes([c[0], c[1]])).collect(); String::from_utf16_lossy(&u) });
    }
    Ok(())
}
