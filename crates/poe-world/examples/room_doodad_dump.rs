//! Prints the doodads of an .arm room as `(x, y, angle, file)` for every orientation:
//! `room_doodad_dump <room.arm>` (cells relative to the room's turned corner).
fn main() {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles")).unwrap();
    let room = poe_world::generate::load_room(&files, &std::env::args().nth(1).unwrap().to_lowercase()).unwrap();
    println!("room {}x{}", room.width(), room.height());
    for o in 0..8u8 {
        for d in poe_world::doodads::room_doodads(&room, (0, 0), o) {
            println!("o{o}\t{}\t{}\t{:.4}\t{}", d.x, d.y, d.angle, d.path.rsplit('/').next().unwrap());
        }
    }
}
