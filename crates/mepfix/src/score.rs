//! Scoring a print reader against the truth.
//!
//! A reader reports the runs it found in building feet (after aligning the sheet; see
//! [`building_from_model`] for the truth's own mapping). Matching is by plan geometry: a found run
//! matches a truth segment of the same level and kind when they are parallel, their centrelines
//! are within a tolerance and they overlap along their length. Precision and recall are by
//! length, so splitting or joining runs differently from the truth costs nothing.

use crate::truth::{Segment, Sheet, Truth};

/// A run a reader found: plan centreline (building feet), what it is, how high.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundRun {
    pub level: String,
    /// `duct` or `pipe`.
    pub kind: String,
    pub system: String,
    pub shape: String,
    pub size_in: [f64; 2],
    pub from_ft: [f64; 2],
    pub to_ft: [f64; 2],
    /// Bottom above the floor, feet, if the reader worked it out.
    pub bottom_ft: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunScore {
    /// Share of the found length that lies on a truth run.
    pub precision: f64,
    /// Share of the truth length found.
    pub recall: f64,
    /// Of the matched length: share with the right size, the right system.
    pub size_accuracy: f64,
    pub system_accuracy: f64,
    /// Mean absolute error of bottom heights over matched length that has one, feet.
    pub height_mae_ft: Option<f64>,
    pub truth_length_ft: f64,
    pub found_length_ft: f64,
}

/// Drawing (model inches) → building feet on a sheet: the inverse of the sheet's placement.
pub fn building_from_model(sh: &Sheet, m: [f64; 2]) -> [f64; 2] {
    let p = [m[0] - sh.model_from_building.offset_in[0], m[1] - sh.model_from_building.offset_in[1]];
    let r = crate::layout::rotate(p, -sh.model_from_building.rotation_deg);
    [r[0] / 12.0, r[1] / 12.0]
}

/// Paper inches → building feet on a sheet.
pub fn building_from_paper(sh: &Sheet, p: [f64; 2]) -> [f64; 2] {
    building_from_model(sh, crate::layout::paper_to_model(sh, p))
}

/// The truth's horizontal runs a reader of `level`'s `kind` sheet should find (not those below
/// the floor, which plans show hidden).
pub fn expected<'a>(t: &'a Truth, level: &'a str, kind: &'a str) -> impl Iterator<Item = &'a Segment> + 'a {
    t.segments.iter().filter(move |s| s.level == level && s.kind == kind && !s.vertical && !s.below_floor && s.length_ft() > 1e-6)
}

/// Overlap length of two parallel plan segments within `tol` of each other, feet.
fn overlap(a0: [f64; 2], a1: [f64; 2], b0: [f64; 2], b1: [f64; 2], tol: f64) -> f64 {
    let d = [a1[0] - a0[0], a1[1] - a0[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 1e-9 {
        return 0.0;
    }
    let u = [d[0] / len, d[1] / len];
    let e = [b1[0] - b0[0], b1[1] - b0[1]];
    let el = (e[0] * e[0] + e[1] * e[1]).sqrt();
    if el < 1e-9 || ((u[0] * e[1] - u[1] * e[0]) / el).abs() > 0.02 {
        return 0.0;
    }
    // Perpendicular offset of b from a's line.
    let off = |p: [f64; 2]| ((p[0] - a0[0]) * -u[1] + (p[1] - a0[1]) * u[0]).abs();
    if off(b0).max(off(b1)) > tol {
        return 0.0;
    }
    let along = |p: [f64; 2]| (p[0] - a0[0]) * u[0] + (p[1] - a0[1]) * u[1];
    let (s0, s1) = (along(b0).min(along(b1)), along(b0).max(along(b1)));
    (s1.min(len) - s0.max(0.0)).max(0.0)
}

/// Score found runs against the truth's runs of one level and kind; `tol_ft` is how far a found
/// centreline may sit from the true one.
pub fn score_runs(t: &Truth, level: &str, kind: &str, found: &[FoundRun], tol_ft: f64) -> RunScore {
    let truth: Vec<&Segment> = expected(t, level, kind).collect();
    let found: Vec<&FoundRun> = found.iter().filter(|f| f.level == level && f.kind == kind).collect();
    let truth_len: f64 = truth.iter().map(|s| s.length_ft()).sum();
    let found_len: f64 = found.iter().map(|f| ((f.to_ft[0] - f.from_ft[0]).powi(2) + (f.to_ft[1] - f.from_ft[1]).powi(2)).sqrt()).sum();
    let (mut recall_len, mut size_ok, mut sys_ok, mut h_err, mut h_len) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for s in &truth {
        let [a, b] = s.path_ft;
        let (a, b) = ([a[0], a[1]], [b[0], b[1]]);
        let mut best = 0.0;
        let mut best_f: Option<&FoundRun> = None;
        for f in &found {
            let o = overlap(a, b, f.from_ft, f.to_ft, tol_ft);
            if o > best {
                best = o;
                best_f = Some(f);
            }
        }
        let covered: f64 = found.iter().map(|f| overlap(a, b, f.from_ft, f.to_ft, tol_ft)).sum::<f64>().min(s.length_ft());
        recall_len += covered;
        if let Some(f) = best_f {
            if (f.size_in[0] - s.size_in[0]).abs() < 0.01 && (f.size_in[1] - s.size_in[1]).abs() < 0.01 && f.shape == s.shape {
                size_ok += covered;
            }
            if f.system == s.system {
                sys_ok += covered;
            }
            if let (Some(fb), Some(h)) = (f.bottom_ft, &s.height) {
                h_err += (fb - h.bottom_ft).abs() * covered;
                h_len += covered;
            }
        }
    }
    let mut precision_len = 0.0;
    for f in &found {
        let fl = ((f.to_ft[0] - f.from_ft[0]).powi(2) + (f.to_ft[1] - f.from_ft[1]).powi(2)).sqrt();
        let on: f64 =
            truth.iter().map(|s| overlap(f.from_ft, f.to_ft, [s.path_ft[0][0], s.path_ft[0][1]], [s.path_ft[1][0], s.path_ft[1][1]], tol_ft)).sum();
        precision_len += on.min(fl);
    }
    let ratio = |a: f64, b: f64| if b > 1e-9 { a / b } else { 0.0 };
    RunScore {
        precision: ratio(precision_len, found_len),
        recall: ratio(recall_len, truth_len),
        size_accuracy: ratio(size_ok, recall_len),
        system_accuracy: ratio(sys_ok, recall_len),
        height_mae_ft: (h_len > 1e-9).then(|| h_err / h_len),
        truth_length_ft: truth_len,
        found_length_ft: found_len,
    }
}

/// The truth's own runs as found runs: a perfect reader (for testing a scorer, or as a baseline).
pub fn perfect(t: &Truth, level: &str, kind: &str) -> Vec<FoundRun> {
    expected(t, level, kind)
        .map(|s| FoundRun {
            level: s.level.clone(),
            kind: s.kind.clone(),
            system: s.system.clone(),
            shape: s.shape.clone(),
            size_in: s.size_in,
            from_ft: [s.path_ft[0][0], s.path_ft[0][1]],
            to_ft: [s.path_ft[1][0], s.path_ft[1][1]],
            bottom_ft: s.height.as_ref().map(|h| h.bottom_ft),
        })
        .collect()
}
