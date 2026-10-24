//! A print set read the way a contractor gets one: a merged PDF (or a PDF per sheet), every
//! page imported and recognised as a sheet without the fixture's layer names, the sheet map
//! assembled, and each sheet kept as DXF for the readers (keyed by its number, as they expect).
//! Then what the readers found, put together for coordination: the conduit runs (branch runs
//! and feeder banks) with how each one's height is known, and the clashes among them.
//!
//! Fixture sheets laid out the way a real print set lays them out, where that differs from one
//! plan per sheet: two levels' plans stacked on one sheet, each titled under its view
//! (`1ST FLOOR LIGHTING PLAN`), each grid line with one bubble per view, the title block's
//! column headed by a block of general notes, and everything on one pen layer as a printed
//! sheet comes in. The sheet readers must split such a sheet into its views
//! ([`crate::sheet_reader::recognise_views`]) and align each on its own.

use crate::clash;
use crate::conduit_reader::ConduitResult;
use crate::draw;
use crate::layout::{self, Kind};
use crate::sheet_reader::{self, RecognisedSheet, SheetMap};
use crate::style::Style;
use crate::tray_reader::TrayResult;
use crate::truth::{HeightSource, Segment, Truth};
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, HAlign, Space, Text, VAlign, entity_bounds};
use cadaddy_geom::{Mat3, Vec2, Vec3};

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
/// `recover_text: false` keeps every stroke intact for downstream geometry analysis (walls,
/// conduits, grids) — SHX recovery and symbol detection would consume strokes that are not text.
pub fn import_page(pdf: &[u8], page: u32) -> Result<cadaddy_doc::Drawing, String> {
    let opts = cadaddy_io::pdf_read::PdfOptions {
        page,
        images: false,
        markups: false,
        layout: false,
        layers: cadaddy_io::pdf_read::LayerMode::Pen,
        recover_text: false,
        ..Default::default()
    };
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

// ---- Fixture sheets laid out as real print sets lay them out ----------------------------------

/// A two-view sheet and what a reader should find on it.
#[derive(Clone, Debug)]
pub struct TwoViewSheet {
    pub drawing: Drawing,
    /// The printed sheet number.
    pub number: String,
    /// The title in the title block (`LIGHTING PLAN - FIRST & SECOND FLOORS`).
    pub title: String,
    /// The levels of the lower and the upper view.
    pub levels: [String; 2],
    /// How far up the upper view was moved, model units: its alignment's offset is the lower
    /// view's plus this.
    pub shift: f64,
}

/// `1` → `1ST`, `2` → `2ND`, `13` → `13TH`.
fn ordinal(code: &str) -> String {
    let n: u32 = code.parse().unwrap_or(0);
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "TH",
        (1, _) => "ST",
        (2, _) => "ND",
        (3, _) => "RD",
        _ => "TH",
    };
    format!("{code}{suffix}")
}

/// `1` → `FIRST` … `10` → `TENTH`.
fn ordinal_word(code: &str) -> String {
    const WORDS: [&str; 10] = ["FIRST", "SECOND", "THIRD", "FOURTH", "FIFTH", "SIXTH", "SEVENTH", "EIGHTH", "NINTH", "TENTH"];
    code.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| WORDS.get(i)).map_or_else(|| ordinal(code), |w| (*w).to_string())
}

/// The plan part of a fixture sheet: everything but the title block, its legend and border.
fn plan_part(d: &Drawing) -> Vec<Entity> {
    d.model.iter().filter(|e| !["TITLE", "BORDER", "LEGEND"].iter().any(|l| e.common.layer.eq_ignore_ascii_case(l))).map(|e| (**e).clone()).collect()
}

/// Drops the grid bubbles at the bottom and the right end of each line (keeping one per line, at
/// the top and the left, as a print with one bubble per line does).
fn one_bubble_per_line(d: &Drawing, items: Vec<Entity>) -> Vec<Entity> {
    let bubble_box = items
        .iter()
        .filter(|e| e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE"))
        .fold(cadaddy_geom::Bounds2::EMPTY, |b, e| b.union(&entity_bounds(d, e, 0)));
    if bubble_box.is_empty() {
        return items;
    }
    let (w, h) = (bubble_box.width(), bubble_box.height());
    items
        .into_iter()
        .filter(|e| {
            if !e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE") {
                return true;
            }
            let c = entity_bounds(d, e, 0).center();
            c.y > bubble_box.min.y + h * 0.05 && c.x < bubble_box.max.x - w * 0.05
        })
        .collect()
}

fn text(at: Vec2, height: f64, s: &str) -> EntityKind {
    EntityKind::Text(Text {
        insert: Vec3::new(at.x, at.y, 0.0),
        align_pt: None,
        height,
        value: s.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "STANDARD".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    })
}

/// Two plan sheets of the set (`lower` and `upper` sheet numbers, both drawn as `kind`'s
/// discipline) printed as one sheet numbered `number`: `lower`'s plan below, `upper`'s above,
/// each titled `<level> FLOOR <discipline word> PLAN` under it, one bubble per grid line, a block of
/// general notes heading the title column, all on layer `0`. `None` when either sheet isn't in
/// the set or isn't a plan of one level.
pub fn two_view_sheet(t: &Truth, st: &Style, lower: &str, upper: &str, number: &str, discipline_word: &str) -> Option<TwoViewSheet> {
    let kinds = layout::hotel_sheet_list();
    let kind = |n: &str| kinds.iter().find(|(k, _, _, _)| k == n).map(|(_, _, _, k)| k.clone());
    let sheet = |n: &str| t.sheets.iter().find(|s| s.number == n);
    let (sl, su) = (sheet(lower)?, sheet(upper)?);
    let (kl, ku): (Kind, Kind) = (kind(lower)?, kind(upper)?);
    let levels = [sl.level.clone()?, su.level.clone()?];
    let (dl, _, _) = draw::draw(t, st, sl, &kl);
    let (du, _, _) = draw::draw(t, st, su, &ku);
    let lower_items = one_bubble_per_line(&dl, plan_part(&dl));
    let upper_items = one_bubble_per_line(&du, plan_part(&du));
    let extent = |d: &Drawing, items: &[Entity]| items.iter().fold(cadaddy_geom::Bounds2::EMPTY, |b, e| b.union(&entity_bounds(d, e, 0)));
    let (bl, bu) = (extent(&dl, &lower_items), extent(&du, &upper_items));
    if bl.is_empty() || bu.is_empty() {
        return None;
    }
    // The sheet's usual lettering, for the view titles and notes.
    let mut heights: Vec<f64> = lower_items
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some(t.height),
            EntityKind::MText(t) => Some(t.height),
            _ => None,
        })
        .filter(|h| h.is_finite() && *h > 0.0)
        .collect();
    heights.sort_by(f64::total_cmp);
    let h = heights.get(heights.len() / 2).copied().unwrap_or(bl.height() * 0.01);
    let gap = h * 8.0;
    // The upper view sits a title's room above the lower one.
    let shift = bl.max.y - bu.min.y + gap * 2.0;
    let move_up = Mat3::translate(Vec2::new(0.0, shift));

    let mut out = Drawing::new_imperial();
    let mut items: Vec<EntityKind> = Vec::new();
    for e in lower_items {
        items.push(e.kind);
    }
    for e in upper_items {
        let mut kind = e.kind;
        kind.transform(&move_up);
        items.push(kind);
    }
    // View titles under each plan.
    let title_h = h * 2.5;
    let view_title = |level: &str| format!("{} FLOOR {discipline_word} PLAN", ordinal(level));
    let [ll, lu] = &levels;
    items.push(text(Vec2::new(bl.center().x - title_h * 6.0, bl.min.y - gap), title_h, &view_title(ll)));
    items.push(text(Vec2::new(bl.center().x - title_h * 6.0, bu.min.y + shift - gap), title_h, &view_title(lu)));
    // The title column on the right: general notes at the top, the sheet's title and number at
    // the foot.
    let top = bu.max.y + shift;
    let bottom = bl.min.y - gap * 2.0;
    let x = bl.max.x.max(bu.max.x) + (top - bottom) * 0.25;
    items.push(text(Vec2::new(x, top - title_h), title_h, "SHEET NOTES -"));
    items.push(text(Vec2::new(x, top - title_h * 2.4), title_h, "FLOOR PLAN"));
    for (i, note) in ["VERIFY ALL DIMENSIONS IN THE FIELD.", "LEVEL MARKS ARE TO FINISHED FLOOR.", "COORDINATE WITH ALL TRADES."].iter().enumerate() {
        items.push(text(Vec2::new(x, top - title_h * 4.0 - h * 2.0 * i as f64), h, &format!("{}. {note}", i + 1)));
    }
    let title = format!("{discipline_word} PLAN - {} & {} FLOORS", ordinal_word(ll), ordinal_word(lu));
    items.push(text(Vec2::new(x, bottom + title_h * 6.0), title_h * 0.8, &format!("{discipline_word} PLAN -")));
    items.push(text(Vec2::new(x, bottom + title_h * 4.8), title_h * 0.8, &format!("{} & {} FLOORS", ordinal_word(ll), ordinal_word(lu))));
    items.push(text(Vec2::new(x, bottom + title_h * 3.0), h, &format!("SCALE: {}", sl.scale_text.clone().unwrap_or_default())));
    items.push(text(Vec2::new(x, bottom), title_h * 3.0, number));
    for kind in items {
        // A print is one pen layer.
        out.add(&Space::Model, Common { layer: "0".into(), ..Common::default() }, kind).ok()?;
    }
    Some(TwoViewSheet { drawing: out, number: number.into(), title, levels, shift })
}

/// How [`one_view_sheet`] prints its plan, where real sets differ from sheet to sheet.
#[derive(Clone, Debug)]
pub struct OneViewOptions {
    /// How many grid lines are printed, first the lines of constant x then of constant y (a
    /// floor's sheets printed with the grid turned off but for a stray line: its bubbles stand
    /// in a row along the top and a column down the left with nothing leaving them).
    pub grid_lines: usize,
    /// Two pairs of same-label keynote tags, each pair in line (one pair up and down, one
    /// across), drawn the size of the grid bubbles: on their own they read as a 1×1 "grid".
    pub keynote_pairs: bool,
}

impl Default for OneViewOptions {
    fn default() -> Self {
        Self { grid_lines: usize::MAX, keynote_pairs: false }
    }
}

/// A one-view printed sheet and what a reader should find on it.
#[derive(Clone, Debug)]
pub struct OneViewSheet {
    pub drawing: Drawing,
    pub number: String,
    pub level: String,
    /// The grid lines a reader should find (every line of the building's grid).
    pub grid_x: usize,
    pub grid_y: usize,
}

/// One plan sheet of the set (`number`, drawn as its kind) printed the way a print set does: one
/// bubble per grid line, the view titled `<level> FLOOR <discipline word> PLAN` under it, a title
/// column on the right, all on layer `0`, with `opts` for the ways a real sheet strays. `None`
/// when the sheet isn't in the set or isn't a plan of one level.
pub fn one_view_sheet(t: &Truth, st: &Style, number: &str, discipline_word: &str, opts: &OneViewOptions) -> Option<OneViewSheet> {
    let kinds = layout::hotel_sheet_list();
    let kind = kinds.iter().find(|(k, _, _, _)| k == number).map(|(_, _, _, k)| k.clone())?;
    let sheet = t.sheets.iter().find(|s| s.number == number)?;
    let level = sheet.level.clone()?;
    let (d, _, _) = draw::draw(t, st, sheet, &kind);
    let mut grid_seen = 0usize;
    let part: Vec<Entity> = plan_part(&d)
        .into_iter()
        .filter(|e| {
            if !e.common.layer.eq_ignore_ascii_case("GRID") {
                return true;
            }
            grid_seen += 1;
            grid_seen <= opts.grid_lines
        })
        .collect();
    let items = one_bubble_per_line(&d, part);
    let b = items.iter().fold(cadaddy_geom::Bounds2::EMPTY, |b, e| b.union(&entity_bounds(&d, e, 0)));
    if b.is_empty() {
        return None;
    }
    let bubble_r = items
        .iter()
        .filter(|e| e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE"))
        .find_map(|e| match &e.kind {
            EntityKind::Circle(c) => Some(c.radius),
            _ => None,
        })
        .unwrap_or(b.height() * 0.01);
    let mut heights: Vec<f64> = items
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some(t.height),
            EntityKind::MText(t) => Some(t.height),
            _ => None,
        })
        .filter(|h| h.is_finite() && *h > 0.0)
        .collect();
    heights.sort_by(f64::total_cmp);
    let h = heights.get(heights.len() / 2).copied().unwrap_or(b.height() * 0.01);
    let gap = h * 8.0;
    let mut kinds_out: Vec<EntityKind> = items.into_iter().map(|e| e.kind).collect();
    if opts.keynote_pairs {
        // Labels no grid line has, each tag a grid bubble's size, in the margin under the plan
        // (where nothing leaves them): one pair stacked, one pair side by side.
        let taken: Vec<&str> = t.building.grid.x.iter().chain(&t.building.grid.y).map(|g| g.label.as_str()).collect();
        let mut free = (1..100u32).map(|n| n.to_string()).filter(|l| !taken.contains(&l.as_str()));
        let (Some(l1), Some(l2)) = (free.next(), free.next()) else { return None };
        let y0 = b.min.y - gap * 3.0;
        let x0 = b.min.x + b.width() * 0.3;
        let step = bubble_r * 6.0;
        for (c, l) in [([x0, y0], &l1), ([x0, y0 - step], &l1), ([x0 + step * 2.0, y0], &l2), ([x0 + step * 3.0, y0], &l2)] {
            kinds_out.push(EntityKind::Circle(cadaddy_doc::Circle { center: Vec3::new(c[0], c[1], 0.0), radius: bubble_r }));
            let th = bubble_r * 0.9;
            kinds_out.push(text(Vec2::new(c[0] - th * 0.35 * l.len() as f64, c[1] - th * 0.5), th, l));
        }
    }
    let title_h = h * 2.5;
    kinds_out.push(text(
        Vec2::new(b.center().x - title_h * 6.0, b.min.y - gap),
        title_h,
        &format!("{} FLOOR {discipline_word} PLAN", ordinal(&level)),
    ));
    let (top, bottom) = (b.max.y, b.min.y - gap * 2.0);
    let x = b.max.x + (top - bottom) * 0.25;
    kinds_out.push(text(Vec2::new(x, bottom + title_h * 6.0), title_h * 0.8, &format!("{} FLOOR {discipline_word} PLAN", ordinal_word(&level))));
    kinds_out.push(text(Vec2::new(x, bottom + title_h * 3.0), h, &format!("SCALE: {}", sheet.scale_text.clone().unwrap_or_default())));
    kinds_out.push(text(Vec2::new(x, bottom), title_h * 3.0, number));
    let mut out = Drawing::new_imperial();
    for kind in kinds_out {
        out.add(&Space::Model, Common { layer: "0".into(), ..Common::default() }, kind).ok()?;
    }
    Some(OneViewSheet { drawing: out, number: number.into(), level, grid_x: t.building.grid.x.len(), grid_y: t.building.grid.y.len() })
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
