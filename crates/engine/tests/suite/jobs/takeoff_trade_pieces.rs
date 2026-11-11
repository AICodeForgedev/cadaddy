//! The takeoff's trade pieces (Q6527): every piece `bim.atoms.build` lays out (studs, plates,
//! track, headers, sheathing, batts, paint, flooring, base, tape, compound, screws and bead, the
//! slab's pour and vapor barrier, a door's frame and hardware set) is a `bim.bom` and
//! `bim.takeoff` line of its trade, by what it is bought as, size and level, its count and
//! measures beside its quantity, its pieces as its elements; it flows to `bim.estimate`,
//! `bim.bid` and `wholejob`. The wall types' formula studs and track, the one-set-per-door rule
//! and the rooms' floor finish give way to the pieces; the concrete takeoff keeps its elements.

use cadaddy_engine::Session;
use std::collections::BTreeSet;

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
            {"name": "STORAGE", "number": "102", "level": "1", "materials": {"floor": "LVT-1"}},
            {"name": "OFFICE", "number": "201", "level": "2", "materials": {"floor": "LVT-1"}},
            {"name": "STORAGE", "number": "202", "level": "2", "materials": {"floor": "LVT-1"}}
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert!(r["tradePieces"].as_u64().unwrap_or(0) > 0, "the build lays out trade pieces: {r}");
    s
}

fn lines(v: &Value) -> Vec<Value> {
    v["lines"].as_array().cloned().unwrap_or_default()
}

fn n(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 0.02 + 1e-4 * b.abs()
}

/// The handles of the model's trade pieces of a trade and kind.
fn piece_handles(s: &Session, trade: &str, kind: &str) -> BTreeSet<String> {
    let d = s.doc().unwrap();
    let (t, k) = (format!("trade:{trade}"), format!("part:atom:{kind}"));
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("atom"))
        .filter(|e| e.common.xdata.iter().any(|x| x.value == t.as_str()) && e.common.xdata.iter().any(|x| x.value == k.as_str()))
        .map(|e| e.handle.hex())
        .collect()
}

/// A BOM's lines of a trade piece kind (their rule names it).
fn piece_lines(bom: &Value, trade: &str, kind: &str) -> Vec<Value> {
    let tag = format!("(bim.count trade {trade}, kind {kind})");
    lines(bom).into_iter().filter(|l| l["trade"] == trade && l["rule"].as_str().is_some_and(|r| r.contains(&tag))).collect()
}

fn sum(ls: &[Value], f: impl Fn(&Value) -> f64) -> f64 {
    ls.iter().map(f).sum()
}

#[test]
fn every_trade_piece_is_a_bom_line_with_its_pieces() {
    let mut s = building();
    let rep = s.execute("bim.atoms.report", &json!({})).unwrap();
    let bom = s.execute("bim.bom", &json!({"group_by": "level", "all_elements": true})).unwrap();
    let mut kinds = 0;
    for (trade, by_kind) in rep["totals"].as_object().unwrap() {
        for (kind, t) in by_kind.as_object().unwrap() {
            // The board sheets are the board lines (Q6370); doors and windows count as themselves.
            if (trade == "drywall" && kind == "sheet") || matches!(kind.as_str(), "door" | "window") {
                continue;
            }
            kinds += 1;
            let ls = piece_lines(&bom, trade, kind);
            assert!(!ls.is_empty(), "{trade} {kind} has BOM lines: {t}");
            // The lines carry exactly the pieces' count and measures.
            let pieces = t["pieces"].as_f64().or_else(|| t["count"].as_f64()).unwrap();
            let m = |k: &str| sum(&ls, |l| l["measures"][k].as_f64().unwrap_or(0.0));
            assert!(close(m("pieces"), pieces), "{trade} {kind} pieces {} vs {pieces}: {ls:?}", m("pieces"));
            for (k, want) in
                [("length_ft", n(t, "length_ft")), ("area_sf", n(t, "area_sf")), ("volume_cy", n(t, "volume_cy")), ("weight_lb", n(t, "weight_lb"))]
            {
                assert!(close(m(k), want), "{trade} {kind} {k} {} vs {want}", m(k));
            }
            // Its elements are exactly its pieces, every one a handle of the model.
            let els: BTreeSet<String> =
                ls.iter().flat_map(|l| l["elements"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string())).collect();
            assert_eq!(els, piece_handles(&s, trade, kind), "{trade} {kind} lines list its pieces");
            assert!(ls.iter().all(|l| l["level"] == "1" || l["level"] == "2"), "on its level: {ls:?}");
        }
    }
    assert!(kinds >= 15, "framing, insulation, finishes, concrete, openings and drywall's accessories: {kinds}");
    // Each in the measure it is bought by.
    let unit = |trade: &str, kind: &str| {
        piece_lines(&bom, trade, kind).first().map(|l| l["unit"].as_str().unwrap_or_default().to_string()).unwrap_or_default()
    };
    for (trade, kind, want) in [
        ("framing", "stud", "ft"),
        ("framing", "plate", "ft"),
        ("framing", "sheathing", "sq ft"),
        ("framing", "header", "each"),
        ("insulation", "batt", "sq ft"),
        ("finishes", "paint", "sq ft"),
        ("finishes", "base", "ft"),
        ("concrete", "pour", "cu yd"),
        ("drywall", "compound", "lb"),
        ("drywall", "screws", "each"),
        ("openings", "frame", "each"),
        ("openings", "hardware_set", "each"),
    ] {
        assert_eq!(unit(trade, kind), want, "{trade} {kind}");
    }
    // A stud line per what it is bought as, per level: level 1's steel studs are its partition's.
    let steel = piece_lines(&bom, "framing", "stud").into_iter().find(|l| l["level"] == "1" && l["item"] == "3 5/8\" 20 ga steel stud").unwrap();
    let c = s.execute("bim.count", &json!({"trade": "framing", "kind": "stud", "level": "1", "item": "steel"})).unwrap();
    assert!(close(n(&steel, "net"), n(&c, "length_ft")) && close(n(&steel["measures"], "pieces"), n(&c, "count")), "{steel} vs {c}");
    // The trade's waste factor where it buys by area (insulation 5%), none on framing.
    let batt = piece_lines(&bom, "insulation", "batt");
    assert!(batt.iter().all(|l| l["waste_pct"] == json!(5.0)), "{batt:?}");
    assert!(piece_lines(&bom, "framing", "stud").iter().all(|l| l["waste_pct"] == json!(0.0)));
    // What the pieces are, the formulas give way to: no wall-type studs or track, no one set
    // per door, no floor finish beside the flooring pieces.
    let all = lines(&bom);
    let item = |l: &Value| l["item"].as_str().unwrap_or_default().to_string();
    assert!(!all.iter().any(|l| l["trade"] == "drywall" && (item(l).starts_with("Studs ") || item(l) == "Track")), "formula studs gone: {all:?}");
    assert!(!all.iter().any(|l| item(l) == "Door hardware set"), "the door's set is its piece");
    assert!(!all.iter().any(|l| item(l).starts_with("Floor finish")), "the rooms' flooring is its pieces");
    let set = piece_lines(&bom, "openings", "hardware_set");
    assert_eq!(sum(&set, |l| n(l, "net")), 1.0, "{set:?}");
}

#[test]
fn trade_pieces_flow_to_the_takeoff_estimate_and_whole_job() {
    let mut s = building();
    let studs = s.execute("bim.count", &json!({"trade": "framing", "kind": "stud"})).unwrap();
    let batts = s.execute("bim.count", &json!({"trade": "insulation", "kind": "batt"})).unwrap();
    assert!(n(&studs, "count") > 0.0 && n(&batts, "count") > 0.0);
    // The takeoff's framing lines, by level: their count and length are the studs', their
    // elements the studs themselves.
    let t = s.execute("bim.takeoff", &json!({"keep": false})).unwrap();
    let stud_lines: Vec<Value> = lines(&t).into_iter().filter(|l| l["trade"] == "framing" && l["category"] == "stud").collect();
    assert!(stud_lines.iter().any(|l| l["level"] == "1") && stud_lines.iter().any(|l| l["level"] == "2"), "{stud_lines:?}");
    assert!(close(sum(&stud_lines, |l| n(l, "count")), n(&studs, "count")), "{stud_lines:?}");
    assert!(close(sum(&stud_lines, |l| n(l, "length_ft")), n(&studs, "length_ft")), "{stud_lines:?}");
    let listed: BTreeSet<String> =
        stud_lines.iter().flat_map(|l| l["elements"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string())).collect();
    assert_eq!(listed, piece_handles(&s, "framing", "stud"));
    assert!(stud_lines.iter().all(|l| l["element_count"].as_u64().unwrap() > 0));
    let one = listed.iter().next().unwrap().clone();
    let got = s.execute("bim.props.get", &json!({"handle": one})).unwrap();
    assert_eq!(got["elements"].as_array().map(Vec::len), Some(1), "{got}");
    for (trade, category) in [("insulation", "batt"), ("finishes", "paint"), ("openings", "hardware set"), ("concrete", "vapor barrier")] {
        assert!(lines(&t).iter().any(|l| l["trade"] == trade && l["category"] == category && n(l, "qty") > 0.0), "{trade} {category} in the takeoff");
    }
    // By the elements' properties: a trade piece is its own kind.
    let by_kind = s.execute("bim.takeoff", &json!({"filter": {"kind": "stud"}, "keep": false})).unwrap();
    assert!(close(sum(&lines(&by_kind), |l| n(l, "count")), n(&studs, "count")), "{by_kind}");
    assert!(lines(&by_kind).iter().all(|l| l["trade"] == "framing"), "{by_kind}");
    let bom_batt = s.execute("bim.bom", &json!({"filter": {"kind": "batt"}})).unwrap();
    assert!(close(sum(&lines(&bom_batt), |l| n(l, "count")), n(&batts, "count")), "{bom_batt}");
    // The estimate prices them by the cost table's rows.
    let est = s
        .execute(
            "bim.estimate",
            &json!({"rows": [{"item": "2x6 wood stud", "unit": "ft", "material": 1.5, "labor_hrs": 0.02}, {"item": "R-21 batt insulation, R-21", "unit": "sq ft", "material": 0.8, "labor_hrs": 0.01}]}),
        )
        .unwrap();
    let wood = s.execute("bim.count", &json!({"trade": "framing", "kind": "stud", "item": "2x6 wood stud"})).unwrap();
    let priced: Vec<Value> = lines(&est).into_iter().filter(|l| l["item"] == "2x6 wood stud").collect();
    assert!(close(sum(&priced, |l| n(l, "qty")), n(&wood, "length_ft")), "{priced:?}");
    assert!(sum(&priced, |l| n(l, "material")) > 0.0 && lines(&est).iter().any(|l| l["cost_item"] == "R-21 batt insulation, R-21"), "{est}");
    // With no cost table, the starter book prices each by its own row (never a wall's or a
    // stud's row for a base or a plate).
    let starter = s.execute("bim.estimate", &json!({})).unwrap();
    for (item, row) in [
        ("2x6 wood plate", "Wood plate"),
        ("4\" wall base", "Wall base"),
        ("interior paint", "Paint"),
        ("R-21 batt insulation, R-21", "Batt insulation"),
        ("hollow metal frame", "Hollow metal frame"),
        ("concrete slab on grade", "Concrete slab"),
        ("10 mil vapor barrier", "Vapor barrier"),
    ] {
        let l = lines(&starter).into_iter().find(|l| l["item"] == item).unwrap_or_default();
        assert_eq!(l["cost_item"], json!(row), "{item}: {l}");
    }
    let unpriced = starter["unmatched"].to_string();
    assert!(!["stud", "batt", "paint", "hardware set", "vapor barrier"].iter().any(|w| unpriced.contains(w)), "{unpriced}");
    // The whole job has the trades.
    let q = s.execute("wholejob.quantities", &json!({"from_bom": true})).unwrap();
    let trades: Vec<String> = q["trades"].as_array().unwrap().iter().map(|t| t["trade"].as_str().unwrap().to_string()).collect();
    for trade in ["framing", "insulation", "finishes", "openings"] {
        assert!(trades.iter().any(|t| t == trade), "{trade} in {trades:?}");
    }
    // A bid of the framing trade has its lines.
    let bid =
        s.execute("bim.bid", &json!({"trades": ["framing"], "show_lines": true, "keep": false, "alternates": false, "underground": false})).unwrap();
    assert!(bid["lines"].as_array().is_some_and(|l| !l.is_empty()), "{}", bid["counts"]);
}

#[test]
fn the_concrete_takeoff_keeps_its_elements_and_another_partys_pieces_are_not_ours() {
    let mut s = building();
    let concrete = json!({"elements": [
        {"id": "SLAB-A", "kind": "slab", "level": "1", "min": [0, 0], "max": [480, 288], "depth_in": 5, "top_ft": 0,
         "mesh": "6x6-W2.9xW2.9", "owner": "electrical", "pour": "P-1", "source": "sample slab note"},
        {"id": "FTG-B", "kind": "footing", "level": "1", "min": [200, 200], "max": [248, 248], "depth_in": 18, "top_ft": -1,
         "rebar": [{"bar": "#5", "direction": "each_way", "spacing_in": 12}], "owner": "concrete", "pour": "P-2"}
    ]});
    s.execute("bspec.set", &json!({"spec": {"concrete": concrete}})).unwrap();
    s.execute("model.build", &json!({"force": true})).unwrap();
    let c = s.execute("bim.count", &json!({"trade": "concrete", "kind": "pour"})).unwrap();
    assert_eq!(n(&c, "count"), 2.0, "both elements poured as pieces: {c}");
    let bom = s.execute("bim.bom", &json!({"group_by": "level", "all_elements": true})).unwrap();
    // Ours: the concrete takeoff's line, once (not again from its pour piece).
    let slab: Vec<Value> = lines(&bom).into_iter().filter(|l| l["item"] == "Concrete, slab").collect();
    assert_eq!(slab.len(), 1, "{slab:?}");
    assert!(piece_lines(&bom, "concrete", "pour").is_empty(), "the pours are the concrete takeoff's");
    assert!(piece_lines(&bom, "concrete", "mesh").is_empty() && piece_lines(&bom, "concrete", "rebar").is_empty());
    // Another party's footing: in by_others, none of its pieces on our lines.
    assert!(bom["by_others"].as_array().unwrap().iter().any(|o| o["element"] == "FTG-B"), "{}", bom["by_others"]);
    let theirs: BTreeSet<String> = {
        let d = s.doc().unwrap();
        d.model
            .iter()
            .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("atom"))
            .filter(|e| {
                let ps = cadaddy_doc::props::read(&e.common.xdata);
                ps.get("trade").map(|p| p.text()).as_deref() == Some("concrete") && ps.get("element").map(|p| p.text()).as_deref() == Some("FTG-B")
            })
            .map(|e| e.handle.hex())
            .collect()
    };
    assert!(!theirs.is_empty());
    let ours: BTreeSet<String> =
        lines(&bom).iter().flat_map(|l| l["elements"].as_array().unwrap().iter().filter_map(|h| h.as_str()).map(str::to_string)).collect();
    assert!(theirs.is_disjoint(&ours), "another party's pour is on no line of ours");
}
