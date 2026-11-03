//! Trade atoms (Q6162): a made-up two-storey building's walls, slab, floors, roof, rooms and
//! doors built as their trades' pieces, counted from the index, laid out from synthetic trade
//! documents, scored to the numbers the geometry gives.

use cadaddy_engine::Session;
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

fn text(s: &mut Session, at: [f64; 2], value: &str) {
    s.execute("text", &json!({"at": at, "text": value, "height": 6.0})).unwrap();
}

const T: f64 = 7.25;
const HALF: f64 = 4.875 / 2.0;

/// A 40' x 24' storey at `ox`: exterior walls, a partition at 20' (with a 3' gap from 100" to
/// 136" on level 1: a door), two numbered rooms.
fn storey(s: &mut Session, ox: f64, door: bool, n: u32) {
    layer(s, "A-WALL");
    rect(s, [ox, 0.0], [ox + 480.0, 288.0]);
    rect(s, [ox + T, T], [ox + 480.0 - T, 288.0 - T]);
    let x = ox + 240.0;
    for dx in [-HALF, HALF] {
        if door {
            line(s, [x + dx, T], [x + dx, 100.0]);
            line(s, [x + dx, 136.0], [x + dx, 288.0 - T]);
            if dx > 0.0 {
                // Its swing: hinged at the gap's start, a 3' leaf.
                s.execute("arc", &json!({"center": [x, 100.0], "radius": 36.0, "start": 0.0, "end": 90.0})).unwrap();
            }
        } else {
            line(s, [x + dx, T], [x + dx, 288.0 - T]);
        }
    }
    layer(s, "A-ROOM-NAME");
    text(s, [ox + 120.0, 144.0], &format!("{}01", n));
    text(s, [ox + 360.0, 144.0], &format!("{}02", n));
}

fn layers(side1: Value, core: Value, side2: Value) -> Value {
    let mut v: Vec<Value> = Vec::new();
    for (side, list) in [("side1", side1), ("core", core), ("side2", side2)] {
        for l in list.as_array().cloned().unwrap_or_default() {
            let mut l = l;
            l["side"] = json!(side);
            v.push(l);
        }
    }
    json!(v)
}

fn building() -> Session {
    let mut s = Session::new();
    storey(&mut s, 0.0, true, 1);
    storey(&mut s, 2000.0, false, 2);
    let exterior = json!({
        "tag": "X1", "description": "EXTERIOR WALL", "external": true, "source": "sample legend, mark X1",
        "layers": layers(
            json!([{"material": "1/2\" wood structural panel sheathing", "thickness_in": 0.5, "category": "sheathing"}]),
            json!([{"material": "2x6 wood studs @ 24\" o.c.", "thickness_in": 5.5, "category": "framing"},
                   {"material": "R-21 batt insulation", "thickness_in": 5.5, "category": "insulation", "in_cavity": true}]),
            json!([{"material": "1/2\" gypsum board", "thickness_in": 0.5, "category": "board"}]))
    });
    let partition = json!({
        "tag": "P1", "description": "RATED PARTITION", "fire_rating": "1 HR", "source": "sample legend, mark P1",
        "layers": layers(
            json!([{"material": "5/8\" Type C gypsum board", "thickness_in": 0.625, "category": "board"}]),
            json!([{"material": "3 5/8\" 20 ga metal studs", "thickness_in": 3.625, "category": "framing", "spacing_in": 16.0},
                   {"material": "sound batt insulation", "thickness_in": 3.5, "category": "insulation", "in_cavity": true}]),
            json!([{"material": "5/8\" Type C gypsum board", "thickness_in": 0.625, "category": "board"}]))
    });
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [
            {"name": "1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000]},
            {"name": "2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "region": [1900, -100, 3000, 1000]}
        ],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [
            {"name": "X1", "thickness_in": 7.25, "from": "pairs", "assembly": exterior},
            {"name": "P1", "thickness_in": 4.875, "from": "pairs", "assembly": partition}
        ],
        "walls": [
            {"level": "1", "a": [240.0, T], "b": [240.0, 288.0 - T], "wall_type": "P1"},
            {"level": "2", "a": [2240.0, T], "b": [2240.0, 288.0 - T], "wall_type": "P1"}
        ],
        "rooms": [
            {"name": "OFFICE", "number": "101", "level": "1", "materials": {"floor": "LVT-1"}},
            {"name": "KING", "number": "102", "level": "1", "materials": {"floor": "LVT-1"}},
            {"name": "OFFICE", "number": "201", "level": "2", "materials": {"floor": "LVT-1"}},
            {"name": "KING", "number": "202", "level": "2", "materials": {"floor": "LVT-1"}}
        ],
        "unit_instances": [
            {"number": "201", "level": "2", "type": "KING-A"},
            {"number": "202", "level": "2", "type": "KING-ADA"}
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert!(r["tradePieces"].as_u64().unwrap_or(0) > 0, "the build lays out trade pieces: {r}");
    s
}

fn count(s: &mut Session, q: Value) -> Value {
    s.execute("bim.count", &q).unwrap()
}

fn n(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}

/// The built element extents of a part (and type tag) on a level: (dx, dy, dz) in feet.
fn extents(s: &Session, part: &str, ty: Option<&str>, level: &str) -> Vec<[f64; 3]> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some(part))
        .filter(|e| {
            let tags: Vec<String> = e.common.xdata.iter().map(|t| t.value.to_string()).collect();
            tags.get(2).is_some_and(|l| l == level) && ty.is_none_or(|t| tags.iter().any(|x| x == &format!("type:{t}")))
        })
        .filter_map(|e| {
            let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { return None };
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for v in &m.vertices {
                for (i, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                    lo[i] = lo[i].min(c);
                    hi[i] = hi[i].max(c);
                }
            }
            Some([(hi[0] - lo[0]) / 12.0, (hi[1] - lo[1]) / 12.0, (hi[2] - lo[2]) / 12.0])
        })
        .collect()
}

#[test]
fn the_build_lays_out_every_trade_and_the_index_counts_it() {
    let mut s = building();
    // Every count is a lookup that equals a full recount.
    let all = count(&mut s, json!({"group_by": "trade", "verify": true}));
    assert_eq!(all["from"], json!("index"));
    assert_eq!(all["verify"]["matches"], json!(true), "{all}");
    let trades: Vec<&str> = all["rows"].as_array().unwrap().iter().filter_map(|r| r["trade"].as_str()).collect();
    for t in ["framing", "drywall", "insulation", "finishes", "concrete", "openings"] {
        assert!(trades.contains(&t), "trade {t} counted: {trades:?}");
    }
    // The partition on level 2 (no door): its Type C board covers both faces whole.
    let p2 = extents(&s, "wall", Some("P1"), "2");
    assert_eq!(p2.len(), 1, "one P1 wall on level 2: {p2:?}");
    let [dx, dy, dz] = p2[0];
    let face = dx.max(dy) * dz;
    let c2 = count(&mut s, json!({"trade": "drywall", "kind": "sheet", "type": "Type C", "level": "2"}));
    assert!((n(&c2, "area_sf") - 2.0 * face).abs() < 0.05, "Type C on level 2: {c2} vs 2 x {face}");
    // Its sheets: 4' columns of one 10' sheet (the wall is 10' high) on each face.
    let cols = (dx.max(dy) / 4.0).ceil();
    assert_eq!(n(&c2, "count"), 2.0 * cols, "{c2}");
    // On level 1 the door takes its opening out of both faces.
    let c1 = count(&mut s, json!({"trade": "drywall", "kind": "sheet", "type": "Type C", "level": "1"}));
    let doors = extents(&s, "door", None, "1");
    assert_eq!(doors.len(), 1, "the gap is a door: {doors:?}");
    // Level 1's partition is two walls either side of the door (one with the header over it).
    let p1 = extents(&s, "wall", Some("P1"), "1");
    let face1: f64 = p1.iter().map(|e| e[0].max(e[1])).sum::<f64>() * p1[0][2];
    let opening = 3.0 * 7.0;
    assert!((n(&c1, "area_sf") - 2.0 * (face1 - opening)).abs() < 0.6, "Type C on level 1: {c1} vs 2 x ({face1} - {opening})");
    // Studs: the partition's at 16" (steel), the exterior's at 24" (wood, from its text).
    let steel = count(&mut s, json!({"trade": "framing", "kind": "studs", "type": "steel 20 ga", "level": "2"}));
    let len2 = dx.max(dy) * 12.0;
    let layout = ((len2 - 1.25 - 0.625) / 16.0).floor() + 1.0 + 1.0;
    assert_eq!(n(&steel, "count"), layout, "{steel} for a {len2}\" wall");
    let wood = count(&mut s, json!({"trade": "framing", "kind": "stud", "type": "wood 2x6", "group_by": "item"}));
    assert_eq!(wood["rows"][0]["item"], json!("2x6 wood stud"), "{wood}");
    // A wood wall's double top plate and single bottom plate; a steel wall's tracks.
    assert!(n(&count(&mut s, json!({"trade": "framing", "kind": "plate"})), "count") > 0.0);
    let tracks = count(&mut s, json!({"trade": "framing", "kind": "track", "level": "2"}));
    assert!((n(&tracks, "length_ft") - 2.0 * dx.max(dy)).abs() < 0.01, "{tracks}");
    // One header over the door (steel track header) on level 1, none on level 2.
    assert_eq!(n(&count(&mut s, json!({"trade": "framing", "kind": "header", "level": "1", "type": "steel"})), "count"), 1.0);
    assert_eq!(n(&count(&mut s, json!({"trade": "framing", "kind": "header", "level": "2", "type": "steel"})), "count"), 0.0);
    // The slab on grade under level 1: its volume is its area by the default 4".
    let pour = count(&mut s, json!({"trade": "concrete", "kind": "pour"}));
    assert_eq!(n(&pour, "count"), 1.0, "{pour}");
    assert!((n(&pour, "volume_cf") - n(&pour, "area_sf") * 4.0 / 12.0).abs() < 1e-6, "{pour}");
    assert!((n(&pour, "area_sf") - 960.0).abs() < 10.0, "{pour}");
    assert!((n(&pour, "volume_cy") - n(&pour, "volume_cf") / 27.0).abs() < 0.01);
    // Its vapor barrier covers it (rule: 10 mil, 12' rolls lapped 6").
    let vb = count(&mut s, json!({"trade": "concrete", "kind": "vapor_barrier"}));
    assert!(n(&vb, "area_sf") >= n(&pour, "area_sf") - 1.0, "{vb} {pour}");
    // No documents: no upper floor framing, no roof framing, no PT; the report says so.
    assert_eq!(n(&count(&mut s, json!({"trade": "framing", "kind": "joist"})), "count"), 0.0);
    assert_eq!(n(&count(&mut s, json!({"trade": "concrete", "kind": "tendon"})), "count"), 0.0);
    // The rooms' flooring by their finish, paint on the faces that face them.
    let floor = count(&mut s, json!({"trade": "finishes", "kind": "flooring", "group_by": "item"}));
    assert!(floor["rows"].as_array().unwrap().iter().any(|r| r["item"] == json!("flooring LVT-1")), "{floor}");
    let paint = count(&mut s, json!({"trade": "finishes", "kind": "paint", "level": "2", "group_by": "type"}));
    assert!(n(&paint, "area_sf") > 0.0, "{paint}");
    // Doors and windows are counted as themselves; each door has a frame and a hardware set.
    let d = count(&mut s, json!({"trade": "openings", "kind": "door"}));
    let frames = count(&mut s, json!({"trade": "openings", "kind": "frame"}));
    let sets = count(&mut s, json!({"trade": "openings", "kind": "hardware_set"}));
    assert_eq!(n(&d, "count"), n(&frames, "count"));
    assert_eq!(n(&d, "count"), n(&sets, "count"));
    // Pieces are clickable with their properties and source.
    let h = {
        let doc = s.doc().unwrap();
        doc.model
            .iter()
            .find(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("atom") && e.common.xdata.iter().any(|t| t.value == "part:atom:stud"))
            .map(|e| e.handle.hex())
            .unwrap()
    };
    let el = s.execute("bim.element", &json!({"handle": h})).unwrap();
    assert_eq!(el["piece"]["kind"], json!("stud"), "{el}");
    assert!(el["piece"]["connects"].as_array().is_some_and(|c| !c.is_empty()), "a stud stands on its plate or track: {el}");
    // The pieces' layers start off: the shell stays the view.
    let off = s.doc().unwrap().layers.iter().filter(|l| l.name.starts_with("3D-Atoms-")).all(|l| !l.on);
    assert!(off);
    // The pieces are never clashed (they are the walls' insides).
    let clash = s.execute("bim.clash", &json!({"a": "architecture", "b": "structure"})).unwrap();
    assert!(clash.to_string().len() < 1_000_000);
}

/// The synthetic trade documents of a made-up job, in both formats the readers know.
fn docs_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-tradeatoms-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let files = [
        ("Sample Building Concrete Mix.txt", "--- page 1 of 1 ---\nCONCRETE MIX DESIGN\nMIX NUMBER:\n3500 - 5SK 3500PSI - Slab on Grade\n3500 psi at 28 days\n"),
        (
            "Sample PT Slab Design.txt",
            "--- page 1 of 2 ---\nPOST-TENSIONING STEEL SHALL BE 1/2\" DIAMETER, SEVEN WIRE, 270 KSI STRAND\n--- page 2 of 2 ---\nTENDONS @ 6'-0\" O.C. EACH WAY\nTENDON SUPPORT CHAIRS @ 3'-0\" O.C.\n5\" SLAB ON 15 MIL VAPOR RETARDER\n",
        ),
        ("Sample I-Joist Layout.txt", "J1 - 12 @ 16\"\nJ1 - 8 @ 16\"\n11 7/8\" TJI\n"),
        ("Sample Roof Trusses.txt", "Truss: T1\nQty: 6\nSpacing: 24.0 \"\nTruss: T2\nQty: 4\nSpacing: 24.0 \"\n"),
        ("Sample Doors Frames and Hardware.txt", "Hardware Set#: 1\n1 Single\nSize:\n3-0 X 7-0 X 1 3/4\nWood Door X HM Frame\nTot Qty\n3\n"),
        ("Sample Guest Toilet Accessories.txt", "Robe Hook\nTA-1:  RH100   QTY-2\n1 EACH - TA-2: GB36\n"),
        ("Sample Interior Painting.txt", "Interior Primer: latex\nAll areas to receive two (2) coats of latex\n"),
        ("Sample Flooring.txt", "LUXURY VINYL PLANK LVT-1 6\" X 36\"\n"),
        ("Sample Electrical Gear.txt", "PANEL LP1 225A\n"),
    ];
    for (name, body) in files {
        std::fs::write(dir.join(name), body).unwrap();
    }
    dir
}

#[test]
fn the_documents_set_the_pieces_and_their_quantities_are_compared() {
    let mut s = building();
    let dir = docs_dir();
    let r = s.execute("bim.atoms.docs", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let facts = r["facts"].as_array().unwrap();
    let has = |k: &str| facts.iter().any(|f| f["key"] == json!(k));
    for k in [
        "concrete.strength_psi",
        "concrete.pt",
        "concrete.strand_in",
        "concrete.tendon_spacing_ft",
        "concrete.chair_spacing_ft",
        "concrete.slab_thickness_in",
        "concrete.vapor_mil",
        "framing.floor_framing",
        "framing.joists_listed",
        "framing.joist_spacing_in",
        "framing.roof_framing",
        "framing.trusses_listed",
        "framing.truss_spacing_in",
        "openings.hardware_set",
        "specialties.accessory",
        "finishes.paint_coats",
        "finishes.flooring_piece_sf",
    ] {
        assert!(has(k), "fact {k} read: {r}");
    }
    // The electrical file is not a trade document here.
    assert!(r["files"].as_array().unwrap().iter().all(|f| !f["file"].as_str().unwrap_or("").contains("Electrical")));
    // Each fact cites its file and page.
    let strand = facts.iter().find(|f| f["key"] == json!("concrete.strand_in")).unwrap();
    assert!(strand["source"].as_str().unwrap().contains("Sample PT Slab Design.txt p.1"), "{strand}");
    let b = s.execute("bim.atoms.build", &json!({})).unwrap();
    assert!(b["pieces"].as_u64().unwrap() > 0, "{b}");
    // The PT slab: 5", tendons 6' o.c. each way 1' in from the edges over 40' x 24': 4 the long
    // way (38') and 7 the short way (22'), an anchor at each end, chairs at 3'.
    let pour = count(&mut s, json!({"trade": "concrete", "kind": "pour"}));
    assert!((n(&pour, "volume_cf") - n(&pour, "area_sf") * 5.0 / 12.0).abs() < 1e-6, "{pour}");
    let tendons = count(&mut s, json!({"trade": "concrete", "kind": "tendon"}));
    assert_eq!(n(&tendons, "count"), 11.0, "{tendons}");
    assert!((n(&tendons, "length_ft") - (4.0 * 38.0 + 7.0 * 22.0)).abs() < 0.5, "{tendons}");
    assert!((n(&tendons, "weight_lb") - n(&tendons, "length_ft") * 0.52).abs() < 0.01);
    assert_eq!(n(&count(&mut s, json!({"trade": "concrete", "kind": "anchor"})), "count"), 22.0);
    let chairs = count(&mut s, json!({"trade": "concrete", "kind": "chair"}));
    assert_eq!(n(&chairs, "pieces"), 4.0 * (38.0f64 / 3.0).ceil() + 7.0 * (22.0f64 / 3.0).ceil(), "{chairs}");
    let vb = count(&mut s, json!({"trade": "concrete", "kind": "vapor_barrier", "group_by": "size"}));
    assert_eq!(vb["rows"][0]["size"], json!("15 mil"), "{vb}");
    // The mix on the pour, from the mix submittal.
    let h = {
        let doc = s.doc().unwrap();
        doc.model.iter().find(|e| e.common.xdata.iter().any(|t| t.value == "part:atom:pour")).map(|e| e.handle.hex()).unwrap()
    };
    let el = s.execute("bim.element", &json!({"handle": h})).unwrap();
    assert!(el.to_string().contains("3500 psi"), "{el}");
    // Level 2's floor framed with I-joists at the document's 16" across its 24' span: 30 joists.
    let joists = count(&mut s, json!({"trade": "framing", "kind": "joist", "level": "2"}));
    assert_eq!(n(&joists, "count"), 30.0, "{joists}");
    assert!((n(&joists, "length_ft") - 30.0 * 24.0).abs() < 0.5, "{joists}");
    assert_eq!(n(&count(&mut s, json!({"trade": "framing", "kind": "rim", "level": "2"})), "count"), 4.0);
    // Trusses at 24" over the top storey: 20; roof and subfloor panels 4x8: 30 each.
    let trusses = count(&mut s, json!({"trade": "framing", "kind": "truss"}));
    assert_eq!(n(&trusses, "count"), 20.0, "{trusses}");
    for item in ["subfloor", "roof sheathing"] {
        let panels = count(&mut s, json!({"trade": "framing", "kind": "sheathing", "item": item}));
        assert_eq!(n(&panels, "count"), 30.0, "{item}: {panels}");
        assert!((n(&panels, "area_sf") - 960.0).abs() < 0.01, "{item}: {panels}");
    }
    // The door takes hardware set 1 (its size) and the set's frame.
    let set = count(&mut s, json!({"trade": "openings", "kind": "hardware_set", "group_by": "item"}));
    assert_eq!(set["rows"][0]["item"], json!("door hardware set 1"), "{set}");
    let frame = count(&mut s, json!({"trade": "openings", "kind": "frame", "group_by": "item"}));
    assert_eq!(frame["rows"][0]["item"], json!("hm frame"), "{frame}");
    // Accessories in the unit rooms (level 2's two): TA-1 one in each, TA-2's one in the
    // accessible unit.
    let ta1 = count(&mut s, json!({"trade": "specialties", "kind": "accessory", "item": "TA-1", "group_by": "room"}));
    assert_eq!(n(&ta1, "count"), 2.0, "{ta1}");
    assert_eq!(ta1["rows"].as_array().unwrap().len(), 2, "one in each unit room: {ta1}");
    let ta2 = count(&mut s, json!({"trade": "specialties", "kind": "accessory", "item": "TA-2", "group_by": "room"}));
    assert_eq!(ta2["rows"].as_array().unwrap().len(), 1);
    assert_eq!(ta2["rows"][0]["room"], json!("KING 202"), "the accessible unit: {ta2}");
    // Paint: a primer and two finish coats; flooring pieces by the 6" x 36" plank.
    let coats = count(&mut s, json!({"trade": "finishes", "kind": "paint", "level": "2", "group_by": "type"}));
    let primer = coats["rows"].as_array().unwrap().iter().find(|r| r["type"] == json!("primer")).map(|r| n(r, "area_sf")).unwrap();
    let finish = coats["rows"].as_array().unwrap().iter().find(|r| r["type"] == json!("finish coat")).map(|r| n(r, "area_sf")).unwrap();
    assert!((finish - 2.0 * primer).abs() < 0.01, "{coats}");
    let fl = count(&mut s, json!({"trade": "finishes", "kind": "flooring", "level": "1"}));
    assert!(n(&fl, "pieces") >= n(&fl, "area_sf") / 1.5, "{fl}");
    // The report: per level and trade, and the documents' quantities beside the model's.
    let rep = s.execute("bim.atoms.report", &json!({})).unwrap();
    let cmp = rep["compare"].as_array().unwrap();
    let row = |what: &str| cmp.iter().find(|c| c["what"] == json!(what)).cloned().unwrap_or(Value::Null);
    assert_eq!(row("floor joists")["documents"], json!(20.0), "{rep}");
    assert_eq!(row("floor joists")["model"], json!(30.0));
    assert_eq!(row("roof trusses")["documents"], json!(10.0));
    assert_eq!(row("roof trusses")["model"], json!(20.0));
    assert_eq!(row("doors (hardware sets' openings)")["documents"], json!(1.0));
    assert!(rep["answer_ms"]["studs"].as_f64().is_some());
    assert!(rep["levels"].as_array().unwrap().len() >= 2);
    assert!(rep["totals"]["concrete"]["volume_cy"].as_f64().unwrap() > 0.0);
    // The rules say where each value came from.
    let rules = s.execute("bim.atoms.rules", &json!({"section": "concrete"})).unwrap();
    assert_eq!(rules["rules"]["concrete"]["chair_spacing_ft"]["from"], json!("documents"), "{rules}");
    assert_eq!(rules["rules"]["concrete"]["rebar_stock_ft"]["from"], json!("defaults"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn project_rules_and_penetrations_change_the_pieces() {
    let mut s = building();
    let before = n(&count(&mut s, json!({"trade": "framing", "kind": "stud", "type": "steel", "level": "2"})), "count");
    // A person sets the partition's studs to 24" o.c.: fewer studs.
    s.execute("project.rules.set", &json!({"domain": "stud_layout", "key": "P1", "value": "24", "reason": "value engineering"})).unwrap();
    s.execute("bim.atoms.build", &json!({"trades": ["framing"]})).unwrap();
    let after = n(&count(&mut s, json!({"trade": "framing", "kind": "stud", "type": "steel", "level": "2"})), "count");
    assert!(after < before, "{after} < {before}");
    // Only the framing was laid out again: the drywall is as it was.
    assert_eq!(count(&mut s, json!({"trade": "drywall", "verify": true}))["verify"]["matches"], json!(true));
    // A duct run through the rated partition: one firestop where it passes, linked to both.
    {
        let d = s.doc_mut().unwrap();
        let mut m = cadaddy_doc::Mesh { vertices: Vec::new(), faces: Vec::new(), kind: cadaddy_doc::MeshKind::Polyface };
        // 12" x 12" duct at y=200, 9' up, from x=100 to x=400 on level 1.
        let (y0, y1, z0, z1) = (194.0, 206.0, 102.0, 114.0);
        for (x, y, z) in [(100.0, y0, z0), (400.0, y0, z0), (400.0, y1, z0), (100.0, y1, z0), (100.0, y0, z1), (400.0, y0, z1), (400.0, y1, z1), (100.0, y1, z1)] {
            m.vertices.push(cadaddy_geom::Vec3::new(x, y, z));
        }
        for f in [[3u32, 2, 1, 0], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]] {
            m.faces.push(cadaddy_doc::MeshFace { v: f.to_vec(), hidden: 0 });
        }
        let t = |v: &str| cadaddy_doc::RawTag { code: 1000, value: v.into() };
        let xdata = vec![
            cadaddy_doc::RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
            t("mep"),
            t("1"),
            t("0"),
            t("system:SA"),
            t("kind:duct"),
            t("part:segment"),
            t("size:12x12"),
        ];
        d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common { layer: "M-DUCT".into(), xdata: xdata.into(), ..Default::default() }, cadaddy_doc::EntityKind::Mesh(m)).unwrap();
    }
    s.execute("bim.atoms.build", &json!({"trades": ["firestopping"]})).unwrap();
    let fs = count(&mut s, json!({"trade": "firestopping", "kind": "firestop", "group_by": "type"}));
    assert_eq!(n(&fs, "count"), 1.0, "{fs}");
    assert_eq!(fs["rows"][0]["type"], json!("1 HR"));
}

#[test]
fn hostile_params_never_panic() {
    let mut s = Session::new();
    assert!(s.execute("bim.atoms.build", &json!({})).is_err(), "no walls built yet");
    assert!(s.execute("bim.atoms.build", &json!({"trades": ["nonsense"]})).is_err());
    assert!(s.execute("bim.atoms.build", &json!({"trades": 7})).is_err());
    assert!(s.execute("bim.atoms.docs", &json!({})).is_err());
    assert!(s.execute("bim.atoms.docs", &json!({"paths": []})).is_err());
    assert!(s.execute("bim.atoms.docs", &json!({"paths": ["/no/such/folder/anywhere"]})).is_err());
    assert!(s.execute("bim.atoms.report", &json!({"group_by": "colour"})).is_err());
    assert!(s.execute("bim.atoms.rules", &json!({"section": "nothing"})).is_err());
    assert!(s.execute("bim.count", &json!({"group_by": "trade"})).is_ok());
    assert!(s.execute("bim.count", &json!({"group_by": "nonsense"})).is_err());
    let r = s.execute("bim.atoms.report", &json!({"level": 5, "trade": null})).unwrap();
    assert!(r["levels"].as_array().unwrap().is_empty());
}
