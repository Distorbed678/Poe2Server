//! World-map pins (`docs/re/terrain/shared-passes.md` "World-map pin directions in detail" in the
//! hub): where each area of an act sits on the act's world map. The natural topology choice reads
//! the directions between connected pins, so it needs these positions.
//!
//! The client lays the act's world map out in a town's prepare phase (0.5.5e.hotfix1 `0x141fa5a80`,
//! 0.5.5e `0x141fa5900`, seed = the generation state's seed): it picks one of the world-map area's
//! topologies with [`lcg_index`], loads its graph (seeded preparation: offsets, comma-list room
//! types, existence rolls, shuffle groups) and walks its nodes without generating the map
//! ([`graph_objects`]), recording where objects whose type is a pin's `MetadataId` land: the
//! node's rooms' entries (`.arm`) and their tile slots' model data (`.tmd`). Each pin type with
//! several positions is reduced to one (`0x141fa3fc0`): the positions sorted by (x, y), then
//! index [`lcg_index`]`(murmur2(MetadataId, 0) ^ seed, n)`. Pins are the act's `MapPins` rows
//! with `WorldAreasKeys`, in row order. Checked against the client's `map_pins` trace lines of
//! acts 1, 2, 3, 4, 6 (state seed 0, the value in batch jobs and in a live load through this
//! server): all 102 pins.

use poe_data::{GameData, Value};

use poe_data_tools::file_parsers::arm::types::{Direction, Slot};

use crate::{
    area::{Forced, Generated, generate_with_files, master_entries, topology_entries},
    doodads::{TILE, turn_cell},
    generate::{Files, GenError, Master, TileMap, load_tdt, slot_key},
    graph::Graph,
    hash::murmur2,
    outer_buffer::Margins,
    region::{compose, turn_point},
    rng::{TerrainRng, lcg_index},
    rooms::{RoomSet, node_orientation},
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

/// The objects the client records for a world map's pins (`0x141fb8b50`, 0.5.5e.hotfix1): a walk
/// over the `.dgr`'s nodes, not the generated layout. `graph` nodes recurse into their `.dgr`
/// (offset by their position); every other node with an orientation below 8 takes every room of
/// its type that allows that orientation (`0x141f9bb90`), and for each (`0x141fb8fc0`) the room's
/// objects turned by the node's orientation within the room, plus, for the room's tagged tile
/// slots, the `.tmd` objects of the first tile with that tag (`0x141fb9c50`), turned by the first
/// orientation whose key equals the slot's composed with the node's. Base: the node's unit
/// (`/23`) times `RoomUnitSize − RoomOverlap` tiles, plus the master's `OuterBufferSize` left / top.
/// `.tgr` sub-graphs (`0x141fba130`) are not walked yet; their names go to `skipped`.
pub fn graph_objects(files: &dyn Files, path: &str, rng: &mut TerrainRng, offset: (i32, i32), out: &mut Vec<MapObject>, skipped: &mut Vec<String>, depth: u32) -> Result<(), GenError> {
    let raw = Graph::parse_bytes(&files.read(&path.to_ascii_lowercase())?).map_err(|e| GenError::Data(path.into(), format!("{e:?}")))?;
    let graph = crate::area::prepared_graph(&raw, rng)?;
    let tsi_path = graph.master_file.clone();
    let tsi_bytes = files.read(&tsi_path.to_ascii_lowercase())?;
    let tsi = master_entries(&tsi_bytes, &tsi_path);
    let dir = tsi_path.rsplit_once('/').map_or("", |d| d.0);
    let rel = |k: &str| tsi.get(k).map(|v| if v.len() > 9 && v[..9].eq_ignore_ascii_case("metadata/") { v.to_string() } else { format!("{dir}/{v}") });
    let set = match rel("RoomSet") {
        Some(p) => RoomSet::load(files, &p)?,
        None => RoomSet { rooms: Vec::new(), path: String::new() },
    };
    let master = match (rel("TileSet"), rel("FillTiles")) {
        (Some(t), Some(f)) => Some(Master::load(files, &t, &f)?),
        (Some(t), None) => Some(Master::load(files, &t, "")?),
        _ => None,
    };
    let unit: i32 = tsi.get("RoomUnitSize").and_then(|v| v.trim().parse().ok()).unwrap_or(9);
    let overlap: i32 = tsi.get("RoomOverlap").and_then(|v| v.trim().parse().ok()).unwrap_or(1);
    let step = unit - overlap;
    let margins = Margins::parse(&crate::text::decode_utf16(&tsi_bytes));
    for node in &graph.nodes {
        let o = node_orientation(&node.orientation);
        if o >= 8 {
            continue;
        }
        let unit_pos = ((node.x / TILE) * step * TILE, (node.y / TILE) * step * TILE);
        if node.room_type == "graph" {
            let file = node.tags.first().cloned().unwrap_or_default();
            let lower = file.to_ascii_lowercase();
            if lower.ends_with(".dgr") && depth < 8 {
                graph_objects(files, &file, rng, (offset.0 + unit_pos.0, offset.1 + unit_pos.1), out, skipped, depth + 1)?;
            } else if lower.ends_with(".tgr") {
                skipped.push(file);
            }
            continue;
        }
        let base = (unit_pos.0 + margins.left as i32 * TILE + offset.0, unit_pos.1 + margins.top as i32 * TILE + offset.1);
        for r in set.rooms.iter().filter(|r| r.tag.eq_ignore_ascii_case(&node.room_type) && r.orientations & (1 << o) != 0) {
            let room = &r.room;
            let (w, h) = (room.width() as i32, room.height() as i32);
            for d in &room.arm.doodads {
                let (x, y) = turn_cell(d.x as i32, d.y as i32, w, h, o);
                out.push(MapObject { x: base.0 + x, y: base.1 + y, kind: d.stub.clone() });
            }
            let Some(master) = &master else { continue };
            for (cy, row) in room.arm.grid.iter().enumerate() {
                for (cx, slot) in row.iter().enumerate() {
                    let Slot::K(k) = slot else { continue };
                    let Some(tag) = k.slot_tag.as_deref().filter(|t| !t.is_empty()) else { continue };
                    let Some(first) = master.tiles.tiles.iter().position(|t| t.tdt.feature.as_deref().is_some_and(|f| f.eq_ignore_ascii_case(tag))) else { continue };
                    let tile = &master.tiles.tiles[first];
                    // The slot's key as the room pass reads it (corner heights above the lowest one),
                    // looked up like a placed slot; the tile's first matching orientation counts.
                    let mut key = slot_key(k);
                    let low = key.heights.iter().map(|&v| v as i8).min().unwrap_or(0);
                    for v in &mut key.heights {
                        *v = (*v as i8 - low) as u8;
                    }
                    let to = master.tiles.candidates(tag, &key).iter().filter(|c| c.tile == first).map(|c| c.orientation).min().map_or(o, |t| compose(t, o));
                    let (sw, sh) = (key.size.0.max(1) as i32, key.size.1.max(1) as i32);
                    let (x0, y0) = match k.origin {
                        Direction::SE => (cx as i32 - (sw - 1), cy as i32),
                        Direction::NE => (cx as i32 - (sw - 1), cy as i32 - (sh - 1)),
                        Direction::NW => (cx as i32, cy as i32 - (sh - 1)),
                        _ => (cx as i32, cy as i32),
                    };
                    let (ax, ay) = turn_point(o, w, h, x0, y0);
                    let (bx, by) = turn_point(o, w, h, x0 + sw - 1, y0 + sh - 1);
                    let (lx, ly) = (ax.min(bx), ay.min(by));
                    let (tw, th) = (tile.key.size.0.max(1) as i32, tile.key.size.1.max(1) as i32);
                    for (ex, ey, kind) in tmd_objects(files, &tile.path) {
                        let (tx, ty) = turn_cell(ex, ey, tw, th, to);
                        if std::env::var("POE_DBG_PINS").is_ok_and(|w| kind.to_ascii_lowercase().contains(&w.to_ascii_lowercase())) {
                            eprintln!("DBG pin {kind} at {:?} room {} slot ({cx},{cy}) tag {tag} tile {} o{o} to{to}", (base.0 + lx * TILE + tx, base.1 + ly * TILE + ty), r.path, tile.path);
                        }
                        out.push(MapObject { x: base.0 + lx * TILE + tx, y: base.1 + ly * TILE + ty, kind });
                    }
                }
            }
        }
    }
    Ok(())
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

/// [`act_pins`] from the generated world map's placed rooms and tiles instead of the client's
/// walk: right where the generator turns a tile like the walk does (acts 1, 2, 4; act 3 17 of
/// 19). Kept for comparison.
pub fn act_pins_generated(gd: &GameData, files: &dyn Files, act: i64, seed: u32) -> Result<Vec<MapPin>, GenError> {
    let Some(g) = world_map(gd, files, act, seed)? else { return Ok(Vec::new()) };
    let mut objects: Vec<MapObject> = g.doodads.iter().map(|d| MapObject { x: d.x, y: d.y, kind: d.kind.clone() }).collect();
    objects.extend(tile_objects(files, &g.map));
    pins_from(gd, act, seed, &objects)
}

/// The pins of act `act` laid out with the generation state's `seed` (`0x141fa5a80`): topology
/// [`lcg_index`]`(seed, n)` of the act's world map, the graph prepared with a fresh
/// [`TerrainRng`] from `seed` (world maps are area kind 0x21: no sub-seed draw), its objects from
/// [`graph_objects`], then [`consolidate`] per pin type. Matches the client's `map_pins` trace
/// lines for every act with a world map (1, 2, 3, 4, 6: 102 pins; the endgame has none).
pub fn act_pins(gd: &GameData, files: &dyn Files, act: i64, seed: u32) -> Result<Vec<MapPin>, GenError> {
    let Some(area) = world_map_area(gd, act)? else { return Ok(Vec::new()) };
    let topologies = topology_entries(gd, &area, 1)?;
    if topologies.is_empty() {
        return Ok(Vec::new());
    }
    let path = &topologies[lcg_index(seed, topologies.len() as u32) as usize].0;
    let (mut objects, mut skipped) = (Vec::new(), Vec::new());
    let mut rng = TerrainRng::new(seed);
    graph_objects(files, path, &mut rng, (0, 0), &mut objects, &mut skipped, 0)?;
    pins_from(gd, act, seed, &objects)
}

/// The act's pins (`MapPins` rows with `WorldAreasKeys`) at their consolidated positions.
fn pins_from(gd: &GameData, act: i64, seed: u32, objects: &[MapObject]) -> Result<Vec<MapPin>, GenError> {
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

    /// Act 1's pins against the client's `map_pins` trace (client 0.5.5e.hotfix1, state seed 0).
    /// Needs the game files (`GameFiles/` of the hub, `POE_GAME_DIR` to point elsewhere); skipped
    /// without them.
    #[test]
    fn act_1_pins_are_the_clients() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let game = std::env::var_os("POE_GAME_DIR").map_or_else(|| root.join("GameFiles"), std::path::PathBuf::from);
        let schema = root.join("third_party/dat-schema/schema-poe2.min.json");
        if !game.join("Bundles2").is_dir() || !schema.is_file() {
            eprintln!("skipped: no game files at {}", game.display());
            return;
        }
        let gd = GameData::open(&game, &schema).unwrap();
        let files = crate::generate::CachedFiles::new(gd.files());
        let pins: Vec<(String, i32, i32)> = act_pins(&gd, &files, 1, 0).unwrap().into_iter().map(|p| (p.id, p.x, p.y)).collect();
        let client = [
            ("G1_1", 424, 518), ("G1_town", 484, 463), ("G1_2", 520, 492), ("G1_3", 495, 533), ("G1_4", 440, 627),
            ("ExpeditionSubArea_Kalguur_Act1", 451, 652), ("G1_5", 375, 621), ("G1_6", 286, 616), ("G1_7", 257, 539),
            ("G1_8", 286, 528), ("G1_9", 285, 563), ("G1_10", 464, 628), ("G1_11", 241, 425), ("G1_12", 391, 437),
            ("G1_13_1", 352, 326), ("G1_13_2", 450, 314), ("G1_14", 487, 262), ("G1_15", 485, 291),
        ];
        let client: Vec<(String, i32, i32)> = client.iter().map(|&(a, x, y)| (a.to_string(), x, y)).collect();
        assert_eq!(pins, client);
    }

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
