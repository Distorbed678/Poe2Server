//! Exploration: statistics of the doodads in every `.arm` room.
use poe_data_tools::file_parsers::{FileParser, arm::ARMParser};
use std::collections::BTreeMap;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let mut st: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let (mut rooms, mut bad) = (0, 0);
    for p in files.list() {
        if !p.to_lowercase().ends_with(".arm") { continue; }
        let b = files.read(&p.to_lowercase())?;
        let Ok(arm) = ARMParser.parse(&b) else { bad += 1; continue };
        rooms += 1;
        for d in &arm.doodads {
            let key = format!("b1={} b2={:?} floats={} kv={:?} pairs={} trig={}", d.bool1, d.bool2, d.floats.len(), d.key_values.as_ref().map(|k| { let mut v: Vec<_> = k.keys().cloned().collect(); v.sort(); v }), d.float_pairs.as_ref().map_or(-1, |f| f.len() as i32), d.trig1.is_some());
            let e = st.entry(key).or_insert((0, format!("{p} ({},{}) rad {} floats {:?} sc {}", d.x, d.y, d.radians1, d.floats, d.scale)));
            e.0 += 1;
        }
    }
    println!("rooms {rooms} unparsed {bad}");
    for (k, (n, ex)) in st { println!("{n}\t{k}\t{ex}"); }
    Ok(())
}
