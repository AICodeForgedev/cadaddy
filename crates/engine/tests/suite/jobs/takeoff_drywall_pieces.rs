//! The takeoff's drywall is the model's drywall (Q6370): `bim.takeoff`, `bim.bom` and
//! `bim.estimate` take the board off the pieces the model laid on its walls' faces and ceilings,
//! each level's on the level its walls stand on, so they answer what `bim.count {trade: drywall,
//! kind: sheet}` counts; walls the model laid no board on keep their faces' board; a job whose
//! print linework comes before its walls still takes its walls off (never the ceiling-box guess).

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Point};
use cadaddy_engine::geom::Vec3;
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

const T: f64 = 7.25;
const HALF: f64 = 4.875 / 2.0;
/// The second storey's plan, beside the first on the sheet.
const BESIDE: f64 = 2000.0;

/// A 40' x 24' storey at `ox`: exterior walls, a partition at 20', two numbered rooms.
fn storey(s: &mut Session, ox: f64, n: u32) {
    layer(s, "A-WALL");
    rect(s, [ox, 0.0], [ox + 480.0, 288.0]);
    rect(s, [ox + T, T], [ox + 480.0 - T, 288.0 - T]);
    for dx in [-HALF, HALF] {
        line(s, [ox + 240.0 + dx, T], [ox + 240.0 + dx, 288.0 - T]);
    }
    layer(s, "A-ROOM-NAME");
    for (x, k) in [(120.0, 1), (360.0, 2)] {
        s.execute("text", &json!({"at": [ox + x, 144.0], "text": format!("{n}0{k}"), "height": 6.0})).unwrap();
    }
}

fn layers(side1: Value, core: Value, side2: Value) -> Value {
    let mut v: Vec<Value> = Vec::new();
    for (side, list) in [("side1", side1), ("core", core), ("side2", side2)] {
        for mut l in list.as_array().cloned().unwrap_or_default() {
            l["side"] = json!(side);
            v.push(l);
        }
    }
    json!(v)
}

fn spec(typed: bool) -> Value {
    let outer = json!({
        "tag": "W1", "description": "OUTSIDE WALL", "external": true, "source": "sample legend, mark W1",
        "layers": layers(
            json!([{"material": "1/2\" wood structural panel sheathing", "thickness_in": 0.5, "category": "sheathing"}]),
            json!([{"material": "2x6 wood studs @ 24\" o.c.", "thickness_in": 5.5, "category": "framing"}]),
            json!([{"material": "1/2\" gypsum board", "thickness_in": 0.5, "category": "board"}]))
    });
    let inner = json!({
        "tag": "W2", "description": "PARTITION", "source": "sample legend, mark W2",
        "layers": layers(
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]),
            json!([{"material": "3 5/8\" 20 ga metal studs", "thickness_in": 3.625, "category": "framing", "spacing_in": 16.0}]),
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]))
    });
    let mut sp = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [
            {"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000]},
            {"name": "Level 2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "region": [BESIDE - 100.0, -100, BESIDE + 1000.0, 1000]}
        ],
        "rooms": [
            {"name": "OFFICE", "number": "101", "level": "Level 1"},
            {"name": "STORAGE", "number": "102", "level": "Level 1"},
            {"name": "OFFICE", "number": "201", "level": "Level 2"},
            {"name": "STORAGE", "number": "202", "level": "Level 2"}
        ]
    });
    if typed {
        sp["layers"] = json!([{"layer": "A-WALL", "role": "wall", "wall_type": "W1"}]);
        sp["wall_types"] = json!([
            {"name": "W1", "thickness_in": 7.25, "from": "pairs", "assembly": outer},
            {"name": "W2", "thickness_in": 4.875, "from": "pairs", "assembly": inner}
        ]);
        sp["walls"] = json!([
            {"level": "Level 1", "a": [240.0, T], "b": [240.0, 288.0 - T], "wall_type": "W2"},
            {"level": "Level 2", "a": [BESIDE + 240.0, T], "b": [BESIDE + 240.0, 288.0 - T], "wall_type": "W2"}
        ]);
    } else {
        sp["layers"] = json!([{"layer": "A-WALL", "role": "wall"}]);
    }
    sp
}

/// Two storeys side by side, built; `ahead` other elements drawn before them (print linework).
fn building(typed: bool, ahead: u32) -> Session {
    let mut s = Session::new();
    if ahead > 0 {
        layer(&mut s, "X-PRINT");
        for i in 0..ahead {
            let p = Vec3::new(5000.0 + f64::from(i % 400), 5000.0 + f64::from(i / 400), 0.0);
            s.add_entity(EntityKind::Point(Point { p, angle: 0.0 })).unwrap();
        }
    }
    storey(&mut s, 0.0, 1);
    storey(&mut s, BESIDE, 2);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec(typed)})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert!(r["tradePieces"].as_u64().unwrap_or(0) > 0, "the build lays out trade pieces: {r}");
    s
}

fn lines(v: &Value) -> Vec<Value> {
    v["lines"].as_array().cloned().unwrap_or_default()
}

/// A level's board (sq ft) in a takeoff or BOM's lines: (area, line count).
fn board(ls: &[Value], level: &str, qty: &str) -> (f64, usize) {
    let b: Vec<&Value> = ls.iter().filter(|l| l["level"] == level && l["category"] == "drywall" && l["unit"] == "sq ft").collect();
    (b.iter().filter_map(|l| l[qty].as_f64()).sum(), b.len())
}

/// The model's board pieces on a level (sq ft).
fn pieces(s: &mut Session, level: &str) -> f64 {
    let c = s.execute("bim.count", &json!({"trade": "drywall", "kind": "sheet", "level": level})).unwrap();
    c["area_sf"].as_f64().unwrap_or(0.0)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 0.5 + 1e-4 * b.abs()
}

#[test]
fn takeoff_bom_and_estimate_drywall_are_the_models_pieces_per_level() {
    let mut s = building(true, 0);
    let t = s.execute("bim.takeoff", &json!({"group_by": "level"})).unwrap();
    let bom = s.execute("bim.bom", &json!({"group_by": "level", "waste": {"drywall": 0}})).unwrap();
    for level in ["Level 1", "Level 2"] {
        let model = pieces(&mut s, level);
        assert!(model > 500.0, "{level}: the model lays board on its walls and ceilings: {model}");
        let (taken, n) = board(&lines(&t), level, "qty");
        assert!(n > 0 && close(taken, model), "{level}: takeoff {taken} sf in {n} lines, the model's pieces {model} sf: {t}");
        let (net, _) = board(&lines(&bom), level, "net");
        assert!(close(net, model), "{level}: BOM {net} sf, the model's pieces {model} sf: {bom}");
        // Every board line names its pieces as its elements.
        let ls = lines(&t);
        for l in ls.iter().filter(|l| l["level"] == level && l["category"] == "drywall" && l["unit"] == "sq ft") {
            assert!(l["element_count"].as_u64().unwrap_or(0) > 0, "{level}: {l}");
        }
    }
    // No ceiling-box estimate, no board on no level.
    for l in lines(&t).iter().chain(lines(&bom).iter()).filter(|l| l["category"] == "drywall" && l["unit"] == "sq ft") {
        assert!(!l.to_string().contains("estimated from ceiling area"), "{l}");
        assert!(l["level"] == "Level 1" || l["level"] == "Level 2", "{l}");
    }
    // The estimate prices the same board.
    let e = s.execute("bim.estimate", &json!({})).unwrap();
    let est: f64 = e["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["category"] == "drywall" && l["unit"] == "sq ft")
        .filter_map(|l| l["qty"].as_f64().or_else(|| l["net"].as_f64()))
        .sum();
    let model = pieces(&mut s, "Level 1") + pieces(&mut s, "Level 2");
    assert!(est >= model - 0.5, "estimate board {est} sf, the model's pieces {model} sf: {e}");
}

#[test]
fn walls_with_no_board_pieces_keep_their_faces_and_ceilings_count_once() {
    // Untyped walls: the model lays board on the ceilings only; the walls' faces are taken off
    // as before, each level's on its own level, and the ceiling board is the pieces' alone.
    let mut s = building(false, 0);
    let t = s.execute("bim.takeoff", &json!({"group_by": "level"})).unwrap();
    let dw = s.execute("drywall.takeoff", &json!({"waste": 0, "groupBy": "level", "ceilings": false})).unwrap();
    for level in ["Level 1", "Level 2"] {
        let model = pieces(&mut s, level);
        let faces: f64 = dw["rows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["group"] == level)
            .flat_map(|r| r["boards"].as_array().cloned().unwrap_or_default())
            .filter_map(|b| b["areaSqFt"].as_f64())
            .sum();
        assert!(model > 0.0 && faces > 0.0, "{level}: pieces {model}, faces {faces}");
        let (taken, _) = board(&lines(&t), level, "qty");
        assert!(close(taken, model + faces), "{level}: takeoff {taken} sf = pieces {model} + wall faces {faces}: {t}");
    }
}

#[test]
fn walls_after_a_hundred_thousand_print_elements_are_still_taken_off() {
    // The drywall takeoff looked at the first 100,000 elements only: a job's print linework
    // comes first, so it found no walls and the BOM fell back to the ceiling-box guess.
    let mut s = building(true, 100_100);
    let dw = s.execute("drywall.takeoff", &json!({"waste": 0, "groupBy": "level"})).unwrap();
    let rows = dw["rows"].as_array().map_or(0, Vec::len);
    assert_eq!(rows, 2, "both levels' walls are taken off: {}", dw["totals"]);
    let t = s.execute("bim.takeoff", &json!({"group_by": "level"})).unwrap();
    for level in ["Level 1", "Level 2"] {
        let model = pieces(&mut s, level);
        let (taken, _) = board(&lines(&t), level, "qty");
        assert!(model > 500.0 && close(taken, model), "{level}: takeoff {taken} sf, the model's pieces {model} sf");
    }
    assert!(!t.to_string().contains("estimated from ceiling area"), "{t}");
}
