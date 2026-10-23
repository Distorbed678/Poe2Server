//! Prints a `.tdt`'s key in each of the eight orientations: `key_orient <path.tdt>...`.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    for path in std::env::args().skip(1) {
        let t = poe_world::tdt::Tdt::parse(&files.read(&path.to_lowercase())?)?;
        let k = poe_world::tileset::TileKey::from_tdt(&t);
        let n = |e: &Option<String>| e.as_deref().map_or("-".to_string(), |s| s.rsplit('/').next().unwrap_or(s).to_string());
        for o in 0..8 {
            let q = k.oriented(o);
            println!("o{o} size {:?} edges {} grounds {} h {:?} x {:?} v {:?}", q.size, q.edges.iter().map(n).collect::<Vec<_>>().join(","), q.grounds.iter().map(n).collect::<Vec<_>>().join(","), q.heights, q.exits, q.virtual_exits);
        }
    }
    Ok(())
}
