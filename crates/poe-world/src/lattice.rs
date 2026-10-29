//! The corner lattice passes between edge tiles and the fill pass (`0x1420bedc0`): grounds
//! flood out from tile borders into empty space, stopped by tile edges. Findings:
//! `docs/re/terrain/shared-passes.md`, "Corner lattice".

use std::collections::VecDeque;

use crate::{
    generate::TileMap,
    tileset::{TileKey, TileSet},
};

/// Lattice steps per direction (`0x142f0c660`): north, east, south, west.
const DIR: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

/// The tile on a cell: its origin, oriented size, offset of the cell and piece index.
fn tile_at(map: &TileMap, x: i32, y: i32) -> Option<((i32, i32), (i32, i32), (i32, i32), usize)> {
    if x < 0 || y < 0 || x as u32 >= map.width || y as u32 >= map.height {
        return None;
    }
    let i = map.index(x, y)?;
    let c = map.cells[i].as_ref()?;
    let p = map.piece_at[i]?;
    let piece = &map.pieces[p];
    let size = TileMap::oriented_size(piece.size, piece.orientation);
    let off = (c.offset.0 as i32, c.offset.1 as i32);
    Some(((x - off.0, y - off.1), size, off, p))
}

/// `0x141fe0390`: the corner lies strictly inside a placed tile.
fn inside(map: &TileMap, p: (i32, i32)) -> bool {
    tile_at(map, p.0, p.1).is_some_and(|(o, (w, h), _, _)| o.0 < p.0 && p.0 < o.0 + w && o.1 < p.1 && p.1 < o.1 + h)
}

/// `0x141fe0160`: the tile's edge on `side` crosses that side of this cell.
fn edge_crosses(map: &TileMap, tiles: &TileSet, cell: (i32, i32), side: usize) -> bool {
    let Some((_, (w, h), off, p)) = tile_at(map, cell.0, cell.1) else { return false };
    let piece = &map.pieces[p];
    let Some(tile) = piece.tile else { return false };
    let tdt = &tiles.tiles[tile].tdt;
    let o = piece.orientation;
    let key = TileKey::from_tdt(tdt).oriented(o);
    let pos = match side {
        0 => off.0,
        1 => off.1,
        2 => w - 1 - off.0,
        _ => h - 1 - off.1,
    };
    let Some(edge) = &key.edges[side] else { return false };
    let flags = map.edge_flags.get(edge).copied().unwrap_or(0);
    if flags & 0x80 != 0 {
        return false;
    }
    if key.exits[side] as i32 / 3 == pos {
        return true;
    }
    flags & 4 != 0 && crate::edge_tiles::virtual_exit(tdt, o, side) / 3 == pos
}

/// `0x141fe0990` ("EdgeInTheWay"): a tile edge crosses the lattice segment `from → to`.
fn edge_in_the_way(map: &TileMap, tiles: &TileSet, from: (i32, i32), to: (i32, i32), dir: usize) -> bool {
    if inside(map, from) {
        return !inside(map, to);
    }
    if inside(map, to) {
        return false;
    }
    let c = |p: (i32, i32), s: usize| edge_crosses(map, tiles, p, s);
    match dir {
        0 => c(to, 3) || c((to.0 - 1, to.1), 1),
        1 => c(from, 0) || c((from.0, from.1 - 1), 2),
        2 => c(from, 3) || c((from.0 - 1, from.1), 1),
        _ => c(to, 0) || c((to.0, to.1 - 1), 2),
    }
}

fn in_lattice(map: &TileMap, p: (i32, i32)) -> bool {
    p.0 >= 0 && p.1 >= 0 && p.0 as u32 <= map.width && p.1 as u32 <= map.height
}

/// `0x1420b9300`: a breadth-first flood of the ground at border corner `p` outward in `dir`
/// (and to either side), over corners no tile set; grounds with `.gt` flag 8 also cover tile
/// corners without a ground.
fn flood(map: &mut TileMap, tiles: &TileSet, p: (i32, i32), dir: usize) {
    let Some(i) = map.point_index(p.0, p.1) else { return };
    let Some(g) = map.lattice[i].ground.clone() else { return };
    let gf = map.gt_flags(&g);
    if gf & 2 == 0 {
        return;
    }
    let blank = gf & 8 != 0;
    let mut queue = VecDeque::new();
    let q = (p.0 + DIR[dir].0, p.1 + DIR[dir].1);
    if in_lattice(map, q) && !edge_in_the_way(map, tiles, p, q, dir) {
        queue.push_back((q, dir));
    }
    while let Some((a, d)) = queue.pop_front() {
        let k = map.point_index(a.0, a.1).unwrap();
        let pt = &map.lattice[k];
        let free = pt.flags & 1 == 0 || (blank && pt.ground.is_none());
        if !free {
            continue;
        }
        let pt = &mut map.lattice[k];
        pt.ground = Some(g.clone());
        pt.flags |= 1;
        for nd in [d, (d + 1) & 3, (d + 3) & 3] {
            let n = (a.0 + DIR[nd].0, a.1 + DIR[nd].1);
            if in_lattice(map, n) && !edge_in_the_way(map, tiles, a, n, nd) {
                queue.push_back((n, nd));
            }
        }
    }
}

/// `0x1420b9650`: floods from every border corner of every placed tile (columns, then rows;
/// per tile its top and bottom corners left to right, then its left and right corners).
pub fn border_floods(map: &mut TileMap, tiles: &TileSet) {
    for x in 0..map.width as i32 {
        for y in 0..map.height as i32 {
            let Some((o, (w, h), off, _)) = tile_at(map, x, y) else { continue };
            if off != (0, 0) || o != (x, y) {
                continue;
            }
            for i in 0..=w {
                flood(map, tiles, (x + i, y), 0);
                flood(map, tiles, (x + i, y + h), 2);
            }
            for j in 0..=h {
                flood(map, tiles, (x, y + j), 3);
                flood(map, tiles, (x + w, y + j), 1);
            }
        }
    }
}

const IMPASSABLE: &str = "metadata/terrain/impassable_ground.gt";

/// State of the unwalkable marking (`0x141fe1150`): visited corners, the flood queue and the
/// corners whose neighbours get marked when the queue runs dry.
struct Unwalkable<'a> {
    map: &'a mut TileMap,
    tiles: &'a TileSet,
    visited: Vec<bool>,
    queue: VecDeque<((i32, i32), usize)>,
    edge_points: Vec<(i32, i32)>,
}

impl Unwalkable<'_> {
    fn ground(&self, p: (i32, i32)) -> Option<&String> {
        self.map.point_index(p.0, p.1).and_then(|i| self.map.lattice[i].ground.as_ref())
    }

    /// A ground that stops the impassable flood: set, without `.gt` flag 4, not impassable.
    fn stops(&self, p: (i32, i32)) -> bool {
        self.ground(p).is_some_and(|g| self.map.gt_flags(g) & 4 == 0 && g != IMPASSABLE)
    }

    fn mark(&mut self, p: (i32, i32)) {
        if let Some(i) = self.map.point_index(p.0, p.1) {
            self.map.lattice[i].flags |= 8;
        }
    }

    /// `+0x16f` bit 0 of the tile a corner lies inside (not read from `.tdt` files yet).
    fn sealed_inside(&self, p: (i32, i32)) -> bool {
        inside(self.map, p) && false
    }

    /// `0x141fe2500`: start the flood at a border corner whose ground does not stop it.
    fn start(&mut self, p: (i32, i32), dir: usize) {
        if self.stops(p) {
            return;
        }
        let q = (p.0 + DIR[dir].0, p.1 + DIR[dir].1);
        if self.sealed_inside(q) {
            return;
        }
        self.queue.push_back((p, dir));
        self.run();
    }

    /// `0x141fe1f70`: look one step on; a stopping ground is marked (and its neighbours later),
    /// anything else joins the flood unless an edge is in the way.
    fn step(&mut self, p: (i32, i32), dir: usize) {
        let n = (p.0 + DIR[dir].0, p.1 + DIR[dir].1);
        if !in_lattice(self.map, n) || self.sealed_inside(n) {
            return;
        }
        if self.stops(n) {
            self.mark(n);
            self.edge_points.push(n);
            return;
        }
        if !edge_in_the_way(self.map, self.tiles, p, n, dir) {
            self.queue.push_back((n, dir));
        }
    }

    /// `0x141fe2280`: the impassable flood (corners take the impassable ground where they have
    /// none), then the stopping corners' walkable neighbours are marked (`0x141fe2160`).
    fn run(&mut self) {
        while let Some((p, d)) = self.queue.pop_front() {
            let k = self.map.point_index(p.0, p.1).unwrap();
            if self.visited[k] {
                continue;
            }
            self.visited[k] = true;
            let pt = &mut self.map.lattice[k];
            if pt.ground.is_none() || pt.flags & 1 == 0 {
                pt.ground = Some(IMPASSABLE.to_string());
                pt.flags |= 1;
            }
            pt.flags |= 8;
            self.step(p, d);
            self.step(p, (d + 3) & 3);
            self.step(p, (d + 1) & 3);
        }
        for (x, y) in std::mem::take(&mut self.edge_points) {
            for n in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                if in_lattice(self.map, n) && self.ground(n).is_some_and(|g| self.map.gt_flags(g) & 4 == 0) {
                    self.mark(n);
                }
            }
        }
    }
}

/// `0x141fe1150`: lattice flag 8 marks corners nobody can walk on: the impassable flood from
/// the border, the border itself, impassable grounds inside, and one ring of corners next to
/// marked ones (unless their ground has `.gt` flag 4). Area transition tiles (`0x141fe0cc0`)
/// and the tile-set option at `+0x165` are not ported yet (nothing in the Riverbank).
pub fn mark_unwalkable(map: &mut TileMap, tiles: &TileSet) {
    let (w, h) = (map.width as i32, map.height as i32);
    let n = map.lattice.len();
    let mut s = Unwalkable { map, tiles, visited: vec![false; n], queue: VecDeque::new(), edge_points: Vec::new() };
    for x in 0..=w {
        s.start((x, 0), 2);
        s.start((x, h), 0);
    }
    for y in 0..=h {
        s.start((0, y), 1);
        s.start((w, y), 3);
    }
    for x in 0..=w {
        s.mark((x, 0));
        s.mark((x, h));
    }
    for y in 0..=h {
        s.mark((0, y));
        s.mark((w, y));
    }
    for y in 1..h {
        for x in 1..w {
            let k = s.map.point_index(x, y).unwrap();
            if s.ground((x, y)).is_some_and(|g| g == IMPASSABLE) && s.map.lattice[k].flags & 8 == 0 {
                s.mark((x, y));
                s.visited[k] = true;
                for d in 0..4 {
                    s.step((x, y), d);
                }
                s.run();
            }
        }
    }
    let marked = |m: &TileMap, p: (i32, i32)| m.point_index(p.0, p.1).is_some_and(|i| m.lattice[i].flags & 8 != 0);
    let mut ring = Vec::new();
    for y in 1..h {
        for x in 1..w {
            let k = s.map.point_index(x, y).unwrap();
            let Some(g) = s.ground((x, y)) else { continue };
            if s.map.gt_flags(g) & 4 != 0 || s.map.lattice[k].flags & 8 != 0 {
                continue;
            }
            if [(x, y - 1), (x - 1, y), (x + 1, y), (x, y + 1)].iter().any(|&p| marked(s.map, p)) {
                ring.push((x, y));
            }
        }
    }
    for p in ring {
        s.mark(p);
    }
}

/// `0x141fe0cc0` (end of `0x141fe1150`): a tile whose edge types are empty or wildcards and with one ground on all
/// four corners (a fill tile; not an inheriting `.tdt`) whose footprint corners on the outline
/// are all unwalkable (flag 8, or off the map) becomes unwalkable inside as well: every corner
/// of its footprint gets flag 8. Tiles are visited row by row, so earlier marks count for later
/// tiles. (`0x141fe0b30` is the outline test, `0x141fd5dd0` the tile test.)
pub fn mark_enclosed_fills(map: &mut TileMap, files: &dyn crate::generate::Files) -> Result<(), crate::generate::GenError> {
    let (w, h) = (map.width as i32, map.height as i32);
    let mut tdts: std::collections::HashMap<String, crate::tdt::Tdt> = std::collections::HashMap::new();
    let mut wildcards: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for ty in 0..h {
        for tx in 0..w {
            let Some(c) = map.get(tx as u32, ty as u32) else { continue };
            if c.offset != (0, 0) {
                continue;
            }
            let key = c.path.to_ascii_lowercase();
            if !tdts.contains_key(&key) {
                tdts.insert(key.clone(), crate::generate::load_tdt(files, &c.path)?);
            }
            let t = &tdts[&key];
            let open = |m: &TileMap, x: i32, y: i32| m.point_index(x, y).is_some_and(|i| m.lattice[i].flags & 8 == 0);
            let uniform = t.grounds.iter().all(|g| g == &t.grounds[0]);
            if t.inherits.is_some() || !uniform {
                continue;
            }
            // `0x141ef2570`: an edge reference passes when it is empty or names a wildcard `.et`.
            let mut edgeless = true;
            for e in t.edges.iter().flatten() {
                let ek = e.to_ascii_lowercase();
                if !wildcards.contains_key(&ek) {
                    let et = crate::rooms::EdgeType::parse(&crate::text::decode_utf16(&files.read(&ek)?));
                    wildcards.insert(ek.clone(), et.wildcard);
                }
                edgeless &= wildcards[&ek];
            }
            if !edgeless {
                continue;
            }
            // Footprint in corners: the tile's size, turned by odd orientations.
            let (tw, th) = if c.orientation & 1 != 0 && c.orientation <= 7 { (t.size.1 as i32, t.size.0 as i32) } else { (t.size.0 as i32, t.size.1 as i32) };
            let enclosed = (0..=tw).all(|i| !open(map, tx + i, ty) && !open(map, tx + i, ty + th))
                && (1..th).all(|j| !open(map, tx, ty + j) && !open(map, tx + tw, ty + j));
            if enclosed {
                for y in ty..=ty + th {
                    for x in tx..=tx + tw {
                        if let Some(i) = map.point_index(x, y) {
                            map.lattice[i].flags |= 8;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// `0x1420b99a0` (height offsets not ported: the Riverbank has none): every corner is set; one
/// without a ground (or never set) takes the graph's default ground (graph +0xb0) if any.
pub fn settle(map: &mut TileMap, default_ground: Option<&str>) {
    for p in &mut map.lattice {
        if p.flags & 1 == 0 || p.ground.is_none() {
            if let Some(g) = default_ground {
                p.ground = Some(g.to_ascii_lowercase());
            }
            p.flags |= 1;
        }
    }
}

/// A lattice height as the client reads it (a signed byte; below -120 wraps up by 256).
fn lat_height(map: &TileMap, k: usize) -> i32 {
    let h = map.lattice[k].height as i32;
    if h < -0x78 { h + 0x100 } else { h }
}

/// The `.tgr` height pass `0x1420c0be0` (after the border floods, before the unwalkable
/// marking): (1) per tile origin not yet reached, the fix-up flood `0x1420c0870`: each tile
/// takes the height its reached corners imply (`0x1420bfe20`: corners whose tile ground is
/// missing or takes heights, `.gt` flag 1; else all four outer corners; the lowest of several)
/// and spreads to the neighbours across such corners (`0x1420c3cc0`); (2) when the map has a
/// non-zero height (view `+0x20`): every corner and tile height drops by the lowest tile height
/// and every corner gets flag 2; (3) unreached corners with exactly one reached neighbour take
/// its height until nothing changes (`0x1420c0f10`). A tile with no such corner reached takes
/// the heights implied through its real (not wildcard, `.et` `+0x48` bit 7 clear) edges: the
/// reached neighbour cell at each edge's exit gives its height minus the tile's corner height
/// plus its own at the shared corner; the flood also crosses such edges. Not ported: the room
/// heights (`0x1420c1080`).
pub fn height_pass(map: &mut TileMap, files: &dyn crate::generate::Files) -> Result<(), crate::generate::GenError> {
    let (w, h) = (map.width as i32, map.height as i32);
    let mut tdts: std::collections::HashMap<String, crate::tdt::Tdt> = std::collections::HashMap::new();
    for p in &map.pieces {
        let key = p.path.to_ascii_lowercase();
        if !tdts.contains_key(&key) {
            tdts.insert(key, crate::generate::load_tdt(files, &p.path)?);
        }
    }
    let mut wild: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for t in tdts.values() {
        for e in t.edges.iter().flatten() {
            let ek = e.to_ascii_lowercase();
            if !wild.contains_key(&ek) {
                let et = crate::rooms::EdgeType::parse(&crate::text::decode_utf16(&files.read(&ek)?));
                wild.insert(ek, et.wildcard);
            }
        }
    }
    // The cells across the tile's real (not wildcard) edges (`0x1420bfe20` / `0x1420c3cc0`):
    // per side, the neighbour cell at the edge's exit and the shared corner in the tile's frame.
    let edge_cells = |m: &TileMap, pi: usize, (tx, ty): (i32, i32), (sw, sh): (i32, i32)| -> Vec<((i32, i32), (i32, i32), usize)> {
        let p = &m.pieces[pi];
        let key = crate::tileset::TileKey::from_tdt(&tdts[&p.path.to_ascii_lowercase()]).oriented(p.orientation);
        let mut out = Vec::new();
        for s in 0..4 {
            let Some(e) = &key.edges[s] else { continue };
            if wild.get(&e.to_ascii_lowercase()).copied().unwrap_or(true) {
                continue;
            }
            let d = key.exits[s] as i32 / 3;
            let (n, own) = match s {
                0 => ((tx + d, ty - 1), (d, 0)),
                1 => ((tx + sw, ty + d), (sw, d)),
                2 => ((tx + sw - d - 1, ty + sh), (sw - d - 1, sh)),
                _ => ((tx - 1, ty + sh - d - 1), (0, sh - d - 1)),
            };
            out.push((n, own, s));
        }
        out
    };
    let cw = w + 1;
    let mut cvis = vec![false; ((w + 1) * (h + 1)) as usize];
    let mut tvis = vec![false; (w * h) as usize];
    // The tile on a cell: origin, oriented size, piece (None: an empty cell, 1×1).
    let tile = |m: &TileMap, x: i32, y: i32| -> ((i32, i32), (i32, i32), Option<usize>) {
        match tile_at(m, x, y) {
            Some((o, s, _, pi)) => (o, s, Some(pi)),
            None => ((x, y), (1, 1), None),
        }
    };
    // The tile's point at oriented offset (u, v): its ground and height.
    let point = |m: &TileMap, tdts: &std::collections::HashMap<String, crate::tdt::Tdt>, pi: usize, u: i32, v: i32| -> (Option<String>, i32) {
        let p = &m.pieces[pi];
        let t = &tdts[&p.path.to_ascii_lowercase()];
        let (tw, th) = (t.size.0 as i32, t.size.1 as i32);
        for lv in 0..=th {
            for lu in 0..=tw {
                if crate::tileset::orient_point(p.orientation, tw, th, lu, lv) == (u, v) {
                    let k = ((tw + 1) * lv + lu) as usize;
                    return (t.point_grounds.get(k).cloned().flatten(), t.point_heights.get(k).copied().unwrap_or(0) as i32);
                }
            }
        }
        (None, 0)
    };
    let takes = |m: &TileMap, g: &Option<String>| g.as_ref().is_none_or(|g| m.gt_flags(&g.to_ascii_lowercase()) & 1 != 0);
    let debug = std::env::var_os("POE_DBG_HEIGHTS").is_some();
    for y in 0..h {
        let mut x = 0;
        while x < w {
            let Some(i) = map.index(x, y) else { x += 1; continue };
            let origin_cell = map.cells[i].as_ref().is_some_and(|c| c.offset == (0, 0));
            if !origin_cell {
                x += 1;
                continue;
            }
            if !tvis[(y * w + x) as usize] {
                // `0x1420c0870`.
                let mut queue: VecDeque<(i32, i32)> = VecDeque::from([(x, y)]);
                while let Some((ox, oy)) = queue.pop_front() {
                    if ox < 0 || oy < 0 || ox >= w || oy >= h || tvis[(oy * w + ox) as usize] {
                        continue;
                    }
                    let ((tx, ty), (sw, sh), pi) = tile(map, ox, oy);
                    // `0x1420bfe20`.
                    let mut cands: Vec<i32> = Vec::new();
                    let push = |c: &mut Vec<i32>, v: i32| {
                        if !c.contains(&v) {
                            c.push(v);
                        }
                    };
                    let corner = |u: i32, v: i32| -> Option<usize> { (tx + u >= 0 && ty + v >= 0 && tx + u <= w && ty + v <= h).then(|| ((ty + v) * cw + tx + u) as usize) };
                    match pi {
                        None => {
                            for (u, v) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                                if corner(u, v).is_some_and(|k| cvis[k]) {
                                    let pk = map.point_index(tx + u, ty + v).unwrap();
                                    if takes(map, &map.lattice[pk].ground) {
                                        push(&mut cands, lat_height(map, pk));
                                    }
                                }
                            }
                        }
                        Some(pi) => {
                            let at = |c: &mut Vec<i32>, u: i32, v: i32, check: bool| {
                                let (g, ph) = point(map, &tdts, pi, u, v);
                                if check && !takes(map, &g) {
                                    return;
                                }
                                if corner(u, v).is_some_and(|k| cvis[k]) {
                                    let pk = map.point_index(tx + u, ty + v).unwrap();
                                    push(c, lat_height(map, pk) - ph);
                                }
                            };
                            for u in 0..=sw {
                                at(&mut cands, u, 0, true);
                                at(&mut cands, u, sh, true);
                            }
                            for v in 1..sh {
                                at(&mut cands, 0, v, true);
                                at(&mut cands, sw, v, true);
                            }
                            if cands.is_empty() && std::env::var_os("POE_NO_EDGE_FALLBACK").is_none() {
                                for ((nx, ny), own, s) in edge_cells(map, pi, (tx, ty), (sw, sh)) {
                                    if nx < 0 || ny < 0 || nx >= w || ny >= h || !tvis[(ny * w + nx) as usize] {
                                        continue;
                                    }
                                    let Some(ni) = map.index(nx, ny) else { continue };
                                    let Some(nc) = map.cells[ni].as_ref() else { continue };
                                    let Some(npi) = map.piece_at[ni] else { continue };
                                    let (ox, oy) = (nc.offset.0 as i32, nc.offset.1 as i32);
                                    let nb_point = match s {
                                        0 => (ox, oy + 1),
                                        3 => (ox + 1, oy),
                                        _ => (ox, oy),
                                    };
                                    let v = nc.height as i32 - point(map, &tdts, pi, own.0, own.1).1 + point(map, &tdts, npi, nb_point.0, nb_point.1).1;
                                    cands.push(v);
                                }
                                if debug && !cands.is_empty() {
                                    eprintln!("HEIGHTS edge fallback at ({tx},{ty}): {cands:?}");
                                }
                            }
                            if cands.is_empty() {
                                for (u, v) in [(0, 0), (sw, 0), (0, sh), (sw, sh)] {
                                    at(&mut cands, u, v, false);
                                }
                            }
                        }
                    }
                    let hh = cands.iter().copied().min().unwrap_or(0);
                    if debug {
                        let ph = pi.map(|pi| tdts[&map.pieces[pi].path.to_ascii_lowercase()].point_heights.clone()).unwrap_or_default();
                        eprintln!("HEIGHTS tile ({tx},{ty}) {sw}x{sh} cands {cands:?} -> {hh} points {ph:?}");
                    }
                    // `0x1420bfc40`.
                    map.raised |= hh != 0;
                    match pi {
                        Some(pi) => {
                            let t = tdts[&map.pieces[pi].path.to_ascii_lowercase()].clone();
                            map.set_heights(tx, ty, &t, hh as i16);
                        }
                        None => {
                            for (u, v) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                                if let Some(pk) = map.point_index(tx + u, ty + v) {
                                    map.lattice[pk].height = hh as i8;
                                }
                            }
                        }
                    }
                    for v in 0..=sh {
                        for u in 0..=sw {
                            if let Some(k) = corner(u, v) {
                                cvis[k] = true;
                            }
                        }
                    }
                    for v in 0..sh {
                        for u in 0..sw {
                            if tx + u >= 0 && ty + v >= 0 && tx + u < w && ty + v < h {
                                tvis[((ty + v) * w + tx + u) as usize] = true;
                            }
                        }
                    }
                    // `0x1420c3cc0`: the neighbours across corners that take heights.
                    let next = |q: &mut VecDeque<(i32, i32)>, m: &TileMap, nx: i32, ny: i32| {
                        let ((ax, ay), _, _) = tile(m, nx, ny);
                        q.push_back((ax, ay));
                    };
                    match pi {
                        None => {
                            if tx > 0 { next(&mut queue, map, tx - 1, ty) }
                            if ty > 0 { next(&mut queue, map, tx, ty - 1) }
                            if tx + 1 < w { next(&mut queue, map, tx + 1, ty) }
                            if ty + 1 < h { next(&mut queue, map, tx, ty + 1) }
                        }
                        Some(pi) => {
                            let open = |m: &TileMap, a: (i32, i32), b: (i32, i32)| takes(m, &point(m, &tdts, pi, a.0, a.1).0) || takes(m, &point(m, &tdts, pi, b.0, b.1).0);
                            let edges = if std::env::var_os("POE_NO_EDGE_FALLBACK").is_none() { edge_cells(map, pi, (tx, ty), (sw, sh)) } else { Vec::new() };
                            let edge_next = |q: &mut VecDeque<(i32, i32)>, m: &TileMap, side: usize| {
                                for &((nx, ny), _, s) in &edges {
                                    if s == side && nx >= 0 && ny >= 0 && nx < w && ny < h && m.index(nx, ny).is_some_and(|i| m.cells[i].is_some()) {
                                        next(q, m, nx, ny);
                                    }
                                }
                            };
                            if tx > 0 {
                                for i in 0..sh {
                                    if open(map, (0, i), (0, i + 1)) { next(&mut queue, map, tx - 1, ty + i) }
                                }
                                edge_next(&mut queue, map, 3);
                            }
                            if tx + sw < w {
                                for i in 0..sh {
                                    if open(map, (sw, i), (sw, i + 1)) { next(&mut queue, map, tx + sw, ty + i) }
                                }
                                edge_next(&mut queue, map, 1);
                            }
                            if ty + sh < h {
                                for i in 0..sw {
                                    if open(map, (i, sh), (i + 1, sh)) { next(&mut queue, map, tx + i, ty + sh) }
                                }
                                edge_next(&mut queue, map, 2);
                            }
                            if ty > 0 {
                                for i in 0..sw {
                                    if open(map, (i, 0), (i + 1, 0)) { next(&mut queue, map, tx + i, ty - 1) }
                                }
                                edge_next(&mut queue, map, 0);
                            }
                        }
                    }
                }
            }
            let (_, (sw, _), _) = tile(map, x, y);
            x += sw.max(1);
        }
    }
    if !map.raised {
        return Ok(());
    }
    // (2) The lowest tile height becomes 0.
    let mut low = i32::MAX;
    for i in 0..map.cells.len() {
        if let Some(c) = &map.cells[i] {
            if c.offset == (0, 0) {
                low = low.min(c.height as i32);
            }
        }
    }
    if low == i32::MAX {
        low = 0;
    }
    for k in 0..map.lattice.len() {
        let v = lat_height(map, k) - low;
        map.raised |= v != 0;
        map.lattice[k].flags |= 2;
        map.lattice[k].height = v as i8;
    }
    for y in 0..h {
        let mut x = 0;
        while x < w {
            let i = map.index(x, y).unwrap();
            let step = match map.cells[i].as_ref() {
                Some(c) if c.offset == (0, 0) => {
                    let ch = c.height as i32;
                    map.raised |= ch != low;
                    let pi = map.piece_at[i].unwrap();
                    let t = tdts[&map.pieces[pi].path.to_ascii_lowercase()].clone();
                    map.set_heights(x, y, &t, (ch - low) as i16);
                    tile(map, x, y).1 .0.max(1)
                }
                _ => 1,
            };
            x += step;
        }
    }
    // (3) `0x1420c0f10` / `0x1420c14c0`.
    loop {
        let mut changed = 0;
        for y in 0..=h {
            for x in 0..=w {
                let k = (y * cw + x) as usize;
                if cvis[k] {
                    continue;
                }
                let mut hs = Vec::new();
                for (dx, dy) in [(-1, 0), (0, -1), (1, 0), (0, 1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx > w || ny > h || !cvis[(ny * cw + nx) as usize] {
                        continue;
                    }
                    hs.push(lat_height(map, map.point_index(nx, ny).unwrap()));
                }
                if hs.len() == 1 {
                    let pk = map.point_index(x, y).unwrap();
                    map.raised |= hs[0] != 0;
                    map.lattice[pk].flags |= 2;
                    map.lattice[pk].height = hs[0] as i8;
                    cvis[k] = true;
                    changed += 1;
                }
            }
        }
        if changed == 0 {
            break;
        }
    }
    Ok(())
}

/// `0x1420c1b50`, after the fill and the overlays: plain tiles (`0x1420c17d0`: not inheriting,
/// edges empty or wildcards, one ground on all four corners) on a ground that does not take
/// heights (`.gt` flag 1 clear) are re-levelled. Every other tile counts as reached. (1)
/// `0x1420c1d90`, per cell row by row: the neighbours left, right, above and below the cell
/// that are not plain and whose facing key corner has the tile's ground at a position along the
/// tile's side give their height plus that corner's key height; one distinct value sets the
/// tile (`0x1420bfc40`). (2) `0x1420c34b0`, until nothing changes: an unreached plain tile
/// whose reached neighbours of the same plain ground give one distinct height takes it.
pub fn refix_fills(map: &mut TileMap, files: &dyn crate::generate::Files) -> Result<(), crate::generate::GenError> {
    let (w, h) = (map.width as i32, map.height as i32);
    let mut tdts: std::collections::HashMap<String, crate::tdt::Tdt> = std::collections::HashMap::new();
    let mut wild: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for p in &map.pieces {
        let key = p.path.to_ascii_lowercase();
        if !tdts.contains_key(&key) {
            let t = crate::generate::load_tdt(files, &p.path)?;
            for e in t.edges.iter().flatten() {
                let ek = e.to_ascii_lowercase();
                if !wild.contains_key(&ek) {
                    let et = crate::rooms::EdgeType::parse(&crate::text::decode_utf16(&files.read(&ek)?));
                    wild.insert(ek, et.wildcard);
                }
            }
            tdts.insert(key, t);
        }
    }
    let norm = |m: &TileMap, g: &Option<String>| g.as_ref().map(|g| g.to_ascii_lowercase()).filter(|g| m.gt_flags(g) & 0x10 == 0);
    // The cell's tile: oriented size, piece, plain ground (Some(ground) when plain).
    let info = |m: &TileMap, x: i32, y: i32| -> Option<((i32, i32), usize, Option<Option<String>>)> {
        if x < 0 || y < 0 || x >= w || y >= h {
            return None;
        }
        let i = m.index(x, y)?;
        m.cells[i].as_ref()?;
        let pi = m.piece_at[i]?;
        let p = &m.pieces[pi];
        let t = &tdts[&p.path.to_ascii_lowercase()];
        let size = TileMap::oriented_size(p.size, p.orientation);
        let g0 = norm(m, &t.grounds[0]);
        let plain = t.inherits.is_none() && t.edges.iter().flatten().all(|e| wild.get(&e.to_ascii_lowercase()).copied().unwrap_or(false)) && t.grounds.iter().all(|g| norm(m, g) == g0);
        Some((size, pi, plain.then_some(g0)))
    };
    let height_at = |m: &TileMap, x: i32, y: i32| m.index(x, y).and_then(|i| m.cells[i].as_ref()).map_or(0, |c| c.height as i32);
    let origin = |m: &TileMap, x: i32, y: i32| {
        let c = m.cells[m.index(x, y).unwrap()].as_ref().unwrap();
        (x - c.offset.0 as i32, y - c.offset.1 as i32)
    };
    let mut tvis = vec![false; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            if let Some((_, _, None)) = info(map, x, y) {
                tvis[(y * w + x) as usize] = true;
            }
        }
    }
    let takes = |m: &TileMap, g: &str| m.gt_flags(g) & 1 != 0;
    let debug = std::env::var_os("POE_DBG_REFIX").is_some();
    let level = |m: &mut TileMap, tvis: &mut Vec<bool>, x: i32, y: i32, v: i32| {
        let (ox, oy) = origin(m, x, y);
        let pi = m.piece_at[m.index(x, y).unwrap()].unwrap();
        let t = tdts[&m.pieces[pi].path.to_ascii_lowercase()].clone();
        m.raised |= v != 0;
        m.set_heights(ox, oy, &t, v as i16);
        let (sw, sh) = TileMap::oriented_size(m.pieces[pi].size, m.pieces[pi].orientation);
        for dy in 0..sh {
            for dx in 0..sw {
                if ox + dx >= 0 && oy + dy >= 0 && ox + dx < w && oy + dy < h {
                    tvis[((oy + dy) * w + ox + dx) as usize] = true;
                }
            }
        }
    };
    // (1) `0x1420c1d90`.
    for y in 0..h {
        let mut x = 0;
        while x < w {
            let Some(((sw, sh), _, plain)) = info(map, x, y) else { x += 1; continue };
            if let Some(Some(g)) = &plain {
                if !takes(map, g) {
                    let mut cands: Vec<i32> = Vec::new();
                    // Neighbour cell, the two facing key corners (index, along-axis offset).
                    let sides: [((i32, i32), [(usize, bool); 2], bool); 4] = [
                        ((x - 1, y), [(1, false), (2, true)], true),
                        ((x + sw, y), [(0, false), (3, true)], true),
                        ((x, y - 1), [(3, false), (2, true)], false),
                        ((x, y + sh), [(0, false), (1, true)], false),
                    ];
                    for ((nx, ny), corners, vertical) in sides {
                        let Some(((nw, nh), npi, nplain)) = info(map, nx, ny) else { continue };
                        if nplain.is_some() {
                            continue;
                        }
                        let (nox, noy) = origin(map, nx, ny);
                        let np = &map.pieces[npi];
                        let key = crate::tileset::TileKey::from_tdt(&tdts[&np.path.to_ascii_lowercase()]).oriented(np.orientation);
                        let nh_val = height_at(map, nx, ny);
                        for (k, far) in corners {
                            let pos = if vertical { noy + if far { nh } else { 0 } } else { nox + if far { nw } else { 0 } };
                            let (lo, len) = if vertical { (y, sh) } else { (x, sw) };
                            let kg = key.grounds[k].clone().filter(|g| map.gt_flags(&g.to_ascii_lowercase()) & 0x10 == 0).map(|g| g.to_ascii_lowercase());
                            if kg.as_deref() == Some(g.as_str()) && lo <= pos && pos < lo + len {
                                let v = nh_val + key.heights[k] as i8 as i32;
                                if !cands.contains(&v) {
                                    cands.push(v);
                                }
                            }
                        }
                    }
                    if debug && !cands.is_empty() {
                        eprintln!("REFIX1 ({x},{y}) {g} cands {cands:?}");
                    }
                    if cands.len() == 1 {
                        level(map, &mut tvis, x, y, cands[0]);
                    }
                }
            }
            x += sw.max(1);
        }
    }
    // (2) `0x1420c34b0`.
    loop {
        let mut changed = 0;
        for y in 0..h {
            let mut x = 0;
            while x < w {
                let Some(((sw, sh), _, plain)) = info(map, x, y) else { x += 1; continue };
                if !tvis[(y * w + x) as usize] {
                    if let Some(Some(g)) = &plain {
                        if !takes(map, g) {
                            let mut cands: Vec<i32> = Vec::new();
                            for (nx, ny) in [(x - 1, y), (x + sw, y), (x, y - 1), (x, y + sh)] {
                                if nx < 0 || ny < 0 || nx >= w || ny >= h || !tvis[(ny * w + nx) as usize] {
                                    continue;
                                }
                                if let Some((_, _, Some(Some(ng)))) = info(map, nx, ny) {
                                    if &ng == g {
                                        let v = height_at(map, nx, ny);
                                        if !cands.contains(&v) {
                                            cands.push(v);
                                        }
                                    }
                                }
                            }
                            if cands.len() == 1 {
                                level(map, &mut tvis, x, y, cands[0]);
                                changed += 1;
                            }
                        }
                    }
                }
                x += sw.max(1);
            }
        }
        if changed == 0 {
            break;
        }
    }
    Ok(())
}
