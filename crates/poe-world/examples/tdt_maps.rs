//! Prints a `.tdt`'s sub-tile cell maps: `tdt_maps <game path> [sub-tile index]`.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let a: Vec<String> = std::env::args().skip(1).collect();
    let t = poe_world::tdt::Tdt::parse(&files.read(&a[0].to_lowercase())?)?;
    println!("hull masks {:?}", t.hull_masks);
    println!("v{} size {:?} inherits {:?} subtiles {}", t.version, t.size, t.inherits, t.subtiles.len());
    for (i, s) in t.subtiles.iter().enumerate() {
        if a.get(1).is_some_and(|x| x.parse::<usize>().ok() != Some(i)) {
            continue;
        }
        println!("sub {i} flags {:#x} list {} bytes {:?} map {:?}", s.flags, s.list.len(), &s.list[..s.list.len().min(24)], s.map.as_ref().map(|m| m.len()));
        if let Some(raw) = poe_world::tdt::lz4_decode(&s.list) {
            let (f, b1) = (s.flags, s.b1);
            let w = (if f & 0x40 != 0 { 27 } else { 23 }) + if b1 & 1 != 0 { 4 } else { 0 };
            let h = (if f & 0x80 != 0 { 27 } else { 23 }) + if f & 0x20 != 0 { 4 } else { 0 };
            println!("  b1 {b1:#x} plane {w}x{h} = {} decoded {} planes {:.2}", w * h, raw.len(), raw.len() as f32 / (w * h) as f32);
        }
        if let Some(m) = &s.map && m.len() == 529 {
            for y in 0..23 {
                println!("  {}", (0..23).map(|x| format!("{:x}", m[y * 23 + x])).collect::<String>());
            }
        }
    }
    Ok(())
}
