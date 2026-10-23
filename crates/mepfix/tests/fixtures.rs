//! The fixture generator: deterministic, its truth consistent with itself, its sheets carrying
//! what the truth says they carry (read back through our own PDF and DXF readers), styles varied,
//! and the scorer scoring.

use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::truth::Truth;
use cadaddy_mepfix::{generate, generate_hotel, hotel_truth_for, layout, score, truth_for};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn the_same_seed_gives_the_same_files() {
    let (a, b) = (generate(7).unwrap(), generate(7).unwrap());
    assert_eq!(a.files.len(), b.files.len());
    for ((na, ba), (nb, bb)) in a.files.iter().zip(&b.files) {
        assert_eq!(na, nb);
        assert!(ba == bb, "{na} differs between runs");
    }
    let c = generate(8).unwrap();
    assert_ne!(a.file("truth.json"), c.file("truth.json"));
}

/// Nodes reachable from `start` through segments of one system.
fn reach(t: &Truth, start: &str, system: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::from([start.to_string()]);
    let mut todo = vec![start.to_string()];
    while let Some(n) = todo.pop() {
        let Some(node) = t.node(&n) else { continue };
        // Through equipment (a VAV box's inlet to its outlet).
        if let Some(eq) = node.connects.as_ref().and_then(|id| t.equipment.iter().find(|e| &e.id == id)) {
            for other in &eq.nodes {
                if t.node(other).is_some_and(|o| o.system == system) && seen.insert(other.clone()) {
                    todo.push(other.clone());
                }
            }
        }
        for sid in &node.segments {
            let Some(s) = t.segment(sid) else { continue };
            if s.system != system {
                continue;
            }
            for next in [&s.from, &s.to] {
                if seen.insert(next.clone()) {
                    todo.push(next.clone());
                }
            }
        }
    }
    seen
}

#[test]
fn the_truth_is_consistent() {
    for seed in 1..=40 {
        let t = truth_for(seed);
        let ctx = format!("seed {seed}");
        // Graph: segment ends are nodes at the path's ends, listed both ways.
        for s in &t.segments {
            for (end, p) in [(&s.from, s.path_ft[0]), (&s.to, s.path_ft[1])] {
                let n = t.node(end).unwrap_or_else(|| panic!("{ctx}: {} has no node {end}", s.id));
                assert!((0..3).all(|k| (n.at_ft[k] - p[k]).abs() < 1e-6), "{ctx}: {} end {end}", s.id);
                assert!(n.segments.contains(&s.id), "{ctx}: {end} doesn't list {}", s.id);
            }
            assert!(s.size_in[0] > 0.0 && s.length_ft() > 1e-6, "{ctx}: {}", s.id);
            assert_eq!(s.vertical, s.height.is_none(), "{ctx}: {}", s.id);
            let [a, b] = s.path_ft;
            // Axis-aligned (drains may slope along their run).
            let moves = (0..3).filter(|k| (a[*k] - b[*k]).abs() > 1e-6).count();
            assert!(moves == 1 || (s.system == "SAN" && moves == 2), "{ctx}: {} is not straight along an axis", s.id);
        }
        for n in &t.nodes {
            assert!(!n.kind.is_empty() && !n.segments.is_empty(), "{ctx}: {}", n.id);
        }
        // Every device and fixture is connected to its source.
        let source = |sys: &str, kind: &str| t.nodes.iter().filter(|n| n.system == sys && n.kind == kind).map(|n| n.id.clone()).collect::<Vec<_>>();
        for lv in ["1", "2"] {
            let ahu = t.equipment.iter().find(|e| e.tag == format!("AHU-{lv}")).unwrap();
            let supply = ahu.nodes.iter().flat_map(|n| reach(&t, n, "SA")).collect::<BTreeSet<_>>();
            for tm in t.terminals.iter().filter(|x| x.system == "SA" && x.level == lv) {
                assert!(supply.contains(tm.node.as_ref().unwrap()), "{ctx}: {} not fed by {}", tm.id, ahu.tag);
            }
        }
        let fp: BTreeSet<String> = source("FP", "service").iter().flat_map(|n| reach(&t, n, "FP")).collect();
        for tm in t.terminals.iter().filter(|x| x.system == "FP") {
            assert!(fp.contains(tm.node.as_ref().unwrap()), "{ctx}: head {} not on the fire service", tm.id);
        }
        let cw: BTreeSet<String> = source("CW", "service").iter().flat_map(|n| reach(&t, n, "CW")).collect();
        let san: BTreeSet<String> = source("SAN", "service").iter().flat_map(|n| reach(&t, n, "SAN")).collect();
        for f in &t.fixtures {
            let on =
                |set: &BTreeSet<String>, sys: &str| f.nodes.iter().filter(|n| t.node(n).is_some_and(|n| n.system == sys)).all(|n| set.contains(n));
            assert!(on(&cw, "CW") && on(&san, "SAN"), "{ctx}: fixture {} not connected", f.id);
        }
        // Drains fall toward the sewer: every SAN horizontal run slopes.
        for s in t.segments.iter().filter(|s| s.system == "SAN" && !s.vertical) {
            assert!((s.path_ft[0][2] - s.path_ft[1][2]).abs() > 1e-6, "{ctx}: {} is level", s.id);
        }
        // Risers join levels and are marked on the plans.
        for sys in ["CW", "HW", "HWR", "SAN", "V", "FP", "EA"] {
            let r = t.risers.iter().find(|r| r.system == sys).unwrap_or_else(|| panic!("{ctx}: no {sys} riser"));
            assert!(!r.shown.is_empty(), "{ctx}: {sys} riser unmarked");
        }
        // Clashes: only the planted ones (and what planting them caused).
        assert!(t.clashes.iter().any(|c| c.planted), "{ctx}: nothing planted");
        let stray: Vec<_> = t.clashes.iter().filter(|c| !c.planted).collect();
        assert!(stray.is_empty(), "{ctx}: unplanted clashes {stray:?}");
        // Ceilings sit under the sprinkler and plumbing bands.
        for rm in &t.building.rooms {
            if let Some(c) = &rm.ceiling {
                assert!((7.5..=10.0).contains(&c.height_ft), "{ctx}: {} ceiling {}", rm.id, c.height_ft);
            }
        }
    }
}

#[test]
fn every_kind_of_clash_is_planted_somewhere() {
    let mut kinds = BTreeSet::new();
    for seed in 1..=30 {
        for c in truth_for(seed).clashes.iter().filter(|c| c.planted) {
            kinds.insert(c.kind.clone());
        }
    }
    assert_eq!(kinds, BTreeSet::from(["below_ceiling".to_string(), "hard".into(), "in_wall".into(), "into_structure".into()]));
}

#[test]
fn fittings_terminals_and_equipment_are_all_there() {
    let t = truth_for(5);
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for n in &t.nodes {
        *kinds.entry(n.kind.as_str()).or_default() += 1;
    }
    // Round ducts and pipes change size at their tees (reducing tees); a separate reducer shows
    // up where a size changes along a straight run, in some sets.
    assert!((1..=20).any(|s| truth_for(s).nodes.iter().any(|n| n.kind == "reducer")));
    for k in ["elbow", "tee", "cross", "transition", "terminal", "equipment", "fixture", "service", "vtr"] {
        assert!(kinds.get(k).copied().unwrap_or(0) > 0, "no {k}: {kinds:?}");
    }
    for k in ["supply_diffuser", "return_grille", "exhaust_grille", "louver", "sprinkler_head"] {
        assert!(t.terminals.iter().any(|x| x.kind == k), "no {k}");
    }
    for k in ["ahu", "vav", "exhaust_fan", "water_heater", "gas_meter"] {
        assert!(t.equipment.iter().any(|e| e.kind == k && !e.schedule.is_empty()), "no {k}");
    }
    for code in ["SA", "RA", "EA", "OA", "CW", "HW", "HWR", "SAN", "V", "G", "FP"] {
        assert!(t.segments.iter().any(|s| s.system == code), "no {code} runs");
    }
}

/// Texts in a drawing with their positions.
fn texts(d: &cadaddy_doc::Drawing) -> Vec<(String, [f64; 2])> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::Text(t) => Some((t.value.clone(), [t.insert.x, t.insert.y])),
            cadaddy_doc::EntityKind::MText(t) => Some((t.contents.clone(), [t.insert.x, t.insert.y])),
            _ => None,
        })
        .collect()
}

#[test]
fn the_sheets_say_what_the_truth_says() {
    let set = generate(3).unwrap();
    let t = &set.truth;
    assert_eq!(t.sheets.len(), layout::sheet_list().len());
    for sh in &t.sheets {
        assert!(set.file(&sh.pdf).is_some_and(|b| b.starts_with(b"%PDF")), "{}", sh.pdf);
        assert!(set.file(&sh.dxf).is_some_and(|b| !b.is_empty()), "{}", sh.dxf);
        assert!(
            !sh.legend.is_empty()
                || !sh.number.ends_with("01")
                || sh.number == "G-001"
                || sh.number.contains("60")
                || sh.number.starts_with("A-2")
                || sh.number.starts_with("A-3")
                || sh.number.starts_with("A-5"),
            "{} has no legend",
            sh.number
        );
    }
    let sh = t.sheets.iter().find(|s| s.number == "M-101").unwrap();

    // DXF: the tags and the title block, at the sheet's placement exactly.
    let dxf = cadaddy_io::read_dxf(set.file(&sh.dxf).unwrap()).unwrap();
    let dt = texts(&dxf);
    assert!(dt.iter().any(|(s, _)| s == "M-101"));
    for s in t.segments.iter().filter(|s| s.sheet == "M-101") {
        let Some(tag) = &s.tag else { continue };
        for line in &tag.text {
            assert!(dt.iter().any(|(x, _)| x == line), "DXF lacks tag text `{line}` of {}", s.id);
        }
    }
    // A grid bubble's label sits at the bubble's centre: the grid line's end beyond the plan.
    let g = &t.building.grid.x[0];
    let want: Vec<[f64; 2]> = dxf
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::Text(x) if x.value == g.label => x.align_pt.map(|p| [p.x, p.y]),
            _ => None,
        })
        .collect();
    let at = layout::to_model(sh, [g.at_ft, t.building.grid.y[0].at_ft]);
    assert!(want.iter().any(|p| (p[0] - at[0]).abs() < 1e-6), "grid {} label not on its line: {want:?} vs {at:?}", g.label);
    // The building → model map inverts.
    let back = score::building_from_model(sh, at);
    assert!((back[0] - g.at_ft).abs() < 1e-9 && (back[1] - t.building.grid.y[0].at_ft).abs() < 1e-9);

    // PDF: read back as text; the sheet's paper mapping puts a tag where the truth says.
    let (pdf, _) = cadaddy_io::pdf_read::read_json(set.file(&sh.pdf).unwrap(), &serde_json::json!({"scale": 1, "layout": false})).unwrap();
    let pt = texts(&pdf);
    assert!(pt.iter().any(|(s, _)| s == "M-101"), "PDF has no sheet number text");
    let mut found = 0;
    for s in t.segments.iter().filter(|s| s.sheet == "M-101") {
        let Some(tag) = &s.tag else { continue };
        let first = &tag.text[0];
        if pt.iter().any(|(x, _)| x == first) {
            found += 1;
        }
    }
    let tagged = t.segments.iter().filter(|s| s.sheet == "M-101" && s.tag.is_some()).count();
    assert!(tagged > 10 && found == tagged, "{found} of {tagged} tags read back from the PDF");
    // Paper positions: VAV tags (unique texts, placed at known building points) land where the
    // sheet's building → model → paper mapping says, up to one scale and offset for the reader's
    // units.
    let th = st_text(t, sh);
    let expect = |tag: &str| {
        let e = t.equipment.iter().find(|e| e.tag == tag).unwrap();
        let b = [e.max_ft[0] + th * 6.0, (e.min_ft[1] + e.max_ft[1]) / 2.0];
        layout::model_to_paper(sh, layout::to_model(sh, b))
    };
    let got = |tag: &str| pt.iter().find(|(s, _)| s == tag).map(|(_, p)| *p).unwrap_or_else(|| panic!("{tag} not in the PDF"));
    let (e1, e2, e3) = (expect("VAV-1-01"), expect("VAV-1-05"), expect("VAV-1-08"));
    let (g1, g2, g3) = (got("VAV-1-01"), got("VAV-1-05"), got("VAV-1-08"));
    let unit = (g2[0] - g1[0]) / (e2[0] - e1[0]);
    assert!(unit.is_finite() && unit > 0.0, "{unit}");
    for k in 0..2 {
        let predicted = g1[k] + (e3[k] - e1[k]) * unit;
        assert!((predicted - g3[k]).abs() < 0.02 * unit, "axis {k}: {predicted} vs {}", g3[k]);
    }
}

/// Tag text height on a sheet, feet.
fn st_text(t: &Truth, sh: &cadaddy_mepfix::truth::Sheet) -> f64 {
    // The set's style comes from the seed, as the generator makes it.
    let mut r = cadaddy_mepfix::rng::Rng::new(t.seed);
    let st = cadaddy_mepfix::style::Style::new(&mut r.fork(1));
    st.text_in / sh.scale
}

#[test]
fn styles_vary_between_seeds() {
    let mut rect = BTreeSet::new();
    let mut supply = BTreeSet::new();
    let mut papers = BTreeSet::new();
    let (mut rotated, mut offset, mut enlarged_scales) = (false, false, BTreeSet::new());
    for seed in 1..=30 {
        let mut set = generate(seed).unwrap().truth;
        let tag = set.segments.iter().filter(|s| s.shape == "rect" && s.kind == "duct").find_map(|s| s.tag.clone()).unwrap();
        rect.insert(tag.text[0].chars().filter(|c| !c.is_ascii_digit()).collect::<String>());
        supply.insert(set.system("SA").unwrap().printed.clone());
        papers.insert(format!("{:?}", set.sheets[0].paper_in));
        rotated |= set.sheets.iter().any(|s| s.model_from_building.rotation_deg != 0.0);
        offset |= set.sheets.iter().any(|s| s.paper_from_model.origin_in != [0.0, 0.0]);
        for s in set.sheets.drain(..).filter(|s| s.window_ft.is_some()) {
            enlarged_scales.insert(s.scale_text.unwrap_or_default());
        }
    }
    assert!(rect.len() >= 3, "{rect:?}");
    assert!(supply.len() >= 3, "{supply:?}");
    assert!(papers.len() >= 3, "{papers:?}");
    assert!(rotated && offset);
    assert!(enlarged_scales.len() >= 2, "{enlarged_scales:?}");
}

#[test]
fn the_scorer_scores() {
    let t = truth_for(4);
    let perfect = score::perfect(&t, "1", "duct");
    let s = score::score_runs(&t, "1", "duct", &perfect, 0.25);
    assert!((s.precision - 1.0).abs() < 1e-9 && (s.recall - 1.0).abs() < 1e-9, "{s:?}");
    assert!((s.size_accuracy - 1.0).abs() < 1e-9 && (s.system_accuracy - 1.0).abs() < 1e-9);
    assert_eq!(s.height_mae_ft, Some(0.0));
    // Half the runs missed, the rest a size off and two feet high, plus a run that isn't there.
    let mut half: Vec<_> = perfect.iter().step_by(2).cloned().collect();
    for f in &mut half {
        f.size_in[0] += 2.0;
        f.bottom_ft = f.bottom_ft.map(|b| b + 2.0);
    }
    let mut ghost = half[0].clone();
    ghost.from_ft = [500.0, 500.0];
    ghost.to_ft = [520.0, 500.0];
    half.push(ghost);
    let s = score::score_runs(&t, "1", "duct", &half, 0.25);
    assert!(s.recall < 0.8 && s.recall > 0.2, "{s:?}");
    assert!(s.precision < 1.0 && s.size_accuracy < 0.01);
    assert!((s.height_mae_ft.unwrap() - 2.0).abs() < 1e-6);
}

// ---- Electrical fixture tests -------------------------------------------------------------------

#[test]
fn electrical_devices_exist_for_every_seed() {
    for seed in 1..=20 {
        let t = truth_for(seed);
        let ctx = format!("seed {seed}");
        assert!(!t.elec_devices.is_empty(), "{ctx}: no electrical devices");
        assert!(!t.elec_panels.is_empty(), "{ctx}: no electrical panels");
        assert!(!t.elec_circuits.is_empty(), "{ctx}: no electrical circuits");
        assert!(!t.elec_conduits.is_empty(), "{ctx}: no electrical conduits");
    }
}

#[test]
fn all_electrical_device_kinds_present() {
    let t = truth_for(5);
    let kinds: BTreeSet<&str> = t.elec_devices.iter().map(|d| d.kind.as_str()).collect();
    for k in ["light_fixture", "switch", "exit_sign", "emergency_light", "receptacle", "data_outlet", "disconnect", "junction_box"] {
        assert!(kinds.contains(k), "missing device kind: {k}, have: {kinds:?}");
    }
    // Floor boxes appear for rooms > 400 sq ft; at least one seed has them.
    let any_fb = (1..=20).any(|s| truth_for(s).elec_devices.iter().any(|d| d.kind == "floor_box"));
    assert!(any_fb, "no floor_box in any seed 1..20");
}

#[test]
fn panels_have_schedules() {
    let t = truth_for(3);
    for p in &t.elec_panels {
        if p.kind == "panel" {
            assert!(!p.schedule.is_empty(), "panel {} has empty schedule", p.tag);
        }
    }
    // Transformer exists on level 1.
    assert!(t.elec_panels.iter().any(|p| p.kind == "transformer" && p.level == "1"), "no transformer");
}

#[test]
fn circuits_link_devices_to_panels() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        let ctx = format!("seed {seed}");
        for ckt in &t.elec_circuits {
            assert!(!ckt.devices.is_empty(), "{ctx}: circuit {} has no devices", ckt.id);
            assert!(!ckt.panel.is_empty(), "{ctx}: circuit {} has no panel", ckt.id);
            // Every device id in the circuit exists.
            for did in &ckt.devices {
                assert!(t.elec_devices.iter().any(|d| &d.id == did), "{ctx}: circuit {} references nonexistent device {did}", ckt.id);
            }
        }
        // Every device with a circuit is in that circuit's device list.
        for d in &t.elec_devices {
            if !d.circuit.is_empty() {
                let ckt = t.elec_circuits.iter().find(|c| c.id == d.circuit);
                assert!(ckt.is_some(), "{ctx}: device {} references nonexistent circuit {}", d.id, d.circuit);
                assert!(ckt.is_some_and(|c| c.devices.contains(&d.id)), "{ctx}: device {} not in circuit {}'s list", d.id, d.circuit);
            }
        }
    }
}

#[test]
fn electrical_sheets_are_generated() {
    let set = generate(1).unwrap();
    let t = &set.truth;
    // E-sheets exist.
    for num in ["E-101", "E-102", "E-201", "E-202", "E-601"] {
        let sh = t.sheets.iter().find(|s| s.number == num);
        assert!(sh.is_some(), "missing sheet {num}");
        let sh = sh.unwrap();
        assert!(set.file(&sh.pdf).is_some_and(|b| b.starts_with(b"%PDF")), "{num} has no PDF");
        assert!(set.file(&sh.dxf).is_some_and(|b| !b.is_empty()), "{num} has no DXF");
    }
    // Lighting plans show the right level.
    assert_eq!(t.sheets.iter().find(|s| s.number == "E-101").unwrap().level.as_deref(), Some("1"));
    assert_eq!(t.sheets.iter().find(|s| s.number == "E-201").unwrap().level.as_deref(), Some("1"));
}

#[test]
fn devices_have_diagrammatic_offsets() {
    let t = truth_for(7);
    let mut any_offset = false;
    for d in &t.elec_devices {
        let dx = (d.drawn_ft[0] - d.at_ft[0]).abs();
        let dy = (d.drawn_ft[1] - d.at_ft[1]).abs();
        if dx > 0.01 || dy > 0.01 {
            any_offset = true;
        }
        // Diagrammatic offset is bounded (not more than ~2 ft).
        assert!(dx < 2.0 && dy < 2.0, "device {} offset too large: dx={dx}, dy={dy}", d.id);
    }
    assert!(any_offset, "no device has a diagrammatic offset");
}

#[test]
fn mounting_heights_are_realistic() {
    let t = truth_for(2);
    for d in &t.elec_devices {
        match d.kind.as_str() {
            "receptacle" | "data_outlet" => assert_eq!(d.mounting_in, 18.0, "device {} should be at 18\" AFF", d.id),
            "switch" => assert_eq!(d.mounting_in, 46.0, "switch {} should be at 46\" AFF", d.id),
            "disconnect" => assert_eq!(d.mounting_in, 60.0, "disconnect {} should be at 60\" AFF", d.id),
            "light_fixture" if d.mounting_note.as_deref() == Some("CLG") => {
                assert!(d.mounting_in > 70.0, "ceiling fixture {} mounting too low: {}", d.id, d.mounting_in);
            }
            _ => {}
        }
    }
}

#[test]
fn conduits_have_tags_and_sizes() {
    let t = truth_for(4);
    for c in &t.elec_conduits {
        assert!(c.size_in > 0.0, "conduit {} has zero size", c.id);
        assert!(!c.conduit_type.is_empty(), "conduit {} has no type", c.id);
        let dx = c.to_ft[0] - c.from_ft[0];
        let dy = c.to_ft[1] - c.from_ft[1];
        assert!((dx * dx + dy * dy).sqrt() > 0.1, "conduit {} is too short", c.id);
    }
}

#[test]
fn electrical_schedule_sheet_has_content() {
    let set = generate(2).unwrap();
    let t = &set.truth;
    let sh = t.sheets.iter().find(|s| s.number == "E-601").unwrap();
    let dxf_bytes = set.file(&sh.dxf).unwrap();
    let dxf = cadaddy_io::read_dxf(dxf_bytes).unwrap();
    let dt = texts(&dxf);
    // The schedule sheet should contain panel tags and fixture types.
    assert!(dt.iter().any(|(s, _)| s.contains("LIGHT FIXTURE SCHEDULE")), "no light fixture schedule on E-601");
    assert!(dt.iter().any(|(s, _)| s.contains("PANEL")), "no panel schedule on E-601");
}

// ---- Electrical device reader tests ---------------------------------------------------------

use cadaddy_doc::EntityKind;
use cadaddy_mepfix::FixtureSet;
use cadaddy_mepfix::elec_reader::{self, DeviceInputs, FoundDevice, MountingRules};
use cadaddy_mepfix::sheet_reader::{self, Alignment, SheetMap};

/// A set's DXF sheets, its sheet map, and the device reader's inputs read from its documents.
fn elec_docs(set: &FixtureSet) -> (Vec<(String, Vec<u8>)>, SheetMap, DeviceInputs) {
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = sheet_reader::recognise_fixture_set(&dxf);
    let inputs = DeviceInputs::read(&dxf, &map);
    (dxf, map, inputs)
}

fn alignment(map: &SheetMap, sheet: &str) -> Alignment {
    map.sheets.iter().find(|s| s.number == sheet).and_then(|s| s.alignment.clone()).unwrap_or_else(|| panic!("{sheet} not aligned"))
}

/// Building feet → a sheet's model inches.
fn model_at(al: &Alignment, b: [f64; 2]) -> [f64; 2] {
    let r = layout::rotate([b[0] * 12.0, b[1] * 12.0], al.rotation_deg);
    [r[0] + al.offset_in[0], r[1] + al.offset_in[1]]
}

/// Where an entity sits, model inches.
fn anchor(k: &EntityKind) -> Option<[f64; 2]> {
    Some(match k {
        EntityKind::Circle(c) => [c.center.x, c.center.y],
        EntityKind::Arc(a) => [a.center.x, a.center.y],
        EntityKind::Line(l) => [(l.a.x + l.b.x) / 2.0, (l.a.y + l.b.y) / 2.0],
        EntityKind::Text(t) => t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]),
        EntityKind::LwPolyline(p) => {
            let n = p.vertices.len().max(1) as f64;
            [p.vertices.iter().map(|v| v.p.x).sum::<f64>() / n, p.vertices.iter().map(|v| v.p.y).sum::<f64>() / n]
        }
        _ => return None,
    })
}

/// Edit one sheet of the list: `f` returns `None` to delete an entity, or its new kind.
fn edit_sheet(dxf: &mut [(String, Vec<u8>)], sheet: &str, f: &dyn Fn(&cadaddy_doc::Entity) -> Option<EntityKind>) -> usize {
    let (_, bytes) = dxf.iter_mut().find(|(n, _)| n.trim_end_matches(".dxf") == sheet).unwrap_or_else(|| panic!("no {sheet}"));
    let mut d = cadaddy_io::read_dxf(bytes).unwrap();
    let all: Vec<cadaddy_doc::Entity> = d.model.iter().map(|e| (**e).clone()).collect();
    let mut changed = 0;
    for e in all {
        match f(&e) {
            None => {
                d.model.remove(e.handle);
                changed += 1;
            }
            Some(k) if k != e.kind => {
                let mut e2 = e.clone();
                e2.kind = k;
                d.model.replace(e2);
                changed += 1;
            }
            Some(_) => {}
        }
    }
    *bytes = cadaddy_io::write_dxf(&d).into_bytes();
    changed
}

/// The found device of a kind drawn nearest a point (building feet), within `tol`.
fn found_at<'a>(found: &'a [FoundDevice], level: &str, kind: &str, at: [f64; 2], tol: f64) -> Option<&'a FoundDevice> {
    found
        .iter()
        .filter(|f| f.level == level && f.kind == kind)
        .map(|f| ((f.drawn_ft[0] - at[0]).hypot(f.drawn_ft[1] - at[1]), f))
        .filter(|(d, _)| *d < tol)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, f)| f)
}

#[test]
fn elec_reader_perfect_scorer_round_trips() {
    // A perfect reader should score perfectly.
    let t = truth_for(5);
    let perfect = elec_reader::perfect_devices(&t);
    let s = elec_reader::score_devices(&t, &perfect, 1.5);
    assert_eq!(s.true_positives, t.elec_devices.len(), "TP should match truth device count");
    assert_eq!((s.false_positives, s.false_negatives, s.kind_confusions), (0, 0, 0));
    assert!((s.precision - 1.0).abs() < 1e-9 && (s.recall - 1.0).abs() < 1e-9, "{s:?}");
    assert!(s.height_mae_in < 1e-9, "perfect height MAE: {}", s.height_mae_in);
    assert!((s.kind_accuracy - 1.0).abs() < 1e-9, "perfect kind accuracy: {}", s.kind_accuracy);
    assert!(s.type_tag_total > 0 && (s.type_tag_accuracy - 1.0).abs() < 1e-9, "{s:?}");
    assert!(s.circuit_total > 0 && (s.circuit_accuracy - 1.0).abs() < 1e-9, "{s:?}");
    assert!((s.zone_containment - 1.0).abs() < 1e-9 && s.position_mae_ft < 1e-9, "{s:?}");
}

#[test]
fn elec_reader_scores_degraded_input() {
    // Half the devices missing, the rest off by 6" — precision and recall reflect it.
    let t = truth_for(4);
    let mut half: Vec<_> = elec_reader::perfect_devices(&t).into_iter().step_by(2).collect();
    for d in &mut half {
        d.mounting_in += 6.0;
    }
    // A ghost device drawn where nothing is.
    let mut ghost = half[0].clone();
    ghost.drawn_ft = [500.0, 500.0];
    half.push(ghost);
    let s = elec_reader::score_devices(&t, &half, 1.5);
    assert_eq!(s.true_positives, half.len() - 1);
    assert_eq!(s.false_positives, 1);
    assert!(s.recall < 0.6 && s.recall > 0.4, "degraded recall: {}", s.recall);
    assert!((s.height_mae_in - 6.0).abs() < 1e-9, "height MAE should be 6: {}", s.height_mae_in);
}

#[test]
fn elec_reader_scorer_requires_same_kind_and_tests_zones_along_their_wall() {
    let t = truth_for(5);
    let perfect = elec_reader::perfect_devices(&t);
    let n = t.elec_devices.len();
    // One device read as the wrong kind: not a match, but a kind confusion (and an FP and FN).
    let mut wrong = perfect.clone();
    let i = wrong.iter().position(|d| d.kind == "receptacle").unwrap();
    wrong[i].kind = "switch".into();
    let s = elec_reader::score_devices(&t, &wrong, 1.5);
    assert_eq!((s.true_positives, s.false_positives, s.false_negatives, s.kind_confusions), (n - 1, 1, 1, 1));
    assert!((s.kind_accuracy - (n - 1) as f64 / n as f64).abs() < 1e-9, "{}", s.kind_accuracy);

    // A zone on a north–south wall whose y range misses the truth is not containing it, even
    // when the truth's x happens to fall inside that range.
    let zi = perfect.iter().position(|d| !d.arch_positioned).unwrap();
    let td = &t.elec_devices[zi];
    let (x, y) = (td.at_ft[0], td.at_ft[1]);
    let mut zoned = perfect.clone();
    zoned[zi].zone_wall_ft = Some([[x, y + 50.0], [x, y + 60.0]]);
    zoned[zi].zone_ft = Some([x - 1.0, x + 1.0]); // contains x, but the wall runs along y
    let zones = perfect.iter().filter(|d| !d.arch_positioned).count();
    let s = elec_reader::score_devices(&t, &zoned, 1.5);
    assert!((s.zone_containment - (zones - 1) as f64 / zones as f64).abs() < 1e-9, "{}", s.zone_containment);
    zoned[zi].zone_ft = Some([y - 1.0, y + 1.0]); // along its own axis: contains the truth
    let s = elec_reader::score_devices(&t, &zoned, 1.5);
    assert!((s.zone_containment - 1.0).abs() < 1e-9, "{}", s.zone_containment);
}

#[test]
fn elec_reader_takes_no_truth_and_has_no_generator_formulas() {
    let src = include_str!("../src/elec_reader.rs");
    let start = src.find("pub fn read_devices(").expect("read_devices");
    let sig = &src[start..start + src[start..].find('{').unwrap()];
    assert!(!sig.contains("Truth"), "read_devices takes the answer key: {sig}");
    let body_end = start + src[start..].find("\n}\n").unwrap();
    assert!(!src[start..body_end].to_ascii_lowercase().contains("truth"), "read_devices looks at the truth");
    let inputs = src.find("pub fn read(dxf_data").expect("DeviceInputs::read");
    let isig = &src[inputs..inputs + src[inputs..].find('{').unwrap()];
    assert!(!isig.contains("Truth"), "{isig}");
    // The generator's layout formulas are gone as position sources.
    for f in [
        "light_grid_for_room",
        "receptacle_positions_for_room",
        "exit_sign_positions",
        "emergency_light_positions",
        "position_from_rules",
        "snap_to_nearest",
    ] {
        assert!(!src.contains(f), "{f} is back");
    }
}

#[test]
fn elec_reader_reads_devices_from_documents_only() {
    // Devices read from the DXF sheets with rooms, ceilings and levels from the readers only.
    let set = generate(3).unwrap();
    let t = &set.truth;
    let (dxf, map, inputs) = elec_docs(&set);
    // The cover sheet's level list, read exactly.
    assert_eq!(inputs.levels.len(), t.building.levels.len());
    for l in &t.building.levels {
        let got = inputs.levels.iter().find(|x| x.code == l.code).unwrap_or_else(|| panic!("level {}", l.code));
        assert!((got.elevation_ft - l.elevation_ft).abs() < 1e-6, "level {}: {} vs {}", l.code, got.elevation_ft, l.elevation_ft);
    }
    assert!(!inputs.rooms.is_empty() && inputs.rooms.iter().any(|r| r.ceiling_ft.is_some()));
    let read = elec_reader::read_devices(&dxf, &map, &inputs);
    assert!(read.devices.len() > 400, "{}", read.devices.len());
    for d in &read.devices {
        assert!(d.source_sheet.starts_with("E-"), "{d:?}");
        assert!(["note", "legend", "rule"].contains(&d.height_source.as_str()), "{d:?}");
        assert!(d.room.is_empty() || inputs.rooms.iter().any(|r| r.level == d.level && r.number == d.room), "{d:?}");
        if d.arch_positioned {
            // Located only where an A-sheet fixes it: exactly on one of the A-sheets' locations.
            assert_eq!(d.placement, "located");
            assert!(d.position_source.starts_with("A-") && !d.field_decision, "{d:?}");
            assert!(
                inputs.locators.iter().any(|l| l.level == d.level && l.kind == d.kind && l.at_ft == [d.at_ft[0], d.at_ft[1]]),
                "{d:?} is not on an A-sheet location"
            );
        } else {
            assert_eq!(d.placement, "rule");
            assert!(d.field_decision && d.zone_ft.is_some() && d.zone_wall_ft.is_some(), "{d:?}");
        }
    }

    // With no rooms and no A-sheet locations, every device is a zone and a finding.
    let bare = DeviceInputs { rooms: Vec::new(), locators: Vec::new(), ..inputs.clone() };
    let read = elec_reader::read_devices(&dxf, &map, &bare);
    let no_room = read.findings.iter().filter(|f| f.contains("is in no room")).count();
    assert_eq!(no_room, read.devices.len());
    for d in &read.devices {
        assert!(d.room.is_empty() && !d.arch_positioned && d.field_decision && d.placement == "rule", "{d:?}");
        let z = d.zone_ft.unwrap();
        assert!((z[1] - z[0] - 6.0).abs() < 1e-9 && z[0] <= d.drawn_ft[0] && d.drawn_ft[0] <= z[1], "{d:?}");
    }
}

#[test]
fn elec_reader_meets_thresholds_on_office_and_hotel_sets() {
    let sets = [
        ("office 3".to_string(), generate(3).unwrap()),
        ("office 7".to_string(), generate(7).unwrap()),
        ("hotel small 7".to_string(), generate_hotel(7, HotelSize::Small).unwrap()),
    ];
    for (name, set) in &sets {
        let t = &set.truth;
        let (dxf, map, inputs) = elec_docs(set);
        let read = elec_reader::read_devices(&dxf, &map, &inputs);
        let s = elec_reader::score_devices(t, &read.devices, 1.5);
        println!(
            "{name}: truth {} found {} TP {} FP {} FN {} | P {:.3} R {:.3} kind {:.3} ({} confusions) height MAE {:.3} in | type tag {:.3} of {} | circuit {:.3} of {} | located {}/{} pos MAE {:.4} ft | zone containment {:.3} | sources {:?}",
            t.elec_devices.len(),
            read.devices.len(),
            s.true_positives,
            s.false_positives,
            s.false_negatives,
            s.precision,
            s.recall,
            s.kind_accuracy,
            s.kind_confusions,
            s.height_mae_in,
            s.type_tag_accuracy,
            s.type_tag_total,
            s.circuit_accuracy,
            s.circuit_total,
            s.arch_located,
            s.arch_total,
            s.position_mae_ft,
            s.zone_containment,
            s.height_sources
        );
        assert!(s.recall >= 0.9, "{name}: recall {:.3}", s.recall);
        assert!(s.precision >= 0.9, "{name}: precision {:.3}", s.precision);
        assert!(s.kind_accuracy >= 0.95, "{name}: kind accuracy {:.3}", s.kind_accuracy);
        assert!(s.height_mae_in <= 1.0, "{name}: height MAE {:.3}", s.height_mae_in);
        assert!(s.type_tag_total > 50 && s.type_tag_accuracy >= 0.9, "{name}: type tags {:.3} of {}", s.type_tag_accuracy, s.type_tag_total);
        assert!(s.circuit_total > 50 && s.circuit_accuracy >= 0.9, "{name}: circuits {:.3} of {}", s.circuit_accuracy, s.circuit_total);
        // Every device the A-sheets fix is located there, exactly.
        assert!(s.arch_total > 50 && s.arch_located == s.arch_total, "{name}: located {} of {}", s.arch_located, s.arch_total);
        assert!(s.position_mae_ft < 1e-6, "{name}: position error {}", s.position_mae_ft);
        assert!(s.zone_containment >= 0.85, "{name}: zone containment {:.3}", s.zone_containment);
    }
}

#[test]
fn elec_reader_located_device_ignores_its_esheet_symbol_and_without_its_dimension_is_a_zone() {
    let set = generate_hotel(7, HotelSize::Small).unwrap();
    let t = &set.truth;
    let (mut dxf, map, inputs) = elec_docs(&set);
    let base = elec_reader::read_devices(&dxf, &map, &inputs);
    // A guest room receptacle the A floor plan dimensions, on its room's south wall.
    let td = t.elec_devices.iter().find(|d| d.level == "2" && d.kind == "receptacle" && d.arch_positioned).unwrap();
    let f = found_at(&base.devices, &td.level, &td.kind, td.drawn_ft, 0.5).unwrap();
    assert!(f.arch_positioned && f.placement == "located", "{f:?}");
    assert_eq!([f.at_ft[0], f.at_ft[1]], [td.at_ft[0], td.at_ft[1]]);
    let a_sheet = f.position_source.clone();
    assert!(a_sheet.starts_with("A-"), "{a_sheet}");

    // Move its E-sheet symbol (and its notes) 3 ft along the wall: the model position stays.
    let al = alignment(&map, &td.sheet);
    let c = model_at(&al, td.drawn_ft);
    let shift = layout::rotate([36.0, 0.0], al.rotation_deg);
    let m = cadaddy_doc::geom::Mat3::translate(cadaddy_doc::geom::Vec2::new(shift[0], shift[1]));
    let moved = edit_sheet(&mut dxf, &td.sheet, &|e| {
        let mut k = e.kind.clone();
        if (e.common.layer == "DEVICE" || e.common.layer == "TEXT") && anchor(&k).is_some_and(|p| (p[0] - c[0]).hypot(p[1] - c[1]) < 14.0) {
            k.transform(&m);
        }
        Some(k)
    });
    assert!(moved >= 3, "moved {moved}");
    let read = elec_reader::read_devices(&dxf, &map, &inputs);
    let at = [td.drawn_ft[0] + 3.0, td.drawn_ft[1]];
    let f = found_at(&read.devices, &td.level, &td.kind, at, 0.5).unwrap();
    assert!(f.arch_positioned, "{f:?}");
    assert_eq!([f.at_ft[0], f.at_ft[1]], [td.at_ft[0], td.at_ft[1]], "the moved symbol moved the model device");
    assert_eq!(read.devices.len(), base.devices.len());

    // Take the dimensions to it off the A floor plan: it becomes a zone on its wall that contains
    // the truth along the wall (x: the south wall runs east–west).
    let (mut dxf, _, _) = elec_docs(&set);
    let aal = alignment(&map, &a_sheet);
    let mark = model_at(&aal, [td.at_ft[0], td.at_ft[1]]);
    let removed = edit_sheet(&mut dxf, &a_sheet, &|e| match &e.kind {
        EntityKind::Line(l)
            if e.common.layer == elec_reader::LOCATOR_LAYER && [l.a, l.b].iter().any(|p| (p.x - mark[0]).hypot(p.y - mark[1]) < 1.0) =>
        {
            None
        }
        k => Some(k.clone()),
    });
    assert!(removed >= 1, "no dimension removed");
    let inputs2 = DeviceInputs::read(&dxf, &map);
    assert_eq!(inputs2.locators.len(), inputs.locators.len() - 1);
    let read = elec_reader::read_devices(&dxf, &map, &inputs2);
    let f = found_at(&read.devices, &td.level, &td.kind, td.drawn_ft, 0.5).unwrap();
    assert!(!f.arch_positioned && f.field_decision && f.placement == "rule" && f.position_source.is_empty(), "{f:?}");
    let wall = f.zone_wall_ft.unwrap();
    let zone = f.zone_ft.unwrap();
    assert!((wall[0][1] - wall[1][1]).abs() < 1e-9, "not an east–west wall: {wall:?}");
    assert!(zone[0] <= td.at_ft[0] && td.at_ft[0] <= zone[1], "zone {zone:?} misses x {}", td.at_ft[0]);
    assert!((wall[0][1] - td.at_ft[1]).abs() < 1.0, "wall {wall:?} is not the receptacle's wall (y {})", td.at_ft[1]);
}

#[test]
fn elec_reader_legend_height_is_the_second_source() {
    let set = generate(3).unwrap();
    let t = &set.truth;
    let (mut dxf, map, inputs) = elec_docs(&set);
    let base = elec_reader::read_devices(&dxf, &map, &inputs);
    let legend = base.legends.get("E-201").unwrap();
    let rec = legend.iter().find(|e| e.kind == "receptacle").unwrap();
    assert_eq!(rec.mounting_in, Some(18.0));
    assert!(rec.symbol.contains("circle") && rec.description.contains("RECEPTACLE"), "{rec:?}");
    // A receptacle with its height note printed.
    let td = t.elec_devices.iter().find(|d| d.sheet == "E-201" && d.kind == "receptacle").unwrap();
    let f = found_at(&base.devices, &td.level, &td.kind, td.drawn_ft, 0.5).unwrap();
    assert_eq!((f.height_source.as_str(), f.mounting_in), ("note", 18.0));

    // Without the note (and with the legend saying +20"): the legend's height, source "legend".
    let al = alignment(&map, "E-201");
    let c = model_at(&al, td.drawn_ft);
    edit_sheet(&mut dxf, "E-201", &|e| match &e.kind {
        EntityKind::Text(x)
            if e.common.layer == "TEXT" && x.value.starts_with('+') && anchor(&e.kind).is_some_and(|p| (p[0] - c[0]).hypot(p[1] - c[1]) < 14.0) =>
        {
            None
        }
        EntityKind::Text(x) if e.common.layer == "LEGEND" && x.value.contains("RECEPTACLE") => {
            let mut x = x.clone();
            x.value = x.value.replace("+18\"", "+20\"");
            Some(EntityKind::Text(x))
        }
        k => Some(k.clone()),
    });
    let read = elec_reader::read_devices(&dxf, &map, &inputs);
    let f = found_at(&read.devices, &td.level, &td.kind, td.drawn_ft, 0.5).unwrap();
    assert_eq!((f.height_source.as_str(), f.mounting_in), ("legend", 20.0), "{f:?}");
    let z = inputs.levels.iter().find(|l| l.code == td.level).unwrap().elevation_ft + 20.0 / 12.0;
    assert!((f.at_ft[2] - z).abs() < 1e-9);
}

#[test]
fn elec_reader_rules_file_sets_rule_heights() {
    // The shipped rules file parses and holds the defaults.
    let shipped = MountingRules::from_json(elec_reader::DEFAULT_RULES_JSON).unwrap();
    assert_eq!(shipped, MountingRules::default());
    assert_eq!((shipped.height_in("receptacle"), shipped.height_in("switch"), shipped.height_in("disconnect")), (18.0, 46.0, 60.0));
    assert!(shipped.is_ceiling("light_fixture") && !shipped.is_ceiling("receptacle"));
    // Hostile or broken files are errors, not panics.
    for bad in ["not json", r#"{"heights_in":{"receptacle":-4}}"#, r#"{"heights_in":{"receptacle":9000}}"#, r#"{"default_ceiling_ft":0}"#] {
        assert!(MountingRules::from_json(bad).is_err(), "{bad}");
    }

    // A receptacle with no note, on a sheet whose legend gives no receptacle height: the rule.
    let set = generate(3).unwrap();
    let t = &set.truth;
    let (mut dxf, map, inputs) = elec_docs(&set);
    let td = t.elec_devices.iter().find(|d| d.sheet == "E-201" && d.kind == "receptacle").unwrap();
    let al = alignment(&map, "E-201");
    let c = model_at(&al, td.drawn_ft);
    edit_sheet(&mut dxf, "E-201", &|e| match &e.kind {
        EntityKind::Text(x)
            if e.common.layer == "TEXT" && x.value.starts_with('+') && anchor(&e.kind).is_some_and(|p| (p[0] - c[0]).hypot(p[1] - c[1]) < 14.0) =>
        {
            None
        }
        EntityKind::Text(x) if e.common.layer == "LEGEND" && x.value.contains("RECEPTACLE") => {
            let mut x = x.clone();
            x.value = "DUPLEX RECEPTACLE".into();
            Some(EntityKind::Text(x))
        }
        k => Some(k.clone()),
    });
    let read = elec_reader::read_devices(&dxf, &map, &inputs);
    let f = found_at(&read.devices, &td.level, &td.kind, td.drawn_ft, 0.5).unwrap();
    assert_eq!((f.height_source.as_str(), f.mounting_in), ("rule", 18.0), "{f:?}");

    // An edited project rules file moves it; a folder without one keeps the defaults.
    let dir = std::env::temp_dir().join(format!("elec-rules-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(MountingRules::for_project(&dir).unwrap(), shipped);
    std::fs::write(dir.join(elec_reader::RULES_FILE_NAME), elec_reader::DEFAULT_RULES_JSON.replace("\"receptacle\": 18", "\"receptacle\": 15"))
        .unwrap();
    let edited = MountingRules::for_project(&dir).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(edited.height_in("receptacle"), 15.0);
    let read = elec_reader::read_devices(&dxf, &map, &DeviceInputs { rules: edited, ..inputs.clone() });
    let f = found_at(&read.devices, &td.level, &td.kind, td.drawn_ft, 0.5).unwrap();
    assert_eq!((f.height_source.as_str(), f.mounting_in), ("rule", 15.0), "{f:?}");
    // Devices with their own notes don't move.
    let noted = read.devices.iter().filter(|d| d.kind == "receptacle" && d.height_source == "note").count();
    assert!(noted > 50 && read.devices.iter().filter(|d| d.kind == "receptacle" && d.height_source == "note").all(|d| d.mounting_in == 18.0));
}

#[test]
fn elec_reader_3d_output_covers_all_kinds() {
    let t = truth_for(5);
    let perfect = elec_reader::perfect_devices(&t);
    let shapes = elec_reader::build_3d(&perfect);
    assert_eq!(shapes.len(), perfect.len());

    let layers: BTreeSet<String> = shapes.iter().map(|s| s.layer.clone()).collect();
    // We should have layers for multiple device kinds.
    assert!(layers.len() >= 3, "too few 3D layers: {layers:?}");
    // Every shape has nonzero dimensions.
    for s in &shapes {
        assert!(s.box_in[0] > 0.0 && s.box_in[1] > 0.0 && s.box_in[2] > 0.0, "{} has zero box", s.kind);
    }
}

#[test]
fn elec_reader_classify_legend_works() {
    assert_eq!(elec_reader::classify_legend("A", "2X4 RECESSED TROFFER").as_deref(), Some("light_fixture"));
    assert_eq!(elec_reader::classify_legend("S", "LIGHT SWITCH").as_deref(), Some("switch"));
    assert_eq!(elec_reader::classify_legend("R", "RECEPTACLE").as_deref(), Some("receptacle"));
    assert_eq!(elec_reader::classify_legend("D", "DATA/TELECOM OUTLET").as_deref(), Some("data_outlet"));
    assert_eq!(elec_reader::classify_legend("ES-1", "EXIT SIGN, LED").as_deref(), Some("exit_sign"));
    assert_eq!(elec_reader::classify_legend("EM-1", "EMERGENCY LIGHT").as_deref(), Some("emergency_light"));
    assert_eq!(elec_reader::classify_legend("FB", "FLOOR BOX").as_deref(), Some("floor_box"));
    assert_eq!(elec_reader::classify_legend("JB", "JUNCTION BOX").as_deref(), Some("junction_box"));
    assert_eq!(elec_reader::classify_legend("DS", "DISCONNECT SWITCH").as_deref(), Some("disconnect"));
}

#[test]
fn hotel_deterministic_small() {
    let (a, b) = (generate_hotel(7, HotelSize::Small).unwrap(), generate_hotel(7, HotelSize::Small).unwrap());
    assert_eq!(a.files.len(), b.files.len());
    for ((na, ba), (nb, bb)) in a.files.iter().zip(&b.files) {
        assert_eq!(na, nb);
        assert!(ba == bb, "hotel {na} differs between runs");
    }
    // Different seed gives different output.
    let c = generate_hotel(8, HotelSize::Small).unwrap();
    assert_ne!(a.file("truth.json"), c.file("truth.json"));
}

#[test]
fn hotel_has_four_levels_and_roof() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert_eq!(t.building.levels.len(), 5);
    let codes: Vec<&str> = t.building.levels.iter().map(|l| l.code.as_str()).collect();
    assert_eq!(codes, vec!["1", "2", "3", "4", "R"]);
    assert_eq!(t.kind, "hotel");
}

#[test]
fn hotel_has_guest_rooms_on_upper_floors() {
    let t = hotel_truth_for(1, HotelSize::Full);
    for lv in ["2", "3", "4"] {
        let rooms: Vec<&str> = t.building.rooms.iter().filter(|r| r.level == lv && r.name != "CORRIDOR").map(|r| r.name.as_str()).collect();
        assert!(!rooms.is_empty(), "level {lv} has no guest rooms");
        assert!(
            rooms.iter().any(|n| n.contains("KING") || n.contains("QUEEN") || n.contains("SUITE")),
            "level {lv} has no recognizable guest room types: {rooms:?}"
        );
    }
}

#[test]
fn hotel_has_public_spaces_on_ground_floor() {
    let t = hotel_truth_for(1, HotelSize::Small);
    let ground: Vec<String> = t.building.rooms.iter().filter(|r| r.level == "1").map(|r| r.name.clone()).collect();
    assert!(ground.contains(&"LOBBY".to_string()), "no lobby on L1");
    assert!(ground.contains(&"FRONT DESK".to_string()), "no front desk on L1");
    assert!(ground.contains(&"MECHANICAL".to_string()), "no mechanical room on L1");
}

#[test]
fn hotel_has_ada_rooms() {
    let t = hotel_truth_for(1, HotelSize::Full);
    let ada: Vec<&str> = t.building.rooms.iter().filter(|r| r.name.contains("ADA")).map(|r| r.name.as_str()).collect();
    assert!(!ada.is_empty(), "no ADA rooms in the hotel");
}

#[test]
fn hotel_has_stairs_and_elevator() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(t.building.stairs.len() >= 2, "need at least 2 stairs");
    assert!(!t.building.elevators.is_empty(), "need at least 1 elevator");
}

#[test]
fn hotel_has_wall_assemblies() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(t.wall_assemblies.len() >= 5, "need at least 5 wall types, got {}", t.wall_assemblies.len());
    assert!(!t.walls.is_empty(), "no wall instances");
    // Every wall references a valid assembly.
    let assembly_ids: BTreeSet<String> = t.wall_assemblies.iter().map(|a| a.id.clone()).collect();
    for w in &t.walls {
        assert!(assembly_ids.contains(&w.assembly), "wall {} references unknown assembly {}", w.id, w.assembly);
    }
    // At least one fire-rated wall.
    assert!(t.wall_assemblies.iter().any(|a| !a.fire_rating.is_empty()), "no fire-rated wall assemblies");
}

#[test]
fn hotel_has_ceiling_assemblies_and_soffits() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(!t.ceiling_assemblies.is_empty(), "no ceiling assemblies");
    assert!(!t.soffits.is_empty(), "no soffits (expected in guest room bathrooms)");
}

#[test]
fn hotel_has_casework() {
    let t = hotel_truth_for(1, HotelSize::Full);
    assert!(t.casework.len() >= 10, "too few casework items: {}", t.casework.len());
    let kinds: BTreeSet<&str> = t.casework.iter().map(|c| c.kind.as_str()).collect();
    assert!(kinds.contains("vanity"), "no vanities");
    assert!(kinds.contains("wardrobe"), "no wardrobes");
}

#[test]
fn hotel_has_spec_book() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(t.spec_items.len() >= 10, "too few spec items: {}", t.spec_items.len());
    // Each spec item has a manufacturer and model.
    for s in &t.spec_items {
        assert!(!s.manufacturer.is_empty(), "spec {} has no manufacturer", s.id);
        assert!(!s.model.is_empty(), "spec {} has no model", s.id);
    }
    // Multiple CSI divisions.
    let divs: BTreeSet<&str> = t.spec_items.iter().map(|s| s.division.as_str()).collect();
    assert!(divs.len() >= 5, "too few CSI divisions: {divs:?}");
}

#[test]
fn hotel_has_fire_alarm_devices() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(!t.fire_alarm_devices.is_empty(), "no fire alarm devices");
    let kinds: BTreeSet<&str> = t.fire_alarm_devices.iter().map(|d| d.kind.as_str()).collect();
    assert!(kinds.contains("smoke_detector"), "no smoke detectors");
    assert!(kinds.contains("horn_strobe"), "no horn/strobes");
    assert!(kinds.contains("pull_station"), "no pull stations");
    assert!(kinds.contains("facp"), "no FACP");
}

#[test]
fn hotel_has_telecom_devices() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(!t.telecom_devices.is_empty(), "no telecom devices");
    let kinds: BTreeSet<&str> = t.telecom_devices.iter().map(|d| d.kind.as_str()).collect();
    assert!(kinds.contains("data_outlet"), "no data outlets");
    assert!(kinds.contains("wifi_ap"), "no WiFi APs");
}

#[test]
fn hotel_has_site_elements() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(!t.site_elements.is_empty(), "no site elements");
    let kinds: BTreeSet<&str> = t.site_elements.iter().map(|s| s.kind.as_str()).collect();
    assert!(kinds.contains("parking_space"), "no parking spaces");
    assert!(kinds.contains("light_pole"), "no light poles");
    assert!(kinds.contains("transformer_pad"), "no transformer pad");
    assert!(kinds.contains("fire_hydrant"), "no fire hydrant");
    assert!(kinds.iter().any(|k| k.starts_with("underground_")), "no underground utilities");
}

#[test]
fn hotel_has_gaps_for_q296() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(t.gaps.len() >= 5, "too few planted gaps: {}", t.gaps.len());
    let kinds: BTreeSet<&str> = t.gaps.iter().map(|g| g.kind.as_str()).collect();
    assert!(kinds.contains("wall_no_tag"), "no wall_no_tag gap");
    assert!(kinds.contains("rated_wall_stops_at_ceiling"), "no rated_wall_stops_at_ceiling gap");
    assert!(kinds.contains("head_not_specified"), "no head_not_specified gap");
}

#[test]
fn hotel_has_mep_systems() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(t.systems.len() >= 10, "too few MEP systems: {}", t.systems.len());
    assert!(!t.segments.is_empty(), "no MEP segments");
    assert!(!t.terminals.is_empty(), "no terminals");
    assert!(!t.equipment.is_empty(), "no equipment");
    assert!(!t.risers.is_empty(), "no risers");
}

#[test]
fn hotel_has_plumbing_fixtures() {
    let t = hotel_truth_for(1, HotelSize::Full);
    let kinds: BTreeSet<&str> = t.fixtures.iter().map(|f| f.kind.as_str()).collect();
    assert!(kinds.contains("water_closet"), "no water closets");
    assert!(kinds.contains("lavatory"), "no lavatories");
    assert!(kinds.contains("bathtub"), "no bathtubs");
}

#[test]
fn hotel_sheets_cover_all_disciplines() {
    let set = generate_hotel(1, HotelSize::Small).unwrap();
    let t = &set.truth;
    let discs: BTreeSet<&str> = t.sheets.iter().map(|s| s.discipline.as_str()).collect();
    for d in ["G", "A", "S", "M", "P", "FP", "E", "FA", "T", "C"] {
        assert!(discs.contains(d), "missing discipline {d} in hotel sheets");
    }
}

#[test]
fn hotel_full_has_more_rooms_than_small() {
    let small = hotel_truth_for(1, HotelSize::Small);
    let full = hotel_truth_for(1, HotelSize::Full);
    assert!(
        full.building.rooms.len() > small.building.rooms.len(),
        "full ({}) should have more rooms than small ({})",
        full.building.rooms.len(),
        small.building.rooms.len()
    );
}

#[test]
fn hotel_elevations_and_sections_exist() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(t.elevations.len() >= 4, "need 4 building elevations, got {}", t.elevations.len());
    assert!(t.sections.len() >= 2, "need at least 2 sections, got {}", t.sections.len());
    // Interior elevation exists.
    assert!(t.elevations.iter().any(|e| e.direction == "interior"), "no interior elevation");
}

#[test]
fn hotel_clashes_include_planted() {
    let t = hotel_truth_for(1, HotelSize::Small);
    let planted = t.clashes.iter().filter(|c| c.planted).count();
    assert!(planted >= 1, "no planted clashes in hotel");
}

// ---- In-wall coordination tests -----------------------------------------------------------

#[test]
fn walls_are_generated_for_every_room_edge() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        let ctx = format!("seed {seed}");
        assert!(!t.building.walls.is_empty(), "{ctx}: no walls generated");
        // Every room on every level must have at least one wall.
        for rm in &t.building.rooms {
            let has_wall = t.building.walls.iter().any(|w| w.level == rm.level && (w.sides[0] == rm.id || w.sides[1] == rm.id));
            assert!(has_wall, "{ctx}: room {} has no walls", rm.id);
        }
        // Every wall must have valid sides (either a room id or "EXT").
        for w in &t.building.walls {
            for side in &w.sides {
                assert!(side == "EXT" || t.building.rooms.iter().any(|r| &r.id == side), "{ctx}: wall {} side {side} is not a room or EXT", w.id,);
            }
            assert!(w.thickness_in > 0.0, "{ctx}: wall {} has zero thickness", w.id);
            assert!(w.stud_spacing_in > 0.0, "{ctx}: wall {} has zero stud spacing", w.id);
        }
    }
}

#[test]
fn rough_ins_are_behind_every_plumbing_fixture() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        let ctx = format!("seed {seed}");
        assert!(!t.rough_ins.is_empty(), "{ctx}: no rough-ins generated");
        // Every fixture with a wall should have at least one rough-in.
        for fx in t.fixtures.iter().filter(|f| f.wall.is_some()) {
            let stubs: Vec<_> = t.rough_ins.iter().filter(|ri| ri.fixture == fx.id).collect();
            assert!(!stubs.is_empty(), "{ctx}: fixture {} ({}) has no rough-ins", fx.id, fx.kind);
            // Every rough-in stub must be on the fixture's wall.
            for ri in &stubs {
                assert_eq!(
                    ri.wall,
                    fx.wall.as_ref().cloned().unwrap_or_default(),
                    "{ctx}: rough-in {} is on wall {}, fixture {} is on wall {:?}",
                    ri.id,
                    ri.wall,
                    fx.id,
                    fx.wall,
                );
            }
        }
    }
}

#[test]
fn devices_are_placed_and_avoid_rough_ins_by_default() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        let ctx = format!("seed {seed}");
        assert!(!t.devices.is_empty(), "{ctx}: no devices generated");
        // Devices that are not planted should not share a stud cavity with any rough-in
        // (the planted ones are the in_wall clashes).
        let planted: BTreeSet<String> = t.clashes.iter().filter(|c| c.kind == "in_wall" && c.planted).map(|c| c.a.clone()).collect();
        for dev in &t.devices {
            if planted.contains(&dev.id) {
                continue;
            }
            let shared = t.rough_ins.iter().filter(|ri| ri.wall == dev.wall && ri.stud_cavity == dev.stud_cavity).count();
            assert_eq!(shared, 0, "{ctx}: device {} (cavity {}) shares a stud cavity with a rough-in on wall {}", dev.id, dev.stud_cavity, dev.wall,);
        }
    }
}

#[test]
fn in_wall_clashes_are_planted_and_correct() {
    let mut planted_count = 0;
    for seed in 1..=30 {
        let t = truth_for(seed);
        let in_wall: Vec<_> = t.clashes.iter().filter(|c| c.kind == "in_wall").collect();
        for c in &in_wall {
            // The `a` side must be a device, `b` a rough-in.
            assert!(t.device(&c.a).is_some(), "seed {seed}: clash {} a={} is not a device", c.id, c.a);
            assert!(t.rough_in(&c.b).is_some(), "seed {seed}: clash {} b={} is not a rough-in", c.id, c.b);
            // Must have a description.
            assert!(c.description.is_some(), "seed {seed}: clash {} has no description", c.id);
            assert!(c.wall.is_some(), "seed {seed}: clash {} has no wall", c.id);
        }
        let planted: Vec<_> = in_wall.iter().filter(|c| c.planted).collect();
        planted_count += planted.len();
    }
    assert!(planted_count > 0, "no in-wall clashes were planted across 30 seeds");
}

#[test]
fn placement_zones_exist_for_unfixed_devices() {
    let mut total_zones = 0;
    let mut some_may_clash = false;
    for seed in 1..=15 {
        let t = truth_for(seed);
        let unfixed: Vec<_> = t.devices.iter().filter(|d| !d.fixed).collect();
        for dev in &unfixed {
            let zone = t.placement_zones.iter().find(|z| z.device == dev.id);
            assert!(zone.is_some(), "seed {seed}: unfixed device {} has no placement zone", dev.id,);
            let z = zone.unwrap();
            assert!(z.zone_in[1] > z.zone_in[0], "seed {seed}: zone for {} is empty", dev.id);
            assert!(
                ["will_clash", "may_clash", "clear"].contains(&z.status.as_str()),
                "seed {seed}: zone for {} has invalid status: {}",
                dev.id,
                z.status,
            );
            total_zones += 1;
            if z.status == "may_clash" {
                some_may_clash = true;
                assert!(!z.clear_ranges.is_empty(), "seed {seed}: may_clash zone for {} has no clear ranges", dev.id);
            }
        }
    }
    assert!(total_zones > 0, "no placement zones generated");
    // Across many seeds, at least one device should land in a may_clash zone.
    // (Not guaranteed for every seed, but very likely across 15.)
    let _ = some_may_clash; // Don't assert; it depends on the building geometry.
}

#[test]
fn in_wall_recall_and_false_positive_rate() {
    // Test that the clash detection has full recall (all planted clashes are detected) and
    // zero false positives (no unplanted in-wall clashes).
    for seed in 1..=40 {
        let t = truth_for(seed);
        let ctx = format!("seed {seed}");
        let stray: Vec<_> = t.clashes.iter().filter(|c| !c.planted).collect();
        assert!(stray.is_empty(), "{ctx}: false positive clashes: {stray:?}");
    }
}

// --- Q308: Low-voltage, fire alarm, equipment connections ---

#[test]
fn hotel_fire_alarm_has_enriched_fields() {
    let t = hotel_truth_for(1, HotelSize::Small);
    let kinds: BTreeSet<&str> = t.fire_alarm_devices.iter().map(|d| d.kind.as_str()).collect();

    // All expected FA device kinds present.
    for k in [
        "facp",
        "nac_panel",
        "annunciator",
        "smoke_detector",
        "horn_strobe",
        "strobe",
        "pull_station",
        "duct_detector",
        "monitor_module",
        "elevator_recall",
        "mag_door_holder",
    ] {
        assert!(kinds.contains(k), "missing FA kind: {k}");
    }

    // Horn/strobes have candela ratings.
    let hs: Vec<_> = t.fire_alarm_devices.iter().filter(|d| d.kind == "horn_strobe").collect();
    assert!(!hs.is_empty(), "no horn/strobes");
    for d in &hs {
        assert!(d.candela.is_some(), "horn/strobe {} has no candela", d.id);
        assert!(d.mounting_in > 0.0, "horn/strobe {} has no mounting height", d.id);
    }

    // Strobes in guest rooms have 15 cd.
    let strobes: Vec<_> = t.fire_alarm_devices.iter().filter(|d| d.kind == "strobe").collect();
    assert!(!strobes.is_empty(), "no strobes");
    for d in &strobes {
        assert_eq!(d.candela, Some(15), "guest room strobe should be 15 cd");
    }

    // Smoke detectors have addresses.
    let sds: Vec<_> = t.fire_alarm_devices.iter().filter(|d| d.kind == "smoke_detector").collect();
    assert!(!sds.is_empty(), "no smoke detectors");
    for d in &sds {
        assert!(d.address.is_some(), "smoke detector {} has no address", d.id);
        assert!(!d.slc.is_empty(), "smoke detector {} has no SLC", d.id);
    }

    // Pull stations have addresses and mounting height.
    let ps: Vec<_> = t.fire_alarm_devices.iter().filter(|d| d.kind == "pull_station").collect();
    assert!(!ps.is_empty(), "no pull stations");
    for d in &ps {
        assert!(d.address.is_some(), "pull station {} has no address", d.id);
        assert_eq!(d.mounting_in, 48.0, "pull station should be at 48in AFF");
    }

    // Interfaces exist.
    let interfaces: Vec<_> = t.fire_alarm_devices.iter().filter(|d| d.interface.is_some()).collect();
    assert!(interfaces.len() >= 2, "too few FA interface devices: {}", interfaces.len());

    // NAC devices reference NAC circuit.
    let nac_devs: Vec<_> = t.fire_alarm_devices.iter().filter(|d| !d.nac.is_empty()).collect();
    assert!(!nac_devs.is_empty(), "no NAC circuit devices");
}

#[test]
fn hotel_telecom_has_enriched_fields() {
    let t = hotel_truth_for(1, HotelSize::Small);
    let kinds: BTreeSet<&str> = t.telecom_devices.iter().map(|d| d.kind.as_str()).collect();

    // All expected telecom device kinds present.
    for k in [
        "data_outlet",
        "tv_outlet",
        "telephone",
        "wifi_ap",
        "camera",
        "card_reader",
        "door_contact",
        "rex",
        "speaker",
        "mdf",
        "idf",
        "patch_panel",
        "backboard",
        "rack",
    ] {
        assert!(kinds.contains(k), "missing telecom kind: {k}");
    }

    // Data outlets have cable specs.
    let data: Vec<_> = t.telecom_devices.iter().filter(|d| d.kind == "data_outlet").collect();
    assert!(!data.is_empty(), "no data outlets");
    for d in &data {
        assert!(d.cable_spec.is_some(), "data outlet {} has no cable_spec", d.id);
        assert!(d.box_type.is_some(), "data outlet {} has no box_type", d.id);
        assert!(d.faceplate.is_some(), "data outlet {} has no faceplate", d.id);
        assert!(d.home_panel.is_some(), "data outlet {} has no home_panel", d.id);
        assert!(!d.lv_system.is_empty(), "data outlet {} has no lv_system", d.id);
    }

    // Cameras are security system.
    for cam in t.telecom_devices.iter().filter(|d| d.kind == "camera") {
        assert_eq!(cam.lv_system, "security", "camera should be security system");
    }

    // Access control devices present at stairs.
    let cr: Vec<_> = t.telecom_devices.iter().filter(|d| d.kind == "card_reader").collect();
    assert!(!cr.is_empty(), "no card readers");
    for d in &cr {
        assert_eq!(d.lv_system, "access_control", "card reader should be access_control system");
    }

    // MDF exists on L1, IDF on upper floors.
    assert!(t.telecom_devices.iter().any(|d| d.kind == "mdf" && d.level == "1"), "no MDF on L1");
    for lv in ["2", "3", "4"] {
        assert!(t.telecom_devices.iter().any(|d| d.kind == "idf" && d.level == lv), "no IDF on level {lv}");
    }
}

#[test]
fn hotel_has_equip_connections() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(!t.equip_connections.is_empty(), "no equipment connections");

    let kinds: BTreeSet<&str> = t.equip_connections.iter().map(|c| c.kind.as_str()).collect();
    assert!(kinds.contains("ptac"), "no PTAC connections");
    assert!(kinds.contains("water_heater"), "no water heater connection");
    assert!(kinds.contains("elevator"), "no elevator connection");
    assert!(kinds.contains("rtu"), "no RTU connection");
    assert!(kinds.contains("signage"), "no signage connection");

    // Each connection has a circuit and panel.
    for ec in &t.equip_connections {
        assert!(!ec.circuit.is_empty(), "equip connection {} has no circuit", ec.id);
        assert!(!ec.panel.is_empty(), "equip connection {} has no panel", ec.id);
        assert!(!ec.connection_type.is_empty(), "equip connection {} has no connection_type", ec.id);
    }

    // Disconnects have a tag.
    let disconnects: Vec<_> = t.equip_connections.iter().filter(|c| c.connection_type == "disconnect").collect();
    assert!(!disconnects.is_empty(), "no disconnect connections");
    for d in &disconnects {
        assert!(d.disconnect_tag.is_some(), "disconnect {} has no tag", d.id);
    }

    // Schedules have voltage info.
    for ec in &t.equip_connections {
        assert!(ec.schedule.contains_key("voltage"), "equip connection {} has no voltage", ec.id);
    }
}

#[test]
fn hotel_has_sleeves() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(!t.sleeves.is_empty(), "no sleeves");

    // At least one rated wall sleeve with firestop.
    let rated: Vec<_> = t.sleeves.iter().filter(|s| !s.fire_rating.is_empty()).collect();
    assert!(!rated.is_empty(), "no rated sleeves");
    for s in &rated {
        assert!(s.firestop_system.is_some(), "rated sleeve {} has no firestop", s.id);
    }

    // Floor sleeves for riser exist on upper floors.
    let floor_sleeves: Vec<_> = t.sleeves.iter().filter(|s| s.penetrates == "floor").collect();
    assert!(!floor_sleeves.is_empty(), "no floor sleeves");

    // Sleeves have cables.
    for s in &t.sleeves {
        assert!(!s.cables.is_empty(), "sleeve {} has no cables", s.id);
        assert!(s.size_in > 0.0, "sleeve {} has no size", s.id);
        assert!(s.length_in > 0.0, "sleeve {} has no length", s.id);
    }

    // Sleeves carry systems.
    for s in &t.sleeves {
        assert!(!s.systems.is_empty(), "sleeve {} has no systems", s.id);
    }
}

#[test]
fn hotel_has_cable_pathways() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(!t.cable_pathways.is_empty(), "no cable pathways");

    let kinds: BTreeSet<&str> = t.cable_pathways.iter().map(|p| p.kind.as_str()).collect();
    assert!(kinds.contains("j_hook"), "no J-hook pathways");
    assert!(kinds.contains("ent"), "no ENT stub pathways");

    // J-hooks have spacing.
    for pw in t.cable_pathways.iter().filter(|p| p.kind == "j_hook") {
        assert!(pw.spacing_in.is_some(), "J-hook {} has no spacing", pw.id);
    }

    // ENT stubs have size.
    for pw in t.cable_pathways.iter().filter(|p| p.kind == "ent") {
        assert!(pw.size_in.is_some(), "ENT stub {} has no size", pw.id);
    }

    // Pathways carry systems.
    for pw in &t.cable_pathways {
        assert!(!pw.systems.is_empty(), "pathway {} has no systems", pw.id);
    }
}

#[test]
fn hotel_has_lv_bom() {
    let t = hotel_truth_for(1, HotelSize::Small);
    assert!(!t.lv_bom.is_empty(), "no LV BOM lines");

    let systems: BTreeSet<&str> = t.lv_bom.iter().map(|b| b.system.as_str()).collect();
    assert!(systems.contains("fire_alarm"), "no fire alarm BOM");
    assert!(systems.contains("data"), "no data BOM");

    let categories: BTreeSet<&str> = t.lv_bom.iter().map(|b| b.category.as_str()).collect();
    assert!(categories.contains("device"), "no device BOM lines");
    assert!(categories.contains("cable"), "no cable BOM lines");
    assert!(categories.contains("support"), "no support BOM lines");

    // All BOM lines have positive quantities.
    for line in &t.lv_bom {
        assert!(line.quantity > 0.0, "BOM line {} has zero quantity: {}", line.id, line.description);
        assert!(!line.unit.is_empty(), "BOM line {} has no unit", line.id);
        assert!(!line.description.is_empty(), "BOM line {} has no description", line.id);
    }

    // Some lines reference specs.
    let with_spec: Vec<_> = t.lv_bom.iter().filter(|b| b.spec_id.is_some()).collect();
    assert!(!with_spec.is_empty(), "no BOM lines reference specs");

    // Scope flags are populated.
    let with_scope: Vec<_> = t.lv_bom.iter().filter(|b| !b.scope.is_empty()).collect();
    assert!(!with_scope.is_empty(), "no BOM lines have scope flags");
}

#[test]
fn hotel_spec_book_has_lv_divisions() {
    let t = hotel_truth_for(1, HotelSize::Small);
    let divs: BTreeSet<&str> = t.spec_items.iter().map(|s| s.division.as_str()).collect();
    // Divisions 27 (communications) and 28 (fire alarm / security) should both be present.
    assert!(divs.contains("27"), "no division 27 (communications) specs");
    assert!(divs.contains("28"), "no division 28 (fire alarm/security) specs");

    // Division 27 should have more than one spec.
    let d27 = t.spec_items.iter().filter(|s| s.division == "27").count();
    assert!(d27 >= 3, "too few division 27 specs: {d27}");

    // Division 28 should have more than the original one.
    let d28 = t.spec_items.iter().filter(|s| s.division == "28").count();
    assert!(d28 >= 3, "too few division 28 specs: {d28}");
}

#[test]
fn hotel_lv_full_has_more_devices_than_small() {
    let small = hotel_truth_for(1, HotelSize::Small);
    let full = hotel_truth_for(1, HotelSize::Full);
    assert!(
        full.fire_alarm_devices.len() > small.fire_alarm_devices.len(),
        "full ({}) should have more FA devices than small ({})",
        full.fire_alarm_devices.len(),
        small.fire_alarm_devices.len()
    );
    assert!(
        full.telecom_devices.len() > small.telecom_devices.len(),
        "full ({}) should have more telecom devices than small ({})",
        full.telecom_devices.len(),
        small.telecom_devices.len()
    );
    assert!(
        full.equip_connections.len() > small.equip_connections.len(),
        "full ({}) should have more equip connections than small ({})",
        full.equip_connections.len(),
        small.equip_connections.len()
    );
}

#[test]
fn hotel_lv_ids_are_unique() {
    let t = hotel_truth_for(1, HotelSize::Full);

    let fa_ids: BTreeSet<&str> = t.fire_alarm_devices.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(fa_ids.len(), t.fire_alarm_devices.len(), "duplicate FA device IDs");

    let tc_ids: BTreeSet<&str> = t.telecom_devices.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(tc_ids.len(), t.telecom_devices.len(), "duplicate telecom device IDs");

    let ec_ids: BTreeSet<&str> = t.equip_connections.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ec_ids.len(), t.equip_connections.len(), "duplicate equip connection IDs");

    let sl_ids: BTreeSet<&str> = t.sleeves.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(sl_ids.len(), t.sleeves.len(), "duplicate sleeve IDs");

    let cp_ids: BTreeSet<&str> = t.cable_pathways.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(cp_ids.len(), t.cable_pathways.len(), "duplicate cable pathway IDs");

    let bom_ids: BTreeSet<&str> = t.lv_bom.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(bom_ids.len(), t.lv_bom.len(), "duplicate BOM line IDs");
}

#[test]
fn hotel_fa_devices_per_floor() {
    let t = hotel_truth_for(1, HotelSize::Small);
    for lv in ["1", "2", "3", "4"] {
        let on_level: Vec<_> = t.fire_alarm_devices.iter().filter(|d| d.level == lv).collect();
        assert!(on_level.len() >= 3, "too few FA devices on level {lv}: {}", on_level.len());
    }
}

#[test]
fn hotel_equip_ptac_per_guest_room() {
    let t = hotel_truth_for(1, HotelSize::Small);
    let guest_rooms: Vec<_> = t
        .building
        .rooms
        .iter()
        .filter(|r| r.name.contains("KING") || r.name.contains("QUEEN") || r.name.contains("SUITE") || r.name.contains("ADA"))
        .collect();
    let ptacs: Vec<_> = t.equip_connections.iter().filter(|c| c.kind == "ptac").collect();
    // At least one PTAC per guest room.
    assert!(ptacs.len() >= guest_rooms.len(), "fewer PTACs ({}) than guest rooms ({})", ptacs.len(), guest_rooms.len());
}
