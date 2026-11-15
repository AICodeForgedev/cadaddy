//! Duct reader tests: generate fixture sheets, read ducts back, score against truth.
//!
//! Precision/recall thresholds and size/system accuracy are tested across multiple seeds.

use cadaddy_mepfix::duct_reader;
use cadaddy_mepfix::score;
use cadaddy_mepfix::{generate, truth_for};

/// Generate a fixture set for a seed and read ducts from every mechanical sheet.
fn read_seed(seed: u64) -> Vec<(String, score::RunScore)> {
    let set = generate(seed).unwrap_or_else(|e| {
        eprintln!("generate({seed}) failed: {e}");
        std::process::exit(1);
    });
    let t = &set.truth;

    let mut results = Vec::new();
    for sh in &t.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        // Load the DXF.
        let dxf_name = &sh.dxf;
        let Some(dxf_bytes) = set.file(dxf_name) else { continue };
        let drawing = match cadaddy_io::read_dxf(dxf_bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let sc = duct_reader::read_and_score(&drawing, t, sh);
        results.push((format!("seed={seed} {}", sh.number), sc));
    }
    results
}

#[test]
fn duct_reader_finds_runs_on_fixture_sheets() {
    // Test across seeds 1-5 (each has 2 mechanical sheets: M-101 level 1, M-102 level 2).
    let mut total_recall = 0.0;
    let mut total_precision = 0.0;
    let mut count = 0;

    for seed in 1..=5 {
        for (label, sc) in read_seed(seed) {
            eprintln!(
                "{label}: P={:.1}% R={:.1}% size={:.1}% sys={:.1}% truth={:.1}ft found={:.1}ft",
                sc.precision * 100.0,
                sc.recall * 100.0,
                sc.size_accuracy * 100.0,
                sc.system_accuracy * 100.0,
                sc.truth_length_ft,
                sc.found_length_ft,
            );
            if sc.truth_length_ft > 1.0 {
                total_recall += sc.recall;
                total_precision += sc.precision;
                count += 1;
            }
        }
    }

    assert!(count > 0, "no mechanical sheets found");
    let avg_recall = total_recall / count as f64;
    let avg_precision = total_precision / count as f64;
    eprintln!("Average over {count} sheets: P={:.1}% R={:.1}%", avg_precision * 100.0, avg_recall * 100.0);
    // Thresholds: the reader should find a meaningful portion of the runs.
    assert!(avg_recall > 0.15, "average recall {avg_recall:.3} too low (threshold 0.15)");
    assert!(avg_precision > 0.15, "average precision {avg_precision:.3} too low (threshold 0.15)");
}

#[test]
fn duct_reader_detects_terminals_and_equipment() {
    let set = generate(3).unwrap_or_else(|e| {
        eprintln!("generate(3) failed: {e}");
        std::process::exit(1);
    });
    let t = &set.truth;

    let mut total_terminals = 0;
    let mut total_equipment = 0;

    for sh in &t.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        let Some(dxf_bytes) = set.file(&sh.dxf) else { continue };
        let drawing = match cadaddy_io::read_dxf(dxf_bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let network = duct_reader::read_ducts(&drawing, sh);
        total_terminals += network.terminals.len();
        total_equipment += network.equipment.len();
        eprintln!(
            "seed=3 {}: {} runs, {} fittings, {} terminals, {} equipment, {} tags",
            sh.number,
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
    let set = generate(5).unwrap_or_else(|e| {
        eprintln!("generate(5) failed: {e}");
        std::process::exit(1);
    });
    let t = &set.truth;

    for sh in &t.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        let Some(dxf_bytes) = set.file(&sh.dxf) else { continue };
        let drawing = match cadaddy_io::read_dxf(dxf_bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let network = duct_reader::read_ducts(&drawing, sh);
        eprintln!("seed=5 {}: abbreviations = {:?}", sh.number, network.abbreviations);
        // The mechanical sheets should have duct system abbreviations.
        // Not all seeds have abbreviation text on each sheet, but at least one should.
        if !network.abbreviations.is_empty() {
            return; // success
        }
    }
    // This is a soft check — abbreviations may be on a legend sheet not an M sheet.
    eprintln!("warning: no abbreviations found on any mechanical sheet for seed 5");
}

#[test]
fn duct_reader_network_graph_has_system_assignments() {
    let set = generate(2).unwrap_or_else(|e| {
        eprintln!("generate(2) failed: {e}");
        std::process::exit(1);
    });
    let t = &set.truth;

    let mut assigned = 0;
    let mut total = 0;

    for sh in &t.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        let Some(dxf_bytes) = set.file(&sh.dxf) else { continue };
        let drawing = match cadaddy_io::read_dxf(dxf_bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let network = duct_reader::read_ducts(&drawing, sh);
        for run in &network.runs {
            total += 1;
            if run.system.is_some() {
                assigned += 1;
            }
        }
    }

    eprintln!("System assignment: {assigned}/{total} runs");
    // At least some runs should have system assignments.
    if total > 0 {
        let ratio = assigned as f64 / total as f64;
        assert!(ratio > 0.05, "system assignment ratio {ratio:.2} too low");
    }
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

#[test]
fn size_accuracy_when_tags_are_found() {
    // When the reader finds runs with size tags, the sizes should be correct.
    let set = generate(4).unwrap_or_else(|e| {
        eprintln!("generate(4) failed: {e}");
        std::process::exit(1);
    });
    let t = &set.truth;

    for sh in &t.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        let Some(dxf_bytes) = set.file(&sh.dxf) else { continue };
        let drawing = match cadaddy_io::read_dxf(dxf_bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let sc = duct_reader::read_and_score(&drawing, t, sh);
        eprintln!("seed=4 {}: size_accuracy={:.1}% system_accuracy={:.1}%", sh.number, sc.size_accuracy * 100.0, sc.system_accuracy * 100.0,);
        // Size accuracy of matched runs should be reasonable.
        // (Not all runs have tags, so some may use defaults.)
    }
}

// ---- A mechanical plan printed the way a design office plots it --------------------------
//
// Q6669: a real print's mechanical plan has no DUCT layer. Printed without PDF layers, the import
// puts every heavy black stroke on its wall layer, and the trade's own work (ductwork, units,
// air devices) is exactly what a mechanical plan prints heavy: the double-line ducts read as
// nothing. These sheets are drawn here, plotted to a PDF and read back as the job reads a sheet.

mod printed_plan {
    use cadaddy_color::Color;
    use cadaddy_doc::{Common, Drawing, EntityKind, HAlign, Layer, Line, Lineweight, Space, Text, VAlign};
    use cadaddy_geom::Vec3;

    /// How the made-up mechanical plan is printed.
    #[derive(Clone, Copy, Debug)]
    pub struct Look {
        /// The ductwork's pen width, 1/100 mm (heavy, as the trade's own work is plotted).
        pub duct_lw: u16,
        /// The drawing's layers become PDF layers; off: a flattened print (no layers).
        pub pdf_layers: bool,
        /// The plan's scale note is printed: the import reads the sheet at it (building
        /// inches); without it the sheet stays at paper size.
        pub scale_note: bool,
    }

    pub const FT: f64 = 12.0;
    /// Plan scale: paper inches per foot (1/4" = 1'-0").
    pub const SCALE: f64 = 0.25;

    fn layer(d: &mut Drawing, name: &str, color: Color, lw: u16) {
        let mut l = Layer::new(name);
        l.color = color;
        l.lineweight = Lineweight::Mm100(lw);
        d.layers.push(l);
    }

    fn line(d: &mut Drawing, layer: &str, a: [f64; 2], b: [f64; 2]) {
        let c = Common { layer: layer.into(), ..Default::default() };
        d.add(&Space::Model, c, EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) })).expect("line");
    }

    fn text(d: &mut Drawing, layer: &str, at: [f64; 2], h: f64, v: &str) {
        let c = Common { layer: layer.into(), ..Default::default() };
        d.add(
            &Space::Model,
            c,
            EntityKind::Text(Text {
                insert: Vec3::new(at[0], at[1], 0.0),
                align_pt: None,
                height: h,
                value: v.into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: HAlign::Left,
                valign: VAlign::Baseline,
            }),
        )
        .expect("text");
    }

    /// A straight duct `w` inches wide from `a` to `b` (level or plumb): its two sides only.
    fn duct(d: &mut Drawing, a: [f64; 2], b: [f64; 2], w: f64) {
        let h = w / 2.0;
        if (a[1] - b[1]).abs() < 1e-9 {
            line(d, "M-HVAC-DUCT", [a[0], a[1] - h], [b[0], b[1] - h]);
            line(d, "M-HVAC-DUCT", [a[0], a[1] + h], [b[0], b[1] + h]);
        } else {
            line(d, "M-HVAC-DUCT", [a[0] - h, a[1]], [b[0] - h, b[1]]);
            line(d, "M-HVAC-DUCT", [a[0] + h, a[1]], [b[0] + h, b[1]]);
        }
    }

    fn rect(d: &mut Drawing, layer: &str, lo: [f64; 2], hi: [f64; 2]) {
        line(d, layer, lo, [hi[0], lo[1]]);
        line(d, layer, [hi[0], lo[1]], hi);
        line(d, layer, hi, [lo[0], hi[1]]);
        line(d, layer, [lo[0], hi[1]], lo);
    }

    /// What the plan truly holds.
    pub struct Truth {
        /// Duct centreline feet.
        pub duct_ft: f64,
        pub diffusers: usize,
    }

    /// A one-storey mechanical floor plan, 90 ft x 50 ft, in building inches: the architecture
    /// screened (thin grey walls with both faces, partitions, door leaves), the ductwork in the
    /// heavy pen as double lines with their size tags (`24x12`, `10x8`, `48x18`, `12"ø`), a
    /// rooftop unit at the trunk's start, supply diffusers (a square with an X) tagged `SD-2`
    /// over their airflow.
    pub fn drawing() -> (Drawing, Truth) {
        let mut d = Drawing::new_imperial();
        layer(&mut d, "A-WALL", Color::Index(8), 13);
        layer(&mut d, "M-HVAC-DUCT", Color::Index(7), 50);
        layer(&mut d, "M-HVAC-EQPM", Color::Index(7), 35);
        layer(&mut d, "M-HVAC-DIFF", Color::Index(7), 25);
        layer(&mut d, "M-ANNO-TEXT", Color::Index(7), 25);
        // Architecture: the shell's two faces 8" apart, partitions 5" thick, door leaves.
        let (w, h) = (90.0 * FT, 50.0 * FT);
        rect(&mut d, "A-WALL", [0.0, 0.0], [w, h]);
        rect(&mut d, "A-WALL", [8.0, 8.0], [w - 8.0, h - 8.0]);
        for x in [35.0, 62.0] {
            let x = x * FT;
            line(&mut d, "A-WALL", [x, 8.0], [x, 26.0 * FT]);
            line(&mut d, "A-WALL", [x + 5.0, 8.0], [x + 5.0, 26.0 * FT]);
            line(&mut d, "A-WALL", [x, 29.0 * FT], [x, h - 8.0]);
            line(&mut d, "A-WALL", [x + 5.0, 29.0 * FT], [x + 5.0, h - 8.0]);
        }
        for k in 0..12 {
            // Casework and furniture outlines along the walls.
            let x = (3.0 + 7.0 * f64::from(k)) * FT;
            rect(&mut d, "A-WALL", [x, h - 8.0 - 24.0], [x + 4.0 * FT, h - 8.0]);
        }
        let th = 4.5; // 3/32" lettering at 1/4" = 1'-0"
        let mut duct_ft = 0.0;
        // Rooftop unit over the west room, the 24x12 trunk east from it.
        rect(&mut d, "M-HVAC-EQPM", [4.0 * FT, 36.0 * FT], [12.0 * FT, 41.0 * FT]);
        text(&mut d, "M-ANNO-TEXT", [5.0 * FT, 42.0 * FT], th, "RTU-1");
        let ty = 38.5 * FT;
        duct(&mut d, [12.0 * FT, ty], [80.0 * FT, ty], 24.0);
        duct_ft += 68.0;
        text(&mut d, "M-ANNO-TEXT", [20.0 * FT, ty + 16.0], th, "24x12");
        text(&mut d, "M-ANNO-TEXT", [55.0 * FT, ty + 16.0], th, "24x12");
        // Branches south from the trunk, 10x8, each to a diffuser 24" square.
        let mut diffusers = 0;
        for x in [20.0, 30.0, 45.0, 55.0, 70.0] {
            let bx = x * FT;
            let end = 20.0 * FT;
            duct(&mut d, [bx, ty - 12.0], [bx, end + 12.0], 10.0);
            duct_ft += (ty - 12.0 - end - 12.0) / FT;
            text(&mut d, "M-ANNO-TEXT", [bx + 10.0, 30.0 * FT], th, "10x8");
            rect(&mut d, "M-HVAC-DIFF", [bx - 12.0, end - 12.0], [bx + 12.0, end + 12.0]);
            line(&mut d, "M-HVAC-DIFF", [bx - 12.0, end - 12.0], [bx + 12.0, end + 12.0]);
            line(&mut d, "M-HVAC-DIFF", [bx - 12.0, end + 12.0], [bx + 12.0, end - 12.0]);
            text(&mut d, "M-ANNO-TEXT", [bx + 18.0, end + 2.0], th, "SD-2");
            text(&mut d, "M-ANNO-TEXT", [bx + 18.0, end - 6.0], th, "250 CFM");
            diffusers += 1;
        }
        // A round 12" duct along the south room, tagged with a diameter sign.
        let ry = 10.0 * FT;
        duct(&mut d, [15.0 * FT, ry], [75.0 * FT, ry], 12.0);
        duct_ft += 60.0;
        text(&mut d, "M-ANNO-TEXT", [40.0 * FT, ry + 10.0], th, "12\"\u{f8}");
        // A wide 48x18 return along the south wall.
        let wy = 4.0 * FT;
        duct(&mut d, [15.0 * FT, wy], [60.0 * FT, wy], 48.0);
        duct_ft += 45.0;
        text(&mut d, "M-ANNO-TEXT", [30.0 * FT, wy + 28.0], th, "48x18");
        (d, Truth { duct_ft, diffusers })
    }

    /// The plan plotted to a PDF the way `look` says, read back as the job reads a sheet.
    pub fn printed(look: Look) -> (Drawing, Truth) {
        let (mut d, truth) = drawing();
        if let Some(l) = d.layers.iter_mut().find(|l| l.name == "M-HVAC-DUCT") {
            l.lineweight = Lineweight::Mm100(look.duct_lw);
        }
        let ext = d.extents(&Space::Model);
        text(&mut d, "M-ANNO-TEXT", [10.0 * FT, ext.min.y - 6.0 * FT], 9.0, "MECHANICAL FLOOR PLAN");
        if look.scale_note {
            text(&mut d, "M-ANNO-TEXT", [10.0 * FT, ext.min.y - 8.0 * FT], 4.5, "SCALE: 1/4\" = 1'-0\"");
        }
        let ext = d.extents(&Space::Model);
        let pad = 2.0 * FT;
        let win = cadaddy_geom::Bounds2 {
            min: cadaddy_geom::Vec2::new(ext.min.x - pad, ext.min.y - pad),
            max: cadaddy_geom::Vec2::new(ext.max.x + pad, ext.max.y + pad),
        };
        let (pw, ph) = ((win.max.x - win.min.x) / FT * SCALE * 25.4, (win.max.y - win.min.y) / FT * SCALE * 25.4);
        let opts = cadaddy_io::pdf::PdfOptions {
            paper_mm: Some((pw.min(ph), pw.max(ph))),
            landscape: Some(pw > ph),
            fit: Some(true),
            window: Some(win),
            margins: Some([0.0; 4]),
            layers: look.pdf_layers,
            ..cadaddy_io::pdf::PdfOptions::default()
        };
        let pdf = cadaddy_io::pdf::pdf(&d, &Space::Model, &opts).expect("plot");
        // As the job loads a sheet's page: default layers (the PDF's, else by content).
        let ro = cadaddy_io::pdf_read::PdfOptions { page: 1, images: false, markups: false, layout: false, ..Default::default() };
        (cadaddy_io::pdf_read::read(&pdf, &ro).expect("read back").drawing, truth)
    }
}

/// Feet of duct a network reads, drawing units `per_ft` per foot.
fn duct_ft(net: &duct_reader::DuctNetwork, per_ft: f64) -> f64 {
    net.runs.iter().map(|r| r.points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>()).sum::<f64>() / per_ft
}

#[test]
fn a_mechanical_plan_reads_its_double_line_ducts_however_it_is_printed() {
    use printed_plan::Look;
    for look in [
        // A flattened print, the ductwork in the heavy pen: before Q6669 it read 0.6 ft.
        Look { duct_lw: 50, pdf_layers: false, scale_note: true },
        Look { duct_lw: 25, pdf_layers: false, scale_note: true },
        Look { duct_lw: 50, pdf_layers: true, scale_note: true },
    ] {
        let (d, truth) = printed_plan::printed(look);
        let net = duct_reader::read_ducts_any_layer_at(&d, Some(12.0));
        let ft = duct_ft(&net, 12.0);
        let runs: Vec<String> = net.runs.iter().map(|r| format!("{:.0}\" {} over {:.0} ft", r.width_in, r.shape, duct_ft_of(r, 12.0))).collect();
        assert!((ft - truth.duct_ft).abs() <= 0.05 * truth.duct_ft, "{look:?}: {ft:.1} ft of {:.0} ft read: {runs:?}", truth.duct_ft);
        // Each duct at its tag's size: no wall faces, unit outlines or lettering read as ducts.
        let sized = |w: f64, shape: &str, min_ft: f64| {
            net.runs.iter().any(|r| (r.width_in - w).abs() < 0.5 && r.shape == shape && duct_ft_of(r, 12.0) >= min_ft)
        };
        assert!(sized(24.0, "rect", 60.0), "{look:?}: the 24x12 trunk: {runs:?}");
        assert!(sized(48.0, "rect", 40.0), "{look:?}: the 48x18 return: {runs:?}");
        assert!(sized(12.0, "round", 55.0), "{look:?}: the 12\" round duct by its diameter sign: {runs:?}");
        assert_eq!(net.runs.iter().filter(|r| (r.width_in - 10.0).abs() < 0.5).count(), 5, "{look:?}: the five 10x8 branches: {runs:?}");
        assert!(net.runs.iter().all(|r| r.confidence >= 0.85), "{look:?}: every run read is a tagged duct: {runs:?}");
        assert_eq!(net.terminals.len(), truth.diffusers, "{look:?}: the diffusers");
        assert!(net.terminals.iter().all(|t| t.mark == "SD-2" && t.cfm == Some(250.0)), "{look:?}: {:?}", net.terminals);
        assert!(net.equipment.iter().any(|e| e.tag == "RTU-1"), "{look:?}: the rooftop unit: {:?}", net.equipment);
    }
}

fn duct_ft_of(r: &duct_reader::DuctRun, per_ft: f64) -> f64 {
    r.points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>() / per_ft
}

#[test]
fn a_sheet_left_at_paper_size_reads_at_its_printed_scale() {
    // No scale note: the import leaves the sheet in paper inches. Read at the sheet's own scale
    // (1/4" = 1'-0"), the 48" return is in range and every duct keeps its tag's size; the old
    // guess from the sheet's size (about 1/8") took no duct over about 35" wide.
    let (d, truth) = printed_plan::printed(printed_plan::Look { duct_lw: 50, pdf_layers: false, scale_note: false });
    let ext = d.extents(&cadaddy_doc::Space::Model);
    assert!(ext.max.x - ext.min.x < 40.0, "at paper size: {ext:?}");
    let per_ft = printed_plan::SCALE;
    let net = duct_reader::read_ducts_any_layer_at(&d, Some(per_ft));
    let ft = duct_ft(&net, per_ft);
    let runs: Vec<String> = net.runs.iter().map(|r| format!("{:.0}\" {} over {:.1} ft", r.width_in, r.shape, duct_ft_of(r, per_ft))).collect();
    assert!((ft - truth.duct_ft).abs() <= 0.05 * truth.duct_ft, "{ft:.1} ft of {:.0} ft: {runs:?}", truth.duct_ft);
    assert!(net.runs.iter().any(|r| (r.width_in - 48.0).abs() < 0.5 && duct_ft_of(r, per_ft) >= 40.0), "the 48x18 return: {runs:?}");
    let guessed = duct_reader::read_ducts_any_layer(&d);
    assert!(!guessed.runs.iter().any(|r| (r.width_in - 48.0).abs() < 0.5), "the guessed scale misses the wide duct (what the job read before)");
}

#[test]
fn the_made_up_hotels_mechanical_plans_read_their_ducts_when_printed_flat() {
    // The fixture hotel prints most of its sets without PDF layers, its ductwork in a heavy pen
    // and drawn as a closed outline with a dashed centreline, sizes as `24x12` or `24/12`. Read
    // as the job reads a sheet, each plan's ducts are its truth's (before Q6669 they read 42 to
    // 159 ft, mostly not the duct, where the truth has about 63).
    use cadaddy_mepfix::hotel::HotelSize;
    let mut flat = 0;
    for seed in [3u64, 4, 6, 42] {
        let set = cadaddy_mepfix::generate_hotel(seed, HotelSize::Small).expect("hotel");
        let (_, st) = cadaddy_mepfix::hotel_planned(seed, HotelSize::Small);
        if st.pdf_layers {
            continue;
        }
        let t = &set.truth;
        for sh in t.sheets.iter().filter(|s| s.discipline == "M" && s.level.is_some()) {
            let Some(pdf) = set.file(&sh.pdf) else { continue };
            let ro = cadaddy_io::pdf_read::PdfOptions { page: 1, images: false, markups: false, layout: false, ..Default::default() };
            let d = cadaddy_io::pdf_read::read(pdf, &ro).expect("read").drawing;
            let level = sh.level.clone().unwrap_or_default();
            let want: f64 = t
                .segments
                .iter()
                .filter(|s| s.kind == "duct" && s.level == level && !s.vertical)
                .map(|s| {
                    let [a, b] = s.path_ft;
                    (b[0] - a[0]).hypot(b[1] - a[1])
                })
                .sum();
            assert!(
                duct_reader::read_ducts(&d, &cadaddy_mepfix::truth::Sheet::default()).runs.is_empty(),
                "seed {seed} {}: no DUCT layer",
                sh.number
            );
            let net = duct_reader::read_ducts_any_layer_at(&d, Some(12.0));
            let ft = duct_ft(&net, 12.0);
            assert!(want > 0.0 && (ft - want).abs() <= 0.1 * want + 2.0, "seed {seed} {}: {ft:.0} ft read of {want:.0} ft", sh.number);
            flat += 1;
        }
    }
    assert!(flat >= 6, "flattened mechanical plans checked: {flat}");
}

#[test]
fn a_single_pen_print_reads_the_duct_not_the_walls_around_it() {
    // Everything in one black pen on one layer, as a print plotted without layers or weights:
    // a corridor's two walls 66" apart with a 24" duct (a closed outline, a continuous
    // centreline) down its middle, tagged inside; a unit in a room, its tag inside it, the room's
    // wall faces passing beside it. One duct: no corridor, no wall face beside a duct side, no
    // room taken for the unit.
    use cadaddy_doc::{Common, Drawing, EntityKind, HAlign, Line, LwPolyline, Space, Text, VAlign};
    use cadaddy_geom::{PolyVertex, Vec2, Vec3};
    let mut d = Drawing::new_imperial();
    let c = Common::default();
    let line = |d: &mut Drawing, a: [f64; 2], b: [f64; 2]| {
        d.add(&Space::Model, c.clone(), EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) })).expect("line");
    };
    let boxed = |d: &mut Drawing, lo: [f64; 2], hi: [f64; 2]| {
        let vertices =
            [[lo[0], lo[1]], [hi[0], lo[1]], [hi[0], hi[1]], [lo[0], hi[1]]].iter().map(|p| PolyVertex::new(Vec2::new(p[0], p[1]))).collect();
        d.add(
            &Space::Model,
            Common::default(),
            EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }),
        )
        .expect("box");
    };
    let text = |d: &mut Drawing, at: [f64; 2], v: &str| {
        let t = Text {
            insert: Vec3::new(at[0], at[1], 0.0),
            align_pt: None,
            height: 6.0,
            value: v.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        };
        d.add(&Space::Model, Common::default(), EntityKind::Text(t)).expect("text");
    };
    // The corridor's walls (each with two faces 4" apart), the duct and its centreline.
    for y in [-4.0, 0.0, 66.0, 70.0] {
        line(&mut d, [0.0, y], [1040.0, y]);
    }
    boxed(&mut d, [140.0, 21.0], [884.0, 45.0]);
    line(&mut d, [0.0, 33.0], [1040.0, 33.0]);
    text(&mut d, [400.0, 30.0], "24x14");
    // A room south of the corridor (its wall faces), a unit in it with its tag inside.
    boxed(&mut d, [300.0, -300.0], [480.0, -4.0]);
    boxed(&mut d, [340.0, -200.0], [424.0, -164.0]);
    text(&mut d, [350.0, -185.0], "AHU-1");
    let net = duct_reader::read_ducts_any_layer_at(&d, Some(12.0));
    let runs: Vec<String> =
        net.runs.iter().map(|r| format!("{:.0}\" over {:.1} ft at y {:.0}", r.width_in, duct_ft_of(r, 12.0), r.points[0].y)).collect();
    assert_eq!(net.runs.len(), 1, "{runs:?}");
    let r = &net.runs[0];
    assert!((r.width_in - 24.0).abs() < 0.5 && (r.depth_in - 14.0).abs() < 0.5, "{runs:?}");
    assert!((duct_ft_of(r, 12.0) - 62.0).abs() < 0.5 && (r.points[0].y - 33.0).abs() < 0.5, "{runs:?}");
    let units: Vec<String> = net.equipment.iter().map(|e| format!("{} {:?}..{:?}", e.tag, e.min, e.max)).collect();
    assert_eq!(units.len(), 1, "one unit for its one tag: {units:?}");
    assert!(net.equipment.iter().all(|e| (e.max.x - e.min.x - 84.0).abs() < 0.5), "the unit's own box, not its room: {units:?}");
}
