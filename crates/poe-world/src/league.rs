//! League mechanic rolls at the start of a run (`phase_cad0` `0x141ffcad0`, before the topology
//! choice). For an area that rolls them, each mechanic draws `below(100)` against a chance from
//! the area's `WorldAreaLeagueChances` row, so the draws shift the whole stream; the mechanics
//! themselves (extra encounter rooms) are not ported. Findings: `docs/re/terrain/tgr.md`.
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
//! `0x141e22dd0` rolls one: a chance above 100 adds `chance / 100` and rolls the remainder; a
//! chance up to 100 rolls once (`below(100) < chance`); then, while the second chance is set and
//! the count is under the mechanic's maximum, it rolls the second chance and keeps going on a hit.
//! Only areas with `IsMapArea` roll (the campaign's traces show no such draws); the mechanic table
//! rows' own conditions (stats, maximum, flags) are assumed to pass, as with no area stats.

use poe_data::{GameData, Value};

use crate::rng::TerrainRng;

/// The mechanics rolled, in order: (index, column of the chance, column of the second chance).
const ROLLS: [(u8, usize, Option<usize>); 4] = [(0, 8, Some(9)), (2, 15, None), (9, 24, None), (0xb, 25, None)];

/// One roll (`0x141e22dd0`); the maximum count is not known per mechanic and taken as unbounded.
fn roll(rng: &mut TerrainRng, chance: i64, second: i64) -> u32 {
    let mut count = 0u32;
    if chance > 0 {
        if chance <= 100 {
            let d = rng.below(100) as i64;
            if std::env::var_os("POE_DBG_LEAGUE").is_some() { eprintln!("draw {d} vs {chance}"); }
            if d < chance {
                count += 1;
            }
        } else {
            count += (chance / 100) as u32;
            if (rng.below(100) as i64) < chance % 100 {
                count += 1;
            }
        }
    }
    if second > 0 {
        while { let d = rng.below(100) as i64; if std::env::var_os("POE_DBG_LEAGUE").is_some() { eprintln!("draw {d} vs second {second}"); } d < second } {
            count += 1;
            if count > 100 {
                break;
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
/// (`0x141ffcad0`: mechanic 9).
const STONE_CIRCLE: usize = 2;

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
    if !area.get("IsMapArea").ok().and_then(|v| v.as_bool()).unwrap_or(false) {
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

/// The rooms the rolled mechanics add to the generator's pre-unique list (`ExtraTerrainFeatures`
/// rows: `Rooms` `.arm` files, `Tiles` `.tdt` files added to the tile set; the room's
/// orientation mask is "any").
pub fn extra_rooms(gd: &GameData, rolled: &Rolled) -> Vec<crate::generate::LeagueRoom> {
    let mut out = Vec::new();
    if rolled.stone_circles() > 0 {
        out.extend(extra_feature(gd, "StoneCircle"));
    }
    out
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
