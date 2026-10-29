//! The natural topology choice (`docs/re/terrain/shared-passes.md` "World-map pin directions in
//! detail" in the hub): which of an area's topologies and region orientations the client picks
//! when the server forces none. Client addresses are 0.5.5e Hotfix 1 ones.
//!
//! The generation state (world `+0x4b0`) holds a direction list (`+0x70`: per pin, the direction
//! from the area's pin to it). A town or an area of kind 2 (`WorldAreas` `+0x14`, `+0x1b4`)
//! rebuilds it on every load (`0x141fa60b0`): its act's pins ([`crate::map_pins::act_pins`]), the
//! direction of every connected pin (`MapConnections`, [`neighbours`]), adjusted for the whole act
//! so that every area keeps a matching topology ([`layout`]). The area's connections then select
//! the topologies whose direction keys match ([`candidates`]: exact, then with wildcards, then
//! with adjacent directions), the generator's sub-seed picks one, and its keys are written back
//! ([`write_back`]). The client starts every load with an empty list (live trace 2026-10-28: the
//! state does not outlive a load), so an area that is neither a town nor kind 2 takes the index
//! draw. Checked against 4,173 unforced client generations (batch `w13-nat1`): all match.

use std::collections::BTreeMap;

use poe_data::{GameData, Value};

use crate::{
    generate::{Files, GenError},
    map_pins::{MapPin, act_pins},
    rng::lcg_index,
    voronoi::isqrt,
};

fn data(e: impl std::fmt::Display) -> GenError {
    GenError::Unsupported(e.to_string())
}

/// `0x141f8c5b0`: direction `d` (0 N, 1 E, 2 S, 3 W, 4 NW, 5 NE, 6 SE, 7 SW, 8 none) under turn
/// code `t` is `TURN[t][d]`. Row 2 is the opposite direction.
const TURN: [[u8; 9]; 9] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8],
    [1, 2, 3, 0, 5, 6, 7, 4, 8],
    [2, 3, 0, 1, 6, 7, 4, 5, 8],
    [3, 0, 1, 2, 7, 4, 5, 6, 8],
    [0, 3, 2, 1, 5, 4, 7, 6, 8],
    [1, 0, 3, 2, 6, 5, 4, 7, 8],
    [2, 1, 0, 3, 7, 6, 5, 4, 8],
    [3, 2, 1, 0, 4, 7, 6, 5, 8],
    [0, 1, 2, 3, 4, 5, 6, 7, 8],
];

/// Turn code of a region orientation (`0x142f0bf50`).
const ORIENTATION_TURN: [u8; 8] = [0, 3, 2, 1, 4, 5, 6, 7];

/// The directions next to each one (pass 3, `0x141fa6cc0`).
const ADJACENT: [[u8; 2]; 8] = [[4, 5], [5, 6], [6, 7], [4, 7], [0, 3], [0, 1], [1, 2], [2, 3]];

/// `0x142f10780`: the angle (u16, 0x8000 = 180°) of the cosine `(i − 127) / 127`, i = 0..254.
const ACOS: [u16; 255] = [
    0x8000, 0x7ae2, 0x78c2, 0x7720, 0x75bf, 0x7487, 0x736d, 0x7269, 0x7176, 0x7092, 0x6fb9, 0x6eeb, 0x6e25, 0x6d67,
    0x6cb0, 0x6bff, 0x6b54, 0x6aad, 0x6a0b, 0x696d, 0x68d3, 0x683c, 0x67a8, 0x6718, 0x668a, 0x65ff, 0x6576, 0x64f0,
    0x646c, 0x63ea, 0x636a, 0x62eb, 0x626f, 0x61f4, 0x617a, 0x6102, 0x608c, 0x6017, 0x5fa3, 0x5f31, 0x5ebf, 0x5e4f,
    0x5de0, 0x5d72, 0x5d05, 0x5c99, 0x5c2e, 0x5bc4, 0x5b5a, 0x5af2, 0x5a8a, 0x5a23, 0x59bd, 0x5958, 0x58f3, 0x588f,
    0x582c, 0x57c9, 0x5767, 0x5705, 0x56a4, 0x5644, 0x55e4, 0x5585, 0x5526, 0x54c8, 0x546a, 0x540c, 0x53af, 0x5353,
    0x52f7, 0x529b, 0x5240, 0x51e5, 0x518a, 0x5130, 0x50d6, 0x507d, 0x5024, 0x4fcb, 0x4f72, 0x4f1a, 0x4ec2, 0x4e6a,
    0x4e13, 0x4dbc, 0x4d65, 0x4d0e, 0x4cb8, 0x4c61, 0x4c0b, 0x4bb6, 0x4b60, 0x4b0b, 0x4ab6, 0x4a61, 0x4a0c, 0x49b7,
    0x4963, 0x490f, 0x48bb, 0x4867, 0x4813, 0x47bf, 0x476b, 0x4718, 0x46c5, 0x4671, 0x461e, 0x45cb, 0x4578, 0x4526,
    0x44d3, 0x4480, 0x442e, 0x43db, 0x4389, 0x4336, 0x42e4, 0x4291, 0x423f, 0x41ed, 0x419b, 0x4149, 0x40f6, 0x40a4,
    0x4052, 0x4000, 0x3fae, 0x3f5c, 0x3f0a, 0x3eb7, 0x3e65, 0x3e13, 0x3dc1, 0x3d6f, 0x3d1c, 0x3cca, 0x3c77, 0x3c25,
    0x3bd2, 0x3b80, 0x3b2d, 0x3ada, 0x3a88, 0x3a35, 0x39e2, 0x398f, 0x393b, 0x38e8, 0x3895, 0x3841, 0x37ed, 0x3799,
    0x3745, 0x36f1, 0x369d, 0x3649, 0x35f4, 0x359f, 0x354a, 0x34f5, 0x34a0, 0x344a, 0x33f5, 0x339f, 0x3348, 0x32f2,
    0x329b, 0x3244, 0x31ed, 0x3196, 0x313e, 0x30e6, 0x308e, 0x3035, 0x2fdc, 0x2f83, 0x2f2a, 0x2ed0, 0x2e76, 0x2e1b,
    0x2dc0, 0x2d65, 0x2d09, 0x2cad, 0x2c51, 0x2bf4, 0x2b96, 0x2b38, 0x2ada, 0x2a7b, 0x2a1c, 0x29bc, 0x295c, 0x28fb,
    0x2899, 0x2837, 0x27d4, 0x2771, 0x270d, 0x26a8, 0x2643, 0x25dd, 0x2576, 0x250e, 0x24a6, 0x243c, 0x23d2, 0x2367,
    0x22fb, 0x228e, 0x2220, 0x21b1, 0x2141, 0x20cf, 0x205d, 0x1fe9, 0x1f74, 0x1efe, 0x1e86, 0x1e0c, 0x1d91, 0x1d15,
    0x1c96, 0x1c16, 0x1b94, 0x1b10, 0x1a8a, 0x1a01, 0x1976, 0x18e8, 0x1858, 0x17c4, 0x172d, 0x1693, 0x15f5, 0x1553,
    0x14ac, 0x1401, 0x1350, 0x1299, 0x11db, 0x1115, 0x1047, 0x0f6e, 0x0e8a, 0x0d97, 0x0c93, 0x0b79, 0x0a41, 0x08e0,
    0x073e, 0x051e, 0x0000,
];

/// An entry of the direction list: the direction from the area's pin to `pin` (a `MapPins` row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinDir {
    pub dir: u8,
    pub pin: usize,
}

/// What the client's generation state keeps for the natural choice: its seed (world `+0x4b0`, the
/// load's instance word; 0 with this server) and the direction list (`+0x70`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinState {
    pub seed: u32,
    pub dirs: Vec<PinDir>,
}

/// A natural candidate: a topology (index into the mode-filtered list,
/// [`crate::area::topology_entries`]) with a region orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub topology: usize,
    pub orientation: u8,
}

/// The tables the choice reads, by row.
struct Tables {
    /// `MapPins`: `WorldAreasKeys`, `ParentMapPin`.
    pin_areas: Vec<Vec<usize>>,
    pin_parent: Vec<Option<usize>>,
    /// World area → the pin listing it in `WorldAreasKeys` (the map at the pin table's `+0x58`).
    area_pin: BTreeMap<usize, usize>,
    /// `MapConnections` pin pairs, both orders.
    connected: std::collections::BTreeSet<(usize, usize)>,
    /// `WorldAreas`: `Connections`, `HASH16`.
    area_connections: Vec<Vec<usize>>,
    area_hash: Vec<i64>,
}

fn rows(v: Value) -> Vec<usize> {
    match v {
        Value::List(l) => l.iter().filter_map(Value::as_row).collect(),
        _ => Vec::new(),
    }
}

fn tables(gd: &GameData) -> Result<std::rc::Rc<Tables>, GenError> {
    thread_local! {
        static CACHE: std::cell::RefCell<Option<std::rc::Rc<Tables>>> = const { std::cell::RefCell::new(None) };
    }
    if let Some(t) = CACHE.with(|c| c.borrow().clone()) {
        return Ok(t);
    }
    let pins = gd.table("MapPins").map_err(data)?;
    let (mut pin_areas, mut pin_parent, mut area_pin) = (Vec::new(), Vec::new(), BTreeMap::new());
    for row in pins.rows() {
        let areas = rows(row.get("WorldAreasKeys").map_err(data)?);
        for &a in &areas {
            area_pin.entry(a).or_insert(row.index());
        }
        pin_areas.push(areas);
        pin_parent.push(row.get("ParentMapPin").map_err(data)?.as_row());
    }
    let mut connected = std::collections::BTreeSet::new();
    for row in gd.table("MapConnections").map_err(data)?.rows() {
        if let (Some(a), Some(b)) = (row.get("MapPinsKey0").map_err(data)?.as_row(), row.get("MapPinsKey1").map_err(data)?.as_row()) {
            connected.insert((a, b));
            connected.insert((b, a));
        }
    }
    let (mut area_connections, mut area_hash) = (Vec::new(), Vec::new());
    for row in gd.table("WorldAreas").map_err(data)?.rows() {
        area_connections.push(rows(row.get("Connections").map_err(data)?));
        area_hash.push(row.int("HASH16").map_err(data)?);
    }
    let t = std::rc::Rc::new(Tables { pin_areas, pin_parent, area_pin, connected, area_connections, area_hash });
    CACHE.with(|c| *c.borrow_mut() = Some(t.clone()));
    Ok(t)
}

/// A topology of the mode-filtered list with what the match reads: its direction keys
/// (Topologies column 5, `+0x1c`) and allowed orientations (column 6, `+0x2c`), as listed.
struct Topology {
    /// `Topologies` row (the candidate set's sort key).
    row: usize,
    keys: Vec<u8>,
    orientations: Vec<i64>,
}

fn int_list(v: Value) -> Vec<i64> {
    match v {
        Value::List(l) => l.iter().filter_map(Value::as_int).collect(),
        _ => Vec::new(),
    }
}

/// `0x141fdffb0`: the area's topologies of league mode `mode` (else those marked 0x39, else mode
/// 1), as [`crate::area::topology_entries`] lists them.
fn topologies(gd: &GameData, area: usize, mode: i64) -> Result<Vec<Topology>, GenError> {
    let areas = gd.table("WorldAreas").map_err(data)?;
    let row = areas.row(area).ok_or_else(|| data("bad area row"))?;
    let table = gd.table("Topologies").map_err(data)?;
    let last = table.schema().columns.len() - 1;
    let (mut exact, mut any) = (Vec::new(), Vec::new());
    for r in rows(row.get("Topologies").map_err(data)?) {
        let t = table.row(r).ok_or_else(|| data("bad topology row"))?;
        let entry = Topology {
            row: r,
            keys: int_list(t.get_index(5).map_err(data)?).into_iter().map(|k| k as u8).collect(),
            orientations: int_list(t.get_index(6).map_err(data)?),
        };
        match t.get_index(last).map_err(data)?.as_int() {
            Some(m) if m == mode => exact.push(entry),
            Some(0x39) => any.push(entry),
            _ => {}
        }
    }
    if !exact.is_empty() {
        return Ok(exact);
    }
    if any.is_empty() && mode != 1 {
        return topologies(gd, area, 1);
    }
    Ok(any)
}

/// `0x1421c5090(a, b)`: the angle between two vectors (u16, 0x8000 = 180°).
fn angle(a: (i32, i32), b: (i32, i32)) -> u16 {
    if a == (0, 0) || b == (0, 0) {
        return 0;
    }
    let la = a.0 * a.0 + a.1 * a.1;
    let lb = b.0 * b.0 + b.1 * b.1;
    let s = (isqrt((lb * la) as u32) as i32).max(1);
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let c = (((lb - dy * dy - dx * dx + la) * 0x7f / s) / 2) as i8 as i32 + 0x7f;
    ACOS[c.clamp(0, 0xfe) as usize]
}

/// The direction of the offset `(dx, dy)` (map cells, y down) in `0x141fa4930`.
fn direction(dx: i32, dy: i32) -> u8 {
    match (dx, dy) {
        (0, 0) => 8,
        (0, y) => if y > 0 { 2 } else { 0 },
        (x, 0) => if x > 0 { 1 } else { 3 },
        _ => {
            let sector = ((angle((dx, dy), (1, 0)) as u32 + 0x1000) >> 13) as usize;
            if dy > 0 { [1, 6, 2, 7, 3][sector] } else { [1, 5, 0, 4, 3][sector] }
        }
    }
}

/// `0x141fa4930`: the direction from `pin` to every other pin of `pins` it shares a
/// `MapConnections` row with, sorted by pin row.
pub fn neighbours(gd: &GameData, pin: usize, pins: &[MapPin]) -> Result<Vec<PinDir>, GenError> {
    let t = tables(gd)?;
    let own = pins.iter().rev().find(|p| p.row == pin).map_or((0, 0), |p| (p.x, p.y));
    let mut others: BTreeMap<usize, (i32, i32)> = BTreeMap::new();
    for p in pins.iter().filter(|p| p.row != pin) {
        if t.connected.contains(&(pin, p.row)) {
            others.insert(p.row, (p.x, p.y));
        }
    }
    Ok(others.into_iter().map(|(row, (x, y))| PinDir { dir: direction(x - own.0, y - own.1), pin: row }).collect())
}

/// `0x141fa62d0`: the direction of each of the area's connections, in `WorldAreas.Connections`
/// order, once per area hash: the list entry of the connected area's pin (or of a pin whose
/// `ParentMapPin` it is). Connections whose pin also lists `area` are skipped.
fn connection_dirs(t: &Tables, area: usize, list: &[PinDir]) -> Vec<(usize, u8)> {
    let mut seen: Vec<i64> = Vec::new();
    let mut out = Vec::new();
    for &c in t.area_connections.get(area).map(Vec::as_slice).unwrap_or(&[]) {
        let hash = t.area_hash.get(c).copied().unwrap_or(-1);
        if seen.contains(&hash) {
            continue;
        }
        let Some(&pin) = t.area_pin.get(&c) else { continue };
        if t.pin_areas[pin].contains(&area) {
            continue;
        }
        if let Some(i) = list.iter().position(|e| e.pin == pin || t.pin_parent.get(e.pin).copied().flatten() == Some(pin)) {
            out.push((i, list[i].dir));
            seen.push(hash);
        }
    }
    out
}

/// One pass of `0x141fa65c0` (`0x141fa6ae0` exact, `0x141fa6bd0` key 8 = any, `0x141fa6cc0` also
/// the adjacent directions): do the connections, turned by orientation `o`, match the keys?
fn matches(pass: u8, dirs: &[(usize, u8)], keys: &[u8], o: u8) -> bool {
    if dirs.len() != keys.len() {
        return false;
    }
    let turn = ORIENTATION_TURN[o as usize] as usize;
    dirs.iter().zip(keys).all(|(&(_, d), &k)| {
        let turned = TURN[turn][d.min(8) as usize];
        turned == k
            || (pass >= 2 && k == 8)
            || (pass == 3 && turned < 8 && k < 8 && ADJACENT[turned as usize].contains(&k))
    })
}

/// `0x141fa65c0`: the (topology, orientation) pairs whose direction keys match the area's
/// connections under `list`; `orientation` < 8 tries only that one (`map_force_orientation`),
/// else each topology's allowed list (an entry 8 = all eight). The candidates are a set sorted by
/// (`Topologies` row, orientation) (`0x141fa98e0`), not in list order. Empty when the list is empty or
/// the first topology has no keys; the client throws when the area has no connection with a pin
/// in the list ("is not connected to any areas with connected MapPins!").
pub fn candidates(gd: &GameData, area: usize, list: &[PinDir], mode: i64, orientation: u8) -> Result<Vec<Candidate>, GenError> {
    if list.is_empty() {
        return Ok(Vec::new());
    }
    let tops = topologies(gd, area, mode)?;
    if tops.first().is_none_or(|t| t.keys.is_empty()) {
        return Ok(Vec::new());
    }
    let t = tables(gd)?;
    let dirs = connection_dirs(&t, area, list);
    if dirs.is_empty() {
        return Err(data(format!("world area row {area} is not connected to any areas with connected MapPins")));
    }
    let mut out: BTreeMap<(usize, u8), Candidate> = BTreeMap::new();
    for pass in 1..=3 {
        for (i, top) in tops.iter().enumerate() {
            let orientations: Vec<i64> = if orientation < 8 {
                vec![orientation as i64]
            } else if top.orientations.contains(&8) {
                (0..8).collect()
            } else {
                top.orientations.clone()
            };
            for o in orientations.into_iter().map(|o| o as u8).filter(|&o| o < 8) {
                if matches(pass, &dirs, &top.keys, o) {
                    out.entry((top.row, o)).or_insert(Candidate { topology: i, orientation: o });
                }
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    Ok(out.into_values().collect())
}

/// `0x141fa4f20`: every pin's direction list, adjusted so each area keeps a candidate. Pins with
/// world areas, sorted by row and then (stably) by neighbour count, most first; each takes the
/// keys of the topology the LCG (seeded with `seed`, afresh for each pin) picks among its first
/// area's candidates, one direction at a time and only while candidates remain; then each
/// neighbour's entry pointing back takes the opposite direction under the same rule.
pub fn layout(gd: &GameData, pins: &[MapPin], seed: u32, mode: i64) -> Result<BTreeMap<usize, Vec<PinDir>>, GenError> {
    let t = tables(gd)?;
    let mut lists: BTreeMap<usize, Vec<PinDir>> = BTreeMap::new();
    let mut order = Vec::new();
    for p in pins.iter().filter(|p| !t.pin_areas[p.row].is_empty()) {
        lists.insert(p.row, neighbours(gd, p.row, pins)?);
        order.push(p.row);
    }
    order.sort();
    order.sort_by_key(|p| std::cmp::Reverse(lists[p].len()));
    let first_area = |pin: usize| t.pin_areas[pin][0];
    for &pin in &order {
        let area = first_area(pin);
        let cands = candidates(gd, area, &lists[&pin], mode, 8)?;
        if !cands.is_empty() {
            let pick = cands[lcg_index(seed, cands.len() as u32) as usize];
            let keys = topologies(gd, area, mode)?.swap_remove(pick.topology).keys;
            if keys.iter().any(|&k| k != 8) && keys.len() == lists[&pin].len() {
                for (i, &k) in keys.iter().enumerate() {
                    let old = lists[&pin][i].dir;
                    if k == 8 || old == k {
                        continue;
                    }
                    lists.get_mut(&pin).unwrap()[i].dir = k;
                    if candidates(gd, area, &lists[&pin], mode, 8)?.is_empty() {
                        lists.get_mut(&pin).unwrap()[i].dir = old;
                    }
                }
            }
        }
        for e in lists[&pin].clone() {
            if t.pin_areas.get(e.pin).is_none_or(Vec::is_empty) {
                continue;
            }
            let back = TURN[2][e.dir.min(8) as usize];
            let list = lists.entry(e.pin).or_default();
            let Some(i) = list.iter().position(|b| b.pin == pin) else { continue };
            let old = list[i].dir;
            if old == back {
                continue;
            }
            list[i].dir = back;
            if candidates(gd, first_area(e.pin), &lists[&e.pin], mode, 8)?.is_empty() {
                lists.get_mut(&e.pin).unwrap()[i].dir = old;
            }
        }
    }
    Ok(lists)
}

/// `0x141fa60b0`: the direction list a town or kind-2 area loads with: its pin's list of the act
/// [`layout`] (its plain [`neighbours`] when the layout has none for it); empty without a pin.
pub fn area_dirs(gd: &GameData, area: usize, pins: &[MapPin], seed: u32, mode: i64) -> Result<Vec<PinDir>, GenError> {
    let t = tables(gd)?;
    let Some(&pin) = t.area_pin.get(&area) else { return Ok(Vec::new()) };
    if pins.is_empty() {
        return Ok(Vec::new());
    }
    match layout(gd, pins, seed, mode)?.remove(&pin) {
        Some(list) => Ok(list),
        None => neighbours(gd, pin, pins),
    }
}

/// `0x141fa6e00`: after a natural pick, the chosen topology's keys (≠ 8) replace the directions
/// of the list entries the area's connections matched, in connection order; only when the
/// topology has as many keys as the list has entries.
pub fn write_back(gd: &GameData, area: usize, list: &mut [PinDir], topology: usize, mode: i64) -> Result<(), GenError> {
    let keys = topologies(gd, area, mode)?.swap_remove(topology).keys;
    if list.is_empty() || keys.is_empty() || keys.len() != list.len() {
        return Ok(());
    }
    let t = tables(gd)?;
    for (n, (i, _)) in connection_dirs(&t, area, list).into_iter().enumerate() {
        let k = *keys.get(n).ok_or_else(|| data("Attempted to get invalid index for 'm_DirectionKey'"))?;
        if k != 8 {
            list[i].dir = k;
        }
    }
    Ok(())
}

/// The act pins a load lays out (`0x141ffe970` prepare phase: towns and kind-2 areas), cached per
/// (act, seed).
pub fn cached_act_pins(gd: &GameData, files: &dyn Files, act: i64, seed: u32) -> Result<Vec<MapPin>, GenError> {
    thread_local! {
        static CACHE: std::cell::RefCell<BTreeMap<(i64, u32), Vec<MapPin>>> = const { std::cell::RefCell::new(BTreeMap::new()) };
    }
    if let Some(p) = CACHE.with(|c| c.borrow().get(&(act, seed)).cloned()) {
        return Ok(p);
    }
    let pins = act_pins(gd, files, act, seed)?;
    CACHE.with(|c| c.borrow_mut().insert((act, seed), pins.clone()));
    Ok(pins)
}

/// The natural choice of `0x141ff9400` for the area (`WorldAreas` row), before the pick: the
/// direction list is rebuilt for a town or kind-2 area (into `state`), then the candidates. The
/// caller draws the index (`lcg_index(sub, n)`, `sub` the `below(0xffffffff)` draw, or the state
/// seed for kind 0x21), then calls [`write_back`].
pub fn natural_candidates(gd: &GameData, files: &dyn Files, area: usize, state: &mut PinState, mode: i64, orientation: u8) -> Result<Vec<Candidate>, GenError> {
    let areas = gd.table("WorldAreas").map_err(data)?;
    let row = areas.row(area).ok_or_else(|| data("bad area row"))?;
    let kind = row.get_index(45).map_err(data)?.as_int();
    if row.bool("IsTown").map_err(data)? || kind == Some(2) {
        let pins = cached_act_pins(gd, files, row.int("Act").map_err(data)?, state.seed)?;
        state.dirs = area_dirs(gd, area, &pins, state.seed, mode)?;
    }
    candidates(gd, area, &state.dirs, mode, orientation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_by_angle() {
        assert_eq!(direction(0, -5), 0);
        assert_eq!(direction(5, 0), 1);
        assert_eq!(direction(0, 5), 2);
        assert_eq!(direction(-5, 0), 3);
        assert_eq!(direction(5, -5), 5);
        assert_eq!(direction(-5, -5), 4);
        assert_eq!(direction(5, 5), 6);
        assert_eq!(direction(-5, 5), 7);
        assert_eq!(direction(10, -1), 1);
        assert_eq!(direction(0, 0), 8);
    }

    /// `G1_2` seed 1001 without a forced layout, as a live load through this server generated it
    /// (client 0.5.5e.hotfix1, 2026-10-28): topology 15 (`felling_S_N_W`), orientation 0. Needs
    /// the game files (`GameFiles/` of the hub, `POE_GAME_DIR` to point elsewhere); skipped
    /// without them.
    #[test]
    fn natural_choice_is_the_clients() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let game = std::env::var_os("POE_GAME_DIR").map_or_else(|| root.join("GameFiles"), std::path::PathBuf::from);
        let schema = root.join("third_party/dat-schema/schema-poe2.min.json");
        if !game.join("Bundles2").is_dir() || !schema.is_file() {
            eprintln!("skipped: no game files at {}", game.display());
            return;
        }
        let gd = GameData::open(&game, &schema).unwrap();
        let files = crate::generate::CachedFiles::new(gd.files());
        let g = crate::area::generate_with_files(&gd, &files, "G1_2", &mut crate::rng::TerrainRng::new(1001), None).unwrap();
        assert_eq!(g.map.tile_hash(), 3122685737);
    }
}
