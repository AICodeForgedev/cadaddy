//! A stepped parapet (fix-parapet-one-height-per-roof): a flat 200' x 60' roof on trusses whose
//! elevations mark three tops of parapet: +5'-0" over the roof on the short (east and west)
//! facades, +7'-0" on the long ones and +9'-6" over a 40' segment of the north facade. Each
//! outline edge (and each piece of one between two marks) gets the top its own facade's
//! elevation marks; an edge no elevation marks takes its neighbour's, marked as a default; every
//! distinct top is kept in the spec and reported by the build.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Mesh};
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn poly(s: &mut Session, pts: &[[f64; 2]]) {
    s.execute("pline", &json!({"vertices": pts, "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// The plate: 200' x 60' (inches).
const LONG: f64 = 2400.0;
const SHORT: f64 = 720.0;
const ROOF_AT: [f64; 2] = [4000.0, 0.0];
const ROOF_PLAN: [f64; 4] = [3900.0, -100.0, 6600.0, 900.0];

/// One elevation at `y0` (its ground line): its title, level lines labelled at their right end,
/// the building's silhouette (`outline`, relative to the left end of the ground line) and the
/// region it stands in.
fn elevation(s: &mut Session, y0: f64, title: &str, length: f64, outline: &[[f64; 2]], marks: &[(f64, &str)]) -> [f64; 4] {
    layer(s, "A-ELEV");
    text(s, [length / 2.0, y0 - 60.0], title);
    for (dy, label) in [(0.0, "LEVEL 1 EL. 100'-0\""), (144.0, "T.O. ROOF EL. 112'-0\"")].iter().chain(marks) {
        line(s, [-60.0, y0 + dy], [length + 60.0, y0 + dy]);
        text(s, [length + 80.0, y0 + dy + 6.0], label);
    }
    let pts: Vec<[f64; 2]> = outline.iter().map(|p| [p[0], y0 + p[1]]).collect();
    poly(s, &pts);
    [-200.0, y0 - 100.0, length + 400.0, y0 + 400.0]
}

const LOW: (f64, &str) = (204.0, "T.O. PARAPET EL. 117'-0\"");
const MID: (f64, &str) = (228.0, "T.O. PARAPET EL. 119'-0\"");
const HIGH: (f64, &str) = (258.0, "T.O. PARAPET EL. 121'-6\"");

/// The one-storey building, its roof plan and spec (Level 1 at 0, the roof bearing at 12'). The
/// elevations' regions: south, north, east, west.
fn building() -> (Session, Vec<[f64; 4]>) {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [LONG, SHORT]);
    rect(&mut s, [6.0, 6.0], [LONG - 6.0, SHORT - 6.0]);
    layer(&mut s, "A-ROOF");
    rect(&mut s, ROOF_AT, [ROOF_AT[0] + LONG, ROOF_AT[1] + SHORT]);
    rect(&mut s, [ROOF_AT[0] + 8.0, ROOF_AT[1] + 8.0], [ROOF_AT[0] + LONG - 8.0, ROOF_AT[1] + SHORT - 8.0]);
    let box_to = |l: f64, h: f64| vec![[0.0, 0.0], [l, 0.0], [l, h], [0.0, h]];
    let mut regions = Vec::new();
    regions.push(elevation(&mut s, -3000.0, "SOUTH ELEVATION", LONG, &box_to(LONG, MID.0), &[MID]));
    // The north facade as seen from the north: its left end is the building's east end; the
    // parapet steps up from 40' to 80' along it.
    let north = [[0.0, 0.0], [LONG, 0.0], [LONG, MID.0], [960.0, MID.0], [960.0, HIGH.0], [480.0, HIGH.0], [480.0, MID.0], [0.0, MID.0]];
    regions.push(elevation(&mut s, -4000.0, "NORTH ELEVATION", LONG, &north, &[MID, HIGH]));
    regions.push(elevation(&mut s, -5000.0, "EAST ELEVATION", SHORT, &box_to(SHORT, LOW.0), &[LOW]));
    regions.push(elevation(&mut s, -6000.0, "WEST ELEVATION", SHORT, &box_to(SHORT, LOW.0), &[LOW]));
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "units_per_foot": 12,
            "levels": [
                {"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": [-100, -100, 3000, 1000], "base": [0, 0]},
                {"name": "Roof", "elevation_ft": 12, "height_ft": 4, "ceiling_ft": 4, "region": ROOF_PLAN}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "exterior": {"framing": [{"level": "Roof", "member": "truss", "spacing_in": 24, "depth_in": 24, "bearing_ft": 10}]}
        }}),
    )
    .unwrap();
    (s, regions)
}

fn made(s: &Session, part: &str) -> Vec<Mesh> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some(part))
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None })
        .collect()
}

fn top(m: &Mesh) -> f64 {
    m.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max)
}

/// The tops the built parapet stands at, highest first, rounded to the inch.
fn built_tops(s: &Session) -> Vec<f64> {
    let mut v: Vec<f64> = made(s, "parapet").iter().map(|m| top(m).round()).collect();
    v.sort_by(|a, b| b.total_cmp(a));
    v.dedup();
    v
}

fn read_all(s: &mut Session, regions: &[[f64; 4]]) -> Vec<Value> {
    regions.iter().map(|r| s.execute("elev.read", &json!({"region": r, "apply": true})).unwrap()).collect()
}

#[test]
fn each_facade_keeps_its_own_top_of_parapet() {
    let (mut s, regions) = building();
    let reads = read_all(&mut s, &regions);
    // The north elevation reads both its tops, the high one over its 40' to 80' only.
    let north = &reads[1];
    let tops: Vec<Value> = north["parapets"].as_array().unwrap().iter().map(|p| json!([p["height_ft"], p["start_ft"], p["end_ft"]])).collect();
    assert_eq!(tops, vec![json!([7.0, 0.0, 40.0]), json!([7.0, 80.0, 200.0]), json!([9.5, 40.0, 80.0])], "{north}");
    // All the facades' tops are kept, none overwrites another's.
    s.execute("roof.read", &json!({"region": ROOF_PLAN, "apply": true})).unwrap();
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    let kept: Vec<(String, f64)> = sp["exterior"]["parapets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["facade"].as_str().unwrap_or("").to_string(), p["height_ft"].as_f64().unwrap()))
        .collect();
    for want in [("south", 7.0), ("north", 7.0), ("north", 9.5), ("east", 5.0), ("west", 5.0)] {
        assert!(kept.iter().any(|(f, h)| f == want.0 && *h == want.1), "{want:?} not kept: {kept:?}");
    }
    // The build: three tops over the roof's surface (12' = 144").
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(built_tops(&s), vec![144.0 + 114.0, 144.0 + 84.0, 144.0 + 60.0], "{r}");
    // Each piece on its facade, as long as the elevations mark it.
    let pieces = r["parapetTops"].as_array().unwrap_or_else(|| panic!("no parapetTops: {r}"));
    let lf = |facade: &str, h: f64| -> f64 {
        pieces.iter().filter(|p| p["facade"] == facade && p["height_ft"].as_f64() == Some(h)).map(|p| p["lf"].as_f64().unwrap()).sum()
    };
    let near = |a: f64, b: f64| (a - b).abs() < 0.5;
    assert!(near(lf("north", 9.5), 40.0) && near(lf("north", 7.0), 160.0), "{pieces:?}");
    assert!(near(lf("south", 7.0), 200.0) && near(lf("east", 5.0), 60.0) && near(lf("west", 5.0), 60.0), "{pieces:?}");
    assert!(pieces.iter().all(|p| p["status"] == "read"), "every edge is marked: {pieces:?}");
    // The high piece is 80' to 120' from the west end (40' to 80' from the east, as seen).
    let high = made(&s, "parapet").into_iter().find(|m| (top(m) - 258.0).abs() < 0.5).unwrap();
    let x0 = made(&s, "roof").iter().flat_map(|m| m.vertices.iter().map(|v| v.x)).fold(f64::INFINITY, f64::min);
    let (lo, hi) = high.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v.x - x0), b.max(v.x - x0)));
    assert!((lo - 1440.0).abs() < 9.0 && (hi - 1920.0).abs() < 9.0, "{lo} {hi}");
    // The takeoff's parapet length is still the outline's.
    let total: f64 = pieces.iter().map(|p| p["lf"].as_f64().unwrap()).sum();
    assert!(near(total, 520.0), "{total}");
}

#[test]
fn an_edge_no_elevation_marks_takes_its_neighbours_top_as_a_default() {
    let (mut s, regions) = building();
    // Only the south and north elevations are read: the short facades are not marked.
    read_all(&mut s, &regions[..2]);
    s.execute("roof.read", &json!({"region": ROOF_PLAN, "apply": true})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    let pieces = r["parapetTops"].as_array().unwrap();
    for side in ["east", "west"] {
        let p: Vec<&Value> = pieces.iter().filter(|p| p["facade"] == side).collect();
        assert!(!p.is_empty() && p.iter().all(|p| p["status"] == "default" && p["height_ft"] == json!(7.0)), "{side}: {p:?}");
        assert!(p.iter().all(|p| p["basis"].as_str().unwrap().contains("neighbour")), "{p:?}");
    }
    assert!(pieces.iter().filter(|p| p["facade"] == "south").all(|p| p["status"] == "read"));
    assert_eq!(built_tops(&s), vec![258.0, 228.0]);
}

#[test]
fn reading_the_elevations_again_or_in_another_order_builds_the_same() {
    let (mut s, regions) = building();
    read_all(&mut s, &regions);
    s.execute("roof.read", &json!({"region": ROOF_PLAN, "apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let a = built_tops(&s);
    let (mut s, mut regions) = building();
    regions.reverse();
    s.execute("roof.read", &json!({"region": ROOF_PLAN, "apply": true})).unwrap();
    read_all(&mut s, &regions);
    read_all(&mut s, &regions);
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    assert_eq!(sp["exterior"]["parapets"].as_array().unwrap().len(), 6, "a facade read twice replaces its own tops: {}", sp["exterior"]["parapets"]);
    s.execute("model.build", &json!({})).unwrap();
    assert_eq!(built_tops(&s), a);
}
