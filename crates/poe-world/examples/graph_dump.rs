//! Prints a `.tgr`/`.dgr` graph's nodes and edges: `graph_dump <path>`.
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: graph_dump <path>")?;
    let files = poe_data::GameFiles::open(Path::new("GameFiles"))?;
    let g = poe_world::graph::Graph::parse_bytes(&poe_world::generate::Files::read(&files, &path.to_lowercase())?).map_err(|e| format!("{e:?}"))?;
    println!("{}x{} master {} region {}", g.width, g.height, g.master_file, g.region);
    for (i, n) in g.nodes.iter().enumerate() {
        println!("node {i}: ({}, {}) edges {:?} room {:?} {} tags {:?} values {:?}", n.x, n.y, n.edges, n.room_type, n.orientation, n.tags, n.values);
    }
    for (i, e) in g.edges.iter().enumerate() {
        println!("edge {i}: {} -> {} points {:?} values {:?} {}", e.from, e.to, e.points, e.values, e.edge_type);
    }
    Ok(())
}
