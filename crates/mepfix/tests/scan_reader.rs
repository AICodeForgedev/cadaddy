//! Scanned prints (Q272): fixture sheets printed to grey images, dirtied as scans are (noise and
//! specks, a 1.5° turn with a shift, JPEG artefacts at quality 40, and all at once), read back by
//! the scan reader and scored against the truth with the same scorers as the vector readers.
//!
//! The table (`cargo test -p cadaddy-mepfix --test scan_reader -- --nocapture`) shows, per sheet
//! and dirt, the alignment misfit, the text read exactly, and duct / pipe precision and recall
//! next to what the vector reader gets from the sheet's own DXF. The thresholds are the tests.

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_mepfix::scan::{self, Dirt};
use cadaddy_mepfix::truth::{Sheet, Truth};
use cadaddy_mepfix::{duct_reader, pipe_reader, scan_reader, score};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Clean,
    Noise,
    Turned,
    Jpeg,
    All,
}

fn dirt(k: Kind) -> Dirt {
    match k {
        Kind::Clean => Dirt::clean(),
        Kind::Noise => Dirt { noise: true, ..Dirt::default() },
        Kind::Turned => Dirt { turn_deg: 1.5, shift_px: [23.0, -17.0], ..Dirt::default() },
        Kind::Jpeg => Dirt { jpeg_quality: Some(40), ..Dirt::default() },
        Kind::All => Dirt::all(),
    }
}

#[derive(Debug)]
struct Row {
    sheet: String,
    dpi: f64,
    kind: Kind,
    rms_ft: f64,
    text_read: f64,
    vector: score::RunScore,
    scanned: score::RunScore,
}

/// The run score of a sheet's drawing (ducts on mechanical sheets, pipes otherwise); `scanned`:
/// with the duct reader's options for scans.
fn runs(d: &Drawing, t: &Truth, sh: &Sheet, scanned: bool) -> score::RunScore {
    if sh.discipline == "M" {
        let o = if scanned { scan_reader::DUCT_OPTIONS } else { duct_reader::DuctOptions::default() };
        duct_reader::read_and_score_with(d, t, sh, &o)
    } else {
        let (found, _) = pipe_reader::read_pipes(d, sh);
        score::score_runs(t, sh.level.as_deref().unwrap_or_default(), "pipe", &found, 0.5)
    }
}

/// The share of the sheet's black text (not the screened background's room names) read exactly.
fn text_read(d: &Drawing, read: &scan_reader::ScanRead) -> f64 {
    let mut pool: Vec<String> = read.text.iter().map(|t| t.text.trim().to_string()).collect();
    let (mut n, mut hit) = (0, 0);
    for e in d.model.iter() {
        let EntityKind::Text(t) = &e.kind else { continue };
        if ["ARCH", "ARCH-TEXT", "GRID", "CENTRE"].contains(&e.common.layer.as_str()) {
            continue;
        }
        n += 1;
        if let Some(i) = pool.iter().position(|p| p == t.value.trim()) {
            pool.swap_remove(i);
            hit += 1;
        }
    }
    if n == 0 { 0.0 } else { f64::from(hit) / f64::from(n) }
}

fn run(seed: u64, sheets: &[&str], dpi: f64, kinds: &[Kind]) -> Vec<Row> {
    let set = cadaddy_mepfix::generate(seed).unwrap();
    let t = &set.truth;
    let mut jobs = Vec::new();
    for sh in t.sheets.iter().filter(|s| sheets.contains(&s.number.as_str())) {
        let d = cadaddy_io::read_dxf(set.file(&sh.dxf).unwrap()).unwrap();
        let printed = scan::print(&d, sh, dpi).unwrap();
        for &k in kinds {
            jobs.push((sh, d.clone(), printed.clone(), k));
        }
    }
    // A few at a time: each scan is tens of megapixels.
    let mut rows = Vec::new();
    for chunk in jobs.chunks(4) {
        let got: Vec<Row> = std::thread::scope(|s| {
            let hs: Vec<_> = chunk
                .iter()
                .map(|(sh, d, printed, k)| {
                    s.spawn(move || {
                        let sc = scan::dirty(printed, &dirt(*k), seed).unwrap();
                        let read = scan_reader::read_scan(&sc, sh, &t.building.grid).unwrap();
                        Row {
                            sheet: format!("{seed}/{}", sh.number),
                            dpi,
                            kind: *k,
                            rms_ft: read.map.rms_ft,
                            text_read: text_read(d, &read),
                            vector: runs(d, t, sh, false),
                            scanned: runs(&read.drawing, t, sh, true),
                        }
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        rows.extend(got);
    }
    rows
}

fn print(rows: &[Row]) {
    eprintln!("| sheet | dpi | dirt | grid misfit ft | text read | vector P | vector R | scan P | scan R | scan size | scan system |");
    eprintln!("|---|---|---|---|---|---|---|---|---|---|---|");
    for r in rows {
        eprintln!(
            "| {} | {} | {:?} | {:.3} | {:.0}% | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |",
            r.sheet,
            r.dpi,
            r.kind,
            r.rms_ft,
            r.text_read * 100.0,
            r.vector.precision,
            r.vector.recall,
            r.scanned.precision,
            r.scanned.recall,
            r.scanned.size_accuracy,
            r.scanned.system_accuracy
        );
    }
}

/// Every row: aligned to the grid within a tenth of a foot, enough of its text read, and runs
/// found close to the vector reader's own precision and recall.
fn check(rows: &[Row], text_min: f64, precision_share: f64, recall_share: f64) {
    for r in rows {
        let name = format!("{} {} dpi {:?}", r.sheet, r.dpi, r.kind);
        assert!(r.rms_ft < 0.1, "{name}: grid misfit {:.3} ft", r.rms_ft);
        assert!(r.text_read >= text_min, "{name}: text read {:.2} < {text_min}", r.text_read);
        assert!(
            r.scanned.precision >= precision_share * r.vector.precision,
            "{name}: precision {:.2} < {precision_share} × the vector reader's {:.2}",
            r.scanned.precision,
            r.vector.precision
        );
        assert!(
            r.scanned.recall >= recall_share * r.vector.recall,
            "{name}: recall {:.2} < {recall_share} × the vector reader's {:.2}",
            r.scanned.recall,
            r.vector.recall
        );
    }
}

const ALL_KINDS: [Kind; 5] = [Kind::Clean, Kind::Noise, Kind::Turned, Kind::Jpeg, Kind::All];

#[test]
fn scans_at_150_dpi_read_like_the_vector_sheets() {
    let rows = run(1, &["M-101", "P-101"], 150.0, &ALL_KINDS);
    print(&rows);
    check(&rows, 0.3, 0.5, 0.6);
}

#[test]
fn scans_at_200_dpi_read_like_the_vector_sheets() {
    let rows = run(1, &["M-101", "P-101", "FP-101"], 200.0, &[Kind::Clean, Kind::All]);
    print(&rows);
    check(&rows, 0.45, 0.5, 0.75);
}

#[test]
fn a_second_building_and_its_larger_sheets() {
    // Seed 3's sheets are 42" × 30": 150 dpi keeps them under the tracer's pixel cap.
    let rows = run(3, &["M-102", "P-102"], 150.0, &[Kind::Clean, Kind::All]);
    print(&rows);
    check(&rows, 0.45, 0.5, 0.55);
}
