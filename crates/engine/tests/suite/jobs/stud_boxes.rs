//! Q6623 fix-stud-layout-ignores-boxes: the trade stud layout stands no stud inside a device box
//! (or anything else another trade set in the wall). The stud the layout puts through a box
//! stands at the box's side, where the electrician fastens it; the stud says where the layout
//! had it and why; `bim.clash` with the hosted boxes finds no box standing in a stud.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

use crate::conduitroute::storey_walls;

type Bx = ([f64; 3], [f64; 3]);

/// The made pieces of `kind` (`part:atom:<kind>`) on `level`: their boxes, handles.
fn atoms(s: &Session, kind: &str, level: &str) -> Vec<(Bx, cadaddy_engine::doc::Handle)> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| e.common.xdata.iter().any(|t| t.value == format!("part:atom:{kind}")) && e.common.xdata.iter().any(|t| t.value == level))
        .filter_map(|e| {
            let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind else { return None };
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for v in &m.vertices {
                for (i, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                    lo[i] = lo[i].min(c);
                    hi[i] = hi[i].max(c);
                }
            }
            Some(((lo, hi), e.handle))
        })
        .collect()
}

/// How far two boxes overlap on their least axis (inches; none when they don't).
fn overlap(a: &Bx, b: &Bx) -> Option<f64> {
    let d = (0..3).map(|i| a.1[i].min(b.1[i]) - a.0[i].max(b.0[i])).fold(f64::INFINITY, f64::min);
    (d > 0.05).then_some(d)
}

fn receptacle(n: usize, x: f64, y: f64) -> Value {
    device(n, "receptacle", x, y, 18)
}

fn device(n: usize, kind: &str, x: f64, y: f64, mount_in: u32) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x2A0 + n), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "circuit": {"value": "LP1-1", "source": "plan"},
        "mount_height_in": {"value": mount_in.to_string(), "source": "note"}
    }})
}

fn circuit() -> Value {
    json!({"id": "circuit:L1:E-101:LP1-1", "kind": "circuit", "level": "L1", "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": "LP1-1", "source": "plan"},
        "panel": {"value": "LP1", "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "conductors": {"value": "2#12 + 1#12G", "source": "default"},
        "conduit": {"value": "3/4\" EMT", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }})
}

/// Two stacked storeys of 2x4 wood stud exterior walls at 16" o.c., and L1's receptacles at
/// `at` (feet; each 18" above the floor).
fn building(at: &[[f64; 2]]) -> (Session, Value) {
    let tags: Vec<Value> = at.iter().enumerate().map(|(i, p)| receptacle(i + 1, p[0], p[1])).collect();
    building_with(tags, false)
}

/// Half the partition's thickness, inches.
const HALF: f64 = 4.875 / 2.0;

/// [`building`] with L1's device `tags`; `door`: a 2x4 partition across L1 at 20' with a 3' door
/// from 100" to 136" (y).
fn building_with(mut tags: Vec<Value>, door: bool) -> (Session, Value) {
    let mut d = Drawing::new_imperial();
    storey_walls(&mut d, 0.0);
    storey_walls(&mut d, 200.0);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    if door {
        if s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).is_err() {
            s.execute("layer.current", &json!({"name": "A-WALL"})).expect("layer");
        }
        for dx in [-HALF, HALF] {
            s.execute("line", &json!({"points": [[240.0 + dx, 7.25], [240.0 + dx, 100.0]]})).expect("line");
            s.execute("line", &json!({"points": [[240.0 + dx, 136.0], [240.0 + dx, 288.0 - 7.25]]})).expect("line");
        }
        // Its swing: hinged at the gap's start, a 3' leaf.
        s.execute("arc", &json!({"center": [240.0, 100.0], "radius": 36.0, "start": 0.0, "end": 90.0})).expect("arc");
    }
    let exterior = json!({
        "tag": "X1", "description": "EXTERIOR WALL", "source": "sample legend, mark X1",
        "layers": [
            {"material": "1/2\" wood structural panel sheathing", "thickness_in": 0.5, "category": "sheathing", "side": "side1"},
            {"material": "2x4 wood studs @ 16\" o.c.", "thickness_in": 3.5, "category": "framing", "side": "core"},
            {"material": "R-13 batt insulation", "thickness_in": 3.5, "category": "insulation", "side": "core", "in_cavity": true},
            {"material": "5/8\" gypsum board", "thickness_in": 0.625, "category": "board", "side": "side2"}
        ]
    });
    tags.push(circuit());
    let partition = json!({
        "tag": "P1", "description": "PARTITION", "source": "sample legend, mark P1",
        "layers": [
            {"material": "5/8\" gypsum board", "thickness_in": 0.625, "category": "board", "side": "side1"},
            {"material": "2x4 wood studs @ 16\" o.c.", "thickness_in": 3.5, "category": "framing", "side": "core"},
            {"material": "5/8\" gypsum board", "thickness_in": 0.625, "category": "board", "side": "side2"}
        ]
    });
    let walls = if door { json!([{"level": "L1", "a": [240.0, 7.25], "b": [240.0, 288.0 - 7.25], "wall_type": "P1"}]) } else { json!([]) };
    let spec = json!({
        "version": 1, "units_per_foot": 12, "stack": true,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]},
            {"name": "L2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "region": [1900, -100, 3400, 1000], "base": [2400, 0]}
        ],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [
            {"name": "X1", "thickness_in": 7.25, "from": "pairs", "assembly": exterior},
            {"name": "P1", "thickness_in": 4.875, "from": "pairs", "assembly": partition}
        ],
        "walls": walls,
        "tags": tags,
        "mep": {"systems": [], "runs": [], "equipment": [
            {"tag": "LP1", "kind": "panel", "level": "L1", "min": [12, 120], "max": [32, 124], "height_ft": 4, "bottom_ft": 2.5,
             "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
        ]}
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    assert!(built["tradePieces"].as_u64().unwrap_or(0) > 0, "the build lays out the trades' pieces: {built}");
    (s, built)
}

/// The south wall's studs on L1 (its inside face at y = 7.25"): centres along x, inches.
fn south_studs(s: &Session) -> Vec<f64> {
    let mut v: Vec<f64> = atoms(s, "stud", "L1")
        .into_iter()
        .filter(|((lo, hi), _)| hi[1] < 8.0 && lo[0] > 30.0 && hi[0] < 450.0)
        .map(|((lo, hi), _)| (lo[0] + hi[0]) / 2.0)
        .collect();
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    v
}

#[test]
fn the_stud_layout_stands_its_studs_beside_the_boxes_in_the_wall() {
    // Where the layout alone stands the south wall's studs.
    let (bare, _) = building(&[]);
    let studs = south_studs(&bare);
    assert!(studs.len() > 10, "the south wall is studded: {studs:?}");
    // Receptacles on the south wall's inside face, each centred on a stud of that layout.
    let picks: Vec<f64> = [3usize, 7, 11].iter().filter_map(|i| studs.get(*i).copied()).collect();
    assert_eq!(picks.len(), 3, "{studs:?}");
    let at: Vec<[f64; 2]> = picks.iter().map(|x| [x / 12.0, 9.0 / 12.0]).collect();
    let (mut s, built) = building(&at);
    let boxes = atoms(&s, "box", "L1");
    let in_wall: Vec<&(Bx, _)> = boxes.iter().filter(|((lo, _), _)| lo[1] < 7.25).collect();
    assert!(in_wall.len() >= 3, "each receptacle's box is in the south wall: {boxes:?} {}", short(&built));
    let studs_now = atoms(&s, "stud", "L1");
    // No box stands in a stud.
    let inside: Vec<(Bx, Bx)> =
        in_wall.iter().flat_map(|(b, _)| studs_now.iter().filter(|(st, _)| overlap(b, st).is_some()).map(|(st, _)| (*b, *st))).collect();
    assert!(inside.is_empty(), "boxes in studs: {inside:?}");
    // Each box has a stud at its side (fastened to it), carrying where the layout had it and why.
    let d = s.doc().unwrap();
    for (b, _) in &in_wall {
        let beside = studs_now.iter().find(|(st, _)| {
            ((st.1[0] - b.0[0]).abs() < 0.05 || (st.0[0] - b.1[0]).abs() < 0.05) && st.0[2] < b.1[2] && st.1[2] > b.0[2] && st.1[1] < 8.0
        });
        let (_, h) = beside.unwrap_or_else(|| panic!("a stud at the side of the box {b:?}"));
        let ps = cadaddy_engine::doc::props::read(&d.entity(*h).unwrap().common.xdata);
        let get = |k: &str| ps.get(k).map(cadaddy_engine::doc::props::Prop::text).unwrap_or_default();
        assert_eq!(get("moved_for"), "box", "{ps:?}");
        let moved: f64 = get("moved_in").parse().unwrap();
        assert!(moved.abs() > 0.5 && moved.abs() < 4.0, "a few inches: {moved}");
        let layout_at: f64 = get("layout_at_in").parse().unwrap();
        assert!(layout_at > 0.0, "{ps:?}");
    }
    // The other studs stay where the layout put them: as many studs as before.
    assert_eq!(south_studs(&s).len(), studs.len());
    let notes = built["tradeNotes"].as_array().cloned().unwrap_or_default();
    assert!(notes.iter().any(|n| n.as_str().unwrap_or("").contains("studs set at the side")), "{notes:?}");
    // The clash check with the boxes the walls host: no box stands in a stud.
    let r = s.execute("bim.clash", &json!({"a": "framing", "b": "electrical", "hosted": true})).unwrap();
    let stands: Vec<&Value> =
        r["clashes"].as_array().into_iter().flatten().filter(|c| c["framing"]["why"].as_str().unwrap_or("").contains("stands in the stud")).collect();
    eprintln!("stands-in-the-stud clashes: {} (framing {})", stands.len(), r["framing"]);
    assert!(stands.is_empty(), "{stands:?}");
    // Laid out again (bim.atoms.build), the same.
    s.execute("bim.atoms.build", &json!({"trades": ["framing"]})).unwrap();
    let again = atoms(&s, "stud", "L1");
    assert!(in_wall.iter().all(|(b, _)| again.iter().all(|(st, _)| overlap(b, st).is_none())));
}

#[test]
fn a_box_drawn_in_a_doors_jamb_studs_moves_off_them_into_the_bay() {
    // A switch drawn 3" past the door's opening (48" AFF, on the partition's east face): its box
    // would stand in the door's king stud (the jack beside the opening under the header, the king
    // beyond it: 137 1/2" to 139").
    let tags = vec![device(1, "switch", (240.0 + HALF + 0.25) / 12.0, 139.0 / 12.0, 48)];
    let (mut s, built) = building_with(tags, true);
    let boxes: Vec<(Bx, _)> = atoms(&s, "box", "L1").into_iter().filter(|((lo, hi), _)| lo[0] > 230.0 && hi[0] < 250.0).collect();
    assert_eq!(boxes.len(), 1, "the switch's box is in the partition: {} {}", boxes.len(), short(&built));
    let (b, h) = boxes[0];
    let studs = atoms(&s, "stud", "L1");
    let inside: Vec<Bx> = studs.iter().filter(|(st, _)| overlap(&b, st).is_some()).map(|(st, _)| *st).collect();
    assert!(inside.is_empty(), "the box {b:?} stands in studs: {inside:?}");
    // Moved off the jambs, away from the door, with the design spot and the reason kept.
    assert!(b.0[1] >= 139.0 - 0.05, "north of the king stud: {b:?}");
    assert!(b.0[1] <= 139.0 + 0.5, "just clear of it: {b:?}");
    let d = s.doc().unwrap();
    let ps = cadaddy_engine::doc::props::read(&d.entity(h).unwrap().common.xdata);
    let corr = ps.get("correction:location").map(cadaddy_engine::doc::props::Prop::text).unwrap_or_default();
    assert!(corr.contains("jamb studs"), "{ps:?}");
    assert!(ps.get("design:location").is_some(), "{ps:?}");
    // The device moves with its box; the clash check finds no box in a stud.
    let r = s.execute("bim.clash", &json!({"a": "framing", "b": "electrical", "hosted": true})).unwrap();
    let stands =
        r["clashes"].as_array().into_iter().flatten().filter(|c| c["framing"]["why"].as_str().unwrap_or("").contains("stands in the stud")).count();
    assert_eq!(stands, 0, "{}", short(&r));
}

fn short(v: &Value) -> String {
    v.to_string().chars().take(3_000).collect()
}
