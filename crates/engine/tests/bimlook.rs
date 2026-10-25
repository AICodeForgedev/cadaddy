//! The BIM view (Q5241): the built model drawn as BIM tools draw a building — every trade in the
//! colour of one editable table, the shell ghosted so the MEP reads, the sheet's own linework
//! (title block, plan lines) left out of 3D; the model tree (Level ▸ Trade ▸ Category ▸ Type ▸
//! element) with branches hidden, selected and zoomed to; an element's properties (type, size,
//! level, system, circuit, quantity, source sheet and spot, linked issues); golden renders of a
//! made-up three-level building with every trade (an ISO view ghosted, an interior view lit
//! solid); and a 60 000-element model composed quickly.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook::{self, GHOST_MATERIAL, Look};
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::color::Color;
use cadaddy_engine::doc::{Drawing, Entity};
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// Feet → inches.
fn ft(p: [f64; 2]) -> [f64; 2] {
    [p[0] * 12.0, p[1] * 12.0]
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [ft(a), ft([b[0], a[1]]), ft(b), ft([a[0], b[1]])], "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [ft(a), ft(b)]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": ft(at), "text": t, "height": 6})).unwrap();
}

/// A block `name` of one line `width` inches long (a door or window symbol) at `at` (feet).
fn opening(s: &mut Session, name: &str, width: f64, at: [f64; 2], on: &str) {
    if !s.doc().unwrap().blocks.contains_key(name) {
        layer(s, "0");
        s.execute("line", &json!({"points": [[-width / 2.0, 0], [width / 2.0, 0]]})).unwrap();
        let h = s.doc().unwrap().model.iter().last().unwrap().handle.hex();
        s.execute("block", &json!({"name": name, "base": [0, 0], "handles": [h], "keep": "delete"})).unwrap();
    }
    layer(s, on);
    s.execute("insert", &json!({"name": name, "at": ft(at)})).unwrap();
}

/// Where each level's plan is drawn on the sheet (feet east).
const LEVELS: [(&str, f64, f64); 3] = [("Level 1", 0.0, 0.0), ("Level 2", 200.0, 12.0), ("Level 3", 400.0, 24.0)];

fn system(name: &str, kind: &str, size: [f64; 2], bottom: f64, duct_type: Option<&str>) -> Value {
    let mut v = json!({"name": name, "layer": format!("M-{name}"), "kind": kind, "size_in": size, "bottom_ft": bottom});
    if let Some(t) = duct_type {
        v["duct_type"] = json!(t);
    }
    v
}

/// "Sample Building": three storeys of 40' × 24' with 6" walls, 12' floor to floor (each plan
/// drawn 200' east of the one below), an office and a lobby split by a partition, a door and a
/// window in the south wall, a column; the roof over level 3 with a parapet and a rooftop unit;
/// on every level supply, return and exhaust duct, cold and hot water, sanitary, vent, gas and
/// sprinkler pipe, conduit and cable tray under the 9' ceiling. A title block and a north arrow
/// on the sheet (linework 3D never shows).
fn building() -> Session {
    let mut s = Session::new();
    let mut runs: Vec<Value> = Vec::new();
    let mut levels: Vec<Value> = Vec::new();
    for (name, x0, elev) in LEVELS {
        layer(&mut s, "A-WALL");
        rect(&mut s, [x0, 0.0], [x0 + 40.0, 24.0]);
        rect(&mut s, [x0 + 0.5, 0.5], [x0 + 39.5, 23.5]);
        layer(&mut s, "A-WALL-CL");
        line(&mut s, [x0 + 16.0, 0.5], [x0 + 16.0, 23.5]);
        layer(&mut s, "A-ROOM-TAG");
        text(&mut s, [x0 + 8.0, 12.0], "OFFICE");
        text(&mut s, [x0 + 28.0, 12.0], "LOBBY");
        opening(&mut s, "D36", 36.0, [x0 + 6.0, 0.25], "A-DOOR");
        opening(&mut s, "W48", 48.0, [x0 + 26.0, 0.25], "A-GLAZ");
        layer(&mut s, "S-COLS");
        s.execute("circle", &json!({"center": ft([x0 + 24.0, 18.0]), "radius": 9.0})).unwrap();
        levels.push(json!({"name": name, "elevation_ft": elev, "height_ft": 12, "ceiling_ft": 9,
            "region": [(x0 - 10.0) * 12.0, -120.0, (x0 + 60.0) * 12.0, 600.0], "base": [x0 * 12.0, 0.0]}));
        let r = |sys: &str, a: [f64; 2], b: [f64; 2], size: [f64; 2], bottom: f64| json!({"system": sys, "level": name, "points": [ft([x0 + a[0], a[1]]), ft([x0 + b[0], b[1]])], "size_in": size, "bottoms_ft": [bottom]});
        runs.extend([
            r("SUPPLY AIR", [2.0, 4.0], [38.0, 4.0], [16.0, 10.0], 9.6),
            r("RETURN AIR", [2.0, 8.0], [38.0, 8.0], [14.0, 10.0], 9.6),
            r("EXHAUST", [4.0, 20.0], [36.0, 20.0], [10.0, 8.0], 9.8),
            r("CW", [2.0, 11.0], [38.0, 11.0], [1.5, 0.0], 10.4),
            r("HW", [2.0, 12.0], [38.0, 12.0], [1.0, 0.0], 10.4),
            r("SAN", [2.0, 14.0], [38.0, 14.0], [4.0, 0.0], 9.4),
            r("VENT", [2.0, 15.5], [38.0, 15.5], [2.0, 0.0], 10.2),
            r("GAS", [2.0, 17.0], [30.0, 17.0], [1.0, 0.0], 10.6),
            r("FP", [2.0, 22.0], [38.0, 22.0], [2.5, 0.0], 10.8),
            r("E-COND", [2.0, 2.0], [38.0, 2.0], [0.75, 0.0], 11.0),
            r("E-TRAY", [10.0, 6.0], [10.0, 22.0], [12.0, 4.0], 10.9),
        ]);
    }
    // The roof plan, 600' east.
    levels.push(json!({"name": "Roof", "elevation_ft": 36, "height_ft": 4, "ceiling_ft": 4, "region": [590.0 * 12.0, -120.0, 660.0 * 12.0, 600.0], "base": [600.0 * 12.0, 0.0]}));
    // The sheet: a title block and a north arrow (never part of the model).
    layer(&mut s, "A-ANNO-TTLB");
    rect(&mut s, [-20.0, -40.0], [680.0, 60.0]);
    text(&mut s, [620.0, -30.0], "SAMPLE BUILDING");
    line(&mut s, [670.0, 40.0], [670.0, 50.0]);
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": levels,
        "layers": [
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-DOOR", "role": "door"},
            {"layer": "A-GLAZ", "role": "window"},
            {"layer": "S-COLS", "role": "column"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6, "from": "pairs"},
            {"name": "Partition", "height_ft": 9, "thickness_in": 4.875, "from": "centerline"}
        ],
        "families": [
            {"name": "Door 3'", "kind": "door", "block": "D*", "width_in": 36, "head_ft": 7},
            {"name": "Window 4'", "kind": "window", "block": "W*", "width_in": 48, "sill_ft": 3, "head_ft": 7}
        ],
        "exterior": {
            "roofs": [{"name": "Main Roof", "level": "Roof", "outline": [ft([600.0, 0.0]), ft([640.0, 0.0]), ft([640.0, 24.0]), ft([600.0, 24.0])], "parapet_ft": 3}],
            "equipment": [{"name": "RTU-1", "level": "Roof", "outline": [ft([620.0, 8.0]), ft([628.0, 8.0]), ft([628.0, 14.0]), ft([620.0, 14.0])], "height_ft": 4}]
        },
        "mep": {
            "systems": [
                system("SUPPLY AIR", "duct", [16.0, 10.0], 9.6, Some("supply")),
                system("RETURN AIR", "duct", [14.0, 10.0], 9.6, Some("return")),
                system("EXHAUST", "duct", [10.0, 8.0], 9.8, Some("exhaust")),
                system("CW", "pipe", [1.5, 0.0], 10.4, None),
                system("HW", "pipe", [1.0, 0.0], 10.4, None),
                system("SAN", "pipe", [4.0, 0.0], 9.4, None),
                system("VENT", "pipe", [2.0, 0.0], 10.2, None),
                system("GAS", "pipe", [1.0, 0.0], 10.6, None),
                system("FP", "pipe", [2.5, 0.0], 10.8, None),
                system("E-COND", "conduit", [0.75, 0.0], 11.0, None),
                system("E-TRAY", "tray", [12.0, 4.0], 10.9, None)
            ],
            "runs": runs
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// Built elements of trade `key` in `d`.
fn of_trade<'a>(d: &'a Drawing, key: &str) -> Vec<&'a Entity> {
    d.model.iter().filter(|e| bimlook::classify(d, e).is_some_and(|c| c.key == key)).map(|e| &**e).collect()
}

/// The colour `e` shows in `d` (its own, else its layer's).
fn shown_rgb(d: &Drawing, e: &Entity) -> Option<[u8; 3]> {
    let c = match e.common.color {
        Color::ByLayer => d.layer(&e.common.layer)?.color,
        c => c,
    };
    match c {
        Color::True(r) => Some([r.0, r.1, r.2]),
        _ => None,
    }
}

fn ghosted(d: &Drawing, e: &Entity) -> bool {
    d.material_of(e, None).is_some_and(|m| m.name == GHOST_MATERIAL && m.opacity < 0.5)
}

#[test]
fn the_bim_view_draws_only_the_built_model_in_trade_colours() {
    let mut s = building();
    let d = s.doc().unwrap().clone();
    let sheet_lines = d.model.iter().filter(|e| made_as(e).is_none()).count();
    assert!(sheet_lines > 20, "the sheet has its own linework: {sheet_lines}");
    let v = bimlook::display(&d).expect("a built model: the BIM view is on by default");
    assert!(v.model.iter().all(|e| made_as(e).is_some()), "no sheet linework (title block, plan lines) in 3D");
    assert_eq!(v.model.len(), d.model.iter().filter(|e| made_as(e).is_some()).count(), "every built element");
    // Every trade the building has, in its table colour; the shell ghosted (there is MEP).
    for key in [
        "architecture",
        "openings",
        "structure",
        "duct_supply",
        "duct_return",
        "duct_exhaust",
        "pipe_cold",
        "pipe_hot",
        "pipe_sanitary",
        "pipe_vent",
        "pipe_gas",
        "fire",
        "conduit",
        "tray",
    ] {
        let els = of_trade(&v, key);
        assert!(!els.is_empty(), "the model has {key}");
        let want = bimlook::trade(key).unwrap().rgb;
        assert!(els.iter().all(|e| shown_rgb(&v, e) == Some(want)), "{key} drawn in {want:?}");
        let ghost = bimlook::trade(key).unwrap().ghost;
        assert!(els.iter().all(|e| ghosted(&v, e) == ghost), "{key} ghosted: {ghost}");
    }
    // The drawing itself is untouched: layers keep their colours, nothing gets the ghost.
    assert!(d.model.iter().all(|e| !ghosted(&d, e)));
    assert!(s.doc().unwrap().materials.iter().all(|m| m.name != GHOST_MATERIAL));
    // The sheet's lines on request; ghosting off; the view off.
    let r = s.execute("bim.look", &json!({"sheet": true, "ghost": false})).unwrap();
    assert_eq!((r["sheet"].clone(), r["ghost"].clone()), (json!(true), json!(false)), "{r}");
    let v = bimlook::display(s.doc().unwrap()).unwrap();
    assert_eq!(v.model.len(), s.doc().unwrap().model.len());
    assert!(v.model.iter().all(|e| !ghosted(&v, e)));
    s.execute("bim.look", &json!({"on": false})).unwrap();
    assert!(bimlook::display(s.doc().unwrap()).is_none(), "off: 3D draws the drawing as it is");
    let r = s.execute("bim.look", &json!({"on": true, "ghost": "auto", "sheet": false, "ghost_opacity": 5})).unwrap();
    assert_eq!((r["on"].clone(), r["ghost"].clone(), r["ghost_opacity"].clone()), (json!(true), json!(true), json!(1.0)), "{r}");
    assert_eq!(r["style"], json!("Shaded with Edges"), "turning it on sets shaded with edges: {r}");
    // The settings live in the building spec and survive a spec rewrite and a rebuild.
    s.execute("bim.trades", &json!({"colors": {"duct_supply": "#102030"}})).unwrap();
    let levels = s.execute("bspec.get", &json!({})).unwrap()["spec"]["levels"].clone();
    s.execute("bspec.set", &json!({"spec": {"levels": levels}})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    assert_eq!(Look::of(s.doc().unwrap()).rgb("duct_supply"), [16, 32, 48]);
    // A drawing with nothing built: no BIM view.
    assert!(bimlook::display(&Drawing::new_imperial()).is_none());
}

#[test]
fn the_trade_table_is_one_editable_list() {
    let mut s = building();
    let r = s.execute("bim.trades", &json!({})).unwrap();
    let rows = r["trades"].as_array().unwrap();
    assert_eq!(rows.len(), bimlook::TRADES.len());
    let row = |r: &Value, k: &str| r["trades"].as_array().unwrap().iter().find(|t| t["trade"] == k).cloned().unwrap();
    assert!(row(&r, "fire")["count"].as_u64().unwrap() > 0);
    assert_eq!(row(&r, "architecture")["ghosted"], json!(true));
    // Colours as [r, g, b] or #rrggbb; a trade hidden.
    let r = s.execute("bim.trades", &json!({"colors": {"pipe_hot": [250, 0, 0], "conduit": "#00ff7f"}, "visible": {"fire": false}})).unwrap();
    assert_eq!(row(&r, "pipe_hot")["color"], json!([250, 0, 0]));
    assert_eq!(row(&r, "conduit")["color"], json!([0, 255, 127]));
    assert_eq!(row(&r, "fire")["visible"], json!(false));
    let v = bimlook::display(s.doc().unwrap()).unwrap();
    assert!(of_trade(&v, "fire").is_empty(), "a hidden trade leaves the view");
    assert!(of_trade(&v, "pipe_hot").iter().all(|e| shown_rgb(&v, e) == Some([250, 0, 0])));
    assert!(!of_trade(s.doc().unwrap(), "fire").is_empty(), "…not the drawing");
    // Undo puts the table back; reset of one trade, then all.
    s.undo().unwrap();
    assert_eq!(Look::of(s.doc().unwrap()).rgb("pipe_hot"), bimlook::trade("pipe_hot").unwrap().rgb);
    s.redo().unwrap();
    let r = s.execute("bim.trades", &json!({"reset": ["pipe_hot"]})).unwrap();
    assert_eq!(row(&r, "pipe_hot")["color"], json!(bimlook::trade("pipe_hot").unwrap().rgb));
    assert_eq!(row(&r, "conduit")["color"], json!([0, 255, 127]));
    let r = s.execute("bim.trades", &json!({"reset": true})).unwrap();
    assert_eq!(row(&r, "conduit")["color"], json!(bimlook::trade("conduit").unwrap().rgb));
    assert_eq!(row(&r, "fire")["visible"], json!(true));
    assert!(s.execute("bim.trades", &json!({"colors": {"plasma": [1, 2, 3]}})).is_err());
    assert!(s.execute("bim.trades", &json!({"colors": {"duct": [1, 2, 300]}})).is_err());
    assert!(s.execute("bim.trades", &json!({"colors": {"duct": "#12345"}})).is_err());
    assert!(s.execute("bim.trades", &json!({"visible": {"duct": "yes"}})).is_err());
}

#[test]
fn system_names_find_their_trade() {
    for (name, want) in [
        ("CW", "pipe_cold"),
        ("Domestic Cold Water", "pipe_cold"),
        ("HWR", "pipe_hot"),
        ("Domestic Hot Water", "pipe_hot"),
        ("SAN", "pipe_sanitary"),
        ("Sanitary Waste", "pipe_sanitary"),
        ("VTR", "pipe_vent"),
        ("Storm Drain", "pipe_storm"),
        ("NG", "pipe_gas"),
        ("Fire Sprinkler Main", "fire"),
        ("FP", "fire"),
        ("Chilled Glycol", "pipe"),
    ] {
        assert_eq!(bimlook::pipe_trade(name), want, "{name}");
    }
    assert_eq!(bimlook::duct_trade(Some("return"), "Anything"), "duct_return");
    assert_eq!(bimlook::duct_trade(None, "SA-1"), "duct_supply");
    assert_eq!(bimlook::duct_trade(None, "Toilet Exhaust"), "duct_exhaust");
    assert_eq!(bimlook::duct_trade(None, "OA"), "duct_outside");
    assert_eq!(bimlook::duct_trade(None, "Transfer"), "duct");
}

/// A tree node's children under `key`, by name.
fn child<'a>(n: &'a Value, key: &str, name: &str) -> &'a Value {
    n[key].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("no {name} under {key} in {n}"))
}

#[test]
fn the_model_tree_is_level_trade_category_type_element() {
    let mut s = building();
    let r = s.execute("bim.tree", &json!({})).unwrap();
    let levels: Vec<&str> = r["levels"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap()).collect();
    for (name, _, _) in LEVELS {
        assert!(levels.contains(&name), "{levels:?}");
    }
    let l2 = child(&r, "levels", "Level 2");
    for g in ["Architecture", "Structure", "Mechanical", "Plumbing", "Fire Protection", "Electrical"] {
        assert!(child(l2, "trades", g)["count"].as_u64().unwrap() > 0, "{g}");
    }
    let arch = child(l2, "trades", "Architecture");
    for c in ["Walls", "Floors", "Ceilings", "Doors", "Windows"] {
        let _ = child(arch, "categories", c);
    }
    let duct = child(child(l2, "trades", "Mechanical"), "categories", "Duct");
    let supply = duct["types"].as_array().unwrap().iter().find(|t| t["name"].as_str().unwrap().starts_with("SUPPLY AIR")).unwrap();
    assert!(supply["name"].as_str().unwrap().contains("16"), "the type names its size: {supply}");
    let el = &supply["elements"][0];
    assert!(el["handle"].as_str().is_some(), "{supply}");
    // Counts add up.
    let total: u64 = r["levels"].as_array().unwrap().iter().map(|l| l["count"].as_u64().unwrap()).sum();
    assert_eq!(total, r["total"].as_u64().unwrap());
    // Hide a branch: it leaves the view and shows unticked; show it again.
    let r = s.execute("bim.tree", &json!({"path": ["Level 2", "Mechanical"], "visible": false})).unwrap();
    assert_eq!(child(child(&r, "levels", "Level 2"), "trades", "Mechanical")["visible"], json!(false));
    let v = bimlook::display(s.doc().unwrap()).unwrap();
    let hidden =
        |v: &Drawing| v.model.iter().filter(|e| bimlook::classify(v, e).is_some_and(|c| c.level == "Level 2" && c.group == "Mechanical")).count();
    assert_eq!(hidden(&v), 0);
    assert!(v.model.iter().any(|e| bimlook::classify(&v, e).is_some_and(|c| c.level == "Level 1" && c.group == "Mechanical")));
    s.execute("bim.tree", &json!({"path": ["level 2"], "visible": true})).unwrap();
    assert!(hidden(&bimlook::display(s.doc().unwrap()).unwrap()) > 0, "showing the level shows what is under it");
    // Select and zoom: the branch's elements are selected.
    let r = s.execute("bim.tree", &json!({"path": ["Level 1", "Plumbing", "Pipe"], "select": true, "zoom": true})).unwrap();
    let n = r["selected"].as_u64().unwrap() as usize;
    assert!(n >= 5, "cold, hot, sanitary, vent and gas (sprinkler pipe is fire protection): {n}");
    assert_eq!(s.selection().len(), n);
    // Filters.
    let r = s.execute("bim.tree", &json!({"level": "Level 3", "group": "Electrical"})).unwrap();
    assert_eq!(r["levels"].as_array().unwrap().len(), 1);
    assert!(s.execute("bim.tree", &json!({"path": ["Level 9"], "select": true})).is_err());
    assert!(s.execute("bim.tree", &json!({"path": "Level 1"})).is_err());
    // Level isolate in the view.
    s.execute("bim.look", &json!({"level": "level 3"})).unwrap();
    let v = bimlook::display(s.doc().unwrap()).unwrap();
    assert!(v.model.iter().all(|e| bimlook::classify(&v, e).is_some_and(|c| c.level == "Level 3")));
    assert!(s.execute("bim.look", &json!({"level": "Level 9"})).is_err());
    s.execute("bim.look", &json!({"level": null})).unwrap();
    assert!(Look::of(s.doc().unwrap()).level.is_none());
}

#[test]
fn the_building_shell_is_complete() {
    let s = building();
    let d = s.doc().unwrap();
    let count = |level: &str, part: &str| {
        d.model
            .iter()
            .filter(|e| made_as(e).as_deref() == Some(part) && cadaddy_engine::cmd::bimlook::classify(d, e).is_some_and(|c| c.level == level))
            .count()
    };
    for (name, _, _) in LEVELS {
        for part in ["wall", "ceiling", "door", "window", "column"] {
            assert!(count(name, part) > 0, "{name} has a {part}");
        }
    }
    // A slab at every level's floor (the build tags a slab with the level under it).
    let floor_z: Vec<f64> = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("floor"))
        .filter_map(|e| match &e.kind {
            cadaddy_engine::doc::EntityKind::Mesh(m) => m.vertices.iter().map(|v| v.z).reduce(f64::max),
            _ => None,
        })
        .collect();
    for (name, _, elev) in LEVELS {
        assert!(floor_z.iter().any(|z| (z - elev * 12.0).abs() < 12.0), "{name}'s floor at {elev}': {floor_z:?}");
    }
    assert!(count("Roof", "roof") > 0 && count("Roof", "parapet") > 0, "the roof over the top level, with its parapet");
}

#[test]
fn doors_and_windows_are_framed() {
    let s = building();
    let d = s.doc().unwrap();
    let meshes = |part: &str| -> Vec<cadaddy_engine::doc::Mesh> {
        d.model
            .iter()
            .filter(|e| made_as(e).as_deref() == Some(part))
            .filter_map(|e| match &e.kind {
                cadaddy_engine::doc::EntityKind::Mesh(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    };
    let span = |m: &cadaddy_engine::doc::Mesh, k: usize| {
        let v: Vec<f64> = m.vertices.iter().map(|p| [p.x, p.y, p.z][k]).collect();
        v.iter().copied().fold(f64::MIN, f64::max) - v.iter().copied().fold(f64::MAX, f64::min)
    };
    let (doors, windows) = (meshes("door"), meshes("window"));
    assert!(!doors.is_empty() && !windows.is_empty());
    // The 6" exterior wall: the frame's casings stand proud of both faces (6" + 2 x 3/4"), the
    // leaf or pane alone is 1 3/4" or 1/2"; leaf + two jambs + head (+ a window's sill).
    for (part, ms, boxes) in [("door", &doors, 4), ("window", &windows, 5)] {
        for m in ms {
            assert!(m.vertices.len() >= 8 * boxes, "a {part} with its frame: {} vertices", m.vertices.len());
            let across = span(m, 0).min(span(m, 1));
            assert!(across > 4.0, "a {part}'s frame spans the wall: {across}\"");
        }
    }
    // The window frame runs from its sill (3') to its head (7').
    assert!(windows.iter().any(|m| (span(m, 2) - 48.0).abs() < 1e-6), "{:?}", windows.iter().map(|m| span(m, 2)).collect::<Vec<_>>());
}

#[test]
fn an_element_shows_its_properties_and_its_source() {
    let mut s = building();
    let d = s.doc().unwrap();
    let duct = of_trade(d, "duct_supply")
        .into_iter()
        .find(|e| bimlook::classify(d, e).is_some_and(|c| c.category == "Duct" && c.level == "Level 1"))
        .map(|e| e.handle.hex())
        .unwrap();
    let r = s.execute("bim.element", &json!({"handle": duct})).unwrap();
    assert_eq!((r["trade"].clone(), r["group"].clone(), r["category"].clone()), (json!("duct_supply"), json!("Mechanical"), json!("Duct")), "{r}");
    assert_eq!(r["system"], json!("SUPPLY AIR"));
    assert_eq!(r["level"], json!("Level 1"));
    assert!(r["size"].as_str().is_some_and(|z| z.contains("16")), "{r}");
    let len = r["quantity"]["length_ft"].as_f64().unwrap();
    assert!(len > 30.0 && len < 40.0, "the 36' run: {len}");
    // A wall: its length and height; a door: its category.
    let d = s.doc().unwrap();
    let wall = d.model.iter().find(|e| bimlook::classify(d, e).is_some_and(|c| c.category == "Walls" && c.level == "Level 2")).unwrap().handle.hex();
    let door = d.model.iter().find(|e| bimlook::classify(d, e).is_some_and(|c| c.category == "Doors")).unwrap().handle.hex();
    let r = s.execute("bim.element", &json!({"handle": wall})).unwrap();
    assert!(r["quantity"]["length_ft"].as_f64().unwrap() > 1.0 && r["quantity"]["height_ft"].as_f64().unwrap() > 8.0, "{r}");
    assert!(r["source"]["handle"].as_str().is_some(), "a wall's source line on the sheet: {r}");
    let r = s.execute("bim.element", &json!({"handle": door})).unwrap();
    assert_eq!((r["trade"].clone(), r["category"].clone()), (json!("openings"), json!("Doors")), "{r}");
    // Linked issues: an RFI placed on it.
    let rfi = json!([{"id": "RFI-001", "level": "Level 1", "at": ft([20.0, 4.0]), "about": "supply duct", "issue": "Duct low over the door"}]);
    s.execute("bspec.set", &json!({"spec": {"rfis": rfi}})).unwrap();
    let r = s.execute("bim.element", &json!({"handle": duct})).unwrap();
    let issue = r["issues"].as_array().unwrap().iter().find(|i| i["kind"] == "rfi").cloned().unwrap_or(Value::Null);
    assert_eq!((issue["id"].clone(), issue["title"].clone()), (json!("RFI-001"), json!("Duct low over the door")), "{r}");
    // Open its source: the plan view, zoomed to it, the source selected.
    let r = s.execute("bim.element", &json!({"handle": wall, "open_source": true})).unwrap();
    assert_eq!(r["opened"], json!(true));
    let src = r["source"]["handle"].as_str().unwrap();
    assert_eq!(s.selection().iter().map(|h| h.hex()).collect::<Vec<_>>(), vec![src.to_string()]);
    let st = s.state().unwrap();
    assert!((st.view_dir.z - 1.0).abs() < 1e-9, "plan view");
    // Not an element; nothing selected; many selected.
    let sheet_line = s.doc().unwrap().model.iter().find(|e| made_as(e).is_none()).unwrap().handle.hex();
    assert!(s.execute("bim.element", &json!({"handle": sheet_line})).is_err());
    s.set_selection(Vec::new());
    assert!(s.execute("bim.element", &json!({})).is_err());
    assert!(s.execute("bim.element", &json!({"handle": "FFFFFF"})).is_err());
}

// ─── Golden renders ──────────────────────────────────────────────────────────

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/bimlook")
}

fn out_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-bimlook-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn render(s: &mut Session, name: &str, p: Value) -> image::RgbaImage {
    let path = out_dir().join(format!("{name}.png"));
    let mut p = p;
    p["path"] = json!(path.display().to_string());
    let r = s.execute("model.render", &p).unwrap();
    assert_eq!(r["style"], json!("bim"), "{r}");
    image::open(&path).unwrap().to_rgba8()
}

/// Pixels within `tol` of colour `c`.
fn near(img: &image::RgbaImage, c: [u8; 3], tol: i32) -> usize {
    img.pixels().filter(|p| (0..3).all(|k| (i32::from(p[k]) - i32::from(c[k])).abs() <= tol)).count()
}

/// Pixels that are strongly coloured with the hue of `c` (any shade: lit faces vary).
fn hue_like(img: &image::RgbaImage, c: [u8; 3]) -> usize {
    let norm = |v: [f64; 3]| {
        let m = v.iter().copied().fold(0.0, f64::max).max(1.0);
        v.map(|x| x / m)
    };
    let want = norm(c.map(f64::from));
    img.pixels()
        .filter(|p| {
            let v = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
            let sat = v.iter().copied().fold(0.0, f64::max) - v.iter().copied().fold(255.0, f64::min);
            sat > 60.0 && norm(v).iter().zip(want).all(|(a, b)| (a - b).abs() < 0.18)
        })
        .count()
}

/// The render matches the golden within a tolerance (mean channel difference), or becomes the
/// golden when there is none (or `UPDATE_BIM_GOLDEN=1`).
fn golden(name: &str, img: &image::RgbaImage) {
    let path = golden_dir().join(format!("{name}.png"));
    if std::env::var_os("UPDATE_BIM_GOLDEN").is_some() || !path.exists() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        img.save(&path).unwrap();
        eprintln!("bimlook golden written: {}", path.display());
        return;
    }
    let g = image::open(&path).unwrap().to_rgba8();
    assert_eq!(g.dimensions(), img.dimensions(), "{name}: size");
    let diff: u64 = g.as_raw().iter().zip(img.as_raw()).map(|(a, b)| u64::from(a.abs_diff(*b))).sum();
    let mean = diff as f64 / g.as_raw().len() as f64;
    assert!(mean < 3.0, "{name}: mean difference {mean:.2} from the golden (see {})", out_dir().display());
}

#[test]
fn golden_iso_view_is_ghosted_shell_and_saturated_mep() {
    let mut s = building();
    let img = render(&mut s, "iso-ghosted", json!({"width": 640, "height": 480, "dir": [-1, -1, 0.9]}));
    // Every MEP trade shows in its own colour.
    // The ducts and the tray read from across the building.
    for key in ["duct_supply", "duct_return", "duct_exhaust", "tray"] {
        let n = hue_like(&img, bimlook::trade(key).unwrap().rgb);
        assert!(n > 300, "{key}: {n} pixels in its colour");
    }
    // The shell is pale (ghosted), not a grey maze: most of the building's pixels are light.
    let building: Vec<&image::Rgba<u8>> = img.pixels().filter(|p| p[0] < 250 || p[1] < 250 || p[2] < 250).collect();
    let pale = building.iter().filter(|p| p[0] > 180 && p[1] > 180 && p[2] > 180).count();
    assert!(pale * 2 > building.len(), "{pale} of {} pixels pale", building.len());
    // No sheet linework: the title block's corner (far from the building) stays white.
    assert!(near(&img, [255, 255, 255], 0) > img.pixels().len() / 4, "the background stays white round the building");
    golden("iso-ghosted", &img);
    // The same with the shell solid: the walls cover most of the MEP from outside.
    let solid = render(&mut s, "iso-solid", json!({"width": 640, "height": 480, "dir": [-1, -1, 0.9], "ghost": false}));
    let mep =
        |img: &image::RgbaImage| ["duct_supply", "pipe_hot", "fire"].iter().map(|k| hue_like(img, bimlook::trade(k).unwrap().rgb)).sum::<usize>();
    assert!(mep(&solid) < mep(&img), "ghosting lets the MEP show: {} vs {}", mep(&solid), mep(&img));
}

#[test]
fn golden_closeup_shows_every_pipe_in_its_colour() {
    let mut s = building();
    // Level 3's ceiling space, close up (the sheet's coordinates: level 3 is drawn 400' east).
    let img = render(
        &mut s,
        "closeup",
        json!({"width": 640, "height": 480, "level": "Level 3", "dir": [-0.5, -1, 0.7], "around": [ft([404.0, 2.0])[0], ft([404.0, 2.0])[1], ft([416.0, 18.0])[0], ft([416.0, 18.0])[1]]}),
    );
    let keys = ["duct_supply", "duct_return", "pipe_cold", "pipe_hot", "pipe_sanitary", "pipe_vent", "pipe_gas", "tray"];
    let counts: Vec<(&str, usize)> = keys.iter().map(|k| (*k, hue_like(&img, bimlook::trade(k).unwrap().rgb))).collect();
    // The level's floor is in the frame too, so the thinnest pipe (vent) is a few dozen pixels.
    for (key, n) in &counts {
        assert!(*n > 20, "{key}: {n} pixels in its colour ({counts:?})");
    }
    golden("closeup", &img);
}

#[test]
fn golden_interior_view_is_lit_solid() {
    let mut s = building();
    // Level 1 seen into from above the south-east, its ceilings off: the rooms and the ceiling
    // space's runs.
    let img = render(&mut s, "interior", json!({"width": 640, "height": 480, "level": "Level 1", "dir": [0.6, -1, 0.8], "ghost": false}));
    // Solid surfaces (not ghosted): few pure-white pixels, walls lit in several shades.
    let white = near(&img, [255, 255, 255], 0);
    assert!(white < img.pixels().len() / 2, "{white} white pixels");
    assert!(hue_like(&img, bimlook::trade("duct_supply").unwrap().rgb) > 30, "the supply duct overhead");
    golden("interior", &img);
}

// ─── Scale ───────────────────────────────────────────────────────────────────

#[test]
fn a_60k_element_model_composes_quickly() {
    let mut s = building();
    let d = s.doc_mut().unwrap();
    let protos: Vec<Entity> = d.model.iter().filter(|e| made_as(e).is_some()).map(|e| (**e).clone()).collect();
    let mut items = Vec::new();
    while items.len() + protos.len() < 60_000 {
        for p in &protos {
            let mut e = p.clone();
            e.handle = d.new_handle();
            items.push(e);
            if items.len() + protos.len() >= 60_000 {
                break;
            }
        }
    }
    d.model.push_many(items);
    let d = s.doc().unwrap().clone();
    let t = std::time::Instant::now();
    let v = bimlook::display(&d).unwrap();
    let took = t.elapsed();
    assert!(v.model.len() >= 60_000);
    assert!(took.as_millis() < 3_000, "the BIM view of 60k elements in {took:?}");
    let t = std::time::Instant::now();
    let r = s.execute("bim.tree", &json!({"limit": 5})).unwrap();
    assert!(r["total"].as_u64().unwrap() >= 60_000);
    assert!(t.elapsed().as_millis() < 5_000, "the tree of 60k elements in {:?}", t.elapsed());
    eprintln!("60k: view {took:?}, tree {:?}", t.elapsed());
}

#[test]
fn hostile_params_never_panic() {
    let mut s = building();
    let bad = [
        json!({"ghost_opacity": "x"}),
        json!({"ghost_opacity": f64::MAX}),
        json!({"ghost": 3}),
        json!({"level": 7}),
        json!({"style": "plaid"}),
        json!({"sheet": "yes"}),
    ];
    for p in &bad {
        let _ = s.execute("bim.look", p);
    }
    for p in [
        json!({"colors": 5}),
        json!({"colors": {"duct": [f64::NAN, 1, 2]}}),
        json!({"reset": [1, 2]}),
        json!({"visible": []}),
        json!({"colors": {"duct": "#ééé"}}),
    ] {
        let _ = s.execute("bim.trades", &p);
    }
    let long: Vec<String> = (0..10).map(|k| "x".repeat(k * 1000)).collect();
    for p in [
        json!({"path": []}),
        json!({"path": long}),
        json!({"path": [1, 2]}),
        json!({"limit": u64::MAX}),
        json!({"limit": -1}),
        json!({"path": ["Level 1"], "visible": "no"}),
    ] {
        let _ = s.execute("bim.tree", &p);
    }
    for p in [json!({"handle": "zz"}), json!({"handle": -1}), json!({"handles": ["1", "2"]}), json!({"handle": u64::MAX})] {
        let _ = s.execute("bim.element", &p);
    }
    for p in [
        json!({"style": "bim", "width": 0, "height": 0}),
        json!({"style": "bim", "dir": [0, 0, 0]}),
        json!({"ghost": "x", "width": 16, "height": 16}),
    ] {
        let _ = s.execute("model.render", &p);
    }
    // A hand-written spec value of the wrong shape reads as the defaults.
    let d = s.doc_mut().unwrap();
    d.building_spec = Some(r#"{"bim_view": {"colors": {"duct": "red"}, "hidden": 4}}"#.into());
    assert_eq!(Look::of(s.doc().unwrap()), Look::default());
    let _ = bimlook::display(s.doc().unwrap());
}

#[test]
fn every_parameter_the_view_takes_is_documented() {
    let specs = cadaddy_engine::command_specs();
    let spec = |id: &str| specs.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id}"));
    for (id, p) in [
        ("bim.look", json!({"on": true, "ghost": true, "ghost_opacity": 0.2, "sheet": false, "level": null, "style": "shadededges"})),
        ("bim.trades", json!({"colors": {}, "visible": {}, "reset": false})),
        (
            "bim.tree",
            json!({"level": "a", "group": "b", "category": "c", "type": "d", "path": ["a"], "visible": true, "select": true, "zoom": true, "limit": 5}),
        ),
        ("bim.element", json!({"handle": "1", "open_source": false})),
        ("model.render", json!({"style": "bim", "ghost": true, "level": "a", "width": 1, "height": 1})),
    ] {
        assert!(!cadaddy_engine::takes_open_params(spec(id)), "{id} is strict");
        assert_eq!(cadaddy_engine::unknown_params(spec(id), &p), Vec::<String>::new(), "{id}");
    }
}
