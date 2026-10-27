//! Q337: the roof and wall envelope read from the prints (roof drainage and flashing plan,
//! assemblies sheet, Division 07 spec), its takeoff and coordination flags, scored against the
//! truth.

use cadaddy_mepfix::envelope::{Model, flags, takeoff, tapered_thickness};
use cadaddy_mepfix::envelope_reader::{EnvScore, read_envelope, read_fixture_envelope, read_spec, score_envelope, spec_patch};
use cadaddy_mepfix::truth::*;

fn exact(s: &EnvScore) -> bool {
    s.items_exact == s.items_total
        && s.items_extra.is_empty()
        && s.flags_found == s.flags_total
        && s.flags_false.is_empty()
        && s.roof_layers_ok
        && s.claddings_ok
        && [s.ridges, s.overflows, s.pipes, s.walkways, s.callouts, s.details, s.datums].iter().all(|(a, b)| a == b)
}

#[test]
fn every_takeoff_line_and_flag_read_exactly() {
    // 1–6, and the first seed whose drainage plan is turned on its sheet.
    let rotated = (1..=60u64).find(|s| {
        cadaddy_mepfix::generate(*s)
            .is_ok_and(|set| set.truth.sheets.iter().any(|sh| sh.number == "A-104" && sh.model_from_building.rotation_deg != 0.0))
    });
    let mut seeds: Vec<u64> = (1..=6).collect();
    seeds.extend(rotated);
    assert!(rotated.is_some(), "some seed turns the drainage plan");
    for seed in seeds {
        let (t, r) = read_fixture_envelope(seed).unwrap();
        let s = score_envelope(&t, &r);
        assert!(s.items_total >= 25, "seed {seed}: {} takeoff lines", s.items_total);
        assert!(exact(&s), "seed {seed}: {s:?}");
        assert!(r.spec_read, "seed {seed}: spec");
        assert!(r.notes.is_empty(), "seed {seed}: {:?}", r.notes);
    }
}

#[test]
fn planted_gaps_are_flagged() {
    let mut seen = std::collections::BTreeSet::new();
    for seed in 1..=40u64 {
        let t = cadaddy_mepfix::truth_for(seed);
        let e = t.envelope.as_ref().unwrap();
        for p in &e.planted {
            let kind = if p == "missing_detail" { "no_curb_detail" } else { p.as_str() };
            assert!(e.flags.iter().any(|f| f.kind == kind), "seed {seed}: planted {p} not flagged: {:?}", e.flags);
            seen.insert(p.clone());
        }
        // Nothing planted: only natural flags (drain on beam, or r_value on shingle/metal roofs).
        if e.planted.is_empty() {
            assert!(e.flags.iter().all(|f| f.kind == "drain_structure" || f.kind == "r_value"), "seed {seed}: {:?}", e.flags);
        }
    }
    assert_eq!(seen.len(), 4, "{seen:?}");
}

/// Seeds 1–40 through the reader: every planted kind is flagged by the READER, no false flags
/// beyond natural drain/beam conflicts and r_value on shingle/metal roofs.
#[test]
fn reader_planted_gaps_are_flagged() {
    let mut seen = std::collections::BTreeSet::new();
    for seed in 1..=40u64 {
        let (t, r) = read_fixture_envelope(seed).unwrap();
        let e = t.envelope.as_ref().unwrap();
        for p in &e.planted {
            let kind = if p == "missing_detail" { "no_curb_detail" } else { p.as_str() };
            assert!(r.flags.iter().any(|f| f.kind == kind), "seed {seed}: reader did not flag planted {p}: {:?}", r.flags);
            seen.insert(p.clone());
        }
        // With nothing planted, only natural flags from the reader.
        if e.planted.is_empty() {
            let unexpected: Vec<&EnvFlag> = r.flags.iter().filter(|f| f.kind != "drain_structure" && f.kind != "r_value").collect();
            assert!(unexpected.is_empty(), "seed {seed}: unexpected reader flags: {unexpected:?}");
        }
    }
    assert_eq!(seen.len(), 4, "reader test saw all four plant kinds: {seen:?}");
}

#[test]
fn truth_for_matches_generate() {
    for seed in [2u64, 5] {
        let a = cadaddy_mepfix::truth_for(seed);
        let b = cadaddy_mepfix::generate(seed).unwrap();
        assert_eq!(a.envelope, b.truth.envelope);
        assert!(b.file("A-104.dxf").is_some() && b.file("A-504.dxf").is_some() && b.file("specs.txt").is_some());
    }
}

fn layer(component: &str, thickness_in: f64, count: u32) -> EnvLayer {
    EnvLayer { component: component.into(), description: component.to_uppercase(), thickness_in, count, ..Default::default() }
}

/// A 100 x 50 ft roof with a 36" parapet, one drain in the middle, one 4 x 4 ft curb (14"),
/// one pipe, a 20 ft walkway; a 100 x 50 ft building, levels at 0 / 12 / 24, top of parapet 27.
fn model() -> Model {
    Model {
        roof_min: [0.0, 0.0],
        roof_max: [100.0, 50.0],
        parapet_in: Some(36.0),
        slope_in_ft: 0.25,
        drains: vec![[50.0, 25.0]],
        curbs: vec![Curb { tag: "RTU-1".into(), kind: "equipment".into(), min_ft: [20.0, 35.0], max_ft: [24.0, 39.0], height_in: 14.0 }],
        overflows: vec![Overflow { kind: "scupper".into(), at_ft: [50.0, 0.0] }],
        pipes: vec![RoofPipe { tag: "VTR".into(), at_ft: [70.0, 10.0], size_in: 3.0 }],
        walkways: vec![Walkway { min_ft: [25.0, 34.0], max_ft: [45.0, 36.5] }],
        edge: "coping".into(),
        roof: RoofAssembly {
            tag: "R-1".into(),
            membrane: "tpo".into(),
            mil: Some(60.0),
            attachment: "fully adhered".into(),
            layers: vec![
                layer("membrane", 0.0, 1),
                layer("cover_board", 0.5, 1),
                layer("tapered", 0.5, 1),
                layer("insulation", 2.0, 2),
                layer("deck", 1.5, 1),
            ],
        },
        claddings: vec![Cladding {
            tag: "EW-1".into(),
            kind: "brick".into(),
            levels: vec!["1".into(), "2".into()],
            to_top: true,
            layers: vec![layer("cladding", 3.625, 1), layer("barrier", 0.0, 1)],
        }],
        datums: ["1", "2", "R", "P"].iter().zip([0.0, 12.0, 24.0, 27.0]).map(|(c, e)| Datum { code: c.to_string(), elevation_ft: e }).collect(),
        outline: vec![[0.0, 0.0], [100.0, 0.0], [100.0, 50.0], [0.0, 50.0]],
        openings: vec![cadaddy_mepfix::envelope::EnvOpening { kind: "window".into(), level: "1".into(), width_in: 48.0, height_in: 60.0 }],
        spec: Some(EnvSpec { membrane_mil: Some(80.0), min_r: 40.0, r_per_in: 5.7, metal_ga: 24 }),
        ..Model::default()
    }
}

fn net(items: &[QtyItem], code: &str) -> (f64, f64, String) {
    let i = items.iter().find(|i| i.code == code).unwrap_or_else(|| panic!("{code} missing: {items:?}"));
    (i.net, i.order, i.unit.clone())
}

#[test]
fn takeoff_by_hand() {
    let m = model();
    let q = takeoff(&m);
    // Field 5000 SF + parapet 300 LF x 3 ft + curb 16 LF x 14/12 ft = 5918.67 SF → 59.19 SQ, +10%.
    assert_eq!(net(&q, "roof.membrane"), (59.19, 66.0, "SQ".into()));
    assert_eq!(net(&q, "roof.insulation"), (10000.0, 10500.0, "SF".into()));
    assert_eq!(net(&q, "roof.coping").0, 300.0);
    assert_eq!(net(&q, "roof.parapet.base_flashing").0, 300.0);
    assert_eq!(net(&q, "roof.curbs.base_flashing").0, 16.0);
    // 20 ft of pads, 30" each → 8.
    assert_eq!(net(&q, "roof.walkway"), (8.0, 9.0, "EA".into()));
    assert_eq!(net(&q, "pen.total").0, 4.0);
    // Tapered: mean Manhattan distance to the centre of a 100 x 50 rectangle is 25 + 12.5 =
    // 37.5 ft → 0.5 + 0.25 x 37.5 = 9.875" average, 0.5 + 0.25 x 75 = 19.25" at the corners.
    let (avg, max) = tapered_thickness(&m, 0.5);
    assert!((avg - 9.875).abs() < 0.01 && (max - 19.25).abs() < 0.3, "{avg} {max}");
    // Walls: 300 ft x 27 ft = 8100 SF less a 20 SF window.
    assert_eq!(net(&q, "wall.EW-1.brick"), (8080.0, 8484.0, "SF".into()));
    // Sealant: 12 control joints x 27 ft + the window's 18 ft.
    assert_eq!(net(&q, "wall.sealant").0, 342.0);
    assert_eq!(net(&q, "wall.EW-1.sill_pan").0, 4.0);
    let f = flags(&m);
    let kinds: Vec<&str> = f.iter().map(|f| f.kind.as_str()).collect();
    // No callout for RTU-1, 60 vs 80 mil, (4 + 9.875) x 5.7 = R-79 ≥ R-40, RTU-1 wider than 2 ft
    // with no cricket; the drain has its scupper.
    assert_eq!(kinds, ["no_curb_detail", "spec_mil", "no_cricket"], "{f:?}");
    let mut thin = m.clone();
    thin.roof.layers.retain(|l| l.component != "insulation");
    thin.spec = Some(EnvSpec { min_r: 80.0, ..m.spec.clone().unwrap() });
    thin.overflows.clear();
    thin.beams = vec![([50.0, 0.0], [50.0, 50.0], "3".into())];
    let kinds: Vec<String> = flags(&thin).into_iter().map(|f| f.kind).collect();
    for k in ["r_value", "no_overflow", "drain_structure"] {
        assert!(kinds.iter().any(|x| x == k), "{k}: {kinds:?}");
    }
    // Q262: a rooftop unit at (80, 25) has no curb → no_curb flag.
    let mut q262 = m.clone();
    q262.rooftop_units = vec![
        ("AHU-1".into(), [22.0, 37.0]), // on RTU-1 curb → no flag
        ("AHU-2".into(), [80.0, 25.0]), // no curb here → no_curb flag
    ];
    let f262 = flags(&q262);
    assert!(f262.iter().any(|f| f.kind == "no_curb" && f.subject == "AHU-2"), "Q262 no_curb: {f262:?}");
    assert!(!f262.iter().any(|f| f.kind == "no_curb" && f.subject == "AHU-1"), "AHU-1 has a curb: {f262:?}");
}

/// An L-shaped building: 100 x 50, with a 40 x 20 notch cut from the top-right corner.
/// The closed-form outline: the L perimeter and area are known exactly.
///
///   0,50 ────────── 60,50
///     │                │
///     │                │ 60,30 ──── 100,30
///     │                           │
///   0,0  ──────────────────── 100,0
///
/// Perimeter: 100 + 30 + 40 + 20 + 60 + 50 = 300 ft
/// Area: 100*50 - 40*20 = 5000 - 800 = 4200 SF
#[test]
fn l_shaped_building_outline() {
    use cadaddy_mepfix::envelope::{polygon_perimeter, takeoff};
    // The L-shaped outline (counterclockwise from bottom-left).
    let outline = vec![[0.0, 0.0], [100.0, 0.0], [100.0, 30.0], [60.0, 30.0], [60.0, 50.0], [0.0, 50.0]];
    assert!((polygon_perimeter(&outline) - 300.0).abs() < 0.01, "perimeter {}", polygon_perimeter(&outline));
    let area = cadaddy_mepfix::envelope::polygon_area_pub(&outline);
    assert!((area - 4200.0).abs() < 0.01, "area {area}");

    // Build a model with this L-shaped outline and one cladding zone over one level.
    let mut m = model();
    m.outline = outline;
    // 1 level, 0→12 ft; coping goes up to top of parapet 15 ft. Zone height = 15 ft.
    m.datums = vec![
        Datum { code: "1".into(), elevation_ft: 0.0 },
        Datum { code: "R".into(), elevation_ft: 12.0 },
        Datum { code: "P".into(), elevation_ft: 15.0 },
    ];
    m.claddings = vec![Cladding {
        tag: "EW-1".into(),
        kind: "brick".into(),
        levels: vec!["1".into()],
        to_top: true,
        layers: vec![layer("cladding", 3.625, 1)],
    }];
    m.openings.clear();
    let q = takeoff(&m);
    // Wall perimeter = 300 ft, zone height = 15 ft (12 level + 3 parapet).
    // Gross wall SF = 300 * 15 = 4500 SF, no openings.
    let brick = q.iter().find(|i| i.code == "wall.EW-1.brick").unwrap();
    assert_eq!(brick.net, 4500.0, "L-shape brick net SF: {}", brick.net);
    assert_eq!(brick.unit, "SF");
}

#[test]
fn spec_patch_is_a_valid_building_spec() {
    let (_, r) = read_fixture_envelope(3).unwrap();
    let patch = spec_patch(&r, &|lv, p| (lv == "R").then_some([p[0] * 12.0, p[1] * 12.0]));
    let mut base = serde_json::json!({"levels": [{"name": "1", "elevation_ft": 0.0, "height_ft": 14.0}, {"name": "2", "elevation_ft": 14.0, "height_ft": 14.0}, {"name": "R", "elevation_ft": 28.0, "height_ft": 4.0, "ceiling_ft": 3.0}]});
    cadaddy_walk3d::merge(&mut base, &patch);
    let spec = cadaddy_walk3d::BuildingSpec::from_json(&base.to_string()).unwrap();
    let errors: Vec<_> = spec.check().into_iter().filter(|i| i.error).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let roof = spec.exterior.roofs.first().unwrap();
    assert_eq!(roof.assembly.as_ref().unwrap().layers.len(), r.model.roof.layers.len());
    assert_eq!(spec.exterior.facades.len(), 4);
    assert!(spec.exterior.facades.iter().all(|f| f.bands.iter().all(|b| b.assembly.is_some())));
    assert!(spec.describe().iter().any(|d| d.text.contains("top down")));
}

/// MUST 1: the envelope reader on a fixture set produces a takeoff that equals the truth's takeoff.
/// This is the test that proves the command would give truth-equal results once the DXF loading
/// pipeline is wired.
#[test]
fn fixture_envelope_read_equals_truth() {
    let seed = 3u64;
    let (t, r) = read_fixture_envelope(seed).unwrap();
    let s = score_envelope(&t, &r);
    assert_eq!(s.items_exact, s.items_total, "seed {seed}: not all items exact: {s:?}");
    assert!(s.items_extra.is_empty(), "seed {seed}: extra items: {:?}", s.items_extra);
    assert_eq!(s.flags_found, s.flags_total, "seed {seed}: flags: {s:?}");
    assert!(r.spec_read, "seed {seed}: spec not read");
    // Source/status: every takeoff item has a source, and at least one is "rule" (wall.sealant).
    assert!(
        r.takeoff.iter().all(|i| !i.source.is_empty() || i.status == "rule"),
        "seed {seed}: missing source: {:?}",
        r.takeoff.iter().find(|i| i.source.is_empty() && i.status != "rule")
    );
    assert!(r.takeoff.iter().any(|i| i.status == "rule"), "seed {seed}: no rule-status item");
}

#[test]
fn hostile_input_does_not_panic() {
    for junk in ["", "R-", "SECTION\nMEMBRANE 1e309 MIL\nR-NaN PER INCH", "\u{0}\u{ffff} GAUGE COPING -- AVERAGE R-9999999999999999999"] {
        let _ = read_spec(junk);
    }
    let map = cadaddy_mepfix::sheet_reader::SheetMap::default();
    let dxf = vec![("A-104.dxf".to_string(), b"0\nEOF\n".to_vec()), ("A-504.dxf".to_string(), vec![0xff; 64])];
    let r = read_envelope(&dxf, &map, &Default::default(), &Default::default(), &Default::default(), Some("junk"), &[]);
    assert!(r.takeoff.iter().all(|i| i.net.is_finite()));
    let mut m = model();
    m.roof_max = [f64::NAN, 1e300];
    m.drains = vec![[f64::INFINITY, 0.0]; 1000];
    m.datums.push(Datum { code: "2".into(), elevation_ft: f64::NAN });
    let _ = (takeoff(&m), flags(&m), tapered_thickness(&m, 0.5));
    m.roof_max = [1e9, 1e9];
    let (avg, max) = tapered_thickness(&m, 0.5);
    assert!(avg.is_finite() && max.is_finite(), "{avg} {max}");
    assert!(takeoff(&m).iter().all(|i| i.net.is_finite() && i.order.is_finite()));
}
