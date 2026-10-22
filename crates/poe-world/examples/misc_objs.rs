//! Exploration: MiscObjects rows with the given Hash32 values.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gd = poe_data::GameData::open(std::path::Path::new("GameFiles"), std::path::Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let t = gd.table("MiscObjects")?;
    let want: Vec<i64> = std::env::args().skip(1).map(|a| i64::from_str_radix(a.trim_start_matches("0x"), 16).unwrap()).collect();
    println!("columns: {:?}", t.schema().columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
    for r in t.rows() {
        let h = r.get("ObjectHASH32").ok().and_then(|v| v.as_int());
        let id = r.get("Id").ok().and_then(|v| v.as_str().map(String::from));
        if let Some(h) = h { if want.contains(&(h & 0xffff_ffff)) { println!("{h:#x} {id:?}"); } }
    }
    Ok(())
}
