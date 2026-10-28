//! The fill pass (`0x141f93060`): every cell still empty after rooms and edge tiles gets a fill
//! tile or a fill room from the `.gft` group of the ground at its top-left corner. Findings:
//! `docs/re/terrain/shared-passes.md`, "Fill".

use std::collections::HashMap;

use crate::{
    generate::{GenError, Files, Master, Room, TileMap, cell_key, load_room, uniform_ground},
    region::compose,
    rng::TerrainRng,
    tdt::Tdt,
    tileset::{orient_cell, pick},
};

/// One `.gft` entry: a fill tile (`.tdt`) or fill room (`.arm`), its weight and orientation
/// (8 = any).
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    weight: u32,
    path: String,
    o: u8,
    room: bool,
}

fn entries(group: &[(u32, String, u8)]) -> Result<Vec<Entry>, GenError> {
    group
        .iter()
        .map(|(w, p, mask)| {
            let o = match mask {
                0xff => 8,
                m if m.count_ones() == 1 => m.trailing_zeros() as u8,
                m => return Err(GenError::Unsupported(format!("fill {p}: orientation mask {m:#x}"))),
            };
            Ok(Entry { weight: *w, path: p.clone(), o, room: p.to_ascii_lowercase().ends_with(".arm") })
        })
        .collect()
}

/// A fill room placed by the pass (for the de-repetition pass's ground overrides).
pub struct PlacedFillRoom {
    pub room: Room,
    pub origin: (i32, i32),
    pub orientation: u8,
}

struct Fill<'a> {
    map: &'a mut TileMap,
    master: &'a mut Master,
    files: &'a dyn Files,
    rooms: HashMap<String, Room>,
    placed: Vec<PlacedFillRoom>,
    /// The caller's height for the cell (`0x141f8f930`'s last argument; `i32::MIN` there means
    /// the corners decide).
    height: Option<i16>,
    /// Dungeon fill: positions of placed tiles the exit wiggle may move (`0x14209a2c0`).
    wiggle: Option<(Vec<(i32, i32)>, crate::dungeon_tiles::Wiggles)>,
}

fn oriented(size: (u8, u8), o: u8) -> (i32, i32) {
    if o & 1 == 1 && o < 8 { (size.1 as i32, size.0 as i32) } else { (size.0 as i32, size.1 as i32) }
}

/// `0x1421c3d00`: the `r`-th (mod count) set bit of an orientation mask.
fn mask_pick(mask: u8, r: u32) -> u8 {
    let bits: Vec<u8> = (0..8).filter(|b| mask >> b & 1 != 0).collect();
    bits[(r as usize) % bits.len()]
}

impl Fill<'_> {
    fn note(&mut self, x: i32, y: i32, tdt: &Tdt) -> Result<(), GenError> {
        if let Some((list, w)) = &mut self.wiggle {
            if w.movable(self.files, tdt)? {
                list.push((x, y));
            }
        }
        Ok(())
    }

    fn free(&self, x: i32, y: i32, w: i32, h: i32) -> bool {
        (0..h).all(|dy| {
            (0..w).all(|dx| match self.map.index(x + dx, y + dy) {
                Some(i) => self.map.cells[i].is_none(),
                None => !self.map.spilled(x + dx, y + dy),
            })
        })
    }

    /// `0x141f9e5f0`: a tile fits at `(x, y)` (one cell of slack past the region's far sides).
    fn tile_fits(&self, x: i32, y: i32, tdt: &Tdt, o: u8) -> bool {
        let (w, h) = oriented(tdt.size, o);
        x >= -1
            && y >= -1
            && x + w <= self.map.width as i32 + 1
            && y + h <= self.map.height as i32 + 1
            && self.free(x, y, w, h)
    }

    /// `0x141f90e70`: a fill room fits at `(x, y)` with orientation `o` (inside the region).
    fn room_fits(&self, x: i32, y: i32, dims: (u8, u8), o: u8) -> bool {
        let (w, h) = oriented(dims, o);
        let (mw, mh) = (self.map.width as i32, self.map.height as i32);
        x < mw && x + w <= mw && y < mh && y + h <= mh && self.free(x, y, w, h)
    }

    /// `0x141f90d60`: orientation for a fill tile, 8 when it does not fit.
    fn tile_orientation(&self, x: i32, y: i32, tdt: &Tdt, o: u8, rng: &mut TerrainRng) -> u8 {
        choose_orientation(tdt, o, &|o| self.tile_fits(x, y, tdt, o), rng)
    }

    /// `0x141f8c500`: a tile turned by `o` (room frame) covers only free uniform cells of the
    /// room grid from `(cx, cy)`: no feature or edges, one ground compatible with the tile's
    /// (equal, or both missing or `.gt` flag 8) and the start cell's height.
    fn grid_fits(&self, room: &Room, cx: usize, cy: usize, tdt: &Tdt, o: u8) -> bool {
        use poe_data_tools::file_parsers::arm::types::Slot;
        let (w, h) = oriented(tdt.size, o);
        let grid = &room.arm.grid;
        let (rw, rh) = (room.width(), room.height());
        if cx + w as usize > rw || cy + h as usize > rh {
            return false;
        }
        let uniform = |slot: &Slot| -> Option<(Option<String>, u8)> {
            match slot {
                Slot::S => Some((None, 0)),
                slot => cell_key(slot)
                    .filter(|(k, f)| uniform_ground(k, f))
                    .map(|(k, _)| (k.grounds[0].clone(), k.heights[0])),
            }
        };
        let tile_ground = tdt.point_grounds.first().cloned().flatten().map(|g| g.to_ascii_lowercase().replace('\\', "/"));
        let loose = |g: &Option<String>| g.as_ref().is_none_or(|g| self.map.gt_flags(g) & 8 != 0);
        let Some((_, height)) = uniform(&grid[cy][cx]) else { return false };
        (0..h as usize).all(|dy| {
            (0..w as usize).all(|dx| {
                uniform(&grid[cy + dy][cx + dx]).is_some_and(|(g, hh)| {
                    hh == height && (g == tile_ground || (loose(&g) && loose(&tile_ground)))
                })
            })
        })
    }

    /// `0x141f90f60`: orientation for a fill room, 8 when it does not fit.
    fn room_orientation(&self, x: i32, y: i32, dims: (u8, u8), o: u8, rng: &mut TerrainRng) -> u8 {
        if o != 8 {
            return if self.room_fits(x, y, dims, o) { o } else { 8 };
        }
        if dims.0 == dims.1 {
            return if self.room_fits(x, y, dims, 0) { rng.below(8) as u8 } else { 8 };
        }
        let mask = if self.room_fits(x, y, dims, 0) { 0x55 } else { 0 } | if self.room_fits(x, y, dims, 1) { 0xaa } else { 0 };
        if mask == 0 { 8 } else { mask_pick(mask, rng.below(840)) }
    }

    fn room(&mut self, path: &str) -> Result<&Room, GenError> {
        let key = path.to_ascii_lowercase();
        if !self.rooms.contains_key(&key) {
            let r = load_room(self.files, &key)?;
            self.rooms.insert(key.clone(), r);
        }
        Ok(&self.rooms[&key])
    }

    /// `0x141f93960`: the fill tiles and fill rooms for a ground (the `default` group without
    /// one, or when its name has neither).
    fn groups(&mut self, ground: Option<&str>) -> Result<(Vec<Entry>, Vec<Entry>), GenError> {
        let split = |g: &[(u32, String, u8)]| -> Result<(Vec<Entry>, Vec<Entry>), GenError> {
            let e = entries(g)?;
            Ok(e.into_iter().partition(|e| !e.room))
        };
        let default = self.master.fills.get("default").cloned().unwrap_or_default();
        let Some(g) = ground else { return split(&default) };
        let name = self.master.ground_name(self.files, g)?;
        match self.master.fills.get(&name).cloned() {
            Some(group) => split(&group),
            None => split(&default),
        }
    }

    /// Height of a fill tile (`0x141f90950`): the caller's, else the middle of its set corner
    /// heights.
    fn corner_height(&self, x: i32, y: i32, w: i32, h: i32) -> i16 {
        if let Some(h) = self.height {
            return h;
        }
        let hs: Vec<i32> = [(x, y), (x + w, y), (x, y + h), (x + w, y + h)]
            .iter()
            .filter_map(|&(px, py)| self.map.point_index(px, py))
            .filter(|&i| self.map.lattice[i].flags & 2 != 0)
            .map(|i| self.map.lattice[i].height as i32)
            .collect();
        if std::env::var_os("POE_DBG_CORNERH").is_some() {
            let all: Vec<(i32, u8)> = [(x, y), (x + w, y), (x, y + h), (x + w, y + h)].iter().filter_map(|&(px, py)| self.map.point_index(px, py)).map(|i| (self.map.lattice[i].height as i32, self.map.lattice[i].flags)).collect();
            eprintln!("CORNERH ({x},{y}) {w}x{h} {all:?}");
        }
        match (hs.iter().min(), hs.iter().max()) {
            (Some(&lo), Some(&hi)) => (hi - (hi - lo) / 2) as i16,
            _ => 0,
        }
    }

    fn place_tile(&mut self, x: i32, y: i32, path: &str, o: u8) -> Result<i32, GenError> {
        let tdt = self.master.header(self.files, path)?.clone();
        let (w, h) = oriented(tdt.size, o);
        let height = self.corner_height(x, y, w, h);
        self.map.place(x, y, path, &tdt, o, height, None);
        self.note(x, y, &tdt)?;
        Ok(w - 1)
    }

    /// `0x141f8f930`: fill cell `(x, y)`; returns how many further cells of the row are taken.
    fn cell(&mut self, x: i32, y: i32, ground: Option<String>, walkable: bool, rng: &mut TerrainRng) -> Result<i32, GenError> {
        let ground = ground.filter(|g| self.map.gt_flags(g) & 0x10 == 0);
        let (tiles, rooms) = self.groups(ground.as_deref())?;
        let mut list: Vec<Entry> = tiles;
        let with_rooms = !rooms.is_empty();
        if with_rooms {
            for r in rooms {
                // `0x141f91110`: on an unwalkable corner only rooms whose room data +0x188 list
                // (0x30-byte entries: the points of interest, by size) is empty.
                let has_pois = self.room(&r.path)?.arm.points_of_interest.iter().any(|g| g.iter().any(|p| std::env::var("POE_POI_ANY").is_ok() || !p.tag.is_empty()));
                if std::env::var("POE_DBG_POI").is_ok() && !walkable && has_pois { eprintln!("POI skip {} w{} pois {:?}", r.path, r.weight, self.room(&r.path)?.arm.points_of_interest); }
                if walkable || !has_pois {
                    list.push(r);
                }
            }
        }
        let mut total: u32 = list.iter().map(|e| e.weight).sum();
        if std::env::var("POE_DBG_FILLTOTAL").is_ok() { eprintln!("FILLTOTAL ({x},{y}) ground {ground:?} walkable {walkable} total {total} entries {}", list.len()); }
        while !list.is_empty() {
            let e = pick_entry(&list, rng.below(total)).clone();
            let o = if e.room {
                let room = self.room(&e.path)?;
                let dims = (room.width() as u8, room.height() as u8);
                self.room_orientation(x, y, dims, e.o, rng)
            } else {
                let tdt = self.master.header(self.files, &e.path)?.clone();
                self.tile_orientation(x, y, &tdt, e.o, rng)
            };
            if o != 8 {
                if !e.room {
                    return self.place_tile(x, y, &e.path, o);
                }
                self.place_room(x, y, &e.path, o, ground.clone(), walkable, rng)?;
                let empty = self.map.index(x, y).is_some_and(|i| self.map.cells[i].is_none());
                return if empty { self.cell(x, y, ground, walkable, rng) } else { Ok(0) };
            }
            let k = list.iter().position(|f| f.path == e.path && f.o == e.o && f.room == e.room).unwrap();
            total -= list[k].weight;
            list.remove(k);
        }
        Err(GenError::Unsupported(format!("failed to find a fill tile or room at ({x},{y})")))
    }

    /// `0x141f8c8c0`: a fill room's cells (x outer, y inner), each re-picked up to five times
    /// until it fits, skipped when its cell is already taken; then the room's ground overrides,
    /// and the fill's ground on its corners that have none.
    #[allow(clippy::too_many_arguments)]
    fn place_room(&mut self, x: i32, y: i32, path: &str, o: u8, ground: Option<String>, _walkable: bool, rng: &mut TerrainRng) -> Result<(), GenError> {
        let room = load_room(self.files, &path.to_ascii_lowercase())?;
        self.map.use_room_grounds(&room);
        let (w, h) = (room.width(), room.height());
        // `0x141f8f930`: without a passed height the room's height is the middle of the corner
        // heights over its whole oriented extent (`0x141f90950`), for all its slots.
        let outer = self.height;
        if self.height.is_none() && std::env::var_os("POE_NO_ROOM_HEIGHT").is_none() {
            let (ow, oh) = oriented((w as u8, h as u8), o);
            self.height = Some(self.corner_height(x, y, ow as i32, oh as i32));
        }
        let r = self.place_room_cells(x, y, path, o, ground, room, rng);
        self.height = outer;
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn place_room_cells(&mut self, x: i32, y: i32, path: &str, o: u8, ground: Option<String>, room: Room, rng: &mut TerrainRng) -> Result<(), GenError> {
        let (w, h) = (room.width(), room.height());
        for cx in 0..w {
            for cy in 0..h {
                // An `s` slot, or a key without feature and edges on one ground: a fill tile of
                // that ground (no ground: `default`), as the fill pass picks it.
                let uniform = match &room.arm.grid[cy][cx] {
                    poe_data_tools::file_parsers::arm::types::Slot::S => Some(None),
                    slot => cell_key(slot).filter(|(k, f)| f.is_empty() && k.edges.iter().all(Option::is_none) && k.grounds.iter().all(|g| *g == k.grounds[0])).map(|(k, _)| k.grounds[0].clone()),
                };
                if let Some(g) = uniform {
                    let (ox, oy) = orient_cell(o, w as i32, h as i32, cx as i32, cy as i32);
                    if self.map.index(x + ox, y + oy).is_none_or(|i| self.map.cells[i].is_some()) {
                        continue;
                    }
                    // `0x141f8e400`: the tile turned by `to` in the room's frame, placed from the
                    // smallest corner its cells reach, turned by `to` then the room.
                    let at = |tdt: &Tdt, to: u8| {
                        let (tw, th) = oriented(tdt.size, to);
                        let (bx, by) = orient_cell(o, w as i32, h as i32, cx as i32 + tw - 1, cy as i32 + th - 1);
                        (x + ox.min(bx), y + oy.min(by))
                    };
                    let (mut list, _) = self.groups(g.as_deref())?;
                    let mut total: u32 = list.iter().map(|e| e.weight).sum();
                    let mut placed = false;
                    while !list.is_empty() {
                        let e = pick_entry(&list, rng.below(total)).clone();
                        let tdt = self.master.header(self.files, &e.path)?.clone();
                        let fits = |to: u8| {
                            let (mx, my) = at(&tdt, to);
                            self.tile_fits(mx, my, &tdt, compose(to, o)) && self.grid_fits(&room, cx, cy, &tdt, to)
                        };
                        let to = choose_orientation(&tdt, e.o, &fits, rng);
                        if to != 8 {
                            let (mx, my) = at(&tdt, to);
                            // `0x141f8c8c0` at `0x141f8d5bf`: the tile's height is the slot's
                            // (+0x72, its lowest corner) plus the room's.
                            let saved = self.height;
                            if std::env::var_os("POE_NO_SLOT_HEIGHT").is_none() {
                                if let (Some(h), Some((k, _))) = (saved, cell_key(&room.arm.grid[cy][cx])) {
                                    self.height = Some(h + k.heights.iter().map(|&v| v as i8).min().unwrap_or(0) as i16);
                                }
                            }
                            let r = self.place_tile(mx, my, &e.path, compose(to, o));
                            self.height = saved;
                            r?;
                            placed = true;
                            break;
                        }
                        let k = list.iter().position(|f| f.path == e.path && f.o == e.o).unwrap();
                        total -= list[k].weight;
                        list.remove(k);
                    }
                    // A `k` slot whose fill group has nothing that fits is placed from the
                    // tile set by its key like any other slot (`G1_4` 1003: a 3x3 inside_woods
                    // slot takes `Fill_01` after two failed fill picks); `s` slots stay empty.
                    if placed || matches!(&room.arm.grid[cy][cx], poe_data_tools::file_parsers::arm::types::Slot::S) {
                        continue;
                    }
                }
                let Some((key, feature)) = cell_key(&room.arm.grid[cy][cx]) else { continue };
                let (sw, sh) = (key.size.0.max(1) as i32, key.size.1.max(1) as i32);
                let mut key = key.oriented(o);
                // The lowest corner's height goes to the tile (slot +0x72).
                let low = key.heights.iter().map(|&v| v as i8).min().unwrap_or(0);
                for v in &mut key.heights {
                    *v = (*v as i8 - low) as u8;
                }
                let (ax, ay) = orient_cell(o, w as i32, h as i32, cx as i32, cy as i32);
                let (bx, by) = orient_cell(o, w as i32, h as i32, cx as i32 + sw - 1, cy as i32 + sh - 1);
                let (mx, my) = (x + ax.min(bx), y + ay.min(by));
                let list = self.master.tiles.candidates(&feature, &key).to_vec();
                if list.is_empty() {
                    return Err(GenError::Unsupported(format!("fill room {path}: no tile for cell ({cx},{cy}) key {key:?}")));
                }
                if self.map.index(mx, my).is_some_and(|i| self.map.cells[i].is_some()) {
                    continue;
                }
                let total = list.iter().map(|c| c.weight).sum();
                let mut chosen = None;
                for _ in 0..5 {
                    let c = pick(&list, rng.below(total)).unwrap();
                    let tdt = &self.master.tiles.tiles[c.tile].tdt;
                    if self.tile_fits(mx, my, tdt, c.orientation) {
                        chosen = Some(c);
                        break;
                    }
                }
                let c = chosen.ok_or_else(|| GenError::Unsupported(format!("no tile fits at ({mx},{my}) for fill room {path}")))?;
                let def = self.master.tiles.tiles[c.tile].clone();
                if std::env::var_os("POE_DBG_FILLKEY").is_some() {
                    eprintln!("FILLKEY {path} ({cx},{cy}) at {mx},{my} low {low} height {:?} {}", self.height, def.path);
                }
                self.map.place(mx, my, &def.path, &def.tdt, c.orientation, low as i16 + self.height.unwrap_or(0), Some(c.tile));
                self.note(mx, my, &def.tdt)?;
            }
        }
        self.map.room_overrides(&room, (x, y), o);
        let (ow, oh) = oriented((w as u8, h as u8), o);
        for v in 0..=oh {
            for u in 0..=ow {
                let Some(i) = self.map.point_index(x + u, y + v) else { continue };
                if self.map.lattice[i].ground.is_none() {
                    if let Some(g) = &ground {
                        self.map.lattice[i].ground = Some(g.clone());
                    }
                    self.map.lattice[i].flags |= 1;
                }
            }
        }
        self.placed.push(PlacedFillRoom { room, origin: (x, y), orientation: o });
        Ok(())
    }
}

/// The orientation for a fill tile (entry orientation `o`, 8 = any) among those `fits`
/// accepts: a square tile any of 8 when unturned fits, else the even or odd ones (`0x1421c3d00`
/// on a draw of 840); 8 when none fits.
fn choose_orientation(tdt: &Tdt, o: u8, fits: &dyn Fn(u8) -> bool, rng: &mut TerrainRng) -> u8 {
    if o != 8 {
        return if fits(o) { o } else { 8 };
    }
    if tdt.size.0 == tdt.size.1 {
        return if fits(0) { rng.below(8) as u8 } else { 8 };
    }
    let mask = if fits(0) { 0x55 } else { 0 } | if fits(1) { 0xaa } else { 0 };
    if mask == 0 { 8 } else { mask_pick(mask, rng.below(840)) }
}

/// `0x1415d5a30`/`0x141faccc0`: weighted pick (`r` in `0..total`).
fn pick_entry(list: &[Entry], mut r: u32) -> &Entry {
    for e in list {
        if r < e.weight {
            return e;
        }
        r -= e.weight;
    }
    list.last().unwrap()
}

/// `0x141f93060`: rows top to bottom, cells left to right; a cell's ground is its top-left
/// corner's, and fill rooms with points of interest only go where that corner is walkable (no
/// flag 8, and no ground or one with `.gt` flag 4).
/// The dungeon fill (`0x14209cf50` per listed cell): each still-empty cell with its listed
/// ground (else its corner's, when set) and reachability.
pub fn fill_cells(
    map: &mut TileMap,
    master: &mut Master,
    files: &dyn Files,
    cells: &[crate::dungeon_tiles::GroundCell],
    rng: &mut TerrainRng,
) -> Result<(Vec<PlacedFillRoom>, Vec<(i32, i32)>), GenError> {
    let mut f = Fill { map, master, files, rooms: HashMap::new(), placed: Vec::new(), height: None, wiggle: Some((Vec::new(), Default::default())) };
    for c in cells {
        let (x, y) = c.pos;
        if !f.free(x, y, 1, 1) {
            continue;
        }
        let ground = c.ground.clone().or_else(|| {
            let p = &f.map.lattice[f.map.point_index(x, y)?];
            (p.flags & 1 != 0).then(|| p.ground.clone()).flatten()
        });
        f.height = Some(c.height as i16);
        if std::env::var_os("POE_DBG_FILL").is_some() {
            eprintln!("FILLCELL {x},{y} {}x{} {:?} d{}", f.map.width, f.map.height, ground, rng.log.as_ref().map_or(0, Vec::len));
        }
        f.cell(x, y, ground, c.accessible, rng)?;
    }
    let wiggle = f.wiggle.take().map(|w| w.0).unwrap_or_default();
    Ok((f.placed, wiggle))
}

pub fn fill_pass(
    map: &mut TileMap,
    master: &mut Master,
    files: &dyn Files,
    rng: &mut TerrainRng,
) -> Result<Vec<PlacedFillRoom>, GenError> {
    let mut f = Fill { map, master, files, rooms: HashMap::new(), placed: Vec::new(), height: None, wiggle: None };
    for y in 0..f.map.height as i32 {
        let mut x = 0;
        while x < f.map.width as i32 {
            if f.map.index(x, y).is_some_and(|i| f.map.cells[i].is_none()) {
                let p = &f.map.lattice[f.map.point_index(x, y).unwrap()];
                let ground = p.ground.clone();
                let walkable = ground.as_ref().is_none_or(|g| f.map.gt_flags(g) & 4 != 0) && p.flags & 8 == 0;
                x += f.cell(x, y, ground, walkable, rng)?;
            }
            x += 1;
        }
    }
    Ok(f.placed)
}

/// Fills one empty cell the way the fill pass does (`0x141f8f930`), for callers outside the pass
/// (the overlays' second phase): `ground` is the cell's corner ground and `walkable` whether fill
/// rooms with points of interest may go there. Returns the fill rooms it placed.
#[allow(clippy::too_many_arguments)]
pub fn fill_cell(map: &mut TileMap, master: &mut Master, files: &dyn Files, pos: (i32, i32), ground: Option<String>, walkable: bool, wiggle: Option<&mut Vec<(i32, i32)>>, rng: &mut TerrainRng) -> Result<Vec<PlacedFillRoom>, GenError> {
    let mut f = Fill { map, master, files, rooms: HashMap::new(), placed: Vec::new(), height: None, wiggle: wiggle.as_ref().map(|_| (Vec::new(), Default::default())) };
    f.cell(pos.0, pos.1, ground, walkable, rng)?;
    if let (Some(out), Some(w)) = (wiggle, f.wiggle.take()) {
        out.extend(w.0);
    }
    Ok(f.placed)
}

/// `0x14209c150` (pre-unique room, run before the overlays when the master has an overlay
/// list and the room set a `preunique` room): the ground-only cells are shuffled (a discarded
/// `below(n)` first), then walked in that order; for each, `below(0x348)` picks an orientation
/// of the room's `.rs` mask, and the first cell that is accessible, has no ground or one with
/// `.gt` flag 8, and whose room rectangle lies free inside the region takes the room
/// (`0x141f8c8c0`). The cell list keeps the shuffled order for the fill. No cell fits: an error.
pub fn place_pre_unique(
    map: &mut TileMap,
    master: &mut Master,
    files: &dyn Files,
    room_path: &str,
    mask: u8,
    cells: &mut Vec<crate::dungeon_tiles::GroundCell>,
    rng: &mut TerrainRng,
) -> Result<(Vec<PlacedFillRoom>, Vec<(i32, i32)>), GenError> {
    let n = cells.len();
    // `POE_DBG_PU_IN`: the list before the shuffle, like the trace's `preunique_cells` lines.
    if std::env::var("POE_DBG_PU_IN").is_ok() { for (i, c) in cells.iter().enumerate() { eprintln!("PUIN {i} {} {} {} {}", c.pos.0, c.pos.1, c.early as u8, c.height); } }
    rng.below(n as u32);
    for i in 0..n {
        let k = rng.uniform((n - i) as u32) as usize;
        cells.swap(i, i + k);
    }
    if std::env::var("POE_DBG_PUALL").is_ok() { for (i, c) in cells.iter().enumerate() { eprintln!("PUL {} {:?} acc {} ground {:?} h {}", i + 1, c.pos, c.accessible, c.ground, c.height); } }
    let mut f = Fill { map, master, files, rooms: HashMap::new(), placed: Vec::new(), height: None, wiggle: Some((Vec::new(), Default::default())) };
    let (rw, rh) = {
        let room = f.room(room_path)?;
        (room.width() as u8, room.height() as u8)
    };
    for (ci, c) in cells.iter().enumerate() {
        let v = rng.below(0x348);
        let o = crate::region::pick(mask, v);
        let (w, h) = oriented((rw, rh), o);
        let (x, y) = c.pos;
        if std::env::var("POE_DBG_PREUNIQUE").is_ok() { eprintln!("PU {} cell {:?} o{o} acc {} ground {:?} flags {:?} free {}", ci + 1, c.pos, c.accessible, c.ground, c.ground.as_deref().map(|g| f.map.gt_flags(g)), f.free(x, y, w, h)); }
        let ground_ok = c.ground.as_deref().is_none_or(|g| f.map.gt_flags(g) & 8 != 0);
        if !(c.early && ground_ok) {
            continue;
        }
        let (mw, mh) = (f.map.width as i32, f.map.height as i32);
        if x < mw && x + w <= mw && y < mh && y + h <= mh && x >= 0 && y >= 0 && f.free(x, y, w, h) {
            if std::env::var("POE_DBG_PREUNIQUE").is_ok() { eprintln!("PREUNIQUE accept iteration {} cell {:?} o{o} size {w}x{h} ground {:?}", ci + 1, c.pos, c.ground); }
            f.height = Some(c.height as i16);
            f.place_room(x, y, room_path, o, c.ground.clone(), c.accessible, rng)?;
            let wiggle = f.wiggle.take().map(|w| w.0).unwrap_or_default();
            return Ok((f.placed, wiggle));
        }
    }
    Err(GenError::Unsupported(format!("Failed to place pre-unique room {room_path}")))
}

/// `0x1420bdae0` (the `.tgr` driver `0x1420bec40`, after the lattice passes and before the
/// overlays): the rooms league mechanics added (`ExtraTerrainFeatures`). `0x1420bd890` lists the
/// cells that are still empty and walkable (no ground, or a ground with `.gt` flags 4 and 8, on
/// a corner without flag 8) row by row, discards a `below(n)` and shuffles the list (forward
/// Fisher-Yates). Each room (`0x1420bde20`) draws one orientation with `below(8)` (a feature
/// whose orientation list holds "any"), then goes to the first cell in list order where its
/// rectangle lies inside the map on free cells: the corners under it whose ground (and the
/// cell's) has `.gt` flag 8 or none take the cell's ground, then the room is placed with the
/// cell's corner height (`0x141f8c8c0`). No cell fits: the room is skipped.
pub fn place_league_rooms(map: &mut TileMap, master: &mut Master, files: &dyn Files, rooms: &[crate::generate::LeagueRoom], rng: &mut TerrainRng) -> Result<Vec<PlacedFillRoom>, GenError> {
    for room in rooms {
        for t in &room.tiles {
            master.add_extra_tile(files, t)?;
        }
    }
    let mut cells: Vec<(i32, i32)> = Vec::new();
    for y in 0..map.height as i32 {
        for x in 0..map.width as i32 {
            if map.index(x, y).is_none_or(|i| map.cells[i].is_some()) {
                continue;
            }
            let Some(k) = map.point_index(x, y) else { continue };
            let p = &map.lattice[k];
            let ok = p.ground.as_deref().is_none_or(|g| {
                let f = map.gt_flags(g);
                f & 4 != 0 && f & 8 != 0
            });
            if ok && p.flags & 8 == 0 {
                cells.push((x, y));
            }
        }
    }
    let n = cells.len();
    rng.below(n as u32);
    for i in 0..n {
        let k = rng.uniform((n - i) as u32) as usize;
        cells.swap(i, i + k);
    }
    let mut f = Fill { map, master, files, rooms: HashMap::new(), placed: Vec::new(), height: None, wiggle: None };
    for lr in rooms {
        let (rw, rh) = {
            let room = f.room(&lr.room)?;
            (room.width() as u8, room.height() as u8)
        };
        let o = rng.below(8) as u8;
        let (w, h) = oriented((rw, rh), o);
        for &(x, y) in &cells {
            let (mw, mh) = (f.map.width as i32, f.map.height as i32);
            if !(x < mw && x + w <= mw && y < mh && y + h <= mh && f.free(x, y, w, h)) {
                continue;
            }
            let k = f.map.point_index(x, y).unwrap();
            let (ground, height) = (f.map.lattice[k].ground.clone(), f.map.lattice[k].height as i16);
            // `0x141d02530`: both grounds missing or with `.gt` flag 8.
            for py in y..=y + h {
                for px in x..=x + w {
                    let Some(i) = f.map.point_index(px, py) else { continue };
                    let open = f.map.lattice[i].ground.as_deref().is_none_or(|g| f.map.gt_flags(g) & 8 != 0);
                    if open {
                        f.map.lattice[i].flags |= 1;
                        if ground.is_some() {
                            f.map.lattice[i].ground = ground.clone();
                        }
                    }
                }
            }
            f.height = Some(height);
            f.place_room(x, y, &lr.room, o, ground, true, rng)?;
            f.files.take_league(&lr.room);
            break;
        }
    }
    Ok(f.placed)
}

/// Squared distance between two rectangles `(x0, y0, x1, y1)` (`0x14209a540`; 0 when they overlap).
fn rect_distance2(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> i32 {
    let dx = (a.0 - b.2).max(b.0 - a.2).max(0);
    let dy = (a.1 - b.3).max(b.1 - a.3).max(0);
    dx * dx + dy * dy
}

/// `0x14209a620` (the `.dgr` counterpart of [`place_league_rooms`], run inside the overlay block
/// of `0x14209c4c0` before the room injectors and the pre-unique room): the ground-only cell
/// list is shuffled (a discarded `below(n)`, then the forward Fisher-Yates), and the feature
/// list goes through two passes (`0x14209a880`): pass 0 refuses a spot closer than 4 cells
/// (squared distance <= 16) to a room placed before, pass 1 (the features that found no spot)
/// has no such check. Per feature (`0x14209aca0`) one orientation is drawn (`below(8)` for an
/// "any" feature), then the first cell of the list that is accessible, whose ground and corner
/// ground are missing or have `.gt` flags 4 and 8, and whose rectangle lies free inside the
/// region takes the room; the corners under it with no ground or one with flag 8 take the
/// cell's ground, and the room is placed with the cell's height (`0x141f8c8c0`). The list order
/// is kept for the pre-unique pass, which shuffles it again. The feature budget (`+0x144`) is
/// the whole list for a job with one graph, so it never stops a feature here.
pub fn place_league_rooms_dgr(
    map: &mut TileMap,
    master: &mut Master,
    files: &dyn Files,
    rooms: &[crate::generate::LeagueRoom],
    cells: &mut Vec<crate::dungeon_tiles::GroundCell>,
    rng: &mut TerrainRng,
) -> Result<(Vec<PlacedFillRoom>, Vec<(i32, i32)>), GenError> {
    let n = cells.len();
    rng.below(n as u32);
    for i in 0..n {
        let k = rng.uniform((n - i) as u32) as usize;
        cells.swap(i, i + k);
    }
    let mut f = Fill { map, master, files, rooms: HashMap::new(), placed: Vec::new(), height: None, wiggle: Some((Vec::new(), Default::default())) };
    let mut pending: Vec<&crate::generate::LeagueRoom> = if std::env::var_os("POE_DGR_LEAGUE_SKIP").is_some() { Vec::new() } else { rooms.iter().collect() };
    let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();
    for pass in 0..2 {
        if pending.is_empty() {
            break;
        }
        let mut left = Vec::new();
        for lr in pending {
            let (rw, rh) = {
                let room = f.room(&lr.room)?;
                (room.width() as u8, room.height() as u8)
            };
            let o = rng.below(8) as u8;
            let (w, h) = oriented((rw, rh), o);
            let mut done = false;
            for c in cells.iter() {
                let (x, y) = c.pos;
                if !c.accessible {
                    continue;
                }
                let flags_ok = |g: Option<&str>, m: &TileMap| g.is_none_or(|g| m.gt_flags(g) & 12 == 12);
                if !flags_ok(c.ground.as_deref(), &*f.map) {
                    continue;
                }
                let Some(k) = f.map.point_index(x, y) else { continue };
                if !flags_ok(f.map.lattice[k].ground.as_deref(), &*f.map) {
                    continue;
                }
                let (mw, mh) = (f.map.width as i32, f.map.height as i32);
                if !(x < mw && x + w <= mw && y < mh && y + h <= mh && f.free(x, y, w, h)) {
                    continue;
                }
                let rect = (x, y, x + w, y + h);
                if pass == 0 && rects.iter().any(|&r| rect_distance2(r, rect) <= 16) {
                    continue;
                }
                for py in y..=y + h {
                    for px in x..=x + w {
                        let Some(i) = f.map.point_index(px, py) else { continue };
                        let open = f.map.lattice[i].ground.as_deref().is_none_or(|g| f.map.gt_flags(g) & 8 != 0);
                        if open {
                            f.map.lattice[i].flags |= 1;
                            if c.ground.is_some() {
                                f.map.lattice[i].ground = c.ground.clone();
                            }
                        }
                    }
                }
                f.height = Some(c.height as i16);
                f.place_room(x, y, &lr.room, o, c.ground.clone(), c.accessible, rng)?;
                rects.push(rect);
                f.files.take_league(&lr.room);
                if std::env::var_os("POE_DBG_LEAGUE").is_some() { eprintln!("league dgr: pass {pass} room placed at ({x},{y}) o{o} size {w}x{h} ground {:?} h {} (cell {} of {})", c.ground, c.height, cells.iter().position(|d| d.pos == c.pos).unwrap_or(0), cells.len()); }
                done = true;
                break;
            }
            if !done {
                if std::env::var_os("POE_DBG_LEAGUE").is_some() { eprintln!("league dgr: pass {pass} room NOT placed o{o}"); }
                left.push(lr);
            }
        }
        pending = left;
    }
    let wiggle = f.wiggle.take().map(|w| w.0).unwrap_or_default();
    Ok((f.placed, wiggle))
}

/// A league feature room put into a `missiontile` slot by the tile pass (`0x1420983d0`, then
/// `0x141f8c8c0` with the slot's height and the lattice ground at the slot's corner). Returns the
/// rooms placed (for the de-repetition pass) and the tiles the exit wiggle may move.
#[allow(clippy::too_many_arguments)]
pub fn place_hosted(
    map: &mut TileMap,
    master: &mut Master,
    files: &dyn Files,
    room_path: &str,
    o: u8,
    pos: (i32, i32),
    height: i32,
    ground: Option<String>,
    rng: &mut TerrainRng,
) -> Result<(Vec<PlacedFillRoom>, Vec<(i32, i32)>), GenError> {
    let mut f = Fill { map, master, files, rooms: HashMap::new(), placed: Vec::new(), height: Some(height as i16), wiggle: Some((Vec::new(), Default::default())) };
    f.place_room(pos.0, pos.1, room_path, o, ground, true, rng)?;
    let wiggle = f.wiggle.take().map(|w| w.0).unwrap_or_default();
    Ok((f.placed, wiggle))
}
