//! Exploration: BaseEvents flags of object types: `ot_check <yes list> <no list>`.
use std::collections::BTreeMap;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gd = poe_data::GameData::open(std::path::Path::new("GameFiles"), std::path::Path::new("third_party/dat-schema/schema-poe2.min.json"))?;
    let a: Vec<String> = std::env::args().skip(1).collect();
    for (name, f) in [("yes", &a[0]), ("no", &a[1])] {
        let mut c: BTreeMap<String, (usize, String)> = BTreeMap::new();
        for k in std::fs::read_to_string(f)?.lines() {
            let t = poe_data::template::ObjectTemplate::load(&gd, k);
            let key = match t {
                Err(_) => "unresolvable".to_string(),
                Ok(t) => {
                    let be = t.component("BaseEvents");
                    let fl = |n: &str| be.iter().any(|(k, v)| k == n && v == "true");
                    format!("server_only={} client_only={} no_serialise={} hasBE={}", fl("server_only"), fl("client_only"), fl("no_serialise"), !be.is_empty() || t.components.contains_key("BaseEvents"))
                }
            };
            let e = c.entry(key).or_insert((0, k.to_string()));
            e.0 += 1;
        }
        println!("{name}:");
        for (k, (n, ex)) in c { println!("  {n}\t{k}\t{ex}"); }
    }
    Ok(())
}
