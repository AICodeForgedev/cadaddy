//! Underground services read from the civil sheets (Q348) into `mep.services`, and the
//! underground sheets (`ugsheets`, Q330) built from them with no services given by hand.

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::site_services::{self, ReadService};
use cadaddy_mepfix::truth::{Sheet, Truth};
use serde_json::{Value, json};

/// The hotel's truth and the services read from its civil sheets (site plan, site utility plan).
fn read(seed: u64) -> (Truth, Vec<ReadService>) {
    let set = generate_hotel(seed, HotelSize::Small).unwrap_or_else(|e| panic!("generate_hotel({seed}): {e}"));
    let mut sheets = Vec::new();
    for number in ["C-101", "C-103"] {
        let Some(sh) = set.truth.sheets.iter().find(|s| s.number == number) else { continue };
        let bytes = set.file(&format!("{number}.dxf")).unwrap_or_else(|| panic!("{number}.dxf"));
        let d = cadaddy_io::read_dxf(bytes).unwrap_or_else(|e| panic!("{number}: {e}"));
        sheets.push(site_services::read_services(&d, sh));
    }
    assert!(!sheets.is_empty(), "seed {seed}: no civil sheets");
    (set.truth, site_services::merge(sheets))
}

fn near(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) <= tol
}

#[test]
fn the_hotels_underground_services_come_off_the_civil_sheets() {
    for seed in 1..=3 {
        let (truth, found) = read(seed);
        let expected: Vec<_> = truth.site_elements.iter().filter(|e| e.kind.starts_with("underground_")).collect();
        assert_eq!(found.len(), expected.len(), "seed {seed}: {found:#?}");
        for e in &expected {
            let kind = match e.kind.trim_start_matches("underground_") {
                "electric" => "power",
                "telecom" => "telephone",
                k => k,
            };
            let s = found.iter().find(|s| s.kind == kind).unwrap_or_else(|| panic!("seed {seed}: no {kind} in {found:#?}"));
            // The route: from the site end to the end at the building.
            let (Some(lo), Some(hi)) = (e.min_ft, e.max_ft) else { panic!("seed {seed}: {} has no route", e.kind) };
            let (first, last) = (s.route_ft.first().copied().unwrap_or_default(), s.route_ft.last().copied().unwrap_or_default());
            assert!(near(first, lo, 0.5) && near(last, hi, 0.5), "seed {seed} {kind}: {:?} vs {lo:?} → {hi:?}", s.route_ft);
            assert!(s.enters, "seed {seed} {kind}");
            assert_eq!(s.depth_ft, e.depth_ft, "seed {seed} {kind}: depth");
            assert_eq!(s.size_in, None, "seed {seed} {kind}: the fixture prints no size");
            assert_eq!(s.sheet, "C-101", "seed {seed} {kind}");
            assert!(s.found_in[0][0] <= s.found_in[1][0] && s.found_in[0][1] < s.found_in[1][1], "seed {seed} {kind}: {:?}", s.found_in);
            assert!((s.confidence - 0.9).abs() < 1e-9, "seed {seed} {kind}: {}", s.confidence);
        }
        assert!(found.iter().all(|s| s.kind != "unknown"), "seed {seed}");
    }
}

/// The hotel's spec for the underground with nothing about services but what the reader found:
/// levels, the grid, the main switchboard, the slab.
fn spec(t: &Truth, services: Value) -> Value {
    let levels: Vec<Value> = t
        .building
        .levels
        .iter()
        .filter(|l| l.code != "R")
        .map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 13}))
        .collect();
    let mut columns = Vec::new();
    for x in &t.building.grid.x {
        for y in &t.building.grid.y {
            columns
                .push(json!({"level": "1", "grid": format!("{}-{}", x.label, y.label), "at": [x.at_ft * 12.0, y.at_ft * 12.0], "size_in": [12, 12]}));
        }
    }
    let elec = t.building.rooms.iter().find(|r| r.level == "1" && r.name.to_uppercase().contains("ELEC")).expect("an electrical room on level 1");
    let c = [(elec.min_ft[0] + elec.max_ft[0]) * 6.0, (elec.min_ft[1] + elec.max_ft[1]) * 6.0];
    let ext = t.building.grid.x.iter().map(|g| g.at_ft).fold(1.0_f64, f64::max) * 12.0;
    let eyt = t.building.grid.y.iter().map(|g| g.at_ft).fold(1.0_f64, f64::max) * 12.0;
    json!({
        "version": 1, "units_per_foot": 12, "levels": levels, "columns": columns,
        "mep": {
            "equipment": [{"tag": "MSB", "kind": "switchboard", "level": "1", "min": [c[0] - 24.0, c[1] - 12.0], "max": [c[0] + 24.0, c[1] + 12.0], "bottom_ft": 0, "height_ft": 7}],
            "services": services["services"], "required_services": ["power", "telephone"]
        },
        "concrete": {
            "elements": [{"id": "SOG-1", "kind": "slab", "level": "1", "min": [0, 0], "max": [ext, eyt], "depth_in": 5, "top_ft": 0, "owner": "concrete", "pour": "P-1"}],
            "pours": [{"id": "P-1", "name": "Slab on grade", "date": "2026-11-15"}]
        }
    })
}

#[test]
fn ugsheets_builds_from_the_read_services_with_no_manual_input() {
    let (truth, found) = read(5);
    let patch = site_services::spec_patch(&found, "1", &|_, p| Some([p[0] * 12.0, p[1] * 12.0]));
    assert_eq!(patch["services"].as_array().map(Vec::len), Some(6), "{patch}");
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec(&truth, patch)})).unwrap();
    let r = s.execute("ugsheets.check", &json!({"today": "2026-11-01"})).unwrap();
    let services = r["services"].as_array().unwrap();
    assert_eq!(services.len(), 6, "{r}");
    for kind in ["power", "telephone"] {
        assert!(services.iter().any(|v| v["kind"] == kind && v["status"] == "shown"), "{kind}: {services:?}");
    }
    let findings = r["findings"].as_array().unwrap();
    assert!(!findings.iter().any(|f| matches!(f["check"].as_str(), Some("service_missing" | "service_unclear"))), "{findings:#?}");
    // Only electrical duct-bank services (power, telephone) become buried conduit runs.
    let service_runs: Vec<&Value> = r["runs"].as_array().unwrap().iter().filter(|v| v["kind"] == "service").collect();
    assert_eq!(service_runs.len(), 2, "only power and telephone should be conduit runs: {service_runs:#?}");
    for run in &service_runs {
        let sys = run["system"].as_str().unwrap();
        assert!(sys == "power" || sys == "telephone", "unexpected conduit run for {sys}");
    }
    // Pipe services (water, sewer, storm, gas) must NOT be conduit runs, must have no material,
    // and must not have stub-ups.
    for pipe in ["water", "sewer", "storm", "gas"] {
        assert!(!service_runs.iter().any(|v| v["system"].as_str() == Some(pipe)), "{pipe} must not be a conduit run");
    }
    let stubs = r["stubups"].as_array().unwrap();
    for pipe in ["water", "sewer", "storm", "gas"] {
        assert!(
            !stubs.iter().any(|st| { st["run"].as_str().is_some_and(|r| r.to_ascii_lowercase().contains(pipe)) }),
            "{pipe} must not have a stub-up"
        );
    }
    // No service_size finding should name a pipe service.
    for f in findings {
        if f["check"].as_str() == Some("service_size") {
            let msg = f["message"].as_str().unwrap_or("");
            for pipe in ["water", "sewer", "storm", "gas"] {
                assert!(!msg.contains(pipe), "service_size must not name {pipe}: {msg}");
            }
        }
    }
    // Power run: route length and depth still match the truth.
    let el = truth.site_elements.iter().find(|e| e.kind == "underground_electric").unwrap();
    let power = service_runs.iter().find(|v| v["system"] == "power").unwrap();
    let length = el.min_ft.zip(el.max_ft).map(|(a, b)| (b[0] - a[0]).hypot(b[1] - a[1])).unwrap();
    assert!((power["length_ft"].as_f64().unwrap() - length).abs() < 0.5, "{power} vs {length}");
    assert_eq!(power["depth_in"], json!(36.0), "{power}");
    let sheets = s.execute("ugsheets.build", &json!({"today": "2026-11-01"})).unwrap();
    let built = sheets["sheets"].as_array().unwrap();
    assert!(!built.is_empty(), "{sheets}");
    // At least 2 runs (power + telephone); pipe services are on the other_trade coordination layer, not runs.
    assert!(built.iter().any(|b| b["runs"].as_u64().unwrap_or(0) >= 2), "{sheets}");
}

/// A sheet drawn in inches at full size (building feet = inches / 12).
fn sheet() -> Sheet {
    Sheet { number: "C-900".into(), ..Sheet::default() }
}

fn text(layer: &str, at: [f64; 2], s: &str) -> String {
    format!("0\nTEXT\n8\n{layer}\n10\n{}\n20\n{}\n30\n0\n40\n6\n1\n{s}\n", at[0], at[1])
}

fn line(layer: &str, a: [f64; 2], b: [f64; 2]) -> String {
    format!("0\nLINE\n8\n{layer}\n10\n{}\n20\n{}\n30\n0\n11\n{}\n21\n{}\n31\n0\n", a[0], a[1], b[0], b[1])
}

#[test]
fn printed_sizes_inverts_bends_and_unknown_lines() {
    let mut e = String::new();
    // The building: 0..1200 × 0..600 inches.
    e += "0\nLWPOLYLINE\n8\nC-SITE\n90\n4\n70\n1\n10\n0\n20\n0\n10\n1200\n20\n0\n10\n1200\n20\n600\n10\n0\n20\n600\n";
    e += &text("C-SITE", [600.0, 300.0], "BUILDING");
    // A water line with a bend, drawn as two lines from the building out, labelled with its size
    // and invert.
    e += &line("C-UNDR", [300.0, 0.0], [300.0, -600.0]);
    e += &line("C-UNDR", [300.0, -600.0], [-300.0, -600.0]);
    e += &text("C-UNDR", [310.0, -300.0], "6\" WATER");
    e += &text("C-UNDR", [310.0, -340.0], "INV 95.50");
    // An electric duct bank as one polyline from the street in, conduits, size and depth.
    e += "0\nLWPOLYLINE\n8\nC-UNDR\n90\n3\n70\n0\n10\n900\n20\n-1200\n10\n900\n20\n-300\n10\n900\n20\n-12\n";
    e += &text("C-UNDR", [910.0, -600.0], "4-4\" PVC ELECTRIC");
    e += &text("C-UNDR", [910.0, -640.0], "3' DEEP");
    // A line whose label names nothing — far from the building (both ends > tolerance).
    e += &line("C-UNDR", [1500.0, -1200.0], [1500.0, -300.0]);
    e += &text("C-UNDR", [1510.0, -700.0], "BY OTHERS");
    // A C-UNDR line away from the building (both ends > 10 ft / 120 inches from the box).
    e += &line("C-UNDR", [-600.0, -1800.0], [-600.0, -900.0]);
    e += &text("C-UNDR", [-590.0, -1400.0], "STORM");
    let dxf = format!("0\nSECTION\n2\nENTITIES\n{e}0\nENDSEC\n0\nEOF\n");
    let d = cadaddy_io::read_dxf(dxf.as_bytes()).unwrap();
    let found = site_services::read_services(&d, &sheet());
    assert_eq!(found.len(), 4, "{found:#?}");
    let water = found.iter().find(|s| s.kind == "water").unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(water.size_in, Some(6.0));
    assert_eq!(water.invert_ft, Some(95.5));
    assert_eq!(water.route_ft, vec![[-25.0, -50.0], [25.0, -50.0], [25.0, 0.0]], "joined, from the site end to the building");
    assert!((water.confidence - 1.0).abs() < 1e-9, "{}", water.confidence);
    let power = found.iter().find(|s| s.kind == "power").unwrap();
    assert_eq!((power.conduits, power.size_in, power.depth_ft), (Some(4), Some(4.0), Some(3.0)));
    assert_eq!(power.route_ft.last(), Some(&[75.0, -1.0]));
    assert_eq!(power.found_in, [[900.0, -1200.0], [900.0, -12.0]]);
    // BY OTHERS: unknown, far from the building → enters must be false.
    let unknown = found.iter().find(|s| s.kind == "unknown").unwrap();
    assert_eq!(unknown.labels, vec!["BY OTHERS".to_string()]);
    assert!(unknown.size_in.is_none() && unknown.depth_ft.is_none());
    assert!((unknown.confidence - 0.2).abs() < 1e-9);
    assert!(!unknown.enters, "BY OTHERS far from the building must not have enters=true");
    // Storm line away from the building: no enters.
    let far_storm = found.iter().find(|s| s.kind == "storm").unwrap();
    assert!(!far_storm.enters, "storm line far from the building must not have enters=true");
    // Into the spec: the duct bank's size is its conduits'; the water's stays the pipe's.
    let patch = site_services::spec_patch(&found, "1", &|_, p| Some([p[0] * 12.0, p[1] * 12.0]));
    let svc = |k: &str| patch["services"].as_array().unwrap().iter().find(|v| v["kind"] == k).cloned().unwrap();
    assert_eq!(svc("power")["conduit_in"], json!(4.0));
    assert_eq!(svc("power")["conduits"], json!(4));
    assert_eq!(svc("power")["cover_in"], json!(36.0));
    assert_eq!(svc("power")["at"], json!([900.0, -12.0]));
    assert!(svc("water").get("conduit_in").is_none());
    assert_eq!(svc("water")["size_in"], json!(6.0));
    assert_eq!(svc("unknown")["source"], json!("C-900"));
    // BY OTHERS: unknown → shown: false and no "at" in the patch.
    assert_eq!(svc("unknown")["shown"], json!(false), "unknown must not be shown");
    assert!(svc("unknown").get("at").is_none(), "unknown must have no at");
    // Storm far from building: no "at" in the patch (enters was false).
    assert!(svc("storm").get("at").is_none(), "storm far from building must have no at");
    // Without a building on the sheet a route is read but doesn't say where it enters.
    let bare = dxf.replace("BUILDING", "PARKING");
    let found = site_services::read_services(&cadaddy_io::read_dxf(bare.as_bytes()).unwrap(), &sheet());
    assert!(found.iter().all(|s| !s.enters), "{found:#?}");
    let patch = site_services::spec_patch(&found, "1", &|_, p| Some(p));
    assert!(patch["services"].as_array().unwrap().iter().all(|v| v.get("at").is_none()), "{patch}");
}

#[test]
fn a_sheet_with_nothing_underground_reads_nothing() {
    let d = cadaddy_io::read_dxf(b"0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n").unwrap();
    assert!(site_services::read_services(&d, &sheet()).is_empty());
    let patch = site_services::spec_patch(&[], "1", &|_, p| Some(p));
    assert_eq!(patch, json!({"services": []}));
}
