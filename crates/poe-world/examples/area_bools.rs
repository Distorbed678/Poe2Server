//! Prints, per boolean column of WorldAreas, which of the given areas have it set:
//! `area_bools <area id>...` (columns with a mixed value across the areas are marked).
use std::path::Path;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ids: Vec<String> = std::env::args().skip(1).collect();
    let gd = poe_data::GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let t = gd.table("WorldAreas")?;
    let rows: Vec<_> = ids.iter().map(|i| t.find_by_str("Id", i).ok().flatten()).collect();
    for (c, col) in t.schema().columns.iter().enumerate() {
        let vals: Vec<String> = rows.iter().map(|r| r.as_ref().map_or("?".into(), |r| match r.get_index(c) { Ok(poe_data::Value::Bool(b)) => (b as u8).to_string(), _ => "x".into() })).collect();
        if vals.iter().all(|v| v == "x") { continue; }
        println!("{c:3} off {:#x} {:?} {}", t.schema().offset(c), col.name, vals.join(""));
    }
    Ok(())
}
