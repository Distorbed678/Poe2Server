//! Tile matching: the key a tile or a room slot is matched by, its eight orientations, and the
//! tile-set index the generator looks keys up in (`docs/re/terrain/tgr.md`).
//!
//! A key (`0x141d09a80` from a tile, `0x141d08bc0` oriented, 0x4e bytes compared) holds the
//! edge type of each side with its exit and virtual-exit positions, the size, and the ground
//! type and height of each corner. Sides and corners are in file order (the `.tdt` header's and
//! the `.arm` slot's, which agree). The index (`0x141d09c30`) lists, per feature name and key,
//! `(tile, orientation, weight)` in `.tst` order and orientation order; the generator picks from
//! a list with `below(total weight)`, so this order is part of the layout.

use std::collections::HashMap;

use crate::tdt::Tdt;

/// `I, R90, R180, R270, FI, FR90, FR180, FR270` (`0x142f08770`).
pub const ORIENTATION_NAMES: [&str; 8] =
    ["I", "R90", "R180", "R270", "FI", "FR90", "FR180", "FR270"];

/// Side permutation per orientation (`0x142f08850`): oriented side `i` is source side `T[o][i]`.
const SIDES: [[usize; 4]; 8] = [
    [0, 1, 2, 3],
    [3, 0, 1, 2],
    [2, 3, 0, 1],
    [1, 2, 3, 0],
    [0, 3, 2, 1],
    [1, 0, 3, 2],
    [2, 1, 0, 3],
    [3, 2, 1, 0],
];
/// Corner permutation per orientation (`0x142f08870`).
const CORNERS: [[usize; 4]; 8] = [
    [0, 1, 2, 3],
    [3, 0, 1, 2],
    [2, 3, 0, 1],
    [1, 2, 3, 0],
    [1, 0, 3, 2],
    [2, 1, 0, 3],
    [3, 2, 1, 0],
    [0, 3, 2, 1],
];

/// Cell transform per orientation (`0x142f02118`): (transpose, x source, y source) where a
/// source 0 mirrors.
const TRANSFORM: [(bool, u8, u8); 8] = [
    (false, 1, 1),
    (true, 0, 1),
    (false, 0, 0),
    (true, 1, 0),
    (false, 0, 1),
    (true, 0, 0),
    (false, 1, 0),
    (true, 1, 1),
];

/// Where corner point `(u, v)` (0..=w, 0..=h) of a `w × h` piece lands with orientation `o`
/// (`0x141d0bc50`, same table as [`orient_cell`]).
pub fn orient_point(o: u8, w: i32, h: i32, u: i32, v: i32) -> (i32, i32) {
    let (t, xs, ys) = TRANSFORM[(o & 7) as usize];
    let a = [w - u, u, h - v, v];
    if t {
        (a[2 + xs as usize], a[ys as usize])
    } else {
        (a[xs as usize], a[2 + ys as usize])
    }
}

/// Where local cell `(x, y)` of a `w × h` piece lands with orientation `o` (relative to the
/// placed piece's corner).
pub fn orient_cell(o: u8, w: i32, h: i32, x: i32, y: i32) -> (i32, i32) {
    let (t, xs, ys) = TRANSFORM[(o & 7) as usize];
    let a = [w - 1 - x, x, h - 1 - y, y];
    if t {
        (a[2 + xs as usize], a[ys as usize])
    } else {
        (a[xs as usize], a[2 + ys as usize])
    }
}

/// Paths compare like the client's resource handles: case-insensitively.
fn norm(p: &Option<String>) -> Option<String> {
    p.as_ref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase().replace('\\', "/"))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TileKey {
    pub edges: [Option<String>; 4],
    pub size: (u8, u8),
    pub grounds: [Option<String>; 4],
    pub heights: [u8; 4],
    pub exits: [u8; 4],
    pub virtual_exits: [u8; 4],
}

impl TileKey {
    pub fn from_tdt(t: &Tdt) -> Self {
        let d = t.edge_distances;
        Self {
            edges: t.edges.clone().map(|e| norm(&e)),
            size: t.size,
            grounds: t.grounds.clone().map(|g| norm(&g)),
            heights: t.unk,
            exits: [d[0], d[1], d[2], d[3]],
            virtual_exits: [d[4], d[5], d[6], d[7]],
        }
    }

    /// The key seen with orientation `o` (`0x141d08bc0`).
    pub fn oriented(&self, o: u8) -> Self {
        let o = (o & 7) as usize;
        let mut k = Self {
            size: if o & 1 == 1 {
                (self.size.1, self.size.0)
            } else {
                self.size
            },
            ..Default::default()
        };
        for i in 0..4 {
            let (g, e) = (CORNERS[o][i], SIDES[o][i]);
            k.grounds[i] = self.grounds[g].clone();
            k.heights[i] = self.heights[g];
            k.edges[i] = self.edges[e].clone();
            k.exits[i] = self.exits[e];
            k.virtual_exits[i] = self.virtual_exits[e];
        }
        if o >= 4 {
            for i in 0..4 {
                let full = if i % 2 == 0 { k.size.0 } else { k.size.1 }.wrapping_mul(3);
                if k.exits[i] != full {
                    let flipped = full.wrapping_sub(k.exits[i]).wrapping_sub(1);
                    k.exits[i] = flipped;
                    if k.virtual_exits[i] != full {
                        k.exits[i] = full.wrapping_sub(k.virtual_exits[i]).wrapping_sub(1);
                        k.virtual_exits[i] = flipped;
                    }
                }
            }
        }
        k
    }
}

/// One `.tst` entry with its parsed header.
#[derive(Debug, Clone)]
pub struct TileDef {
    pub path: String,
    pub tdt: Tdt,
    /// The key the tile is indexed by: the `.tdt`'s, without grounds whose `.gt` has flag 0x10
    /// (`0x141d09a80`, e.g. river trims).
    pub key: TileKey,
    pub weight: u32,
    /// Allowed orientations (bit per orientation).
    pub orientations: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub tile: usize,
    pub orientation: u8,
    pub weight: u32,
}

#[derive(Debug, Default)]
pub struct TileSet {
    pub tiles: Vec<TileDef>,
    index: HashMap<(String, TileKey), Vec<Candidate>>,
    /// The second index (`0x141d09c30`, node `+0x18`): the keys of tiles whose corners with a
    /// height and a ground without `.gt` flag 1 are flattened one after the other (cumulative).
    flat: HashMap<(String, TileKey), Vec<Candidate>>,
    /// `.gt` flags of the grounds the tiles use (lower-case path).
    flags: HashMap<String, u8>,
}

/// Orientation mask of a `.tst` rotation list (empty: all eight).
pub fn orientation_mask(rotations: &[String]) -> u8 {
    if rotations.is_empty() {
        return 0xff;
    }
    rotations.iter().fold(0u8, |m, r| {
        ORIENTATION_NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(r))
            .map_or(m, |o| m | 1 << o)
    })
}

impl TileSet {
    /// Builds the index from tiles in `.tst` order.
    pub fn new(tiles: Vec<TileDef>) -> Self {
        Self::with_flags(tiles, HashMap::new())
    }

    /// With the `.gt` flags of the tile grounds (the second index and the flattening lookup).
    pub fn with_flags(tiles: Vec<TileDef>, flags: HashMap<String, u8>) -> Self {
        let flags: HashMap<String, u8> = flags.into_iter().map(|(k, v)| (k.to_ascii_lowercase().replace('\\', "/"), v)).collect();
        let mut set = Self { tiles: Vec::new(), index: HashMap::new(), flat: HashMap::new(), flags };
        for t in tiles {
            set.add(t);
        }
        set
    }

    /// Adds one tile after the others (`0x141d09c30`): one candidate per allowed orientation, in
    /// both indexes.
    pub fn add(&mut self, t: TileDef) {
        let i = self.tiles.len();
        if t.weight != 0 {
            let flags = &self.flags;
            let no_height = |g: &Option<String>| g.as_ref().is_some_and(|g| flags.get(g).copied().unwrap_or(0x0f) & 1 == 0);
            let feature = t.tdt.feature.clone().unwrap_or_default().to_ascii_lowercase();
            let key = &t.key;
            for o in 0..8u8 {
                if t.orientations & (1 << o) == 0 {
                    continue;
                }
                let c = Candidate { tile: i, orientation: o, weight: t.weight };
                let oriented = key.oriented(o);
                self.index.entry((feature.clone(), oriented.clone())).or_default().push(c);
                let mut k = oriented;
                for src in 0..4usize {
                    if key.heights[src] == 0 || !no_height(&key.grounds[src]) {
                        continue;
                    }
                    let at = (0..4).find(|&j| CORNERS[o as usize][j] == src).expect("permutation");
                    k.heights[at] = 0;
                    self.flat.entry((feature.clone(), k.clone())).or_default().push(c);
                }
            }
        }
        self.tiles.push(t);
    }

    pub fn candidates(&self, feature: &str, key: &TileKey) -> &[Candidate] {
        if let Some(f) = key_dump() {
            use std::io::Write;
            let n = |e: &Option<String>| e.as_deref().map_or("-".to_string(), |s| s.rsplit('/').next().unwrap_or(s).to_string());
            let _ = writeln!(
                f.lock().unwrap(),
                "size [{}, {}] h {:?} x {:?} v {:?}\te {} g {}\t{}",
                key.size.0, key.size.1, key.heights, key.exits, key.virtual_exits,
                key.edges.iter().map(n).collect::<Vec<_>>().join(","),
                key.grounds.iter().map(n).collect::<Vec<_>>().join(","),
                feature
            );
        }
        // `0x141d0a640`: the key, then the flattened-tile index, then the key with the heights
        // of its corners (a ground without `.gt` flag 1) zeroed, again.
        let feature = feature.to_ascii_lowercase();
        let mut k = key.clone();
        if std::env::var_os("POE_NO_KEYNORM").is_none() {
            // A ground with `.gt` flag 0x10 is no ground in a tile's key (`add` stores it as none):
            // the lookup key is read the same way.
            for g in k.grounds.iter_mut() {
                if g.as_ref().is_some_and(|p| self.flags.get(p).copied().unwrap_or(0) & 0x10 != 0) {
                    *g = None;
                }
            }
        }
        loop {
            let probe = (feature.clone(), k);
            if let Some(l) = self.index.get(&probe).filter(|l| !l.is_empty()) {
                return l;
            }
            if let Some(l) = self.flat.get(&probe).filter(|l| !l.is_empty()) {
                return l;
            }
            k = probe.1;
            let mut changed = false;
            for i in 0..4 {
                if k.heights[i] != 0 && k.grounds[i].as_ref().is_some_and(|g| self.flags.get(g).copied().unwrap_or(0x0f) & 1 == 0) {
                    k.heights[i] = 0;
                    changed = true;
                }
            }
            if !changed {
                return &[];
            }
        }
    }
}

/// `POE_DUMP_KEYS=<file>`: every tile lookup in the client trace's `tile_key` format (debug aid
/// to diff the key sequence against a trace; only meaningful on one thread).
fn key_dump() -> Option<&'static std::sync::Mutex<std::fs::File>> {
    static F: std::sync::OnceLock<Option<std::sync::Mutex<std::fs::File>>> = std::sync::OnceLock::new();
    F.get_or_init(|| std::env::var("POE_DUMP_KEYS").ok().and_then(|p| std::fs::File::create(p).ok()).map(std::sync::Mutex::new)).as_ref()
}

/// The weighted pick the generator does (`0x1415d5ae0`): `r` in `0..total`.
pub fn pick(list: &[Candidate], mut r: u32) -> Option<Candidate> {
    for c in list {
        if r < c.weight {
            return Some(*c);
        }
        r -= c.weight;
    }
    list.last().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientations_round_trip() {
        // A 3×2 piece: identity keeps cells, R180 maps the corners onto each other.
        assert_eq!(orient_cell(0, 3, 2, 0, 0), (0, 0));
        assert_eq!(orient_cell(2, 3, 2, 0, 0), (2, 1));
        // Odd orientations transpose: the result fits a 2×3 box.
        for o in [1u8, 3, 5, 7] {
            let (x, y) = orient_cell(o, 3, 2, 2, 1);
            assert!(x < 2 && y < 3, "o={o}: ({x},{y})");
        }
    }

    #[test]
    fn masks() {
        assert_eq!(orientation_mask(&[]), 0xff);
        assert_eq!(orientation_mask(&["FI".into(), "R270".into()]), 0b0001_1000);
    }
}
