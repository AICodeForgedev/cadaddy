//! Test finding (fix-box-separation-no-putty-pad-choice): the rated-wall box-separation rule
//! only knew one remedy for boxes back to back in a fire-rated wall (move one a stud bay). The
//! code (IBC 714.4.2) allows the other: listed putty pads on both boxes, the devices where the
//! architect dimensioned them. The project chooses; the default is the move, with a note.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn wall_line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

/// Two mirrored 19'-6" x 9' units (made-up units A and B) with one 1-hour 6" wall between them
/// (its faces at 19'-9" and 20'-3"), each unit's receptacle back to back with the other's at
/// `y` ft in that wall, on circuit LP1-3 from panel LP1 in unit A. `rule` sets the project's
/// choice first. The session and the build's report.
fn mirrored_units(y: (f64, f64), rule: Option<&str>) -> (Session, Value) {
    let mut d = Drawing::new_imperial();
    let o = [((0., 0.), (40., 0.)), ((40., 0.), (40., 10.)), ((40., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let i = [((0.5, 0.5), (39.5, 0.5)), ((39.5, 0.5), (39.5, 9.5)), ((39.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    let mid = [((19.75, 0.5), (19.75, 9.5)), ((20.25, 0.5), (20.25, 9.5))];
    for (a, b) in o.into_iter().chain(i).chain(mid) {
        wall_line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    if let Some(v) = rule {
        s.execute(
            "project.rules.set",
            &json!({"domain": "clearance", "key": "rated_wall_boxes", "value": v, "tier": "spec", "source": "Spec 26 05 33", "reason": "the specification's choice"}),
        )
        .expect("rule");
    }
    let dev = |n: usize, x: f64, y: f64| {
        json!({"id": format!("device:L1:{n}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y], "fields": {
            "symbol": {"value": format!("{:X}", 0x1F0 + n), "source": "plan"}, "sheet": {"value": "E-101", "source": "plan"},
            "circuit": {"value": "LP1-3", "source": "plan"}, "mount_height_in": {"value": "18", "source": "note"}
        }})
    };
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6, "assembly": {"tag": "P1", "fire_rating": "1 HR"}}],
        "tags": [
            dev(1, 19.6, y.0),
            dev(2, 20.4, y.1),
            {"id": "circuit:L1:E-101:LP1-3", "kind": "circuit", "level": "L1", "at_ft": [2.0, 2.0], "fields": {
                "circuit": {"value": "LP1-3", "source": "plan"}, "panel": {"value": "LP1", "source": "plan"},
                "sheet": {"value": "E-101", "source": "plan"},
                "conductors": {"value": "2#12 + 1#12G", "source": "default"}, "conduit": {"value": "3/4\" EMT", "source": "default"},
                "route": {"value": "estimated route", "source": "rule"}
            }}
        ],
        "mep": {
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [6, 7], "max": [26, 11], "height_ft": 4, "bottom_ft": 2.5,
                 "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    (s, built)
}

/// Each receptacle: its handle, its plan centre (inches) and its props.
fn receptacles(s: &Session) -> Vec<(Handle, [f64; 2], cadaddy_engine::doc::props::Props)> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device"))
        .filter_map(|e| {
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let n = m.vertices.len().max(1) as f64;
            let c = m.vertices.iter().fold([0.0; 2], |s, v| [s[0] + v.x / n, s[1] + v.y / n]);
            Some((e.handle, c, cadaddy_engine::doc::props::read(&e.common.xdata)))
        })
        .collect()
}

/// Each box atom: its device, its body's centre (inches) and its props.
fn boxes(s: &Session) -> Vec<(String, [f64; 3], cadaddy_engine::doc::props::Props)> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter_map(|e| {
            let ps = cadaddy_engine::doc::props::read(&e.common.xdata);
            if ps.get("atom").map(|p| p.text()).as_deref() != Some("box") {
                return None;
            }
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for v in &m.vertices {
                for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                    lo[k] = lo[k].min(c);
                    hi[k] = hi[k].max(c);
                }
            }
            let of = ps.get("box_of").map(|p| p.text()).unwrap_or_default();
            Some((of, [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0], ps))
        })
        .collect()
}

/// The putty pads in the takeoff (its `firestop` lines of the rules' item): how many.
fn pads_in_takeoff(s: &mut Session) -> f64 {
    let t = s.execute("conduit.takeoff", &json!({"price": false})).expect("takeoff");
    t["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["kind"] == json!("firestop") && l["item"].as_str().is_some_and(|i| i.to_ascii_lowercase().contains("putty pad")))
        .map(|l| l["qty"].as_f64().unwrap_or(0.0))
        .sum()
}

/// The ends of every conduit stick of LP1-3 (inches).
fn stick_ends(s: &Session) -> Vec<[f64; 3]> {
    let d = s.doc().expect("doc");
    let xyz = |t: String| {
        let v: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
        [v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0), v.get(2).copied().unwrap_or(0.0)]
    };
    d.model
        .iter()
        .map(|e| cadaddy_engine::doc::props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(|x| x.text()).as_deref() == Some("conduit"))
        .flat_map(|p| [xyz(p.get("start").map(|x| x.text()).unwrap_or_default()), xyz(p.get("end").map(|x| x.text()).unwrap_or_default())])
        .collect()
}

/// Whether some conduit stick ends at the box centred at `c` (plan within 6", the end at or
/// above the box: a drop into its top).
fn conduit_ends_at(ends: &[[f64; 3]], c: [f64; 3]) -> bool {
    ends.iter().any(|e| (e[0] - c[0]).hypot(e[1] - c[1]) <= 6.0 && e[2] >= c[2] - 3.0 && e[2] <= c[2] + 12.0)
}

#[test]
fn putty_pads_keep_back_to_back_devices_where_they_are_drawn() {
    // As drawn: the two units' receptacles back to back at 5'-0" in the rated wall.
    let (mut s, built) = mirrored_units((5.0, 5.0), Some("putty pads"));
    let devs = receptacles(&s);
    assert_eq!(devs.len(), 2, "{built}");
    for (h, c, ps) in &devs {
        assert!(ps.get("design:location").is_none(), "device {} moved: {:?}", h.hex(), ps.get("correction:location").map(|p| p.text()));
        assert!((c[1] - 60.0).abs() < 1.0, "device {} at y {} in, drawn at 60", h.hex(), c[1]);
    }
    let bx = boxes(&s);
    assert_eq!(bx.len(), 2, "{built}");
    for (of, c, ps) in &bx {
        assert!(ps.get("design:location").is_none(), "box of {of} moved");
        assert!((c[1] - 60.0).abs() < 3.0, "box of {of} at y {} in", c[1]);
        let why = ps.get("correction:firestop").map(|p| p.text()).unwrap_or_default();
        assert!(why.contains("IBC 714.4.2") && why.contains("putty pads"), "{why}");
        assert!(why.contains("Spec 26 05 33"), "the choice's source: {why}");
    }
    // One putty pad per box, each its own takeoff item.
    assert_eq!(built["boxClearance"]["puttyPads"], json!(2), "{built}");
    let fs = s.execute("bim.count", &json!({"kind": "firestop"})).expect("count");
    let tk = s.execute("conduit.takeoff", &json!({"price": false})).expect("takeoff");
    assert_eq!(pads_in_takeoff(&mut s), 2.0, "{fs} {tk}");
    // The routed raceway lands in each box where it stands (the drawn position).
    let ends = stick_ends(&s);
    for (of, c, _) in &bx {
        assert!(conduit_ends_at(&ends, *c), "no conduit ends at the box of {of} at {c:?}: {ends:?}");
    }
    // The report says which remedy was used and why.
    let notes = built.to_string();
    assert!(notes.contains("putty pads at the drawn positions"), "{notes}");
}

#[test]
fn by_default_one_box_moves_a_stud_bay_and_the_note_names_the_alternative() {
    let (mut s, built) = mirrored_units((5.0, 5.0), None);
    let moved: Vec<_> = receptacles(&s).into_iter().filter(|d| d.2.get("design:location").is_some()).collect();
    assert_eq!(moved.len(), 1, "one of the pair moves: {built}");
    assert_eq!(built["boxClearance"]["puttyPads"], json!(0), "{built}");
    assert_eq!(pads_in_takeoff(&mut s), 0.0);
    let bx = boxes(&s);
    let (p, q) = (&bx[0], &bx[1]);
    assert!((p.1[1] - q.1[1]).abs() >= 28.0 - 1e-6, "24\" clear edge to edge: {:?} {:?}", p.1, q.1);
    let ends = stick_ends(&s);
    for (of, c, _) in &bx {
        assert!(conduit_ends_at(&ends, *c), "no conduit ends at the box of {of} at {c:?}: {ends:?}");
    }
    let notes = built.to_string();
    assert!(notes.contains("separated by distance") && notes.contains("putty pads"), "which was used, and the alternative: {notes}");
}

#[test]
fn switching_the_choice_replaces_only_the_affected_boxes() {
    // Separated first (the default), then the project chooses putty pads: the moved device goes
    // back to its drawn spot, its box and raceway with it; an unaffected device stays put.
    let (mut s, _) = mirrored_units((5.0, 5.0), None);
    let before = receptacles(&s);
    assert_eq!(before.iter().filter(|d| d.2.get("design:location").is_some()).count(), 1);
    s.execute(
        "project.rules.set",
        &json!({"domain": "clearance", "key": "rated_wall_boxes", "value": "putty pads", "tier": "rfi", "source": "RFI 12", "reason": "answered"}),
    )
    .expect("rule");
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let after = receptacles(&s);
    for (h, c, ps) in &after {
        assert!(ps.get("design:location").is_none(), "device {} still moved: {:?}", h.hex(), ps.get("correction:location").map(|p| p.text()));
        assert!((c[1] - 60.0).abs() < 1.0, "device {} at y {}", h.hex(), c[1]);
    }
    assert_eq!(pads_in_takeoff(&mut s), 2.0);
    let ends = stick_ends(&s);
    for (of, c, _) in &boxes(&s) {
        assert!((c[1] - 60.0).abs() < 3.0, "box of {of} at y {}", c[1]);
        assert!(conduit_ends_at(&ends, *c), "no conduit ends at the box of {of} at {c:?}: {ends:?}");
    }
    // And back: separation moves one again.
    s.execute(
        "project.rules.set",
        &json!({"domain": "clearance", "key": "rated_wall_boxes", "value": "separate", "tier": "rfi", "source": "RFI 13", "reason": "answered"}),
    )
    .expect("rule");
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    assert_eq!(receptacles(&s).iter().filter(|d| d.2.get("design:location").is_some()).count(), 1);
    assert_eq!(pads_in_takeoff(&mut s), 0.0);
}
