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
//! | 9 | `+0x64` (column 25) | none |
//! | 0xb | `+0x68` (column 26) | none |
//!
//! `0x141e22dd0` rolls one: a chance above 100 adds `chance / 100` and rolls the remainder; a
//! chance up to 100 rolls once (`below(100) < chance`); then, while the second chance is set and
//! the count is under the mechanic's maximum, it rolls the second chance and keeps going on a hit.
//! Only areas with `IsMapArea` roll (the campaign's traces show no such draws); the mechanic table
//! rows' own conditions (stats, maximum, flags) are assumed to pass, as with no area stats.

use poe_data::{GameData, Value};

use crate::rng::TerrainRng;

/// The mechanics rolled, in order: (index, column of the chance, column of the second chance).
const ROLLS: [(u8, usize, Option<usize>); 4] = [(0, 8, Some(9)), (2, 15, None), (9, 25, None), (0xb, 26, None)];

/// One roll (`0x141e22dd0`); the maximum count is not known per mechanic and taken as unbounded.
fn roll(rng: &mut TerrainRng, chance: i64, second: i64) -> u32 {
    let mut count = 0u32;
    if chance > 0 {
        if chance <= 100 {
            if (rng.below(100) as i64) < chance {
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
        while (rng.below(100) as i64) < second {
            count += 1;
            if count > 100 {
                break;
            }
        }
    }
    count
}

/// Draws the league rolls of `area_id` from `rng`; nothing for areas that do not roll.
pub fn roll_area(gd: &GameData, area_id: &str, rng: &mut TerrainRng) {
    let Ok(areas) = gd.table("WorldAreas") else { return };
    let Ok(Some(area)) = areas.find_by_str("Id", area_id) else { return };
    if !area.get("IsMapArea").ok().and_then(|v| v.as_bool()).unwrap_or(false) {
        return;
    }
    let Some(chances) = area.get("LeagueChances").ok().and_then(|v| v.as_row()) else { return };
    let Ok(table) = gd.table("WorldAreaLeagueChances") else { return };
    let Some(row) = table.row(chances) else { return };
    let col = |c: usize| match row.get_index(c) {
        Ok(Value::Int(v)) => v,
        _ => 0,
    };
    for (_, chance, second) in ROLLS {
        roll(rng, col(chance), second.map_or(0, col));
    }
}
