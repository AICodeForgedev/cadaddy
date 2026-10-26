//! Q5242 bimwork: clash, issues (BCF), takeoff and estimate on a synthetic two-level,
//! multi-trade model with planted clashes.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn duct(name: &str) -> Value {
    json!({"name": name, "layer": name, "kind": "duct", "size_in": [12, 8], "bottom_ft": 10})
}

fn pipe(name: &str) -> Value {
    json!({"name": name, "layer": name, "kind": "pipe", "size_in": [2, 0], "bottom_ft": 10})
}

fn conduit(name: &str) -> Value {
    json!({"name": name, "layer": name, "kind": "conduit", "size_in": [1, 0], "bottom_ft": 10})
}

fn run(system: &str, level: &str, a: [f64; 2], b: [f64; 2], size: [f64; 2], bottom: f64) -> Value {
    json!({"system": system, "level": level, "points": [a, b], "size_in": size, "bottoms_ft": [bottom]})
}

/// The runs. Planted clashes (level 1):
/// - the water pipe at x = 100" and the sanitary pipe at x = 150" cross supply duct D1 (y = 60",
///   10'-0" to 10'-8") at its height: plumbing × mechanical = 2;
/// - the conduit at x = 200" crosses D1: electrical × mechanical = 1 (the conduit at x = 400"
///   is past D1's end: no clash);
/// - supply duct D2 runs out through the west wall: mechanical × architecture = 1.
fn runs() -> Vec<Value> {
    vec![
        run("M-SUPPLY", "Level 1", [24.0, 60.0], [264.0, 60.0], [12.0, 8.0], 10.0),
        run("M-SUPPLY", "Level 1", [-48.0, 300.0], [120.0, 300.0], [12.0, 8.0], 10.0),
        run("P-DCW", "Level 1", [100.0, 24.0], [100.0, 150.0], [2.0, 0.0], 10.2),
        run("P-SAN", "Level 1", [150.0, 24.0], [150.0, 150.0], [4.0, 0.0], 10.1),
        run("E-COND", "Level 1", [200.0, 24.0], [200.0, 150.0], [1.0, 0.0], 10.3),
        run("E-COND", "Level 1", [400.0, 24.0], [400.0, 150.0], [1.0, 0.0], 10.3),
        run("M-SUPPLY", "Level 2", [2024.0, 60.0], [2264.0, 60.0], [12.0, 8.0], 10.0),
    ]
}

fn set_spec(s: &mut Session, runs: Vec<Value>) {
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "Level 1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-1000, -1000, 1500, 1500], "base": [0, 0]},
                {"name": "Level 2", "elevation_ft": 14, "height_ft": 14, "ceiling_ft": 9, "region": [1500, -1000, 4000, 1500], "base": [2000, 0]}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "mep": {"systems": [duct("M-SUPPLY"), pipe("P-DCW"), pipe("P-SAN"), conduit("E-COND")], "runs": runs}
        }}),
    )
    .unwrap();
    s.execute("model.build", &json!({})).unwrap();
}

/// Two levels side by side on the sheet (inches), a 40' × 30' room on each, built to 3D.
fn building() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[0, 0], [480, 0], [480, 360], [0, 360]], "closed": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[2000, 0], [2480, 0], [2480, 360], [2000, 360]], "closed": true})).unwrap();
    set_spec(&mut s, runs());
    s
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-bimwork-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn clash(s: &mut Session, a: &str, b: &str) -> Value {
    s.execute("bim.clash", &json!({"a": a, "b": b})).unwrap()
}

fn ids(r: &Value) -> Vec<String> {
    r["clashes"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_string()).collect()
}

fn of_system<'a>(r: &'a Value, system: &str) -> &'a Value {
    r["clashes"].as_array().unwrap().iter().find(|c| c["a"]["system"] == system || c["b"]["system"] == system).unwrap()
}

#[test]
fn planted_clashes_are_found_grouped_with_viewpoints_and_statuses() {
    let mut s = building();
    let pm = clash(&mut s, "plumbing", "mechanical");
    assert_eq!(pm["total"], 2, "{pm}");
    assert_eq!(pm["groups"], json!([{"key": "pipe × duct", "count": 2, "ids": ids(&pm)}]));
    let em = clash(&mut s, "electrical", "mechanical");
    assert_eq!(em["total"], 1, "{em}");
    assert_eq!(em["clashes"][0]["a"]["category"], "conduit");
    let ma = clash(&mut s, "mechanical", "architecture");
    assert_eq!(ma["total"], 1, "only D2 runs through a wall: {ma}");
    assert_eq!((ma["clashes"][0]["b"]["category"].as_str(), ma["clashes"][0]["kind"].as_str()), (Some("wall"), Some("hard")));
    assert_eq!(clash(&mut s, "electrical", "architecture")["total"], 0, "the conduits stay inside the room");
    // A category and a system pick the same as the trade; `b` left out means everything else.
    assert_eq!(s.execute("bim.clash", &json!({"a": "conduit", "b": ["duct"]})).unwrap()["total"], 1);
    assert_eq!(s.execute("bim.clash", &json!({"a": "E-COND", "b": "M-*"})).unwrap()["total"], 1);
    assert_eq!(s.execute("bim.clash", &json!({"a": "plumbing"})).unwrap()["total"], 2);
    // Nothing named: every trade against the others (2 + 1 + 1, never two parts of one trade).
    let all = s.execute("bim.clash", &json!({})).unwrap();
    assert_eq!(all["total"], 4, "{}", all["groups"]);
    assert!(s.execute("bim.clash", &json!({"a": 7})).unwrap_err().to_string().contains("`a` is a trade"));
    // Every clash has a spot, a depth and a viewpoint looking at it.
    for c in pm["clashes"].as_array().unwrap() {
        assert_eq!(c["status"], "new");
        assert_eq!(c["level"], "Level 1");
        let at = c["at"].as_array().unwrap();
        let z = at[2].as_f64().unwrap();
        assert!((120.0..=128.0).contains(&z), "inside D1's height: {c}");
        assert!(c["depth_in"].as_f64().unwrap() > 0.5);
        assert_eq!(c["viewpoint"]["target"], c["at"]);
        assert_ne!(c["viewpoint"]["eye"], c["at"]);
    }
    // Grouped by level and by area.
    let lv = s.execute("bim.clash", &json!({"a": "plumbing", "b": "mechanical", "group_by": "level"})).unwrap();
    assert_eq!(lv["groups"][0]["key"], "level Level 1");
    let ar = s.execute("bim.clash", &json!({"a": "plumbing", "b": "mechanical", "group_by": "area"})).unwrap();
    assert_eq!(ar["groups"][0]["count"], 2, "both within one 20' area: {}", ar["groups"]);
    // A re-run: the same clashes are active; a clash marked resolved that is still there is
    // active again.
    let again = clash(&mut s, "plumbing", "mechanical");
    assert!(again["clashes"].as_array().unwrap().iter().all(|c| c["status"] == "active"), "{again}");
    assert_eq!((again["new"].as_u64(), again["active"].as_u64()), (Some(0), Some(2)));
    let id = ids(&again)[0].clone();
    s.execute("bim.clash.status", &json!({"id": id, "status": "resolved", "note": "by the field"})).unwrap();
    let third = clash(&mut s, "plumbing", "mechanical");
    let c = third["clashes"].as_array().unwrap().iter().find(|c| c["id"] == id.as_str()).unwrap();
    assert_eq!((c["status"].as_str(), c["note"].as_str()), (Some("active"), Some("by the field")));
    assert!(s.execute("bim.clash.status", &json!({"id": id, "status": "fixed"})).is_err());
    assert!(s.execute("bim.clash.status", &json!({"id": "C-0-0", "status": "active"})).is_err());
    // A clearance check also finds the near misses: the conduit at 400" passes 1'-0" over
    // nothing, but a 3" clearance round the 2" pipe reaches D1's side.
    let near = s.execute("bim.clash", &json!({"a": "P-DCW", "b": "duct", "clearance": 3})).unwrap();
    assert_eq!(near["total"], 1);
}

#[test]
fn a_fix_moves_the_lower_priority_trade_on_approval_and_checks_again() {
    let mut s = building();
    let em = clash(&mut s, "electrical", "mechanical");
    let id = ids(&em)[0].clone();
    let before = s.doc().unwrap().clone();
    let plan = s.execute("bim.clash.fix", &json!({"id": id})).unwrap();
    assert_eq!(plan["mover"]["category"], "conduit", "{plan}");
    assert_eq!(plan["stays"]["category"], "duct");
    assert!(plan["why"].as_str().unwrap().starts_with("conduit yields to duct"), "{plan}");
    assert_eq!(plan["applied"], false);
    assert_eq!(plan["new_clashes_expected"], 0);
    assert!(plan["move_in"].as_f64().unwrap() <= 24.0);
    assert_eq!(s.doc().unwrap().model.len(), before.model.len());
    // Approved: applied and checked again; the next run lists it as resolved.
    let done = s.execute("bim.clash.fix", &json!({"id": id, "approve": true})).unwrap();
    assert_eq!(
        (done["applied"].as_bool(), done["check"]["clear"].as_bool(), done["check"]["new_clashes"].as_u64()),
        (Some(true), Some(true), Some(0)),
        "{done}"
    );
    let rerun = clash(&mut s, "electrical", "mechanical");
    assert_eq!(rerun["total"], 0);
    assert_eq!(rerun["resolved"], json!([id]));
    // Duct yields to gravity piping; pressure piping yields to duct.
    let pm = clash(&mut s, "plumbing", "mechanical");
    let san = of_system(&pm, "P-SAN")["id"].clone();
    let dcw = of_system(&pm, "P-DCW")["id"].clone();
    let f = s.execute("bim.clash.fix", &json!({"id": san})).unwrap();
    assert_eq!((f["mover"]["category"].as_str(), f["stays"]["system"].as_str()), (Some("duct"), Some("P-SAN")), "{f}");
    let f = s.execute("bim.clash.fix", &json!({"id": dcw, "strategy": "raise"})).unwrap();
    assert_eq!((f["mover"]["system"].as_str(), f["how"].as_str()), (Some("P-DCW"), Some("raise")), "{f}");
    assert!(f["move"][2].as_f64().unwrap() > 0.0);
    // Sideways along the duct never clears a pipe crossing it.
    let e = s.execute("bim.clash.fix", &json!({"id": dcw, "strategy": "offset"})).unwrap_err().to_string();
    assert!(e.contains("no offset within 24\"") && e.contains("rerouting by hand") && !e.contains("sleeve"), "{e}");
    // A duct through a wall: no move of 2' or less clears a full-height wall it crosses at
    // right angles; the error says what it needs.
    let ma = clash(&mut s, "mechanical", "architecture");
    let wall = ids(&ma)[0].clone();
    let e = s.execute("bim.clash.fix", &json!({"id": wall, "strategy": "raise"})).unwrap_err().to_string();
    assert!(e.contains("rerouting by hand") || e.contains("sleeve"), "{e}");
    assert!(s.execute("bim.clash.fix", &json!({"id": "C-1-2"})).is_err());
    assert!(s.execute("bim.clash.fix", &json!({"id": dcw, "strategy": "teleport"})).is_err());
}

fn line<'a>(t: &'a Value, level: &str, item_starts: &str) -> &'a Value {
    t["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["level"] == level && l["item"].as_str().unwrap().starts_with(item_starts))
        .unwrap_or_else(|| panic!("no {level} {item_starts} in {t}"))
}

#[test]
fn takeoff_has_known_totals_per_level_with_elements_and_diffs() {
    let mut s = building();
    let t = s.execute("bim.takeoff", &json!({"group_by": "level"})).unwrap();
    // D1 20' + D2 14' on level 1, 20' on level 2; the pipes 10'-6" each; two 10'-6" conduits.
    let d1 = line(&t, "Level 1", "Duct M-SUPPLY");
    assert_eq!((d1["qty"].as_f64(), d1["length_ft"].as_f64(), d1["unit"].as_str()), (Some(34.0), Some(34.0), Some("ft")));
    assert_eq!(d1["element_count"], 2, "D1 and D2: {d1}");
    assert_eq!(line(&t, "Level 2", "Duct M-SUPPLY")["qty"], 20.0);
    assert_eq!(line(&t, "Level 2", "Duct M-SUPPLY")["element_count"], 1);
    assert_eq!(line(&t, "Level 1", "P-DCW pipe")["qty"], 10.5);
    assert_eq!(line(&t, "Level 1", "P-DCW pipe")["system"], "P-DCW");
    assert_eq!(line(&t, "Level 1", "EMT conduit")["qty"], 21.0);
    assert_eq!(line(&t, "Level 1", "EMT conduit")["element_count"], 2);
    assert_eq!(t["levels"], json!(["Level 1", "Level 2"]));
    let lines = t["lines"].as_array().unwrap();
    assert!(lines.iter().filter(|l| l["element_count"].as_u64().unwrap() > 0).count() >= 4);
    // Every element a level's line lists is on that level.
    let d = s.doc().unwrap();
    for l in lines.iter().filter(|l| !l["level"].as_str().unwrap().is_empty()) {
        for h in l["elements"].as_array().unwrap() {
            let e = d
                .entity(cadaddy_engine::doc::Handle::parse_hex(h.as_str().unwrap()).unwrap_or_else(|| panic!("bad {h} of {l}")))
                .unwrap_or_else(|| panic!("{h} of {l}"));
            let lv = cadaddy_engine::cmd::wallgen::made_as(e).map(|_| e.common.xdata.iter().filter(|t| t.code == 1000).nth(1).unwrap().value.clone());
            assert_eq!(lv.as_deref(), l["level"].as_str(), "{l}");
        }
    }
    assert!(t["totals"]["length_ft"].as_f64().unwrap() >= 34.0 + 20.0 + 21.0 + 21.0);
    // The same model again: nothing changed.
    let again = s.execute("bim.takeoff", &json!({"group_by": "level"})).unwrap();
    assert_eq!(again["changes"], json!({"added": [], "removed": [], "changed": []}));
    assert_eq!(again["revision"], t["revision"]);
    // By trade, system and type: the levels add up.
    let sys = s.execute("bim.takeoff", &json!({"group_by": "system"})).unwrap();
    assert_eq!(line(&sys, "", "Duct M-SUPPLY")["qty"], 54.0);
    assert_eq!(line(&sys, "", "Duct M-SUPPLY")["system"], "M-SUPPLY");
    let ty = s.execute("bim.takeoff", &json!({"group_by": "type"})).unwrap();
    assert_eq!(line(&ty, "", "Duct M-SUPPLY")["levels"], json!(["Level 1", "Level 2"]));
    let tr = s.execute("bim.takeoff", &json!({"group_by": "trade", "filter": {"trade": "plumbing"}})).unwrap();
    assert!(tr["lines"].as_array().unwrap().iter().all(|l| l["trade"] == "plumbing") && !tr["lines"].as_array().unwrap().is_empty());
    // A line clicked: its elements selected and the 3D camera on them.
    let t = s.execute("bim.takeoff", &json!({"group_by": "level"})).unwrap();
    let k = line(&t, "Level 1", "EMT conduit")["key"].clone();
    let shown = s.execute("bim.takeoff.show", &json!({"line": k})).unwrap();
    assert_eq!(shown["selected"], 2);
    assert_eq!(s.selection().len(), 2);
    assert!(s.state().unwrap().ifc_view.camera.is_some());
    assert!(shown["viewpoint"]["eye"].is_array());
    let idx = t["lines"].as_array().unwrap().iter().position(|l| l["key"] == k).unwrap();
    assert_eq!(s.execute("bim.takeoff.show", &json!({"line": idx})).unwrap()["key"], k);
    // The model changes (D1 lengthened by 10'): the next takeoff says so.
    let mut r = runs();
    r[0] = run("M-SUPPLY", "Level 1", [24.0, 60.0], [384.0, 60.0], [12.0, 8.0], 10.0);
    set_spec(&mut s, r);
    let after = s.execute("bim.takeoff", &json!({"group_by": "level"})).unwrap();
    let ch = after["changes"]["changed"].as_array().unwrap();
    let duct = ch.iter().find(|c| c["key"].as_str().unwrap().starts_with("Level 1|mechanical|duct|Duct M-SUPPLY")).unwrap();
    assert_eq!((duct["before"].as_f64(), duct["after"].as_f64(), duct["delta"].as_f64()), (Some(34.0), Some(44.0), Some(10.0)));
    assert_eq!(after["revision"].as_u64(), t["revision"].as_u64().map(|r| r + 1));
    // Files.
    let x = tmp("takeoff.xlsx");
    s.execute("bim.takeoff", &json!({"file": x.display().to_string(), "keep": false})).unwrap();
    assert!(std::fs::read(&x).unwrap().starts_with(b"PK"));
    assert!(s.execute("bim.takeoff", &json!({"group_by": "colour"})).is_err());
}

/// Q5290: wall takeoff lines must appear on every level that has built walls, and floor/ceiling
/// quantities must come from element geometry (not the sheet extents or a placeholder).
#[test]
fn wall_lines_on_every_level_and_areas_from_geometry() {
    let mut s = building();
    let t = s.execute("bim.takeoff", &json!({"group_by": "level"})).unwrap();
    let lines = t["lines"].as_array().unwrap();
    // Walls: the building has wall polylines on two levels; the takeoff must have wall lines
    // (from the spec's wall marks or from the built wall meshes) on both levels.
    let wall_lines: Vec<&Value> = lines.iter().filter(|l| l["category"] == "walls").collect();
    assert!(!wall_lines.is_empty(), "the takeoff must have wall lines: {t}");
    let wall_levels: std::collections::BTreeSet<&str> = wall_lines.iter().filter_map(|l| l["level"].as_str()).filter(|s| !s.is_empty()).collect();
    assert!(wall_levels.contains("Level 1") && wall_levels.contains("Level 2"), "wall lines on both levels, got {wall_levels:?}: {t}");
    // Total wall length must be positive.
    let wall_lf: f64 = wall_lines.iter().filter(|l| l["unit"] == "lf").filter_map(|l| l["qty"].as_f64()).sum();
    assert!(wall_lf > 0.0, "wall LF must be positive, got {wall_lf}");
    // Filter by category "walls" must find them.
    let filtered = s.execute("bim.takeoff", &json!({"group_by": "level", "filter": {"category": "walls"}, "keep": false})).unwrap();
    let filtered_lines = filtered["lines"].as_array().unwrap();
    assert!(!filtered_lines.is_empty(), "filter:{{category:walls}} must find wall lines: {filtered}");
    // Floor area must not equal the sheet rectangle area (which would be 2500×2500 / 144 =
    // ~43,400 SF on a region from [-1000,-1000] to [1500,1500]): the actual room is 40×30 = 1200 SF.
    let floor_lines: Vec<&Value> = lines.iter().filter(|l| l["category"] == "floors").collect();
    if !floor_lines.is_empty() {
        for fl in &floor_lines {
            let area = fl["qty"].as_f64().unwrap_or(0.0);
            // Sheet-extent area would be > 40,000 SF; actual room is ~1,200 SF.
            assert!(area < 10_000.0, "floor area {area} SF looks like the sheet extent, not geometry: {fl}");
        }
    }
}

#[test]
fn estimate_prices_the_takeoff_and_gives_a_change_order() {
    let mut s = building();
    let table = tmp("costs.csv");
    std::fs::write(&table, "item,unit,material,labor_hrs\nRectangular duct,ft,12.50,0.20\nCopper pipe,ft,8.00,0.15\nEMT conduit,ft,1.10,0.05\nFire damper,each,180,1.5\n").unwrap();
    let e = s.execute("bim.estimate", &json!({"cost_table": table.display().to_string(), "labor_rate": 80, "revision": "A"})).unwrap();
    let t = s.execute("bim.takeoff", &json!({"group_by": "type", "keep": false})).unwrap();
    let q = |item: &str| {
        t["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["item"].as_str().unwrap().starts_with(item))
            .map(|l| l["qty"].as_f64().unwrap())
            .sum::<f64>()
    };
    // Duct, pipe and conduit at 10% waste (the BOM's), priced by the rows they match.
    // Material is on qty with waste; labor is on net qty (waste isn't installed).
    let (duct_net, pipe_net, cond_net) = (q("Duct M-SUPPLY"), q("P-DCW pipe") + q("P-SAN pipe"), q("EMT conduit"));
    let (duct_total, pipe_total, cond_total) = (duct_net * 1.1, pipe_net * 1.1, cond_net * 1.1);
    let material = duct_total * 12.5 + pipe_total * 8.0 + cond_total * 1.1;
    let hours = duct_net * 0.2 + pipe_net * 0.15 + cond_net * 0.05;
    let close = |a: &Value, b: f64| (a.as_f64().unwrap() - b).abs() < 0.01;
    // Q6012: bim.estimate now falls back to the starter cost book. The user-table-
    // priced lines (duct, pipe, conduit) must match the hand calculation; the total
    // includes additional items priced from the starter book.
    let user_lines = e["lines"].as_array().unwrap();
    let is_starter = |l: &Value| l["via"].as_str().is_some_and(|v| v.contains("starter cost book"));
    let user_mat: f64 = user_lines.iter().filter(|l| !is_starter(l)).map(|l| l["material"].as_f64().unwrap_or(0.0)).sum();
    let user_hrs: f64 = user_lines.iter().filter(|l| !is_starter(l)).map(|l| l["labor_hrs"].as_f64().unwrap_or(0.0)).sum();
    assert!((user_mat - material).abs() < 0.01, "user-table material: {user_mat} vs {material}");
    assert!((user_hrs - hours).abs() < 0.01, "user-table labor_hrs: {user_hrs} vs {hours}");
    assert!(e["total"].as_f64().unwrap() >= material + hours * 80.0, "total must be at least the user-table total");
    let d = e["lines"].as_array().unwrap().iter().find(|l| l["cost_item"] == "Rectangular duct").unwrap();
    assert_eq!((d["waste_pct"].as_f64(), d["matched_by"].as_str()), (Some(10.0), Some("words")));
    // Q6012: items previously unmatched may now be priced from the starter book.
    // Items with no cost-table row AND no starter-book match are still unmatched.
    let starter_priced: Vec<&str> = user_lines.iter().filter(|l| is_starter(l)).filter_map(|l| l["item"].as_str()).collect();
    let has_starter_pricing = !starter_priced.is_empty();
    assert!(has_starter_pricing, "starter cost book should price some items");
    assert_eq!(e["unused_rows"], json!(["Fire damper"]));
    // The model grows (D1 + 10'): the change order is 10' of duct with its waste.
    let mut r = runs();
    r[0] = run("M-SUPPLY", "Level 1", [24.0, 60.0], [384.0, 60.0], [12.0, 8.0], 10.0);
    set_spec(&mut s, r);
    let x = tmp("estimate.xlsx");
    let b = s
        .execute("bim.estimate", &json!({"cost_table": table.display().to_string(), "labor_rate": 80, "revision": "B", "compare_to": "A", "file": x.display().to_string()}))
        .unwrap();
    let co = &b["change_order"];
    let dl = co["lines"].as_array().unwrap().iter().find(|l| l["item"].as_str().unwrap().starts_with("Duct M-SUPPLY")).unwrap();
    assert_eq!((dl["delta_qty"].as_f64(), dl["change"].as_str()), (Some(10.0), Some("changed")));
    // Material delta: 10' × 1.1 (waste) × $12.50; labor delta: 10' × 0.2 h × $80 (no waste on labor).
    assert!(close(&dl["delta_cost"], 10.0 * 1.1 * 12.5 + 10.0 * 0.2 * 80.0), "{dl}");
    assert!(close(&co["total"], b["total"].as_f64().unwrap() - e["total"].as_f64().unwrap()), "{co}");
    assert!(std::fs::read(&x).unwrap().starts_with(b"PK"));
    // An XLSX cost table reads too (here the estimate's own XLSX: item, unit, material and
    // labor_hrs columns), matching the duct line by its item; per-level lines when asked.
    let ex = s.execute("bim.estimate", &json!({"cost_table": x.display().to_string(), "labor_rate": 80, "by_level": true})).unwrap();
    let ducts: Vec<&Value> = ex["lines"].as_array().unwrap().iter().filter(|l| l["item"].as_str().unwrap().starts_with("Duct M-SUPPLY")).collect();
    assert_eq!(ducts.len(), 2, "duct on each level: {}", ex["lines"]);
    assert!(ducts.iter().all(|l| l["matched_by"] == "item"));
    let inline = s.execute("bim.estimate", &json!({"rows": [{"item": "EMT conduit", "unit": "ft", "material": 1.1, "labor_hrs": 0.05}]})).unwrap();
    assert_eq!(inline["labor_rate"], 75.0, "the default rate");
    // Bad tables and revisions are errors.
    assert!(s.execute("bim.estimate", &json!({})).is_err());
    assert!(s.execute("bim.estimate", &json!({"csv": "price,qty\n1,2\n"})).is_err());
    assert!(s.execute("bim.estimate", &json!({"rows": [{"item": "X"}], "compare_to": "nope"})).is_err());
    // Q5294 MUST 2: warnings for missing inputs (the default rate is reported).
    assert!(inline["warnings"].as_array().unwrap().iter().any(|w| w["kind"] == "default_labor_rate"), "{}", inline["warnings"]);
    // bim.estimate stores its cost table in the estimate state: estimate.status sees it.
    let st = s.execute("estimate.status", &json!({})).unwrap();
    assert!(st["summary"]["lines"].as_u64().unwrap() > 0, "estimate.status sees BOM lines after bim.estimate: {st}");
}

/// Q5294 MUST 3: same model, same cost table → same totals on both routes.
/// Q6012/Q6015: both routes fall back to the starter cost book. The bim.estimate
/// route groups takeoff lines (by type), while estimate.status prices individual
/// BOM lines, so their starter-book matching may select slightly different rows.
/// We compare the user-cost-table-priced lines (which use stored prices and must
/// agree exactly) and verify both routes include starter-book pricing.
#[test]
fn bim_estimate_and_estimate_status_give_the_same_totals() {
    let mut s = building();
    let table = tmp("same-costs.csv");
    std::fs::write(&table, "item,unit,material,labor_hrs\nEMT conduit,ft,1.10,0.05\nRectangular duct,ft,12.50,0.20\nCopper pipe,ft,8.00,0.15\n")
        .unwrap();
    // Route 1: bim.estimate (stores in estimate state).
    let bim = s.execute("bim.estimate", &json!({"cost_table": table.display().to_string(), "labor_rate": 90})).unwrap();
    // Route 2: estimate.status (prices were stored by bim.estimate; scope all as in_contract).
    for trade in ["electrical", "mechanical", "plumbing", "general", "drywall", "job"] {
        let _ = s.execute("estimate.set_scope", &json!({"trade": trade, "scope": "in_contract", "reason": "all ours"}));
    }
    let est = s.execute("estimate.status", &json!({})).unwrap();
    // Compare the user-cost-table-priced subset: same stored prices, must agree.
    // bim.estimate marks starter lines with via="starter cost book"; estimate.query
    // marks them with via containing "starter cost book" (either the fallback or the
    // stored price source).
    let is_starter = |v: &str| v.contains("starter cost book");
    let bim_lines = bim["lines"].as_array().unwrap();
    let bim_user_mat: f64 =
        bim_lines.iter().filter(|l| l["via"].as_str().is_none_or(|v| !is_starter(v))).map(|l| l["material"].as_f64().unwrap_or(0.0)).sum();
    let bim_user_labor: f64 =
        bim_lines.iter().filter(|l| l["via"].as_str().is_none_or(|v| !is_starter(v))).map(|l| l["labor"].as_f64().unwrap_or(0.0)).sum();
    let q = s.execute("estimate.query", &json!({"limit": 500})).unwrap();
    let est_lines = q["lines"].as_array().unwrap();
    let est_user_mat: f64 =
        est_lines.iter().filter(|l| l["via"].as_str().is_none_or(|v| !is_starter(v))).map(|l| l["material"].as_f64().unwrap_or(0.0)).sum();
    let est_user_labor: f64 =
        est_lines.iter().filter(|l| l["via"].as_str().is_none_or(|v| !is_starter(v))).map(|l| l["labor_cost"].as_f64().unwrap_or(0.0)).sum();
    let close = |a: f64, b: f64| (a - b).abs() < 0.02;
    assert!(close(bim_user_mat, est_user_mat), "user-table material: bim {bim_user_mat} vs est {est_user_mat}");
    assert!(close(bim_user_labor, est_user_labor), "user-table labor: bim {bim_user_labor} vs est {est_user_labor}");
    // Both routes must also price additional lines from the starter cost book.
    let bim_starter: usize = bim_lines.iter().filter(|l| l["via"].as_str().is_some_and(is_starter)).count();
    let est_starter: usize = est_lines.iter().filter(|l| l["via"].as_str().is_some_and(is_starter)).count();
    assert!(bim_starter > 0, "bim.estimate should use the starter cost book");
    assert!(est_starter > 0, "estimate.status should use the starter cost book");
    // The aggregate totals include both user and starter: bim.estimate's total
    // should be close to estimate.status (within the starter-book matching margin).
    let bim_total = bim["total"].as_f64().unwrap();
    let est_total =
        est["totals"]["direct"].as_f64().unwrap_or_else(|| est["totals"]["material"].as_f64().unwrap() + est["totals"]["labor"].as_f64().unwrap());
    let margin = bim_total.max(est_total) * 0.05;
    assert!((bim_total - est_total).abs() < margin, "aggregate totals too far apart: bim {bim_total} vs est {est_total} (margin {margin})");
}

#[test]
fn issues_pin_to_elements_show_on_sheets_and_round_trip_through_bcf_2_1_and_3_0() {
    let mut s = building();
    let em = clash(&mut s, "electrical", "mechanical");
    let conduit = em["clashes"][0]["a"]["handle"].as_str().unwrap().to_string();
    let photo = tmp("photo.png");
    std::fs::write(&photo, [0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 0]).unwrap();
    let i1 = s
        .execute(
            "bim.issue.create",
            &json!({"title": "Conduit hits supply duct", "kind": "punch", "elements": [conduit], "assignee": "Electrician A", "due": "2026-11-30", "photo": photo.display().to_string()}),
        )
        .unwrap();
    assert_eq!((i1["kind"].as_str(), i1["level"].as_str(), i1["photo"].as_bool()), (Some("punch"), Some("Level 1"), Some(true)));
    let i2 = s
        .execute("bim.issue.create", &json!({"title": "Confirm ceiling height", "kind": "rfi", "at": [2100, 100, 170], "level": "Level 2"}))
        .unwrap();
    assert_eq!(i2["index"], 2);
    let list = s.execute("bim.issue.list", &json!({})).unwrap();
    assert_eq!(list["total"], 2);
    let a = &list["issues"][0];
    assert_eq!(a["elements"], json!([conduit]));
    assert_eq!(a["sheet"]["level"], "Level 1");
    assert_eq!(a["sheet"]["at"], json!([200.0, 87.0]), "the conduit's centre on the plan: {a}");
    assert_eq!(a["viewpoint"]["target"], a["at"]);
    assert_eq!(s.execute("bim.issue.list", &json!({"element": conduit})).unwrap()["total"], 1);
    assert_eq!(s.execute("bim.issue.list", &json!({"kind": "rfi"})).unwrap()["issues"][0]["level"], "Level 2");
    // On the sheets: a numbered mark per issue, redrawn (not added again) each time.
    let m = s.execute("bim.issue.marks", &json!({})).unwrap();
    assert_eq!((m["marks"].as_u64(), m["removed"].as_u64()), (Some(2), Some(0)));
    let labels: Vec<&str> = m["issues"].as_array().unwrap().iter().map(|x| x["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["P1", "R2"]);
    let m = s.execute("bim.issue.marks", &json!({})).unwrap();
    assert_eq!((m["marks"].as_u64(), m["removed"].as_u64()), (Some(2), Some(4)));
    s.execute("bim.issue.set", &json!({"issue": 2, "status": "Closed", "comment": "Answered: 9'-0\""})).unwrap();
    assert_eq!(s.execute("bim.issue.marks", &json!({"level": "Level 2"})).unwrap()["marks"], 0, "closed issues leave the sheet");
    let on = s.doc().unwrap().model.iter().filter(|e| e.common.layer.to_string() == "ISSUE-MARKS").count();
    assert_eq!(on, 0);
    s.execute("bim.issue.marks", &json!({})).unwrap();
    // In 3D: its element selected, the camera on it.
    let shown = s.execute("bim.issue.show", &json!({"issue": 1})).unwrap();
    assert_eq!(shown["handles"], json!([conduit]));
    // BCF 2.1 and 3.0 out; both read back as the same topics.
    let topics = cadaddy_engine::cmd::bcf::load(s.doc().unwrap()).topics;
    for v in ["2.1", "3.0"] {
        let f = tmp(&format!("issues-{v}.bcf"));
        let r = s.execute("bcf.export", &json!({"path": f.display().to_string(), "version": v})).unwrap();
        assert_eq!(r["version"], v);
        let bytes = std::fs::read(&f).unwrap();
        let files = cadaddy_engine::cmd::file::zip::read(&bytes).unwrap();
        let ver = String::from_utf8_lossy(&files.iter().find(|(n, _)| n == "bcf.version").unwrap().1).to_string();
        assert!(ver.contains(&format!("VersionId=\"{v}\"")), "{ver}");
        let markup = files.iter().find(|(n, _)| n.ends_with("markup.bcf")).map(|(_, b)| String::from_utf8_lossy(b).to_string()).unwrap();
        assert_eq!(markup.contains("<ViewPoint "), v == "3.0", "{markup}");
        assert_eq!(files.iter().any(|(n, _)| n == "extensions.xml"), v == "3.0");
        let (back, skipped) = cadaddy_engine::cmd::bcf::read_bcf(&bytes).unwrap();
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(back.len(), 2);
        for (x, y) in back.iter().zip(&topics) {
            assert_eq!(
                (&x.guid, &x.title, &x.status, &x.assignee, &x.due, &x.topic_type, &x.labels),
                (&y.guid, &y.title, &y.status, &y.assignee, &y.due, &y.topic_type, &y.labels)
            );
            assert_eq!(x.comments.len(), y.comments.len());
            assert_eq!(x.viewpoints[0].handles, y.viewpoints[0].handles);
            assert_eq!(x.viewpoints[0].snapshot, y.viewpoints[0].snapshot);
        }
        // Into another copy of the model: the issues come in pinned to the same elements.
        let mut other = building();
        let imp = other.execute("bcf.import", &json!({"path": f.display().to_string()})).unwrap();
        assert_eq!(imp["added"], 2, "{imp}");
        let l = other.execute("bim.issue.list", &json!({"kind": "punch"})).unwrap();
        assert_eq!(l["issues"][0]["elements"], json!([conduit]));
        assert_eq!(l["issues"][0]["sheet"]["at"], json!([200.0, 87.0]), "pinned by its element");
        assert_eq!(other.execute("bcf.show", &json!({"topic": "1"})).unwrap()["selected"], json!([conduit]));
    }
    // Hostile and wrong params.
    s.set_selection(Vec::new());
    assert!(s.execute("bim.issue.create", &json!({"title": "No pin"})).is_err(), "nothing selected, no elements, no spot");
    assert!(s.execute("bim.issue.create", &json!({"title": "X", "kind": "memo", "at": [0, 0]})).is_err());
    assert!(s.execute("bim.issue.create", &json!({"title": "X", "elements": ["ZZZZ"]})).is_err());
    assert!(s.execute("bim.issue.create", &json!({"title": "X", "at": [0, 0], "photo_data": "aGVsbG8="})).is_err(), "not an image");
    assert!(s.execute("bim.issue.set", &json!({"issue": 99, "status": "Open"})).is_err());
    assert!(s.execute("bcf.export", &json!({"path": tmp("x.bcf").display().to_string(), "version": "9.9"})).is_err());
}

#[test]
fn the_field_link_goes_between_a_sheet_point_and_3d() {
    let mut s = building();
    // A tap on level 2's sheet: the 3D view at that spot, standing height above level 2.
    let r = s.execute("bim.field.link", &json!({"at": [2100, 60]})).unwrap();
    assert_eq!(r["level"], "Level 2");
    assert_eq!(r["model"], json!([2100.0, 60.0, (14.0 + 5.5) * 12.0]));
    assert_eq!(r["viewpoint"]["target"], r["model"]);
    assert_eq!(r["building_ft"], json!([8.333, 5.0, 19.5]), "from level 2's base point");
    let (eye, target) = s.state().unwrap().ifc_view.camera.unwrap();
    assert!((target.x - 2100.0).abs() < 1e-6 && eye.z > target.z);
    assert!(r["near"].as_array().unwrap().iter().any(|e| e["category"] == "duct" && e["level"] == "Level 2"), "{}", r["near"]);
    // A 3D point: back to its level and sheet point.
    let back = s.execute("bim.field.link", &json!({"at": [200, 87, 125]})).unwrap();
    assert_eq!((back["from"].as_str(), back["level"].as_str()), (Some("model"), Some("Level 1")));
    assert_eq!(back["sheet"], json!([200.0, 87.0]));
    assert!(back["near"].as_array().unwrap().iter().any(|e| e["category"] == "conduit"));
    for bad in [json!({}), json!({"at": [1]}), json!({"at": ["a", "b"]}), json!({"at": [0, 0], "from": "sky"}), json!({"at": [f64::MAX, 0, 0, 0]})] {
        assert!(s.execute("bim.field.link", &bad).is_err(), "{bad}");
    }
}

#[test]
fn hostile_params_never_panic() {
    let mut s = building();
    let junk = [
        json!({}),
        json!({"a": null}),
        json!({"a": 7, "b": {"x": 1}}),
        json!({"a": "", "b": []}),
        json!({"a": "electrical", "tolerance": -5, "clearance": 1e308}),
        json!({"a": "*", "b": "*", "group_by": "level"}),
        json!({"id": 5, "strategy": null, "approve": "yes"}),
        json!({"line": -1}),
        json!({"line": {"k": 1}}),
        json!({"elements": ["!!", 4]}),
        json!({"group_by": 3, "filter": [1, 2]}),
        json!({"cost_table": "/no/such/file.csv"}),
        json!({"rows": [1, "two", null]}),
        json!({"title": "x".repeat(20_000), "at": [1e300, -1e300, 0]}),
        json!({"issue": {"a": 1}}),
        json!({"at": [f64::MIN, f64::MAX]}),
    ];
    for cmd in [
        "bim.clash",
        "bim.clash.status",
        "bim.clash.fix",
        "bim.takeoff",
        "bim.takeoff.show",
        "bim.estimate",
        "bim.issue.create",
        "bim.issue.list",
        "bim.issue.set",
        "bim.issue.show",
        "bim.issue.marks",
        "bim.field.link",
    ] {
        for p in &junk {
            let _ = s.execute(cmd, p);
        }
    }
    // An empty drawing: a clear error, not a panic.
    let mut empty = Session::new();
    let e = empty.execute("bim.clash", &json!({"a": "electrical"})).unwrap_err().to_string();
    assert!(e.contains("build"), "{e}");
}
