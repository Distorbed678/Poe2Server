//! Post-unique tiles (`0x141ff2090`, per region after the fill and the de-repetition, before the
//! outer buffer): tiles of the tile set whose `.tdt` feature is `postunique` (quest item drops
//! such as `T2_WeaponDrop_01..10` in `G1_5`) each replace one tile that has the same tile-set
//! key. They are not in the tile-set index (`0x141d09ab0` puts them on the master's own list,
//! in `.tst` order, with their rotation mask).
//!
//! Per list entry: the index keys of the tile in each allowed orientation; every region cell
//! (row by row) that is the origin of a feature-less tile with at least one walkable corner
//! (lattice flag 8 clear on one of the four corners of its footprint) and whose oriented index
//! key equals one of the entry's keys is a candidate, with an orientation drawn among the
//! equal ones (`0x141ff2920`: `below(n)`, none when no key is equal); then `below(candidates)`
//! picks the cell, and the tile is placed there. No candidate is an error of the client's
//! ("Failed to place post-unique tile").

use crate::{
    generate::{Files, GenError, Master, TileMap},
    rng::TerrainRng,
    tileset::TileKey,
};

/// The `postunique` feature name (an exact, case-sensitive compare).
const FEATURE: &str = "postunique";

pub fn run(map: &mut TileMap, master: &mut Master, files: &dyn Files, rng: &mut TerrainRng) -> Result<(), GenError> {
    let entries: Vec<(usize, u8)> = master
        .tiles
        .tiles
        .iter()
        .enumerate()
        .filter(|(_, t)| t.tdt.feature.as_deref() == Some(FEATURE))
        .map(|(i, t)| (i, t.orientations))
        .collect();
    for (ti, mask) in entries {
        let keys: Vec<(u8, TileKey)> = (0..8u8).filter(|o| mask & (1 << o) != 0).map(|o| (o, master.tiles.tiles[ti].key.oriented(o))).collect();
        // (x, y, orientation, height)
        let mut cands: Vec<(i32, i32, u8, i16)> = Vec::new();
        // The region's rectangle in the full map, so the cells a fill tile overhangs by one past
        // the region are seen too (MapFrozenFalls: ledge tiles at x = -1).
        let r = if std::env::var_os("POE_PU_REGION_ONLY").is_some() { 0 } else { 1 };
        for y in -r..map.height as i32 + r {
            for x in -r..map.width as i32 + r {
                let Some(cell) = map.cell_or_spill(x, y).cloned() else { continue };
                if cell.offset != (0, 0) {
                    continue;
                }
                let tdt = master.header(files, &cell.path)?.clone();
                if tdt.feature.as_deref().is_some_and(|f| !f.is_empty()) {
                    if std::env::var_os("POE_DBG_PU").is_some() {
                        eprintln!("PU featured ({x},{y}) {} {:?}", cell.path, tdt.feature);
                    }
                    continue;
                }
                let mut key = match map.piece_or_spill(x, y).and_then(|p| map.pieces[p].tile) {
                    Some(t) => master.tiles.tiles[t].key.clone(),
                    None => {
                        let mut k = TileKey::from_tdt(&tdt);
                        for g in k.grounds.iter_mut() {
                            if g.as_ref().is_some_and(|g| map.gt_flags(g) & 0x10 != 0) {
                                *g = None;
                            }
                        }
                        k
                    }
                };
                key = key.oriented(cell.orientation);
                let (kw, kh) = (key.size.0 as i32, key.size.1 as i32);
                let walkable = |px: i32, py: i32| map.point_index(px, py).is_some_and(|k| map.lattice[k].flags & 8 == 0);
                if !(walkable(x, y) || walkable(x, y + kh) || walkable(x + kw, y) || walkable(x + kw, y + kh)) {
                    if std::env::var_os("POE_DBG_PU").is_some() && keys.iter().any(|(_, k)| *k == key) {
                        eprintln!("PU unwalkable ({x},{y}) {}", cell.path);
                    }
                    continue;
                }
                let same: Vec<u8> = keys.iter().filter(|(_, k)| *k == key).map(|(o, _)| *o).collect();
                if std::env::var_os("POE_DBG_PU").is_some() && y >= 68 && key.edges.iter().flatten().any(|e| e.contains("unwalkable_ledge")) {
                    eprintln!("PU row ({x},{y}) {} o{} same {:?} key {:?}", cell.path, cell.orientation, same, key);
                }
                if same.is_empty() {
                    continue;
                }
                let o = same[rng.below(same.len() as u32) as usize];
                if std::env::var_os("POE_DBG_PU").is_some() {
                    eprintln!("PU cand ({x},{y}) o{o} {} key {:?}", cell.path, key);
                }
                cands.push((x, y, o, cell.height));
            }
        }
        if cands.is_empty() {
            return Err(GenError::Unsupported(format!("Failed to place post-unique tile {}", master.tiles.tiles[ti].path)));
        }
        let (x, y, o, h) = cands[rng.below(cands.len() as u32) as usize];
        let (path, tdt) = (master.tiles.tiles[ti].path.clone(), master.tiles.tiles[ti].tdt.clone());
        map.remove_piece_at(x, y);
        map.place(x, y, &path, &tdt, o, h, Some(ti));
    }
    Ok(())
}
