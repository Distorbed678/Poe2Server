//! Exploration: which game file path hashes (several hash functions) equal the given values.
use poe_world::hash::{murmur2, murmur2_utf16};
fn fnv1a(b: &[u8]) -> u32 { let mut h = 0x811c9dc5u32; for &c in b { h ^= c as u32; h = h.wrapping_mul(0x01000193); } h }
fn crc32(b: &[u8]) -> u32 { let mut c = !0u32; for &x in b { c ^= x as u32; for _ in 0..8 { c = if c & 1 != 0 { (c >> 1) ^ 0xedb88320 } else { c >> 1 }; } } !c }
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = poe_data::GameFiles::open(std::path::Path::new("GameFiles"))?;
    let want: Vec<u32> = std::env::args().skip(1).map(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).unwrap()).collect();
    let seeds = [0xf36b12u32, 0, 0x5bd1e995, 0x9747b28c, 1];
    for p in files.list() {
        let lp = p.to_lowercase();
        if !lp.starts_with("metadata/") { continue; }
        let mut cands: Vec<(String, String)> = Vec::new();
        for (tag, c) in [("orig", p.clone()), ("lower", lp.clone())] {
            cands.push((tag.into(), c.clone()));
            if let Some((stem, _)) = c.rsplit_once('.') { cands.push((format!("{tag}-stem"), stem.to_string())); }
            if let Some(r) = c.strip_prefix("Metadata/").or(c.strip_prefix("metadata/")) { cands.push((format!("{tag}-nometa"), r.to_string())); }
        }
        for (tag, c) in cands {
            let mut hs: Vec<(String, u32)> = Vec::new();
            for &sd in &seeds {
                hs.push((format!("m16 {sd:#x}"), murmur2_utf16(&c, sd)));
                hs.push((format!("m8 {sd:#x}"), murmur2(c.as_bytes(), sd)));
            }
            hs.push(("fnv".into(), fnv1a(c.as_bytes())));
            hs.push(("crc".into(), crc32(c.as_bytes())));
            for (n, h) in hs { if want.contains(&h) { println!("{h:#x} {n} [{tag}] {p}"); } }
        }
    }
    Ok(())
}
