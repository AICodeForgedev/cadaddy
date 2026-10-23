//! Duct reader tests: generate fixture sheets, align them with the sheet reader, read ducts back,
//! score against truth.
//!
//! Every score here places the reader's runs with the alignment `sheet_reader` read from the
//! sheet's grid, never the truth's placement. Thresholds: size and system accuracy ≥ 0.70,
//! precision ≥ 0.80 and recall ≥ 0.75 on average over seeds 1–5, no sheet below 0.50.

use cadaddy_doc::Drawing;
use cadaddy_mepfix::duct_reader::{self, DuctNetwork, Source, Status};
use cadaddy_mepfix::score;
use cadaddy_mepfix::sheet_reader::{self, RecognisedSheet};
use cadaddy_mepfix::truth::{Sheet, Truth};
use cadaddy_mepfix::{generate, truth_for};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// One mechanical plan of a fixture set: the truth's sheet, its drawing, and what the sheet
/// reader made of it.
struct MechSheet {
    sheet: Sheet,
    drawing: Drawing,
    rec: RecognisedSheet,
}

/// Generate a seed's set, recognise every sheet (the common grid needs them all), and return the
/// truth and its mechanical plans.
fn mech_sheets(seed: u64) -> (Truth, Vec<MechSheet>) {
    let set = generate(seed).unwrap_or_else(|e| panic!("generate({seed}) failed: {e}"));
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = sheet_reader::recognise_fixture_set(&dxf);
    let mut out = Vec::new();
    for sh in &set.truth.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        let bytes = set.file(&sh.dxf).unwrap_or_else(|| panic!("seed {seed}: no {}", sh.dxf));
        let drawing = cadaddy_io::read_dxf(bytes).unwrap_or_else(|e| panic!("seed {seed} {}: {e}", sh.dxf));
        let rec = map.sheets.iter().find(|r| r.number == sh.number).cloned().unwrap_or_else(|| panic!("seed {seed}: {} not recognised", sh.number));
        out.push(MechSheet { sheet: sh.clone(), drawing, rec });
    }
    (set.truth, out)
}

/// Score every mechanical plan of the seeds.
fn scores(seeds: impl IntoIterator<Item = u64>) -> Vec<(String, score::RunScore)> {
    let mut out = Vec::new();
    for seed in seeds {
        let (t, sheets) = mech_sheets(seed);
        for m in &sheets {
            let sc = duct_reader::read_and_score(&m.drawing, &t, &m.rec);
            out.push((format!("seed={seed} {}", m.sheet.number), sc));
        }
    }
    out
}

/// Print the table, then assert the averages and the worst sheet.
fn assert_thresholds(what: &str, rows: &[(String, score::RunScore)]) {
    for (label, sc) in rows {
        eprintln!(
            "{what} {label}: P={:.1}% R={:.1}% size={:.1}% sys={:.1}% truth={:.1}ft found={:.1}ft",
            sc.precision * 100.0,
            sc.recall * 100.0,
            sc.size_accuracy * 100.0,
            sc.system_accuracy * 100.0,
            sc.truth_length_ft,
            sc.found_length_ft,
        );
    }
    let rows: Vec<&score::RunScore> = rows.iter().filter(|(_, s)| s.truth_length_ft > 1.0).map(|(_, s)| s).collect();
    assert!(!rows.is_empty(), "{what}: no mechanical sheets");
    let n = rows.len() as f64;
    let avg = |f: fn(&score::RunScore) -> f64| rows.iter().map(|s| f(s)).sum::<f64>() / n;
    let (p, r, size, sys) = (avg(|s| s.precision), avg(|s| s.recall), avg(|s| s.size_accuracy), avg(|s| s.system_accuracy));
    eprintln!("{what} average over {n} sheets: P={:.1}% R={:.1}% size={:.1}% sys={:.1}%", p * 100.0, r * 100.0, size * 100.0, sys * 100.0);
    assert!(p >= 0.80, "{what}: average precision {p:.3} < 0.80");
    assert!(r >= 0.75, "{what}: average recall {r:.3} < 0.75");
    assert!(size >= 0.70, "{what}: average size accuracy {size:.3} < 0.70");
    assert!(sys >= 0.70, "{what}: average system accuracy {sys:.3} < 0.70");
    for s in &rows {
        assert!(s.precision >= 0.5 && s.recall >= 0.5, "{what}: a sheet below 0.50: P={:.3} R={:.3}", s.precision, s.recall);
    }
}

#[test]
fn duct_reader_finds_runs_on_fixture_sheets() {
    // Seeds 1-5, each with two mechanical sheets (M-101 level 1, M-102 level 2).
    assert_thresholds("fixture", &scores(1..=5));
}

#[test]
fn radius_elbow_sheet_recall() {
    // Seed 4 draws no centrelines: its runs come from wall pairs alone (and chaining that skips
    // the pairs already used rather than stopping at the first).
    for (label, sc) in scores([4]) {
        eprintln!("{label}: R={:.1}%", sc.recall * 100.0);
        assert!(sc.recall >= 0.60, "{label}: recall {:.3} < 0.60", sc.recall);
    }
    // Sheets drawn with radius elbows: each pair of concentric DUCT arcs reads as one elbow.
    let (mut arcs, mut elbows) = (0, 0);
    for seed in 1..=12 {
        let (_, sheets) = mech_sheets(seed);
        for m in &sheets {
            arcs += m.drawing.model.iter().filter(|e| matches!(e.kind, cadaddy_doc::EntityKind::Arc(_)) && e.common.layer.as_str() == "DUCT").count();
            let net = duct_reader::read_ducts(&m.drawing);
            elbows += net.fittings.iter().filter(|f| f.kind == "elbow" && f.source == Source::Geometry && f.status == Status::Read).count();
        }
    }
    eprintln!("{arcs} duct arcs, {elbows} radius elbows read");
    assert!(arcs > 0, "no radius elbows in seeds 1..=12");
    assert!(elbows as f64 >= arcs as f64 / 2.0 * 0.8, "{elbows} radius elbows read from {arcs} arcs");
}

#[test]
fn size_accuracy_when_tags_are_found() {
    // Of the truth length that carries a size tag, the reader must get the size right: the
    // tag is read, not the wall gap.
    let mut acc = Vec::new();
    for seed in 1..=5 {
        let (t, sheets) = mech_sheets(seed);
        let mut tagged = t.clone();
        tagged.segments.retain(|s| s.tag.is_some());
        for m in &sheets {
            let sc = duct_reader::read_and_score(&m.drawing, &tagged, &m.rec);
            eprintln!("seed={seed} {}: tagged-run size accuracy {:.1}% (recall {:.1}%)", m.sheet.number, sc.size_accuracy * 100.0, sc.recall * 100.0);
            if sc.truth_length_ft > 1.0 {
                acc.push(sc.size_accuracy);
            }
            // A run sized `read` has a tag, and its size is the tag's.
            let net = duct_reader::read_ducts(&m.drawing);
            for r in net.runs.iter().filter(|r| r.size.status == Status::Read) {
                let tag = r.tag.and_then(|i| net.tags.get(i)).unwrap_or_else(|| panic!("seed {seed}: a run sized `read` has no tag: {r:?}"));
                let [w, d] = tag.size_in.unwrap_or_default();
                assert_eq!(r.width_in, w, "seed {seed}: {}", tag.text);
                assert_eq!(if r.shape == "rect" { r.depth_in } else { 0.0 }, d, "seed {seed}: {}", tag.text);
            }
        }
    }
    let avg = acc.iter().sum::<f64>() / acc.len().max(1) as f64;
    eprintln!("tagged-run size accuracy average {:.1}%", avg * 100.0);
    assert!(!acc.is_empty());
    assert!(avg >= 0.85, "tagged-run size accuracy {avg:.3} < 0.85");
}

#[test]
fn defaulted_depths_are_flagged_defaulted() {
    let mut defaulted = 0;
    for seed in 1..=5 {
        let (_, sheets) = mech_sheets(seed);
        for m in &sheets {
            let net = duct_reader::read_ducts(&m.drawing);
            for r in &net.runs {
                // The stock 12" depth: always `defaulted`, never shown as read.
                if r.shape == "rect" && r.depth.source == Source::Default {
                    defaulted += 1;
                    assert_eq!(r.depth.status, Status::Defaulted, "seed {seed}: {r:?}");
                    assert_eq!(r.status(), Status::Defaulted, "seed {seed}: {r:?}");
                    assert!(r.depth_in <= duct_reader::DEFAULT_DEPTH_IN, "seed {seed}: {r:?}");
                }
                // Read means a tag said so.
                if r.depth.status == Status::Read || r.size.status == Status::Read {
                    assert!(r.tag.is_some() && r.size.source == Source::Tag, "seed {seed}: read without a tag: {r:?}");
                }
                // The wall gap alone is a guess.
                if r.size.source == Source::Geometry {
                    assert_eq!(r.size.status, Status::Inferred, "seed {seed}: {r:?}");
                }
                if r.system.is_none() {
                    assert_eq!(r.system_from.status, Status::Defaulted, "seed {seed}: {r:?}");
                }
            }
            for f in &net.fittings {
                assert!(matches!(f.source, Source::Geometry | Source::Tag), "{f:?}");
            }
            for e in &net.equipment {
                assert_eq!(e.source, Source::Tag, "{e:?}");
                assert_eq!(e.status == Status::Inferred, e.kind == "unknown", "{e:?}");
            }
        }
    }
    eprintln!("{defaulted} runs with the defaulted depth");
}

/// The truth's branch points on a level: duct nodes where three or more horizontal runs meet.
fn truth_branches(t: &Truth, level: &str) -> usize {
    let horizontal: BTreeSet<&str> =
        t.segments.iter().filter(|s| s.kind == "duct" && !s.vertical && s.level == level).map(|s| s.id.as_str()).collect();
    t.nodes.iter().filter(|n| n.level == level && n.segments.iter().filter(|id| horizontal.contains(id.as_str())).count() >= 3).count()
}

/// The truth's terminals on a level whose ducts lead (on that level) to a box.
fn truth_terminals_reaching_equipment(t: &Truth, level: &str) -> usize {
    let nodes: BTreeMap<&str, &cadaddy_mepfix::truth::Node> = t.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let segs: BTreeMap<&str, &cadaddy_mepfix::truth::Segment> = t.segments.iter().filter(|s| s.kind == "duct").map(|s| (s.id.as_str(), s)).collect();
    let reaches = |start: &str| {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut q = VecDeque::from([start]);
        while let Some(id) = q.pop_front() {
            if !seen.insert(id) {
                continue;
            }
            let Some(n) = nodes.get(id) else { continue };
            if n.kind == "equipment" {
                return true;
            }
            for s in n.segments.iter().filter_map(|s| segs.get(s.as_str())) {
                for next in [s.from.as_str(), s.to.as_str()] {
                    if nodes.get(next).is_some_and(|m| m.level == level) {
                        q.push_back(next);
                    }
                }
            }
        }
        false
    };
    t.terminals
        .iter()
        .filter(|tm| tm.level == level && ["SA", "RA", "EA", "OA"].contains(&tm.system.as_str()))
        .filter(|tm| tm.node.as_deref().is_some_and(&reaches))
        .count()
}

fn within(found: usize, truth: usize, share: f64) -> bool {
    (found as f64 - truth as f64).abs() <= truth as f64 * share
}

#[test]
fn network_graph_matches_truth_connectivity() {
    for seed in 1..=5 {
        let (t, sheets) = mech_sheets(seed);
        for m in &sheets {
            let level = m.sheet.level.clone().unwrap_or_default();
            let net: DuctNetwork = duct_reader::read_ducts(&m.drawing);
            let g = &net.graph;
            // Every duct edge joins two nodes and carries its run's size.
            for e in g.edges.iter().filter(|e| e.run.is_some()) {
                assert!(e.a < g.nodes.len() && e.b < g.nodes.len());
                let r = e.run.and_then(|i| net.runs.get(i)).unwrap_or_else(|| panic!("edge without a run"));
                assert_eq!(e.size_in, r.size_in());
                assert_eq!(e.system, r.system);
            }
            let (fb, tb) = (g.branches(), truth_branches(&t, &level));
            let (fc, tc) = (g.terminals_reaching_equipment(), truth_terminals_reaching_equipment(&t, &level));
            eprintln!(
                "seed={seed} {}: branches {fb} (truth {tb}), terminals reaching a box {fc} (truth {tc}); {} nodes, {} edges",
                m.sheet.number,
                g.nodes.len(),
                g.edges.len()
            );
            assert!(within(fb, tb, 0.2), "seed {seed} {}: {fb} tees/crosses vs truth {tb}", m.sheet.number);
            assert!(within(fc, tc, 0.2), "seed {seed} {}: {fc} terminals reach a box vs truth {tc}", m.sheet.number);
        }
    }
}

#[test]
fn aligned_sheets_meet_the_thresholds() {
    // Mechanical plans the generator turned, moved off the origin or drew at another scale,
    // read and placed only through the sheet reader's grid alignment.
    let mut rows = Vec::new();
    let (mut rotated, mut shifted, mut rescaled) = (0, 0, 0);
    for seed in 1..=40 {
        let (t, sheets) = mech_sheets(seed);
        for m in &sheets {
            let rot = m.sheet.model_from_building.rotation_deg.abs() > 1.0;
            let shift = m.sheet.paper_from_model.origin_in.iter().any(|v| v.abs() > 1.0);
            let scale = (m.sheet.scale - 0.125).abs() > 1e-9;
            if !(rot || shift || scale) {
                continue;
            }
            rotated += usize::from(rot);
            shifted += usize::from(shift);
            rescaled += usize::from(scale);
            assert!(m.rec.alignment.is_some(), "seed {seed} {}: the sheet reader did not align it", m.sheet.number);
            let sc = duct_reader::read_and_score(&m.drawing, &t, &m.rec);
            rows.push((format!("seed={seed} {} rot={rot} shift={shift} scale={}", m.sheet.number, m.sheet.scale), sc));
        }
        if rotated >= 2 && shifted >= 2 && rescaled >= 2 && rows.len() >= 6 {
            break;
        }
    }
    eprintln!("{rotated} rotated, {shifted} shifted, {rescaled} rescaled sheets");
    assert!(rotated > 0, "no rotated mechanical sheet in seeds 1..=40");
    assert_thresholds("aligned", &rows);
}

#[test]
fn duct_reader_detects_terminals_and_equipment() {
    let (_, sheets) = mech_sheets(3);
    let (mut total_terminals, mut total_equipment) = (0, 0);
    for m in &sheets {
        let network = duct_reader::read_ducts(&m.drawing);
        total_terminals += network.terminals.len();
        total_equipment += network.equipment.len();
        eprintln!(
            "seed=3 {}: {} runs, {} fittings, {} terminals, {} equipment, {} tags",
            m.sheet.number,
            network.runs.len(),
            network.fittings.len(),
            network.terminals.len(),
            network.equipment.len(),
            network.tags.len(),
        );
    }
    // The building has terminals and equipment on each level.
    assert!(total_terminals > 0, "no terminals detected");
    assert!(total_equipment > 0, "no equipment detected");
}

#[test]
fn duct_reader_reads_legend_abbreviations() {
    // Seed 5's mechanical sheets print the system abbreviations.
    let (_, sheets) = mech_sheets(5);
    let found = sheets.iter().any(|m| {
        let network = duct_reader::read_ducts(&m.drawing);
        eprintln!("seed=5 {}: abbreviations = {:?}", m.sheet.number, network.abbreviations);
        !network.abbreviations.is_empty()
    });
    assert!(found, "no abbreviations found on any mechanical sheet for seed 5");
}

#[test]
fn duct_reader_network_graph_has_system_assignments() {
    let (_, sheets) = mech_sheets(2);
    let (mut assigned, mut total) = (0, 0);
    for m in &sheets {
        let network = duct_reader::read_ducts(&m.drawing);
        total += network.runs.len();
        assigned += network.runs.iter().filter(|r| r.system.is_some()).count();
    }
    eprintln!("System assignment: {assigned}/{total} runs");
    assert!(total > 0);
    let ratio = assigned as f64 / total as f64;
    assert!(ratio > 0.5, "system assignment ratio {ratio:.2} too low");
}

#[test]
fn perfect_reader_scores_perfectly() {
    // Verify the scoring infrastructure: the truth's own runs should score 100%.
    for seed in [1, 7, 15] {
        let t = truth_for(seed);
        for lv in ["1", "2"] {
            let perfect = score::perfect(&t, lv, "duct");
            let sc = score::score_runs(&t, lv, "duct", &perfect, 0.5);
            assert!(
                sc.precision > 0.99 && sc.recall > 0.99,
                "seed={seed} level={lv}: perfect reader should score ~1.0, got P={:.3} R={:.3}",
                sc.precision,
                sc.recall,
            );
        }
    }
}
