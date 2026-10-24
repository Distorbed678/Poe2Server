//! Lists the rooms of a room set with their tags: `rs_tags <room set .rs path>...`
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    for p in std::env::args().skip(1) {
        let set = poe_world::rooms::RoomSet::load(&files, &p.to_lowercase())?;
        println!("{p}");
        for r in &set.rooms {
            println!("  {:<20} w{} o{:#04x} {}", r.tag, r.weight, r.orientations, r.path);
        }
    }
    Ok(())
}
