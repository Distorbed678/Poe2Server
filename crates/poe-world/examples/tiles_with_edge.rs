//! Lists tiles of a tile set (and the feature) whose key has an edge containing a substring:
//! `tiles_with_edge <tiles.tst> <fill.gft> <substr>...` (all substrings must be present).
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let master = poe_world::generate::Master::load(&files, &args[0].to_lowercase(), &args[1].to_lowercase())?;
    for t in &master.tiles.tiles {
        let edges: Vec<String> = t.key.edges.iter().map(|e| e.clone().unwrap_or("-".into())).collect();
        if args[2..].iter().all(|s| edges.iter().any(|e| e.contains(&s.to_lowercase()))) {
            let short: Vec<&str> = edges.iter().map(|e| e.rsplit('/').next().unwrap()).collect();
            println!("{:?}\t{}\tw{} o{:#04x} size {:?} feature {:?}", short, t.path, t.weight, t.orientations, t.key.size, t.tdt.feature);
        }
    }
    Ok(())
}
