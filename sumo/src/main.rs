use std::collections::BTreeSet;

use abstutil::{CmdArgs, MapName, Timer};
use geom::Distance;
use map_model::{osm, Intersection, IntersectionID, IntersectionType, Map};

use sumo::Network;

fn main() {
    let mut timer = Timer::new("convert SUMO network");
    let mut args = CmdArgs::new();
    let input = args.required_free();
    args.done();

    let network = sumo::Network::load(input, &mut timer).unwrap();
    let map = convert(network);
    map.save();
}

fn convert(network: Network) -> Map {
    let mut intersections = Vec::new();
    for (_, junction) in network.junctions {
        intersections.push(Intersection {
            id: IntersectionID(intersections.len()),
            polygon: junction.shape,
            turns: BTreeSet::new(),
            elevation: Distance::ZERO,
            intersection_type: IntersectionType::StopSign,
            orig_id: osm::NodeID(123),
            incoming_lanes: Vec::new(),
            outgoing_lanes: Vec::new(),
            roads: BTreeSet::new(),
        });
    }

    Map::import_minimal(MapName::new("sumo", "test"), network.bounds, intersections)
}
