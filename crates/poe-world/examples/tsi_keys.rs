fn main() {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles")).unwrap();
    let mut keys: std::collections::BTreeMap<String, (usize, String)> = Default::default();
    for p in files.list() {
        if !p.ends_with(".tsi") { continue; }
        let b = files.read(&p).unwrap();
        for (k, _) in poe_world::rooms::tsi_entries(&b) {
            let e = keys.entry(k).or_insert((0, p.clone()));
            e.0 += 1;
        }
    }
    for (k, (n, p)) in keys { println!("{n:5} {k} {p}"); }
}
