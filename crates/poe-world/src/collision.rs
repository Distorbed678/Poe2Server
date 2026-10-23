//! The collision grid (`docs/re/terrain/doodads.md` in the hub): the four 4-bit cell layers the client
//! builds from the generated tile map in `0x141d0e5f0` (phase `terrain_e5f0`), in the `PWG1`
//! format of `poe_game::grid::Grid`.
//!
//! Every layer starts at 5 ("open"). Walking the tile records row by row, each record's sub-tile
//! contributes a plane of cells (the sub-tile's LZ4 hull plane, or a default plane built from its
//! flags), turned by the record's orientation and written into layers 0 (low nibble: walk) and 1
//! (high nibble: projectile) as the minimum with what is there (a 5 counts as unset); records
//! without a sub-tile, or with blocked flags, clear whole 23×23 blocks. Layers 2 and 3 stay at 5
//! in every export checked. Static doodads stamp their hulls into layers 0 and 1 afterwards.

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
pub fn build(map: &TileMap, files: &dyn Files, doodads: &[crate::doodads::Doodad]) -> Result<CellGrid, GenError> {
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
    // Object hulls (any order: the layers only ever take minimums).
    let mut cache = crate::doodads::AoCache::new(files);
    let mut hulls: HashMap<String, std::sync::Arc<Vec<HullSection>>> = HashMap::new();
    for d in doodads {
        if d.leaf.is_empty() || !d.solid {
            continue;
        }
        let key = d.leaf.to_ascii_lowercase();
        if !hulls.contains_key(&key) {
            let info = cache.get(&d.leaf);
            hulls.insert(key.clone(), std::sync::Arc::new(parse_hull(&info.hull)));
        }
        if std::env::var("POE_DBG_HULLS").is_ok() && hulls[&key].iter().any(|s| !s.walk_tri.is_empty() || !s.proj_tri.is_empty() || !s.walk_circle.is_empty() || !s.proj_circle.is_empty()) {
            eprintln!("HULL {} counted {} kind {} leaf {}", d.path.rsplit('/').next().unwrap_or(""), d.counted, d.kind, d.leaf.rsplit('/').next().unwrap_or(""));
        }
        stamp_hull(&mut grid, &hulls[&key], (d.x, d.y), d.angle, d.scale, d.flip);
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


// ---------------------------------------------------------------------------------------------
// Doodad hulls (0x141d104d0): every created object whose .ao has a Hull stamps its walk
// and projectile shapes into layers 0 and 1.

/// 0x1434bfab0: the clearance stamped around a triangle corner, (dx, dy, value).
const CORNER_KERNEL: [(i32, i32, u8); 48] = [(-4,0,4),(-3,-2,4),(-3,-1,3),(-3,0,3),(-3,1,3),(-3,2,4),(-2,-3,4),(-2,-2,3),(-2,-1,2),(-2,0,2),(-2,1,2),(-2,2,3),(-2,3,4),(-1,-3,3),(-1,-2,2),(-1,-1,1),(-1,0,1),(-1,1,1),(-1,2,2),(-1,3,3),(0,-4,4),(0,-3,3),(0,-2,2),(0,-1,1),(0,1,1),(0,2,2),(0,3,3),(0,4,4),(1,-3,3),(1,-2,2),(1,-1,1),(1,0,1),(1,1,1),(1,2,2),(1,3,3),(2,-3,4),(2,-2,3),(2,-1,2),(2,0,2),(2,1,2),(2,2,3),(2,3,4),(3,-2,4),(3,-1,3),(3,0,3),(3,1,3),(3,2,4),(4,0,4)];

/// One hull section's shapes (0x141d6d6b0): triangles x0 y0 x1 y1 x2 y2 and circles
/// x y r, walk and projectile.
#[derive(Debug, Clone, Default)]
pub struct HullSection {
    pub walk_tri: Vec<[f32; 6]>,
    pub proj_tri: Vec<[f32; 6]>,
    pub walk_circle: Vec<[f32; 3]>,
    pub proj_circle: Vec<[f32; 3]>,
}

/// The sections of a Hull block's lines (section = name starts a new one; the first is
/// unnamed).
pub fn parse_hull(lines: &[(String, String)]) -> Vec<HullSection> {
    let mut out = vec![HullSection::default()];
    let nums = |v: &str| v.split_whitespace().filter_map(|n| n.parse::<f32>().ok()).collect::<Vec<_>>();
    for (k, v) in lines {
        let n = nums(v);
        let last = out.last_mut().unwrap();
        match k.as_str() {
            "section" => out.push(HullSection::default()),
            "walk_tri" if n.len() == 6 => last.walk_tri.push(n.try_into().unwrap()),
            "proj_tri" if n.len() == 6 => last.proj_tri.push(n.try_into().unwrap()),
            "walk_circle" if n.len() == 3 => last.walk_circle.push(n.try_into().unwrap()),
            "proj_circle" if n.len() == 3 => last.proj_circle.push(n.try_into().unwrap()),
            _ => {}
        }
    }
    out
}

/// Where a stamp lands: the object's cell, the map bounds and the layer.
struct Stamp<'a> {
    grid: &'a mut CellGrid,
    origin: (i32, i32),
    layer: usize,
}

impl Stamp<'_> {
    fn inside(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.grid.cells_x && (y as usize) < self.grid.cells_y
    }

    /// Zero the nibble at local (x, y) (0x141d1d250).
    fn clear(&mut self, x: i32, y: i32) {
        let (x, y) = (self.origin.0 + x, self.origin.1 + y);
        if self.inside(x, y) {
            let i = y as usize * self.grid.cells_x + x as usize;
            self.grid.layers[self.layer][i] = 0;
        }
    }

    /// Lower the nibble at absolute (x, y) to v (a 5 counts as unset); 0 clears it.
    fn lower(&mut self, x: i32, y: i32, v: i32) {
        if !self.inside(x, y) {
            return;
        }
        let i = y as usize * self.grid.cells_x + x as usize;
        let v = v.max(0) as u8;
        let old = self.grid.layers[self.layer][i];
        let old = if old == 5 { 0xff } else { old };
        self.grid.layers[self.layer][i] = if v == 0 { 0 } else { old.min(v) };
    }
}

type V2 = (f32, f32);

fn sub(a: V2, b: V2) -> V2 {
    (a.0 - b.0, a.1 - b.1)
}

fn dot(a: V2, b: V2) -> f32 {
    a.1 * b.1 + a.0 * b.0
}

fn len(a: V2) -> f32 {
    (a.0 * a.0 + a.1 * a.1).sqrt()
}

fn normalize(a: V2) -> V2 {
    let l = len(a);
    if l > 0.0 {
        let inv = 1.0 / l;
        (a.0 * inv, a.1 * inv)
    } else {
        (0.0, 0.0)
    }
}

/// 0x141d1d250 / 0x141d18d60: the cells (4-bit fixed point, top-left rule) a triangle covers,
/// as local (x, y).
fn scan(a: V2, b: V2, c: V2, mut act: impl FnMut(i32, i32)) {
    let fx = |v: f32| (v * 16.0 + 0.5).floor() as i32;
    let (ax0, ay0, bx, by, cx0, cy0) = (fx(a.0), fx(a.1), fx(b.0), fx(b.1), fx(c.0), fx(c.1));
    let (mut ax, mut ay, mut cx, mut cy) = (ax0, ay0, cx0, cy0);
    if b.0 * c.1 + a.0 * b.1 + c.0 * a.1 <= a.0 * c.1 + b.0 * a.1 + c.0 * b.1 {
        (ax, ay, cx, cy) = (cx0, cy0, ax0, ay0);
    }
    let (d3, d21, d12) = (bx - ax, cx - bx, ax - cx);
    let (d10, d16, d11) = (cy - by, by - ay, ay - cy);
    let xmin = (ax.min(bx).min(cx) + 15) >> 4;
    let xmax = (ax.max(bx).max(cx) + 15) >> 4;
    let ymin = (ay.min(by).min(cy) + 15) >> 4;
    let ymax = (ay.max(by).max(cy) + 15) >> 4;
    if ymax == ymin || xmax == xmin {
        return;
    }
    let mut e0 = cx * d10 - cy * d21;
    let mut e1 = bx * d16 - by * d3;
    let mut e2 = ax * d11 - ay * d12;
    if d10 < 0 || (d10 == 0 && d21 > 0) {
        e0 += 1;
    }
    if d16 < 0 || (d16 == 0 && d3 > 0) {
        e1 += 1;
    }
    if d11 < 0 || (d11 == 0 && d12 > 0) {
        e2 += 1;
    }
    let (y16, x16) = (ymin * 16, xmin * 16);
    e0 += y16 * d21 - x16 * d10;
    e1 += y16 * d3 - x16 * d16;
    e2 += y16 * d12 - x16 * d11;
    for y in ymin..=ymax {
        let (mut r0, mut r1, mut r2) = (e0, e1, e2);
        for x in xmin..=xmax {
            if r0 >= 0 && r1 >= 0 && r2 >= 0 {
                act(x, y);
            }
            r0 += d10 * -16;
            r1 += d16 * -16;
            r2 += d11 * -16;
        }
        e0 += d21 * 16;
        e1 += d3 * 16;
        e2 += d12 * 16;
    }
}

/// 0x141d10be0: one triangle of the object at its origin.
fn stamp_triangle(st: &mut Stamp, v: [V2; 3]) {
    if v[0] == v[1] || v[0] == v[2] || v[1] == v[2] {
        return;
    }
    scan(v[0], v[1], v[2], |x, y| st.clear(x, y));
    for i in 0..3 {
        let cur = v[i];
        let next = v[(i + 1) % 3];
        let third = v[(i + 2) % 3];
        let (vx, vy) = (st.origin.0 + cur.0 as i32, st.origin.1 + cur.1 as i32);
        if st.inside(vx, vy) {
            let j = vy as usize * st.grid.cells_x + vx as usize;
            st.grid.layers[st.layer][j] = 0;
        }
        for &(dx, dy, kv) in &CORNER_KERNEL {
            st.lower(vx + dx, vy + dy, kv as i32);
        }
        let dir = normalize(sub(next, cur));
        let mut off = (-dir.1 * 4.0, dir.0 * 4.0);
        if dot(off, sub(third, cur)) >= 0.0 {
            off = (-off.0, -off.1);
        }
        let cur_o = (cur.0 + off.0, cur.1 + off.1);
        let next_o = (next.0 + off.0, next.1 + off.1);
        let edge = |a: V2, b: V2, c: V2, st: &mut Stamp| {
            scan(a, b, c, |x, y| {
                let (ax, ay) = (st.origin.0 + x, st.origin.1 + y);
                if !st.inside(ax, ay) {
                    return;
                }
                let delta = sub((x as f32, y as f32), cur);
                let p = dot(dir, delta);
                let perp = sub(delta, (dir.0 * p, dir.1 * p));
                let d = (len(perp) + 0.5).floor() as i32;
                st.lower(ax, ay, d);
            });
        };
        edge(cur, cur_o, next, st);
        edge(cur_o, next_o, next, st);
    }
}

/// 0x141d19370: one circle.
fn stamp_circle(st: &mut Stamp, c: V2, r: f32) {
    let (cx, cy) = ((c.0 + 0.5).floor() as i32, (c.1 + 0.5).floor() as i32);
    let reach = (r + 5.0) - 1.0;
    let n = (reach + 0.5).floor() as i32;
    let limit = (reach * reach + 0.5).floor() as i32;
    for dy in -n..=n {
        for dx in -n..=n {
            if dx * dx + dy * dy < limit {
                let (x, y) = (dx + cx, dy + cy);
                let (ax, ay) = (st.origin.0 + x, st.origin.1 + y);
                if st.inside(ax, ay) {
                    let d = len(sub((x as f32, y as f32), c)) - r;
                    st.lower(ax, ay, d.ceil() as i32);
                }
            }
        }
    }
}

/// Stamps an object's hull at cell (x, y) with angle, scale and mirroring (0x141d104d0:
/// walk and projectile shapes into layer 0, projectile shapes into layer 1).
pub fn stamp_hull(grid: &mut CellGrid, sections: &[HullSection], (x, y): (i32, i32), angle: f32, scale: f32, flip: bool) {
    let angle = if angle >= std::f32::consts::TAU { angle - std::f32::consts::TAU } else { angle };
    let (s, c) = (angle.sin(), angle.cos());
    let f9 = c * scale;
    let f12 = -s * scale;
    let (mut f10, mut f11) = (f9, s * scale);
    if flip {
        f10 = -f9;
        f11 = -f11;
    }
    let tf = |p: V2| (p.0 * f10 + p.1 * f12, p.0 * f11 + p.1 * f9);
    for (layer, kind) in [(0usize, 0usize), (0, 1), (1, 1)] {
        let mut st = Stamp { grid, origin: (x, y), layer };
        for sec in sections {
            let tris = if kind == 0 { &sec.walk_tri } else { &sec.proj_tri };
            for t in tris {
                let f = flip as usize;
                let v = [tf((t[0], t[1])), tf((t[2 + f * 2], t[3 + f * 2])), tf((t[4 - f * 2], t[5 - f * 2]))];
                stamp_triangle(&mut st, v);
            }
            let circles = if kind == 0 { &sec.walk_circle } else { &sec.proj_circle };
            for ci in circles {
                stamp_circle(&mut st, tf((ci[0], ci[1])), scale * ci[2]);
            }
        }
    }
}

/// The cells a hull stamps (layer, absolute x, y, value) for an object at `(x, y)`, with the
/// window `grid` as scratch (for tools).
pub fn hull_footprint(sections: &[HullSection], (x, y): (i32, i32), angle: f32, scale: f32, flip: bool, cells: (usize, usize)) -> Vec<(usize, i32, i32, u8)> {
    let (ox, oy) = (64, 64);
    let mut g = CellGrid::new(128 + cells.0.min(0), 128 + cells.1.min(0));
    stamp_hull(&mut g, sections, (ox, oy), angle, scale, flip);
    let mut out = Vec::new();
    for l in 0..2 {
        for gy in 0..g.cells_y {
            for gx in 0..g.cells_x {
                let v = g.get(l, gx, gy);
                if v != 5 {
                    out.push((l, x + gx as i32 - ox, y + gy as i32 - oy, v));
                }
            }
        }
    }
    out
}
