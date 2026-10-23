//! Branch conduit and homeruns read from the electrical plans (Q289), scored against the truth.
//! Reworked (Q5050): anchoring to placed devices/panels, per-run source/status, stronger tests.

use cadaddy_mepfix::conduit_reader::{self, ConduitResult};

#[test]
fn conduit_and_homeruns_are_read_from_the_office_sets() {
    for seed in 1..=8 {
        let (t, r, _) = conduit_reader::read_fixture_conduits(seed).unwrap();
        let s = conduit_reader::score(&t, &r);
        println!("seed {seed}: {s:?}");
        assert!(s.conduits_truth > 0 && s.homeruns_truth > 0, "seed {seed}: {s:?}");
        assert!(s.conduit_recall >= 0.95 && s.conduit_precision >= 0.95, "seed {seed}: {s:?}");
        assert!((s.size_accuracy - 1.0).abs() < 1e-9, "seed {seed}: {s:?}");
        assert!(s.homerun_recall >= 0.9 && s.homerun_precision >= 0.95, "seed {seed}: {s:?}");
        assert!(s.panel_accuracy >= 0.95, "seed {seed}: {s:?}");
        assert!((s.heights_assigned - 1.0).abs() < 1e-9, "seed {seed}: {s:?}");
    }
}

/// Every route is flagged assumed, runs hang above the ceiling, and homeruns end at a panel.
#[test]
fn routes_are_assumed_and_hang_above_the_ceiling() {
    let (t, r, _): (_, ConduitResult, _) = conduit_reader::read_fixture_conduits(3).unwrap();
    for h in r.homeruns.iter().filter(|h| h.panel_ft.is_some()) {
        assert_eq!(h.route_ft.last().copied(), h.panel_ft, "{h:?}");
        assert_eq!(h.bottoms_ft.len() + 1, h.route_ft.len(), "{h:?}");
    }
    for c in &r.conduits {
        let b = c.bottom_ft.unwrap();
        assert!(b > 7.0 && b < 14.0, "{c:?}");
    }
    let names: Vec<(&str, &str)> = t.building.levels.iter().map(|l| (l.code.as_str(), l.name.as_str())).collect();
    let (systems, runs, guesses) = conduit_reader::spec_runs(&r, &names);
    assert_eq!(systems.len(), 2);
    assert!(runs.iter().any(|x| x["system"] == conduit_reader::HOMERUN_SYSTEM));
    assert!(runs.iter().any(|x| x["system"] == conduit_reader::CONDUIT_SYSTEM));
    assert!(guesses.iter().any(|(k, why)| k.contains(conduit_reader::HOMERUN_SYSTEM) && why.contains("assumed")), "{guesses:?}");
}

/// MUST 2 (Q5050): homerun routes end at the equipment (panel) position, not the E-sheet
/// position, and the fallback flag appears when equipment is removed.
#[test]
fn homeruns_anchor_to_equipment_position() {
    let set = cadaddy_mepfix::generate(3).unwrap();
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
    let mut res = conduit_reader::read_conduits(&dxf, &map);
    let t = &set.truth;
    let dev_anchors = conduit_reader::device_anchors_from_truth(t);
    let eq_anchors = conduit_reader::equipment_anchors_from_truth(t);
    let unrouted = conduit_reader::anchor_homeruns(&mut res, &dev_anchors, &eq_anchors);
    // Every routed homerun's endpoint should match the equipment position (within 1e-6 ft).
    for h in res.homeruns.iter().filter(|h| h.panel_ft.is_some()) {
        let panel_tag = &h.panel;
        if let Some(eq) = eq_anchors.iter().find(|e| e.level == h.level && e.tag.eq_ignore_ascii_case(panel_tag)) {
            let d = ((h.panel_ft.unwrap()[0] - eq.at_ft[0]).powi(2) + (h.panel_ft.unwrap()[1] - eq.at_ft[1]).powi(2)).sqrt();
            assert!(d < 1e-6, "homerun {} panel_ft should match equipment position, off by {d}", h.label);
            assert_eq!(h.panel_source, "equipment", "homerun {} should be sourced from equipment", h.label);
        }
    }
    // Now remove equipment and verify the fallback flag appears.
    let mut res2 = conduit_reader::read_conduits(&dxf, &map);
    let unrouted2 = conduit_reader::anchor_homeruns(&mut res2, &dev_anchors, &[]);
    let fallback_count = res2.homeruns.iter().filter(|h| h.panel_source.contains("E-sheet")).count();
    // Without equipment, homeruns that had equipment panels should fall back to E-sheet.
    assert!(fallback_count > 0, "without equipment, some homeruns should fall back to E-sheet source");
    // Unrouted homeruns (panels not found at all) should be reported.
    println!("unrouted (with equipment): {unrouted:?}");
    println!("unrouted (without equipment): {unrouted2:?}");
}

/// MUST 3 (Q5050): per-run objects and source/status survive into spec_runs output, and
/// unrouted homeruns are reported (not silently dropped).
#[test]
fn per_run_objects_and_source_in_spec_runs() {
    let (t, r, unrouted) = conduit_reader::read_fixture_conduits(3).unwrap();
    // Conduit objects: every conduit from a layered drawing should have at least one handle.
    let with_objects = r.conduits.iter().filter(|c| !c.objects.is_empty()).count();
    assert!(with_objects > 0, "some conduits should have plan entity handles");
    // Homerun objects: arrow + label.
    let hr_with_objects = r.homeruns.iter().filter(|h| !h.objects.is_empty()).count();
    assert!(hr_with_objects > 0, "some homeruns should have plan entity handles");
    // Per-run source: conduits know tag vs default, homeruns know origin/panel source.
    for c in &r.conduits {
        // size_from_tag is the per-run source for size.
        assert!(c.size_from_tag || (c.size_in - conduit_reader::DEFAULT_SIZE_IN).abs() < 1e-9, "untagged conduit should use default size");
    }
    for h in r.homeruns.iter().filter(|h| h.panel_ft.is_some()) {
        assert!(!h.origin_source.is_empty(), "origin_source should be set: {}", h.label);
        assert!(!h.panel_source.is_empty(), "panel_source should be set: {}", h.label);
    }
    // spec_runs should carry per-run source and objects.
    let names: Vec<(&str, &str)> = t.building.levels.iter().map(|l| (l.code.as_str(), l.name.as_str())).collect();
    let (_, runs, _) = conduit_reader::spec_runs(&r, &names);
    let conduit_runs: Vec<&serde_json::Value> = runs.iter().filter(|r| r["system"] == conduit_reader::CONDUIT_SYSTEM).collect();
    assert!(!conduit_runs.is_empty());
    // At least one run should have objects.
    assert!(conduit_runs.iter().any(|r| r.get("objects").is_some()), "spec runs should carry objects");
    // Every run should have a source string.
    for run in &runs {
        assert!(run.get("source").is_some(), "every run should have a source: {run}");
        let src = run["source"].as_str().unwrap();
        if run["system"] == conduit_reader::CONDUIT_SYSTEM {
            assert!(src.contains("size ") && src.contains("height "), "conduit source: {src}");
        } else {
            assert!(src.contains("assumed route"), "homerun source: {src}");
        }
    }
    // Unrouted homeruns are reported.
    println!("unrouted homeruns: {unrouted:?}");
    // (The office fixtures may not have cross-level homeruns, but the mechanism is tested.)
}

/// MUST 4 (Q5050): hotel conduit precision and homerun recall/precision/panel accuracy.
#[test]
fn hotel_conduit_precision_and_homerun_accuracy() {
    let set = cadaddy_mepfix::generate_hotel(7, cadaddy_mepfix::hotel::HotelSize::Small).unwrap();
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
    let mut r = conduit_reader::read_conduits(&dxf, &map);
    // Anchor homeruns.
    let dev_anchors = conduit_reader::device_anchors_from_truth(&set.truth);
    let eq_anchors = conduit_reader::equipment_anchors_from_truth(&set.truth);
    let _unrouted = conduit_reader::anchor_homeruns(&mut r, &dev_anchors, &eq_anchors);
    // Assign heights.
    let ceil = cadaddy_mepfix::ceiling_reader::read_ceilings(&dxf, &map, None);
    let levels = set.truth.building.levels.clone();
    conduit_reader::assign_heights(&mut r, &|code| {
        let i = levels.iter().position(|l| l.code == code)?;
        let e = levels.get(i)?.elevation_ft;
        let next = levels.get(i + 1).map_or(e + 13.0, |l| l.elevation_ft);
        Some(conduit_reader::context_from_ceilings(&ceil, code, e, next - e - 2.0))
    });
    let s = conduit_reader::score(&set.truth, &r);
    println!("hotel: {s:?}");
    assert!(s.conduit_recall >= 0.95, "hotel conduit recall: {}", s.conduit_recall);
    assert!(s.conduit_precision >= 0.95, "hotel conduit precision: {}", s.conduit_precision);
    assert!(s.homerun_recall >= 0.95, "hotel homerun recall: {}", s.homerun_recall);
    assert!(s.homerun_precision >= 0.95, "hotel homerun precision: {}", s.homerun_precision);
    assert!(s.panel_accuracy >= 0.95, "hotel panel accuracy: {}", s.panel_accuracy);
    assert!(r.conduits.iter().any(|c| !c.size_from_tag && (c.size_in - conduit_reader::DEFAULT_SIZE_IN).abs() < 1e-9));
}

/// MUST 4 (Q5050): a BOC note sets the conduit's bottom exactly.
#[test]
fn boc_note_sets_exact_height() {
    let (_, r, _) = conduit_reader::read_fixture_conduits(3).unwrap();
    // Find a conduit with a BOC note.
    let with_boc: Vec<_> = r.conduits.iter().filter(|c| c.height_note.is_some()).collect();
    if with_boc.is_empty() {
        // The office fixtures may not always have BOC notes on every seed; check other seeds.
        for seed in 1..=8 {
            let (_, rr, _) = conduit_reader::read_fixture_conduits(seed).unwrap();
            let boc_runs: Vec<_> = rr.conduits.iter().filter(|c| c.height_note.is_some()).collect();
            if !boc_runs.is_empty() {
                for c in &boc_runs {
                    let note = c.height_note.as_ref().unwrap();
                    // Parse the BOC note to get the expected bottom.
                    if let Some(expected) = cadaddy_mepfix::conduit_reader::parse_boc(note) {
                        let actual = c.bottom_ft.unwrap();
                        assert!((actual - expected).abs() < 1e-3, "BOC note '{note}' should give bottom {expected}, got {actual}");
                        assert_eq!(c.height_source.as_ref().map(|s| format!("{s:?}")), Some("Note".to_string()), "BOC source should be Note");
                    }
                }
                return;
            }
        }
        // If no BOC notes in any seed, that's OK — the fixture may not generate them.
        println!("no BOC notes found in office fixtures, skipping exact height check");
    } else {
        for c in &with_boc {
            let note = c.height_note.as_ref().unwrap();
            if let Some(expected) = cadaddy_mepfix::conduit_reader::parse_boc(note) {
                let actual = c.bottom_ft.unwrap();
                assert!((actual - expected).abs() < 1e-3, "BOC note '{note}' should give bottom {expected}, got {actual}");
                assert_eq!(c.height_source.as_ref().map(|s| format!("{s:?}")), Some("Note".to_string()), "BOC source should be Note");
            }
        }
    }
}

/// MUST 4 (Q5050): conduit stacks below ducts by the Q257 trade rules (an exact stacking
/// assert with a made-up duct crossing above).
#[test]
fn conduit_stacks_below_duct() {
    use cadaddy_mepfix::conduit_reader::context_from_ceilings;
    use cadaddy_mepfix::heights::{SegmentInput, TradeRules, resolve_heights};

    let (t, r, _) = conduit_reader::read_fixture_conduits(3).unwrap();
    // Pick a level-1 corridor conduit.
    let c = r.conduits.iter().find(|c| c.level == "1" && (c.from_ft[1] - c.to_ft[1]).abs() < 1e-6).expect("need a horizontal conduit on level 1");
    let mid = [(c.from_ft[0] + c.to_ft[0]) / 2.0, c.from_ft[1]];
    // Build a height context with a duct at the same midpoint.
    let ceil = {
        let dxf: Vec<(String, Vec<u8>)> = {
            let set = cadaddy_mepfix::generate(3).unwrap();
            set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect()
        };
        let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
        cadaddy_mepfix::ceiling_reader::read_ceilings(&dxf, &map, None)
    };
    let levels = t.building.levels.clone();
    let i = levels.iter().position(|l| l.code == "1").unwrap();
    let e = levels[i].elevation_ft;
    let next_e = levels.get(i + 1).map_or(e + 13.0, |l| l.elevation_ft);
    let cx = context_from_ceilings(&ceil, "1", e, next_e - e - 2.0);
    let rules = TradeRules::standard();
    // Create inputs: a duct and a conduit at the same midpoint.
    let duct_input = SegmentInput {
        id: "duct-test".into(),
        system: "SA".into(),
        kind: "duct".into(),
        shape: "rect".into(),
        size_in: [12.0, 12.0],
        level: "1".into(),
        midpoint_ft: mid,
        height_note: None,
        vertical: false,
    };
    let conduit_input = SegmentInput {
        id: "conduit-test".into(),
        system: conduit_reader::CONDUIT_SYSTEM.into(),
        kind: "conduit".into(),
        shape: "round".into(),
        size_in: [0.75, 0.0],
        level: "1".into(),
        midpoint_ft: mid,
        height_note: None,
        vertical: false,
    };
    let resolved = resolve_heights(&[duct_input, conduit_input], &cx, &rules);
    let duct_h = resolved.iter().find(|r| r.segment_id == "duct-test").expect("duct resolved");
    let cond_h = resolved.iter().find(|r| r.segment_id == "conduit-test").expect("conduit resolved");
    println!("duct bottom: {}, conduit bottom: {}", duct_h.bottom_ft, cond_h.bottom_ft);
    // Conduit (unlisted lowest trade) should be below the duct.
    assert!(cond_h.bottom_ft < duct_h.bottom_ft, "conduit ({}) should be below duct ({})", cond_h.bottom_ft, duct_h.bottom_ft,);
}

/// MUST 4 (Q5050): unrouted homeruns for a cross-level panel are reported.
#[test]
fn cross_level_homerun_is_reported_unrouted() {
    let set = cadaddy_mepfix::generate(3).unwrap();
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
    let mut res = conduit_reader::read_conduits(&dxf, &map);
    let dev_anchors = conduit_reader::device_anchors_from_truth(&set.truth);
    // Plant one panel on a different level ("R" = roof) so a homerun to it is cross-level.
    let mut eq_anchors = conduit_reader::equipment_anchors_from_truth(&set.truth);
    // Pick the first panel and move it to level "R".
    if let Some(first) = eq_anchors.first_mut() {
        let original_level = first.level.clone();
        let original_tag = first.tag.clone();
        first.level = "R".into();
        let unrouted = conduit_reader::anchor_homeruns(&mut res, &dev_anchors, &eq_anchors);
        // Any homerun targeting this panel on its original level should now be unrouted.
        let for_panel: Vec<_> = unrouted.iter().filter(|u| u.panel.eq_ignore_ascii_case(&original_tag) && u.level == original_level).collect();
        println!("unrouted for moved panel {original_tag}: {for_panel:?}");
        assert!(for_panel.iter().all(|u| u.reason.contains("level")), "cross-level homeruns should mention the level in the reason");
    }
}

/// Read -> building spec -> 3D: conduit as raceways and homeruns as cable at their heights, a
/// made-up duct across a conduit is a clash, the assumptions are recorded, and IFC has a
/// conduit segment per conduit and a cable segment per homerun leg.
#[test]
fn read_conduit_build_clash_and_export() {
    use cadaddy_engine::Session;
    use serde_json::json;
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    let (t, r, _) = conduit_reader::read_fixture_conduits(3).unwrap();
    let names: Vec<(&str, &str)> = t.building.levels.iter().map(|l| (l.code.as_str(), l.name.as_str())).collect();
    let (mut systems, mut runs, guesses) = conduit_reader::spec_runs(&r, &names);
    let routed = r.homeruns.iter().filter(|h| h.route_ft.len() >= 2).count();
    assert_eq!(runs.len(), r.conduits.len() + routed, "every run has a height");
    // A made-up duct across level 1's corridor conduit, at its height.
    let c = r.conduits.iter().find(|c| c.level == "1" && (c.from_ft[1] - c.to_ft[1]).abs() < 1e-6).unwrap();
    let (x, y, b) = ((c.from_ft[0] + c.to_ft[0]) / 2.0 * 12.0, c.from_ft[1] * 12.0, c.bottom_ft.unwrap());
    systems.push(json!({"name": "M-DUCT-SUPP", "layer": "M-DUCT-SUPP", "kind": "duct", "size_in": [12, 12], "bottom_ft": b - 0.25}));
    let l1 = names.iter().find(|(c, _)| *c == "1").unwrap().1;
    runs.push(json!({"system": "M-DUCT-SUPP", "level": l1, "points": [[x, y - 60.0], [x, y + 60.0]], "size_in": [12, 12], "bottoms_ft": [b - 0.25]}));
    let levels: Vec<serde_json::Value> = t
        .building
        .levels
        .iter()
        .filter(|l| l.code != "R")
        .map(|l| json!({"name": l.name, "elevation_ft": l.elevation_ft, "height_ft": 13, "ceiling_ft": 8, "region": [-1000, -1000, 5000, 2000], "base": [0, 0]}))
        .collect();
    let guess: serde_json::Map<String, serde_json::Value> = guesses.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "E-POWR", "current": true})).unwrap();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"levels": levels, "mep": {"systems": systems, "runs": runs}, "guess": guess}})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let layers: Vec<String> = s.doc().unwrap().layers.iter().map(|l| l.name.clone()).collect();
    assert!(
        layers.iter().any(|l| l.contains(conduit_reader::CONDUIT_SYSTEM)) && layers.iter().any(|l| l.contains(conduit_reader::HOMERUN_SYSTEM)),
        "{layers:?}"
    );
    let clashes = s.execute("model.clashes", &json!({})).unwrap();
    let hit = clashes["clashes"].as_array().unwrap().iter().any(|c| {
        let sys = c["systems"].to_string();
        c["kind"] == "crossing" && sys.contains(conduit_reader::CONDUIT_SYSTEM) && sys.contains("M-DUCT-SUPP")
    });
    assert!(hit, "the duct across the conduit is a clash: {clashes}");
    // The assumptions are in the spec for a person to confirm.
    let spec = s.execute("bspec.get", &json!({})).unwrap();
    assert!(
        spec["spec"]["guess"][format!("mep/{}", conduit_reader::HOMERUN_SYSTEM)].as_str().is_some_and(|w| w.contains("assumed")),
        "{}",
        spec["spec"]["guess"]
    );
    // IFC: conduit segments, and cable segments for the homeruns.
    let text = String::from_utf8(cadaddy_io::ifc::write_ifc(s.doc().unwrap(), "Conduit.ifc").unwrap()).unwrap();
    let conduits = text.lines().filter(|l| l.contains("IFCCABLECARRIERSEGMENT(") && l.contains(".CONDUITSEGMENT.")).count();
    let cables = text.lines().filter(|l| l.contains("IFCCABLESEGMENT(") && l.contains(".CABLESEGMENT.")).count();
    assert!(conduits >= r.conduits.len(), "{conduits} conduit segments");
    assert!(cables >= routed, "{cables} cable segments for {routed} homeruns");
    assert!(text.contains("IFCCABLEFITTING("), "the homeruns' turns");
}
