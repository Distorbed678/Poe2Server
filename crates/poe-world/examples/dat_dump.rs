//! Prints the first rows of a dat table: `dat_dump <Table> [rows]`.
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut a = std::env::args().skip(1);
    let name = a.next().ok_or("usage: dat_dump <Table> [rows]")?;
    let n: usize = a.next().map(|v| v.parse()).transpose()?.unwrap_or(20);
    let find = a.next();
    let gd = poe_data::GameData::open(Path::new("GameFiles"), Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let t = gd.table(&name)?;
    for r in 0..n {
        let Some(row) = t.row(r) else { break };
        if let Some(f) = &find {
            if row.get_index(0).ok().map(|v| format!("{v:?}")).is_none_or(|v| !v.contains(f.as_str())) {
                continue;
            }
        }
        let v: Vec<String> = (0..t.schema().columns.len()).map(|c| format!("{:?}", row.get_index(c).ok())).collect();
        println!("{r}: {}", v.join(" | "));
    }
    Ok(())
}
