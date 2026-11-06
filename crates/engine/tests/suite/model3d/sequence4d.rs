//! The construction sequence (Q6014) on a made-up two-level model: two 40' × 30' floors of 6"
//! walls with a partition and a door, a lobby with a ceiling, receptacles on the walls and
//! lights at the ceiling, and under the first floor's deck a supply duct crossed by a sprinkler
//! main, a cold-water pipe and a conduit. Built to 3D, sequenced, shown on dates, exported,
//! checked for trade conflicts in time and space, and its ceiling space checked for fit.

use cadaddy_engine::Session;
use cadaddy_walk3d::fourd::day;
use serde_json::{Value, json};

/// The second plan's offset up the sheet (inches).
const UP: f64 = 1000.0;
const START: &str = "2026-03-02";

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    line(s, [x0, y0], [x1, y0]);
    line(s, [x1, y0], [x1, y1]);
    line(s, [x1, y1], [x0, y1]);
    line(s, [x0, y1], [x0, y0]);
}

fn floor(s: &mut Session, oy: f64, number: usize, title: &str) {
    layer(s, "A-WALL");
    rect(s, 0.0, oy, 480.0, oy + 360.0);
    rect(s, 6.0, oy + 6.0, 474.0, oy + 354.0);
    for x in [238.0, 242.0] {
        line(s, [x, oy + 6.0], [x, oy + 150.0]);
        line(s, [x, oy + 186.0], [x, oy + 354.0]);
    }
    layer(s, "A-DOOR");
    s.execute("arc", &json!({"center": [240.0, oy + 150.0], "radius": 36.0, "start": 0.0, "end": 90.0})).unwrap();
    layer(s, "A-AREA-IDEN");
    for (x, name, n) in [(100.0, "OFFICE", 1), (340.0, "LOBBY", 2)] {
        s.execute("text", &json!({"at": [x, oy + 180.0], "text": name, "height": 8.0})).unwrap();
        s.execute("text", &json!({"at": [x, oy + 160.0], "text": format!("{number}0{n}"), "height": 6.0})).unwrap();
    }
    layer(s, "A-ANNO-TTLB");
    s.execute("text", &json!({"at": [0.0, oy - 40.0], "text": title, "height": 24.0})).unwrap();
}

/// The model, built to 3D: on the first floor a 24x12 duct with a 4" sprinkler main, a 2"
/// cold-water pipe and a 1" conduit crossing under it, in the lobby's ceiling space; on the
/// second a 12x8 duct.
fn model() -> Session {
    let mut s = Session::new();
    floor(&mut s, 0.0, 1, "1ST FLOOR PLAN");
    floor(&mut s, UP, 2, "2ND FLOOR PLAN");
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    let mut sp = s.execute("bspec.get", &json!({})).unwrap();
    let mut sp = sp.get_mut("spec").cloned().unwrap_or(sp);
    // Two receptacles on the first floor's south wall and an occupancy sensor at its ceiling, as
    // the device reader puts them in the spec.
    let device = |id: &str, at: [f64; 2], mark: &str| {
        json!({"id": format!("device:{id}"), "kind": "device", "level": "Level 1", "at_ft": at,
               "fields": {"symbol": {"value": "0", "source": "plan"}, "type": {"value": mark, "source": "plan"}}})
    };
    sp["tags"] = json!([device("r1", [25.0, 0.75], "A"), device("r2", [35.0, 0.75], "A"), device("os1", [30.0, 15.0], "OS")]);
    sp["mep"] = json!({
        "systems": [
            {"name": "M-SUPPLY", "layer": "M-DUCT-SUPP", "kind": "duct", "size_in": [24, 12], "bottom_ft": 8.3},
            {"name": "FP-SPRK", "layer": "F-SPRK", "kind": "pipe", "size_in": [4, 0], "bottom_ft": 8.6},
            {"name": "P-DCW", "layer": "P-DCW", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 9.0},
            {"name": "E-PWR", "layer": "E-COND", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 9.2}
        ],
        "runs": [
            {"system": "M-SUPPLY", "level": "Level 1", "points": [[260, 60], [460, 60]], "size_in": [24, 12], "bottoms_ft": [8.3]},
            {"system": "FP-SPRK", "level": "Level 1", "points": [[300, 20], [300, 300]], "size_in": [4, 0], "bottoms_ft": [8.6]},
            {"system": "P-DCW", "level": "Level 1", "points": [[320, 20], [320, 300]], "size_in": [2, 0], "bottoms_ft": [9.0]},
            {"system": "E-PWR", "level": "Level 1", "points": [[280, 40], [280, 300]], "size_in": [1, 0], "bottoms_ft": [9.2]},
            {"system": "M-SUPPLY", "level": "Level 2", "points": [[260, 1060], [460, 1060]], "size_in": [12, 8], "bottoms_ft": [8.5]}
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

fn built() -> (Session, Value) {
    let mut s = model();
    let r = s.execute("schedule.build", &json!({"start": START})).unwrap();
    (s, r)
}

fn act<'a>(r: &'a Value, id: &str) -> &'a Value {
    r["list"].as_array().unwrap().iter().find(|a| a["id"] == id).unwrap_or_else(|| panic!("no {id}: {r:#}"))
}

fn d(v: &Value) -> i32 {
    day(v.as_str().unwrap()).unwrap()
}

fn handles(s: &Session, part: &str, level: &str) -> Vec<String> {
    let doc = s.doc().unwrap();
    doc.model
        .iter()
        .filter(|e| {
            let v: Vec<&str> = e.common.xdata.iter().map(|t| t.value.as_str()).collect();
            v.first() == Some(&"CADADDY_WALLGEN") && v.get(1) == Some(&part) && v.get(2) == Some(&level)
        })
        .map(|e| e.handle.hex())
        .collect()
}

/// Every zone's activities go in physical order, each after the one before; the second floor's
/// slab waits for the walls under it; overhead rough-in waits for the deck above; one crew per
/// kind of work moves floor to floor; nobody works a weekend; the quantities, cost-book hours
/// and durations are the model's.
#[test]
fn the_sequence_goes_in_the_order_a_building_goes_up() {
    let (mut s, r) = built();
    assert_eq!(r["activities"], 25, "{r:#}");
    assert_eq!(r["span"][0], START);
    assert_eq!(r["not_sequenced"], json!({}), "every built element is in an activity");
    let order = [
        "slab",
        "exterior_walls",
        "framing",
        "duct",
        "sprinkler",
        "plumbing",
        "electrical",
        "inwall_electrical",
        "inspection",
        "insulation",
        "drywall",
        "ceilings",
        "trim_out",
        "casework",
        "finishes",
    ];
    for pair in order.windows(2) {
        let (a, b) = (act(&r, &format!("Level 1/Z1/{}", pair[0])), act(&r, &format!("Level 1/Z1/{}", pair[1])));
        assert!(d(&b["start"]) > d(&a["finish"]), "{} must follow {}: {a} {b}", pair[1], pair[0]);
        assert!(b["predecessors"].as_array().unwrap().contains(&a["id"]), "{b}");
    }
    for a in r["list"].as_array().unwrap() {
        for k in ["start", "finish"] {
            assert!((d(&a[k]) + 3).rem_euclid(7) < 5, "{} {k} on a weekend: {}", a["id"], a[k]);
        }
    }
    assert!(d(&act(&r, "Level 2/Z1/slab")["start"]) > d(&act(&r, "Level 1/Z1/exterior_walls")["finish"]), "the deck sits on the walls below");
    assert!(d(&act(&r, "Level 1/Z1/duct")["start"]) > d(&act(&r, "Level 2/Z1/slab")["finish"]), "duct hangs from the deck above");
    assert!(d(&act(&r, "Level 2/Z1/duct")["start"]) > d(&act(&r, "Level 1/Z1/duct")["finish"]), "the duct crew goes floor to floor");

    let duct = act(&r, "Level 1/Z1/duct");
    assert_eq!((duct["trade"].as_str(), duct["quantity"].as_f64(), duct["unit"].as_str()), (Some("Mechanical"), Some(16.67), Some("ft")), "{duct}");
    assert_eq!(duct["hours"], 4.17, "200 in of duct at the cost book's 0.25 h/ft");
    assert_eq!((duct["crew"].as_f64(), duct["days"].as_i64(), duct["elements"].as_u64()), (Some(4.0), Some(1), Some(1)));
    assert_eq!(duct["rate"], json!(["cost book: Duct"]));
    let dw = act(&r, "Level 1/Z1/drywall");
    assert_eq!(dw["quantity"], 1980.0, "outside walls one face (1386 sf), the partition both (2 × 297): {dw}");
    // Its elements: the six walls and their board's pieces (sheets, tape, bead: Q6162), which go
    // in with it at no hours of their own.
    let pieces = s.execute("bim.count", &json!({"trade": "drywall", "level": "Level 1"})).unwrap()["count"].as_u64().unwrap();
    assert!(pieces > 0);
    assert_eq!(
        (dw["hours"].as_f64(), dw["days"].as_i64(), dw["elements"].as_u64()),
        (Some(39.6), Some(2), Some(6 + pieces)),
        "39.6 h ÷ (4 × 8 h) → 2 days"
    );
    assert_eq!(act(&r, "Level 1/Z1/exterior_walls")["elements"], 4, "the four outside walls");
    assert_eq!(act(&r, "Level 1/Z1/framing")["elements"], 2, "the partition's two pieces");
    assert_eq!(act(&r, "Level 1/Z1/inwall_electrical")["quantity"], 2.0, "the receptacles' boxes");
    assert_eq!(act(&r, "Level 1/Z1/trim_out")["elements"], 3, "the receptacles and the sensor");
    let insp = act(&r, "Level 1/Z1/inspection");
    for ph in ["rough_elec", "rough_mech", "rough_plumb", "firestop"] {
        assert!(insp["phases"].as_array().unwrap().contains(&json!(ph)), "cover-check phase {ph}: {insp}");
    }
    assert_eq!(act(&r, "Level 2/Z1/inspection")["phases"], json!(["rough_mech"]));

    // The sequence is the drawing's 4D schedule: the duct is built once its activity is done.
    let l = s.execute("fourd.list", &json!({})).unwrap();
    assert_eq!(l["tasks"].as_array().unwrap().len(), 25);
    let duct_h = handles(&s, "mep", "Level 1");
    let st = s.execute("fourd.state", &json!({"date": duct["finish"]})).unwrap();
    assert!(st["inProgress"].as_array().unwrap().contains(&json!(duct_h[0])), "in progress through its finish day: {st}");
    let st = s.execute("schedule.view", &json!({"activity": "Level 1/Z1/duct", "handles": true})).unwrap();
    assert!(st["built"].as_array().unwrap().contains(&json!(duct_h[0])), "built the day after: {st}");
    // Without apply4d the drawing's 4D schedule stays as it was.
    s.execute("fourd.clear", &json!({})).unwrap();
    s.execute("schedule.build", &json!({"apply4d": false})).unwrap();
    assert!(s.execute("fourd.list", &json!({})).unwrap()["tasks"].as_array().unwrap().is_empty());
}

/// Crews and rates from the call, an activity's edit, a seven-day week; the edits stay with the
/// drawing and move the dates after them.
#[test]
fn crews_rates_and_edits_change_the_durations() {
    let (mut s, r) = built();
    let finish = d(&r["span"][1]);
    let r2 = s.execute("schedule.build", &json!({"crew": {"drywall": 1}, "rates": {"duct": 1.0}})).unwrap();
    assert_eq!(r2["start"], START, "the start stays as built");
    assert_eq!(act(&r2, "Level 1/Z1/drywall")["days"], 5, "39.6 h for one person");
    let duct = act(&r2, "Level 1/Z1/duct");
    assert_eq!((duct["hours"].as_f64(), duct["rate"][0].as_str()), (Some(16.67), Some("rate set for duct")));
    assert!(d(&r2["span"][1]) > finish);

    let e = s.execute("schedule.edit", &json!({"activity": "level 1/z1/framing", "days": 5})).unwrap();
    assert_eq!(e["activity"]["days"], 5, "{e}");
    let changed = e["changed"].as_array().unwrap();
    assert!(changed.contains(&json!("Level 1/Z1/framing")) && changed.contains(&json!("Level 1/Z1/finishes")), "{e}");
    assert!(!changed.contains(&json!("Level 1/Z1/slab")), "work before it doesn't move");
    assert_eq!(e["applied4d"], true);
    let l = s.execute("schedule.list", &json!({"level": "Level 1", "trade": "Framing"})).unwrap();
    assert_eq!(l["activities"], 2, "{l}");
    let fr = l["list"].as_array().unwrap().iter().find(|a| a["kind"] == "framing").unwrap();
    assert_eq!(fr["days"], 5);
    let task =
        s.execute("fourd.list", &json!({})).unwrap()["tasks"].as_array().unwrap().iter().find(|t| t["id"] == "Level 1/Z1/framing").cloned().unwrap();
    assert_eq!(task["finish"], fr["finish"], "the 4D schedule follows the edit");
    let back = s.execute("schedule.edit", &json!({"activity": "Level 1/Z1/framing", "clear": true})).unwrap();
    assert_eq!(back["activity"]["days"], 1);
    assert!(s.execute("schedule.edit", &json!({"activity": "Level 9/Z1/framing", "days": 2})).is_err());
    assert!(s.execute("schedule.edit", &json!({"activity": "Level 1/Z1/framing", "days": 0})).is_err());

    let week7 = s.execute("schedule.build", &json!({"workdays": 7, "crew": 4, "rates": {}})).unwrap();
    assert!(d(&week7["span"][1]) < d(&r2["span"][1]));
    assert_eq!(week7["working_days"].as_i64().unwrap(), i64::from(d(&week7["span"][1]) - d(&week7["span"][0]) + 1));
}

/// The model as built when the first floor's slab is done (the slab, no walls yet), and on the
/// last day (everything but the work of that day); a picture of it.
#[test]
fn the_model_is_shown_as_built_on_a_date() {
    let (mut s, r) = built();
    let v = s.execute("schedule.view", &json!({"activity": "Level 1/Z1/slab", "handles": true})).unwrap();
    assert_eq!(d(&v["date"]), d(&act(&r, "Level 1/Z1/slab")["finish"]) + 1, "the day after the slab is done");
    let built: Vec<&str> = v["built"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    let slab = handles(&s, "floor", "Level 1");
    assert!(built.contains(&slab[0].as_str()), "{v}");
    for w in handles(&s, "wall", "Level 1") {
        assert!(!built.contains(&w.as_str()), "wall {w} is not up yet");
    }
    assert!(v["upcoming"][0]["id"].as_str().is_some(), "{v}");
    let end = s.execute("schedule.view", &json!({"date": r["span"][1]})).unwrap();
    // The last activity's room and its finishes' pieces (paint, flooring, base).
    let finishing: u64 = ["paint", "flooring", "base"]
        .iter()
        .map(|k| s.execute("bim.count", &json!({"trade": "finishes", "kind": k, "level": "Level 2"})).unwrap()["count"].as_u64().unwrap())
        .sum();
    assert_eq!((end["counts"]["notYet"].as_u64(), end["counts"]["inProgress"].as_u64()), (Some(0), Some(1 + finishing)), "{end}");
    assert_eq!(end["in_progress"], json!(["Level 2/Z1/finishes"]));
    let dir = std::env::temp_dir().join(format!("cadaddy-seq-view-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("Sample Day.png");
    let p = s.execute("schedule.view", &json!({"date": "2026-03-20", "png": png.display().to_string(), "width": 320, "height": 200})).unwrap();
    assert_eq!(p["png"], json!(png.display().to_string()));
    assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(s.execute("schedule.view", &json!({"activity": "nope"})).is_err());
    assert!(s.execute("schedule.view", &json!({"date": "someday"})).is_err());
    let mut fresh = model();
    assert!(fresh.execute("schedule.view", &json!({})).is_err(), "no 4D schedule yet");
}

#[test]
fn the_sequence_exports_to_csv_xlsx_and_ms_project() {
    let (mut s, _) = built();
    let dir = std::env::temp_dir().join(format!("cadaddy-seq-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |n: &str| dir.join(n).display().to_string();
    let c = s.execute("schedule.export", &json!({"path": path("Sample Sequence.csv")})).unwrap();
    assert_eq!((c["activities"].as_u64(), c["format"].as_str()), (Some(25), Some("csv")));
    let text = std::fs::read_to_string(path("Sample Sequence.csv")).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("id,activity,level,zone,trade,kind,quantity,unit,hours,crew,days,start,finish,predecessors,elements"));
    assert_eq!(lines.count(), 25);
    assert!(text.contains("Level 1/Z1/duct,Overhead rough-in: duct — Level 1 Z1,Level 1,Z1,Mechanical,duct,16.67,ft,4.17,4,1,"), "{text}");
    s.execute("schedule.export", &json!({"path": path("Sample Sequence.xlsx")})).unwrap();
    assert!(std::fs::read(path("Sample Sequence.xlsx")).unwrap().starts_with(b"PK"));
    s.execute("schedule.export", &json!({"path": path("Sample Sequence.xml")})).unwrap();
    let xml = std::fs::read_to_string(path("Sample Sequence.xml")).unwrap();
    assert_eq!(xml.matches("<Task>").count(), 25);
    assert!(xml.contains("<Name>Overhead rough-in: duct — Level 1 Z1</Name>") && xml.contains("<PredecessorUID>"), "{xml}");
    assert!(s.execute("schedule.export", &json!({"path": path("x.pdf")})).is_err());
    assert!(s.execute("schedule.export", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The sequence the model gives has no trade conflicts.
#[test]
fn the_built_sequence_has_no_conflicts() {
    let (mut s, _) = built();
    let c = s.execute("schedule.conflicts", &json!({})).unwrap();
    assert_eq!(c["schedule"], 25);
    assert_eq!(c["ok"], true, "{c:#}");
}

/// An imported schedule with the work out of order: conduit before the duct in the same ceiling
/// space (a tight one) and overlapping it, drywall before the boxes, the ceiling grid with no
/// inspection, doors before their walls.
#[test]
fn conflicts_in_an_imported_schedule_are_found() {
    let (mut s, _) = built();
    let rows = |extra: &[[&str; 4]]| {
        let mut v = vec![json!(["Task", "Start", "Finish", "Filter"])];
        for r in [
            ["Conduit", "2026-03-02", "2026-03-03", "system=E-PWR"],
            ["Duct", "2026-03-03", "2026-03-05", "system=M-SUPPLY; level=Level 1"],
            ["Sprinkler main", "2026-03-02", "2026-03-03", "system=FP-SPRK"],
            ["Hang doors", "2026-03-02", "2026-03-02", "class=Door; level=Level 1"],
            ["Hang drywall", "2026-03-04", "2026-03-06", "class=Wall; level=Level 1"],
            ["Rough-in boxes", "2026-03-05", "2026-03-09", "name=3D-Devices"],
            ["Ceiling grid", "2026-03-10", "2026-03-11", "class=Covering; level=Level 1"],
        ]
        .iter()
        .chain(extra)
        {
            v.push(json!(r));
        }
        v
    };
    s.execute("fourd.import", &json!({"rows": rows(&[]), "merge": false})).unwrap();
    let c = s.execute("schedule.conflicts", &json!({})).unwrap();
    let kinds = c["by_kind"].as_object().unwrap();
    for k in ["trade_overlap", "overhead_order", "covered_before_roughin", "cover_without_inspection", "before_host"] {
        assert!(kinds.contains_key(k), "{k} not found: {c:#}");
    }
    let find = |c: &Value, k: &str| c["findings"].as_array().unwrap().iter().filter(|f| f["kind"] == k).cloned().collect::<Vec<_>>();
    let order = find(&c, "overhead_order");
    let o = order.iter().find(|f| f["trades"] == json!(["Electrical", "Mechanical"])).unwrap_or_else(|| panic!("{order:?}"));
    assert_eq!((o["severity"].as_str(), o["level"].as_str()), (Some("high"), Some("Level 1")), "the lobby's ceiling space has no margin: {o}");
    assert!(o["message"].as_str().unwrap().contains("tight"), "{o}");
    let ov = find(&c, "trade_overlap");
    assert!(
        ov.iter().any(|f| f["tasks"] == json!(["1", "2"]) && f["dates"] == json!(["2026-03-03", "2026-03-03"])),
        "conduit and duct on 03-03: {ov:?}"
    );
    let cov = find(&c, "covered_before_roughin");
    assert!(cov.iter().any(|f| f["tasks"] == json!(["5", "6"]) && f["count"] == 2), "drywall over the two boxes: {cov:?}");
    let insp = find(&c, "cover_without_inspection");
    let ceiling = insp.iter().find(|f| f["tasks"] == json!(["7"])).unwrap_or_else(|| panic!("{insp:?}"));
    for ph in ["rough_mech", "rough_elec", "firestop"] {
        assert!(ceiling["phases"].as_array().unwrap().contains(&json!(ph)), "{ph}: {ceiling}");
    }
    let host = find(&c, "before_host");
    assert!(host.iter().any(|f| f["tasks"] == json!(["4", "5"]) && f["message"].as_str().unwrap().contains("door")), "{host:?}");

    // An inspection between the rough-in and the ceiling clears that finding.
    s.execute("fourd.import", &json!({"rows": rows(&[["Inspection Level 1", "2026-03-06", "2026-03-09", ""]]), "merge": false})).unwrap();
    let c2 = s.execute("schedule.conflicts", &json!({})).unwrap();
    assert!(!find(&c2, "cover_without_inspection").iter().any(|f| f["tasks"] == json!(["7"])), "{c2:#}");
    assert!(s.execute("schedule.conflicts", &json!({"level": "Level 2"})).unwrap()["ok"] == true, "nothing on the second floor");
}

/// The lobby's ceiling space: 18" from the ceiling to the deck; the duct (12") with the
/// sprinkler main (4") under it takes 18" with 1" around each, so it just fits (tight, and the
/// two are modeled through each other); with 2" around each it doesn't. Upstairs a 12x8 duct
/// leaves 15".
#[test]
fn the_ceiling_space_is_checked_for_fit() {
    let (mut s, _) = built();
    let f = s.execute("schedule.fit", &json!({})).unwrap();
    let area =
        |f: &Value, level: &str| f["areas"].as_array().unwrap().iter().find(|a| a["level"] == level).cloned().unwrap_or_else(|| panic!("{f:#}"));
    let l1 = area(&f, "Level 1");
    assert_eq!((l1["area"].as_str(), l1["number"].as_str()), (Some("Lobby"), Some("102")));
    assert_eq!(
        (l1["available_in"].as_f64(), l1["required_in"].as_f64(), l1["fits"].as_bool(), l1["tight"].as_bool()),
        (Some(18.0), Some(18.0), Some(true), Some(true)),
        "{l1}"
    );
    let systems: Vec<&str> = l1["governing"]["elements"].as_array().unwrap().iter().filter_map(|e| e["system"].as_str()).collect();
    assert_eq!(systems, ["M-SUPPLY", "FP-SPRK"]);
    assert_eq!(l1["modeled_overlap"], true);
    assert_eq!(f["ok"], true);
    let l2 = area(&f, "Level 2");
    assert_eq!((l2["required_in"].as_f64(), l2["margin_in"].as_f64(), l2["tight"].as_bool()), (Some(9.0), Some(15.0), Some(false)), "{l2}");
    let f2 = s.execute("schedule.fit", &json!({"clearance_in": 2, "level": "Level 1"})).unwrap();
    assert_eq!(f2["ok"], false);
    assert_eq!(f2["areas"].as_array().unwrap().len(), 1);
    let msg = f2["findings"][0]["message"].as_str().unwrap();
    assert!(msg.contains("20.0\" of runs") && msg.contains("2.0\" short"), "{msg}");
    assert_eq!(f2["findings"][0]["elements"].as_array().unwrap().len(), 2);
}

/// The takeoff by activity: each activity's items add up to its hours, all of them to the
/// sequence's, with material from the cost book.
#[test]
fn takeoff_by_activity_adds_up_to_the_sequence() {
    let (mut s, r) = built();
    let t = s.execute("schedule.takeoff", &json!({})).unwrap();
    let acts = t["activities"].as_array().unwrap();
    assert_eq!(acts.len(), 25);
    for a in acts {
        let sum: f64 = a["items"].as_array().unwrap().iter().map(|i| i["labor_hours"].as_f64().unwrap()).sum();
        assert!((sum - a["hours"].as_f64().unwrap()).abs() < 0.05, "{a}");
    }
    assert!((t["totals"]["hours"].as_f64().unwrap() - r["hours"].as_f64().unwrap()).abs() < 0.01);
    let duct = acts.iter().find(|a| a["id"] == "Level 1/Z1/duct").unwrap();
    assert_eq!(duct["items"], json!([{"item": "Duct", "size": "", "unit": "ft", "qty": 16.67, "labor_hours": 4.17, "material": 141.67}]));
    let elec = s.execute("schedule.takeoff", &json!({"trade": "Electrical", "level": "Level 1"})).unwrap();
    assert_eq!(elec["activities"].as_array().unwrap().len(), 3, "overhead, in-wall, trim-out");
}

#[test]
fn hostile_requests_are_refused_or_capped() {
    let mut s = Session::new();
    let r = s.execute("schedule.build", &json!({"start": "2026-03-02"})).unwrap();
    assert_eq!(r["activities"], 0);
    assert!(r["notes"][0].as_str().unwrap().contains("no model elements"), "{r}");
    for p in [
        json!({"start": "soon"}),
        json!({"crew": "many"}),
        json!({"crew": {"drywall": -2}}),
        json!({"workdays": 3}),
        json!({"rates": {"painting": 1}}),
        json!({"rates": {"duct": 1e9}}),
        json!({"hours_per_day": 0}),
        json!({"zone_sf": f64::MAX}),
    ] {
        let out = s.execute("schedule.build", &p);
        if p.get("zone_sf").is_none() {
            assert!(out.is_err(), "{p}");
        }
    }
    assert!(s.execute("schedule.export", &json!({"path": "x.csv"})).is_err(), "nothing to export");
    assert!(s.execute("schedule.edit", &json!({})).is_err());
    assert!(s.execute("schedule.conflicts", &json!({})).is_err(), "no 4D schedule");
    assert!(s.execute("schedule.fit", &json!({"clearance_in": -1})).is_err());
    assert!(s.execute("schedule.conflicts", &json!({"tight_in": 1e9})).is_err());
    let (mut s, _) = built();
    let z = s.execute("schedule.build", &json!({"zone_sf": 100})).unwrap();
    assert!(z["list"].as_array().unwrap().iter().any(|a| a["zone"] == "Z12"), "a small zone size splits the floor (capped at 50 zones)");
    assert!(s.execute("schedule.conflicts", &json!({})).unwrap()["schedule"].as_u64().unwrap() > 25);
}
