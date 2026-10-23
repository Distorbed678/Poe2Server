//! Doodads (`docs/re/terrain/doodads.md` in the hub): the objects a room (`.arm`) brings to the map, and
//! the doodad hash the client reports.
//!
//! `TerrainGen_Doodads` (`0x141ff5550`) walks every region's room placements (the list the
//! de-repetition pass also uses, region +0x58, 0x38 bytes each) and calls `TerrainGen_TileDoodads`
//! (`0x141ff3310`) for each. A placement's doodad entries are the room's `doodads`; the cell
//! of each comes from `0x141fe78d0`: the entry's cell inside the room, turned by the placement's
//! orientation (table `0x142f020c8`) within the room's `w × h` tiles, plus the placement's
//! corner. [`resolve`] then follows each entry's `AOSet` picks (`0x141fe7520`, drawing from the
//! layout's random stream, in placement order). Entries whose object type has `+0x2d0 == 0` add
//! `y + murmur(path) + (x << 16)` to the doodad hash; static types stamp their hull into the
//! collision grid ([`crate::collision`]).

use crate::{
    generate::{Files, Room},
    hash,
    rng::TerrainRng,
};

/// Cells per tile side.
pub const TILE: i32 = 23;

/// One placed doodad, in cells of the map it was placed in.
#[derive(Debug, Clone, PartialEq)]
pub struct Doodad {
    pub x: i32,
    pub y: i32,
    /// Radians, in `0..=2π` (the export's angle).
    pub angle: f32,
    /// The `.ao` file (`Metadata/...`), as the room names it.
    pub path: String,
    /// The object type (`Metadata/MiscellaneousObjects/Doodad`, ...).
    pub kind: String,
    /// Whether the doodad adds to the doodad hash ([`type_counts`]); [`resolve`] sets it,
    /// placement leaves it `false`.
    pub counted: bool,
    /// The object type is static (`Positioned.static = true`): only those stamp their hull into
    /// the collision grid; set by [`resolve`].
    pub solid: bool,
    /// The object file the doodad resolved to after the `AOSet` picks (its `Hull` is rasterised
    /// into the collision grid); empty before [`resolve`].
    pub leaf: String,
    /// The placement is mirrored (orientations 4..7) and the arm entry's flip flag.
    pub mirrored: bool,
    pub entry_flip: bool,
    /// The arm entry's scale.
    pub entry_scale: f32,
    /// Mirrored hull (placement XOR pick XOR entry flag; `0x141fe78d0`); set by [`resolve`].
    pub flip: bool,
    /// Entry scale times the `AOSet` scale pick; set by [`resolve`].
    pub scale: f32,
}

/// `0x141fe78d0`: the cell of an entry `(ex, ey)` of a room of `w × h` tiles turned by
/// orientation `o`, relative to the placement's corner. The table at `0x142f020c8` (three bytes
/// per orientation) says which of `(w·23 - ex - 1, ex, h·23 - ey - 1, ey)` give x and y.
pub fn turn_cell(ex: i32, ey: i32, w: i32, h: i32, o: u8) -> (i32, i32) {
    const T: [[usize; 3]; 9] = [
        [0, 1, 1],
        [1, 0, 1],
        [0, 0, 0],
        [1, 1, 0],
        [0, 0, 1],
        [1, 0, 0],
        [0, 1, 0],
        [1, 1, 1],
        [0, 1, 1],
    ];
    let a = [w * TILE - ex - 1, ex, h * TILE - ey - 1, ey];
    let t = T[(o as usize).min(8)];
    let x = a[t[1] + t[0] * 2];
    let y = a[t[2] + if t[0] == 0 { 2 } else { 0 }];
    (x, y)
}

/// The entry's angle after the placement's turn (`0x141ff3310`/`0x141fe78d0`): mirrored
/// orientations (4..7) negate it, each quarter turn adds π/2, then it is wrapped into `0..=2π`.
pub fn turn_angle(radians: f32, o: u8) -> f32 {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let mut a = if (4..8).contains(&o) { TAU - radians } else { radians };
    a += (o & 3) as f32 * FRAC_PI_2;
    while a < 0.0 {
        a += TAU;
    }
    while a > TAU {
        a -= TAU;
    }
    a
}

/// The doodads of `room` placed with its turned corner at `origin` (tiles) and orientation `o`.
pub fn room_doodads(room: &Room, origin: (i32, i32), o: u8) -> Vec<Doodad> {
    let (w, h) = (room.width() as i32, room.height() as i32);
    room.arm
        .doodads
        .iter()
        .map(|d| {
            let (x, y) = turn_cell(d.x as i32, d.y as i32, w, h, o);
            Doodad {
                x: x + origin.0 * TILE,
                y: y + origin.1 * TILE,
                angle: turn_angle(d.radians1, o),
                path: d.ao_file.clone(),
                kind: d.stub.clone(),
                counted: false,
                leaf: String::new(),
                solid: false,
                mirrored: (4..8).contains(&o),
                entry_flip: d.bool1,
                entry_scale: d.scale,
                flip: false,
                scale: 1.0,
            }
        })
        .collect()
}

/// Whether objects of type `kind` count for the hash (`+0x2d0 == 0` and `+0x2d4` accepted on
/// the type's data, `0x141ff3310`): the type exists (a template `.ot`, or an `.ao` extending
/// one) and its merged `BaseEvents` sets neither `server_only` nor `client_only` (the side flag
/// that `+0x2d0` holds) and no `league_category` (`+0x2d4`: 0x39 without one; with one it must
/// equal the area's league category, which is 0 for areas without league mechanics rolled).
pub fn type_solid(gd: &poe_data::GameData, kind: &str) -> bool {
    poe_data::template::ObjectTemplate::load(gd, kind)
        .map(|t| t.component("Positioned").iter().any(|(k, v)| k == "static" && v == "true"))
        .unwrap_or(false)
}

/// Whether objects of type `kind` count for the hash (see above).
pub fn type_counts(gd: &poe_data::GameData, kind: &str) -> bool {
    let Ok(t) = poe_data::template::ObjectTemplate::load(gd, kind) else { return false };
    // The type has a template of its own: a path that only has an `.ao` (extending another
    // type, such as `Metadata/Chests/SirenEggs/SirenEgg_02` in G1_3) is not an object type.
    if !t.files.last().is_some_and(|f| f.ends_with(".ot")) {
        return false;
    }
    !t.component("BaseEvents").iter().any(|(k, v)| match k.as_str() {
        "server_only" | "client_only" => v == "true",
        "league_category" => !v.is_empty(),
        _ => false,
    })
}

/// What the doodad pass reads from an object file (`.ao`): its `AOSet` component, merged over the
/// `extends` chain (`FUN_141d5ada0` looks the component up, `0x141fe7520` uses it).
#[derive(Debug, Clone, Default)]
pub struct AoInfo {
    /// Some file of the chain declares `AOSet` (for example through `Metadata/AOSetParent`).
    pub has_set: bool,
    /// `ao = "<weight> <path>"` entries; the path `null` is `None`.
    pub choices: Vec<(Option<String>, u32)>,
    /// Number of `fixed_ao` entries.
    pub fixed: usize,
    pub rotation: (f32, f32),
    pub random_rotation: bool,
    pub scale: (f32, f32),
    pub translate_radius: f32,
    /// `random_orientation` (all eight orientations) / `random_flip` (the mirrored pair): the
    /// allowed-orientation mask at `+0x5c` (`0x11` flip, `0xff` orientation; 0 none).
    pub orientation_mask: u8,
    /// `Hull` block lines (`key`, unquoted value) over the `extends` chain, base first.
    pub hull: Vec<(String, String)>,
}

fn quoted(v: &str) -> &str {
    v.trim().trim_matches('"')
}

/// Parses one `.ao` file's text: its `extends` and `AOSet` block.
fn parse_ao(text: &str) -> (Option<String>, Option<AoInfo>, Vec<(String, String)>) {
    let mut extends = None;
    let mut hull: Vec<(String, String)> = Vec::new();
    let mut set: Option<AoInfo> = None;
    let (mut depth, mut name) = (0usize, String::new());
    for line in text.lines() {
        let l = line.trim();
        if depth == 0 {
            if let Some(r) = l.strip_prefix("extends") {
                extends = Some(quoted(r).to_string());
                continue;
            }
            let head = l.split(['{', '}']).next().unwrap_or("").trim();
            if !head.is_empty() && !l.starts_with("//") {
                name = head.to_string();
            }
            if name == "AOSet" && set.is_none() {
                set = Some(AoInfo { has_set: true, scale: (1.0, 1.0), ..Default::default() });
            }
        }
        if depth == 1 && name == "Hull" && let Some((k, v)) = l.split_once('=') {
            hull.push((k.trim().to_string(), quoted(v).to_string()));
        }
        let inside = depth == 1 && name == "AOSet";
        if inside && let Some((k, v)) = l.split_once('=') {
            let (k, v) = (k.trim(), quoted(v));
            let s = set.as_mut().unwrap();
            let f = |v: &str| v.parse::<f32>().unwrap_or(0.0);
            match k {
                "ao" => {
                    let (w, p) = v.split_once(' ').unwrap_or((v, "null"));
                    let p = p.trim();
                    s.choices.push((if p == "null" { None } else { Some(p.to_string()) }, w.trim().parse().unwrap_or(0)));
                }
                "fixed_ao" => s.fixed += 1,
                "random_rotation" => s.random_rotation = v == "true",
                "random_rotation_min" => s.rotation.0 = f(v),
                "random_rotation_max" => s.rotation.1 = f(v),
                "random_scale_min" => s.scale.0 = f(v),
                "random_scale_max" => s.scale.1 = f(v),
                "random_translate_radius" => s.translate_radius = f(v),
                "random_orientation" if v == "true" => s.orientation_mask |= 0xff,
                "random_flip" if v == "true" => s.orientation_mask |= 0x11,
                _ => {}
            }
        }
        for c in l.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    (extends, set, hull)
}

/// Object files by path, parsed once.
pub struct AoCache<'a> {
    files: &'a dyn Files,
    cache: std::collections::HashMap<String, std::sync::Arc<AoInfo>>,
}

impl<'a> AoCache<'a> {
    pub fn new(files: &'a dyn Files) -> Self {
        Self { files, cache: Default::default() }
    }

    /// The merged `AOSet` information of `path` (a missing file is a plain object).
    pub fn get(&mut self, path: &str) -> std::sync::Arc<AoInfo> {
        let key = crate::generate::normalize_path(path);
        if let Some(i) = self.cache.get(&key) {
            return i.clone();
        }
        let mut info = AoInfo { scale: (1.0, 1.0), ..Default::default() };
        let mut chain = Vec::new();
        let mut next = Some(key.clone());
        while let Some(p) = next.take() {
            if chain.len() > 16 {
                break;
            }
            let Ok(bytes) = self.files.read(&format!("{p}")) else { break };
            let (ext, set, hull) = parse_ao(&crate::text_utf16(&bytes));
            chain.push((set, hull));
            next = ext.filter(|e| !e.is_empty() && e != "nothing").map(|e| crate::generate::normalize_path(&format!("{e}.ao")));
        }
        for (set, hull) in chain.into_iter().rev() {
            info.hull.extend(hull);
            let Some(set) = set else { continue };
            info.has_set = true;
            info.choices.extend(set.choices);
            info.fixed += set.fixed;
            info.random_rotation |= set.random_rotation;
            info.orientation_mask |= set.orientation_mask;
            if set.rotation != (0.0, 0.0) {
                info.rotation = set.rotation;
            }
            if set.scale != (1.0, 1.0) {
                info.scale = set.scale;
            }
            if set.translate_radius != 0.0 {
                info.translate_radius = set.translate_radius;
            }
        }
        let info = std::sync::Arc::new(info);
        self.cache.insert(key, info.clone());
        info
    }
}

/// `0x1421c3d00`: the orientation `u` selects among those the mask allows (bit `i` for
/// orientation `i`); a mask of 0 or 0xff takes `u & 7`.
fn orientation_pick(mask: u8, u: u32) -> u8 {
    if (1..0xff).contains(&mask) {
        let list: Vec<u8> = (0..8).filter(|i| mask >> i & 1 != 0).collect();
        list[u as usize % list.len()]
    } else {
        (u & 7) as u8
    }
}

/// `0x141fe7520`: follows the `AOSet` choices of a doodad's object file (one `below(total)` draw
/// per level, `0x141fe75b7`) until a file without choices; `None` when a `null` choice (or an
/// empty set) ends the chain: the client then skips the entry. A file without an `AOSet` or with
/// `fixed_ao` entries ends the chain, and the last set seen then draws for its rotation
/// (`0x200`), orientation (`0x348`), scale (`0x200`, `0x141fe775c`) and translation (two `0x200`).
/// Returns the angle and cell offset that adds to the entry.
struct Pick {
    angle: f32,
    off: (i32, i32),
    scale: f32,
    mirror: bool,
    leaf: String,
}

fn pick(cache: &mut AoCache, path: &str, rng: &mut TerrainRng) -> Option<Pick> {
    let mut path = path.to_string();
    let mut last: Option<std::sync::Arc<AoInfo>> = None;
    loop {
        let info = cache.get(&path);
        if std::env::var("POE_DBG_DOODADS").is_ok() {
            eprintln!("DOODAD pick {path}: set {} choices {} fixed {} rot {:?}/{} scale {:?} radius {} orient {:#x}", info.has_set, info.choices.len(), info.fixed, info.rotation, info.random_rotation, info.scale, info.translate_radius, info.orientation_mask);
        }
        if info.has_set {
            last = Some(info.clone());
        }
        if !info.has_set || info.fixed != 0 {
            let mut angle = 0.0;
            let mut off = (0, 0);
            let (mut scale, mut mirror) = (1.0f32, false);
            if let Some(s) = &last {
                // `random_rotation = true` without a range is -π..π.
                let (lo, hi) = if s.random_rotation && s.rotation == (0.0, 0.0) { (-std::f32::consts::PI, std::f32::consts::PI) } else { s.rotation };
                if lo < 0.0 || hi > 0.0 {
                    let r = rng.below(0x200) as f32;
                    angle = (hi - lo) * r / 512.0 + lo;
                }
                if s.orientation_mask != 0 {
                    let u = rng.below(0x348);
                    let o = orientation_pick(s.orientation_mask, u);
                    if std::env::var("POE_DBG_DOODAD_ORDER").is_ok() {
                        eprintln!("    orientation u {u} mask {:#x} -> {o}", s.orientation_mask);
                    }
                    angle = turn_angle(angle, o);
                    mirror = (4..8).contains(&o);
                }
                if s.scale.0 < 1.0 || s.scale.1 > 1.0 {
                    let u = rng.below(0x200) as f32;
                    scale = (s.scale.1 - s.scale.0) * u * 0.001953125 + s.scale.0;
                }
                if s.translate_radius > 0.0 {
                    let d = (s.translate_radius * s.translate_radius * rng.below(0x200) as f32 / 512.0).sqrt();
                    let t = rng.below(0x200) as f32 / 512.0 * std::f32::consts::TAU;
                    off = ((d * t.sin() + 0.5).floor() as i32, (-d * t.cos() + 0.5).floor() as i32);
                }
            }
            return Some(Pick { angle, off, scale, mirror, leaf: path });
        }
        let total: u32 = info.choices.iter().map(|c| c.1).sum();
        let mut r = rng.below(total) as i64;
        let mut chosen = None;
        for (p, w) in &info.choices {
            if r < *w as i64 {
                chosen = Some(p.clone());
                break;
            }
            r -= *w as i64;
        }
        match chosen.flatten() {
            Some(p) => path = p,
            None => return None,
        }
    }
}

/// `(counts for the hash, static)` of an object type, read once per process: both come from the
/// type's `.ot`/`.ao` files, and reading them through `GameData` takes its bundle lock, which
/// serialised the threads of the batch tools. One process uses one game version.
fn type_flags(gd: &poe_data::GameData, kind: &str) -> (bool, bool) {
    use std::sync::{Mutex, OnceLock};
    static FLAGS: OnceLock<Mutex<std::collections::HashMap<String, (bool, bool)>>> = OnceLock::new();
    let map = FLAGS.get_or_init(Default::default);
    if let Some(f) = map.lock().unwrap_or_else(|e| e.into_inner()).get(kind) {
        return *f;
    }
    let counts = type_counts(gd, kind);
    // An `.ao`-only path is no object type, so it neither counts nor stamps a hull; `type_solid`
    // reads the template only.
    let own = poe_data::template::ObjectTemplate::load(gd, kind).is_ok_and(|t| t.files.last().is_some_and(|f| f.ends_with(".ot")));
    let f = (counts, own && type_solid(gd, kind));
    map.lock().unwrap_or_else(|e| e.into_inner()).insert(kind.to_string(), f);
    f
}

/// The doodad pass over the placed doodads in placement order: each entry resolves its object
/// file (drawing from `rng` for `AOSet` choices), entries that resolve to nothing are dropped, and
/// [`Doodad::counted`] is set from the object type.
pub fn resolve(gd: &poe_data::GameData, files: &dyn Files, rng: &mut TerrainRng, doodads: Vec<Doodad>) -> Vec<Doodad> {
    let mut cache = AoCache::new(files);
    let mut out = Vec::with_capacity(doodads.len());
    let dbg = std::env::var("POE_DBG_DOODAD_ORDER").is_ok();
    for (i, mut d) in doodads.into_iter().enumerate() {
        let before = rng.draws;
        if dbg {
            eprintln!("    base angle {:.4} at ({}, {})", d.angle, d.x, d.y);
        }
        let picked = pick(&mut cache, &d.path, rng);
        if dbg && rng.draws != before {
            eprintln!("DOODAD #{i} ({}, {}) {} draws {} -> {}", d.x, d.y, d.path.rsplit('/').next().unwrap_or(""), rng.draws - before, if picked.is_some() { "kept" } else { "null" });
        }
        let Some(p) = picked else { continue };
        d.angle += p.angle;
        d.x += p.off.0;
        d.y += p.off.1;
        d.leaf = p.leaf;
        d.flip = d.mirrored ^ p.mirror ^ d.entry_flip;
        d.scale = d.entry_scale * p.scale;
        (d.counted, d.solid) = type_flags(gd, &d.kind);
        out.push(d);
    }
    out
}

/// The doodad hash of the doodads that count (`+0x2d0 == 0` on their type), in any order.
pub fn doodad_hash<'a>(doodads: impl IntoIterator<Item = &'a Doodad>) -> u32 {
    hash::doodad_hash(doodads.into_iter().map(|d| (d.x, d.y, d.path.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_are_bijections() {
        // A 2×1 tile room: the 8 orientations send (3, 5) to distinct, in-range cells.
        let mut seen = std::collections::HashSet::new();
        for o in 0..8 {
            let (x, y) = turn_cell(3, 5, 2, 1, o);
            let (rw, rh) = if o & 1 == 1 && !(4..8).contains(&o) || matches!(o, 3 | 5 | 7) { (23, 46) } else { (46, 23) };
            assert!(x >= 0 && x < rw && y >= 0 && y < rh, "o{o}: {x},{y}");
            seen.insert((x, y));
        }
        assert!(seen.len() >= 6);
        assert_eq!(turn_cell(3, 5, 2, 1, 0), (3, 5));
    }
}
