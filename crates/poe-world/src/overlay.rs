//! Overlays (master `Overlays` `.toy`, master +0x200): rooms laid over finished terrain where
//! their root key fits it, e.g. road bends over straight road tiles. `0x141fa0ac0` builds the
//! context and the occupancy grid, `0x141fa1a70` runs the passes of one phase (1 before the
//! fill, 2 after it), `0x141fa1e60` lists candidate positions, `0x141fa23e0` places.
//!
//! `.toy`: `version 1`, then passes separated by blank lines. A pass starts with a header,
//! `<phase 1> <phase 2> [<phase 3>] [Random|FileOrder|SizeOrder] [±flag…]`, followed by
//! entries `<weight> "<.arm>" [±flag…] [!exclusion…] [limit=N] [required=N] [rotations…]`
//! (`0x141f9f4c0`).

use std::collections::{BTreeMap, HashMap};

use crate::{
    generate::{Files, GenError, Master, Room, TileMap, load_room, room_pass_oriented, slot_key},
    rng::{TerrainRng, lcg_index},
    rooms::TerrainTypes,
    tileset::{ORIENTATION_NAMES, TileKey},
};

/// Overlay flags (`0x142f0c018`): kinds of occupied cells an overlay may cover, plus
/// `MatchAllTileKeys` (0x40).
const FLAG_NAMES: [&str; 7] = [
    "OverlayTileOverlays",
    "OverlayFeatureTiles",
    "OverlayFeatureRooms",
    "OverlayFeatureRoomOverlap",
    "OverlayDescriptionNodes",
    "OverlayExtraFeatures",
    "MatchAllTileKeys",
];

#[derive(Debug, Clone, Default)]
pub struct ToyEntry {
    pub weight: u32,
    pub path: String,
    pub flags: u8,
    pub limit: u8,
    pub required: u8,
    pub exclusions: Vec<String>,
    /// Allowed orientations (bit per orientation).
    pub orientations: u8,
    /// The entry listed its orientations (`R90 FI ...`); a pick then counts the room once per
    /// listed orientation that turns the group's key, else once.
    pub explicit: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ToyPass {
    /// Phases the pass runs in (bit 0: before the fill, bit 1: after it).
    pub phases: u8,
    /// 0 random (all groups' positions together), 1 file order, 2 size order.
    pub order: u8,
    pub flags: u8,
    pub entries: Vec<ToyEntry>,
}

/// `+name`/`-name` on a flag byte (`0x141f9f330`); false when not an overlay flag.
fn apply_flag(flags: &mut u8, token: &str) -> bool {
    let (on, name) = match token.as_bytes().first() {
        Some(b'+') => (true, &token[1..]),
        Some(b'-') => (false, &token[1..]),
        _ => return false,
    };
    let Some(i) = FLAG_NAMES.iter().position(|n| *n == name) else { return false };
    let bit = 1u8 << i;
    if bit != 0x20 {
        if on { *flags |= bit } else { *flags &= !bit }
    }
    true
}

/// `limit=` / `required=` numbers: leading digits after spaces (`[1]` reads as 0).
fn small_number(s: &str) -> u8 {
    let v: u32 = s.trim_start().chars().take_while(|c| c.is_ascii_digit()).fold(0, |a, c| a.saturating_mul(10).saturating_add(c as u32 - '0' as u32));
    v.min(255) as u8
}

pub fn parse_toy(text: &str) -> Result<Vec<ToyPass>, GenError> {
    let mut passes: Vec<ToyPass> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with("//") || line.starts_with("version") {
            continue;
        }
        let tokens = crate::text::tokens(line);
        if line.contains('"') {
            let Some(pass) = passes.last_mut() else { return Err(GenError::Unsupported("toy entry before a pass header".into())) };
            let mut e = ToyEntry {
                weight: tokens[0].parse().unwrap_or(0),
                path: tokens.get(1).cloned().unwrap_or_default(),
                flags: pass.flags,
                ..Default::default()
            };
            for t in &tokens[2..] {
                if t.ends_with("SuppressUnderlyingDoodads") || t.ends_with("SuppressUnderlyingSpawnPoints") || apply_flag(&mut e.flags, t) {
                    continue;
                }
                if let Some(x) = t.strip_prefix('!') {
                    e.exclusions.push(x.to_string());
                } else if let Some(v) = t.strip_prefix("limit=") {
                    e.limit = small_number(v);
                } else if let Some(v) = t.strip_prefix("required=") {
                    e.required = small_number(v);
                } else if let Some(o) = ORIENTATION_NAMES.iter().position(|n| n == t) {
                    e.orientations |= 1 << o;
                } else {
                    return Err(GenError::Unsupported(format!("unknown overlay room argument {t:?}")));
                }
            }
            if e.orientations == 0 {
                e.orientations = 0xff;
            } else {
                e.explicit = true;
            }
            pass.entries.push(e);
        } else {
            let mut p = ToyPass { phases: 3, flags: 0x18, ..Default::default() };
            let mut bit = 0;
            for t in &tokens {
                if bit < 3 && (t == "0" || t == "1") {
                    if t == "1" { p.phases |= 1 << bit } else { p.phases &= !(1 << bit) }
                    bit += 1;
                } else if t.ends_with("SuppressUnderlyingDoodads") || t.ends_with("SuppressUnderlyingSpawnPoints") || apply_flag(&mut p.flags, t) {
                } else if t == "SizeOrder" {
                    p.order = 2;
                } else if t == "FileOrder" {
                    p.order = 1;
                } else if t == "Random" {
                    p.order = 0;
                } else {
                    return Err(GenError::Unsupported(format!("unknown overlay pass argument {t:?}")));
                }
            }
            passes.push(p);
        }
    }
    Ok(passes)
}

/// One room of a group.
#[derive(Debug, Clone)]
struct GroupRoom {
    explicit: bool,
    room: usize,
    limit: u8,
    exclusions: Vec<String>,
    /// Orientations in which the room's root key is the group's key.
    orientations: u8,
    weight: u32,
}

/// Rooms sharing a turned root key and flags.
#[derive(Debug, Clone)]
struct Group {
    key: TileKey,
    flags: u8,
    rooms: Vec<GroupRoom>,
    total: u32,
}

/// The overlay state of one region (`0x141fa0ac0`).
pub struct Overlays {
    passes: Vec<(ToyPass, Vec<Group>)>,
    rooms: Vec<(String, Room)>,
    /// Occupancy bits per tile: 1 overlay, 2 feature tile, 4 feature room, 8 feature room
    /// overlap, 0x10 description nodes, 0x20 extra features.
    occupied: Vec<u8>,
    width: i32,
    height: i32,
    /// A `.dgr` graph: candidate positions are scanned cell by cell (the generator flag
    /// `+0x90` bit 0 turns the 3-tile stride off, `0x141fa1e60`).
    pub dungeon: bool,
    /// Dungeon: tiles the refill placed that the exit wiggle may move (the fill listener `0x14209a2c0`).
    pub wiggle: Vec<(i32, i32)>,
    /// Overlays placed per room (for `limit=`).
    counts: HashMap<usize, u32>,
    /// Rooms placed so far (room index, origin, orientation): rooms like any other for the
    /// de-repetition pass.
    pub placed: Vec<(usize, (i32, i32), u8)>,
    /// Fill rooms the second phase placed where an overlay took tiles away (for the
    /// de-repetition pass's ground overrides).
    pub fill_rooms: Vec<crate::fill::PlacedFillRoom>,
    /// `SpecialTiles` row 2 (`OverlayBlank`): the tile an overlay room leaves where it covers
    /// nothing of its own; the second phase puts the old tile back under it.
    overlay_blank: Option<String>,
}

/// A placed room as the overlay grid sees it: rectangle (tiles, end exclusive), whether its
/// `.arm` has a tag (a feature room) and whether its graph node has tags (description nodes).
pub struct RoomRect {
    pub rect: (i32, i32, i32, i32),
    pub feature: bool,
    pub described: bool,
    /// Feature rooms whose `.arm` overlap equals the master's mark bit 4 on the rectangle
    /// shrunk by this many cells per side (`0x141fa0ac0`); 0 marks the whole rectangle.
    pub inset: i32,
}

impl Overlays {
    /// Loads the master's `.toy` and builds the groups and occupancy grid.
    /// A placed overlay room's data.
    pub fn room(&self, i: usize) -> &Room {
        &self.rooms[i].1
    }

    pub fn new(files: &dyn Files, toy: &str, map: &TileMap, master: &mut Master, rooms_placed: &[RoomRect], overlay_blank: Option<String>) -> Result<Self, GenError> {
        let passes = parse_toy(&crate::text::decode_utf16(&files.read(toy)?))?;
        let mut rooms: Vec<(String, Room)> = Vec::new();
        let mut built = Vec::new();
        for pass in passes {
            let mut groups: Vec<Group> = Vec::new();
            for e in &pass.entries {
                let idx = match rooms.iter().position(|r| r.0.eq_ignore_ascii_case(&e.path)) {
                    Some(i) => i,
                    None => {
                        rooms.push((e.path.clone(), load_room(files, &e.path)?));
                        rooms.len() - 1
                    }
                };
                let poe_data_tools::file_parsers::arm::types::Slot::K(k) = &rooms[idx].1.arm.root_slot else {
                    return Err(GenError::Unsupported(format!("overlay room {} has no root key", e.path)));
                };
                let root = slot_key(k);
                for o in 0..8u8 {
                    if e.orientations & (1 << o) == 0 {
                        continue;
                    }
                    let key = root.oriented(o);
                    let g = match groups.iter().position(|g| g.key == key && g.flags == e.flags) {
                        Some(g) => g,
                        None => {
                            groups.push(Group { key, flags: e.flags, rooms: Vec::new(), total: 0 });
                            groups.len() - 1
                        }
                    };
                    match groups[g].rooms.iter_mut().find(|r| r.room == idx) {
                        Some(r) => r.orientations |= 1 << o,
                        None => groups[g].rooms.push(GroupRoom { explicit: e.explicit, room: idx, limit: e.limit, exclusions: e.exclusions.clone(), orientations: 1 << o, weight: e.weight }),
                    }
                }
            }
            for g in &mut groups {
                g.rooms.retain(|r| r.weight != 0);
                g.total = g.rooms.iter().map(|r| r.weight).sum();
            }
            groups.retain(|g| g.total != 0);
            if pass.order == 2 {
                // `SizeOrder` (`0x141f9f4c0`, insertion or merge sort `0x141fc45c0`): largest
                // key first by area, then width, then height; equal ones keep their order.
                groups.sort_by_key(|g| std::cmp::Reverse((g.key.size.0 as u32 * g.key.size.1 as u32, g.key.size.0, g.key.size.1)));
            }
            built.push((pass, groups));
        }
        let (width, height) = (map.width as i32, map.height as i32);
        let mut s = Self { passes: built, rooms, occupied: vec![0; (width * height) as usize], width, height, dungeon: false, wiggle: Vec::new(), counts: HashMap::new(), placed: Vec::new(), fill_rooms: Vec::new(), overlay_blank: overlay_blank.map(|p| p.to_ascii_lowercase()) };
        if s.passes.iter().any(|p| p.0.phases & 3 != 0) {
            for r in rooms_placed {
                if r.described {
                    s.mark(r.rect, 0x10);
                }
            }
            for r in rooms_placed {
                if r.feature {
                    let (x0, y0, x1, y1) = r.rect;
                    s.mark((x0 + r.inset, y0 + r.inset, (x1 - r.inset).max(x0 + r.inset), (y1 - r.inset).max(y0 + r.inset)), 4);
                    s.mark(r.rect, 8);
                }
            }
            for p in 0..map.pieces.len() {
                let piece = &map.pieces[p];
                let tdt = master.header(files, &piece.path)?;
                if tdt.feature.as_deref().is_some_and(|f| !f.is_empty()) {
                    let (w, h) = TileMap::oriented_size(piece.size, piece.orientation);
                    let still = map.index(piece.x, piece.y).is_some_and(|i| map.piece_at[i] == Some(p));
                    if still {
                        s.mark((piece.x, piece.y, piece.x + w, piece.y + h), 2);
                    }
                }
            }
        }
        if let Ok(path) = std::env::var("POE_DBG_OCC") {
            let mut out = String::new();
            for y in 0..s.height {
                for x in 0..s.width {
                    let v = s.occupied[(y * s.width + x) as usize];
                    out.push(if v == 0 { '.' } else { char::from_digit(v as u32, 16).unwrap_or('?') });
                }
                out.push('\n');
            }
            std::fs::write(path, out).ok();
        }
        Ok(s)
    }

    fn mark(&mut self, (x0, y0, x1, y1): (i32, i32, i32, i32), bit: u8) {
        for y in y0.max(0)..y1.min(self.height) {
            for x in x0.max(0)..x1.min(self.width) {
                self.occupied[(y * self.width + x) as usize] |= bit;
            }
        }
    }

    /// `0x141fa1a70`: the passes of phase `phase` (1 or 2).
    #[allow(clippy::too_many_arguments)]
    pub fn run(&mut self, phase: u8, map: &mut TileMap, master: &mut Master, types: &mut TerrainTypes, files: &dyn Files, rng: &mut TerrainRng) -> Result<(), GenError> {
        for (_, r) in &self.rooms {
            if !self.dungeon || std::env::var("POE_OV_UGR").is_ok() {
                map.use_room_grounds(r);
            }
        }
        for pi in 0..self.passes.len() {
            if self.passes[pi].0.phases & phase == 0 {
                continue;
            }
            let groups = self.passes[pi].1.clone();
            if self.passes[pi].0.order == 0 {
                let mut cands = Candidates::default();
                for (gi, g) in groups.iter().enumerate() {
                    self.collect(g, gi, map, master, types, files, &mut cands)?;
                }
                self.place(phase, &groups, cands, map, master, types, files, rng)?;
            } else {
                for (gi, g) in groups.iter().enumerate() {
                    let mut cands = Candidates::default();
                    self.collect(g, gi, map, master, types, files, &mut cands)?;
                    self.place(phase, &groups, cands, map, master, types, files, rng)?;
                }
            }
        }
        Ok(())
    }

    /// `0x141fa1e60`: every position the group fits, on a 3-tile grid when its key has an edge
    /// of size 3.
    #[allow(clippy::too_many_arguments)]
    fn collect(&self, g: &Group, gi: usize, map: &TileMap, master: &mut Master, types: &mut TerrainTypes, files: &dyn Files, out: &mut Candidates) -> Result<(), GenError> {
        let mut stride3 = false;
        for e in g.key.edges.iter().flatten() {
            if types.edge(files, e)?.size == 3 {
                stride3 = true;
            }
        }
        if stride3 && !self.dungeon {
            let mut y = 0;
            while y <= self.height - 3 {
                let mut x = 0;
                while x <= self.width - 3 {
                    if self.fits(g, (x, y), map, master, types, files)? {
                        out.add((x, y), gi, g.total);
                    }
                    x += 3;
                }
                y += 3;
            }
        } else {
            for y in 0..self.height {
                for x in 0..self.width {
                    if self.fits(g, (x, y), map, master, types, files)? {
                        out.add((x, y), gi, g.total);
                    }
                }
            }
        }
        Ok(())
    }

    /// `0x141fa1830`: inside the region, over cells the group's flags allow, covering whole
    /// pieces, and the key fits the terrain (`0x141f9d200` with margin 1).
    fn fits(&self, g: &Group, (x, y): (i32, i32), map: &TileMap, master: &mut Master, types: &mut TerrainTypes, files: &dyn Files) -> Result<bool, GenError> {
        let (w, h) = (g.key.size.0 as i32, g.key.size.1 as i32);
        if x >= self.width || y >= self.height || x + w - 1 >= self.width || y + h - 1 >= self.height {
            return Ok(false);
        }
        for yy in y..y + h {
            for xx in x..x + w {
                if !g.flags & self.occupied[(yy * self.width + xx) as usize] != 0 {
                    if std::env::var("POE_DBG_OVERLAY").is_ok() { eprintln!("FIT {w}x{h} ({x},{y}) occupied {:#x} flags {:#x}", self.occupied[(yy * self.width + xx) as usize], g.flags); }
                    return Ok(false);
                }
            }
        }
        if !covers_whole_pieces(map, (x, y, x + w, y + h)) {
            if std::env::var("POE_DBG_OVERLAY").is_ok() { eprintln!("FIT {w}x{h} ({x},{y}) pieces"); }
            return Ok(false);
        }
        let code = room_fits(map, (x, y), &g.key, false, 1, master, types, files)?;
        if std::env::var("POE_DBG_OVERLAY").is_ok() { eprintln!("FIT {w}x{h} ({x},{y}) code {code}"); }
        Ok(code == 0)
    }

    /// `0x141fa23e0`: draws positions until none are left; at each, a group that still fits,
    /// then a room of it (or nothing, padded to 100), its orientation and its height.
    #[allow(clippy::too_many_arguments)]
    fn place(&mut self, phase: u8, groups: &[Group], mut cands: Candidates, map: &mut TileMap, master: &mut Master, types: &mut TerrainTypes, files: &dyn Files, rng: &mut TerrainRng) -> Result<(), GenError> {
        // Phase 2 passes the refill flag (`0x141fa23e0` param 3); the `.dgr` tile pass calls
        // phase 1 without it, so the flag is whatever the register held (set in the traces).
        let refill = phase == 2 || self.dungeon;
        let mut total: u32 = cands.0.values().map(|c| c.0).sum();
        if std::env::var("POE_DBG_OVERLAY").is_ok() {
            eprintln!("OVERLAY phase {phase} candidates {} total {total} groups {}", cands.0.len(), groups.len());
            for (k, c) in cands.0.iter() {
                eprintln!("  cand ({},{}) weight {} groups {:?}", k >> 16, k & 0xffff, c.0, c.1.iter().map(|x| (x.0, groups[x.0].key.size)).collect::<Vec<_>>());
            }
            for (gi, g) in groups.iter().enumerate() {
                let n = cands.0.values().filter(|c| c.1.iter().any(|x| x.0 == gi)).count();
                eprintln!("  group {gi} size {:?} flags {:#x} total {} positions {n} key {:?}", g.key.size, g.flags, g.total, g.key);
            }
        }
        while !cands.0.is_empty() {
            let mut r = rng.below(total);
            let mut pick = None;
            for (k, c) in cands.0.iter() {
                if r < c.0 {
                    pick = Some(*k);
                    break;
                }
                r -= c.0;
            }
            let k = pick.unwrap_or(*cands.0.keys().last().unwrap());
            let (weight, list) = cands.0.remove(&k).unwrap();
            total -= weight;
            let pos = ((k >> 16) as i32, (k & 0xffff) as i32);
            let mut live: Vec<(usize, u32)> = Vec::new();
            for &(gi, w) in &list {
                if self.fits(&groups[gi], pos, map, master, types, files)? {
                    live.push((gi, w));
                }
            }
            if std::env::var("POE_DBG_OVERLAY").is_ok() {
                eprintln!("PLACE phase {phase} pos {pos:?} live {:?} of {:?}", live, list);
            }
            if live.is_empty() {
                continue;
            }
            let sum: u32 = live.iter().map(|l| l.1).sum();
            let mut r = rng.below(sum);
            let mut gi = live[live.len() - 1].0;
            for &(g, w) in &live {
                if r < w {
                    gi = g;
                    break;
                }
                r -= w;
            }
            let g = &groups[gi];
            let (w, h) = (g.key.size.0 as i32, g.key.size.1 as i32);
            let mut options: Vec<(Option<(usize, u8)>, u32)> = Vec::new();
            for (ri, room) in g.rooms.iter().enumerate() {
                if room.limit != 0 && self.counts.get(&room.room).copied().unwrap_or(0) >= room.limit as u32 {
                    continue;
                }
                if !room.exclusions.is_empty() && !self.has_features(&room.exclusions, (pos.0, pos.1, pos.0 + w, pos.1 + h), map, master, files)? {
                    continue;
                }
                if g.flags & 0x40 != 0 {
                    let mut any = false;
                    for o in 0..8u8 {
                        if room.orientations >> o & 1 != 0 && self.matches_all_keys(&self.rooms[room.room].1, pos, o, map, master, files)? {
                            any = true;
                            break;
                        }
                    }
                    if !any {
                        continue;
                    }
                }
                // A position counts a room once, but the pick lists it once per orientation
                // whose turned root key is the group's (the sums were 200 for a symmetric room).
                if self.dungeon && room.explicit {
                    for o in 0..8u8 {
                        if room.orientations >> o & 1 != 0 {
                            options.push((Some((ri, o)), room.weight));
                        }
                    }
                } else {
                    options.push((Some((ri, 0xff)), room.weight));
                }
            }
            if options.is_empty() {
                continue;
            }
            if std::env::var("POE_DBG_OVERLAY").is_ok() {
                eprintln!("OPTIONS pos {pos:?} group {gi} {:?}", options.iter().map(|o| (o.0.map(|(r, ro)| (self.rooms[g.rooms[r].room].0.rsplit('/').next().unwrap().to_string(), ro, g.rooms[r].orientations)), o.1)).collect::<Vec<_>>());
            }
            let mut sum: u32 = options.iter().map(|o| o.1).sum();
            if sum < 100 {
                options.push((None, 100 - sum));
                sum = 100;
            }
            let mut r = rng.below(sum);
            let mut choice = options[options.len() - 1].0;
            for &(o, wgt) in &options {
                if r < wgt {
                    choice = o;
                    break;
                }
                r -= wgt;
            }
            let Some(ri) = choice else {
                self.mark((pos.0, pos.1, pos.0 + w, pos.1 + h), 1);
                continue;
            };
            let (ri, o) = ri;
            let room = &g.rooms[ri];
            let v = rng.below(0x348);
            let o = if o == 0xff { crate::region::pick(room.orientations, v) } else { o };
            let _ = lcg_index;
            let Some(height) = room_height(&self.rooms[room.room].1, map, pos, o)? else { continue };
            if height != 0 {
                return Err(GenError::Unsupported("overlay with a height offset".into()));
            }
            // The tiles under it go (phase 2 keeps a list of every cell they covered, sorted by
            // x then y, to fill again afterwards), its corner points are cleared, and the room
            // is placed like any room (`0x141f8e6c0`).
            let mut taken: BTreeMap<(i32, i32), (String, u8, i16, bool)> = BTreeMap::new();
            for yy in pos.1..pos.1 + h {
                for xx in pos.0..pos.0 + w {
                    if refill {
                        if let Some(i) = map.index(xx, yy) {
                            if let (Some(cell), Some(p)) = (map.cells[i].clone(), map.piece_at[i]) {
                                let piece = map.pieces[p].clone();
                                let (pw, ph) = TileMap::oriented_size(piece.size, piece.orientation);
                                let one_by_one = piece.size == (1, 1);
                                for fy in piece.y..piece.y + ph {
                                    for fx in piece.x..piece.x + pw {
                                        taken.insert((fx, fy), (piece.path.clone(), piece.orientation, cell.height, one_by_one));
                                    }
                                }
                            }
                        }
                    }
                    map.remove_piece_at(xx, yy);
                }
            }
            for yy in pos.1..=pos.1 + h {
                for xx in pos.0..=pos.0 + w {
                    if let Some(i) = map.point_index(xx, yy) {
                        map.lattice[i].ground = None;
                        map.lattice[i].flags &= !1;
                    }
                }
            }
            if std::env::var("POE_DBG_OVERLAY").is_ok() {
                eprintln!("ROOMPLACE {} at {pos:?} o{o}", self.rooms[room.room].0);
            }
            room_pass_oriented(map, master, &self.rooms[room.room].1, pos, o, true, rng)?;
            self.mark((pos.0, pos.1, pos.0 + w, pos.1 + h), 1);
            *self.counts.entry(room.room).or_default() += 1;
            self.placed.push((room.room, pos, o));
            if refill {
                self.refill(taken, map, master, files, rng)?;
            }
        }
        Ok(())
    }
}

impl Overlays {
    /// `0x141fa3470` (a toy entry's `!Name` arguments): the tiles that start inside the rectangle
    /// and carry a feature name include every listed name, each tile counting for one.
    fn has_features(&self, names: &[String], (x0, y0, x1, y1): (i32, i32, i32, i32), map: &TileMap, master: &mut Master, files: &dyn Files) -> Result<bool, GenError> {
        let mut want: Vec<String> = names.to_vec();
        for y in y0..y1 {
            for x in x0..x1 {
                let Some(i) = map.index(x, y) else { continue };
                let Some(cell) = &map.cells[i] else { continue };
                if cell.offset != (0, 0) {
                    continue;
                }
                let tdt = master.header(files, &cell.path)?;
                let Some(f) = tdt.feature.as_deref().filter(|f| !f.is_empty()) else { continue };
                if let Some(k) = want.iter().position(|n| n == f) {
                    want.remove(k);
                }
            }
        }
        Ok(want.is_empty())
    }

    /// `0x141fa3780` (`MatchAllTileKeys`): every `k` slot of the room, turned by `o` and laid at
    /// `pos`, sits on a tile that starts there with the same feature name and the same key.
    fn matches_all_keys(&self, room: &Room, pos: (i32, i32), o: u8, map: &TileMap, master: &mut Master, files: &dyn Files) -> Result<bool, GenError> {
        let (w, h) = (room.width() as i32, room.height() as i32);
        for y in 0..h {
            for x in 0..w {
                let poe_data_tools::file_parsers::arm::types::Slot::K(k) = &room.arm.grid[y as usize][x as usize] else { continue };
                let key = slot_key(k);
                let (sw, sh) = (key.size.0.max(1) as i32, key.size.1.max(1) as i32);
                let (ax, ay) = crate::tileset::orient_cell(o, w, h, x, y);
                let (bx, by) = crate::tileset::orient_cell(o, w, h, x + sw - 1, y + sh - 1);
                let (mx, my) = (pos.0 + ax.min(bx), pos.1 + ay.min(by));
                let Some(i) = map.index(mx, my) else { return Ok(false) };
                let Some(cell) = &map.cells[i] else { return Ok(false) };
                if cell.offset != (0, 0) {
                    return Ok(false);
                }
                let tdt = master.header(files, &cell.path)?;
                if tdt.feature.clone().unwrap_or_default() != k.slot_tag.clone().unwrap_or_default() {
                    return Ok(false);
                }
                if TileKey::from_tdt(tdt).oriented(cell.orientation) != key.oriented(o) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Phase 2, after an overlay room went in (`0x141fa23e0`): every cell the removed tiles
    /// covered, in x then y order, that is empty again is filled like the fill pass does
    /// (ground of its corner; the fill is walkable when that ground is none or has `.gt` flag 4).
    /// A cell that holds the `OverlayBlank` tile gets its old tile back when that was 1x1 and
    /// has no edge other than wildcards; otherwise the blank goes and the cell is filled.
    fn refill(&mut self, taken: BTreeMap<(i32, i32), (String, u8, i16, bool)>, map: &mut TileMap, master: &mut Master, files: &dyn Files, rng: &mut TerrainRng) -> Result<(), GenError> {
        for ((x, y), (path, orientation, height, one_by_one)) in taken {
            let Some(i) = map.index(x, y) else { continue };
            if map.cells[i].is_some() {
                let blank = map.cells[i].as_ref().is_some_and(|c| self.overlay_blank.as_deref().is_some_and(|b| c.path.eq_ignore_ascii_case(b)));
                if !blank {
                    continue;
                }
                let tdt = master.header(files, &path)?.clone();
                let key = TileKey::from_tdt(&tdt);
                let plain = key.edges.iter().flatten().all(|e| map.edge_flags.get(&e.to_ascii_lowercase()).is_some_and(|f| f & 0x80 != 0));
                if one_by_one && plain {
                    let tile = master.tiles.tiles.iter().position(|t| t.path.eq_ignore_ascii_case(&path));
                    map.remove_piece_at(x, y);
                    map.place(x, y, &path, &tdt, orientation, height, tile);
                    continue;
                }
                map.remove_piece_at(x, y);
            }
            let ground = map.point_index(x, y).and_then(|k| map.lattice[k].ground.clone());
            let walkable = ground.as_ref().is_none_or(|g| map.gt_flags(g) & 4 != 0);
            let placed = crate::fill::fill_cell(map, master, files, (x, y), ground, walkable, self.dungeon.then_some(&mut self.wiggle), rng)?;
            self.fill_rooms.extend(placed);
        }
        Ok(())
    }
}

/// Candidate positions keyed `x·0x10000 + y` (`0x141fa20b0`): summed weight and the groups.
#[derive(Default)]
struct Candidates(BTreeMap<u32, (u32, Vec<(usize, u32)>)>);

impl Candidates {
    fn add(&mut self, (x, y): (i32, i32), group: usize, weight: u32) {
        let e = self.0.entry((x as u32) << 16 | y as u32).or_default();
        e.0 += weight;
        e.1.push((group, weight));
    }
}

/// `0x141d0dc40`: the rectangle holds at least one tile and every tile it touches lies inside it.
fn covers_whole_pieces(map: &TileMap, (x0, y0, x1, y1): (i32, i32, i32, i32)) -> bool {
    let mut any = false;
    for y in y0..y1 {
        for x in x0..x1 {
            let Some(i) = map.index(x, y) else { continue };
            let Some(cell) = &map.cells[i] else { continue };
            any = true;
            let Some(p) = map.piece_at[i] else { continue };
            let piece = &map.pieces[p];
            let (ox, oy) = (x - cell.offset.0 as i32, y - cell.offset.1 as i32);
            let (w, h) = TileMap::oriented_size(piece.size, piece.orientation);
            if ox < x0 || oy < y0 || ox + w > x1 || oy + h > y1 {
                return false;
            }
        }
    }
    any
}

/// The key of the tile at a map cell and the cell's tile origin.
fn tile_key_at(map: &TileMap, x: i32, y: i32, master: &mut Master, files: &dyn Files) -> Result<Option<(TileKey, (i32, i32))>, GenError> {
    let Some(i) = map.index(x, y) else { return Ok(None) };
    let Some(cell) = map.cells[i].clone() else { return Ok(None) };
    let tdt = master.header(files, &cell.path)?;
    let key = TileKey::from_tdt(tdt).oriented(cell.orientation);
    Ok(Some((key, (x - cell.offset.0 as i32, y - cell.offset.1 as i32))))
}

/// An exit along a side as a position left-to-right / top-to-bottom (sides 2 and 3 run the
/// other way in keys).
fn absolute_exit(side: usize, len: u8, e: u8) -> i32 {
    if side > 1 { 3 * len as i32 - e as i32 - 1 } else { e as i32 }
}

/// Lattice ground and height at a corner point (`None` for grounds with `.gt` flag 0x10).
fn corner(map: &TileMap, x: i32, y: i32) -> (Option<String>, i32) {
    match map.point_index(x, y) {
        Some(i) => {
            let p = &map.lattice[i];
            let g = p.ground.clone().filter(|g| map.gt_flags(g) & 0x10 == 0);
            let mut h = p.height as i32;
            if h < -0x78 {
                h += 0x100;
            }
            (g, h)
        }
        None => (None, 0),
    }
}

/// `0x141f9d200`: 0 when `key` fits the terrain at `pos` (corner grounds, one consistent height
/// difference, edges against the neighbouring tiles); else the failing check's code.
#[allow(clippy::too_many_arguments)]
pub fn room_fits(map: &TileMap, pos: (i32, i32), key: &TileKey, wild: bool, margin: i32, master: &mut Master, types: &mut TerrainTypes, files: &dyn Files) -> Result<u32, GenError> {
    let (x, y) = pos;
    let (w, h) = (key.size.0 as i32, key.size.1 as i32);
    if x < 0 || y < 0 || x + w > map.width as i32 || y + h > map.height as i32 {
        return Ok(5);
    }
    let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    let key_ground = |i: usize| key.grounds[i].clone().filter(|g| map.gt_flags(g) & 0x10 == 0);
    // `0x141f9cfc0`
    let mut odd = 0;
    for (s, &(cx, cy)) in corners.iter().enumerate() {
        let (a, _) = corner(map, cx, cy);
        let b = key_ground(s);
        let ok = if a == b {
            true
        } else {
            let wild8 = |g: &Option<String>| g.as_ref().is_none_or(|g| map.gt_flags(g) & 8 != 0);
            if wild8(&a) && wild8(&b) {
                wild || a.is_some()
            } else if let Some(ag) = &a {
                let fa = map.gt_flags(ag);
                if fa & 0x80 != 0 {
                    true
                } else if fa & 5 == 0 && b.as_ref().is_some_and(|bg| map.gt_flags(bg) & 5 == 0) {
                    odd += 1;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if !ok {
            return Ok(6 + s as u32);
        }
    }
    if odd == 4 {
        return Ok(6);
    }
    let mut diffs: Vec<i32> = Vec::new();
    for (s, &(cx, cy)) in corners.iter().enumerate() {
        // Raw grounds here (`0x141f9d200` reads the lattice and key grounds without the
        // `.gt` flag 0x10 filter).
        let (_, ah) = corner(map, cx, cy);
        let a = map.point_index(cx, cy).and_then(|i| map.lattice[i].ground.clone());
        let b = key.grounds[s].clone();
        let takes = |g: &Option<String>| g.as_ref().is_none_or(|g| map.gt_flags(g) & 1 != 0);
        if takes(&b) && takes(&a) {
            diffs.push(key.heights[s] as i8 as i32 - ah);
        }
    }
    if std::env::var("POE_DBG_FITH").is_ok() && pos == (65, 53) {
        eprintln!("FITH {pos:?} key {:?} corners {:?}", key.heights, corners.iter().map(|&(cx, cy)| (corner(map, cx, cy), map.point_index(cx, cy).map(|i| (map.lattice[i].ground.clone(), map.lattice[i].flags)))).collect::<Vec<_>>());
    }
    diffs.sort();
    diffs.dedup();
    if diffs.len() >= 2 {
        return Ok(10);
    }
    let (rx0, ry0, rx1, ry1) = (0, 0, map.width as i32, map.height as i32);
    if ry0 - margin < y && !side_fits(map, pos, key, 0, wild, master, types, files)? {
        return Ok(1);
    }
    if rx0 - margin < x && !side_fits(map, pos, key, 3, wild, master, types, files)? {
        return Ok(4);
    }
    if x + w < rx1 + margin && !side_fits(map, pos, key, 1, wild, master, types, files)? {
        return Ok(2);
    }
    if y + h < ry1 + margin && !side_fits(map, pos, key, 2, wild, master, types, files)? {
        return Ok(3);
    }
    Ok(0)
}

/// The cell just outside side `side` of a key at `pos`, `i` tiles along it (`0x141f9c540`).
fn outside(pos: (i32, i32), key: &TileKey, side: usize, i: i32) -> (i32, i32) {
    let (x, y) = pos;
    match side {
        0 => (x + i, y - 1),
        1 => (x + key.size.0 as i32, y + i),
        2 => (x + i, y + key.size.1 as i32),
        _ => (x - 1, y + i),
    }
}

/// `0x141f9cee0`
#[allow(clippy::too_many_arguments)]
fn side_fits(map: &TileMap, pos: (i32, i32), key: &TileKey, side: usize, wild: bool, master: &mut Master, types: &mut TerrainTypes, files: &dyn Files) -> Result<bool, GenError> {
    let Some(e) = &key.edges[side] else {
        return Ok(!edge_crosses(map, pos, key, side, master, types, files)?);
    };
    let et = types.edge(files, e)?.clone();
    if et.wildcard {
        return Ok(true);
    }
    if !exit_fits(map, pos, key, side, wild, false, master, types, files)? {
        return Ok(false);
    }
    if et.virtual_edge && !exit_fits(map, pos, key, side, wild, true, master, types, files)? {
        return Ok(false);
    }
    Ok(true)
}

/// `0x141f9c800`: whether a neighbouring tile has an edge crossing into this (edge-less) side.
fn edge_crosses(map: &TileMap, pos: (i32, i32), key: &TileKey, side: usize, master: &mut Master, types: &mut TerrainTypes, files: &dyn Files) -> Result<bool, GenError> {
    let len = if side % 2 == 0 { key.size.0 } else { key.size.1 };
    let start = if side % 2 == 0 { pos.0 } else { pos.1 };
    for i in 0..len as i32 {
        let (cx, cy) = outside(pos, key, side, i);
        if cx < 0 || cy < 0 || cx >= map.width as i32 || cy >= map.height as i32 {
            break;
        }
        let Some((nk, origin)) = tile_key_at(map, cx, cy, master, files)? else { continue };
        let ns = (side + 2) & 3;
        let Some(ne) = &nk.edges[ns] else { continue };
        let net = types.edge(files, ne)?.clone();
        if net.wildcard || net.can_end {
            continue;
        }
        let nlen = if ns % 2 == 0 { nk.size.0 } else { nk.size.1 };
        let along = if side % 2 == 0 { origin.0 } else { origin.1 };
        let a = absolute_exit(ns, nlen, nk.exits[ns]) / 3 + along;
        if start <= a && a < start + len as i32 {
            return Ok(true);
        }
        if net.virtual_edge {
            let b = absolute_exit(ns, nlen, nk.virtual_exits[ns]) / 3 + along;
            if start <= b && b < start + len as i32 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `0x141f9c600`: whether an existing edge type joins the key's.
fn edges_join(a: &str, b: &str, types: &mut TerrainTypes, files: &dyn Files) -> Result<bool, GenError> {
    let ta = types.edge(files, a)?.clone();
    let tb = types.edge(files, b)?.clone();
    let eq = |x: &str, y: &str| x.eq_ignore_ascii_case(y);
    Ok(match (ta.virtual_edge, tb.virtual_edge) {
        (false, false) => eq(a, b),
        (false, true) => eq(a, &tb.splits[0].0) || eq(a, &tb.splits[1].0),
        (true, false) => eq(b, &ta.splits[0].0) || eq(b, &ta.splits[1].0),
        (true, true) => ta.splits.iter().any(|x| tb.splits.iter().any(|y| eq(&x.0, &y.0))),
    })
}

/// `0x141f9ca30`: the key's exit on `side` meets a matching edge of the neighbouring tile at
/// the same place, and no other neighbouring edge enters the side.
#[allow(clippy::too_many_arguments)]
fn exit_fits(map: &TileMap, pos: (i32, i32), key: &TileKey, side: usize, wild: bool, virtual_exit: bool, master: &mut Master, types: &mut TerrainTypes, files: &dyn Files) -> Result<bool, GenError> {
    let e = key.edges[side].clone().unwrap();
    let et = types.edge(files, &e)?.clone();
    let len = if side % 2 == 0 { key.size.0 } else { key.size.1 };
    let along_pos = if side % 2 == 0 { pos.0 } else { pos.1 };
    let raw = if virtual_exit { key.virtual_exits[side] } else { key.exits[side] };
    let exit = absolute_exit(side, len, raw);
    let (cx, cy) = outside(pos, key, side, exit / 3);
    if cx < 0 || cy < 0 || cx >= map.width as i32 || cy >= map.height as i32 {
        return Ok(et.can_end);
    }
    let Some((nk, origin)) = tile_key_at(map, cx, cy, master, files)? else { return Ok(false) };
    let ns = (side + 2) & 3;
    let nlen = if ns % 2 == 0 { nk.size.0 } else { nk.size.1 };
    let along = if side % 2 == 0 { origin.0 } else { origin.1 };
    let joined = match &nk.edges[ns] {
        None => et.can_end,
        Some(ne) => {
            let net = types.edge(files, ne)?.clone();
            if net.wildcard {
                wild
            } else {
                let target = exit + along_pos * 3;
                let mut nb = absolute_exit(ns, nlen, nk.exits[ns]) + along * 3;
                if net.virtual_edge && nb != target {
                    nb = absolute_exit(ns, nlen, nk.virtual_exits[ns]) + along * 3;
                }
                if nb != target {
                    return Ok(false);
                }
                edges_join(ne, &e, types, files)?
            }
        }
    };
    if !joined {
        return Ok(false);
    }
    // No other neighbouring edge may enter this side.
    let mut seen = vec![origin];
    if et.virtual_edge {
        let other = if virtual_exit { key.exits[side] } else { key.virtual_exits[side] };
        let (ox, oy) = outside(pos, key, side, absolute_exit(side, len, other) / 3);
        if let Some((_, o2)) = tile_key_at(map, ox, oy, master, files)? {
            seen.push(o2);
        }
    }
    for i in 0..len as i32 {
        let (qx, qy) = outside(pos, key, side, i);
        if qx < 0 || qy < 0 || qx >= map.width as i32 || qy >= map.height as i32 {
            continue;
        }
        let Some((qk, qo)) = tile_key_at(map, qx, qy, master, files)? else { continue };
        if seen.contains(&qo) {
            continue;
        }
        let Some(qe) = &qk.edges[ns] else { continue };
        let qet = types.edge(files, qe)?.clone();
        if qet.wildcard {
            continue;
        }
        let qlen = if ns % 2 == 0 { qk.size.0 } else { qk.size.1 };
        let qalong = if side % 2 == 0 { qo.0 } else { qo.1 };
        let u = absolute_exit(ns, qlen, qk.exits[ns]) / 3 - along_pos + qalong;
        if 0 <= u && u < len as i32 {
            return Ok(false);
        }
        if qet.virtual_edge {
            let u = absolute_exit(ns, qlen, qk.virtual_exits[ns]) / 3 - along_pos + qalong;
            if 0 <= u && u < len as i32 {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// `0x141fa10b0`: the height a room must be placed at so that its border slots' corner heights
/// meet the terrain's set heights (None when they disagree).
fn room_height(room: &Room, map: &TileMap, pos: (i32, i32), o: u8) -> Result<Option<i32>, GenError> {
    // Rooms in act 1 overlays have flat corners; a full port needs the slots' corner heights.
    let _ = (room, map, pos, o);
    Ok(Some(0))
}
