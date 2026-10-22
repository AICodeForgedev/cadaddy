//! Underground and under-slab field drawings (Q330) on the synthetic hotel: the power service
//! from the site's underground electric line to the main switchboard, the level 1 panels' feeders
//! under the slab, a distribution panel at the far end whose feeder the drawings leave open, and
//! the telephone entrance only named in a note (not drawn). Every buried run and stub-up is on
//! the sheet and dimensioned from the grid; the telephone entrance is a validation failure and an
//! RFI before the slab; the open feeder is suggested under the slab, cheaper at the estimator's
//! prices.

use cadaddy_engine::Session;
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::truth::Truth;
use serde_json::{Value, json};

/// A sheet-units (inch) box of `w` × `h` feet centred on `c` (feet).
fn boxed(c: [f64; 2], w: f64, h: f64) -> (Value, Value) {
    (json!([(c[0] - w / 2.0) * 12.0, (c[1] - h / 2.0) * 12.0]), json!([(c[0] + w / 2.0) * 12.0, (c[1] + h / 2.0) * 12.0]))
}

fn centre(min: [f64; 2], max: [f64; 2]) -> [f64; 2] {
    [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0]
}

/// The hotel's spec for the underground: levels, the grid as columns, the level 1 gear, the
/// services, the buried runs, the slab pour and the estimator's prices.
fn spec(t: &Truth) -> Value {
    let levels: Vec<Value> = t
        .building
        .levels
        .iter()
        .filter(|l| l.code != "R")
        .map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 13, "ceiling_ft": 9}))
        .collect();
    let mut columns = Vec::new();
    for x in &t.building.grid.x {
        for y in &t.building.grid.y {
            columns
                .push(json!({"level": "1", "grid": format!("{}-{}", x.label, y.label), "at": [x.at_ft * 12.0, y.at_ft * 12.0], "size_in": [12, 12]}));
        }
    }
    let l1: Vec<_> = t.elec_panels.iter().filter(|p| p.level == "1").collect();
    assert!(!l1.is_empty(), "the hotel has level 1 panels");
    let mut equipment: Vec<Value> = l1
        .iter()
        .map(|p| json!({"tag": p.tag, "kind": "panel", "level": "1", "min": [p.min_ft[0] * 12.0, p.min_ft[1] * 12.0], "max": [p.max_ft[0] * 12.0, p.max_ft[1] * 12.0], "bottom_ft": 4, "height_ft": 4}))
        .collect();
    // The main switchboard in the level 1 electrical room; a distribution panel and two house
    // panels in the room farthest from it.
    let elec = t.building.rooms.iter().find(|r| r.level == "1" && r.name.to_uppercase().contains("ELEC")).expect("an electrical room on level 1");
    let msb = centre(elec.min_ft, elec.max_ft);
    let far = t
        .building
        .rooms
        .iter()
        .filter(|r| r.level == "1")
        .max_by(|a, b| {
            let d = |r: &&cadaddy_mepfix::truth::Room| {
                let c = centre(r.min_ft, r.max_ft);
                (c[0] - msb[0]).abs() + (c[1] - msb[1]).abs()
            };
            d(a).total_cmp(&d(b))
        })
        .unwrap();
    let dp = centre(far.min_ft, far.max_ft);
    for (tag, kind, c, w, h, bottom) in [
        ("MSB", "switchboard", msb, 4.0, 2.0, 0.0),
        ("DP-A", "panel", dp, 2.0, 1.0, 1.0),
        ("HA", "panel", [dp[0] + 3.0, dp[1]], 1.5, 0.5, 4.0),
        ("HB", "panel", [dp[0] - 3.0, dp[1]], 1.5, 0.5, 4.0),
    ] {
        let (min, max) = boxed(c, w, h);
        equipment.push(json!({"tag": tag, "kind": kind, "level": "1", "min": min, "max": max, "bottom_ft": bottom, "height_ft": 5}));
    }
    // The level 1 panels' feeders under the slab (routed: run ids are the feeder tags).
    let mut runs = Vec::new();
    let mut feeders = Vec::new();
    for p in &l1 {
        let c = centre(p.min_ft, p.max_ft);
        let tag = format!("F-{}", p.tag);
        runs.push(json!({"id": tag, "system": "E-UG", "level": "1", "points": [[msb[0] * 12.0, msb[1] * 12.0], [c[0] * 12.0, msb[1] * 12.0], [c[0] * 12.0, c[1] * 12.0]], "size_in": [1.5, 1.5], "bottoms_ft": [-2]}));
        feeders.push(json!({"tag": tag, "from": "MSB", "to": p.tag, "phases": 3, "size": "#1", "material": "cu", "neutrals": 1, "grounds": 1, "ground_size": "#6", "conduits": 1, "conduit_in": 1.5, "entry": "bottom"}));
    }
    // The open one: the distribution panel's feeder, and the house panels off it.
    feeders.push(json!({"tag": "F-DPA", "from": "MSB", "to": "DP-A", "phases": 3, "size": "4/0", "material": "cu", "neutrals": 1, "grounds": 1, "ground_size": "#4", "conduits": 1, "conduit_in": 2.5, "entry": "bottom"}));
    // Services: power from the site's underground electric line to the switchboard; telephone
    // named in a note only (the hidden one).
    let ug = |kind: &str| t.site_elements.iter().find(|e| e.kind == kind).unwrap_or_else(|| panic!("the hotel site has {kind}"));
    let el = ug("underground_electric");
    let tel = ug("underground_telecom");
    let start = |e: &cadaddy_mepfix::truth::SiteElement| {
        let lo = e.min_ft.unwrap_or(e.at_ft);
        json!([e.at_ft[0] * 12.0, lo[1] * 12.0])
    };
    let services = json!([
        {"kind": "power", "tag": "SVC-P", "level": "1", "from": start(el), "at": [msb[0] * 12.0, msb[1] * 12.0], "to": "MSB",
         "conduits": 4, "conduit_in": 4, "material": "PVC Sch 40", "cover_in": 30, "source": "C-101"},
        {"kind": "telephone", "tag": "SVC-T", "level": "1", "from": start(tel), "at": [tel.at_ft[0] * 12.0, 0], "to": "MDF",
         "conduits": 2, "conduit_in": 4, "shown": false, "source": "E-001 general note only"}
    ]);
    // The slab, its pour, and the estimator's prices (PVC under the slab, EMT overhead, wire).
    let ext = t.building.grid.x.iter().map(|g| g.at_ft).fold(0.0_f64, f64::max).max(1.0) * 12.0;
    let eyt = t.building.grid.y.iter().map(|g| g.at_ft).fold(0.0_f64, f64::max).max(1.0) * 12.0;
    let price = |item: &str, size: &str, material: f64, hours: f64| json!({"item": item, "size": size, "unit": "ft", "material": material, "labor_hours": hours, "date": "2026-09-01", "source": "Supplier quote"});
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": levels,
        "columns": columns,
        "mep": {
            "systems": [{"name": "E-UG", "layer": "E-UG", "kind": "conduit", "size_in": [1.5, 1.5], "bottom_ft": -2}],
            "runs": runs, "equipment": equipment, "feeders": feeders, "services": services,
            "required_services": ["power", "telephone"]
        },
        "concrete": {
            "elements": [{"id": "SOG-1", "kind": "slab", "level": "1", "min": [0, 0], "max": [ext, eyt], "depth_in": 5, "top_ft": 0, "owner": "concrete", "pour": "P-1"}],
            "pours": [{"id": "P-1", "name": "Slab on grade", "date": "2026-11-15"}]
        },
        "estimate": {
            "baseline": {"prices": [
                price("PVC conduit", "2-1/2\"", 3.1, 0.06), price("EMT conduit", "2-1/2\"", 6.4, 0.09),
                price("PVC conduit", "1-1/2\"", 1.4, 0.04), price("EMT conduit", "1-1/2\"", 2.9, 0.06),
                price("THHN copper", "#4/0", 9.8, 0.02), price("THHN copper", "#4", 1.9, 0.01), price("THHN copper", "#1", 4.6, 0.015),
                price("THHN copper", "#6", 1.2, 0.01)
            ]},
            "settings": {"crew_rate": 95}
        }
    })
}

fn session(t: &Truth) -> Session {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec(t)})).unwrap();
    s
}

#[test]
fn the_hotel_gets_its_underground_sheets() {
    let t = cadaddy_mepfix::hotel_truth_for(5, HotelSize::Small);
    let mut s = session(&t);
    let l1_panels = t.elec_panels.iter().filter(|p| p.level == "1").count();
    let r = s.execute("ugsheets.check", &json!({"today": "2026-11-01"})).unwrap();

    // Every buried run: the panel feeders and the power service; the telephone isn't drawn.
    let runs = r["runs"].as_array().unwrap();
    assert_eq!(runs.len(), l1_panels + 1, "{r}");
    let svc = runs.iter().find(|x| x["id"] == "SVC-P").unwrap();
    assert_eq!(
        (svc["kind"].clone(), svc["to"].clone(), svc["conduits"].clone(), svc["depth_in"].clone()),
        (json!("service"), json!("MSB"), json!(4), json!(30.0))
    );
    for run in runs.iter().filter(|x| x["kind"] == "run") {
        assert_eq!((run["from"].clone(), run["material"].clone()), (json!("MSB"), json!("PVC Sch 40")), "{run}");
        assert!(run["depth_in"].as_f64().unwrap() > 20.0 && run["length_ft"].as_f64().unwrap() > 0.0, "{run}");
    }
    // Every stub-up dimensioned from the grid, with what it feeds and coordinates.
    let stubs = r["stubups"].as_array().unwrap();
    // One at each panel; one group at the switchboard (every feeder and the service come up there).
    assert_eq!(stubs.len(), l1_panels + 1, "{r}");
    let at_msb: Vec<&Value> = stubs.iter().filter(|st| st["feeds"] == "MSB").collect();
    assert_eq!(at_msb.len(), 1, "{r}");
    assert_eq!(at_msb[0]["conduits"], json!(l1_panels + 4), "the feeders' conduits and the service's four: {r}");
    assert_eq!(at_msb[0]["mix"], json!([{"count": 4, "size_in": 4.0}, {"count": l1_panels, "size_in": 1.5}]), "{r}");
    for st in stubs {
        for axis in ["x", "y"] {
            assert!(st[axis]["from"].as_str().unwrap().starts_with("grid "), "{st}");
        }
        assert!(!st["feeds"].as_str().unwrap().is_empty() && st["coords_ft"].is_array(), "{st}");
    }
    assert!(stubs.iter().any(|st| st["feeds"] == "MSB" && st["material"] == "PVC Sch 80"));

    // The hidden telephone entrance is a validation failure.
    assert_eq!(r["ok"], json!(false));
    let tel: Vec<&Value> = r["findings"].as_array().unwrap().iter().filter(|f| f["check"] == "service_unclear").collect();
    assert_eq!(tel.len(), 1, "{r}");
    assert_eq!(tel[0]["about"], json!("SVC-T"));
    assert!(!r["findings"].as_array().unwrap().iter().any(|f| f["check"] == "service_missing"), "power is there: {r}");
    let services = r["services"].as_array().unwrap();
    assert_eq!(services.iter().find(|x| x["tag"] == "SVC-T").unwrap()["status"], json!("unclear"));

    // The open feeder is suggested under the slab, cheaper at the estimator's prices; the
    // routed ones aren't reviewed.
    let routes = r["routes"].as_array().unwrap();
    assert_eq!(routes.len(), 1, "only the open feeder: {r}");
    let dpa = &routes[0];
    assert_eq!(
        (dpa["feeder"].clone(), dpa["suggest"].clone(), dpa["basis"].clone()),
        (json!("F-DPA"), json!("under_slab"), json!("estimator prices and crew rate")),
        "{dpa}"
    );
    let (under, over) = (dpa["under_slab"]["cost"].as_f64().unwrap(), dpa["overhead"]["cost"].as_f64().unwrap());
    assert!(under < over && dpa["under_slab"]["length_ft"].as_f64() < dpa["overhead"]["length_ft"].as_f64(), "{dpa}");

    // The slab's hold points: the conduits under it.
    let holds = r["holds"].as_array().unwrap();
    assert!(holds.iter().any(|h| h["pour"] == "P-1"), "{r}");

    // The RFI before the slab, once.
    let rv = s.execute("ugsheets.review", &json!({})).unwrap();
    assert_eq!(rv["rfis"].as_array().unwrap().len(), 1, "{rv}");
    let again = s.execute("ugsheets.review", &json!({})).unwrap();
    assert!(again["rfis"].as_array().unwrap().is_empty());
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    let rfis = sp["spec"]["rfis"].as_array().cloned().unwrap_or_default();
    assert!(rfis.iter().any(|x| x["kind"] == "service_entrance" && x["severity"] == "high"), "{rfis:?}");

    // The sheet: every run and stub-up on it, each stub-up dimensioned both ways.
    let b = s.execute("ugsheets.build", &json!({"number": "E-010", "today": "2026-11-01"})).unwrap();
    let sheets = b["sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 1, "one slab level: {b}");
    let sh = &sheets[0];
    assert_eq!((sh["number"].clone(), sh["runs"].clone(), sh["stubups"].clone()), (json!("E-010"), json!(l1_panels + 1), json!(stubs.len())), "{sh}");
    assert_eq!(sh["dimensions"], json!(2 * stubs.len()), "{sh}");
    let layout = sh["layout"].as_str().unwrap().to_string();
    let d = s.doc().unwrap();
    let paper = d.space(&cadaddy_engine::doc::Space::Paper(layout.clone())).expect("the sheet's layout");
    let ents: Vec<_> = paper.iter().collect();
    let dims = ents.iter().filter(|e| matches!(e.kind, cadaddy_engine::doc::EntityKind::Dimension(_))).count();
    assert_eq!(dims, 2 * stubs.len());
    let texts: Vec<String> = ents
        .iter()
        .filter_map(|e| match &e.kind {
            cadaddy_engine::doc::EntityKind::Text(t) => Some(t.value.clone()),
            _ => None,
        })
        .collect();
    for want in ["UNDERGROUND RUN SCHEDULE", "STUB-UP SCHEDULE", "POUR HOLD POINTS - CONFIRM BEFORE CONCRETE", "SVC-P", "E-010"] {
        assert!(texts.iter().any(|x| x.contains(want)), "{want} on the sheet");
    }
    // The notes (wrapped over several lines).
    let all = texts.join(" ");
    let note = |start: &str| all.split(start).nth(1).map(|rest| rest.split(" 1. ").next().unwrap_or(rest).to_string()).unwrap_or_default();
    assert!(note("ROUTE F-DPA").contains("UNDER SLAB"), "the route suggestion is a note");
    assert!(note("SVC-T:").contains("RFI BEFORE THE SLAB"), "the hidden telephone entrance is a note");
    // Nothing drawn in the model.
    assert!(!d.model.iter().any(|e| e.common.layer.starts_with("E-UG-")));
}

#[test]
fn sheets_export_to_pdf_and_dwg() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
    });
    let t = cadaddy_mepfix::hotel_truth_for(5, HotelSize::Small);
    let mut s = session(&t);
    let dir = std::env::temp_dir().join(format!("cadaddy-ugsheets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (pdf, dwg) = (dir.join("underground.pdf"), dir.join("underground.dwg"));
    let b = s.execute("ugsheets.build", &json!({"pdf": pdf.to_string_lossy(), "dwg": dwg.to_string_lossy()})).unwrap();
    assert_eq!(b["files"].as_array().unwrap().len(), 2, "{b}");
    assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));
    assert!(std::fs::metadata(&dwg).unwrap().len() > 1000);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_building_with_no_services_fails_for_each_required_one() {
    let mut s = Session::new();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {"levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}], "mep": {"required_services": ["power", "telephone", "catv"]}}}),
    )
    .unwrap();
    let r = s.execute("ugsheets.check", &json!({})).unwrap();
    let missing: Vec<&str> =
        r["findings"].as_array().unwrap().iter().filter(|f| f["check"] == "service_missing").map(|f| f["about"].as_str().unwrap()).collect();
    assert_eq!(missing, vec!["power service", "telephone service", "catv service"]);
    // Hostile: a bad date, no spec.
    assert!(s.execute("ugsheets.check", &json!({"today": "soon"})).is_err());
    let mut empty = Session::new();
    assert!(empty.execute("ugsheets.check", &json!({})).is_err());
    // Built with nothing buried: still one sheet for the slab level, nothing to dimension.
    let b = s.execute("ugsheets.build", &json!({})).unwrap();
    assert_eq!(b["sheets"][0]["stubups"], json!(0), "{b}");
}
