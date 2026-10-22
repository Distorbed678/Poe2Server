//! The collision grid (`docs/re/terrain/collision.md`): the four 4-bit cell layers the client
//! builds from the generated tile map in `0x141d0e5f0` (phase `terrain_e5f0`), in the `PWG1`
//! format of `poe_game::grid::Grid`.
//!
//! Every layer starts at 5 ("open"). Walking the tile records row by row, each record's sub-tile
//! contributes a plane of cells (the sub-tile's LZ4 hull plane, or a default plane built from its
//! flags), turned by the record's orientation and written into layers 0 (low nibble: walk) and 1
//! (high nibble: projectile) as the minimum with what is there (a 5 counts as unset); records
//! without a sub-tile, or with blocked flags, clear whole 23×23 blocks. Layers 2 and 3 stay at 5
//! until doodads write to them.

use std::collections::HashMap;

use crate::{
    generate::{Files, GenError, TileMap, load_tdt},
    tdt::{SubTile, Tdt, lz4_decode},
};

/// Cells per tile side.
pub const TILE: i32 = 23;

/// The four layers, one byte per cell holding a nibble.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellGrid {
    pub cells_x: usize,
    pub cells_y: usize,
    pub layers: [Vec<u8>; 4],
}

impl CellGrid {
    pub fn new(cells_x: usize, cells_y: usize) -> Self {
        Self { cells_x, cells_y, layers: std::array::from_fn(|_| vec![5; cells_x * cells_y]) }
    }

    pub fn get(&self, layer: usize, x: usize, y: usize) -> u8 {
        self.layers[layer][y * self.cells_x + x]
    }

    /// The `PWG1` bytes: header (magic, cells x, cells y, bytes per row, layer count), then each
    /// layer at 4 bits per cell, low nibble first.
    pub fn pwg1(&self) -> Vec<u8> {
        let bpr = self.cells_x.div_ceil(2);
        let mut out = b"PWG1".to_vec();
        for v in [self.cells_x, self.cells_y, bpr, 4] {
            out.extend_from_slice(&(v as u32).to_le_bytes());
        }
        for l in &self.layers {
            for y in 0..self.cells_y {
                for bx in 0..bpr {
                    let lo = l[y * self.cells_x + bx * 2] & 15;
                    let hi = if bx * 2 + 1 < self.cells_x { l[y * self.cells_x + bx * 2 + 1] & 15 } else { 5 };
                    out.push(lo | hi << 4);
                }
            }
        }
        out
    }
}

/// A plane of cell codes (low nibble layer 0, high nibble layer 1) over `x0..x0+w`, `y0..y0+h`
/// of a tile (the tile is 0..23; planes may reach 4 cells past its sides).
struct Plane {
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    data: Vec<u8>,
}

/// `0x141d04ed0` + `0x1429173d0`: the plane of a sub-tile without a hull list.
fn default_plane(f: u8, b1: u8) -> Plane {
    let x0 = if b1 & 1 != 0 { -4 } else { 0 };
    let xe = if f & 0x40 != 0 { 27 } else { 23 };
    let y0 = if f & 0x20 != 0 { -4 } else { 0 };
    let ye = if f & 0x80 != 0 { 27 } else { 23 };
    let (w, h) = (xe - x0, ye - y0);
    let fill = (if f & 8 != 0 { 0 } else { 5 }) | (if f & 0x10 != 0 { 0 } else { 0x50 });
    let mut data = vec![fill; (w * h) as usize];
    let (lo_b, hi_b) = (f & 8 != 0, f & 0x10 != 0);
    if lo_b || hi_b {
        let put = |data: &mut Vec<u8>, i: usize, v: u8, max: bool| {
            let old = data[i];
            let lo = if !lo_b { 5 } else if max { (old & 15).max(v) } else { v };
            let hi = if !hi_b { 0x50 } else if max { (old >> 4).max(v) << 4 } else { v << 4 };
            data[i] = lo | hi;
        };
        if f & 0x20 != 0 {
            for row in 0..4 {
                for col in 0..w {
                    put(&mut data, (row * w + col) as usize, (4 - row) as u8, false);
                }
            }
        }
        if f & 0x80 != 0 {
            for row in h - 4..h {
                for col in 0..w {
                    put(&mut data, (row * w + col) as usize, (row - h + 5) as u8, false);
                }
            }
        }
        if b1 & 1 != 0 {
            for row in 0..h {
                for col in 0..4 {
                    put(&mut data, (row * w + col) as usize, (4 - col) as u8, true);
                }
            }
        }
        if f & 0x40 != 0 {
            for row in 0..h {
                for col in w - 4..w {
                    put(&mut data, (row * w + col) as usize, (col - w + 5) as u8, true);
                }
            }
        }
    }
    Plane { x0, y0, w, h, data }
}

/// `0x141d05040`: the plane for orientation `o`: the default plane without a list, else plane
/// `index` of the sub-tile's LZ4 stack.
fn plane(st: &SubTile, index: usize) -> Option<Plane> {
    let (f, b1) = (st.flags, st.b1);
    if f & 1 != 0 && f & 2 != 0 {
        return Some(default_plane(f, b1));
    }
    let x0 = if b1 & 1 != 0 { -4 } else { 0 };
    let w = (if f & 0x40 != 0 { 27 } else { 23 }) + if b1 & 1 != 0 { 4 } else { 0 };
    let y0 = if f & 0x20 != 0 { -4 } else { 0 };
    let h = (if f & 0x80 != 0 { 27 } else { 23 }) + if f & 0x20 != 0 { 4 } else { 0 };
    let raw = lz4_decode(&st.list)?;
    let size = (w * h) as usize;
    let data = raw.get(index * size..(index + 1) * size)?.to_vec();
    Some(Plane { x0, y0, w, h, data })
}

/// `0x142f020c8`: the cell of tile-local `(x, y)` (a 23×23 tile, positions may lie outside)
/// under orientation `o`.
fn turn(x: i32, y: i32, o: u8) -> (i32, i32) {
    const T: [[usize; 3]; 9] = [[0, 1, 1], [1, 0, 1], [0, 0, 0], [1, 1, 0], [0, 0, 1], [1, 0, 0], [0, 1, 0], [1, 1, 1], [0, 1, 1]];
    let a = [TILE - 1 - x, x, TILE - 1 - y, y];
    let t = T[(o as usize).min(8)];
    (a[t[1] + t[0] * 2], a[t[2] + if t[0] == 0 { 2 } else { 0 }])
}

/// Builds the cell grid of a generated tile map (`files` read the tiles' `.tdt`).
pub fn build(map: &TileMap, files: &dyn Files) -> Result<CellGrid, GenError> {
    let (w, h) = (map.width as i32, map.height as i32);
    let mut grid = CellGrid::new((w * TILE) as usize, (h * TILE) as usize);
    let mut tdts: HashMap<String, Tdt> = HashMap::new();
    // The sub-tile flags of every record (None: no sub-tile), for the margin test of blocked tiles.
    let mut rec_flags: Vec<Option<u8>> = vec![None; (w * h) as usize];
    for ty in 0..h {
        for tx in 0..w {
            if let Some(c) = map.get(tx as u32, ty as u32) {
                let key = c.path.to_ascii_lowercase();
                if !tdts.contains_key(&key) {
                    tdts.insert(key.clone(), load_tdt(files, &c.path)?);
                }
                rec_flags[(ty * w + tx) as usize] = tdts[&key].subtiles.get(c.sub as usize).map(|s| s.flags);
            }
        }
    }
    let unwalkable = |x: i32, y: i32| map.point_index(x, y).is_none_or(|i| map.lattice[i].flags & 8 != 0);
    for ty in 0..h {
        for tx in 0..w {
            let corners = [(tx, ty), (tx + 1, ty), (tx, ty + 1), (tx + 1, ty + 1)].iter().filter(|&&(x, y)| unwalkable(x, y)).count();
            let cell = map.get(tx as u32, ty as u32);
            let entry = match cell {
                Some(c) => {
                    let key = c.path.to_ascii_lowercase();
                    if !tdts.contains_key(&key) {
                        tdts.insert(key.clone(), load_tdt(files, &c.path)?);
                    }
                    tdts[&key].subtiles.get(c.sub as usize).map(|s| (c, &tdts[&key], s))
                }
                None => None,
            };
            let Some((c, tdt, st)) = entry else {
                clear(&mut grid, &rec_flags, w, h, tx, ty, true, true);
                continue;
            };
            let f = st.flags;
            let o = c.orientation;
            if std::env::var("POE_DBG_GRID_TILE").is_ok_and(|v| v == format!("{tx},{ty}")) {
                eprintln!("tile ({tx},{ty}) {} sub {} flags {f:#x} b1 {} o{o} corners {corners} lattice {:?}", c.path, c.sub, st.b1,
                    [(tx, ty), (tx + 1, ty), (tx, ty + 1), (tx + 1, ty + 1)].map(|(x, y)| map.point_index(x, y).map(|i| map.lattice[i].flags)));
            }
            // The hull plane serving this orientation.
            let masks: &[u8] = if tdt.hull_masks.is_empty() { &[0] } else { &tdt.hull_masks };
            let index = masks.iter().position(|&m| m == 0 || m >> o & 1 != 0).unwrap_or(masks.len() - 1);
            if std::env::var("POE_DBG_GRID_TILE").is_ok_and(|v| v == format!("{tx},{ty}")) {
                if let Some(p) = plane(st, index) {
                    eprintln!("plane x0 {} y0 {} {}x{}", p.x0, p.y0, p.w, p.h);
                    for y in 0..p.h {
                        eprintln!("  {}", (0..p.w).map(|x| format!("{:02x} ", p.data[(y * p.w + x) as usize])).collect::<String>());
                    }
                }
            }
            let apply = |grid: &mut CellGrid, p: &Plane, lo: bool, hi: bool| {
                for py in 0..p.h {
                    for px in 0..p.w {
                        let v = p.data[(py * p.w + px) as usize];
                        let (lx, ly) = turn(p.x0 + px, p.y0 + py, o);
                        let (cx, cy) = ((tx * TILE + lx).clamp(0, w * TILE - 1), (ty * TILE + ly).clamp(0, h * TILE - 1));
                        let i = cy as usize * grid.cells_x + cx as usize;
                        for (layer, on, nib) in [(0, lo, v & 15), (1, hi, v >> 4)] {
                            if on {
                                let old = grid.layers[layer][i];
                                let old = if old == 5 { 0xff } else { old };
                                grid.layers[layer][i] = old.min(nib);
                            }
                        }
                    }
                }
            };
            if f & 2 == 0 {
                if f & 1 == 0 && corners < 4 {
                    if let Some(p) = plane(st, index) {
                        apply(&mut grid, &p, true, true);
                    }
                } else {
                    if f & 8 != 0 || corners > 3 {
                        clear(&mut grid, &rec_flags, w, h, tx, ty, true, false);
                    }
                    if let Some(p) = plane(st, index) {
                        apply(&mut grid, &p, false, true);
                    }
                }
            } else if f & 0x10 != 0 {
                clear(&mut grid, &rec_flags, w, h, tx, ty, true, true);
            } else if f & 1 == 0 && corners < 4 {
                if let Some(p) = plane(st, index) {
                    apply(&mut grid, &p, true, false);
                }
            } else if f & 8 != 0 || corners > 3 {
                clear(&mut grid, &rec_flags, w, h, tx, ty, true, false);
            }
        }
    }
    Ok(grid)
}

/// `0x141d0f490` on a whole tile: the layers named by the mask (bit 3 layer 0, bit 4 layer 1)
/// are blocked over its 23×23 block, and the blocked plane also reaches 4 cells into each
/// neighbour (up, right, down, left) that has a sub-tile open on one of those layers
/// (`0x141d0fa60`), with the default plane's 1..4 ramp.
fn clear(grid: &mut CellGrid, rec: &[Option<u8>], w: i32, h: i32, tx: i32, ty: i32, l0: bool, l1: bool) {
    let mut f = (if l0 { 8 } else { 0 }) | (if l1 { 0x10 } else { 0 });
    let mut b1 = 0;
    for (k, (dx, dy)) in [(0, -1), (1, 0), (0, 1), (-1, 0)].into_iter().enumerate() {
        let (nx, ny) = (tx + dx, ty + dy);
        if nx < 0 || ny < 0 || nx >= w || ny >= h {
            continue;
        }
        let Some(nf) = rec[(ny * w + nx) as usize] else { continue };
        if (f & 8 != 0 && nf & 8 == 0) || (f & 0x10 != 0 && nf & 0x10 == 0) {
            match k {
                0 => f |= 0x20,
                1 => f |= 0x40,
                2 => f |= 0x80,
                _ => b1 |= 1,
            }
        }
    }
    // The margin tests above read the base mask only, as the client's bits are tested one by one
    // against the same layer bits.
    let p = default_plane(f, b1);
    for py in 0..p.h {
        for px in 0..p.w {
            let v = p.data[(py * p.w + px) as usize];
            let (cx, cy) = (tx * TILE + p.x0 + px, ty * TILE + p.y0 + py);
            let i = cy as usize * grid.cells_x + cx as usize;
            for (layer, on, nib) in [(0, l0, v & 15), (1, l1, v >> 4)] {
                if on {
                    let old = grid.layers[layer][i];
                    let old = if old == 5 { 0xff } else { old };
                    grid.layers[layer][i] = old.min(nib);
                }
            }
        }
    }
}
