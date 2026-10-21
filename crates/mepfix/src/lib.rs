//! MEP print fixtures: made-up buildings — a two-storey office and a four-storey hotel — with
//! mechanical, plumbing, fire protection, electrical, fire alarm, telecom, structural and
//! architectural sheets, drawn as vector PDF and DXF the way consulting engineers draw them,
//! with the ground truth (every run, size, height, fitting, terminal, device, wall assembly,
//! ceiling assembly, casework, spec item, site element) as JSON. Deterministic by seed; drafting
//! style varies by seed.
//!
//! The architectural sheets add wall types and floor plans ([`walls`]), and ceiling types, soffit
//! and head of wall details and a room finish schedule ([`ceilings`]).
//!
//! Print readers (sheet recognition, ductwork, piping, heights, clashes…) are scored against the
//! truth: see [`score`].
//!
//! Extending: a new kind of element goes into [`truth`] (its numbers), [`design`] (where the
//! building gets it), [`draw`] (how a sheet shows it) and, if readers will find it, [`score`].

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod bore;
pub mod callout_reader;
pub mod ceilasm_reader;
pub mod arch;
pub mod arch_reader;
pub mod ceiling_reader;
pub mod ceilings;
pub mod clash;
pub mod design;
pub mod draw;
pub mod duct_reader;
pub mod elec_reader;
pub mod equip_reader;
pub mod framing;
pub mod gaps;
pub mod heights;
pub mod hotel;
pub mod layout;
pub mod pipe_reader;
pub mod printset;
pub mod rfi;
pub mod rng;
pub mod score;
pub mod sheet_reader;
pub mod struct_reader;
pub mod style;
pub mod truth;
pub mod wall_reader;
pub mod walls;

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
    let mut st = style::Style::new(&mut r.fork(1));
    let d = design::design(seed, &st, &mut r.fork(2));
    let mut t = d.truth;
    t.clashes = clash::clashes(&t, &d.planted);
    t.placement_zones = clash::placement_zones(&t);
    add_walls(&mut t, &mut st);
    add_ceilings(&mut t, &mut st);
    add_arch(&mut t, &mut st);
    t
}

/// The wall types and walls, from a random stream of their own (so the rest of a set stays as
/// it was before walls were added, and [`truth_for`] gives the same walls as [`generate`]).
fn add_walls(t: &mut Truth, st: &mut style::Style) {
    let mut r = Rng::new(t.seed ^ 0x5741_4C4C_5459_5045).fork(5);
    st.walls = walls::WallStyle::new(&mut r);
    walls::design_walls(t, &st.walls, &mut r);
    framing::choose_layouts(t);
}

/// The ceiling types, soffit and head of wall details and the soffits they detail, from a random
/// stream of their own (after the walls: the head details are per wall type).
fn add_ceilings(t: &mut Truth, st: &mut style::Style) {
    let mut r = Rng::new(t.seed ^ 0x4345_494C_4153_4D42).fork(6);
    st.ceilings = ceilings::CeilStyle::new(&mut r);
    ceilings::design_ceilings(t, &st.ceilings, &mut r);
}

/// Doors, windows, the stair and elevator, slab edges, the roof and room finishes, from a random
/// stream of their own (after the walls: the doors are the walls' openings).
fn add_arch(t: &mut Truth, st: &mut style::Style) {
    let mut r = Rng::new(t.seed ^ 0x4152_4348_5245_4144).fork(6);
    let nb = t.building.grid.x.len().saturating_sub(1);
    st.arch = arch::ArchStyle::new(&mut r, st.walls.symbol, nb);
    arch::design_arch(t, &st.arch, &mut r);
}

/// The fixture set for a seed.
pub fn generate(seed: u64) -> Result<FixtureSet, String> {
    generate_set(seed, false)
}

/// The fixture set for a seed with gaps and conflicts planted in its prints ([`gaps`]): the
/// same set plus the planted gaps, sleeve notes and an enlarged architectural plan (`A-401`).
pub fn generate_with_gaps(seed: u64) -> Result<FixtureSet, String> {
    generate_set(seed, true)
}

/// The truth of [`generate_with_gaps`] without drawing anything.
pub fn truth_with_gaps(seed: u64) -> Truth {
    let mut t = truth_for(seed);
    gaps::plant(&mut t);
    t
}

fn generate_set(seed: u64, with_gaps: bool) -> Result<FixtureSet, String> {
    let mut r = Rng::new(seed);
    let mut st = style::Style::new(&mut r.fork(1));
    let d = design::design(seed, &st, &mut r.fork(2));
    let mut t = d.truth;
    layout::plan_sheets(&mut t, &st, &mut r.fork(3));
    layout::tag_runs(&mut t, &st, &d.planted, &mut r.fork(4));
    layout::plan_callouts(&mut t, &mut r.fork(5));
    t.clashes = clash::clashes(&t, &d.planted);
    t.placement_zones = clash::placement_zones(&t);
    add_walls(&mut t, &mut st);
    add_ceilings(&mut t, &mut st);
    add_arch(&mut t, &mut st);
    let mut kinds: Vec<(String, layout::Kind)> = layout::sheet_list().into_iter().map(|(n, _, _, k)| (n, k)).collect();
    if with_gaps {
        gaps::plant(&mut t);
        if let Some(sh) = gaps::enlarged_sheet(&t) {
            t.sheets.push(sh);
            kinds.push((gaps::ENLARGED_SHEET.into(), layout::Kind::EnlargedFloor("1".into(), "TOILET".into())));
        }
    }
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

/// The truth for a hotel seed without drawing anything.
pub fn hotel_truth_for(seed: u64, size: hotel::HotelSize) -> Truth {
    let mut r = Rng::new(seed);
    let st = style::Style::new(&mut r.fork(1));
    let d = hotel::hotel_design(seed, &st, size, &mut r.fork(10));
    let mut t = d.truth;
    t.clashes = clash::clashes(&t, &d.planted);
    t
}

/// The hotel fixture set for a seed.
pub fn generate_hotel(seed: u64, size: hotel::HotelSize) -> Result<FixtureSet, String> {
    let mut r = Rng::new(seed);
    let st = style::Style::new(&mut r.fork(1));
    let d = hotel::hotel_design(seed, &st, size, &mut r.fork(10));
    let mut t = d.truth;
    layout::plan_hotel_sheets(&mut t, &st, &mut r.fork(11));
    layout::tag_runs(&mut t, &st, &d.planted, &mut r.fork(12));
    layout::plan_hotel_callouts(&mut t, &mut r.fork(13));
    t.clashes = clash::clashes(&t, &d.planted);
    let kinds: Vec<(String, layout::Kind)> = layout::hotel_sheet_list().into_iter().map(|(n, _, _, k)| (n, k)).collect();
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
    // Spec book as a separate JSON
    let spec_json = serde_json::to_vec_pretty(&t.spec_items).map_err(|e| e.to_string())?;
    files.push(("spec-book.json".into(), spec_json));
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
