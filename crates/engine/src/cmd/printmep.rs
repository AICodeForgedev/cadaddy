//! Print-set MEP (Q5272): ductwork read on the mechanical plans and piping on the plumbing
//! plans of a print set, each sheet on its own, each run on its sheet's level.
//!
//! A job's drawing holds its plan sheets side by side (one rectangle each, with the handles its
//! import made); the readers see one sheet at a time, cut out of it, so an electrical or
//! architectural sheet never adds ducts or pipes, and what one sheet's tags say never reaches
//! another sheet's lines. What a sheet reads is moved onto the architectural plan of its level
//! (same sheet size: the same title block and plan placement) so a level's trades line up.

use cadaddy_doc::{Drawing, EntityStore, Space};
use cadaddy_walk3d::{BuildingSpec, Level, Mep, MepKind, MepRun};
use serde_json::{Value, json};

use crate::Session;

/// How many runs a print set may add (hostile or huge sets).
const MAX_SET_RUNS: usize = 50_000;
/// How many sheets one read takes.
const MAX_SHEETS: usize = 500;
/// Invert notes kept per sheet and per set.
const MAX_SHEET_INVERTS: usize = 200;
const MAX_SET_INVERTS: usize = 2_000;
/// How near a drain an invert note is printed (feet): farther, it is another drawing's.
const INVERT_REACH_FT: f64 = 6.0;

/// An invert note a plan prints: its depth, its text, and the points it marks (along its text,
/// and the far end of a leader drawn from it).
struct InvertNote {
    depth_ft: f64,
    text: String,
    points: Vec<[f64; 2]>,
}

/// A character's width in its text's height (a plan's sans-serif text), for the box a note
/// covers: its insertion point is its left end.
const CHAR_WIDTH: f64 = 0.6;

/// The longest leader line from a note, in its text's heights.
const LEADER_MAX_H: f64 = 12.0;

/// The invert notes of a sheet: each with points along its text's box (horizontal text: its
/// width from its characters) and the far end of each leader line that starts at the box
/// (within a text height of it, no longer than [`LEADER_MAX_H`] heights).
fn invert_notes(sub: &Drawing) -> Vec<InvertNote> {
    let mut out: Vec<(InvertNote, f64)> = cadaddy_mepfix::sheet_facts::texts(sub)
        .into_iter()
        .take(200_000)
        .filter_map(|t| {
            let depth_ft = super::mep::invert_tag(&t.value)?;
            let (x, y, h) = (t.at.x, t.at.y, t.height);
            let w = if t.rotation.abs() <= 0.01 { CHAR_WIDTH * h * t.value.trim().chars().count() as f64 } else { 0.0 };
            let points = (0..=4).flat_map(|k| [[x + w * f64::from(k) / 4.0, y], [x + w * f64::from(k) / 4.0, y + h]]).collect();
            Some((InvertNote { depth_ft, text: t.value.trim().to_string(), points }, h))
        })
        .take(MAX_SHEET_INVERTS)
        .collect();
    if out.is_empty() {
        return Vec::new();
    }
    let lines: Vec<([f64; 2], [f64; 2])> = sub
        .model
        .iter()
        .take(2_000_000)
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::Line(l) => Some(([l.a.x, l.a.y], [l.b.x, l.b.y])),
            cadaddy_doc::EntityKind::LwPolyline(p) if p.vertices.len() == 2 && !p.closed => {
                let (a, b) = (p.vertices.first()?.p, p.vertices.last()?.p);
                Some(([a.x, a.y], [b.x, b.y]))
            }
            _ => None,
        })
        .filter(|(a, b)| a.iter().chain(b).all(|v| v.is_finite()))
        .collect();
    for (note, h) in &mut out {
        let h = *h;
        // The text's baseline and top, start to end.
        let base: Vec<[f64; 2]> = note.points.iter().step_by(2).copied().collect();
        let top: Vec<[f64; 2]> = note.points.iter().skip(1).step_by(2).copied().collect();
        let near_box = |p: [f64; 2]| {
            [&base, &top].iter().any(|row| match (row.first(), row.last()) {
                (Some(a), Some(b)) => seg_dist(p, *a, *b) <= h,
                _ => false,
            })
        };
        let mut ends: Vec<[f64; 2]> = Vec::new();
        for (a, b) in &lines {
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            if len <= h || len > LEADER_MAX_H * h {
                continue;
            }
            match (near_box(*a), near_box(*b)) {
                (true, false) => ends.push(*b),
                (false, true) => ends.push(*a),
                _ => {}
            }
            if ends.len() >= 8 {
                break;
            }
        }
        note.points.extend(ends);
    }
    out.into_iter().map(|(n, _)| n).collect()
}

/// The distance from `p` to the segment `a`–`b`.
fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 1e-18 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - dx * t).hypot(p[1] - a[1] - dy * t)
}

/// Feet to one decimal (never `-0.0`).
pub(crate) fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0 + 0.0
}

/// One imported sheet of the drawing.
#[derive(Clone, Debug, Default)]
pub(crate) struct SheetRef {
    /// `M-101`.
    pub number: String,
    /// The level code the print set gives it (`1`, `R`), empty when none.
    pub level: String,
    /// `M`, `P`, `A`, `E`…
    pub discipline: String,
    /// `plan`, `enlarged_plan`…
    pub kind: String,
    /// Where it sits in the drawing, `[x0, y0, x1, y1]`.
    pub rect: [f64; 4],
    /// The handles its import made, `lo..hi` (none: what lies in `rect`).
    pub span: Option<(u64, u64)>,
    /// Its scale as printed (`1/8" = 1'-0"`), for a sheet left at paper size.
    pub scale_text: Option<String>,
    /// Every level this sheet covers (a typical-floor plan).  Empty = only `level`.
    pub levels: Vec<String>,
    /// The one view of a several-view sheet this is (`[x0, y0, x1, y1]`): only what lies in it
    /// is read. None: the whole sheet.
    pub view: Option<[f64; 4]>,
}

impl SheetRef {
    fn is_plan(&self) -> bool {
        matches!(self.kind.as_str(), "plan" | "enlarged_plan")
    }
    /// Whether model elements are read off this sheet: a plan on a building level. A plan of no
    /// level (uncertain, not answered yet) or on the site datum is a reference sheet.
    pub fn builds(&self) -> bool {
        self.is_plan() && building_level(&self.level)
    }
    /// The trade read on this sheet: ducts on mechanical plans, pipes on plumbing plans.
    pub fn trade(&self) -> Option<MepKind> {
        if !self.builds() {
            return None;
        }
        match self.discipline.to_ascii_uppercase().as_str() {
            "M" => Some(MepKind::Duct),
            "P" => Some(MepKind::Pipe),
            _ => None,
        }
    }
}

/// Whether a print-set level code is a building level elements go on: not empty, not the
/// site datum (`SITE`, `GRADE`).
pub(crate) fn building_level(code: &str) -> bool {
    let c = code.trim();
    !c.is_empty() && !cadaddy_walk3d::levels::is_datum_name(c)
}

/// The spec's level name for a print-set level code: `1` → `Level 1`, `R` → `Roof`.
pub(crate) fn level_name(code: &str) -> String {
    let c = code.trim();
    match c {
        "" => "Level 1".to_string(),
        "R" | "r" => "Roof".to_string(),
        l if l.chars().all(|ch| ch.is_ascii_digit()) => format!("Level {l}"),
        l => l.to_string(),
    }
}

/// What one sheet read.
#[derive(Clone, Debug, Default)]
pub(crate) struct SheetRead {
    pub sheet: String,
    pub level: String,
    pub trade: &'static str,
    pub runs: usize,
    pub length_ft: f64,
    pub terminals: usize,
    pub equipment: usize,
    pub warnings: Vec<String>,
}

impl SheetRead {
    pub fn to_json(&self) -> Value {
        json!({
            "sheet": self.sheet, "level": self.level, "trade": self.trade, "runs": self.runs,
            "length_ft": r1(self.length_ft), "terminals": self.terminals,
            "equipment": self.equipment, "warnings": self.warnings,
        })
    }
}

/// What a print set's M and P plans read: the MEP parts, sheet by sheet.
#[derive(Clone, Debug, Default)]
pub(crate) struct SetRead {
    pub mep: Mep,
    pub sheets: Vec<SheetRead>,
    pub warnings: Vec<String>,
}

impl SetRead {
    pub fn length_ft(&self, trade: &str) -> f64 {
        self.sheets.iter().filter(|s| s.trade == trade).map(|s| s.length_ft).sum()
    }
    pub fn runs(&self, trade: &str) -> usize {
        self.sheets.iter().filter(|s| s.trade == trade).map(|s| s.runs).sum()
    }
    pub fn to_json(&self) -> Value {
        json!({
            "duct_runs": self.runs("duct"),
            "duct_ft": r1(self.length_ft("duct")),
            "pipe_runs": self.runs("pipe"),
            "pipe_ft": r1(self.length_ft("pipe")),
            "terminals": self.mep.terminals.len(),
            "equipment": self.mep.equipment.len(),
            "sheets": self.sheets.iter().map(SheetRead::to_json).collect::<Vec<_>>(),
            "warnings": self.warnings,
        })
    }
}

/// The objects of one sheet of `d`, as a drawing of their own (tables and blocks kept).
pub(crate) fn sheet_drawing(d: &Drawing, sh: &SheetRef) -> Drawing {
    let mut out = d.clone();
    let mut store = EntityStore::new();
    let [x0, y0, x1, y1] = sh.rect;
    let mine: Vec<cadaddy_doc::Entity> = d
        .model
        .iter()
        .filter(|e| {
            let middle = || {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                let (cx, cy) = ((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
                (cx.is_finite() && cy.is_finite()).then_some((cx, cy))
            };
            let on_sheet = match sh.span {
                Some((lo, hi)) => e.handle.0 >= lo && e.handle.0 < hi,
                None => middle().is_some_and(|(cx, cy)| cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1),
            };
            // One view of a several-view sheet: its own plan only, never the other level's.
            on_sheet && sh.view.is_none_or(|[vx0, vy0, vx1, vy1]| middle().is_some_and(|(cx, cy)| cx >= vx0 && cx <= vx1 && cy >= vy0 && cy <= vy1))
        })
        .map(|e| (**e).clone())
        .collect();
    store.push_many(mine);
    out.model = store;
    out.building_spec = None;
    out.agent = None;
    out
}

/// Drawing units per building foot of a sheet left at paper size (its printed scale, else
/// 1/8" = 1'-0"); none when it was imported at its scale.
pub(crate) fn paper_units_per_foot(d: &Drawing, sh: &SheetRef) -> Option<f64> {
    let ext = d.extents(&Space::Model);
    let (w, h) = (ext.max.x - ext.min.x, ext.max.y - ext.min.y);
    let diag = (w * w + h * h).sqrt();
    if !(diag.is_finite() && diag > 1.0 && diag < 100.0) {
        return None;
    }
    let printed = sh.scale_text.as_deref().and_then(cadaddy_mepfix::sheet_reader::parse_scale).filter(|k| k.is_finite() && *k > 0.0 && *k <= 12.0);
    Some(printed.unwrap_or(0.125))
}

/// Where a sheet's reading goes: `to + (p - from) * k`.
#[derive(Clone, Copy, Debug)]
struct Placement {
    from: [f64; 2],
    to: [f64; 2],
    k: f64,
}

impl Placement {
    fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [self.to[0] + (p[0] - self.from[0]) * self.k, self.to[1] + (p[1] - self.from[1]) * self.k]
    }
}

/// Where `sh`'s reading goes: onto its level's architectural floor plan when that sheet is the
/// same size (the same title block and plan placement), else where it is; scaled up from paper
/// size by `k`.
/// One view of a several-view sheet goes onto the same view of a several-view architectural
/// sheet laid out alike, else stays where it is.
fn placement(sh: &SheetRef, all: &[SheetRef], k: f64) -> Placement {
    let size = |r: &[f64; 4]| (r[2] - r[0], r[3] - r[1]);
    let own = sh.view.unwrap_or(sh.rect);
    let (w, h) = size(&own);
    let arch = all.iter().find(|a| {
        let (aw, ah) = size(&a.view.unwrap_or(a.rect));
        a.view.is_some() == sh.view.is_some()
            && a.discipline.eq_ignore_ascii_case("A")
            && a.kind == "plan"
            && sh.kind == "plan"
            && !sh.level.is_empty()
            && a.level.eq_ignore_ascii_case(&sh.level)
            && w > 0.0
            && h > 0.0
            && ((aw - w * k) / (w * k)).abs() < 0.02
            && ((ah - h * k) / (h * k)).abs() < 0.02
    });
    let from = [own[0], own[1]];
    Placement {
        from,
        to: arch.map_or(from, |a| {
            let r = a.view.unwrap_or(a.rect);
            [r[0], r[1]]
        }),
        k,
    }
}

fn path_ft(points: &[[f64; 2]], upf: f64) -> f64 {
    let mut total = 0.0;
    for w in points.windows(2) {
        if let [a, b] = w {
            let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
            if d.is_finite() {
                total += d;
            }
        }
    }
    total / upf
}

/// Words of a title that make a sheet or view a reference one (not a plan to read runs off).
const REFERENCE_WORDS: &[(&str, &str)] = &[
    ("SCHEDULE", "schedule"),
    ("DETAIL", "detail"),
    ("LEGEND", "legend"),
    ("DIAGRAM", "diagram"),
    ("RISER", "riser"),
    ("SECTION", "section"),
    ("ELEVATION", "elevation"),
    ("NOTES", "notes"),
];

/// What kind of reference sheet `title` names (`detail`), none for a plan's title.
fn reference_title(title: &str) -> Option<&'static str> {
    let up = title.to_ascii_uppercase();
    if up.contains("PLAN") {
        return None;
    }
    REFERENCE_WORDS.iter().find(|(w, _)| up.contains(w)).map(|(_, k)| *k)
}

/// Whether the cut-out sheet `sub` is a details/schedules/diagrams sheet by its own titles: its
/// title block's title, else (no title read) its view titles when there are several and every
/// one is a reference view's.
fn reference_sheet(sub: &Drawing) -> Option<&'static str> {
    use cadaddy_mepfix::sheet_facts;
    let texts = sheet_facts::texts(sub);
    if let Some(id) = sheet_facts::identify_sheet(&texts)
        && !id.title.trim().is_empty()
    {
        return reference_title(&id.title);
    }
    let views = sheet_facts::view_titles(sub, &texts);
    let kinds: Vec<Option<&'static str>> = views.iter().map(|v| reference_title(&v.title)).collect();
    if kinds.len() < 2 || kinds.iter().any(Option::is_none) {
        return None;
    }
    kinds.into_iter().flatten().next()
}

/// Read one sheet's trade into spec parts in `d`'s units (`upf` per foot).
fn read_sheet(d: &Drawing, sh: &SheetRef, all: &[SheetRef], trade: MepKind, upf: f64) -> (Mep, SheetRead) {
    let level = level_name(&sh.level);
    let mut read = SheetRead {
        sheet: sh.number.clone(),
        level: level.clone(),
        trade: if trade == MepKind::Duct { "duct" } else { "pipe" },
        ..SheetRead::default()
    };
    let sub = sheet_drawing(d, sh);
    if sub.model.is_empty() {
        read.warnings.push(format!("{}: no objects imported for this sheet", sh.number));
        return (Mep::default(), read);
    }
    let paper = paper_units_per_foot(&sub, sh);
    let sheet_upf = paper.unwrap_or(upf);
    // No MEP systems: the readers take them from the sheet's layers and tags.
    let spec = BuildingSpec {
        units_per_foot: sheet_upf,
        levels: vec![Level {
            name: level.clone(),
            elevation_ft: 0.0,
            height_ft: 10.0,
            ceiling_ft: 8.0,
            region: None,
            base: None,
            attr: None,
            sheet: None,
        }],
        mep: Mep::default(),
        ..BuildingSpec::default()
    };
    // A notes sheet (DO NOT SCALE, as the per-sheet duct read skips): its schedule tables'
    // parallel lines are not ducts.
    if trade == MepKind::Duct
        && let Some(note) = super::import_check::detect_do_not_scale(&sub)
    {
        read.warnings.push(format!("{}: a notes sheet (\"{note}\"): no ducts read", sh.number));
        return (Mep::default(), read);
    }
    // A sheet classify took for a plan (or had no entry for) whose own title block or view
    // titles say details, schedules or diagrams: their linework is no ductwork or piping.
    if let Some(what) = reference_sheet(&sub) {
        read.warnings.push(format!("{}: a {what} sheet, not a plan: no {} read", sh.number, if trade == MepKind::Duct { "ducts" } else { "pipes" }));
        return (Mep::default(), read);
    }
    let mut mep = match trade {
        MepKind::Duct => {
            let plain = cadaddy_mepfix::duct_reader::read_ducts(&sub, &cadaddy_mepfix::truth::Sheet::default());
            let net = if plain.runs.is_empty() { cadaddy_mepfix::duct_reader::read_ducts_any_layer(&sub) } else { plain };
            super::readers::duct_mep(&net, &level, &sh.number, &spec)
        }
        _ => {
            // The inverts the sheet prints (`24" INVERT`): where its drains' depths come from.
            let invert_notes: Vec<InvertNote> = invert_notes(&sub);
            let mut t = Session::empty();
            t.open_drawing(sub, &sh.number, None);
            match super::mep::read(&t, &spec, None) {
                Ok((systems, runs)) => {
                    let pipes: Vec<String> = systems.iter().filter(|s| s.kind == MepKind::Pipe).map(|s| s.name.clone()).collect();
                    let mut m = Mep {
                        runs: runs.into_iter().filter(|r| pipes.contains(&r.system)).collect(),
                        systems: systems.into_iter().filter(|s| s.kind == MepKind::Pipe).collect(),
                        ..Mep::default()
                    };
                    for r in &mut m.runs {
                        r.source_sheet.clone_from(&sh.number);
                        r.objects.clear();
                    }
                    m.runs = super::runtidy::tidy_runs(std::mem::take(&mut m.runs), sheet_upf);
                    // An invert is a drain's: one by a drainage run (within reach of its text or
                    // of its leader's end) is kept, at its point nearest the drain.
                    let reach = INVERT_REACH_FT * sheet_upf;
                    let drains: Vec<&MepRun> = m.runs.iter().filter(|r| super::ugplan::is_drainage(&r.system)).collect();
                    for note in invert_notes {
                        let nearest = note
                            .points
                            .iter()
                            .filter_map(|p| {
                                let d = drains
                                    .iter()
                                    .flat_map(|r| r.points.windows(2).filter_map(|w| if let [a, b] = w { Some(seg_dist(*p, *a, *b)) } else { None }))
                                    .fold(f64::INFINITY, f64::min);
                                d.is_finite().then_some((d, *p))
                            })
                            .min_by(|a, b| a.0.total_cmp(&b.0));
                        if let Some((d, at)) = nearest
                            && d <= reach
                        {
                            m.inverts.push(cadaddy_walk3d::MepInvert {
                                level: level.clone(),
                                at,
                                depth_ft: note.depth_ft,
                                text: note.text,
                                source_sheet: sh.number.clone(),
                            });
                        }
                    }
                    m
                }
                Err(e) => {
                    read.warnings.push(format!("{}: the pipe reader failed: {e}", sh.number));
                    Mep::default()
                }
            }
        }
    };
    // Into the drawing's units, onto the level's architectural plan.
    let k = if paper.is_some() { upf / sheet_upf } else { 1.0 };
    if paper.is_some() {
        read.warnings.push(format!("{}: at paper size; read at {} drawing units per foot", sh.number, sheet_upf));
    }
    let at = placement(sh, all, k);
    for r in &mut mep.runs {
        for p in &mut r.points {
            *p = at.apply(*p);
        }
    }
    for t in &mut mep.terminals {
        t.at = at.apply(t.at);
    }
    for v in &mut mep.inverts {
        v.at = at.apply(v.at);
    }
    for e in &mut mep.equipment {
        e.min = at.apply(e.min);
        e.max = at.apply(e.max);
    }
    // A segment drawn again (traced back and forth) is one run: the reading counts it once, as the
    // model does.
    if trade == MepKind::Duct {
        super::runtidy::drop_repeats(&mut mep.runs, upf, &|_| true);
    }
    read.runs = mep.runs.len();
    read.length_ft = mep.runs.iter().map(|r| path_ft(&r.points, upf)).sum();
    read.terminals = mep.terminals.len();
    read.equipment = mep.equipment.len();
    if read.runs == 0 {
        read.warnings.push(format!(
            "{} ({}): no {} runs read on this {} plan",
            sh.number,
            level,
            read.trade,
            if trade == MepKind::Duct { "mechanical" } else { "plumbing" }
        ));
    }
    (mep, read)
}

/// Read every mechanical plan's ducts and plumbing plan's pipes among `sheets` of `d` (`upf`
/// drawing units per foot); `only`: one trade.
pub(crate) fn read_sheets(d: &Drawing, sheets: &[SheetRef], upf: f64, only: Option<MepKind>) -> SetRead {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let mut out = SetRead::default();
    let mine: Vec<&SheetRef> = sheets.iter().filter(|s| s.trade().is_some_and(|t| only.is_none_or(|o| o == t))).take(MAX_SHEETS).collect();
    for sh in mine {
        let Some(trade) = sh.trade() else { continue };
        let (mep, read) = read_sheet(d, sh, sheets, trade, upf);
        out.warnings.extend(read.warnings.iter().cloned());
        // Typical-floor replication: if the sheet covers several levels, clone the MEP
        // for each additional level (the first is the primary, already read).
        let extra_levels = extra_levels_for(sh);
        for extra in &extra_levels {
            let extra_name = level_name(extra);
            let cloned = clone_mep_for_level(&mep, &extra_name);
            out.sheets.push(SheetRead {
                sheet: sh.number.clone(),
                level: extra_name,
                trade: read.trade,
                runs: cloned.runs.len(),
                length_ft: read.length_ft,
                terminals: cloned.terminals.len(),
                equipment: cloned.equipment.len(),
                warnings: Vec::new(),
            });
            let room = MAX_SET_RUNS.saturating_sub(out.mep.runs.len());
            if room == 0 {
                break;
            }
            add(&mut out.mep, cloned, room);
        }
        out.sheets.push(read);
        let room = MAX_SET_RUNS.saturating_sub(out.mep.runs.len());
        if room == 0 {
            out.warnings.push(format!("more than {MAX_SET_RUNS} runs: the rest of the sheets were not read"));
            break;
        }
        add(&mut out.mep, mep, room);
    }
    out
}

/// Additional level codes (beyond the primary) for a multi-level sheet.
fn extra_levels_for(sh: &SheetRef) -> Vec<String> {
    if sh.levels.len() <= 1 {
        return Vec::new();
    }
    // Skip the first level (the primary, already used by read_sheet).
    sh.levels[1..].to_vec()
}

/// Clone a MEP read, changing every run/terminal/equipment level to `target`.
fn clone_mep_for_level(src: &Mep, target: &str) -> Mep {
    let mut out = src.clone();
    for r in &mut out.runs {
        r.level = target.to_string();
    }
    for t in &mut out.terminals {
        t.level = target.to_string();
    }
    for e in &mut out.equipment {
        e.level = target.to_string();
    }
    for v in &mut out.inverts {
        v.level = target.to_string();
    }
    out
}

/// Add `from`'s parts to `to` (at most `room` runs; systems once by name).
fn add(to: &mut Mep, from: Mep, room: usize) {
    for sy in from.systems {
        if !to.systems.iter().any(|s| s.name == sy.name) {
            to.systems.push(sy);
        }
    }
    to.runs.extend(from.runs.into_iter().take(room));
    to.terminals.extend(from.terminals);
    to.equipment.extend(from.equipment);
    to.inverts.extend(from.inverts.into_iter().take(MAX_SET_INVERTS.saturating_sub(to.inverts.len())));
}

/// Put a print set's MEP parts into the spec: what `sheets` read before goes (a re-read
/// replaces it, so it never counts twice), every run gets an id, and every level a run is on
/// exists (above the spec's top level when the plans gave it none).
pub(crate) fn merge_mep(sp: &mut BuildingSpec, mep: Mep, sheets: &[String]) {
    if !sheets.is_empty() {
        let from = |s: &str| sheets.iter().any(|x| x.eq_ignore_ascii_case(s));
        sp.mep.runs.retain(|r| !from(&r.source_sheet));
        sp.mep.equipment.retain(|e| !e.sources.get("sheet").is_some_and(|s| from(s)));
        let levels: Vec<&str> = mep.terminals.iter().map(|t| t.level.as_str()).collect();
        sp.mep.terminals.retain(|t| !levels.iter().any(|l| l.eq_ignore_ascii_case(&t.level)));
        sp.mep.inverts.retain(|v| !from(&v.source_sheet));
    }
    let mut next_id = sp.mep.runs.len();
    for sy in mep.systems {
        if !sp.mep.systems.iter().any(|s| s.name == sy.name) {
            sp.mep.systems.push(sy);
        }
    }
    for mut r in mep.runs.into_iter().take(MAX_SET_RUNS) {
        next_id += 1;
        if r.id.is_empty() {
            r.id = format!("R{next_id}");
        }
        sp.mep.runs.push(r);
    }
    sp.mep.terminals.extend(mep.terminals);
    sp.mep.equipment.extend(mep.equipment);
    for v in mep.inverts {
        if sp.mep.inverts.len() >= MAX_SET_INVERTS {
            break;
        }
        if !sp.mep.inverts.contains(&v) {
            sp.mep.inverts.push(v);
        }
    }
    // Nothing is built on the site datum (a site plan's utilities are no building's MEP).
    sp.mep.runs.retain(|r| r.level.trim().is_empty() || building_level(&r.level));
    sp.mep.terminals.retain(|t| t.level.trim().is_empty() || building_level(&t.level));
    sp.mep.equipment.retain(|e| e.level.trim().is_empty() || building_level(&e.level));
    sp.mep.inverts.retain(|v| v.level.trim().is_empty() || building_level(&v.level));
    ensure_levels(sp);
}

/// A region no sheet reaches (a unit square far off; the job's sheets sit at x, y >= 0): a
/// level only the MEP plans give takes no plan objects, so the architecture stays on the levels
/// its plans built.
const NO_PLAN_REGION: [f64; 4] = [-1.0e9, -1.0e9, -1.0e9 + 1.0, -1.0e9 + 1.0];

/// Every level an MEP run, terminal or piece of equipment is on exists in the spec: a missing
/// one is stacked above the top level, one storey high, with [`NO_PLAN_REGION`].
fn ensure_levels(sp: &mut BuildingSpec) {
    let mut wanted: Vec<String> = Vec::new();
    let names = sp
        .mep
        .runs
        .iter()
        .map(|r: &MepRun| r.level.as_str())
        .chain(sp.mep.terminals.iter().map(|t| t.level.as_str()))
        .chain(sp.mep.equipment.iter().map(|e| e.level.as_str()));
    for n in names {
        // The site is a datum, never a storey stacked on the building.
        if building_level(n) && sp.level(n).is_none() && !wanted.iter().any(|w| w.eq_ignore_ascii_case(n)) {
            wanted.push(n.to_string());
        }
    }
    // Numbered levels in order, the roof last.
    wanted.sort_by_key(|n| (n == "Roof", n.trim_start_matches("Level ").parse::<u32>().unwrap_or(u32::MAX), n.clone()));
    for name in wanted.into_iter().take(200) {
        let (top, height, ceiling) = sp
            .levels
            .iter()
            .map(|l| (l.elevation_ft + l.height_ft, l.height_ft, l.ceiling_ft))
            .filter(|(t, _, _)| t.is_finite())
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap_or((0.0, 10.0, 8.0));
        sp.levels.push(Level {
            name,
            elevation_ft: top,
            height_ft: height,
            ceiling_ft: ceiling,
            region: Some(NO_PLAN_REGION),
            base: None,
            attr: None,
            sheet: None,
        });
    }
}

/// Most plan sheets looked at for several level views in one drawing (a guard on hostile sets).
const MAX_VIEW_SHEETS: usize = 400;

/// One plan view of a sheet that prints several plans of different levels.
#[derive(Clone, Debug, Default)]
pub(crate) struct SheetViewRead {
    pub title: String,
    /// The view's level code (`3`), and every level it shows when it is a typical plan of
    /// several (else empty).
    pub level: String,
    pub levels: Vec<String>,
    /// Its part of the sheet `[x0, y0, x1, y1]`, inside the sheet's drawing area.
    pub rect: [f64; 4],
}

/// For each sheet of `d` (`Some((rect, drawing area, levels))`; None: not looked at), its plan
/// views when it prints plans of different levels (`3RD FLOOR PLAN` beside `4TH FLOOR PLAN`),
/// lowest level first; empty for a sheet of one view. Read on the imported sheet
/// ([`cadaddy_mepfix::sheet_reader::level_views`]), the drawing gone over once. `levels`: the
/// levels the set says a sheet shows that is no typical plan (else empty); when its views carry
/// no title the readers find, its blocks of linework in the drawing area split it, one per level
/// ([`cadaddy_mepfix::sheet_reader::level_blocks`]).
pub(crate) fn sheet_views(d: &Drawing, sheets: &[Option<([f64; 4], [f64; 4], Vec<String>)>]) -> Vec<Vec<SheetViewRead>> {
    let mut out: Vec<Vec<SheetViewRead>> = vec![Vec::new(); sheets.len()];
    let looked: Vec<(usize, [f64; 4], [f64; 4], &[String])> = sheets
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.as_ref().map(|(r, a, l)| (i, *r, *a, l.as_slice())))
        .filter(|(_, r, a, _)| r.iter().chain(a.iter()).all(|v| v.is_finite()))
        .take(MAX_VIEW_SHEETS)
        .collect();
    if looked.is_empty() {
        return out;
    }
    let inside = |r: &[f64; 4], x: f64, y: f64| x >= r[0].min(r[2]) && x <= r[0].max(r[2]) && y >= r[1].min(r[3]) && y <= r[1].max(r[3]);
    // Each object to the sheet its middle is on.
    let mut per: Vec<Vec<cadaddy_doc::Entity>> = vec![Vec::new(); looked.len()];
    for e in d.model.iter() {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() {
            continue;
        }
        let c = b.center();
        if let Some(k) = looked.iter().position(|(_, r, _, _)| inside(r, c.x, c.y))
            && let Some(v) = per.get_mut(k)
        {
            v.push((**e).clone());
        }
    }
    let mut shell = d.clone();
    shell.model = EntityStore::new();
    let rank = |l: &str| (cadaddy_walk3d::levels::code_rank(l).unwrap_or(i32::MAX - 1), l.to_string());
    for ((i, _, area, levels), ents) in looked.iter().zip(per) {
        if ents.is_empty() {
            continue;
        }
        let a = [area[0].min(area[2]), area[1].min(area[3]), area[0].max(area[2]), area[1].max(area[3])];
        let mut sub = shell.clone();
        sub.model.push_many(ents);
        let mut found = cadaddy_mepfix::sheet_reader::level_views(&sub);
        if found.is_empty() && levels.len() > 1 {
            // Untitled views: the blocks of linework in the drawing area (not the title block's).
            let mut area_only = shell.clone();
            area_only.model.push_many(
                sub.model
                    .iter()
                    .filter(|e| {
                        let c = cadaddy_doc::entity_bounds(&sub, e, 0).center();
                        inside(&a, c.x, c.y)
                    })
                    .map(|e| (**e).clone())
                    .collect::<Vec<_>>(),
            );
            found = cadaddy_mepfix::sheet_reader::level_blocks(&area_only, levels);
        }
        let mut views: Vec<SheetViewRead> = found
            .into_iter()
            .filter_map(|(v, cell)| {
                let level = v.levels.first()?.clone();
                // Cells are open at the sheet's edges: a view ends at the sheet's drawing area.
                let rect = [cell[0].max(a[0]), cell[1].max(a[1]), cell[2].min(a[2]), cell[3].min(a[3])];
                (rect.iter().all(|x| x.is_finite()) && rect[2] > rect[0] && rect[3] > rect[1]).then(|| SheetViewRead {
                    title: v.title,
                    level,
                    levels: if v.levels.len() > 1 { v.levels } else { Vec::new() },
                    rect,
                })
            })
            .collect();
        views.sort_by_key(|v| rank(&v.level));
        // One view left (the others fell off the sheet): the sheet is read whole.
        if views.len() > 1
            && let Some(slot) = out.get_mut(*i)
        {
            *slot = views;
        }
    }
    out
}

/// The sheets of a print-set project, imported side by side into a scratch session: the plan
/// sheets of `trade` (both when none). Their regions and handle spans, and the session.
pub(crate) fn load_project_sheets(
    project: &cadaddy_mepfix::printset::PrintSetProject,
    only: Option<MepKind>,
) -> crate::Result<(Session, Vec<SheetRef>)> {
    load_project_plans(project, &|r| r.trade().is_some_and(|t| only.is_none_or(|o| o == t)))
}

/// The sheets of a print-set project `keep` takes, imported side by side into a scratch
/// session: their regions and handle spans, and the session.
pub(crate) fn load_project_plans(
    project: &cadaddy_mepfix::printset::PrintSetProject,
    keep: &dyn Fn(&SheetRef) -> bool,
) -> crate::Result<(Session, Vec<SheetRef>)> {
    let mut wanted: Vec<SheetRef> = Vec::new();
    let mut pages: Vec<super::exchange::PdfPage> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // Sheets titled as typical plans: one plan for every level they show, never split.
    let typicals: std::collections::HashSet<&String> =
        project.sheets.iter().filter(|s| cadaddy_mepfix::sheet_reader::says_typical(&s.title)).map(|s| &s.number).collect();
    for sh in project.sheets.iter().filter(|s| s.superseded_by.is_empty()) {
        let r = SheetRef {
            number: sh.number.clone(),
            level: sh.level.clone(),
            discipline: sh.discipline.clone(),
            kind: sh.sheet_type.clone(),
            rect: [0.0; 4],
            span: None,
            scale_text: sh.scale_text.clone(),
            levels: sh.levels.clone(),
            view: None,
        };
        if !keep(&r) || sh.file.is_empty() || !seen.insert((sh.file.clone(), sh.page)) || pages.len() >= MAX_SHEETS {
            continue;
        }
        let path = if std::path::Path::new(&sh.file).is_absolute() { sh.file.clone() } else { format!("{}/{}", project.source_dir, sh.file) };
        pages.push(super::exchange::PdfPage { path, params: json!({"page": sh.page.max(1), "images": false, "fills": "merge"}), at: None });
        wanted.push(r);
    }
    let mut t = Session::new();
    if pages.is_empty() {
        return Ok((t, Vec::new()));
    }
    let mut spans: Vec<Option<(u64, u64)>> = vec![None; pages.len()];
    let mut last = t.doc().map_or(0, |d| d.handseed);
    let mut on_page = |s: &mut Session, i: usize, r: &super::exchange::PageImport| {
        let seed = s.doc().map_or(0, |d| d.handseed);
        if matches!(r, super::exchange::PageImport::Done(_))
            && seed > last
            && let Some(slot) = spans.get_mut(i)
        {
            *slot = Some((last, seed));
        }
        last = seed;
    };
    let results = super::exchange::import_pdf_pages(&mut t, &pages, true, true, &mut on_page)?;
    let mut out = Vec::new();
    for (i, (r, mut sh)) in results.iter().zip(wanted).enumerate() {
        let super::exchange::PageImport::Done(rep) = r else { continue };
        let rect =
            rep.get("rect").and_then(Value::as_array).and_then(|v| <[f64; 4]>::try_from(v.iter().filter_map(Value::as_f64).collect::<Vec<_>>()).ok());
        let Some(rect) = rect else { continue };
        sh.rect = rect;
        sh.span = spans.get(i).copied().flatten();
        out.push(sh);
    }
    // A sheet printing two levels' plans: one sheet per view, each read on its own level.
    let sheets: Vec<Option<([f64; 4], [f64; 4], Vec<String>)>> =
        out.iter().map(|r| r.is_plan().then(|| (r.rect, r.rect, if typicals.contains(&r.number) { Vec::new() } else { r.levels.clone() }))).collect();
    let views = sheet_views(t.doc()?, &sheets);
    let mut split = Vec::with_capacity(out.len());
    for (r, vs) in out.into_iter().zip(views) {
        if vs.is_empty() {
            split.push(r);
            continue;
        }
        // Each view its own sheet, named as classify names one: `<number>/<level>`.
        let base = r.number.split('/').next().unwrap_or(&r.number).to_string();
        for v in vs {
            split.push(SheetRef { number: format!("{base}/{}", v.level), level: v.level, levels: v.levels, view: Some(v.rect), ..r.clone() });
        }
    }
    Ok((t, split))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_walk3d::MepSystem;

    fn sheet(number: &str, discipline: &str, level: &str, x: f64) -> SheetRef {
        SheetRef {
            number: number.into(),
            level: level.into(),
            discipline: discipline.into(),
            kind: "plan".into(),
            rect: [x, 0.0, x + 100.0, 80.0],
            span: None,
            scale_text: None,
            levels: Vec::new(),
            view: None,
        }
    }

    fn run(sheet: &str, level: &str, points: Vec<[f64; 2]>) -> MepRun {
        MepRun {
            system: "CW".into(),
            level: level.into(),
            points,
            size_in: [1.0, 0.0],
            bottoms_ft: vec![10.0],
            objects: Vec::new(),
            id: String::new(),
            encased: false,
            size_source: "tag".into(),
            height_source: "default".into(),
            reviewed: false,
            edited_by: String::new(),
            top_ft: None,
            source_sheet: sheet.into(),
        }
    }

    fn pipe_system() -> MepSystem {
        MepSystem {
            name: "CW".into(),
            layer: "P-CW".into(),
            kind: MepKind::Pipe,
            size_in: [1.0, 0.0],
            bottom_ft: 10.0,
            show: true,
            duct_type: None,
            color_index: 5,
        }
    }

    #[test]
    fn only_mechanical_and_plumbing_plans_are_read() {
        assert_eq!(sheet("M-101", "M", "1", 0.0).trade(), Some(MepKind::Duct));
        assert_eq!(sheet("P-101", "P", "1", 0.0).trade(), Some(MepKind::Pipe));
        for (n, d) in [("E-101", "E"), ("A-101", "A"), ("FP-101", "FP"), ("S-101", "S")] {
            assert_eq!(sheet(n, d, "1", 0.0).trade(), None, "{n}");
        }
        let mut notes = sheet("M-001", "M", "", 0.0);
        notes.kind = "schedule".into();
        assert_eq!(notes.trade(), None, "a schedule sheet is not a plan");
    }

    #[test]
    fn a_plan_of_no_level_or_on_the_site_reads_nothing() {
        assert_eq!(sheet("P-201", "P", "", 0.0).trade(), None, "level not known yet: a reference sheet");
        assert_eq!(sheet("P-001", "P", "SITE", 0.0).trade(), None, "the site is a datum");
        assert!(!sheet("E-001", "E", "SITE", 0.0).builds());
        assert!(sheet("E-101", "E", "1", 0.0).builds());
    }

    #[test]
    fn nothing_read_on_the_site_becomes_a_storey() {
        let lvl = |name: &str, e: f64| Level {
            name: name.into(),
            elevation_ft: e,
            height_ft: 12.0,
            ceiling_ft: 9.0,
            region: None,
            base: None,
            attr: None,
            sheet: None,
        };
        let mut sp = BuildingSpec { levels: vec![lvl("Level 1", 0.0), lvl("Level 2", 12.0)], ..BuildingSpec::default() };
        let mep = Mep {
            systems: vec![pipe_system()],
            runs: vec![run("P-001", "SITE", vec![[0.0, 0.0], [120.0, 0.0]]), run("P-102", "Level 2", vec![[0.0, 0.0], [240.0, 0.0]])],
            ..Mep::default()
        };
        merge_mep(&mut sp, mep, &["P-001".into(), "P-102".into()]);
        assert_eq!(sp.levels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Level 1", "Level 2"], "no SITE storey");
        assert_eq!(sp.mep.runs.iter().map(|r| r.level.as_str()).collect::<Vec<_>>(), ["Level 2"], "the site's run is no building's");
    }

    #[test]
    fn level_codes_name_the_spec_levels() {
        assert_eq!(level_name("2"), "Level 2");
        assert_eq!(level_name("R"), "Roof");
        assert_eq!(level_name(""), "Level 1");
        assert_eq!(level_name("Mezzanine"), "Mezzanine");
    }

    #[test]
    fn a_sheets_reading_goes_onto_its_levels_floor_plan() {
        let all = vec![sheet("A-101", "A", "1", 0.0), sheet("A-102", "A", "2", 200.0), sheet("P-102", "P", "2", 600.0)];
        let p = placement(&all[2], &all, 1.0);
        assert_eq!(p.apply([610.0, 5.0]), [210.0, 5.0]);
        // A sheet of another size stays where it is (no common placement to register by).
        let mut big = sheet("P-103", "P", "2", 900.0);
        big.rect = [900.0, 0.0, 1100.0, 160.0];
        assert_eq!(placement(&big, &all, 1.0).apply([910.0, 5.0]), [910.0, 5.0]);
        // Scaled up from paper size about the sheet's corner.
        let mut paper = sheet("P-104", "P", "3", 900.0);
        paper.rect = [900.0, 0.0, 901.0, 0.8];
        assert_eq!(placement(&paper, &all, 96.0).apply([900.5, 0.5]), [948.0, 48.0]);
    }

    #[test]
    fn a_reread_replaces_what_the_sheet_read_and_levels_are_added() {
        let mut sp = BuildingSpec {
            levels: vec![Level {
                name: "Level 1".into(),
                elevation_ft: 0.0,
                height_ft: 12.0,
                ceiling_ft: 9.0,
                region: None,
                base: None,
                attr: None,
                sheet: None,
            }],
            mep: Mep::default(),
            ..BuildingSpec::default()
        };
        let read = || Mep {
            systems: vec![pipe_system()],
            runs: vec![run("P-101", "Level 1", vec![[0.0, 0.0], [120.0, 0.0]]), run("P-102", "Level 2", vec![[0.0, 0.0], [240.0, 0.0]])],
            ..Mep::default()
        };
        merge_mep(&mut sp, read(), &["P-101".into(), "P-102".into()]);
        merge_mep(&mut sp, read(), &["P-101".into(), "P-102".into()]);
        assert_eq!(sp.mep.runs.len(), 2, "read twice, counted once");
        assert_eq!(sp.mep.systems.len(), 1);
        assert!(sp.mep.runs.iter().all(|r| !r.id.is_empty()));
        let two = sp.level("Level 2").expect("the plumbing plan's level is added");
        assert_eq!(two.elevation_ft, 12.0, "stacked on the top level");
        assert_eq!(two.region, Some(NO_PLAN_REGION), "it takes no plan objects");
        assert!(sp.check().iter().all(|i| !(i.error && i.path.contains("region"))), "{:?}", sp.check());
    }

    #[test]
    fn a_sheets_objects_are_cut_out_by_their_handles() {
        let mut d = Drawing::default();
        for i in 0..6 {
            let x = f64::from(i) * 10.0;
            let line = cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(x, 0.0, 0.0), b: cadaddy_geom::Vec3::new(x + 5.0, 0.0, 0.0) };
            d.add(&Space::Model, cadaddy_doc::Common::default(), cadaddy_doc::EntityKind::Line(line)).expect("add");
        }
        let hs: Vec<u64> = d.model.iter().map(|e| e.handle.0).collect();
        let (Some(&lo), Some(&mid)) = (hs.first(), hs.get(3)) else { panic!("handles") };
        let mut sh = sheet("M-101", "M", "1", 0.0);
        sh.span = Some((lo, mid));
        assert_eq!(sheet_drawing(&d, &sh).model.len(), 3);
        // No handles: what lies in the rectangle.
        sh.span = None;
        sh.rect = [0.0, -1.0, 26.0, 1.0];
        assert_eq!(sheet_drawing(&d, &sh).model.len(), 3);
    }

    #[test]
    fn a_notes_sheet_reads_no_ducts() {
        let mut s = Session::new();
        // A duct's two sides with its size tag, on a sheet that says DO NOT SCALE.
        for (a, b) in [([0.0, 0.0], [600.0, 0.0]), ([0.0, 12.0], [600.0, 12.0])] {
            s.execute("line", &json!({"points": [a, b]})).expect("line");
        }
        s.execute("text", &json!({"at": [300, 20], "text": "12x12 SA", "height": 4})).expect("tag");
        let d = s.doc().expect("doc").clone();
        let sh = SheetRef { rect: [-10.0, -10.0, 700.0, 100.0], ..sheet("M-001", "M", "1", 0.0) };
        let (_, plain) = read_sheet(&d, &sh, std::slice::from_ref(&sh), MepKind::Duct, 12.0);
        s.execute("text", &json!({"at": [300, 80], "text": "DO NOT SCALE DRAWINGS", "height": 4})).expect("note");
        let d = s.doc().expect("doc").clone();
        let (mep, notes) = read_sheet(&d, &sh, std::slice::from_ref(&sh), MepKind::Duct, 12.0);
        assert!(plain.runs > 0, "the same sheet without the note reads its duct: {plain:?}");
        assert!(mep.runs.is_empty(), "{notes:?}");
        assert!(notes.warnings.iter().any(|w| w.contains("notes sheet")), "{notes:?}");
    }

    #[test]
    fn a_plumbing_plans_invert_notes_by_its_drains_are_read_and_merged_once() {
        let mut s = Session::new();
        // A sanitary line tagged with the legend's letter, an invert by its end, one far off a
        // drain, and the legend's row naming the abbreviation.
        s.execute("line", &json!({"points": [[0, 0], [600, 0]]})).expect("line");
        s.execute("text", &json!({"at": [300, 6], "text": "6\" S", "height": 4})).expect("tag");
        s.execute("text", &json!({"at": [12, 20], "text": "24\" INVERT", "height": 4})).expect("invert");
        s.execute("text", &json!({"at": [300, 400], "text": "48\" INVERT", "height": 4})).expect("far");
        s.execute("text", &json!({"at": [500, 300], "text": "IE INVERT ELEVATION", "height": 4})).expect("legend");
        // A note whose insertion point (its left end) is past reach but whose text ends by the
        // drain; one that reaches its drain by a leader line only.
        s.execute("text", &json!({"at": [-90, 10], "text": "36\" INVERT", "height": 4})).expect("long");
        s.execute("text", &json!({"at": [400, 100], "text": "30\" INVERT", "height": 4})).expect("leadered");
        s.execute("line", &json!({"points": [[398, 100], [410, 60]]})).expect("leader");
        let d = s.doc().expect("doc").clone();
        let sh = SheetRef { rect: [-100.0, -10.0, 700.0, 500.0], ..sheet("P-101", "P", "1", 0.0) };
        let (mep, read) = read_sheet(&d, &sh, std::slice::from_ref(&sh), MepKind::Pipe, 12.0);
        assert!(mep.runs.iter().any(|r| r.system == "SAN"), "{read:?}");
        let got: Vec<(f64, &str)> = mep.inverts.iter().map(|v| (v.depth_ft, v.text.as_str())).collect();
        assert_eq!(got, vec![(2.0, "24\" INVERT"), (3.0, "36\" INVERT"), (2.5, "30\" INVERT")], "{:?}", mep.inverts);
        let v = &mep.inverts[0];
        assert_eq!((v.source_sheet.as_str(), v.level.as_str()), ("P-101", mep.runs[0].level.as_str()));
        // The leadered note marks its leader's end.
        assert!((mep.inverts[2].at[0] - 410.0).abs() < 1e-6 && (mep.inverts[2].at[1] - 60.0).abs() < 1e-6, "{:?}", mep.inverts[2].at);
        // Into the spec once; a re-read of the sheet replaces its inverts.
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        merge_mep(&mut sp, mep.clone(), &["P-101".to_string()]);
        merge_mep(&mut sp, mep.clone(), &["P-101".to_string()]);
        assert_eq!(sp.mep.inverts.len(), 3);
        // A duct read takes none.
        let (ducts, _) = read_sheet(&d, &sh, std::slice::from_ref(&sh), MepKind::Duct, 12.0);
        assert!(ducts.inverts.is_empty());
    }

    /// A duct's two sides with its size tag, and a title block titled `title`.
    fn titled_sheet(title: &str) -> Drawing {
        let mut s = Session::new();
        for (a, b) in [([0.0, 0.0], [600.0, 0.0]), ([0.0, 12.0], [600.0, 12.0])] {
            s.execute("line", &json!({"points": [a, b]})).expect("line");
        }
        s.execute("text", &json!({"at": [300, 20], "text": "12x12 SA", "height": 4})).expect("tag");
        s.execute("text", &json!({"at": [620, 80], "text": "DRAWING TITLE", "height": 3})).expect("label");
        s.execute("text", &json!({"at": [620, 75], "text": title, "height": 3})).expect("title");
        s.execute("text", &json!({"at": [620, 60], "text": "M-5", "height": 6})).expect("number");
        s.doc().expect("doc").clone()
    }

    #[test]
    fn a_details_sheet_taken_for_a_plan_reads_no_ducts() {
        // Classify had no entry for the sheet (so it counts as a plan on level 1), but its
        // own title block says details and schedules.
        let sh = SheetRef { rect: [-10.0, -10.0, 700.0, 100.0], ..sheet("M-5", "M", "1", 0.0) };
        let (mep, read) = read_sheet(&titled_sheet("MECHANICAL DETAILS AND SCHEDULES"), &sh, std::slice::from_ref(&sh), MepKind::Duct, 12.0);
        assert!(mep.runs.is_empty(), "{read:?}");
        assert!(read.warnings.iter().any(|w| w.contains("schedule sheet, not a plan")), "{read:?}");
        let (mep, read) = read_sheet(&titled_sheet("MECHANICAL PLAN - LEVEL 1"), &sh, std::slice::from_ref(&sh), MepKind::Duct, 12.0);
        assert!(!mep.runs.is_empty(), "a plan's title reads its ducts: {read:?}");
        let (_, read) = read_sheet(&titled_sheet("PLUMBING RISER DIAGRAMS"), &sh, std::slice::from_ref(&sh), MepKind::Pipe, 12.0);
        assert!(read.warnings.iter().any(|w| w.contains("diagram sheet") && w.contains("no pipes")), "{read:?}");
    }

    #[test]
    fn reference_titles() {
        assert_eq!(reference_title("MECHANICAL DETAILS"), Some("detail"));
        assert_eq!(reference_title("HVAC SCHEDULES"), Some("schedule"));
        assert_eq!(reference_title("ENLARGED MECHANICAL PLANS AND DETAILS"), None);
        assert_eq!(reference_title("SECOND FLOOR HVAC"), None);
    }
}
