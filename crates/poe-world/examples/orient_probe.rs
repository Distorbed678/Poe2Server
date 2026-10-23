//! Prints where cell (x, y) of a w x h grid lands for each orientation: `orient_probe w h x y`.
fn main() {
    let a: Vec<i32> = std::env::args().skip(1).map(|v| v.parse().unwrap()).collect();
    for o in 0..8u8 {
        println!("o{o}: {:?}", poe_world::tileset::orient_cell(o, a[0], a[1], a[2], a[3]));
    }
}
