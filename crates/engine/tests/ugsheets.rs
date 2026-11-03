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
    session_with(spec(t))
}

fn session_with(spec: Value) -> Session {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s
}

/// The switchboard's and the distribution panel's centres as [`spec`] places them (feet).
fn hub_points(t: &Truth) -> ([f64; 2], [f64; 2]) {
    let sp = spec(t);
    let eq = sp["mep"]["equipment"].as_array().unwrap().clone();
    let at = |tag: &str| {
        let e = eq.iter().find(|e| e["tag"] == tag).unwrap();
        let v = |k: &str, i: usize| e[k][i].as_f64().unwrap() / 12.0;
        [(v("min", 0) + v("max", 0)) / 2.0, (v("min", 1) + v("max", 1)) / 2.0]
    };
    (at("MSB"), at("DP-A"))
}

type Seg = ([f64; 2], [f64; 2]);

/// Whether two segments cross or touch.
fn segs_cross(a: Seg, b: Seg) -> bool {
    let cr = |o: [f64; 2], p: [f64; 2], q: [f64; 2]| (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]);
    let (d1, d2) = (cr(b.0, b.1, a.0), cr(b.0, b.1, a.1));
    let (d3, d4) = (cr(a.0, a.1, b.0), cr(a.0, a.1, b.1));
    d1 * d2 <= 0.0 && d3 * d4 <= 0.0
}

/// The least distance between two segments that don't cross.
fn seg_dist(a: Seg, b: Seg) -> f64 {
    let near = |p: [f64; 2], s: Seg| {
        let d = [s.1[0] - s.0[0], s.1[1] - s.0[1]];
        let l2 = d[0] * d[0] + d[1] * d[1];
        let t = if l2 > 0.0 { (((p[0] - s.0[0]) * d[0] + (p[1] - s.0[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
        (p[0] - s.0[0] - t * d[0]).hypot(p[1] - s.0[1] - t * d[1])
    };
    near(b.0, a).min(near(b.1, a)).min(near(a.0, b)).min(near(a.1, b))
}

/// The texts on a layout and where each is.
fn texts_of(s: &Session, layout: &str) -> Vec<(String, [f64; 2])> {
    let d = s.doc().unwrap();
    d.space(&cadaddy_engine::doc::Space::Paper(layout.to_string()))
        .unwrap()
        .iter()
        .filter_map(|e| match &e.kind {
            cadaddy_engine::doc::EntityKind::Text(t) => Some((t.value.clone(), [t.insert.x, t.insert.y])),
            _ => None,
        })
        .collect()
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
        // 2 ft below the floor to the bottom of a 1-1/2" conduit: its top is 24 - 1.5 = 22.5".
        assert_eq!(run["depth_in"], json!(22.5), "{run}");
        assert!(run["length_ft"].as_f64().unwrap() > 0.0, "{run}");
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
        // Each dimension is the stub-up's distance from the nearest grid line of the truth.
        for (axis, i, lines) in [("x", 0, &t.building.grid.x), ("y", 1, &t.building.grid.y)] {
            let at_ft = st["at"][i].as_f64().unwrap() / 12.0;
            let near = lines.iter().min_by(|a, b| (a.at_ft - at_ft).abs().total_cmp(&(b.at_ft - at_ft).abs())).unwrap();
            assert_eq!(st[axis]["from"], json!(format!("grid {}", near.label)), "{st}");
            let ft = st[axis]["ft"].as_f64().unwrap();
            assert!((ft - (at_ft - near.at_ft)).abs() <= 0.01, "{axis}: {ft} vs {at_ft} - {}: {st}", near.at_ft);
        }
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
    // By hand from the fixture: the run is MSB to DP-A centre to centre, square (feet); under
    // the slab 2 ft down and 1 ft up at each end, overhead up the 9 ft ceiling and down. Per
    // foot at $95/h: 2-1/2" PVC 3.10 + 0.06 h, EMT 6.40 + 0.09 h; three #4/0 and a #4/0
    // neutral at 9.80 + 0.02 h; a #4 ground at 1.90 + 0.01 h.
    let (msb, dp) = hub_points(&t);
    let run = (msb[0] - dp[0]).abs() + (msb[1] - dp[1]).abs();
    let wire = 4.0 * (9.8 + 0.02 * 95.0) + (1.9 + 0.01 * 95.0);
    let want_under = (run + 6.0) * ((3.1 + 0.06 * 95.0) + wire);
    let want_over = (run + 18.0) * ((6.4 + 0.09 * 95.0) + wire);
    assert!((under - want_under).abs() <= 1.0, "under {under} vs {want_under}");
    assert!((over - want_over).abs() <= 1.0, "overhead {over} vs {want_over}");

    // Bends: an L-shaped feeder's one turn and the 90 at each end; the service's 90 inside.
    let mut ells = 0;
    for run in runs.iter().filter(|x| x["kind"] == "run") {
        let id = run["id"].as_str().unwrap();
        let p = t.elec_panels.iter().find(|p| format!("F-{}", p.tag) == id).unwrap();
        let c = centre(p.min_ft, p.max_ft);
        if (c[0] - msb[0]).abs() > 0.01 && (c[1] - msb[1]).abs() > 0.01 {
            assert_eq!(run["bends_deg"], json!(270.0), "{run}");
            ells += 1;
        }
        assert_eq!(run["encased"], json!(false), "{run}");
    }
    assert!(ells > 0, "the hotel has L-shaped feeders");
    assert_eq!(svc["bends_deg"], json!(90.0), "{svc}");
    assert!(!r["findings"].as_array().unwrap().iter().any(|f| f["check"] == "bends" || f["check"] == "crossing"), "{r}");

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
    // The stub-ups' dimensions (the trench sections' are on their own layer).
    let dims =
        ents.iter().filter(|e| matches!(e.kind, cadaddy_engine::doc::EntityKind::Dimension(_)) && e.common.layer.starts_with("E-UG-DIMS")).count();
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
        read_dwf: None,
        write_dwfx: None,
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

/// Each buried electrical run's segments (feet), the power service as one straight segment.
fn buried_segments(sp: &Value) -> Vec<(String, Vec<Seg>)> {
    let ft = |q: &Value| [q[0].as_f64().unwrap() / 12.0, q[1].as_f64().unwrap() / 12.0];
    let mut out: Vec<(String, Vec<Seg>)> = sp["mep"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let p: Vec<[f64; 2]> = r["points"].as_array().unwrap().iter().map(ft).collect();
            (r["id"].as_str().unwrap().to_string(), p.windows(2).map(|w| (w[0], w[1])).collect())
        })
        .collect();
    let svc = &sp["mep"]["services"][0];
    out.push(("SVC-P".into(), vec![(ft(&svc["from"]), ft(&svc["at"]))]));
    out
}

#[test]
fn a_sewer_across_a_feeder_is_one_crossing_marked_on_the_sheet() {
    let t = cadaddy_mepfix::hotel_truth_for(5, HotelSize::Small);
    let mut sp = spec(&t);
    let buried = buried_segments(&sp);
    // A feeder's leg into its panel (along y, 10 ft or longer), and a sewer lateral coming
    // 6 ft from one side across its middle and ending 6" past it, where no other buried run
    // crosses it or comes within 15" (the check's clearance is 12").
    let (feeder, side, cross, sewer) = buried
        .iter()
        .flat_map(|(id, ss)| [(id, ss, -1.0), (id, ss, 1.0)])
        .find_map(|(id, ss, side): (&String, &Vec<Seg>, f64)| {
            let (a, b) = *ss.last()?;
            if (a[0] - b[0]).abs() > 1e-6 || (a[1] - b[1]).abs() < 10.0 {
                return None;
            }
            let at = [a[0], (a[1] + b[1]) / 2.0];
            let sewer: Seg = ([at[0] + 6.0 * side, at[1]], [at[0] - 0.5 * side, at[1]]);
            let alone =
                buried.iter().filter(|(o, _)| o != id).all(|(_, oss)| oss.iter().all(|o| !segs_cross(*o, sewer) && seg_dist(*o, sewer) > 1.25));
            alone.then(|| (id.clone(), side, at, sewer))
        })
        .expect("a feeder leg with nothing else near it");
    let route = json!([[sewer.0[0] * 12.0, sewer.0[1] * 12.0], [sewer.1[0] * 12.0, sewer.1[1] * 12.0]]);
    let sewer = json!({"kind": "sewer", "tag": "SS-1", "level": "1", "route": route, "at": route[1], "size_in": 6, "depth_ft": 4, "source": "C-201"});
    sp["mep"]["services"].as_array_mut().unwrap().push(sewer);
    let mut s = session_with(sp);
    let r = s.execute("ugsheets.check", &json!({})).unwrap();
    let xs: Vec<&Value> = r["findings"].as_array().unwrap().iter().filter(|f| f["check"] == "crossing").collect();
    assert_eq!(xs.len(), 1, "{r}");
    let x = xs[0];
    assert_eq!((x["items"].clone(), x["how"].clone()), (json!([feeder, "SS-1"]), json!("crosses")), "{x}");
    let at = [x["at"][0].as_f64().unwrap() / 12.0, x["at"][1].as_f64().unwrap() / 12.0];
    assert!((at[0] - cross[0]).hypot(at[1] - cross[1]) <= 1.0, "{at:?} vs {cross:?}");
    // The feeder's top 22.5" down and its bottom 24"; the sewer's top 4 ft down: 24" clear.
    assert_eq!((x["vertical_separation_in"].clone(), x["severity"].clone()), (json!(24.0), json!("warning")), "{x}");
    assert_eq!(r["crossings"].as_array().unwrap().len(), 1, "{r}");
    // The sewer isn't an electrical run.
    assert!(!r["runs"].as_array().unwrap().iter().any(|x| x["id"] == "SS-1"));

    // Marked on the sheet where it is, and listed.
    let b = s.execute("ugsheets.build", &json!({})).unwrap();
    let sh = &b["sheets"][0];
    assert_eq!(sh["crossings"], json!(1), "{sh}");
    let layout = sh["layout"].as_str().unwrap();
    let d = s.doc().unwrap();
    let paper = d.space(&cadaddy_engine::doc::Space::Paper(layout.to_string())).unwrap();
    let marks =
        paper.iter().filter(|e| matches!(e.kind, cadaddy_engine::doc::EntityKind::Circle(_)) && e.common.layer.starts_with("E-UG-XING")).count();
    assert_eq!(marks, 1);
    let texts = texts_of(&s, layout);
    for want in ["X-1", "CROSSINGS - POTHOLE AND VERIFY BEFORE TRENCHING"] {
        assert!(texts.iter().any(|(x, _)| x == want), "{want} on the sheet");
    }

    // A pipe below the floor (another trade's run) 6" beside the leg, on the side the sewer
    // came from: within a foot's clearance.
    let mut sp = spec(&t);
    let (x0, mid) = (cross[0] + 0.5 * side, cross[1]);
    sp["mep"]["systems"].as_array_mut().unwrap().push(json!({"name": "P-SAN", "layer": "P-SAN", "kind": "pipe", "size_in": [4, 4], "bottom_ft": -3}));
    sp["mep"]["runs"].as_array_mut().unwrap().push(json!({"id": "SAN-1", "system": "P-SAN", "level": "1",
        "points": [[x0 * 12.0, (mid - 1.0) * 12.0], [x0 * 12.0, (mid + 1.0) * 12.0]], "size_in": [4, 4], "bottoms_ft": [-3]}));
    let mut s = session_with(sp);
    let r = s.execute("ugsheets.check", &json!({"clearance_in": 12})).unwrap();
    let xs: Vec<&Value> = r["findings"].as_array().unwrap().iter().filter(|f| f["check"] == "crossing").collect();
    assert_eq!(xs.len(), 1, "{r}");
    assert_eq!(
        (xs[0]["items"].clone(), xs[0]["how"].clone(), xs[0]["distance_ft"].clone()),
        (json!([feeder, "SAN-1"]), json!("within clearance"), json!(0.5)),
        "{}",
        xs[0]
    );
    // The pipe's top 36 - 4 = 32" down, the feeder's bottom 24": 8" clear, under a foot.
    assert_eq!((xs[0]["vertical_separation_in"].clone(), xs[0]["severity"].clone()), (json!(8.0), json!("error")), "{}", xs[0]);
    let r = s.execute("ugsheets.check", &json!({"clearance_in": 3})).unwrap();
    assert!(!r["findings"].as_array().unwrap().iter().any(|f| f["check"] == "crossing"), "6\" apart is outside a 3\" clearance");
    assert!(s.execute("ugsheets.check", &json!({"clearance_in": -1})).is_err());
}

#[test]
fn bends_and_encasement_are_reported() {
    let t = cadaddy_mepfix::hotel_truth_for(5, HotelSize::Small);
    let mut sp = spec(&t);
    let (msb, _) = hub_points(&t);
    // A run with four turns out of the switchboard: 4 x 90, and 90 at each end that comes up.
    let m = [msb[0] * 12.0, msb[1] * 12.0];
    let zig = json!([m, [m[0], m[1] + 60.0], [m[0] + 60.0, m[1] + 60.0], [m[0] + 60.0, m[1] + 120.0], [m[0], m[1] + 120.0], [m[0], m[1] + 180.0]]);
    sp["mep"]["runs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "F-ZIG", "system": "E-UG", "level": "1", "points": zig, "size_in": [1, 1], "bottoms_ft": [-2], "encased": true}));
    sp["mep"]["services"][0]["encased"] = json!(true);
    let mut s = session_with(sp);
    let r = s.execute("ugsheets.check", &json!({})).unwrap();
    let runs = r["runs"].as_array().unwrap();
    let zig = runs.iter().find(|x| x["id"] == "F-ZIG").unwrap();
    let ends = if zig["to"].as_str().unwrap().is_empty() { 1.0 } else { 2.0 };
    let want = 360.0 + 90.0 * ends;
    assert_eq!((zig["bends_deg"].clone(), zig["encased"].clone()), (json!(want), json!(true)), "{zig}");
    let bends: Vec<&Value> = r["findings"].as_array().unwrap().iter().filter(|f| f["check"] == "bends").collect();
    assert_eq!(bends.len(), 1, "only F-ZIG is over 360: {r}");
    assert_eq!((bends[0]["about"].clone(), bends[0]["bends_deg"].clone()), (json!("F-ZIG"), json!(want)));
    let svc = runs.iter().find(|x| x["id"] == "SVC-P").unwrap();
    assert_eq!(svc["encased"], json!(true), "{svc}");
    assert_eq!(r["services"].as_array().unwrap().iter().find(|x| x["tag"] == "SVC-P").unwrap()["encased"], json!(true));

    // The run schedule: ENCASED on the service's row, and each row's bends.
    let b = s.execute("ugsheets.build", &json!({})).unwrap();
    let texts = texts_of(&s, b["sheets"][0]["layout"].as_str().unwrap());
    let row_of = |id: &str| texts.iter().find(|(x, _)| x == id).map(|(_, at)| at[1]).unwrap();
    let on_row = |y: f64, want: &str| texts.iter().any(|(x, at)| x == want && (at[1] - y).abs() < 1e-6);
    assert!(on_row(row_of("SVC-P"), "ENCASED"), "the service's row says ENCASED");
    assert!(on_row(row_of("SVC-P"), "90\u{b0}"), "the service's row has its bends");
    assert!(on_row(row_of("F-ZIG"), &format!("{want}\u{b0}")), "the zigzag's row has its bends");
    // The service's trench section draws the encasement.
    assert!(texts.iter().any(|(x, _)| x.starts_with("CONCRETE ENCASED")), "the section says encased");
}

#[test]
fn the_sheet_has_trench_sections_and_builds_again_in_place() {
    let t = cadaddy_mepfix::hotel_truth_for(5, HotelSize::Small);
    let mut s = session(&t);
    let b = s.execute("ugsheets.build", &json!({"number": "E-020"})).unwrap();
    let sh = b["sheets"][0].clone();
    assert_eq!((sh["sections"].clone(), sh["replaced"].clone()), (json!(2), json!(false)), "{sh}");
    let layout = sh["layout"].as_str().unwrap().to_string();
    let texts = texts_of(&s, &layout);
    for want in ["TYPICAL TRENCH SECTION - SERVICE", "TYPICAL TRENCH SECTION - UNDER SLAB"] {
        assert!(texts.iter().any(|(x, _)| x == want), "{want} on the sheet");
    }
    let all = texts.iter().map(|(x, _)| x.as_str()).collect::<Vec<_>>().join(" ");
    assert!(all.contains("4-4\" PVC Sch 40") && all.contains("WARNING TAPE") && all.contains("DIRECT BURIED"), "{all}");
    // The cover dimensioned from the spec: SVC-P's 30" is 2'-6"; the feeders' 22.5" 1'-11".
    let count = |s: &Session| {
        let d = s.doc().unwrap();
        let paper = d.space(&cadaddy_engine::doc::Space::Paper(layout.clone())).unwrap();
        let sect: Vec<String> = paper
            .iter()
            .filter(|e| e.common.layer.starts_with("E-UG-SECT"))
            .filter_map(|e| match &e.kind {
                cadaddy_engine::doc::EntityKind::Dimension(dm) => Some(dm.text.clone()),
                _ => None,
            })
            .collect();
        (paper.iter().count(), sect, d.layouts.len())
    };
    let (n1, sect, layouts1) = count(&s);
    assert_eq!(sect, vec!["2'-6\"".to_string(), "1'-11\"".to_string()]);

    // Again: the same layout, drawn afresh, nothing doubled.
    let b2 = s.execute("ugsheets.build", &json!({"number": "E-020"})).unwrap();
    let sh2 = &b2["sheets"][0];
    assert_eq!((sh2["layout"].as_str().unwrap(), sh2["replaced"].clone()), (layout.as_str(), json!(true)), "{sh2}");
    let (n2, sect2, layouts2) = count(&s);
    assert_eq!((layouts2, n2, sect2.len()), (layouts1, n1, 2));
    assert_eq!(sh2["dimensions"], sh["dimensions"]);
}

#[test]
fn a_short_hop_with_trenching_is_cheaper_overhead() {
    let t = cadaddy_mepfix::hotel_truth_for(5, HotelSize::Small);
    let mut sp = spec(&t);
    // HA to HB, 6 ft apart centre to centre, a #1 feeder in 1-1/2"; the estimator prices
    // trenching under the slab at $60 and half an hour a foot.
    sp["mep"]["feeders"].as_array_mut().unwrap().push(json!({"tag": "F-HAB", "from": "HA", "to": "HB", "phases": 3, "size": "#1", "material": "cu", "neutrals": 1, "grounds": 1, "ground_size": "#6", "conduits": 1, "conduit_in": 1.5}));
    sp["estimate"]["baseline"]["prices"]
        .as_array_mut()
        .unwrap()
        .push(json!({"item": "Trenching", "size": "", "unit": "ft", "material": 60.0, "labor_hours": 0.5}));
    let mut s = session_with(sp);
    let r = s.execute("ugsheets.check", &json!({})).unwrap();
    let hab = r["routes"].as_array().unwrap().iter().find(|x| x["feeder"] == "F-HAB").unwrap().clone();
    assert_eq!((hab["suggest"].clone(), hab["basis"].clone()), (json!("overhead"), json!("estimator prices and crew rate, with trenching")), "{hab}");
    // By hand at $95/h: three #1 and a #1 neutral at 4.60 + 0.015 h, a #6 ground at 1.20 +
    // 0.01 h; 12 ft of 1-1/2" PVC and 6 ft of trench, against 24 ft of 1-1/2" EMT.
    let wire = 4.0 * (4.6 + 0.015 * 95.0) + (1.2 + 0.01 * 95.0);
    let want_under = 12.0 * ((1.4 + 0.04 * 95.0) + wire) + 6.0 * (60.0 + 0.5 * 95.0);
    let want_over = 24.0 * ((2.9 + 0.06 * 95.0) + wire);
    assert_eq!((hab["under_slab"]["length_ft"].clone(), hab["overhead"]["length_ft"].clone()), (json!(12.0), json!(24.0)), "{hab}");
    assert!((hab["under_slab"]["cost"].as_f64().unwrap() - want_under).abs() <= 1.0, "{hab} vs {want_under}");
    assert!((hab["overhead"]["cost"].as_f64().unwrap() - want_over).abs() <= 1.0, "{hab} vs {want_over}");
    // Only under-slab suggestions are sheet notes.
    let b = s.execute("ugsheets.build", &json!({})).unwrap();
    let all = texts_of(&s, b["sheets"][0]["layout"].as_str().unwrap()).into_iter().map(|(x, _)| x).collect::<Vec<_>>().join(" ");
    assert!(!all.contains("ROUTE F-HAB"), "{all}");
}

#[test]
fn called_runs_are_solid_proposed_ones_dashed_and_notes_without_a_place_are_listed() {
    let t = cadaddy_mepfix::hotel_truth_for(5, HotelSize::Small);
    let mut sp = spec(&t);
    // One feeder CADaddy proposes under the slab; the rest the documents call for.
    let runs = sp["mep"]["runs"].as_array_mut().unwrap();
    let first = runs.iter_mut().find(|r| r["system"] == "E-UG").unwrap();
    first["height_source"] = json!("proposed");
    first["source_sheet"] = json!("feeder 604 MSB→P1");
    let id = first["id"].as_str().unwrap().to_string();
    let mut s = session_with(sp);
    let r = s.execute("ugsheets.check", &json!({"today": "2026-11-01"})).unwrap();
    let mine = r["runs"].as_array().unwrap().iter().find(|x| x["id"] == id.as_str()).unwrap().clone();
    assert_eq!((mine["proposed"].clone(), mine["basis"].clone()), (json!(true), json!("feeder 604 MSB→P1; proposed")), "{mine}");
    assert!(r["runs"].as_array().unwrap().iter().filter(|x| x["proposed"] == false).count() > 1, "{r}");
    let called = json!([{"sheet": "E-1", "text": "TWO (2) 5\" SCH. 40 PVC CONDUIT, MIN. 48\" BELOW GRADE."}]);
    let b = s.execute("ugsheets.build", &json!({"number": "E-030", "called": called})).unwrap();
    let layout = b["sheets"][0]["layout"].as_str().unwrap().to_string();
    let d = s.doc().unwrap();
    let paper: Vec<_> = d.space(&cadaddy_engine::doc::Space::Paper(layout.clone())).unwrap().iter().cloned().collect();
    let on = |layer: &str| {
        paper
            .iter()
            .filter(|e| {
                e.common.layer == layer && matches!(e.kind, cadaddy_engine::doc::EntityKind::LwPolyline(_) | cadaddy_engine::doc::EntityKind::Line(_))
            })
            .count()
    };
    assert!(on("E-UG-PROP") >= 1 && on("E-UG-COND") >= 1, "proposed and called runs on their own layers");
    let lt = |name: &str| d.layers.iter().find(|l| l.name == name).map(|l| l.linetype.clone()).unwrap_or_default();
    assert_eq!(lt("E-UG-PROP").to_ascii_uppercase(), "DASHED");
    assert_ne!(lt("E-UG-COND").to_ascii_uppercase(), "DASHED");
    let all = texts_of(&s, &layout).into_iter().map(|(x, _)| x).collect::<Vec<_>>().join(" ");
    for want in
        ["(PROPOSED)", "PROPOSED - RFI", "CALLED, NOT LOCATED ON ANY PLAN (E-1)", "REQUIRES ENGINEER OF RECORD REVIEW", "DASHED RUNS ARE NOT CALLED"]
    {
        assert!(all.contains(want), "{want} on the sheet: {all}");
    }
}
