use std::collections::BTreeMap;
use std::error::Error;

use abstutil::Timer;
use geom::{Bounds, Distance, PolyLine, Polygon, Pt2D, Ring, Speed};

/// Reference: <https://sumo.dlr.de/docs/Networks/SUMO_Road_Networks.html>
#[derive(Debug)]
pub struct Network {
    pub bounds: Bounds,
    pub edges: BTreeMap<EdgeID, Edge>,
    pub junctions: BTreeMap<NodeID, Junction>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct EdgeID(String);
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct NodeID(String);
#[derive(Debug)]
pub struct LaneID(String);

#[derive(Debug)]
pub struct Edge {
    pub id: EdgeID,
    pub from: NodeID,
    pub to: NodeID,
    pub priority: Option<usize>,
    pub function: Function,
    pub lanes: Vec<Lane>,
}

#[derive(Debug)]
pub enum Function {
    Normal,
    Internal,
}

#[derive(Debug)]
pub struct Lane {
    pub id: LaneID,
    // 0 is the rightmost lane
    pub index: usize,
    pub speed: Speed,
    pub length: Distance,
    pub center_line: PolyLine,
}

#[derive(Debug)]
pub struct Junction {
    pub id: NodeID,
    pub pt: Pt2D,
    pub incoming_lanes: Vec<LaneID>,
    pub internal_lanes: Vec<LaneID>,
    pub shape: Polygon,
}

impl Network {
    pub fn load(path: String, timer: &mut Timer) -> Result<Network, Box<dyn Error>> {
        timer.start(format!("read {}", path));
        let bytes = abstutil::slurp_file(&path)?;
        let raw_string = std::str::from_utf8(&bytes)?;
        let tree = roxmltree::Document::parse(raw_string)?;
        timer.stop(format!("read {}", path));

        let mut network = Network {
            bounds: Bounds::new(),
            edges: BTreeMap::new(),
            junctions: BTreeMap::new(),
        };

        for obj in tree.root_element().children() {
            if !obj.is_element() {
                continue;
            }
            match obj.tag_name().name() {
                "location" => {
                    network.bounds = parse_bounds(obj.attribute("convBoundary").unwrap());
                }
                "type" => {}
                "edge" => {
                    if let Some(edge) = Edge::parse(obj) {
                        network.edges.insert(edge.id.clone(), edge);
                    }
                }
                "tlLogic" => {}
                "junction" => {
                    // TODO
                    if obj.attribute("type") == Some("internal") {
                        continue;
                    }
                    let junction = Junction::parse(obj);
                    network.junctions.insert(junction.id.clone(), junction);
                }
                "connection" => {}
                x => {
                    println!("sup {}", x);
                }
            }
        }

        Ok(network)
    }
}

fn parse_shape(raw: &str) -> Vec<Pt2D> {
    raw.split(" ").map(|pt| parse_pt(pt)).collect()
}

fn parse_pt(pt: &str) -> Pt2D {
    let parts: Vec<f64> = pt.split(",").map(|x| x.parse::<f64>().unwrap()).collect();
    // Ignore the Z coordinate if it's there
    assert!(parts.len() == 2 || parts.len() == 3);
    Pt2D::new(parts[0], parts[1])
}

fn parse_bounds(raw: &str) -> Bounds {
    let parts: Vec<f64> = raw.split(",").map(|x| x.parse::<f64>().unwrap()).collect();
    assert_eq!(parts.len(), 4);
    Bounds {
        min_x: parts[0],
        min_y: parts[1],
        max_x: parts[2],
        max_y: parts[3],
    }
}

impl Edge {
    fn parse(node: roxmltree::Node) -> Option<Edge> {
        let mut edge = Edge {
            id: EdgeID(node.attribute("id").unwrap().to_string()),
            from: NodeID(node.attribute("from")?.to_string()),
            to: NodeID(node.attribute("to").unwrap().to_string()),
            priority: node
                .attribute("priority")
                .and_then(|x| x.parse::<usize>().ok()),
            function: match node.attribute("function") {
                Some("internal") => Function::Internal,
                Some(x) => unimplemented!("Handle function={}", x),
                None => Function::Normal,
            },
            lanes: Vec::new(),
        };

        for child in node.children() {
            if !child.is_element() || child.tag_name().name() != "lane" {
                continue;
            }
            edge.lanes.push(Lane::parse(child));
        }

        Some(edge)
    }
}

impl Lane {
    fn parse(node: roxmltree::Node) -> Lane {
        Lane {
            id: LaneID(node.attribute("id").unwrap().to_string()),
            index: node.attribute("index").unwrap().parse::<usize>().unwrap(),
            speed: Speed::meters_per_second(
                node.attribute("speed").unwrap().parse::<f64>().unwrap(),
            ),
            length: Distance::meters(node.attribute("length").unwrap().parse::<f64>().unwrap()),
            center_line: PolyLine::must_new(parse_shape(node.attribute("shape").unwrap())),
        }
    }
}

impl Junction {
    fn parse(node: roxmltree::Node) -> Junction {
        Junction {
            id: NodeID(node.attribute("id").unwrap().to_string()),
            pt: Pt2D::new(
                node.attribute("x")
                    .and_then(|x| x.parse::<f64>().ok())
                    .unwrap(),
                node.attribute("y")
                    .and_then(|x| x.parse::<f64>().ok())
                    .unwrap(),
            ),
            incoming_lanes: node
                .attribute("incLanes")
                .unwrap()
                .split(" ")
                .map(|x| LaneID(x.to_string()))
                .collect(),
            internal_lanes: node
                .attribute("intLanes")
                .unwrap()
                .split(" ")
                .map(|x| LaneID(x.to_string()))
                .collect(),
            shape: {
                let mut pts = parse_shape(node.attribute("shape").unwrap());
                pts.push(pts[0]);
                pts.dedup();
                Ring::must_new(pts).to_polygon()
            },
        }
    }
}
