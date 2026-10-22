//! A print set read the way a contractor gets one: a merged PDF (or a PDF per sheet), every
//! page imported and recognised as a sheet without the fixture's layer names, the sheet map
//! assembled, and each sheet kept as DXF for the readers (keyed by its number, as they expect).
//! Then what the readers found, put together for coordination: the conduit runs (branch runs
//! and feeder banks) with how each one's height is known, and the clashes among them.

use crate::clash;
use crate::conduit_reader::ConduitResult;
use crate::sheet_reader::{self, RecognisedSheet, SheetMap};
use crate::tray_reader::TrayResult;
use crate::truth::{HeightSource, Segment, Truth};

/// Most pages read from one PDF (hostile files).
pub const MAX_PAGES: u32 = 2_000;

/// A print set read page by page.
#[derive(Clone, Debug, Default)]
pub struct PrintedSet {
    /// Every page recognised, in page order.
    pub pages: Vec<RecognisedSheet>,
    pub map: SheetMap,
    /// Each numbered page as DXF, named `<number>.dxf` (the first page of a number wins).
    pub dxf: Vec<(String, Vec<u8>)>,
    /// Pages that didn't import or write, as error messages.
    pub errors: Vec<String>,
}

/// One page of a PDF as a drawing: its linework, and its lettering as text.
pub fn import_page(pdf: &[u8], page: u32) -> Result<cadaddy_doc::Drawing, String> {
    let opts = cadaddy_io::pdf_read::PdfOptions { page, images: false, markups: false, layout: false, ..Default::default() };
    cadaddy_io::pdf_read::read(pdf, &opts).map(|i| i.drawing).map_err(|e| e.to_string())
}

/// Read a merged print set: each page recognised, the sheet map, each sheet as DXF.
pub fn read_merged(pdf: &[u8]) -> Result<PrintedSet, String> {
    let n = cadaddy_io::pdf_read::page_count(pdf).map_err(|e| e.to_string())?.min(MAX_PAGES);
    let mut set = PrintedSet::default();
    for page in 1..=n {
        let d = match import_page(pdf, page) {
            Ok(d) => d,
            Err(e) => {
                set.errors.push(format!("page {page}: {e}"));
                continue;
            }
        };
        let rec = sheet_reader::recognise_dxf(&d);
        if !rec.number.is_empty() && !set.dxf.iter().any(|(n, _)| n.trim_end_matches(".dxf") == rec.number) {
            match cadaddy_io::write(&d, "sheet.dxf") {
                Ok(b) => set.dxf.push((format!("{}.dxf", rec.number), b)),
                Err(e) => set.errors.push(format!("page {page}: {e}")),
            }
        }
        set.pages.push(rec);
    }
    set.map = sheet_reader::sheet_map_from(set.pages.clone());
    Ok(set)
}

/// A conduit run read from the electrical plans: a branch run or one conduit of a feeder bank.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrintRun {
    pub level: String,
    pub from_ft: [f64; 2],
    pub to_ft: [f64; 2],
    pub size_in: f64,
    /// Bottom above the floor, feet, when known.
    pub bottom_ft: Option<f64>,
    /// The height was printed on the plan (a `BOC @` note), not set by a rule.
    pub from_note: bool,
    /// `branch` or `feeder`.
    pub kind: String,
}

/// The conduit runs read: the branch runs (heights from [`crate::conduit_reader::assign_heights`])
/// and the feeder banks' conduits (their bank tag's `BOC @` the height of each).
pub fn conduit_runs(branch: &ConduitResult, trays: &TrayResult) -> Vec<PrintRun> {
    let mut out: Vec<PrintRun> = branch
        .conduits
        .iter()
        .map(|c| PrintRun {
            level: c.level.clone(),
            from_ft: c.from_ft,
            to_ft: c.to_ft,
            size_in: c.size_in,
            bottom_ft: c.bottom_ft,
            from_note: c.height_source == Some(HeightSource::Note),
            kind: "branch".into(),
        })
        .collect();
    out.extend(trays.runs.iter().filter(|r| r.kind == "conduit").map(|r| PrintRun {
        level: r.level.clone(),
        from_ft: r.from_ft,
        to_ft: r.to_ft,
        size_in: r.size_in[0],
        bottom_ft: r.bottom_ft,
        from_note: r.bottom_ft.is_some(),
        kind: "feeder".into(),
    }));
    out
}

/// Ends within this (feet) are one node: runs meeting there are connected, not clashing.
const NODE_FT: f64 = 0.25;

/// Whether a run's end lies on another run's centreline (a tee: a junction, not a clash).
fn tee(a: &PrintRun, b: &PrintRun) -> bool {
    let on = |p: [f64; 2], r: &PrintRun| {
        let d = [r.to_ft[0] - r.from_ft[0], r.to_ft[1] - r.from_ft[1]];
        let l2 = d[0] * d[0] + d[1] * d[1];
        if l2 <= 1e-12 {
            return false;
        }
        let t = (((p[0] - r.from_ft[0]) * d[0] + (p[1] - r.from_ft[1]) * d[1]) / l2).clamp(0.0, 1.0);
        (p[0] - r.from_ft[0] - t * d[0]).hypot(p[1] - r.from_ft[1] - t * d[1]) <= NODE_FT
    };
    [a.from_ft, a.to_ft].iter().any(|p| on(*p, b)) || [b.from_ft, b.to_ft].iter().any(|p| on(*p, a))
}

/// The clashes among the conduit runs read from the prints, level by level, each at its height
/// above its floor (a run without one is left out; level elevations aren't read from prints, so
/// levels are never compared). Runs that meet (an end on the other) are connected, not
/// clashing. Overlaps by [`clash::clashes`], as for a model.
pub fn clashes(runs: &[PrintRun]) -> Vec<crate::truth::Clash> {
    let mut levels: Vec<&str> = runs.iter().map(|r| r.level.as_str()).collect();
    levels.sort_unstable();
    levels.dedup();
    let mut out = Vec::new();
    for level in levels {
        let here: Vec<(usize, &PrintRun)> = runs.iter().enumerate().filter(|(_, r)| r.level == level && r.bottom_ft.is_some()).collect();
        let mut t = Truth::default();
        for (i, r) in &here {
            let z = r.bottom_ft.unwrap_or_default() + r.size_in / 24.0;
            t.segments.push(Segment {
                id: format!("{}-{i}", r.kind),
                system: if r.kind == "feeder" { "E-FDR-COND".into() } else { crate::conduit_reader::CONDUIT_SYSTEM.into() },
                kind: "conduit".into(),
                shape: "round".into(),
                size_in: [r.size_in, r.size_in],
                from: format!("{i}a"),
                to: format!("{i}b"),
                level: level.to_string(),
                path_ft: [[r.from_ft[0], r.from_ft[1], z], [r.to_ft[0], r.to_ft[1], z]],
                ..Segment::default()
            });
        }
        let run_of = |id: &str| id.rsplit('-').next().and_then(|n| n.parse::<usize>().ok()).and_then(|i| runs.get(i));
        out.extend(clash::clashes(&t, &[]).into_iter().filter(|c| match (run_of(&c.a), run_of(&c.b)) {
            (Some(a), Some(b)) => !tee(a, b),
            _ => true,
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(level: &str, a: [f64; 2], b: [f64; 2], bottom: f64) -> PrintRun {
        PrintRun { level: level.into(), from_ft: a, to_ft: b, size_in: 1.0, bottom_ft: Some(bottom), from_note: false, kind: "branch".into() }
    }

    #[test]
    fn runs_crossing_at_one_height_clash_and_tees_levels_and_heights_dont() {
        let across = run("1", [0.0, 10.0], [40.0, 10.0], 9.0);
        // Crossing it at its height: a clash.
        assert_eq!(clashes(&[across.clone(), run("1", [20.0, 0.0], [20.0, 30.0], 9.0)]).len(), 1);
        // Crossing a foot under it: none.
        assert!(clashes(&[across.clone(), run("1", [20.0, 0.0], [20.0, 30.0], 8.0)]).is_empty());
        // Ending on it (a tee): connected.
        assert!(clashes(&[across.clone(), run("1", [20.0, 0.0], [20.0, 10.0], 9.0)]).is_empty());
        // On another level: never compared.
        assert!(clashes(&[across.clone(), run("2", [20.0, 0.0], [20.0, 30.0], 9.0)]).is_empty());
        // Without a height: left out.
        let unknown = PrintRun { bottom_ft: None, ..run("1", [20.0, 0.0], [20.0, 30.0], 9.0) };
        assert!(clashes(&[across, unknown]).is_empty());
    }
}
