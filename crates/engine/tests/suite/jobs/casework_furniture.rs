//! Q6422: casework and furniture are model elements. `model.build` (the guided job's build)
//! reads the plan's casework with its schedule and the furniture the plans draw (a block named for
//! it, an outline on a furniture layer, an outline labelled with its name), builds each as an
//! element with its size, height, tag, source and room, and repeats a typical room's furniture into
//! the empty rooms of its type whose outline matches. Heights no document gives are generic, and
//! marked so. Both are their own trades in the BIM view, the takeoff and the AR export, never the
//! electrical or mechanical work. A made-up one-storey plan only.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Arc, Drawing, Entity, EntityKind, Handle, Line, LwPolyline};
use cadaddy_engine::geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

fn push(d: &mut Drawing, layer: &str, kind: EntityKind) -> Handle {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, kind);
    e.common.layer = layer.into();
    d.model.push(e);
    h
}

/// A line in feet.
fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    push(d, layer, EntityKind::Line(Line { a: p(a), b: p(b) }));
}

/// A closed rectangle, inches: its lower-left corner and size.
fn rect(d: &mut Drawing, layer: &str, x: f64, y: f64, w: f64, h: f64) -> Handle {
    let v = |x: f64, y: f64| PolyVertex { p: Vec2::new(x, y), bulge: 0.0, start_width: 0.0, end_width: 0.0 };
    push(
        d,
        layer,
        EntityKind::LwPolyline(LwPolyline {
            vertices: vec![v(x, y), v(x + w, y), v(x + w, y + h), v(x, y + h)],
            closed: true,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        }),
    )
}

/// A room's faces (feet), its door gap in the bottom side.
fn room(d: &mut Drawing, x0: f64, y0: f64, x1: f64, y1: f64, gap: (f64, f64)) {
    line(d, "A-WALL", (x0, y0), (gap.0, y0));
    line(d, "A-WALL", (gap.1, y0), (x1, y0));
    line(d, "A-WALL", (x0, y1), (x1, y1));
    line(d, "A-WALL", (x0, y0), (x0, y1));
    line(d, "A-WALL", (x1, y0), (x1, y1));
}

/// Three guest rooms over a corridor: 101 and 102 the same 14 x 20 ft, 103 a larger 20 x 20 ft;
/// a corridor below them.
fn plan() -> Drawing {
    let mut d = Drawing::new_imperial();
    // Outside faces.
    line(&mut d, "A-WALL", (-0.5, -8.5), (50.5, -8.5));
    line(&mut d, "A-WALL", (-0.5, 20.5), (50.5, 20.5));
    line(&mut d, "A-WALL", (-0.5, -8.5), (-0.5, 20.5));
    line(&mut d, "A-WALL", (50.5, -8.5), (50.5, 20.5));
    room(&mut d, 0.0, 0.0, 14.0, 20.0, (5.0, 8.0));
    room(&mut d, 14.5, 0.0, 28.5, 20.0, (19.5, 22.5));
    room(&mut d, 29.0, 0.0, 50.0, 20.0, (34.0, 37.0));
    // The corridor's faces: its top face under the rooms' walls, with their doors' gaps.
    line(&mut d, "A-WALL", (0.0, -8.0), (50.0, -8.0));
    line(&mut d, "A-WALL", (0.0, -8.0), (0.0, -0.5));
    line(&mut d, "A-WALL", (50.0, -8.0), (50.0, -0.5));
    let mut at = 0.0;
    for (a, b) in [(5.0, 8.0), (19.5, 22.5), (34.0, 37.0)] {
        line(&mut d, "A-WALL", (at, -0.5), (a, -0.5));
        door(&mut d, a, b);
        at = b;
    }
    line(&mut d, "A-WALL", (at, -0.5), (50.0, -0.5));
    d
}

/// A door in the rooms' bottom wall (faces at -0.5 and 0 ft) from `x0` to `x1` feet: jambs, the
/// leaf and its swing into the corridor.
fn door(d: &mut Drawing, x0: f64, x1: f64) {
    line(d, "A-WALL", (x0, -0.5), (x0, 0.0));
    line(d, "A-WALL", (x1, -0.5), (x1, 0.0));
    line(d, "A-DOOR", (x0, -0.5), (x0, -0.5 - (x1 - x0)));
    push(
        d,
        "A-DOOR",
        EntityKind::Arc(Arc {
            center: Vec3 { x: x0 * 12.0, y: -6.0, z: 0.0 },
            radius: (x1 - x0) * 12.0,
            start: 1.5 * std::f64::consts::PI,
            end: 0.0,
        }),
    );
}

fn text_on(s: &mut Session, layer: &str, at: [f64; 2], t: &str, height: f64) {
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": height})).expect("text");
}

fn spec() -> Value {
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [-120, -240, 720, 360]}],
        "layers": [
            {"layer": "A-WALL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-DOOR", "role": "door"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "openings": {"enabled": true, "min_ft": 1.5, "max_ft": 16.0, "door_head_ft": 7.0, "window_head_ft": 7.0, "window_sill_ft": 3.0}
    })
}

/// The plan with casework (a schedule naming the vanity, an upper cabinet's AFF note, a base
/// cabinet by its tag alone) and room 101's furniture: a bed block, a nightstand and an odd
/// piece on a furniture layer, a desk outline labelled DESK, a pillow inside the bed.
fn session(furnished: bool) -> Session {
    let mut s = Session::empty();
    let mut d = plan();
    // Casework in room 101 (inches).
    rect(&mut d, "CASEWORK", 12.0, 200.0, 48.0, 22.0); // V-1
    rect(&mut d, "CASEWORK", 100.0, 216.0, 36.0, 12.0); // UC-1
    rect(&mut d, "CASEWORK", 100.0, 180.0, 36.0, 24.0); // BC-1
    if furnished {
        // A furniture layer's pieces.
        rect(&mut d, "I-FURN", 130.0, 120.0, 24.0, 18.0); // a nightstand by its size
        rect(&mut d, "I-FURN", 12.0, 40.0, 37.0, 13.0); // no kind's size: furniture
        rect(&mut d, "I-FURN", 72.0, 130.0, 22.0, 16.0); // a pillow on the bed (inside its block)
        // A desk drawn on an annotation layer, labelled.
        rect(&mut d, "A-ANNO", 12.0, 100.0, 48.0, 24.0);
    }
    s.open_drawing(d, "Sample Building", None);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec()})).expect("spec");
    for (x, name, number) in [(7.0, "GUEST", "101"), (21.5, "GUEST", "102"), (39.5, "GUEST", "103"), (25.0, "CORRIDOR", "100")] {
        let y = if number == "100" { -4.0 } else { 10.0 };
        text_on(&mut s, "A-ROOM-TAG", [x * 12.0, (y + 1.0) * 12.0], name, 6.0);
        text_on(&mut s, "A-ROOM-TAG", [x * 12.0, (y - 1.0) * 12.0], number, 6.0);
    }
    // Casework tags (on the casework layer) and the upper cabinet's height note.
    text_on(&mut s, "CASEWORK", [30.0, 208.0], "V-1", 3.0);
    text_on(&mut s, "CASEWORK", [112.0, 220.0], "UC-1", 3.0);
    text_on(&mut s, "TEXT", [112.0, 232.0], "54\" AFF", 3.0);
    text_on(&mut s, "CASEWORK", [112.0, 190.0], "BC-1", 3.0);
    // The schedule, outside the rooms.
    for (y, row) in [(-60.0, ["TAG", "DESCRIPTION", "SIZE", "HEIGHT"]), (-66.0, ["V-1", "VANITY", "48\" x 22\"", "34\" AFF"])] {
        for (i, c) in row.iter().enumerate() {
            text_on(&mut s, "CASEWORK-SCHEDULE", [400.0 + 60.0 * i as f64, y], c, 3.0);
        }
    }
    if furnished {
        text_on(&mut s, "A-ANNO", [30.0, 110.0], "DESK", 3.0);
        // A label with no outline round it, and one round a room-sized outline: nothing is built.
        text_on(&mut s, "A-ANNO", [300.0, -40.0], "BED", 3.0);
        // A bed block.
        s.execute("layer.new", &json!({"name": "I-FURN-BLK", "current": true})).expect("layer");
        let h = s.execute("pline", &json!({"vertices": [[0, 0], [60, 0], [60, 80], [0, 80]], "closed": true})).unwrap()["handle"]
            .as_str()
            .unwrap()
            .to_string();
        s.execute("block", &json!({"name": "BED-60X80", "base": [0, 0], "handles": [h], "keep": "delete"})).expect("block");
        s.execute("insert", &json!({"name": "BED-60X80", "at": [60.0, 70.0]})).expect("bed");
    }
    s
}

/// The build's tags of an element (after the build's marker).
fn tags(e: &Entity) -> Vec<String> {
    let x = &e.common.xdata;
    let Some(at) = x.iter().position(|t| t.code == 1001 && t.value == "CADADDY_WALLGEN") else { return Vec::new() };
    x[at + 1..].iter().take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.clone()).collect()
}

fn tag<'a>(t: &'a [String], key: &str) -> Option<&'a str> {
    t.iter().find_map(|v| v.strip_prefix(&format!("{key}:")))
}

/// The built elements of a part: (handle, tags, z range, plan centre).
fn built(s: &Session, part: &str) -> Vec<(Handle, Vec<String>, [f64; 2], [f64; 2])> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter_map(|e| {
            let t = tags(e);
            if t.first().map(String::as_str) != Some(part) {
                return None;
            }
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let z0 = m.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
            let z1 = m.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
            let n = m.vertices.len().max(1) as f64;
            let c = [m.vertices.iter().map(|v| v.x).sum::<f64>() / n, m.vertices.iter().map(|v| v.y).sum::<f64>() / n];
            Some((e.handle, t, [z0, z1], c))
        })
        .collect()
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-cwfu-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_build_makes_casework_and_furniture_elements_and_repeats_a_typical_room() {
    let _g = serial();
    let mut s = session(true);
    let r = s.execute("model.build", &json!({})).expect("build");
    println!("CASEWORK {}", r["casework"]);
    println!("FURNITURE {}", r["furniture"]);
    assert!(r["rooms"].as_u64().unwrap_or(0) >= 3, "rooms: {}", r["rooms"]);

    // Casework: three items, each with its kind, tag, height and room; the base cabinet's height
    // (no schedule row, no note) is generic.
    let cw = built(&s, "casework");
    assert_eq!(cw.len(), 3, "{cw:?}");
    assert_eq!(r["casework"]["built"], json!(3), "{}", r["casework"]);
    assert_eq!(r["casework"]["generic"], json!(1), "{}", r["casework"]);
    let by = |t: &str| cw.iter().find(|c| tag(&c.1, "tag") == Some(t)).unwrap_or_else(|| panic!("{t} in {cw:?}"));
    let v = by("V-1");
    assert_eq!(tag(&v.1, "kind"), Some("vanity"));
    assert!((v.2[0] - 0.0).abs() < 1e-6 && (v.2[1] - 34.0).abs() < 1e-6, "vanity to its scheduled 34\": {:?}", v.2);
    assert!(tag(&v.1, "room").is_some_and(|r| r.contains("101")), "{:?}", v.1);
    assert!(tag(&v.1, "generic").is_none());
    let uc = by("UC-1");
    assert_eq!(tag(&uc.1, "kind"), Some("upper_cabinet"));
    assert!((uc.2[0] - 54.0).abs() < 1e-6 && uc.2[1] > 54.0, "upper cabinet hung at its note's 54\": {:?}", uc.2);
    let bc = by("BC-1");
    assert_eq!(tag(&bc.1, "kind"), Some("base_cabinet"));
    assert_eq!(tag(&bc.1, "generic"), Some("height"));
    assert!((bc.2[1] - 34.5).abs() < 1e-6, "{:?}", bc.2);
    for c in &cw {
        assert_eq!(tag(&c.1, "status"), Some("drawn"), "{:?}", c.1);
    }
    // Its source: the casework outline it was read from.
    let d = s.doc().unwrap();
    let src = cadaddy_engine::doc::Handle(u64::from_str_radix(&v.1[2], 16).unwrap());
    assert_eq!(d.model.get(src).map(|e| e.common.layer.to_string()), Some("CASEWORK".into()));

    // Furniture: four drawn in 101 (the bed block, nightstand by size, desk by its label, an odd
    // piece on the furniture layer), never the pillow inside the bed nor a label alone; repeated
    // into 102, whose outline matches; 103 (another size) stays empty.
    let fu = built(&s, "furniture");
    let kinds = |room: &str| -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for f in fu.iter().filter(|f| tag(&f.1, "room").is_some_and(|r| r.contains(room))) {
            *m.entry(tag(&f.1, "kind").unwrap_or_default().to_string()).or_default() += 1;
        }
        m
    };
    let want: BTreeMap<String, usize> =
        [("bed", 1), ("desk", 1), ("furniture", 1), ("nightstand", 1)].into_iter().map(|(k, n)| (k.to_string(), n)).collect();
    assert_eq!(kinds("101"), want, "{fu:?}");
    assert_eq!(kinds("102"), want, "the typical room repeated: {fu:?}");
    assert!(kinds("103").is_empty(), "{fu:?}");
    assert_eq!(fu.len(), 8, "{fu:?}");
    assert_eq!(r["furniture"]["read"], json!(4), "{}", r["furniture"]);
    assert_eq!(r["furniture"]["repeated"], json!(4), "{}", r["furniture"]);
    assert_eq!(r["furniture"]["generic"], json!(8), "every height is generic: {}", r["furniture"]);
    assert!(r["furniture"]["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap_or_default().contains("type GUEST")), "{}", r["furniture"]);
    for f in &fu {
        let in_101 = tag(&f.1, "room").is_some_and(|r| r.contains("101"));
        assert_eq!(tag(&f.1, "status"), Some(if in_101 { "drawn" } else { "derived" }), "{:?}", f.1);
        if !in_101 {
            assert!(tag(&f.1, "basis").is_some_and(|b| b.contains("101")), "its basis names the room it repeats: {:?}", f.1);
        }
        assert_eq!(tag(&f.1, "generic"), Some("height"));
    }
    let bed = fu.iter().find(|f| tag(&f.1, "kind") == Some("bed") && tag(&f.1, "room").is_some_and(|r| r.contains("101"))).unwrap();
    assert!((bed.2[1] - 24.0).abs() < 1e-6, "the bed's generic height: {:?}", bed.2);
    assert!((bed.3[0] - 90.0).abs() < 0.5 && (bed.3[1] - 110.0).abs() < 0.5, "at the block's footprint: {:?}", bed.3);
    let bed2 = fu.iter().find(|f| tag(&f.1, "kind") == Some("bed") && tag(&f.1, "room").is_some_and(|r| r.contains("102"))).unwrap();
    assert!((bed2.3[0] - (90.0 + 174.0)).abs() < 0.5 && (bed2.3[1] - 110.0).abs() < 0.5, "moved to room 102: {:?}", bed2.3);

    // Objects with properties: kind, size, height (its source the rules), room.
    let p = s.execute("bim.props.get", &json!({"handle": bed.0.hex()})).expect("props");
    let props = &p["elements"][0]["props"];
    assert_eq!(p["elements"][0]["kind"], json!("furniture"), "{p}");
    assert_eq!(props["width"]["value"], json!(80.0), "{p}");
    assert_eq!(props["depth"]["value"], json!(60.0), "{p}");
    assert_eq!(props["height"]["source"]["kind"], json!("rule"), "{p}");
    assert_eq!(props["generic"]["value"], json!(true), "{p}");
    let pv = s.execute("bim.props.get", &json!({"handle": v.0.hex()})).expect("props");
    assert_eq!(pv["elements"][0]["props"]["height"]["source"]["kind"], json!("schedule"), "{pv}");
    assert_eq!(pv["elements"][0]["props"]["type_mark"]["value"], json!("V-1"), "{pv}");

    // The BIM view: each its own trade, a switch of its own.
    let t = s.execute("bim.trades", &json!({})).expect("trades");
    let row = |k: &str| t["trades"].as_array().unwrap().iter().find(|r| r["trade"] == json!(k)).cloned().unwrap_or_else(|| panic!("{k}: {t}"));
    assert_eq!(row("cabinets")["count"], json!(3), "{t}");
    assert_eq!(row("furniture")["count"], json!(8), "{t}");
    let off = s.execute("bim.trades", &json!({"visible": {"furniture": false}})).expect("hide");
    let fr = off["trades"].as_array().unwrap().iter().find(|r| r["trade"] == json!("furniture")).unwrap().clone();
    assert_eq!(fr["visible"], json!(false), "{off}");
    assert_eq!(off["trades"].as_array().unwrap().iter().find(|r| r["trade"] == json!("cabinets")).unwrap()["visible"], json!(true));
    s.execute("bim.trades", &json!({"visible": {"furniture": true}})).expect("show");

    // The takeoff: lines of their own trades, never electrical or mechanical.
    let tk = s.execute("bim.takeoff", &json!({"keep": false})).expect("takeoff");
    let lines = tk["lines"].as_array().unwrap();
    let qty = |trade: &str| lines.iter().filter(|l| l["trade"] == json!(trade)).map(|l| l["qty"].as_f64().unwrap_or(0.0)).sum::<f64>();
    assert_eq!(qty("cabinets"), 3.0, "{tk}");
    assert_eq!(qty("furniture"), 8.0, "{tk}");
    let fur: Vec<String> = fu.iter().map(|f| f.0.hex()).collect();
    for l in lines.iter().filter(|l| l["trade"] != json!("furniture")) {
        let els: Vec<&str> = l["elements"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        assert!(!els.iter().any(|h| fur.iter().any(|f| f == h)), "furniture on another trade's line: {l}");
    }
    assert!(lines.iter().any(|l| l["trade"] == json!("furniture") && l["made"] == json!("derived")), "{tk}");

    // The AR export: furniture in room packages only, off; cabinets in level and room packages.
    let out = dir("ar");
    s.execute("bim.export.ar", &json!({"set": true, "dir": out.to_string_lossy()})).expect("ar set");
    let index: Value = serde_json::from_slice(&std::fs::read(out.join("building.json")).unwrap()).unwrap();
    let graph = index["graph"]["elements"].as_array().unwrap();
    let pkgs_of = |h: &str| -> Vec<String> {
        graph
            .iter()
            .find(|g| g["id"] == json!(h))
            .map(|g| g["packages"].as_array().unwrap().iter().filter_map(|p| p.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    for f in &fu {
        let p = pkgs_of(&f.0.hex());
        assert!(!p.is_empty() && p.iter().all(|x| x.starts_with("rooms/")), "furniture only in rooms: {p:?}");
        let side: Value = serde_json::from_slice(&std::fs::read(out.join(format!("{}.json", p[0]))).unwrap()).unwrap();
        let n = side["elements"].as_array().unwrap().iter().find(|e| e["id"] == json!(f.0.hex())).unwrap().clone();
        assert_eq!(n["group"], json!("furniture"), "{n}");
        assert_eq!(n["default_on"], json!(false), "{n}");
    }
    for c in &cw {
        let p = pkgs_of(&c.0.hex());
        assert!(p.iter().any(|x| x.starts_with("levels/")) && p.iter().all(|x| !x.starts_with("building-")), "{p:?}");
    }
    let _ = std::fs::remove_dir_all(&out);

    // Built again, the same elements, not twice as many.
    s.execute("model.build", &json!({})).expect("again");
    assert_eq!(built(&s, "furniture").len(), 8);
    assert_eq!(built(&s, "casework").len(), 3);
}

#[test]
fn nothing_is_furnished_where_nothing_is_drawn() {
    let _g = serial();
    let mut s = session(false);
    let r = s.execute("model.build", &json!({})).expect("build");
    assert!(built(&s, "furniture").is_empty(), "{}", r["furniture"]);
    assert_eq!(r["furniture"]["built"], json!(0));
    assert_eq!(built(&s, "casework").len(), 3, "{}", r["casework"]);
}

/// A plan drawn at a quarter inch to the foot (the sheet's units are not the building's inches):
/// the same casework and furniture, the same sizes in inches.
#[test]
fn a_plan_drawn_to_scale_reads_the_same() {
    let _g = serial();
    let mut s = session(true);
    let all: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    s.execute("scale", &json!({"handles": all, "base": [0, 0], "factor": 1.0 / 48.0})).expect("scale");
    let mut sp = spec();
    sp["units_per_foot"] = json!(0.25);
    sp["levels"][0]["region"] = json!([-2.5, -5.0, 15.0, 7.5]);
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    let r = s.execute("model.build", &json!({})).expect("build");
    assert_eq!(r["casework"]["built"], json!(3), "{}", r["casework"]);
    assert_eq!(r["furniture"]["read"], json!(4), "{}", r["furniture"]);
    assert_eq!(r["furniture"]["built"], json!(8), "{}", r["furniture"]);
    let bed = built(&s, "furniture").into_iter().find(|f| tag(&f.1, "kind") == Some("bed")).unwrap();
    assert_eq!(tag(&bed.1, "size"), Some("80x60"), "{:?}", bed.1);
    assert!((bed.2[1] - 24.0 / 48.0).abs() < 1e-6, "its 24\" height at the sheet's scale: {:?}", bed.2);
    let v = built(&s, "casework").into_iter().find(|c| tag(&c.1, "tag") == Some("V-1")).unwrap();
    assert_eq!(tag(&v.1, "size"), Some("48x22"), "{:?}", v.1);
}

/// Room 101's furniture drawn only on an enlarged plan of it at twice the floor plan's scale, off
/// the level's plan, the sheet registered onto the level (by the door and room numbers they share):
/// built in room 101 at the floor plan's size, and repeated by type into 102.
#[test]
fn an_enlarged_plan_registered_onto_its_level_furnishes_its_room_and_its_type() {
    let _g = serial();
    let mut s = session(false);
    // Sheet point = 2 x plan point + (2000, 0).
    let e = |x: f64, y: f64| [2000.0 + 2.0 * x, 2.0 * y];
    {
        let d = s.doc_mut().unwrap();
        for (x, y, w, h) in [(130.0, 120.0, 24.0, 18.0), (12.0, 40.0, 37.0, 13.0)] {
            let p = e(x, y);
            rect(d, "I-FURN", p[0], p[1], w * 2.0, h * 2.0);
        }
        let p = e(12.0, 100.0);
        rect(d, "A-ANNO", p[0], p[1], 96.0, 48.0);
    }
    text_on(&mut s, "A-ANNO", e(30.0, 110.0), "DESK", 6.0);
    s.execute("layer.new", &json!({"name": "I-FURN-BLK", "current": true})).expect("layer");
    let h = s.execute("pline", &json!({"vertices": [[0, 0], [60, 0], [60, 80], [0, 80]], "closed": true})).unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_string();
    s.execute("block", &json!({"name": "BED-60X80", "base": [0, 0], "handles": [h], "keep": "delete"})).expect("block");
    s.execute("insert", &json!({"name": "BED-60X80", "at": e(60.0, 70.0), "scale": 2.0})).expect("bed");
    let mut sp = spec();
    sp["sheets"] = json!([{
        "number": "A-501", "title": "ENLARGED GUEST ROOM PLAN", "level": "L1", "rect": [1900, -100, 2600, 600],
        "registration": {"reference": "A-101", "method": "tags", "a": 0.5, "b": 0.0, "tx": -1000.0, "ty": 0.0, "matched": 4}
    }]);
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).expect("spec");
    let r = s.execute("model.build", &json!({})).expect("build");
    println!("FURNITURE {}", r["furniture"]);
    assert_eq!(r["furniture"]["read"], json!(4), "{}", r["furniture"]);
    assert_eq!(r["furniture"]["built"], json!(8), "{}", r["furniture"]);
    let fu = built(&s, "furniture");
    let bed = fu.iter().find(|f| tag(&f.1, "kind") == Some("bed") && tag(&f.1, "room").is_some_and(|r| r.contains("101"))).unwrap();
    assert!((bed.3[0] - 90.0).abs() < 0.5 && (bed.3[1] - 110.0).abs() < 0.5, "on the floor plan at its room: {:?}", bed.3);
    assert_eq!(tag(&bed.1, "size"), Some("80x60"), "{:?}", bed.1);
    assert_eq!(tag(&bed.1, "found"), Some("block on A-501"), "{:?}", bed.1);
    assert_eq!(tag(&bed.1, "status"), Some("drawn"));
    assert_eq!(fu.iter().filter(|f| tag(&f.1, "room").is_some_and(|r| r.contains("102"))).count(), 4, "{fu:?}");
}
