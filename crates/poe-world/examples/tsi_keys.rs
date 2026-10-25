//! Every `.tsi` key with its count and one file; `tsi_keys <key>...`: every file with one of the
//! keys, with the values of those keys.
fn main() {
    let want: Vec<String> = std::env::args().skip(1).collect();
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles")).unwrap();
    let mut keys: std::collections::BTreeMap<String, (usize, String)> = Default::default();
    for p in files.list() {
        if !p.ends_with(".tsi") { continue; }
        let b = files.read(&p).unwrap();
        if !want.is_empty() {
            let e = poe_world::rooms::tsi_entries(&b);
            if e.iter().any(|(k, _)| want.contains(k)) {
                let vals: Vec<String> = e.iter().filter(|(k, _)| want.contains(k)).map(|(k, v)| format!("{k}={v}")).collect();
                println!("{p}\t{}", vals.join("\t"));
            }
            continue;
        }
        for (k, _) in poe_world::rooms::tsi_entries(&b) {
            let e = keys.entry(k).or_insert((0, p.clone()));
            e.0 += 1;
        }
    }
    for (k, (n, p)) in keys { println!("{n:5} {k} {p}"); }
}
