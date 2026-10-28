//! World-map pins (`docs/re/terrain/shared-passes.md` "World-map pin directions in detail" in the
//! hub): where each area of an act sits on the act's world map. The natural topology choice reads
//! the directions between connected pins, so it needs these positions.
//!
//! The client lays the act's world map out in a town's prepare phase (0.5.5e.hotfix1 `0x141fa5a80`,
//! 0.5.5e `0x141fa5900`, seed = the generation state's seed): it picks one of the world-map area's
//! topologies with [`lcg_index`], generates it like any area, and records where objects whose
//! type is a pin's `MetadataId` land. Those objects are entries of the placed rooms (`.arm`, the
//! room's doodads) and of the placed tiles' model data (`.tmd`, [`tile_objects`]). Each pin type
//! with several positions is reduced to one (`0x141fa3fc0`): the positions sorted by (x, y), then
//! index [`lcg_index`]`(murmur2(MetadataId, 0) ^ seed, n)`. Pins are the act's `MapPins` rows
//! with `WorldAreasKeys`, in row order. Checked against the client's `map_pins` trace lines of
//! acts 1 to 4 (state seed 0, the value in batch jobs and in a live load through this server).

use poe_data::{GameData, Value};

use crate::{
    area::{Forced, Generated, generate_with_files, topology_entries},
    doodads::{TILE, turn_cell},
    generate::{Files, GenError, TileMap, load_tdt},
    hash::murmur2,
    rng::{TerrainRng, lcg_index},
};

fn data(e: impl std::fmt::Display) -> GenError {
    GenError::Unsupported(e.to_string())
}

/// An object a placed tile or room brings to the map, in cells of the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapObject {
    pub x: i32,
    pub y: i32,
    /// The object type (`Metadata/...`), as the file names it.
    pub kind: String,
}

/// The objects of a tile's model data (`.tmd` next to its `.tgt` art, else the one the `.tdt`
/// names), in cells of the tile (its own orientation). Text `.tmd` files (`version 8`): one
/// object per line, `x y ...` first, the quoted object type last; binary ones are skipped.
pub fn tmd_objects(files: &dyn Files, tdt_path: &str) -> Vec<(i32, i32, String)> {
    let Ok(tdt) = load_tdt(files, &tdt_path.to_ascii_lowercase()) else {
        return Vec::new();
    };
    let art = tdt.tgt.as_ref().map(|tgt| format!("{}.tmd", tgt.trim_end_matches(".tgt").trim_end_matches(".TGT")));
    for tmd in [art, tdt.tmd.clone()].into_iter().flatten() {
        let Ok(bytes) = files.read(&tmd.to_ascii_lowercase()) else { continue };
        let text = crate::text::decode_utf16(&bytes);
        if !text.trim_start().starts_with("version") {
            continue;
        }
        let mut out = Vec::new();
        for line in text.lines().skip(1) {
            let mut fields = line.split_whitespace();
            let (Some(x), Some(y)) = (fields.next(), fields.next()) else { continue };
            let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) else { continue };
            let Some(kind) = line.rsplit('"').nth(1).filter(|k| !k.is_empty()) else { continue };
            out.push((x as i32, y as i32, kind.to_string()));
        }
        return out;
    }
    Vec::new()
}

/// The objects of every tile placed in `map`: each `.tmd` entry turned by the tile's orientation
/// within its size ([`turn_cell`], as room entries are) plus its corner.
pub fn tile_objects(files: &dyn Files, map: &TileMap) -> Vec<MapObject> {
    let mut live: Vec<usize> = map.piece_at.iter().flatten().copied().collect();
    live.sort_unstable();
    live.dedup();
    let mut out = Vec::new();
    for i in live {
        let p = &map.pieces[i];
        let (w, h) = (p.size.0 as i32, p.size.1 as i32);
        for (ex, ey, kind) in tmd_objects(files, &p.path) {
            let (x, y) = turn_cell(ex, ey, w, h, p.orientation);
            out.push(MapObject { x: x + p.x * TILE, y: y + p.y * TILE, kind });
        }
    }
    out
}

/// One pin of an act's world map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapPin {
    /// `MapPins` row.
    pub row: usize,
    pub id: String,
    pub x: i32,
    pub y: i32,
}

/// `0x141fa3fc0`: the position a pin type with several candidates keeps.
pub fn consolidate(metadata_id: &str, seed: u32, mut positions: Vec<(i32, i32)>) -> Option<(i32, i32)> {
    positions.retain(|&p| p != (0, 0));
    positions.sort_unstable();
    let k = lcg_index(murmur2(metadata_id.as_bytes(), 0) ^ seed, positions.len() as u32) as usize;
    positions.get(k).copied()
}

/// The world-map area of act `act` (`Acts.ActNumber`), from `Acts.WorldAreaMap`.
pub fn world_map_area(gd: &GameData, act: i64) -> Result<Option<String>, GenError> {
    let acts = gd.table("Acts").map_err(data)?;
    let areas = gd.table("WorldAreas").map_err(data)?;
    for row in acts.rows() {
        if row.get("ActNumber").map_err(data)?.as_int() != Some(act) {
            continue;
        }
        let Some(r) = row.get("WorldAreaMap").map_err(data)?.as_row() else { return Ok(None) };
        let area = areas.row(r).ok_or_else(|| data("bad WorldAreaMap row"))?;
        return Ok(Some(area.str("Id").map_err(data)?));
    }
    Ok(None)
}

/// The act's world map as the client lays it out for its pins (`0x141fa5a80`): topology
/// [`lcg_index`]`(seed, n)` of the world-map area (league mode 1, the default), generated with
/// `seed`, orientation 0.
pub fn world_map(gd: &GameData, files: &dyn Files, act: i64, seed: u32) -> Result<Option<Generated>, GenError> {
    let Some(area) = world_map_area(gd, act)? else { return Ok(None) };
    let n = topology_entries(gd, &area, 1)?.len() as u32;
    if n == 0 {
        return Ok(None);
    }
    let topology = lcg_index(seed, n) + 1;
    let mut rng = TerrainRng::new(seed);
    let forced = Forced { topology: Some(topology), orientation: Some(0) };
    generate_with_files(gd, files, &area, &mut rng, Some(forced)).map(Some)
}

/// The pins of act `act` laid out with the generation state's `seed` (`0x141fa5a80`).
pub fn act_pins(gd: &GameData, files: &dyn Files, act: i64, seed: u32) -> Result<Vec<MapPin>, GenError> {
    let Some(g) = world_map(gd, files, act, seed)? else { return Ok(Vec::new()) };
    let mut objects: Vec<MapObject> = g.doodads.iter().map(|d| MapObject { x: d.x, y: d.y, kind: d.kind.clone() }).collect();
    objects.extend(tile_objects(files, &g.map));

    let pins = gd.table("MapPins").map_err(data)?;
    let mut out = Vec::new();
    for row in pins.rows() {
        if row.get("Act").map_err(data)?.as_int() != Some(act) {
            continue;
        }
        match row.get("WorldAreasKeys").map_err(data)? {
            Value::List(l) if !l.is_empty() => {}
            _ => continue,
        }
        let meta = row.str("MetadataId").map_err(data)?;
        let positions = objects.iter().filter(|o| o.kind.eq_ignore_ascii_case(&meta)).map(|o| (o.x, o.y)).collect();
        if let Some((x, y)) = consolidate(&meta, seed, positions) {
            out.push(MapPin { row: row.index(), id: row.str("Id").map_err(data)?, x, y });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Act 1's pin types with several positions on the seed-0 world map (`act1_NE_01.dgr`), and
    /// the one the client kept (its `map_pins` trace, client 0.5.5e.hotfix1).
    #[test]
    fn consolidation_keeps_the_clients_positions() {
        let base = "Metadata/Terrain/WorldMaps/Act1/Objects/";
        let cases: [(&str, &[(i32, i32)], (i32, i32)); 9] = [
            ("MapPinClearfell", &[(477, 515), (520, 492)], (520, 492)),
            ("MapPinMudBurrow", &[(457, 536), (495, 533), (518, 433), (529, 508), (548, 448)], (495, 533)),
            ("MapPinOldForest", &[(440, 627), (452, 614)], (440, 627)),
            ("MapPinGrimTangle", &[(286, 616), (324, 605), (341, 533)], (286, 616)),
            ("MapPinMausoleum", &[(217, 526), (226, 516), (252, 592), (286, 528)], (286, 528)),
            ("MapPinTomb", &[(226, 572), (227, 553), (264, 506), (285, 563)], (285, 563)),
            ("MapPinTreeOfSouls", &[(436, 632), (464, 628)], (464, 628)),
            ("MapPinVillage", &[(403, 289), (450, 314)], (450, 314)),
            ("MapPinCourtyard", &[(470, 280), (487, 262), (501, 283)], (487, 262)),
        ];
        for (name, positions, kept) in cases {
            let mut shuffled = positions.to_vec();
            shuffled.reverse();
            assert_eq!(consolidate(&format!("{base}{name}"), 0, shuffled), Some(kept), "{name}");
        }
    }
}
