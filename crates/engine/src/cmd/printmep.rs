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
}

impl SheetRef {
    fn is_plan(&self) -> bool {
        matches!(self.kind.as_str(), "plan" | "enlarged_plan")
    }
    /// The trade read on this sheet: ducts on mechanical plans, pipes on plumbing plans.
    pub fn trade(&self) -> Option<MepKind> {
        if !self.is_plan() {
            return None;
        }
        match self.discipline.to_ascii_uppercase().as_str() {
            "M" => Some(MepKind::Duct),
            "P" => Some(MepKind::Pipe),
            _ => None,
        }
    }
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
        .filter(|e| match sh.span {
            Some((lo, hi)) => e.handle.0 >= lo && e.handle.0 < hi,
            None => {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                let (cx, cy) = ((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
                cx.is_finite() && cy.is_finite() && cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1
            }
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
fn placement(sh: &SheetRef, all: &[SheetRef], k: f64) -> Placement {
    let size = |r: &[f64; 4]| (r[2] - r[0], r[3] - r[1]);
    let (w, h) = size(&sh.rect);
    let arch = all.iter().find(|a| {
        let (aw, ah) = size(&a.rect);
        a.discipline.eq_ignore_ascii_case("A")
            && a.kind == "plan"
            && sh.kind == "plan"
            && !sh.level.is_empty()
            && a.level.eq_ignore_ascii_case(&sh.level)
            && w > 0.0
            && h > 0.0
            && ((aw - w * k) / (w * k)).abs() < 0.02
            && ((ah - h * k) / (h * k)).abs() < 0.02
    });
    let from = [sh.rect[0], sh.rect[1]];
    Placement { from, to: arch.map_or(from, |a| [a.rect[0], a.rect[1]]), k }
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
    let mut mep = match trade {
        MepKind::Duct => {
            let plain = cadaddy_mepfix::duct_reader::read_ducts(&sub, &cadaddy_mepfix::truth::Sheet::default());
            let net = if plain.runs.is_empty() { cadaddy_mepfix::duct_reader::read_ducts_any_layer(&sub) } else { plain };
            super::readers::duct_mep(&net, &level, &sh.number, &spec)
        }
        _ => {
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
    for e in &mut mep.equipment {
        e.min = at.apply(e.min);
        e.max = at.apply(e.max);
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
        if !n.is_empty() && sp.level(n).is_none() && !wanted.iter().any(|w| w.eq_ignore_ascii_case(n)) {
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
    Ok((t, out))
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
}
