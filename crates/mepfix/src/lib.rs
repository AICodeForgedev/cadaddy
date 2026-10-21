//! MEP print fixtures: a made-up two-storey building's mechanical, plumbing, fire protection,
//! reflected ceiling and structural sheets, drawn as vector PDF and DXF the way consulting
//! engineers draw them, with the ground truth (every run, size, height, fitting, terminal and
//! piece of equipment) as JSON. Deterministic by seed; drafting style varies by seed.
//!
//! Print readers (sheet recognition, ductwork, piping, heights, clashes…) are scored against the
//! truth: see [`score`].
//!
//! Extending: a new kind of element goes into [`truth`] (its numbers), [`design`] (where the
//! building gets it), [`draw`] (how a sheet shows it) and, if readers will find it, [`score`].

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod clash;
pub mod design;
pub mod draw;
pub mod layout;
pub mod rng;
pub mod score;
pub mod style;
pub mod truth;

use cadaddy_doc::Space;
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_io::pdf::PdfOptions;
use rng::Rng;
use std::path::Path;
use truth::Truth;

/// A generated set: the truth and the files (`<sheet>.pdf`, `<sheet>.dxf`, `set.pdf` with every
/// sheet, `truth.json`).
pub struct FixtureSet {
    pub truth: Truth,
    pub files: Vec<(String, Vec<u8>)>,
}

impl FixtureSet {
    pub fn file(&self, name: &str) -> Option<&[u8]> {
        self.files.iter().find(|(n, _)| n == name).map(|(_, b)| b.as_slice())
    }

    /// Write the files into `dir` (created if missing).
    pub fn write_to(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for (name, bytes) in &self.files {
            std::fs::write(dir.join(name), bytes)?;
        }
        Ok(())
    }
}

/// The truth for a seed without drawing anything (no sheets, no tags): for tests of the
/// building and systems themselves.
pub fn truth_for(seed: u64) -> Truth {
    let mut r = Rng::new(seed);
    let st = style::Style::new(&mut r.fork(1));
    let d = design::design(seed, &st, &mut r.fork(2));
    let mut t = d.truth;
    t.clashes = clash::clashes(&t, &d.planted);
    t
}

/// The fixture set for a seed.
pub fn generate(seed: u64) -> Result<FixtureSet, String> {
    let mut r = Rng::new(seed);
    let st = style::Style::new(&mut r.fork(1));
    let d = design::design(seed, &st, &mut r.fork(2));
    let mut t = d.truth;
    layout::plan_sheets(&mut t, &st, &mut r.fork(3));
    layout::tag_runs(&mut t, &st, &d.planted, &mut r.fork(4));
    t.clashes = clash::clashes(&t, &d.planted);
    let kinds: Vec<(String, layout::Kind)> = layout::sheet_list().into_iter().map(|(n, _, _, k)| (n, k)).collect();
    let mut files = Vec::new();
    let mut set = Vec::new();
    let sheets = t.sheets.clone();
    for (i, sh) in sheets.iter().enumerate() {
        let Some((_, kind)) = kinds.iter().find(|(n, _)| *n == sh.number) else { continue };
        let (drawing, legend, abbr) = draw::draw(&t, &st, sh, kind);
        if let Some(s) = t.sheets.get_mut(i) {
            s.legend = legend;
            s.abbreviations = abbr;
        }
        let pdf = cadaddy_io::pdf::pdf(&drawing, &Space::Model, &pdf_options(sh, &st)).map_err(|e| format!("{}: {e}", sh.number))?;
        let dxf = cadaddy_io::write_dxf(&drawing);
        set.push((pdf.clone(), format!("{} {}", sh.number, sh.title), String::new()));
        files.push((sh.pdf.clone(), pdf));
        files.push((sh.dxf.clone(), dxf.into_bytes()));
    }
    files.push(("set.pdf".into(), cadaddy_io::pdf_merge::merge(&set).map_err(|e| format!("set.pdf: {e}"))?));
    let json = serde_json::to_vec_pretty(&t).map_err(|e| e.to_string())?;
    files.push(("truth.json".into(), json));
    Ok(FixtureSet { truth: t, files })
}

/// How a sheet is plotted: its paper, exactly its window, no margins.
pub fn pdf_options(sh: &truth::Sheet, st: &style::Style) -> PdfOptions {
    let [w, h] = sh.paper_in;
    let o = layout::paper_to_model(sh, [0.0, 0.0]);
    let e = layout::paper_to_model(sh, [w, h]);
    PdfOptions {
        paper_mm: Some((h * 25.4, w * 25.4)),
        landscape: Some(true),
        fit: Some(true),
        window: Some(Bounds2::new(Vec2::new(o[0], o[1]), Vec2::new(e[0], e[1]))),
        margins: Some([0.0; 4]),
        lineweights: Some(true),
        layers: st.pdf_layers,
        compress: true,
        title: format!("{} {}", sh.number, sh.title),
        stamp: Some(false),
        hyperlinks: false,
        ..PdfOptions::default()
    }
}
