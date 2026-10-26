//! A whole area's terrain layout from its `WorldAreas` id and seed: the client's generator
//! passes in order (topology, graph layout, rooms, edges, edge tiles, corner lattice, fill,
//! de-repetition, outer buffer). The tile hash of the result is what the client reports.

use std::collections::HashMap;

use poe_data::{GameData, Value};

use crate::{
    node_tiles,
    edge_tiles::{EdgeInfo, EdgeTiler},
    fill::fill_pass,
    generate::{GenError, Master, RoomPlacement, TileMap, derepeat_pass, room_pass_oriented},
    graph::Graph,
    layout::{self, JitterNode},
    lattice,
    outer_buffer::{Margins, outer_buffer},
    rng::{TerrainRng, lcg_index},
    rooms::{PlacedGraph, PlacedRoom, RoomSet, TerrainTypes, place_node, tsi_entries},
    routing::{EXIT_CELL, RouteEdge, Router, SIDE_DIR},
    smoothing::{self, LEdge, LNode, LayoutGraph},
    split::{SplitType, split_virtual},
};

fn data(e: impl std::fmt::Display) -> GenError {
    GenError::Unsupported(e.to_string())
}

/// A point in the pipeline [`generate_probed`] reports.
pub enum Stage<'a> {
    /// The layout graph after smoothing (`"smooth"`), the virtual split (`"split"`), before the
    /// edge tiles (`"tiles"`).
    Graph(&'a str, &'a LayoutGraph),
    /// The tile map after the room passes.
    Rooms(&'a TileMap),
    /// The tile map after the edge tiles, with the number of pieces before them.
    Edges(&'a TileMap, usize),
    /// The corner lattice before the client step it is named after (`borders`, `flood`,
    /// `heights`, `92ce0`, `fill`), as the DLL's `lattice` snapshots are.
    Lattice(&'a str, &'a TileMap),
}

/// A generated area.
pub struct Generated {
    /// The topology's graph file.
    pub graph: String,
    pub map: TileMap,
    /// The doodads of every placed room, in cells of the final map (`map.doodads`, with
    /// `counted` set); the ones that count make the doodad hash ([`Generated::doodad_hash`]).
    pub doodads: Vec<crate::doodads::Doodad>,
    /// RNG draws used by the layout (the client continues with doodads and entities).
    pub draws: u64,
    /// The bounded draws `(n, result)` of the doodad pass, when the RNG records them
    /// ([`TerrainRng::log`]); empty otherwise.
    pub doodad_draws: Vec<(u32, u32)>,
}

impl Generated {
    /// The client's doodad hash of this layout.
    pub fn doodad_hash(&self) -> u32 {
        crate::doodads::doodad_hash(self.doodads.iter().filter(|d| d.counted))
    }
}

/// The topology graphs (`.tgr`/`.dgr`) an area chooses from (`0x141fdfe30`): those whose last
/// Topologies column (league/mode, topology +0x40) equals `mode`, else those marked 0x39 (any);
/// none at all retries with mode 1 (the default).
pub fn topologies(gd: &GameData, area_id: &str) -> Result<Vec<String>, GenError> {
    topologies_for(gd, area_id, 1)
}

pub fn topologies_for(gd: &GameData, area_id: &str, mode: i64) -> Result<Vec<String>, GenError> {
    Ok(topology_entries(gd, area_id, mode)?.into_iter().map(|t| t.0).collect())
}

/// Every `WorldAreas.Topologies` entry of an area in table order whatever its league mode:
/// `(graph path, orientation mask, mode)`. This is the list `map_force_topology` indexes
/// (`0x141ff9280` checks the stat against row `+0x60` and `0x141ff90f0` takes entry `stat - 1`
/// of row `+0x68`, no mode filter); [`topology_entries`] is the mode-filtered list the client
/// draws from when nothing is forced.
pub fn raw_topology_entries(gd: &GameData, area_id: &str) -> Result<Vec<(String, u8, i64)>, GenError> {
    let areas = gd.table("WorldAreas").map_err(data)?;
    let area = areas.find_by_str("Id", area_id).map_err(data)?.ok_or_else(|| data(format!("no area {area_id}")))?;
    let topologies = gd.table("Topologies").map_err(data)?;
    let rows: Vec<usize> = match area.get("Topologies").map_err(data)? {
        Value::List(l) => l.iter().filter_map(Value::as_row).collect(),
        _ => Vec::new(),
    };
    let last = topologies.schema().columns.len() - 1;
    let mut out = Vec::new();
    for &r in &rows {
        let row = topologies.row(r).ok_or_else(|| data("bad topology row"))?;
        let path = row.str("DGRFile").map_err(data)?;
        let mut mask = 0u8;
        if let Value::List(l) = row.get_index(6).map_err(data)? {
            for v in l {
                if let Value::Int(o) = v {
                    mask |= if o == 8 { 0xff } else { 1 << (o & 7) };
                }
            }
        }
        let mode = match row.get_index(last).map_err(data)? {
            Value::Int(v) => v,
            _ => -1,
        };
        out.push((path, mask, mode));
    }
    Ok(out)
}

/// [`topologies_for`] with each topology's allowed region orientations (bit per orientation,
/// Topologies list column 6 at topology +0x34; 8 = all).
///
/// The mode-1 entries of every area come first in the raw list ([`raw_topology_entries`]), so a
/// 1-based index into this list is the same index into the raw one (`map_force_topology`); the
/// areas that also list other-mode entries (G1_2, G1_6, G1_11, G1_13_1, G2_4_1, G2_5_2, G2_8,
/// G3_2_1, G3_3, G3_4, G3_7: mode 54 after the mode 1 ones) have more raw entries than this
/// returns.
pub fn topology_entries(gd: &GameData, area_id: &str, mode: i64) -> Result<Vec<(String, u8)>, GenError> {
    let (mut exact, mut any) = (Vec::new(), Vec::new());
    for (path, mask, m) in raw_topology_entries(gd, area_id)? {
        match m {
            v if v == mode => exact.push((path, mask)),
            0x39 => any.push((path, mask)),
            _ => {}
        }
    }
    if !exact.is_empty() {
        return Ok(exact);
    }
    if any.is_empty() && mode != 1 {
        return topology_entries(gd, area_id, 1);
    }
    Ok(any)
}

/// Generates area `area_id` with `seed` (`TerrainGen_Run` up to the tile hash).
pub fn generate(gd: &GameData, area_id: &str, seed: u32) -> Result<Generated, GenError> {
    generate_with(gd, area_id, &mut TerrainRng::new(seed), None)
}

/// A layout choice the server makes for the client: `map_force_topology` (1-based index into
/// [`topologies`]) and `map_force_orientation` (stat value − 1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Forced {
    pub topology: Option<u32>,
    pub orientation: Option<u8>,
}

/// [`generate`] with a caller's RNG (for draw logs) and optionally a forced topology
/// (`map_force_topology`, stat key 0xb92: 1-based index into [`topologies`]; `0x141ff90f0`
/// still draws the topology value).
///
/// Without one the client chooses by the world map (`0x141ff9280` → `0x141fa6440`: the
/// directions of the connected areas' map pins against each topology's direction list, three
/// passes from exact to adjacent) and only falls back to the index draw here when nothing
/// matches; that choice is not ported yet.
pub fn generate_with(gd: &GameData, area_id: &str, rng: &mut TerrainRng, forced: Option<Forced>) -> Result<Generated, GenError> {
    generate_probed(gd, area_id, rng, forced, &mut |_| {})
}

/// [`generate_with`] reading the game files through `files` instead of `gd.files()`: a
/// [`crate::generate::CachedFiles`] makes repeated and parallel generation (verification tools)
/// cheap.
pub fn generate_with_files(gd: &GameData, files: &dyn crate::generate::Files, area_id: &str, rng: &mut TerrainRng, forced: Option<Forced>) -> Result<Generated, GenError> {
    generate_probed_files(gd, files, area_id, rng, forced, &mut |_| {})
}

/// [`generate_with`] reporting intermediate [`Stage`]s to `probe`, for comparing with the
/// client's trace.
pub fn generate_probed(gd: &GameData, area_id: &str, rng: &mut TerrainRng, forced: Option<Forced>, probe: &mut dyn FnMut(Stage)) -> Result<Generated, GenError> {
    generate_probed_files(gd, gd.files(), area_id, rng, forced, probe)
}

/// The files plus the `SpecialTiles` table (`OverlayBlank` is row 2, `0x141d08fe0`).
struct WithSpecialTiles<'a> {
    inner: &'a dyn crate::generate::Files,
    special: Vec<String>,
    league: std::cell::RefCell<Vec<crate::generate::LeagueRoom>>,
    room_sets: Vec<String>,
}

impl crate::generate::Files for WithSpecialTiles<'_> {
    fn read(&self, path: &str) -> Result<Vec<u8>, GenError> {
        self.inner.read(path)
    }
    fn special_tile(&self, row: usize) -> Option<String> {
        self.special.get(row).cloned()
    }
    fn league_rooms(&self) -> Vec<crate::generate::LeagueRoom> {
        self.league.borrow().clone()
    }
    fn take_league(&self, room: &str) {
        let mut l = self.league.borrow_mut();
        if let Some(i) = l.iter().position(|r| r.room == room) {
            l.remove(i);
        }
    }    fn area_room_sets(&self) -> Vec<String> {
        self.room_sets.clone()
    }
}

thread_local! {
    static ROOM_SETS: std::cell::RefCell<std::collections::HashMap<String, Vec<String>>> = Default::default();
}

/// See [`crate::generate::Files::area_room_sets`]; cached per area.
fn area_room_sets(gd: &GameData, files: &dyn crate::generate::Files, area_id: &str) -> Vec<String> {
    if std::env::var_os("POE_OWN_ROOMSET_ONLY").is_some() {
        return Vec::new();
    }
    if let Some(v) = ROOM_SETS.with(|c| c.borrow().get(area_id).cloned()) {
        return v;
    }
    let mut out: Vec<String> = Vec::new();
    for (path, _, _) in raw_topology_entries(gd, area_id).unwrap_or_default() {
        let Ok(bytes) = files.read(&path) else { continue };
        let Ok(g) = Graph::parse_bytes(&bytes) else { continue };
        let Ok(tsi_bytes) = files.read(&g.master_file) else { continue };
        let tsi = master_entries(&tsi_bytes, &g.master_file);
        let dir = g.master_file.rsplit_once('/').map_or("", |d| d.0);
        if let Some(v) = tsi.get("RoomSet") {
            let p = if v.len() > 9 && v[..9].eq_ignore_ascii_case("metadata/") { v.to_string() } else { format!("{dir}/{v}") };
            if !out.iter().any(|o| o.eq_ignore_ascii_case(&p)) {
                out.push(p);
            }
        }
    }
    ROOM_SETS.with(|c| c.borrow_mut().insert(area_id.to_string(), out.clone()));
    out
}

/// [`generate_probed`] with the file source given.
pub fn generate_probed_files(gd: &GameData, files: &dyn crate::generate::Files, area_id: &str, rng: &mut TerrainRng, forced: Option<Forced>, probe: &mut dyn FnMut(Stage)) -> Result<Generated, GenError> {
    let forced = forced.unwrap_or_default();
    // `TerrainGen_Run` seeds with `disabled` set for hideouts (WorldAreas `IsHideout`, row
    // +0x12a): every draw then returns its lower bound.
    if poe_data::views::WorldArea::find(gd, area_id).ok().flatten().is_some_and(|a| a.is_hideout) {
        rng.disabled = true;
    }
    let graphs = topology_entries(gd, area_id, 1)?;
    if graphs.is_empty() {
        return Err(data(format!("{area_id}: no topologies")));
    }
    // `phase_cad0`: league mechanic rolls come before the topology choice.
    let rolled = crate::league::roll_area(gd, area_id, rng);
    // `0x141ff90f0`: the sub-seed is drawn unless the area's kind (`+0x1b4`, the unnamed i32
    // column 45 of `WorldAreas`) is 0x21, a world-map panel: it then comes from the caller's
    // seed word and the main stream is not touched (unforced choice not ported: 0).
    let sub = if area_kind(gd, area_id) == Some(0x21) { 0 } else { rng.below(u32::MAX) };
    // A forced topology indexes the raw list (all league modes, `0x141ff9280`); the client
    // throws for an index past its end ("Cannot force graph"). Without one it draws from the
    // mode-filtered list.
    let (dgr, mask) = match forced.topology {
        Some(f) if f >= 1 => {
            let raw = raw_topology_entries(gd, area_id)?;
            let e = raw.get(f as usize - 1).ok_or_else(|| data(format!("{area_id}: cannot force topology {f}: only {} topologies", raw.len())))?;
            (e.0.clone(), e.1)
        }
        _ => graphs[lcg_index(sub, graphs.len() as u32) as usize].clone(),
    };
    // Region orientation: forced, else what the world-map match gives (not ported: the lowest
    // allowed one, right for the act 1 areas checked). Without a match the client draws it from
    // the topology's list (`0x141ff90f0`: `region::pick(mask, lcg_index(sub, 0x348))`).
    let orientation = forced.orientation.filter(|&o| o < 8).unwrap_or(if mask == 0 || mask == 0xff { 0 } else { mask.trailing_zeros() as u8 });
    let mut graph = Graph::parse_bytes(&files.read(&dgr)?).map_err(|e| data(format!("{dgr}: {e:?}")))?;
    turn(files, &mut graph, orientation)?;
    // `ShipPlugin` (`0x141f03c40` via `0x141ff9860` / `0x141ff89e0`): the plugin adds the `G4_ship`
    // area as a second graph job: a topology draw among its topologies, then its region
    // orientation draw (`below(0x348)` among the topology's allowed ones). It is generated after
    // the main graph and sits beside the finished map.
    let mut extras: Vec<Graph> = Vec::new();
    if area_plugin(gd, area_id).as_deref() == Some(SHIP_PLUGIN) {
        extras.push(extra_job(gd, files, rng, SHIP_AREA)?);
    }
    // G4_13's expedition sub-area (`0x141efcec0` adds `ExpeditionSubArea_Kalguur_Act4` for the area
    // whose id hashes to 0x1c03): a third graph job drawn like the ship's, appended after it.
    if area_id == KALGUUR_HOST {
        extras.push(extra_job(gd, files, rng, KALGUUR_AREA)?);
    }
    let special: Vec<String> = gd.table("SpecialTiles").ok().map(|t| (0..).map_while(|r| t.row(r)).filter_map(|r| r.str("TDTFile").ok().map(String::from)).collect()).unwrap_or_default();
    let room_sets = area_room_sets(gd, files, area_id);
    let files = WithSpecialTiles { inner: files, special, league: std::cell::RefCell::new(crate::league::extra_rooms(gd, &rolled)), room_sets };
    let map = generate_graph_probed(&files, &graph, rng, probe, &extras)?;
    let draws = rng.draws;
    let logged = rng.log.as_ref().map(Vec::len);
    let doodads = crate::doodads::resolve(gd, &files, rng, map.doodads.clone());
    let doodad_draws = logged.and_then(|n| rng.log.as_ref().map(|l| l[n..].to_vec())).unwrap_or_default();
    Ok(Generated { graph: dgr, map, doodads, draws, doodad_draws })
}

/// The area kind of a `WorldAreas` row (area record `+0x1b4`: 0x20/0x21/0x23 are world-map panels).
fn area_kind(gd: &GameData, area_id: &str) -> Option<i64> {
    let areas = gd.table("WorldAreas").ok()?;
    let area = areas.find_by_str("Id", area_id).ok()??;
    area.get_index(45).ok()?.as_int()
}

/// The `TerrainPlugins` row name of a `WorldAreas` row (`0x141d04820` registers the plugins by
/// name).
fn area_plugin(gd: &GameData, area_id: &str) -> Option<String> {
    let areas = gd.table("WorldAreas").ok()?;
    let area = areas.find_by_str("Id", area_id).ok()??;
    let row = area.get("TerrainPlugins").ok()?.as_row()?;
    gd.table("TerrainPlugins").ok()?.row(row)?.str("Id").ok().map(String::from)
}

/// The plugin whose hook adds a second area to the generation, and that area (the client's
/// hook, `0x141f03c40`, looks `G4_ship` up in `WorldAreas` by this literal).
const SHIP_PLUGIN: &str = "ShipPlugin";
const SHIP_AREA: &str = "G4_ship";
const KALGUUR_HOST: &str = "G4_13";
const KALGUUR_AREA: &str = "ExpeditionSubArea_Kalguur_Act4";

/// A second graph job of another `WorldAreas` row (`0x141ff89e0`): a topology draw among its
/// topologies, then its region orientation draw (`below(0x348)` among the topology's allowed ones).
/// It is generated after the main graph and sits beside the finished map.
fn extra_job(gd: &GameData, files: &dyn crate::generate::Files, rng: &mut TerrainRng, area: &str) -> Result<Graph, GenError> {
    let entries = topology_entries(gd, area, 1)?;
    if entries.is_empty() {
        return Err(data(format!("{area}: no topologies")));
    }
    let (path, mask) = entries[rng.below(entries.len() as u32) as usize].clone();
    let o = crate::region::pick(mask, rng.below(0x348));
    let mut g = Graph::parse_bytes(&files.read(&path)?).map_err(|e| data(format!("{path}: {e:?}")))?;
    turn(files, &mut g, o)?;
    Ok(g)
}

/// A second graph job's map beside the finished first one with a one-tile gap: to its right
/// for a map at least as wide as tall, else below it, from the far edge on the other axis's 0.
fn append_job(main: TileMap, job: &TileMap) -> TileMap {
    let (w, h) = (main.width, main.height);
    let (at, size) = if w >= h { ((w as i32 + 1, 0), (w + 1 + job.width, h.max(job.height))) } else { ((0, h as i32 + 1), (w.max(job.width), h + 1 + job.height)) };
    let mut m = main.embed((0, 0), size.0, size.1);
    m.paste(job, at);
    m
}

/// Turns a graph into region orientation `o` (edge types decide their mirroring).
fn turn(files: &dyn crate::generate::Files, graph: &mut Graph, o: u8) -> Result<(), GenError> {
    let mut types = TerrainTypes::default();
    let mut symmetric = std::collections::HashMap::new();
    for e in &graph.edges {
        let key = e.edge_type.to_ascii_lowercase();
        if !symmetric.contains_key(&key) {
            let v = edge_symmetric(&mut types, files, &key)?;
            symmetric.insert(key, v);
        }
    }
    crate::region::turn_graph(graph, o, &|t| symmetric.get(&t.to_ascii_lowercase()).copied().unwrap_or(false));
    Ok(())
}

/// `0x141fe9970`: existence rolls (nodes, node +0x44 = value 0: 0 removes, ≥100 keeps, else
/// `below(100)`; then every edge with a non-zero chance), the `shuffle` groups and the `unique`
/// groups. Removed nodes leave the graph (later node indices move up, as the client erases
/// them) and take their edges with them; removed edges stay as `true` in the returned flags.
/// `0x141fea2e0` / `0x141fee830` (`.dgr`, before the existence rolls): each node with a random
/// offset (values 5..9: x min, y min, x max, y max) moves by min + `range(0, max - min)` on x,
/// then on y.
fn random_offsets(graph: &mut Graph, rng: &mut TerrainRng) {
    for n in &mut graph.nodes {
        let v = |i: usize| n.values.get(i).copied().unwrap_or(0);
        let (x0, y0, x1, y1) = (v(5), v(6), v(7), v(8));
        if x0 == x1 && y0 == y1 && x0 == 0 && y0 == 0 {
            continue;
        }
        let dx = if x0 < x1 { x0 + rng.range(0, (x1 - x0) as u32) as i32 } else { x0 };
        let dy = if y0 < y1 { y0 + rng.range(0, (y1 - y0) as u32) as i32 } else { y0 };
        n.x += dx;
        n.y += dy;
    }
    // `0x141fea420` / `0x141fee6e0` (right after the offsets, all nodes): a room type holding a
    // comma list (`Gates_N, Gates_FN`) keeps one entry, `below(count)`.
    for n in &mut graph.nodes {
        if n.room_type.contains(',') {
            let entries: Vec<&str> = n.room_type.split(',').map(str::trim).collect();
            if entries.len() > 1 {
                let k = rng.below(entries.len() as u32) as usize;
                n.room_type = entries[k].to_string();
            }
        }
    }
}

thread_local! {
    /// The job's lettered-unique history (`0x141feb050`), by letter; cleared per generated area.
    static LETTER_HISTORY: std::cell::RefCell<std::collections::BTreeMap<u8, Vec<usize>>> = Default::default();
}

fn prune(graph: &Graph, rng: &mut TerrainRng) -> Result<(Graph, Vec<bool>), GenError> {
    let mut drop_node = vec![false; graph.nodes.len()];
    for (i, n) in graph.nodes.iter().enumerate() {
        let chance = n.values.first().copied().unwrap_or(100);
        drop_node[i] = chance == 0 || (chance < 100 && rng.below(100) as i32 >= chance);
    }
    let mut removed: Vec<bool> = graph
        .edges
        .iter()
        .map(|e| {
            let exist = e.values.first().copied().unwrap_or(100);
            !(exist != 0 && (rng.below(100) as i32) < exist)
        })
        .collect();
    let mut g = graph.clone();
    // `0x141d1c580` gives each edge, in file order, the first free slot of both its nodes: a node's
    // slots hold its edges by edge index (the node line lists them in its own order). `.dgr` only: sorting the
    // `.tgr` slots loses 1,172 of the match sample's outdoor jobs.
    if g.dungeon_defaults.is_some() {
        for n in &mut g.nodes {
            n.edges.sort();
        }
    }
    let mut dropped = vec![false; g.nodes.len()];
    for n in 0..g.nodes.len() {
        if drop_node[n] {
            remove_node(&mut g, &mut removed, &mut dropped, n);
        }
    }
    let drop_node_before = dropped.clone();
    shuffle_nodes(&mut g, &drop_node_before, rng);
    // `unique<N>[#count]` / `unique<L><N>[#count]` (`0x141feb3b0`, collected by `0x141fe96e0`).
    let mut groups: Vec<(u32, Vec<usize>)> = Vec::new();
    let mut letters: std::collections::BTreeMap<u8, (u32, Vec<Vec<usize>>)> = Default::default();
    for (i, n) in g.nodes.iter().enumerate() {
        if dropped[i] {
            continue;
        }
        let Some(t) = n.tags.iter().find(|t| t.len() >= 6 && t[..6].eq_ignore_ascii_case("unique")) else { continue };
        let mut rest = &t[6..];
        let letter = rest.chars().next().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase() as u8);
        if letter.is_some() {
            rest = &rest[1..];
        }
        let mut count = 1u32;
        if let Some((a, b)) = rest.split_once('#') {
            rest = a;
            count = b.trim().parse().unwrap_or(1);
        }
        let k = if rest.is_empty() && letter.is_none() { 0 } else { match rest.trim().parse::<usize>() { Ok(v) => v, Err(_) => continue } };
        if let Some(l) = letter {
            let e = letters.entry(l).or_insert((0, Vec::new()));
            if e.1.len() <= k {
                e.1.resize(k + 1, Vec::new());
            }
            e.0 = e.0.max(count);
            e.1[k].push(i);
            continue;
        }
        if groups.len() <= k {
            groups.resize(k + 1, (0, Vec::new()));
        }
        groups[k].0 = groups[k].0.max(count);
        groups[k].1.push(i);
    }
    for (count, mut list) in groups {
        if (count as usize) < list.len() {
            for _ in 0..count {
                let k = rng.below(list.len() as u32) as usize;
                list.remove(k);
            }
            for n in list {
                remove_node(&mut g, &mut removed, &mut dropped, n);
            }
        }
    }
    // Letter groups (`0x141fe9970`): the numbered subgroups that stay are kept per letter in
    // the job's history of this graph (the vector `0x141feb050` returns), empty on the first
    // generation: a discarded `below(n)`, a partial shuffle of `0..n` for `count` steps
    // (`0x141fecde0`), cut to `count`; the nodes of the other subgroups go.
    for (letter, (count, subgroups)) in letters {
        let n = subgroups.len();
        let mut keep: Vec<usize> = LETTER_HISTORY.with(|h| h.borrow().get(&letter).cloned().unwrap_or_default());
        if keep.is_empty() && count != 0 {
            keep = (0..n).collect();
            rng.below(n as u32);
            for i in 0..(count as usize).min(n) {
                let k = rng.uniform((n - i) as u32) as usize;
                keep.swap(i, i + k);
            }
            keep.truncate(count as usize);
            LETTER_HISTORY.with(|h| h.borrow_mut().insert(letter, keep.clone()));
        }
        for (k, list) in subgroups.into_iter().enumerate() {
            if !keep.contains(&k) {
                for node in list {
                    remove_node(&mut g, &mut removed, &mut dropped, node);
                }
            }
        }
    }
    let drop_node = dropped;
    // Erase the dropped nodes.
    let mut new_index = vec![usize::MAX; g.nodes.len()];
    let mut k = 0;
    for i in 0..g.nodes.len() {
        if !drop_node[i] {
            new_index[i] = k;
            k += 1;
        }
    }
    for (e, edge) in g.edges.iter_mut().enumerate() {
        let (f, t) = (new_index[edge.from as usize], new_index[edge.to as usize]);
        if f == usize::MAX || t == usize::MAX {
            removed[e] = true;
        }
        edge.from = f.min(u32::MAX as usize) as u32;
        edge.to = t.min(u32::MAX as usize) as u32;
    }
    let mut i = 0;
    g.nodes.retain(|_| {
        i += 1;
        !drop_node[i - 1]
    });
    Ok((g, removed))
}

/// A node's live edge slots (`.tgr` order, first four, removed edges empty).
fn live_slots(g: &Graph, removed: &[bool], n: usize) -> [Option<usize>; 4] {
    let mut s = [None; 4];
    for (k, &e) in g.nodes[n].edges.iter().take(4).enumerate() {
        if !removed[e as usize] {
            s[k] = Some(e as usize);
        }
    }
    s
}

/// `0x141feb9d0`: removes a node. Two edges of one type with the far nodes at the node's
/// height merge into one through it (`0x141fec1c0`: the first edge takes the second's far
/// node, the node's position and the second edge's points); a room node with two or more
/// edges only loses its room; anything else goes with its edges (`0x141fec520`).
fn remove_node(g: &mut Graph, removed: &mut [bool], dropped: &mut Vec<bool>, n: usize) {
    let slots: Vec<usize> = live_slots(g, removed, n).into_iter().flatten().collect();
    let height = |g: &Graph, i: usize| g.nodes[i].values.get(1).copied().unwrap_or(0);
    let other = |g: &Graph, e: usize| if g.edges[e].from as usize == n { g.edges[e].to as usize } else { g.edges[e].from as usize };
    if slots.len() == 2 {
        let (e1, e2) = (slots[0], slots[1]);
        let (o1, o2) = (other(g, e1), other(g, e2));
        if g.edges[e1].edge_type.eq_ignore_ascii_case(&g.edges[e2].edge_type) && height(g, o1) == height(g, n) && height(g, o2) == height(g, n) {
            let e1_from = g.edges[e1].from as usize == n;
            let e2_from = g.edges[e2].from as usize == n;
            // e2's slot at its far node: cleared, then e1 takes that node's first free slot.
            removed[e2] = true;
            let free = live_slots(g, removed, o2).iter().position(|s| s.is_none());
            if let Some(k) = free {
                if k < g.nodes[o2].edges.len() {
                    g.nodes[o2].edges[k] = e1 as u32;
                } else {
                    g.nodes[o2].edges.push(e1 as u32);
                }
            }
            if e1_from { g.edges[e1].from = o2 as u32 } else { g.edges[e1].to = o2 as u32 }
            // Points: the node, then e2's (running away from the node).
            let pos = (g.nodes[n].x, g.nodes[n].y);
            let mut far: Vec<(i32, i32)> = g.edges[e2].points.clone();
            if !e2_from {
                far.reverse();
            }
            // `far` now runs node → o2.
            let pts = &mut g.edges[e1].points;
            if e1_from {
                // e1 now starts at o2: o2 … node, then e1's own points.
                let mut v: Vec<(i32, i32)> = far.into_iter().rev().collect();
                v.push(pos);
                v.extend(pts.iter().copied());
                *pts = v;
            } else {
                pts.push(pos);
                pts.extend(far);
            }
            dropped[n] = true;
            return;
        }
    }
    if slots.len() >= 2 && !g.nodes[n].room_type.is_empty() {
        g.nodes[n].room_type.clear();
        g.nodes[n].tags.clear();
        g.nodes[n].orientation = "(any)".into();
        return;
    }
    // `0x141fec520`: an edge with internal points ends at a new node on its nearest point
    // instead of going.
    for e in slots {
        if g.edges[e].points.is_empty() {
            removed[e] = true;
            continue;
        }
        let from = g.edges[e].from as usize == n;
        let p = if from { g.edges[e].points.remove(0) } else { g.edges[e].points.pop().unwrap() };
        let nn = g.nodes.len();
        g.nodes.push(crate::graph::Node {
            x: p.0,
            y: p.1,
            edges: vec![e as u32],
            room_type: String::new(),
            orientation: "(any)".into(),
            tags: Vec::new(),
            values: vec![100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            extra: Vec::new(),
        });
        if from { g.edges[e].from = nn as u32 } else { g.edges[e].to = nn as u32 }
    }
    dropped.resize(g.nodes.len(), false);
    dropped[n] = true;
}

/// The `shuffle` / `shuffle<N>` groups (by N, in node order): while a group holds two or more
/// nodes, two draws pick nodes that leave it, and two different ones swap room type and tags
/// (positions and orientations stay).
fn shuffle_nodes(g: &mut Graph, dropped: &[bool], rng: &mut TerrainRng) {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, n) in g.nodes.iter().enumerate() {
        if dropped[i] {
            continue;
        }
        let Some(t) = n.tags.iter().find(|t| t.starts_with("shuffle")) else { continue };
        let rest = &t["shuffle".len()..];
        let k = if rest.is_empty() {
            0
        } else {
            match rest.trim().parse::<i32>() {
                Ok(v) if v >= 0 => v as usize,
                _ => continue,
            }
        };
        if groups.len() <= k {
            groups.resize(k + 1, Vec::new());
        }
        groups[k].push(i);
    }
    for mut list in groups {
        while list.len() > 1 {
            let n = list.len() as u32;
            let a = list[rng.below(n) as usize];
            let b = list[rng.below(n) as usize];
            list.retain(|&x| x != a);
            list.retain(|&x| x != b);
            if a != b {
                let (ra, ta) = (g.nodes[a].room_type.clone(), g.nodes[a].tags.clone());
                g.nodes[a].room_type = g.nodes[b].room_type.clone();
                g.nodes[a].tags = g.nodes[b].tags.clone();
                g.nodes[b].room_type = ra;
                g.nodes[b].tags = ta;
            }
        }
    }
}

/// `0x141d14d10`: an edge type that a mirrored region leaves alone (no height change, the same
/// ground either side, and for a virtual edge a symmetric split).
fn edge_symmetric(types: &mut TerrainTypes, files: &dyn crate::generate::Files, et: &str) -> Result<bool, GenError> {
    let t = types.edge(files, et)?.clone();
    if t.height_change != 0 || t.grounds[0] != t.grounds[1] {
        return Ok(false);
    }
    if !t.virtual_edge {
        return Ok(true);
    }
    if t.splits[0].0 != t.splits[1].0 || t.splits[0].1 != t.splits[1].1 {
        return Ok(false);
    }
    edge_symmetric(types, files, &t.splits[0].0)
}

/// The master's keys, with `OuterGroundType` made a full path (like the other file keys it is
/// relative to the `.tsi` unless it starts with `Metadata/`: `Tiles/water.gt` of MapUniqueLake).
fn master_entries(bytes: &[u8], tsi_path: &str) -> std::collections::HashMap<String, String> {
    let mut tsi = tsi_entries(bytes);
    if let Some(v) = tsi.get_mut("OuterGroundType") {
        if !(v.len() > 9 && v[..9].eq_ignore_ascii_case("metadata/")) {
            *v = format!("{}/{v}", tsi_path.rsplit_once('/').map_or("", |d| d.0));
        }
    }
    tsi
}

/// A `.dgr` graph: the dungeon layout (`crate::dungeon`), then its tile pass
/// (`crate::dungeon_tiles`).
fn generate_dungeon(files: &dyn crate::generate::Files, graph: &Graph, removed: &[bool], rng: &mut TerrainRng, extra: &[Graph], probe: &mut dyn FnMut(Stage)) -> Result<TileMap, GenError> {
    let mut built = build_dungeon(files, graph, removed, rng, None)?;
    // `0x141ff7e00` / `0x141ff05f0`: each `graph` node's subgraph is generated after the main
    // graph's fill and de-repetition, straight into its reserved rectangle of the map.
    let mut done: Vec<(Built, (i32, i32), Margins)> = Vec::new();
    // A subgraph's own `graph` nodes (BossRush_Area1's `_TheForsakenSon.dgr`) are generated right
    // after it, depth first, into the same map (their origins are relative to the subgraph's grid).
    let mut work: Vec<(String, (i32, i32))> = std::mem::take(&mut built.subgraphs);
    work.reverse();
    while let Some((file, origin)) = work.pop() {
        let mut sub = Graph::parse_bytes(&files.read(&file)?).map_err(|e| data(format!("{file}: {e:?}")))?;
        // Turned like the main graph (`0x141ff05f0` sets the parent's region).
        turn(files, &mut sub, graph.region)?;
        let is_dgr = sub.dungeon_defaults.is_some();
        if is_dgr {
            random_offsets(&mut sub, rng);
        }
        let (sub, removed) = prune(&sub, rng)?;
        // The region sits inside the subgraph master's `OuterBufferSize` margins.
        let m = Margins::parse(&crate::text::decode_utf16(&files.read(&sub.master_file)?));
        let at = (origin.0 + m.left as i32, origin.1 + m.top as i32);
        if std::env::var_os("POE_DBG_WIN").is_some() {
            for y in at.1 - 1..at.1 + 14 {
                let row: String = (at.0 - 1..at.0 + 14).map(|x| match built.map.index(x, y) { Some(i) if built.map.cells[i].is_some() => '#', Some(_) => '.', None => ' ' }).collect();
                eprintln!("WIN {file} at {at:?} {y}: {row}");
            }
        }
        let inner = if is_dgr {
            build_dungeon(files, &sub, &removed, rng, Some((&built.map, at)))?
        } else {
            build_tgr(files, &sub, removed, rng, probe, Some((&built.map, at)))?
        };
        for (f, o) in inner.subgraphs.iter().rev() {
            work.push((f.clone(), (o.0 + at.0, o.1 + at.1)));
        }
        if std::env::var("POE_DBG").is_ok() {
            eprintln!("DBG subgraph {file} origin {origin:?} margins {m:?} inner {}x{}", inner.map.width, inner.map.height);
        }
        built.map.paste(&inner.map, at);
        done.push((inner, origin, m));
    }
    // The plugin's second job (`ShipPlugin`) runs its pipeline after the main graph's and its
    // subgraphs', before any outer pass; its outer passes come first (last graph first).
    let mut job_built: Vec<Built> = Vec::new();
    for g in extra {
        let mut g = g.clone();
        random_offsets(&mut g, rng);
        let (g, rem) = prune(&g, rng)?;
        job_built.push(build_dungeon(files, &g, &rem, rng, None)?);
    }
    let mut job_maps: Vec<TileMap> = Vec::new();
    for b in job_built.into_iter().rev() {
        job_maps.push(finish_dungeon(files, b, &[], rng)?);
    }
    job_maps.reverse();
    // `0x141ff7a10`: the outer fill and outer buffer per graph, the last subgraph first, then the
    // main graph's below.
    // `StoreyHeight` (tile records keep heights in world units): each cell takes the one of the
    // graph that placed it (a subgraph's own cells and the outer fill and buffer cells it added,
    // else the main graph's), so a cell is scaled exactly once.
    let storey_of = |tsi: &std::collections::HashMap<String, String>| -> i16 { tsi.get("StoreyHeight").and_then(|v| v.trim().parse().ok()).unwrap_or(16) };
    let mut owner: Vec<Option<i16>> = vec![None; built.map.cells.len()];
    for (inner, origin, m) in &done {
        let st = storey_of(&inner.tsi);
        let at = (origin.0 + m.left as i32, origin.1 + m.top as i32);
        for y in 0..inner.map.height as i32 {
            for x in 0..inner.map.width as i32 {
                if inner.map.index(x, y).is_some_and(|i| inner.map.cells[i].is_some()) {
                    if let Some(j) = built.map.index(x + at.0, y + at.1) {
                        owner[j] = Some(st);
                    }
                }
            }
        }
    }
    for (mut inner, origin, m) in done.into_iter().rev() {
        if std::env::var("POE_DBG").is_ok() {
            eprintln!("DBG subgraph outer pass origin {:?} size {}x{} pieces {}", origin, inner.map.width, inner.map.height, inner.map.cells.iter().filter(|c| c.is_some()).count());
        }
        let g = inner.tsi.get("OuterGroundType").cloned();
        let (w, h) = (inner.map.width, inner.map.height);
        let st = storey_of(&inner.tsi);
        let before: Vec<bool> = built.map.cells.iter().map(Option::is_some).collect();
        if m.top + m.right + m.bottom + m.left == 0 {
            if let Some(g) = g {
                let rect = (origin.0, origin.1, origin.0 + w as i32, origin.1 + h as i32);
                crate::outer_buffer::outer_fill(&mut built.map, rect, &g, true, &mut inner.master, files)?;
            }
        } else {
            let mut window = built.map.window(origin, w + m.left + m.right, h + m.top + m.bottom);
            let rect = (m.left as i32, m.top as i32, (m.left + w) as i32, (m.top + h) as i32);
            let edges = inner.master.edge_type_table(files)?;
            crate::outer_buffer::outer_buffer_window(&mut window, rect, g.as_deref(), true, &mut inner.master, files, &edges, rng).map_err(|e| match e {
                GenError::Unsupported(s) => GenError::Unsupported(format!("{s} (outer buffer of the subgraph window at {origin:?}, {}x{}, margins {m:?})", w + m.left + m.right, h + m.top + m.bottom)),
                e => e,
            })?;
            built.map.paste(&window, origin);
        }
        for (i, was) in before.iter().enumerate() {
            if !was && built.map.cells[i].is_some() {
                owner[i] = Some(st);
            }
        }
    }
    let main = finish_dungeon(files, built, &owner, rng)?;
    Ok(job_maps.iter().fold(main, |m, job| append_job(m, job)))
}

/// The tail of a dungeon graph: the outer buffer of its own master, then tile heights in world
/// units (`owner`: the storey of the subgraph that placed a cell, by cell of the unbuffered map).
fn finish_dungeon(files: &dyn crate::generate::Files, built: Built, owner: &[Option<i16>], rng: &mut TerrainRng) -> Result<TileMap, GenError> {
    let storey_of = |tsi: &std::collections::HashMap<String, String>| -> i16 { tsi.get("StoreyHeight").and_then(|v| v.trim().parse().ok()).unwrap_or(16) };
    let Built { map, mut master, tsi_bytes, tsi, .. } = built;
    let margins = Margins::parse(&crate::text::decode_utf16(&tsi_bytes));
    let edge_types = master.edge_type_table(files)?;
    // A container graph has no fill tiles to fill with (its subgraphs fill their own rectangles).
    let outer_ground = tsi.get("OuterGroundType").map(String::as_str).filter(|_| !master.fills.is_empty());
    let mut map = outer_buffer(&map, margins, outer_ground, true, &mut master, files, &edge_types, rng)?;
    // Tile records keep heights in world units (`StoreyHeight` per storey).
    let storey: i16 = storey_of(&tsi);
    let (dx, dy) = (margins.left as i32, margins.top as i32);
    let src_w = map.width as i32 - (margins.left + margins.right) as i32;
    for y in 0..map.height as i32 {
        for x in 0..map.width as i32 {
            let (sx, sy) = (x - dx, y - dy);
            let st = if sx >= 0 && sy >= 0 && sx < src_w { owner.get((sy * src_w + sx) as usize).copied().flatten() } else { None };
            if let Some(i) = map.index(x, y) {
                if let Some(c) = map.cells[i].as_mut() {
                    c.height *= st.unwrap_or(storey);
                }
            }
        }
    }
    Ok(map)
}

struct Built {
    map: TileMap,
    master: Master,
    tsi_bytes: Vec<u8>,
    tsi: std::collections::HashMap<String, String>,
    /// Reserved subgraph rectangles: graph file and tile origin.
    subgraphs: Vec<(String, (i32, i32))>,
}

/// One dungeon graph up to its de-repetition: layout, tile pass, fill. `window` is the map and
/// origin a subgraph is generated into (its corners start as the map's).
fn build_dungeon(files: &dyn crate::generate::Files, graph: &Graph, removed: &[bool], rng: &mut TerrainRng, window: Option<(&TileMap, (i32, i32))>) -> Result<Built, GenError> {
    let tsi_path = graph.master_file.clone();
    let tsi_bytes = files.read(&tsi_path)?;
    let tsi = master_entries(&tsi_bytes, &tsi_path);
    let dir = tsi_path.rsplit_once('/').map_or("", |d| d.0);
    let rel = |k: &str| tsi.get(k).map(|v| if v.len() > 9 && v[..9].eq_ignore_ascii_case("metadata/") { v.to_string() } else { format!("{dir}/{v}") });
    // Container graphs (`master_container.tsi` of MapKarst, MapDerelictMansion, P1_6,
    // HideoutVampireManor: nodes that only hold subgraphs) name no `RoomSet`.
    let set = match rel("RoomSet") {
        Some(p) => RoomSet::load(files, &p)?,
        None => RoomSet { rooms: Vec::new(), path: String::new() },
    };
    // `RoomOverlap` is master +0x14b; the parser's default is 1 (constructor `0x14292e190`).
    let overlap: u32 = tsi.get("RoomOverlap").and_then(|v| v.trim().parse().ok()).unwrap_or(1);
    // `DungeonFillRoomHeightOverride` is master +0x14d (a byte), present flag +0x159.
    let fill_height: Option<i32> = tsi.get("DungeonFillRoomHeightOverride").and_then(|v| v.trim().parse::<i32>().ok()).map(|v| v as u8 as i32);
    let file_groups = match rel("FileGroups") {
        Some(p) => {
            let fdir = p.rsplit_once('/').map_or("", |d| d.0).to_string();
            crate::dungeon::FileGroups::parse(&crate::text::decode_utf16(&files.read(&p)?), &fdir)
        }
        None => Default::default(),
    };
    let unit: u32 = tsi.get("RoomUnitSize").and_then(|v| v.trim().parse().ok()).unwrap_or(9);
    // The features still on the generator's list when the graph starts (the layout below takes the ones its mission rooms serve).
    let league = files.league_rooms();
    let grid = crate::dungeon::layout(files, graph, removed, &set, overlap, unit, fill_height, &file_groups, rng)?;
    if std::env::var("POE_DBG").is_ok() {
        for (i, r) in grid.rooms.iter().enumerate() {
            eprintln!("DBG room {i} {} o{} origin {:?} size {:?} special {} acc {} node {:?} height {} exits {:?}", r.path, r.orientation, r.origin, r.size, r.special, r.accessible, r.node, r.height, r.exits.iter().map(|e| (e.door.as_deref().map(|d| d.rsplit('/').next().unwrap_or("")), e.height)).collect::<Vec<_>>());
        }
    }
    let mut master = match (rel("TileSet"), rel("FillTiles")) {
        (Some(t), Some(f)) => Master::load(files, &t, &f)?,
        // `G_login`: a tile set and no fill tiles (nothing needs a fill group there).
        (Some(t), None) => Master::load(files, &t, "")?,
        (None, None) => Master::empty(),
        _ => return Err(data("master names FillTiles without a TileSet")),
    };
    // `OuterFillHeightOverride` (master +0x161 set, value +0x153): the outer fill places plain.
    // The league features' tiles join the tile set when the features are added (`0x141f8aed0`).
    for lr in &league {
        for t in &lr.tiles {
            master.add_extra_tile(files, t)?;
        }
    }
    master.outer_fill_height = tsi.get("OuterFillHeightOverride").and_then(|v| v.trim().parse::<i8>().ok()).or_else(|| (tsi.contains_key("OverrideSpawns") && (tsi.contains_key("DefaultKillZone") || std::env::var_os("POE_OVSP_ANY").is_some())).then_some(0));
    master.force_outer_gt = tsi.get("ForceOuterGT").and_then(|v| v.trim().parse::<i32>().ok()).is_some_and(|v| v != 0);
    let ecf = match rel("EdgeCombination") {
        Some(p) => crate::dungeon_tiles::Ecf::parse(&crate::text::decode_utf16(&files.read(&p)?)),
        None => Default::default(),
    };
    let step = unit - grid.overlap;
    let mut map = TileMap::new(grid.w * step + grid.overlap, grid.h * step + grid.overlap);
    map.ground_flags = master.ground_flag_table(files)?;
    map.edge_flags = master.edge_flag_table(files)?;
    let mut inherited = Vec::new();
    if let Some((outer, origin)) = window {
        map.copy_lattice(outer, origin);
        if std::env::var_os("POE_NO_INHERIT").is_none() {
            inherited = map.inherit_cells(outer, origin);
        }
    }
    let mut rooms = std::collections::HashMap::new();
    for r in &grid.rooms {
        if !r.path.is_empty() && !rooms.contains_key(&r.path) {
            rooms.insert(r.path.clone(), crate::generate::load_room(files, &r.path)?);
        }
    }
    let edge_types = master.edge_type_table(files)?;
    let flag = |k: &str| tsi.get(k).is_some_and(|v| v.trim() != "0");
    // Pending league feature rooms are placed by the tile pass in mission slots; it borrows the
    // master, so the placement works on a second one.
    let host = if grid.pending.is_empty() {
        None
    } else {
        let mut m = match (rel("TileSet"), rel("FillTiles")) {
            (Some(t), Some(f)) => Master::load(files, &t, &f)?,
            _ => return Err(data("league feature rooms need a tile set and fill tiles")),
        };
        for lr in &league {
            for t in &lr.tiles {
                m.add_extra_tile(files, t)?;
            }
        }
        Some(crate::dungeon_tiles::Host { files, master: std::cell::RefCell::new(m) })
    };
    let pass = crate::dungeon_tiles::TilePass { master: &master, ecf: &ecf, unit, overlap: grid.overlap, edge_types: &edge_types, overlap_fixes: flag("AllowInteriorOverlapFixes"), doors_disabled: flag("DisableDoors"), grounds: [1, 2, 0].map(|i| (!graph.ground_types[i].is_empty()).then(|| graph.ground_types[i].clone())), host, hosted: Default::default() };
    // The graph's own grounds reach the lattice through the per-room settle; their `.gt` flags
    // (for example `CarverUpperGround`: 0x1f, which makes the fill treat it as none) are not in
    // the tile set's table unless a tile uses them.
    for g in pass.grounds.iter().flatten().filter(|_| std::env::var("POE_NO_GRFLAGS").is_err()) {
        let key = g.to_ascii_lowercase().replace('\\', "/");
        if !map.ground_flags.contains_key(&key) {
            let f = crate::rooms::ground_flags(&crate::text::decode_utf16(&files.read(&key)?));
            map.ground_flags.insert(key, f);
        }
    }
    let (cells, moved) = pass.run(&grid, &rooms, &mut map, rng)?;
    let settle_grounds = pass.grounds.clone();
    if let Ok(path) = std::env::var("POE_DUMP_PIECES") {
        let mut out = String::new();
        for p in &map.pieces {
            let h = map.get(p.x as u32, p.y as u32).map_or(0, |c| c.height);
            out += &format!("{} {} o{} h{}\t{}\n", p.x, p.y, p.orientation, h, p.path);
        }
        std::fs::write(path, out).ok();
    }
    if let Ok(path) = std::env::var("POE_DUMP_LATTICE") {
        let mut out = String::new();
        for y in 0..=map.height as i32 {
            for x in 0..=map.width as i32 {
                if let Some(i) = map.point_index(x, y) {
                    let p = &map.lattice[i];
                    out += &format!("{x} {y} {} {} {}\n", p.ground.as_deref().map(|g| g.rsplit('/').next().unwrap().to_ascii_lowercase()).unwrap_or("-".into()), p.height, p.flags);
                }
            }
        }
        std::fs::write(format!("{path}.{}x{}", map.width, map.height), out).ok();
    }
    // Overlays around the fill (`0x14209c4c0`: the context `0x141fa0ac0`, phase 1, the fill of
    // the listed cells, phase 2). The generator's room list (`+0x58`) is empty for `.dgr`.
    // `0x14209c150`: the pre-unique room, before the overlay context is built.
    let mut cells = cells;
    let (mut pre_rooms, mut pre_wiggle) = (pass.hosted.take(), Vec::new());
    // League mechanics' rooms come first (`0x14209a620`, before the injectors and the pre-unique room).
    if rel("FillTiles").is_some() && std::env::var("POE_NO_DGR_LEAGUE").is_err() {
        let league_rooms = files.league_rooms();
        // `0x14209a620`: a room set with mission rooms (`mission` tag) leaves the features without
        // the priority flag (+0x60; not the stone circle) to the mission-tile slots of the rooms.
        let has_missions = set.rooms.iter().any(|r| r.tag.eq_ignore_ascii_case("mission"));
        if !league_rooms.is_empty() && !has_missions {
            (pre_rooms, pre_wiggle) = crate::fill::place_league_rooms_dgr(&mut map, &mut master, files, &league_rooms, &mut cells, rng)?;
        }
    }
    if rel("Overlays").is_some() && std::env::var("POE_NO_PREUNIQUE").is_err() {
        if let Some(r) = set.rooms.iter().find(|r| r.tag.eq_ignore_ascii_case("preunique")) {
            let (r2, w2) = crate::fill::place_pre_unique(&mut map, &mut master, files, &r.path, r.orientations, &mut cells, rng)?;
            pre_rooms.extend(r2);
            pre_wiggle.extend(w2);
        }
    }
    cells.sort_by_key(|c| c.pos);
    let mut types = TerrainTypes::default();
    let mut overlays = match rel("Overlays").filter(|_| std::env::var("POE_NO_DOV").is_err()) {
        Some(toy) => {
            // The generator's room list (`+0x58`): a feature room per placed room whose `.arm`
            // has a tag (a `special` room over a generic one does not count, but a mission room
            // does: MapCreek's overlays skip the cells round `mission_02`); its inner rectangle (shrunk by the shared border when the room's own
            // overlap equals the master's) and the whole one are marked, nodes with tags too.
            let rects: Vec<crate::overlay::RoomRect> = grid
                .rooms
                .iter()
                .filter_map(|r| {
                    let room = rooms.get(&r.path)?;
                    let (x0, y0) = (r.origin.0 * step as i32, r.origin.1 * step as i32);
                    let rect = (x0, y0, x0 + (r.size.0 * step + grid.overlap) as i32, y0 + (r.size.1 * step + grid.overlap) as i32);
                    let inset = if room.arm.dimensions.uint1.unwrap_or(1) == grid.overlap { grid.overlap as i32 } else { 0 };
                    if std::env::var_os("POE_DBG_LEAGUE").is_some() && (r.special || !room.arm.tag.is_empty()) {
                        eprintln!("ROOMRECT {:?} {} special {} tag {:?} node {:?}", rect, r.path, r.special, room.arm.tag, r.node);
                    }
                    Some(crate::overlay::RoomRect { rect, feature: !room.arm.tag.is_empty() && (!r.special || room.arm.tag.eq_ignore_ascii_case("mission")), described: r.node.is_some_and(|n| !graph.nodes[n].tags.is_empty()), inset })
                })
                .collect();
            let mut o = crate::overlay::Overlays::new(files, &toy, &map, &mut master, &rects, files.special_tile(2))?;
            o.dungeon = true;
            Some(o)
        }
        None => None,
    };
    dbg_cell(&map, "pre-overlay1");
    if let Ok(path) = std::env::var("POE_DUMP_LAT") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::File::create(path) {
            for y in 0..=map.height as i32 {
                for x in 0..=map.width as i32 {
                    if let Some(i) = map.point_index(x, y) {
                        let p = &map.lattice[i];
                        let _ = writeln!(f, "{x}\t{y}\t{}\t{}\t{}", p.ground.as_deref().unwrap_or("-"), p.height, p.flags);
                    }
                }
            }
        }
    }
    if let Some(o) = &mut overlays {
        o.run(1, &mut map, &mut master, &mut types, files, rng)?;
    }
    // The rooms' lattice settle (`0x142099e90`) gives a ground again to the corners the overlay
    // rooms cleared and left without one (MapFrozenFalls: a bend overlay's far corners keep the
    // graph's `upper_ground.gt`, which the fill reads); verified on the client's fill groups.
    if std::env::var_os("POE_NO_RESETTLE").is_none() {
        crate::dungeon_tiles::settle_rooms(&settle_grounds, unit, grid.overlap, &grid, &rooms, &mut map);
    }
    dbg_cell(&map, "post-overlay1");
    if let Ok(path) = std::env::var("POE_DUMP_LAT2") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::File::create(path) {
            for y in 0..=map.height as i32 {
                for x in 0..=map.width as i32 {
                    if let Some(i) = map.point_index(x, y) {
                        let p = &map.lattice[i];
                        let _ = writeln!(f, "{x}\t{y}\t{}\t{}\t{}", p.ground.as_deref().unwrap_or("-"), p.height, p.flags);
                    }
                }
            }
        }
    }
    let (mut fill_rooms, wiggle) = crate::fill::fill_cells(&mut map, &mut master, files, &cells, rng)?;
    fill_rooms.splice(0..0, pre_rooms);
    let wiggle = [pre_wiggle, wiggle].concat();
    dbg_cell(&map, "post-fill");
    if let Some(o) = &mut overlays {
        o.run(2, &mut map, &mut master, &mut types, files, rng)?;
    }
    dbg_cell(&map, "post-overlay2");
    let wiggle = [moved, overlays.as_ref().map(|o| o.wiggle.clone()).unwrap_or_default(), wiggle].concat();
    crate::dungeon_tiles::wiggle(&mut master, files, &mut map, wiggle, rng)?;
    dbg_cell(&map, "post-wiggle");
    // De-repetition over the rooms and fill rooms, then the outer buffer.
    let mut placements: Vec<RoomPlacement> = grid
        .rooms
        .iter()
        .filter_map(|r| Some(RoomPlacement { room: rooms.get(&r.path)?, origin: (r.origin.0 * step as i32, r.origin.1 * step as i32), orientation: r.orientation }))
        .collect();
    placements.extend(fill_rooms.iter().map(|r| RoomPlacement { room: &r.room, origin: r.origin, orientation: r.orientation }));
    if let Some(o) = &overlays {
        placements.extend(o.placed.iter().map(|&(i, origin, orientation)| RoomPlacement { room: o.room(i), origin, orientation }));
        placements.extend(o.fill_rooms.iter().map(|r| RoomPlacement { room: &r.room, origin: r.origin, orientation: r.orientation }));
    }
    derepeat_pass(&mut map, &master, &placements, rng);
    dbg_cell(&map, "post-derepeat");
    drop(placements);
    // `0x141ff2090` (post-unique tiles, per region after the de-repetition: the village graphs of
    // G1_13_2 place `Fields_01`-style tiles of the tile set's own list here).
    crate::postunique::run(&mut map, &mut master, files, rng)?;
    let subgraphs = grid.rooms.iter().filter_map(|r| Some((r.subgraph.clone()?, (r.origin.0 * step as i32, r.origin.1 * step as i32)))).collect();
    map.release(&inherited);
    Ok(Built { map, master, tsi_bytes, tsi, subgraphs })
}

/// Everything after the topology choice, for one graph.
pub fn generate_graph(files: &dyn crate::generate::Files, graph: &Graph, rng: &mut TerrainRng) -> Result<TileMap, GenError> {
    generate_graph_probed(files, graph, rng, &mut |_| {}, &[])
}

fn dbg_cell(map: &TileMap, stage: &str) {
    if let Ok(v) = std::env::var("POE_DBG_CELL") {
        let xy: Vec<i32> = v.split(',').filter_map(|t| t.parse().ok()).collect();
        if xy.len() == 2 {
            let c = map.index(xy[0], xy[1]).and_then(|i| map.cells[i].as_ref());
            eprintln!("CELL {stage}: {:?}", c.map(|c| (c.path.rsplit('/').next().unwrap_or("").to_string(), c.orientation, c.height)));
        }
    }
}

fn generate_graph_probed(files: &dyn crate::generate::Files, graph: &Graph, rng: &mut TerrainRng, probe: &mut dyn FnMut(Stage), extra: &[Graph]) -> Result<TileMap, GenError> {
    let mut moved;
    let graph = if graph.dungeon_defaults.is_some() {
        moved = graph.clone();
        random_offsets(&mut moved, rng);
        &moved
    } else {
        graph
    };
    LETTER_HISTORY.with(|h| h.borrow_mut().clear());
    let (pruned, removed) = prune(graph, rng)?;
    let graph = &pruned;
    if graph.dungeon_defaults.is_some() {
        return generate_dungeon(files, graph, &removed, rng, extra, probe);
    }
    let Built { map, mut master, tsi_bytes, tsi, .. } = build_tgr(files, graph, removed, rng, probe, None)?;
    let margins = Margins::parse(&crate::text::decode_utf16(&tsi_bytes));
    // Outer buffer.
    let edge_types = master.edge_type_table(files)?;
    let mut map = outer_buffer(&map, margins, tsi.get("OuterGroundType").map(String::as_str), false, &mut master, files, &edge_types, rng)?;
    if std::env::var_os("POE_GRID2").is_some() {
        lattice::mark_unwalkable(&mut map, &master.tiles);
        lattice::mark_enclosed_fills(&mut map, files).map_err(data)?;
    }
    Ok(map)
}

/// A `.tgr` graph up to its outer buffer: the whole outdoor pipeline in `graph`'s own frame.
/// `window` is the map and origin a subgraph is generated into (its corners start as the map's).
#[allow(clippy::too_many_lines)]
fn build_tgr(files: &dyn crate::generate::Files, graph: &Graph, removed: Vec<bool>, rng: &mut TerrainRng, probe: &mut dyn FnMut(Stage), window: Option<(&TileMap, (i32, i32))>) -> Result<Built, GenError> {
    let mut nodes: Vec<JitterNode> = graph
        .nodes
        .iter()
        .map(|n| JitterNode {
            pos: (n.x, n.y),
            fix_x: n.values.get(9).copied().unwrap_or(0) != 0,
            fix_y: n.values.get(10).copied().unwrap_or(0) != 0,
        })
        .collect();
    let mut points: Vec<(i32, i32)> = graph
        .edges
        .iter()
        .zip(&removed)
        .filter(|(_, r)| !**r)
        .flat_map(|(e, _)| e.points.iter().copied())
        .collect();
    let tiles = (graph.width as i32, graph.height as i32);
    layout::jitter(&mut nodes, &mut points, tiles, rng).map_err(|e| data(format!("{e:?}")))?;
    let mut it = points.iter().copied();
    let edge_points: Vec<Vec<(i32, i32)>> = graph
        .edges
        .iter()
        .zip(&removed)
        .map(|(e, r)| if *r { Vec::new() } else { e.points.iter().map(|_| it.next().unwrap()).collect() })
        .collect();

    // Master: tile set, fill tiles, room set, outer buffer.
    let tsi_path = graph.master_file.clone();
    let tsi_bytes = files.read(&tsi_path)?;
    let tsi = master_entries(&tsi_bytes, &tsi_path);
    let dir = tsi_path.rsplit_once('/').map_or("", |d| d.0);
    let rel = |k: &str| tsi.get(k).map(|v| if v.len() > 9 && v[..9].eq_ignore_ascii_case("metadata/") { v.to_string() } else { format!("{dir}/{v}") });
    let mut master = match (rel("TileSet"), rel("FillTiles")) {
        (Some(t), Some(f)) => Master::load(files, &t, &f)?,
        // `G_login`: a tile set and no fill tiles (nothing needs a fill group there).
        (Some(t), None) => Master::load(files, &t, "")?,
        (None, None) => Master::empty(),
        _ => return Err(data("master names FillTiles without a TileSet")),
    };
    // `OuterFillHeightOverride` (master +0x161 set, value +0x153): the outer fill places plain.
    master.outer_fill_height = tsi.get("OuterFillHeightOverride").and_then(|v| v.trim().parse::<i8>().ok()).or_else(|| (tsi.contains_key("OverrideSpawns") && (tsi.contains_key("DefaultKillZone") || std::env::var_os("POE_OVSP_ANY").is_some())).then_some(0));
    master.force_outer_gt = tsi.get("ForceOuterGT").and_then(|v| v.trim().parse::<i32>().ok()).is_some_and(|v| v != 0);
    // Container graphs (`master_container.tsi` of MapKarst, MapDerelictMansion, P1_6,
    // HideoutVampireManor: nodes that only hold subgraphs) name no `RoomSet`.
    let set = match rel("RoomSet") {
        Some(p) => RoomSet::load(files, &p)?,
        None => RoomSet { rooms: Vec::new(), path: String::new() },
    };
    let mut placed_graph = PlacedGraph { graph, nodes: nodes.iter().map(|n| n.pos).collect(), ends: vec![[None; 2]; graph.edges.len()], points: edge_points, removed };
    let mut types = TerrainTypes::default();

    // `0x1420c7400` sorts the room-less nodes before placement: virtual nodes (touching a
    // virtual edge; not dead ends at the border) and junctions of three or more edges get
    // exploded before routing (virtual ones only when no tile fits them at the room stage).
    let (mut virtual_nodes, mut junctions) = (Vec::new(), Vec::new());
    for n in 0..graph.nodes.len() {
        if !graph.nodes[n].room_type.is_empty() {
            continue;
        }
        let edges: Vec<usize> = crate::rooms::EdgeView::slots(&placed_graph, n).into_iter().flatten().collect();
        let mut virtual_end = false;
        for &e in &edges {
            virtual_end |= types.edge(files, &graph.edges[e].edge_type)?.virtual_edge;
        }
        match edges.len() {
            0 => {}
            1 => {
                let b = types.edge(files, &graph.edges[edges[0]].edge_type)?.size as i32;
                let (x, y) = placed_graph.nodes[n];
                let (tx, ty) = ((x / 23).min(tiles.0 - 1), (y / 23).min(tiles.1 - 1));
                let near = !(b <= tx && b <= ty && tx < tiles.0 - b && ty < tiles.1 - b);
                if virtual_end && !near {
                    virtual_nodes.push(n);
                }
            }
            2 if graph.edges[edges[0]].edge_type.eq_ignore_ascii_case(&graph.edges[edges[1]].edge_type) => {}
            k => {
                if virtual_end {
                    virtual_nodes.push(n);
                } else if k > 2 {
                    junctions.push(n);
                }
            }
        }
    }

    // Rooms.
    let mut placed: Vec<PlacedRoom> = Vec::new();
    for n in 0..graph.nodes.len() {
        if graph.nodes[n].room_type.is_empty() {
            continue;
        }
        let p = place_node(&set, &master.tiles, &placed_graph, n, tiles, &placed, &mut types, files, rng)?;
        if std::env::var_os("POE_DEBUG_ROOMS").is_some() {
            let name = p.tile.map_or_else(|| set.rooms[p.room].path.clone(), |t| master.tiles.tiles[t].path.clone());
            eprintln!("room node {n} {name} o{} at {},{} sides {:?}", p.orientation, p.pos.0, p.pos.1, p.sides);
        }
        placed.push(p);
    }
    // `0x1420caa40`, second loop: the nodes touching a virtual edge are tried like rooms (rooms
    // of the empty type, else tiles); those nothing fits are exploded below.
    let mut placed_virtual: Vec<usize> = Vec::new();
    for &n in &virtual_nodes {
        if let Some(p) = crate::rooms::place_node_opt(&set, &master.tiles, &placed_graph, n, tiles, &placed, &mut types, files, rng)? {
            placed.push(p);
            placed_virtual.push(n);
        }
    }
    virtual_nodes.retain(|n| !placed_virtual.contains(n));
    let fixed: Vec<(bool, bool)> = graph
        .nodes
        .iter()
        .map(|n| (n.values.get(9).copied().unwrap_or(0) != 0, n.values.get(10).copied().unwrap_or(0) != 0))
        .collect();
    crate::rooms::repel(&mut placed, &mut placed_graph, &fixed, tiles, &mut types, files)?;
    let mut map = TileMap::new(graph.width, graph.height);
    map.ground_flags = master.ground_flag_table(files)?;
    map.edge_flags = master.edge_flag_table(files)?;
    if let Some((outer, origin)) = window {
        map.copy_lattice(outer, origin);
    }
    for p in placed.iter_mut() {
        // `0x1420c9970` first lets a room that does not fit move (`0x1420c9260`).
        let align = crate::rooms::node_alignment(&placed_graph, p.node, &mut types, files)?;
        let before = p.pos;
        if std::env::var_os("POE_DEBUG_ROOMS").is_some() {
            eprintln!("settle node {} was {:?}", p.node, before);
        }
        if !crate::rooms::settle(&map, p, align, placed_graph.graph.region, &mut types, files)? {
            return Err(data(format!("Overlapping node tiles at {:?}", p.pos)));
        }
        match p.tile {
            // `0x1420c9970`: a node that took a tile places just that tile.
            Some(t) => {
                let def = &master.tiles.tiles[t];
                map.place(p.pos.0, p.pos.1, &def.path, &def.tdt, p.orientation, 0, Some(t));
            }
            None => room_pass_oriented(&mut map, &master, &set.rooms[p.room].room, p.pos, p.orientation, false, 0, rng)?,
        }
    }
    probe(Stage::Rooms(&map));

    // Edge ends at the rooms' exits (`0x1420c5750`, `0x1420c5b50`), then routing.
    let mut route: Vec<RouteEdge> = Vec::new();
    let mut route_index = vec![usize::MAX; graph.edges.len()];
    for (i, e) in graph.edges.iter().enumerate() {
        if placed_graph.removed[i] {
            continue;
        }
        let et = types.edge(files, &e.edge_type)?.clone();
        if et.wildcard {
            continue;
        }
        route_index[i] = route.len();
        route.push(RouteEdge {
            ends: [placed_graph.nodes[e.from as usize], placed_graph.nodes[e.to as usize]],
            offsets: [(0, 0); 2],
            points: placed_graph.points[i].clone(),
            margin: if et.virtual_edge { e.tail.get(1).copied().unwrap_or(0) / 23 + 3 } else { et.size as i32 + 2 },
        });
    }
    let mut vends: Vec<[Option<[(i32, i32); 2]>; 2]> = vec![[None, None]; route.len()];
    for p in &placed {
        let node = p.node;
        let (rx, ry) = (p.pos.0 * 23, p.pos.1 * 23);
        let (rw, rh) = (p.size.0 as i32 * 23, p.size.1 as i32 * 23);
        let inside = |q: &(i32, i32)| q.0 >= rx && q.0 < rx + rw && q.1 >= ry && q.1 < ry + rh;
        for (slot, &ei) in graph.nodes[node].edges.iter().take(4).enumerate() {
            let ei = ei as usize;
            if route_index.get(ei).copied().unwrap_or(usize::MAX) == usize::MAX {
                continue;
            }
            let r = &mut route[route_index[ei]];
            let from = graph.edges[ei].from as usize == node;
            if from {
                let k = r.points.iter().position(|q| !inside(q)).unwrap_or(r.points.len());
                r.points.drain(..k);
            } else if let Some(k) = r.points.iter().position(|q| inside(q)) {
                r.points.truncate(k);
            }
            let side = p.sides[slot];
            if side > 3 {
                continue;
            }
            let at = |e: u8| {
                let (t, c) = (e as i32 / 3, EXIT_CELL[(e % 3) as usize]);
                let o = c + t * 23;
                let (x, y) = match side {
                    0 => (o, 0),
                    1 => (rw - 1, o),
                    2 => (rw - o - 1, rh - 1),
                    _ => (0, rh - o - 1),
                };
                (rx + x, ry + y)
            };
            let et = types.edge(files, &graph.edges[ei].edge_type)?;
            let which = (!from) as usize;
            let end = if et.virtual_edge {
                let mut v1 = at(p.key.virtual_exits[side]);
                let mut v2 = at(p.key.exits[side]);
                let n = placed_graph.nodes[node];
                let q = if from { r.points.first().copied().unwrap_or(r.ends[1]) } else { r.points.last().copied().unwrap_or(r.ends[0]) };
                let mut c = (v1.1 - q.1) * (n.0 - q.0) - (v1.0 - q.0) * (n.1 - q.1);
                let c2 = (n.0 - q.0) * (v2.1 - q.1) - (n.1 - q.1) * (v2.0 - q.0);
                if (c2 < 0) == (c < 0) {
                    c = ((v1.0 + v2.0) / 2 - n.0) * (v1.1 - n.1) - (v1.0 - n.0) * ((v1.1 + v2.1) / 2 - n.1);
                }
                if c < 0 {
                    std::mem::swap(&mut v1, &mut v2);
                }
                let m = ((v1.0 + v2.0) / 2, (v2.1 + v1.1) / 2);
                let d = SIDE_DIR[side];
                if (v1.1 - (d.1 + m.1)) * (m.0 - (d.0 + m.0)) - (v1.0 - (d.0 + m.0)) * (m.1 - (d.1 + m.1)) < 0 {
                    std::mem::swap(&mut v1, &mut v2);
                }
                vends[route_index[ei]][which] = Some([v1, v2]);
                m
            } else {
                at(p.key.exits[side])
            };
            r.ends[which] = end;
            r.offsets[which] = SIDE_DIR[side];
        }
    }
    // The client has cut the rooms' edges and split the room nodes per exit before it
    // explodes anything (`0x1420c9970` before `0x1420cb200`): node keys of junctions see the
    // exits as the other ends.
    for (i, ri) in route_index.iter().enumerate() {
        if *ri != usize::MAX {
            placed_graph.points[i] = route[*ri].points.clone();
            placed_graph.ends[i] = [Some(route[*ri].ends[0]), Some(route[*ri].ends[1])];
        }
    }

    // Exploded nodes (`0x1420cb200`). A virtual node that a tile fits is placed with the rooms
    // instead (`0x1420caa40`), which is not ported yet.
    for &n in virtual_nodes.iter().chain(&junctions) {
        if virtual_nodes.contains(&n) {
        }
        node_tiles::explode(&mut placed_graph, n, tiles, &mut types, files)?;
    }
    for (i, ri) in route_index.iter().enumerate() {
        if *ri != usize::MAX {
            route[*ri].points = placed_graph.points[i].clone();
        }
    }

    let mut router = Router::new(tiles.0, tiles.1, &map, rng);
    for e in &route {
        router.reserve(e);
    }
    // `0x1420cdc30`: edges that fail keep their reservation and are retried after the others
    // (the client gives up when a whole round routes none).
    crate::routing::route_reserved(&mut router, &mut route, rng).map_err(|e| data(format!("could not route edges: {e}")))?;

    // The layout graph in the client's numbering (`0x141ef2800` splits room nodes per exit).
    let mut lg = LayoutGraph::default();
    for i in 0..graph.nodes.len() {
        let mut slots = [None; 4];
        for (k, &ei) in graph.nodes[i].edges.iter().take(4).enumerate() {
            let ei = ei as usize;
            if route_index.get(ei).copied().unwrap_or(usize::MAX) != usize::MAX {
                slots[k] = Some(route_index[ei]);
            }
        }
        let mut virtual_end = false;
        for &ei in &graph.nodes[i].edges {
            virtual_end |= types.edge(files, &graph.edges[ei as usize].edge_type)?.virtual_edge;
        }
        let fixed = placed.iter().any(|p| p.node == i);
        lg.nodes.push(LNode { pos: placed_graph.nodes[i], slots, fixed, virtual_end, orig: Some(i) });
    }
    for (gi, e) in graph.edges.iter().enumerate() {
        let ri = route_index[gi];
        if ri == usize::MAX {
            continue;
        }
        let et = types.edge(files, &e.edge_type)?.clone();
        lg.edges.push(LEdge {
            from: e.from as usize,
            to: e.to as usize,
            points: route[ri].points.clone(),
            offsets: route[ri].offsets,
            edge_type: e.edge_type.to_ascii_lowercase(),
            virtual_edge: et.virtual_edge,
            width: (e.tail.first().copied().unwrap_or(0), e.tail.get(1).copied().unwrap_or(0)),
            chirality: e.values.get(1).copied().unwrap_or(0) != 0,
            vends: vends[ri],
            split: false,
            fixed: false,
        });
    }
    for p in &placed {
        let node = p.node;
        let mut first = true;
        let slots = lg.nodes[node].slots;
        for (k, s) in slots.iter().enumerate() {
            let Some(e) = *s else { continue };
            let which = (lg.edges[e].from != node) as usize;
            let pos = route[e].ends[which];
            if first {
                lg.nodes[node].pos = pos;
                first = false;
            } else {
                let nn = lg.nodes.len();
                lg.nodes.push(LNode { pos, slots: [Some(e), None, None, None], fixed: true, virtual_end: false, orig: None });
                if which == 0 { lg.edges[e].from = nn } else { lg.edges[e].to = nn }
                lg.nodes[node].slots[k] = None;
            }
        }
    }
    smoothing::smooth(&mut lg);
    probe(Stage::Graph("smooth", &lg));
    // Virtual nodes no tile fitted (`0x1420c6cd0`).
    let nodes_before = lg.nodes.len();
    for &n in &virtual_nodes {
        node_tiles::virtual_junction(&mut lg, n, rng)?;
    }
    // The nodes this split made were not in the tile list built at the start (`0x1420c7400` ran
    // on the original nodes).
    let junction_made: std::collections::HashSet<usize> = (nodes_before..lg.nodes.len()).collect();

    // Virtual edges split into their two banks.
    let mut split_types: HashMap<String, SplitType> = HashMap::new();
    let mut want: Vec<String> = lg.edges.iter().map(|e| e.edge_type.clone()).collect();
    while let Some(t) = want.pop() {
        if split_types.contains_key(&t) {
            continue;
        }
        let et = types.edge(files, &t)?.clone();
        want.extend(et.splits.iter().filter(|s| !s.0.is_empty()).map(|s| s.0.clone()));
        split_types.insert(t, SplitType { virtual_edge: et.virtual_edge, grounds: et.grounds.clone(), height_change: et.height_change, splits: et.splits.clone() });
    }
    let lookup = |t: &str| split_types.get(&t.to_ascii_lowercase()).cloned().unwrap_or_default();
    split_virtual(&mut lg, &lookup, (tiles.0 * 23, tiles.1 * 23), rng);
    probe(Stage::Graph("split", &lg));
    // Logical links of tile-less virtual nodes (`0x1420bcff0`).
    let (link_tiles, moved_to) = node_tiles::resolve_links(&mut lg)?;
    // Erased nodes shifted the later indices: the nodes the junction split made keep their flag.
    let junction_made: std::collections::HashSet<usize> = junction_made.iter().filter_map(|&n| moved_to.get(n).copied().flatten()).collect();

    // Rounding, border dead ends and node tiles (`0x1420cade0`), slot compaction, rounding,
    // end points.
    smoothing::round_corners(&mut lg);
    let mut tile_nodes = Vec::new();
    for n in 0..lg.nodes.len() {
        // Sorted at the start of the tile stage (`0x1420c7400`): virtual nodes are not in the
        // tile list. Nodes the splits made (no original) count too unless they are fixed (they
        // copy the flag of the node they were made from).
        match lg.nodes[n].orig {
            Some(gi) => {
                if !graph.nodes[gi].room_type.is_empty() || virtual_nodes.contains(&gi) || placed_virtual.contains(&gi) {
                    continue;
                }
            }
            None => {
                if lg.nodes[n].fixed {
                    continue;
                }
            }
        }
        let edges: Vec<usize> = lg.nodes[n].slots.iter().flatten().copied().collect();
        let mut near = false;
        if let [e] = edges[..] {
            let b = types.edge(files, &lg.edges[e].edge_type)?.size as i32;
            let (x, y) = lg.nodes[n].pos;
            let (tx, ty) = ((x / 23).min(tiles.0 - 1), (y / 23).min(tiles.1 - 1));
            near = !(b <= tx && b <= ty && tx < tiles.0 - b && ty < tiles.1 - b);
        }
        if !node_tiles::needs_tile(&lg, n, near, &mut types, files)? {
            continue;
        }
        if junction_made.contains(&n) && !near {
            continue;
        }
        if !near {
            tile_nodes.push(n);
            continue;
        }
        let e = edges[0];
        let b = types.edge(files, &lg.edges[e].edge_type)?.size as i32;
        let (x, y) = lg.nodes[n].pos;
        let (tx, ty) = ((x / 23).min(tiles.0 - 1), (y / 23).min(tiles.1 - 1));
        let w = (lg.edges[e].from != n) as usize;
        let mut pos = lg.nodes[n].pos;
        let mut off = lg.edges[e].offsets[w];
        if tx < b {
            pos.0 = -1;
            off.0 = 1;
        } else if ty < b {
            pos.1 = -1;
            off.1 = 1;
        }
        if tx < tiles.0 - b {
            if tiles.1 - b <= ty {
                pos.1 = tiles.1 * 23;
                off.1 = -1;
            }
        } else {
            pos.0 = tiles.0 * 23;
            off.0 = -1;
        }
        lg.nodes[n].pos = pos;
        lg.nodes[n].fixed = true;
        lg.edges[e].offsets[w] = off;
    }
    tile_nodes.extend(link_tiles);
    node_tiles::place_node_tiles(&mut lg, graph, &tile_nodes, &master, &mut map, tiles, &mut types, files, rng)?;
    smoothing::compact_slots(&mut lg);
    smoothing::round_corners(&mut lg);
    smoothing::add_end_points(&mut lg);
    probe(Stage::Graph("tiles", &lg));

    // Edge tiles.
    let mut infos = Vec::new();
    for e in &lg.edges {
        let et = types.edge(files, &e.edge_type)?;
        infos.push(EdgeInfo {
            path: e.edge_type.clone(),
            size: et.size as u32,
            grounds: et.grounds.clone(),
            height: et.height_change,
            centred: et.centred,
            hashed: et.hashed,
        });
    }
    let before = map.pieces.len();
    let tiled = EdgeTiler::new(&mut lg, &mut map, &master.tiles, infos).run(rng).map_err(data);
    probe(Stage::Edges(&map, before));
    tiled?;

    // Corner lattice passes, fill, de-repetition.
    for p in &mut map.lattice {
        if p.flags & 1 != 0 { p.flags |= 4 } else { p.flags &= !4 }
    }
    probe(Stage::Lattice("borders", &map));
    lattice::border_floods(&mut map, &master.tiles);
    probe(Stage::Lattice("flood", &map));
    // The unwalkable marking (`0x141fe0fd0`) works from the real map's border: a subgraph's
    // window marks nothing (G3_1's `End_*.tgr`: no corner gets flag 8).
    if window.is_none() {
        lattice::mark_unwalkable(&mut map, &master.tiles);
        lattice::mark_enclosed_fills(&mut map, files).map_err(data)?;
    }
    probe(Stage::Lattice("heights", &map));
    let default_ground = graph.ground_types.iter().find(|g| !g.is_empty()).cloned();
    lattice::settle(&mut map, default_ground.as_deref());
    probe(Stage::Lattice("92ce0", &map));
    for p in &mut map.lattice {
        p.flags &= !4;
    }
    // League mechanics' rooms (`0x1420bd890`, `0x1420be940`), before the overlays.
    let league_rooms = files.league_rooms();
    let pre_rooms = if league_rooms.is_empty() { Vec::new() } else { crate::fill::place_league_rooms(&mut map, &mut master, files, &league_rooms, rng)? };
    // Overlays before and after the fill (`0x141fa1a70`).
    let mut overlays = match rel("Overlays") {
        Some(toy) => {
            let rects: Vec<crate::overlay::RoomRect> = placed
                .iter()
                .map(|p| {
                    let feature = p.tile.is_none() && !set.rooms[p.room].room.arm.tag.is_empty();
                    crate::overlay::RoomRect {
                        rect: (p.pos.0, p.pos.1, p.pos.0 + p.size.0 as i32, p.pos.1 + p.size.1 as i32),
                        feature,
                        described: !graph.nodes[p.node].tags.is_empty(),
                        inset: 0,
                    }
                })
                .collect();
            Some(crate::overlay::Overlays::new(files, &toy, &map, &mut master, &rects, files.special_tile(2))?)
        }
        None => None,
    };
    if let Some(o) = &mut overlays {
        o.run(1, &mut map, &mut master, &mut types, files, rng)?;
    }
    probe(Stage::Lattice("fill", &map));
    let mut fill_rooms = pre_rooms;
    fill_rooms.extend(fill_pass(&mut map, &mut master, files, rng)?);
    if let Some(o) = &mut overlays {
        o.run(2, &mut map, &mut master, &mut types, files, rng)?;
    }
    let mut rooms: Vec<RoomPlacement> = placed
        .iter()
        .filter(|p| p.tile.is_none())
        .map(|p| RoomPlacement { room: &set.rooms[p.room].room, origin: p.pos, orientation: p.orientation })
        .collect();
    rooms.extend(fill_rooms.iter().map(|r| RoomPlacement { room: &r.room, origin: r.origin, orientation: r.orientation }));
    if let Some(o) = &overlays {
        rooms.extend(o.placed.iter().map(|&(i, origin, orientation)| RoomPlacement { room: o.room(i), origin, orientation }));
        rooms.extend(o.fill_rooms.iter().map(|r| RoomPlacement { room: &r.room, origin: r.origin, orientation: r.orientation }));
    }
    derepeat_pass(&mut map, &master, &rooms, rng);
    crate::postunique::run(&mut map, &mut master, files, rng)?;

    Ok(Built { map, master, tsi_bytes, tsi, subgraphs: Vec::new() })
}

