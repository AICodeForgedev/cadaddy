//! The fixture generator: deterministic, its truth consistent with itself, its sheets carrying
//! what the truth says they carry (read back through our own PDF and DXF readers), styles varied,
//! and the scorer scoring.

use cadaddy_mepfix::truth::Truth;
use cadaddy_mepfix::{generate, layout, score, truth_for};
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
    assert_eq!(kinds, BTreeSet::from(["below_ceiling".to_string(), "hard".into(), "into_structure".into()]));
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
            !sh.legend.is_empty() || !sh.number.ends_with("01") || sh.number == "G-001" || sh.number.contains("60"),
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
