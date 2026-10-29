//! League mechanic rolls at the start of a run (`phase_cad0` `0x141ffcc50`, before the topology
//! choice). For an area that rolls them, each mechanic draws `below(100)` against a chance from
//! the area's `WorldAreaLeagueChances` row, so the draws shift the whole stream; the stone circle and the
//! Incursion waygate requests are served by the dungeon layout, the other mechanics are not ported. Findings: `docs/re/terrain/tgr.md`.
//!
//! The rolls, in order (mechanic index `i` of the client's mechanics table; chance column
//! offsets are those of the row struct, 4 bytes per column from `+8` for column 1):
//!
//! | i | chance | second chance |
//! |---|---|---|
//! | 0 | `+0x24` (column 8) | `+0x28` (column 9) |
//! | 2 | `+0x40` (column 15) | none |
//! | 9 | `+0x64` (column 24) | none |
//! | 0xb | `+0x68` (column 25) | none |
//!
//! `0x141e22f50` rolls one: a chance above 100 adds `chance / 100` and rolls the remainder; a
//! chance up to 100 rolls once (`below(100) < chance`); then, while the second chance is set and
//! the count is under the mechanic's maximum, it rolls the second chance and keeps going on a hit.
//! Towns, hideouts and some area kinds do not roll; a chance of 100 or more succeeds without a
//! draw (the Act 4 interlude's Incursion waygates). The mechanic table rows' own conditions
//! (stats, maximum, flags) are assumed to pass, as with no area stats.

use poe_data::{GameData, Value};

use crate::rng::TerrainRng;

/// The mechanics rolled, in order: (index, column of the chance, column of the second chance).
const ROLLS: [(u8, usize, Option<usize>); 4] = [(0, 8, Some(9)), (2, 15, None), (9, 24, None), (0xb, 25, None)];

/// The chance test (`0x1420032d0`): below 1 fails and 100 or more succeeds, both without a
/// draw; otherwise `below(100) < chance`.
fn hit(rng: &mut TerrainRng, chance: i64) -> bool {
    if chance < 1 {
        return false;
    }
    if chance >= 100 && std::env::var_os("POE_OLD_ROLL").is_none() {
        return true;
    }
    let d = rng.below(100) as i64;
    if std::env::var_os("POE_DBG_LEAGUE").is_some() {
        eprintln!("draw {d} vs {chance}");
    }
    d < chance
}

/// One roll (`0x141e22f50`); the maximum count is not known per mechanic and taken as unbounded.
fn roll(rng: &mut TerrainRng, chance: i64, second: i64) -> u32 {
    let mut count = 0u32;
    if chance > 0 {
        if chance <= 100 {
            count += hit(rng, chance) as u32;
        } else {
            count += (chance / 100) as u32;
            count += hit(rng, chance % 100) as u32;
        }
        if second > 0 {
            while hit(rng, second) {
                count += 1;
                if count > 100 {
                    break;
                }
            }
        }
    }
    count
}

/// What the rolls of one run decided (the mechanics' counts, in the order of [`ROLLS`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rolled {
    pub counts: [u32; 4],
}

/// The mechanic index whose positive roll adds the `StoneCircle` extra terrain feature
/// (`0x141ffcc50`: mechanic 9).
const STONE_CIRCLE: usize = 2;
/// Mechanic 0xb: `IncursionEncounter`, as many requests as the count.
const INCURSION: usize = 3;

impl Rolled {
    /// Number of stone circles the run rolled.
    pub fn stone_circles(&self) -> u32 {
        self.counts[STONE_CIRCLE]
    }
}

/// Draws the league rolls of `area_id` from `rng`; nothing for areas that do not roll.
pub fn roll_area(gd: &GameData, area_id: &str, rng: &mut TerrainRng) -> Rolled {
    let mut rolled = Rolled::default();
    let Ok(areas) = gd.table("WorldAreas") else { return rolled };
    let Ok(Some(area)) = areas.find_by_str("Id", area_id) else { return rolled };
    // `0x141ffcc50`: no rolls in towns (row +0x14), hideouts (+0x12a) or areas of kind (column
    // 45, +0x1b4) 0xf, 0x13, 0x20, 0x21, 0x23, 0x27 (the tag test at +0x133 is not ported).
    let flag = |c: &str| area.get(c).ok().and_then(|v| v.as_bool()).unwrap_or(false);
    let kind = match area.get_index(45) {
        Ok(Value::Int(v)) => v,
        _ => 0,
    };
    let old = std::env::var_os("POE_ROLL_MAPS_ONLY").is_some();
    if (old && !flag("IsMapArea")) || flag("IsTown") || flag("IsHideout") || matches!(kind, 0xf | 0x13 | 0x20 | 0x21 | 0x23 | 0x27) {
        return rolled;
    }
    // `0x141ffcc50`: a map whose `EndgameMaps` row sets its last bool (column 28, row +0xf0: the
    // Jade Isles, the Copper Citadel, the unique maps) rolls nothing.
    if endgame_no_rolls(gd, area.index()) {
        return rolled;
    }
    let Some(chances) = area.get("LeagueChances").ok().and_then(|v| v.as_row()) else { return rolled };
    let Ok(table) = gd.table("WorldAreaLeagueChances") else { return rolled };
    let Some(row) = table.row(chances) else { return rolled };
    let col = |c: usize| match row.get_index(c) {
        Ok(Value::Int(v)) => v,
        _ => 0,
    };
    for (i, (_, chance, second)) in ROLLS.iter().enumerate() {
        rolled.counts[i] = roll(rng, col(*chance), second.map_or(0, col));
    }
    if std::env::var_os("POE_DBG_LEAGUE").is_some() {
        eprintln!("league rolls {:?} chances {:?}", rolled, ROLLS.iter().map(|r| (col(r.1), r.2.map_or(0, col))).collect::<Vec<_>>());
    }
    rolled
}

/// The `EndgameMaps` row of the `WorldAreas` row `area` (column 0) has column 28 set.
fn endgame_no_rolls(gd: &GameData, area: usize) -> bool {
    let Ok(t) = gd.table("EndgameMaps") else { return false };
    (0..).map_while(|r| t.row(r)).any(|r| r.get_index(0).ok().and_then(|v| v.as_row()) == Some(area) && matches!(r.get_index(28), Ok(Value::Bool(true))))
}

/// The rooms the rolled mechanics add to the generator's pre-unique list (`ExtraTerrainFeatures`
/// rows: `Rooms` `.arm` files, `Tiles` `.tdt` files added to the tile set; the room's
/// orientation mask is "any").
pub fn extra_rooms(gd: &GameData, area_id: &str, rolled: &Rolled) -> Vec<crate::generate::LeagueRoom> {
    let mut out = Vec::new();
    if rolled.stone_circles() > 0 {
        out.extend(extra_feature(gd, "StoneCircle"));
    }
    if std::env::var_os("POE_NO_INCURSION").is_none() {
        // `0x141ffcc50`: the area's waygate tile (`Incursion2EncounterTilePerArea`, `0x141fef790`, else `DefaultIncursion`)
        // joins the tile set (a `.tdt`; a `.tst` would add a tile set, not ported).
        let tile = incursion_tile(gd, area_id);
        for _ in 0..rolled.counts[INCURSION] {
            for mut r in extra_feature(gd, "IncursionEncounter") {
                r.tiles.extend(tile.clone().filter(|t| t.to_ascii_lowercase().ends_with(".tdt")));
                out.push(r);
            }
        }
    }
    out
}

fn incursion_tile(gd: &GameData, area_id: &str) -> Option<String> {
    let area = gd.table("WorldAreas").ok()?.find_by_str("Id", area_id).ok()??.index();
    let t = gd.table("Incursion2EncounterTilePerArea").ok()?;
    let own = (0..).map_while(|r| t.row(r)).find(|r| r.get_index(0).ok().and_then(|v| v.as_row()) == Some(area)).and_then(|r| r.get_index(1).ok()?.as_str().map(String::from));
    // Without a row: `SpecialTiles` row 3 (`DefaultIncursion`, `0x141633ae0`).
    own.or_else(|| gd.table("SpecialTiles").ok()?.row(3)?.str("TDTFile").ok().map(String::from))
}

fn extra_feature(gd: &GameData, id: &str) -> Vec<crate::generate::LeagueRoom> {
    let Ok(table) = gd.table("ExtraTerrainFeatures") else { return Vec::new() };
    let Ok(Some(row)) = table.find_by_str("Id", id) else { return Vec::new() };
    let strings = |c: usize| -> Vec<String> {
        match row.get_index(c) {
            Ok(Value::List(v)) => v.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            _ => Vec::new(),
        }
    };
    let tiles = strings(2);
    strings(1).into_iter().map(|room| crate::generate::LeagueRoom { room, mask: 0xff, tiles: tiles.clone() }).collect()
}
