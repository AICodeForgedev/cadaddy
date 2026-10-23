//! The bill of materials of a fixture set read from its prints (Q326) against the truth:
//! conduit by size, luminaires by type, devices by type, every homerun's wire (route plus makeup)
//! and raceway, the GFCI devices and the required items, exactly; each trade's lines (framing,
//! plumbing, mechanical, drywall) against the truth within a stated tolerance; the PDF summary.

use cadaddy_doc::projrules::{ProjectRules, RuleDomain, RuleValue};
use cadaddy_engine::Session;
use cadaddy_mepfix::{conduit_reader, framing, spectag, wall_reader};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn net(lines: &[Value], pred: &dyn Fn(&Value) -> bool) -> f64 {
    lines.iter().filter(|l| pred(l)).map(|l| l["net"].as_f64().unwrap()).sum::<f64>()
}

/// The office fixture set (seed 3) read: tags and conduit/homeruns into a spec; `rules` set.
fn read_set(rules: &[(&str, &str)]) -> (cadaddy_mepfix::truth::Truth, Session, Value) {
    let (t, tags) = spectag::read_fixture_tags(3, &ProjectRules::new()).unwrap();
    let (_, mut runs) = conduit_reader::read_fixture_conduits(3).unwrap();
    // Every run gets a height so the BOM counts it (heights don't change quantities).
    for c in &mut runs.conduits {
        c.bottom_ft.get_or_insert(9.0);
    }
    let names: Vec<(&str, &str)> = t.building.levels.iter().map(|l| (l.code.as_str(), l.code.as_str())).collect();
    let (systems, mep_runs, _) = conduit_reader::spec_runs(&runs, &names);
    let levels: Vec<Value> = t
        .building
        .levels
        .iter()
        .filter(|l| l.code != "R")
        .map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 13}))
        .collect();
    let mut s = Session::new();
    for (k, v) in rules {
        s.doc_mut().unwrap().project_rules.set(RuleDomain::Custom, k, RuleValue::new(*v), "test");
    }
    s.execute("bspec.set", &json!({ "replace": true, "spec": { "levels": levels, "mep": { "systems": systems, "runs": mep_runs }, "tags": spectag::spec_patch(&tags) } })).unwrap();
    let bom = s.execute("bim.bom", &json!({})).unwrap();
    (t, s, bom)
}

#[test]
fn the_bom_of_a_read_set_matches_the_truth() {
    // The homeruns' raceway as the company runs it (the fixture's notes don't name one): 1/2"
    // EMT, a size no branch conduit uses, so it is told apart exactly.
    let (t, mut s, bom) = read_set(&[("bom.homerun_raceway", "1/2\" EMT")]);
    let lines = bom["lines"].as_array().unwrap();
    let net = |pred: &dyn Fn(&Value) -> bool| net(lines, pred);
    // Conduit by size: the truth's drawn conduit, exactly.
    for (size, want_in) in [("3/4\"", 0.75), ("1\"", 1.0), ("1-1/4\"", 1.25)] {
        let truth: f64 = t
            .elec_conduits
            .iter()
            .filter(|c| !c.sheet.is_empty() && (c.size_in - want_in).abs() < 1e-6)
            .map(|c| (c.to_ft[0] - c.from_ft[0]).hypot(c.to_ft[1] - c.from_ft[1]))
            .sum();
        let got = net(&|l| l["item"] == json!("EMT conduit") && l["size"] == json!(size));
        assert!((got - truth).abs() < 0.5, "{size}: BOM {got} ft vs truth {truth} ft");
    }
    // Luminaires by type (within 10%: the device reader misses a few close-packed troffers) and
    // receptacles (within 5%) against the truth's drawn elements.
    let mut types: BTreeMap<String, usize> = Default::default();
    for d in t.elec_devices.iter().filter(|d| d.kind == "light_fixture" && !d.sheet.is_empty() && !t.elec_spec.untagged.contains(&d.id)) {
        *types.entry(d.type_tag.clone()).or_default() += 1;
    }
    for (ty, n) in &types {
        let got = net(&|l| l["item"] == json!(format!("Luminaire type {ty}")));
        assert!((got - *n as f64).abs() <= (*n as f64 * 0.1).max(1.0), "type {ty}: BOM {got} vs truth {n}");
    }
    let recep = t.elec_devices.iter().filter(|d| d.kind == "receptacle" && !d.sheet.is_empty()).count() as f64;
    let got = net(&|l| l["category"] == json!("devices") && l["item"].as_str().unwrap_or_default().ends_with("receptacle"));
    assert!((got - recep).abs() <= recep * 0.05, "receptacles: BOM {got} vs truth {recep}");
    // The GFCI devices the documents call for, exactly: the drawn receptacles with a GFCI subscript.
    let gfci = t
        .elec_devices
        .iter()
        .filter(|d| d.kind == "receptacle" && !d.sheet.is_empty())
        .filter(|d| {
            t.elec_spec.note(&d.id).is_some_and(|n| n.subscripts.iter().any(|x| x.eq_ignore_ascii_case("GFCI") || x.eq_ignore_ascii_case("GFI")))
        })
        .count() as f64;
    assert!(gfci >= 1.0);
    assert_eq!(net(&|l| l["item"] == json!("GFCI receptacle")), gfci, "GFCI");
    // Wire, exactly: each homerun's conductors (the truth's note for its circuit) over its route
    // plus 1' of makeup at each end, by size, ground apart.
    let sp: Value = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    let upf = 12.0;
    let mut want: BTreeMap<(String, bool), f64> = BTreeMap::new();
    let mut routes_ft = 0.0;
    for r in sp["mep"]["runs"].as_array().unwrap().iter().filter(|r| r["system"] == json!(conduit_reader::HOMERUN_SYSTEM)) {
        let pts: Vec<[f64; 2]> = r["points"].as_array().unwrap().iter().map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()]).collect();
        let len: f64 = pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum::<f64>() / upf;
        let origin = [pts[0][0] / upf, pts[0][1] / upf];
        let level = r["level"].as_str().unwrap();
        let c = t
            .elec_circuits
            .iter()
            .find(|c| c.level == level && (c.homerun_ft[0] - origin[0]).hypot(c.homerun_ft[1] - origin[1]) < 1.0)
            .unwrap_or_else(|| panic!("no circuit at {origin:?}"));
        let note = &t.elec_spec.conductors.iter().find(|(id, _)| *id == c.id).unwrap().1;
        routes_ft += len;
        for part in note.split([',', '+']) {
            let (n, rest) = part.trim().split_once('#').unwrap();
            let size: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            *want.entry((format!("#{size}"), rest.ends_with('G'))).or_default() += n.trim().parse::<f64>().unwrap_or(1.0) * (len + 2.0);
        }
    }
    assert!(!want.is_empty());
    for ((size, ground), ft) in &want {
        let item = if *ground { "Copper ground, green" } else { "THHN copper" };
        let got = net(&|l| l["item"] == json!(item) && l["size"] == json!(size));
        assert!((got - ft).abs() < 0.01, "{item} {size}: BOM {got} vs route + makeup {ft}");
    }
    // The homeruns' raceway, exactly their routes.
    let got = net(&|l| l["item"] == json!("EMT conduit") && l["size"] == json!("1/2\""));
    assert!((got - routes_ft).abs() < 0.01, "homerun EMT: BOM {got} vs routes {routes_ft}");
    // The required items for its two floors and the default crew of 4, exactly.
    let req = |item: &str| bom["required"].as_array().unwrap().iter().find(|r| r["item"] == json!(item)).map_or(0.0, |r| r["qty"].as_f64().unwrap());
    let floors = t.building.levels.iter().filter(|l| l.code != "R").count() as f64;
    assert_eq!(floors, 2.0);
    for (item, want) in [
        ("Knockout set", 1.0),
        ("Fish tape", 1.0),
        ("Gang box", floors),
        ("Padlock", floors),
        ("Step ladder, 8'", floors * 2.0),
        ("Material cart", floors),
        ("Right-angle drill (hole hog)", 2.0),
        ("Pull rope, 600' spool", floors),
        ("Carbide hole saw for 1/2\" conduit", 1.0),
        ("Carbide hole saw for 3/4\" conduit", 1.0),
    ] {
        assert_eq!(req(item), want, "{item}: {}", bom["required"]);
    }
}

/// The level codes of the set and the drawing's plan points (inches) for building feet.
fn levels_of(t: &cadaddy_mepfix::truth::Truth) -> Vec<Value> {
    t.building.levels.iter().filter(|l| l.code != "R").map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 13})).collect()
}

/// Each level's plan drawn this far to the right of the one below (inches), in its own region.
const PLAN_STEP_IN: f64 = 60_000.0;

/// The levels, each with its plan region and base point (one plan per region, as a set is drawn).
fn levels_with_regions(t: &cadaddy_mepfix::truth::Truth) -> Vec<Value> {
    levels_of(t)
        .into_iter()
        .enumerate()
        .map(|(i, mut l)| {
            let dx = i as f64 * PLAN_STEP_IN;
            l["region"] = json!([dx - 20_000.0, -20_000.0, dx + 40_000.0, 40_000.0]);
            l["base"] = json!([dx, 0.0]);
            l
        })
        .collect()
}

/// Each trade's lines against the truth on the same set: framing (the truth's framing, members
/// exactly), plumbing (fixtures by kind exactly; hot water pipe insulation within 1%),
/// mechanical (flex duct and dampers per air outlet exactly; supply duct wrap within 1%), drywall
/// (wall board within 15% of the truth's walls × height × board layers — openings and wall ends
/// come off — every line with its level, and tape, mud and screws by the rules exactly).
#[test]
fn every_trades_lines_match_the_truth() {
    let set = cadaddy_mepfix::generate(3).unwrap();
    let t = &set.truth;
    let mut s = Session::new();
    // Framing: the truth's walls framed (Q303), as the spec's takeoff rows.
    let fr = framing::truth_framing(t);
    let rows = framing::spec_rows(&fr);
    let members: usize = fr
        .walls
        .iter()
        .map(|w| {
            let open = w.members.iter().any(|m| m.layout.is_some());
            let (a, b) = (w.resolved(framing::End::A).count(), w.resolved(framing::End::B).count());
            if open { a.max(b) } else { a }
        })
        .sum();
    // Plumbing fixtures and water heaters as scheduled equipment.
    let mut equipment: Vec<Value> = t
        .fixtures
        .iter()
        .map(|f| json!({"tag": f.tag, "kind": f.kind, "level": f.level, "min": [f.at_ft[0] * 12.0 - 9.0, f.at_ft[1] * 12.0 - 9.0], "max": [f.at_ft[0] * 12.0 + 9.0, f.at_ft[1] * 12.0 + 9.0], "bottom_ft": 0, "height_ft": 3}))
        .collect();
    for e in t.equipment.iter().filter(|e| e.kind == "water_heater") {
        equipment.push(json!({"tag": e.tag, "kind": e.kind, "level": e.level, "min": [e.min_ft[0] * 12.0, e.min_ft[1] * 12.0], "max": [e.max_ft[0] * 12.0, e.max_ft[1] * 12.0], "bottom_ft": 0, "height_ft": 5, "properties": {"MODEL": "WH-MODEL-50"}}));
    }
    // Ducts and pipes (horizontal) as runs, supply and outside air marked; air outlets as terminals.
    let mut systems: Vec<Value> = Vec::new();
    let mut runs: Vec<Value> = Vec::new();
    for sg in t.segments.iter().filter(|x| !x.vertical && (x.kind == "duct" || x.kind == "pipe")) {
        let kind = &t.systems.iter().find(|y| y.code == sg.system).map(|y| y.kind.clone()).unwrap_or_default();
        let duct_type = match sg.system.as_str() {
            "SA" => Some("supply"),
            "OA" => Some("outside_air"),
            "RA" => Some("return"),
            "EA" => Some("exhaust"),
            _ => None,
        };
        if !systems.iter().any(|x| x["name"] == json!(sg.system)) {
            let mut sy = json!({"name": sg.system, "layer": sg.system, "kind": sg.kind, "size_in": [6, 0], "bottom_ft": 9});
            if let Some(dt) = duct_type.filter(|_| sg.kind == "duct") {
                sy["duct_type"] = json!(dt);
            }
            systems.push(sy);
        }
        let _ = kind;
        let [a, b] = sg.path_ft;
        let size = if sg.kind == "duct" && sg.shape == "rect" { sg.size_in } else { [sg.size_in[0], 0.0] };
        runs.push(json!({"system": sg.system, "level": sg.level, "points": [[a[0] * 12.0, a[1] * 12.0], [b[0] * 12.0, b[1] * 12.0]], "size_in": size, "bottoms_ft": [a[2].min(b[2]) - t.building.levels.iter().find(|l| l.code == sg.level).map_or(0.0, |l| l.elevation_ft)]}));
    }
    let terminals: Vec<Value> = t
        .terminals
        .iter()
        .map(|x| json!({"system": x.system, "level": x.level, "at": [x.at_ft[0] * 12.0, x.at_ft[1] * 12.0], "size_in": [x.neck_in, 0], "bottom_ft": 9, "terminal_kind": x.kind}))
        .collect();
    // Walls read from the prints, with their types' assemblies, built.
    let (_, walls) = wall_reader::read_fixture_walls(3).unwrap();
    let wp = wall_reader::spec_patch(&walls, &|_, p| Some([p[0] * 12.0, p[1] * 12.0]));
    let mut wp = wp;
    let lv = levels_with_regions(t);
    let layers = draw_walls(&mut s, &mut wp, &lv);
    let spec = json!({
        "levels": lv,
        "layers": layers,
        "framing": rows,
        "wall_types": wp["wall_types"],
        "walls": wp["walls"],
        "mep": {"systems": systems, "runs": runs, "terminals": terminals, "equipment": equipment},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let bom = s.execute("bim.bom", &json!({"group_by": "level"})).unwrap();
    let lines = bom["lines"].as_array().unwrap();
    let net = |pred: &dyn Fn(&Value) -> bool| net(lines, pred);
    // Framing: every member the truth's framing orders, by member and status.
    assert!(members > 0);
    assert_eq!(net(&|l| l["trade"] == json!("framing")), members as f64, "framing members");
    assert!(
        lines.iter().filter(|l| l["trade"] == json!("framing")).all(|l| ["fixed", "rule", "field_verified"].contains(&l["status"].as_str().unwrap()))
    );
    assert!(
        net(&|l| l["trade"] == json!("framing") && l["status"] == json!("fixed")) > 0.0
            && net(&|l| l["trade"] == json!("framing") && l["status"] == json!("rule")) > 0.0
    );
    // Plumbing fixtures by kind, exactly; the water heater by its model.
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for f in &t.fixtures {
        *kinds.entry(f.kind.clone()).or_default() += 1;
    }
    assert!(!kinds.is_empty());
    for (kind, n) in &kinds {
        let words = kind.replace('_', " ");
        let got = net(&|l| l["category"] == json!("plumbing fixtures") && l["item"].as_str().unwrap().to_ascii_lowercase().starts_with(&words));
        assert_eq!(got, *n as f64, "{kind}");
    }
    let heaters = t.equipment.iter().filter(|e| e.kind == "water_heater").count() as f64;
    assert_eq!(net(&|l| l["item"] == json!("Water heater, WH-MODEL-50")), heaters);
    // Hot water pipe insulation: the truth's horizontal HW pipe, within 1%.
    let hw: f64 = t
        .segments
        .iter()
        .filter(|x| !x.vertical && x.kind == "pipe" && x.system.contains("HW"))
        .map(|x| (x.path_ft[1][0] - x.path_ft[0][0]).hypot(x.path_ft[1][1] - x.path_ft[0][1]))
        .sum();
    let got = net(&|l| l["category"] == json!("insulation") && l["unit"] == json!("ft"));
    assert!(hw > 0.0 && (got - hw).abs() <= hw * 0.01, "pipe insulation {got} vs HW pipe {hw}");
    // Flex and a damper per air outlet, exactly.
    let outlets = t.terminals.iter().filter(|x| ["diffuser", "register", "grille"].iter().any(|k| x.kind.contains(k))).count() as f64;
    assert!(outlets > 0.0);
    assert_eq!(net(&|l| l["item"] == json!("Flex duct, insulated")), outlets * 5.0);
    assert_eq!(net(&|l| l["item"] == json!("Volume damper")), outlets);
    // Supply and outside air duct wrap: the truth's duct surface, within 1%.
    let surface: f64 = t
        .segments
        .iter()
        .filter(|x| !x.vertical && x.kind == "duct" && (x.system == "SA" || x.system == "OA"))
        .map(|x| {
            let len = (x.path_ft[1][0] - x.path_ft[0][0]).hypot(x.path_ft[1][1] - x.path_ft[0][1]);
            let perim = if x.shape == "rect" { 2.0 * (x.size_in[0] + x.size_in[1]) / 12.0 } else { std::f64::consts::PI * x.size_in[0] / 12.0 };
            len * perim
        })
        .sum();
    let got = net(&|l| l["category"] == json!("insulation") && l["unit"] == json!("sq ft"));
    assert!(surface > 0.0 && (got - surface).abs() <= surface * 0.01, "duct wrap {got} vs truth {surface}");
    // Drywall: wall board against the truth's walls, by level; finishing by the rules.
    let board_layers = |tag: &str| {
        t.wall_types.iter().find(|w| w.tag == tag).map_or(0, |w| w.layers.iter().filter(|l| l.material.starts_with("gyp")).count()) as f64
    };
    let truth_board: f64 =
        t.plan_walls.iter().map(|w| (w.b_ft[0] - w.a_ft[0]).hypot(w.b_ft[1] - w.a_ft[1]) * w.height_ft * board_layers(&w.wall_type)).sum();
    let board =
        net(&|l| l["trade"] == json!("drywall") && l["item"].as_str().unwrap().contains("gypsum") && l["item"].as_str().unwrap().ends_with(", wall"));
    assert!(truth_board > 0.0 && (board - truth_board).abs() <= truth_board * 0.15, "wall board {board} vs truth {truth_board}");
    assert!(
        lines.iter().filter(|l| l["trade"] == json!("drywall")).all(|l| !l["level"].as_str().unwrap().is_empty()),
        "every drywall line has its level"
    );
    let all_board = net(&|l| l["trade"] == json!("drywall") && l["item"].as_str().unwrap().starts_with("Board"));
    assert!((net(&|l| l["item"] == json!("Joint tape")) - all_board * 0.37).abs() < 0.05);
    assert!((net(&|l| l["item"] == json!("Joint compound")) - all_board * 0.014).abs() < 0.05);
    assert!(net(&|l| l["item"] == json!("Drywall screws")) >= all_board.floor());
}

/// Each wall's two faces drawn on its type's layer (`A-WALL-<type>`); the layers' roles.
fn draw_walls(s: &mut Session, wp: &mut Value, levels: &[Value]) -> Vec<Value> {
    let thick = |ty: &str| {
        wp["wall_types"].as_array().unwrap().iter().find(|t| t["name"] == json!(ty)).and_then(|t| t["thickness_in"].as_f64()).unwrap_or(4.875)
    };
    let mut layers: Vec<Value> = Vec::new();
    let thick_of: Vec<(String, f64)> =
        wp["wall_types"].as_array().unwrap().iter().map(|t| (t["name"].as_str().unwrap().to_string(), thick(t["name"].as_str().unwrap()))).collect();
    let thick = |ty: &str| thick_of.iter().find(|(n, _)| n == ty).map_or(4.875, |x| x.1);
    for w in wp["walls"].as_array_mut().unwrap() {
        // Onto its level's plan.
        let dx = levels.iter().find(|l| l["name"] == w["level"]).map_or(0.0, |l| l["base"][0].as_f64().unwrap());
        for k in ["a", "b"] {
            w[k][0] = json!(w[k][0].as_f64().unwrap() + dx);
        }
        let ty = w["wall_type"].as_str().unwrap().to_string();
        let ty = ty.as_str();
        let layer = format!("A-WALL-{ty}");
        if !layers.iter().any(|l| l["layer"] == json!(layer)) {
            s.execute("layer.new", &json!({"name": layer})).unwrap();
            layers.push(json!({"layer": layer, "role": "wall", "wall_type": ty}));
        }
        s.execute("layer.current", &json!({"name": layer})).unwrap();
        let p = |k: &str| [w[k][0].as_f64().unwrap(), w[k][1].as_f64().unwrap()];
        let (a, b) = (p("a"), p("b"));
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let n = [-(b[1] - a[1]) / len * thick(ty) / 2.0, (b[0] - a[0]) / len * thick(ty) / 2.0];
        for k in [1.0, -1.0] {
            s.execute("line", &json!({"points": [[a[0] + k * n[0], a[1] + k * n[1]], [b[0] + k * n[0], b[1] + k * n[1]]]})).unwrap();
        }
    }
    layers
}

/// The PDF summary carries every category's total.
#[test]
fn the_pdf_summary_has_every_category_total() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    let (_, mut s, _) = read_set(&[]);
    let dir = std::env::temp_dir().join(format!("cadaddy-bom-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bom-summary.pdf");
    let r = s.execute("bim.bom", &json!({"file": path.to_string_lossy()})).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"%PDF"), "a PDF");
    let pages = cadaddy_io::pdf_read::page_count(&bytes).unwrap();
    let mut texts: Vec<String> = Vec::new();
    for page in 1..=pages {
        let opts = cadaddy_io::pdf_read::PdfOptions { page, images: false, markups: false, layout: false, ..Default::default() };
        let d = cadaddy_io::pdf_read::read(&bytes, &opts).unwrap().drawing;
        texts.extend(
            d.model.iter().filter_map(|e| if let cadaddy_doc::EntityKind::Text(t) = &e.kind { Some(t.value.trim().to_string()) } else { None }),
        );
    }
    let summary = r["summary"].as_array().unwrap();
    assert!(summary.len() >= 5, "{summary:?}");
    for c in summary {
        let (cat, total) = (c["category"].as_str().unwrap(), c["total"].to_string());
        assert!(texts.iter().any(|x| x == cat), "{cat} in the PDF: {texts:?}");
        assert!(texts.contains(&total), "{cat}'s total {total} in the PDF");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
