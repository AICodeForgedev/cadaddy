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
    /// Lined up on its level's architectural plan (its background then removed before reading).
    pub registered: bool,
    /// Runs read that still lay on architectural lines: dropped.
    pub dropped_on_walls: usize,
    /// Where it was lined up and what was removed ([`super::register::Registration::to_json`]).
    pub registration: Option<Value>,
}

impl SheetRead {
    pub fn to_json(&self) -> Value {
        json!({
            "sheet": self.sheet, "level": self.level, "trade": self.trade, "runs": self.runs,
            "length_ft": r1(self.length_ft), "terminals": self.terminals,
            "equipment": self.equipment, "warnings": self.warnings,
            "registered": self.registered, "dropped_on_walls": self.dropped_on_walls,
            "registration": self.registration,
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
    #[cfg(test)]
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

/// Where a sheet's placement puts it on its level's architectural floor plan (same sheet size,
/// scaled up from paper size), as a transform; none when no A plan of the level is its size.
pub(crate) fn sheet_placement(d: &Drawing, sh: &SheetRef, all: &[SheetRef], upf: f64) -> Option<super::register::Similarity> {
    let mut b = cadaddy_geom::Bounds2::EMPTY;
    for e in super::register::sheet_objects(d, sh) {
        b = b.union(&cadaddy_doc::entity_bounds(d, e, 0));
    }
    let diag = if b.is_empty() { 0.0 } else { (b.max.x - b.min.x).hypot(b.max.y - b.min.y) };
    let k = if diag.is_finite() && diag > 1.0 && diag < 100.0 {
        let printed =
            sh.scale_text.as_deref().and_then(cadaddy_mepfix::sheet_reader::parse_scale).filter(|k| k.is_finite() && *k > 0.0 && *k <= 12.0);
        upf / printed.unwrap_or(0.125)
    } else {
        1.0
    };
    let at = placement(sh, all, k);
    (at.to != at.from || (k - 1.0).abs() > 1e-12).then(|| super::register::Similarity::place(at.from, at.to, at.k))
}

/// How much of a run's path lies on the architectural plan's lines, by length.
fn on_arch_share(points: &[[f64; 2]], arch: &super::register::ArchPlan) -> f64 {
    let (mut on, mut all) = (0.0, 0.0);
    for w in points.windows(2) {
        if let [a, b] = w {
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            if len.is_finite() && len > 0.0 {
                all += len;
                on += len * arch.on_lines(*a, *b);
            }
        }
    }
    if all > 0.0 { on / all } else { 0.0 }
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

/// Read one sheet's trade into spec parts in `d`'s units (`upf` per foot). Registered (`reg`) on
/// its level's architectural plan `arch`, the background is removed first, the sheet is read at
/// the registration's scale and moved by it, and the runs still on the plan's lines are dropped;
/// else it is placed by its sheet position, with a warning.
fn read_sheet(
    d: &Drawing,
    sh: &SheetRef,
    all: &[SheetRef],
    trade: MepKind,
    upf: f64,
    reg: Option<&super::register::Registration>,
    arch: Option<&super::register::ArchPlan>,
) -> (Mep, SheetRead) {
    let level = level_name(&sh.level);
    let mut read = SheetRead {
        sheet: sh.number.clone(),
        level: level.clone(),
        trade: if trade == MepKind::Duct { "duct" } else { "pipe" },
        ..SheetRead::default()
    };
    let mut sub = sheet_drawing(d, sh);
    if sub.model.is_empty() {
        read.warnings.push(format!("{}: no objects imported for this sheet", sh.number));
        return (Mep::default(), read);
    }
    let paper = paper_units_per_foot(&sub, sh);
    let reg = reg.filter(|r| r.registered);
    read.registration = reg.map(super::register::Registration::to_json);
    read.registered = reg.is_some();
    if let Some(r) = reg {
        for h in &r.background {
            sub.model.remove(*h);
        }
    }
    // Registered: read at the scale it was lined up at.
    let sheet_upf = match reg {
        Some(r) if r.transform.scale() > 0.0 => upf / r.transform.scale(),
        _ => paper.unwrap_or(upf),
    };
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
    let at = match reg {
        Some(r) => r.transform,
        None => {
            let k = if paper.is_some() { upf / sheet_upf } else { 1.0 };
            let p = placement(sh, all, k);
            read.warnings.push(format!(
                "{} ({}): not lined up on an architectural plan of its level; placed by its sheet position{}",
                sh.number,
                level,
                if p.to == p.from { " (where it is)" } else { "" }
            ));
            super::register::Similarity::place(p.from, p.to, p.k)
        }
    };
    if paper.is_some() {
        read.warnings.push(format!("{}: at paper size; read at {} drawing units per foot", sh.number, sheet_upf));
    }
    for r in &mut mep.runs {
        for p in &mut r.points {
            *p = at.apply(*p);
        }
    }
    for t in &mut mep.terminals {
        t.at = at.apply(t.at);
    }
    for e in &mut mep.equipment {
        (e.min, e.max) = at.apply_box(e.min, e.max);
    }
    // A run still lying on the architectural plan's lines is the building, not the trade.
    if let (Some(_), Some(arch)) = (reg, arch) {
        let before = mep.runs.len();
        mep.runs.retain(|r| on_arch_share(&r.points, arch) < 0.8);
        read.dropped_on_walls = before - mep.runs.len();
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
    let mut plans = std::collections::HashMap::new();
    for sh in mine {
        let Some(trade) = sh.trade() else { continue };
        let reg = super::register::register_with(d, sh, sheets, upf, &mut plans);
        let arch = reg.arch_sheet.as_ref().and_then(|n| plans.get(n));
        let (mep, read) = read_sheet(d, sh, sheets, trade, upf, Some(&reg), arch);
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
                registered: read.registered,
                dropped_on_walls: read.dropped_on_walls,
                registration: read.registration.clone(),
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
    // Nothing is built on the site datum (a site plan's utilities are no building's MEP).
    sp.mep.runs.retain(|r| r.level.trim().is_empty() || building_level(&r.level));
    sp.mep.terminals.retain(|t| t.level.trim().is_empty() || building_level(&t.level));
    sp.mep.equipment.retain(|e| e.level.trim().is_empty() || building_level(&e.level));
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
        let (_, plain) = read_sheet(&d, &sh, std::slice::from_ref(&sh), MepKind::Duct, 12.0, None, None);
        s.execute("text", &json!({"at": [300, 80], "text": "DO NOT SCALE DRAWINGS", "height": 4})).expect("note");
        let d = s.doc().expect("doc").clone();
        let (mep, notes) = read_sheet(&d, &sh, std::slice::from_ref(&sh), MepKind::Duct, 12.0, None, None);
        assert!(plain.runs > 0, "the same sheet without the note reads its duct: {plain:?}");
        assert!(mep.runs.is_empty(), "{notes:?}");
        assert!(notes.warnings.iter().any(|w| w.contains("notes sheet")), "{notes:?}");
    }

    /// A made-up plan in `tf`'s frame: a row of rooms' wall faces (inches), grid lines and
    /// their labelled bubbles, on `layer`.
    fn background(d: &mut Drawing, tf: &super::super::register::Similarity, layer: &str) {
        let w = |d: &mut Drawing, a: [f64; 2], b: [f64; 2]| add_line(d, layer, tf.apply(a), tf.apply(b));
        for x in [0.0, 240.0, 480.0, 720.0, 960.0] {
            w(d, [x, 0.0], [x, 600.0]);
            w(d, [x + 6.0, 0.0], [x + 6.0, 600.0]);
        }
        for y in [0.0, 600.0] {
            w(d, [0.0, y], [966.0, y]);
            w(d, [0.0, y + 6.0], [966.0, y + 6.0]);
        }
        for (i, x) in [0.0, 480.0, 960.0].iter().enumerate() {
            w(d, [*x + 3.0, -120.0], [*x + 3.0, 720.0]);
            add_text(d, &format!("{}", i + 1), tf.apply([*x + 3.0, -150.0]), 6.0 * tf.scale());
        }
        for (i, y) in [0.0, 600.0].iter().enumerate() {
            w(d, [-120.0, *y + 3.0], [1080.0, *y + 3.0]);
            add_text(d, ["A", "B"].get(i).copied().unwrap_or("C"), tf.apply([-150.0, *y + 3.0]), 6.0 * tf.scale());
        }
        // Room tags.
        for (i, x) in [120.0, 360.0, 600.0, 840.0].iter().enumerate() {
            add_text(d, &format!("10{}", i + 1), tf.apply([*x, 560.0]), 6.0 * tf.scale());
        }
    }

    fn add_line(d: &mut Drawing, layer: &str, a: [f64; 2], b: [f64; 2]) -> cadaddy_doc::Handle {
        let line = cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(a[0], a[1], 0.0), b: cadaddy_geom::Vec3::new(b[0], b[1], 0.0) };
        d.add(&Space::Model, cadaddy_doc::Common { layer: layer.into(), ..cadaddy_doc::Common::default() }, cadaddy_doc::EntityKind::Line(line))
            .expect("line")
    }

    fn add_text(d: &mut Drawing, s: &str, at: [f64; 2], h: f64) {
        let t = cadaddy_doc::Text {
            insert: cadaddy_geom::Vec3::new(at[0], at[1], 0.0),
            align_pt: None,
            height: h,
            value: s.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: cadaddy_doc::HAlign::Left,
            valign: cadaddy_doc::VAlign::Baseline,
        };
        d.add(&Space::Model, cadaddy_doc::Common { layer: "TEXT".into(), ..cadaddy_doc::Common::default() }, cadaddy_doc::EntityKind::Text(t))
            .expect("text");
    }

    fn border(d: &mut Drawing, r: [f64; 4]) {
        let [x0, y0, x1, y1] = r;
        for (a, b) in [([x0, y0], [x1, y0]), ([x1, y0], [x1, y1]), ([x1, y1], [x0, y1]), ([x0, y1], [x0, y0])] {
            add_line(d, "BORDER", a, b);
        }
    }

    fn placed(number: &str, discipline: &str, rect: [f64; 4], span: (u64, u64)) -> SheetRef {
        SheetRef {
            number: number.into(),
            level: "1".into(),
            discipline: discipline.into(),
            kind: "plan".into(),
            rect,
            span: Some(span),
            scale_text: None,
            levels: Vec::new(),
        }
    }

    /// The rectangle round `tf`'s plan, with a margin.
    fn rect_of(tf: &super::super::register::Similarity) -> [f64; 4] {
        let (lo, hi) = tf.apply_box([-300.0, -300.0], [1_260.0, 900.0]);
        [lo[0], lo[1], hi[0], hi[1]]
    }

    #[test]
    fn trade_plans_at_another_scale_and_turn_line_up_and_read_only_their_trade() {
        use super::super::register::Similarity;
        let mut d = Drawing::default();
        let lo = d.handseed;
        let id = Similarity::identity();
        background(&mut d, &id, "A-WALL");
        border(&mut d, rect_of(&id));
        let a_hi = d.handseed;
        // The mechanical plan: half scale, turned a quarter, far right; two tagged ducts.
        let m = Similarity { m: [0.0, 0.5], t: [6_000.0, 100.0] };
        background(&mut d, &m, "M-BKGD");
        border(&mut d, rect_of(&m));
        for (a, b, off, tag, at) in [
            ([60.0, 300.0], [420.0, 300.0], [0.0, 6.0], "12x12 SA", [240.0, 316.0]),
            ([600.0, 120.0], [600.0, 480.0], [6.0, 0.0], "12x12 SA", [616.0, 300.0]),
        ] {
            for k in [-1.0, 1.0] {
                let (p, q) = ([a[0] + off[0] * k, a[1] + off[1] * k], [b[0] + off[0] * k, b[1] + off[1] * k]);
                add_line(&mut d, "0", m.apply(p), m.apply(q));
            }
            add_text(&mut d, tag, m.apply(at), 3.0);
        }
        let m_hi = d.handseed;
        // The plumbing plan: twice the scale, turned half round; two pipe runs.
        let pl = Similarity { m: [-2.0, 0.0], t: [14_000.0, 1_400.0] };
        background(&mut d, &pl, "P-BKGD");
        border(&mut d, rect_of(&pl));
        for (a, b) in [([100.0, 450.0], [860.0, 450.0]), ([300.0, 100.0], [300.0, 560.0])] {
            add_line(&mut d, "P-CW", pl.apply(a), pl.apply(b));
        }
        let p_hi = d.handseed;
        let sheets = vec![
            placed("A-101", "A", rect_of(&id), (lo, a_hi)),
            placed("M-101", "M", rect_of(&m), (a_hi, m_hi)),
            placed("P-101", "P", rect_of(&pl), (m_hi, p_hi)),
        ];
        let read = read_sheets(&d, &sheets, 12.0, None);
        for r in &read.sheets {
            assert!(r.registered, "{} registered: {:?}", r.sheet, r.registration);
            let reg = r.registration.clone().unwrap_or_default();
            assert!(reg["matched_pct"].as_f64().unwrap_or(0.0) > 0.95, "{reg}");
            assert_eq!(r.dropped_on_walls, 0, "no run lies on a wall: {r:?}");
        }
        // Exact counts, lengths within 5% (60 ft of duct, 101.7 ft of pipe), on the A plan.
        assert_eq!(read.runs("duct"), 2, "{:?}", read.to_json());
        assert_eq!(read.runs("pipe"), 2, "{:?}", read.to_json());
        let (duct, pipe) = (read.length_ft("duct"), read.length_ft("pipe"));
        assert!((duct - 60.0).abs() <= 3.0, "duct {duct}");
        assert!((pipe - 101.67).abs() <= 5.1, "pipe {pipe}");
        for r in &read.mep.runs {
            for p in &r.points {
                assert!(p[0] > -1.0 && p[0] < 967.0 && p[1] > -1.0 && p[1] < 607.0, "{} point {p:?} on the A plan", r.system);
            }
        }
        // The background never reached a reader: the walls were not read as ducts or pipes.
        let plan = super::super::register::ArchPlan::new(&d, &sheets[0]);
        for r in &read.mep.runs {
            assert!(on_arch_share(&r.points, &plan) < 0.2, "{r:?}");
        }
    }

    #[test]
    fn a_trade_plan_without_the_background_is_not_registered_and_says_so() {
        use super::super::register::Similarity;
        let mut d = Drawing::default();
        let lo = d.handseed;
        let id = Similarity::identity();
        background(&mut d, &id, "A-WALL");
        let a_hi = d.handseed;
        let m = Similarity { m: [1.0, 0.0], t: [6_000.0, 0.0] };
        // Only the grid labels in common, and its ducts.
        for (i, x) in [0.0, 480.0, 960.0].iter().enumerate() {
            add_text(&mut d, &format!("{}", i + 1), m.apply([*x + 3.0, -150.0]), 6.0);
        }
        for k in [-1.0, 1.0] {
            add_line(&mut d, "0", m.apply([60.0, 300.0 + 6.0 * k]), m.apply([420.0, 300.0 + 6.0 * k]));
        }
        add_text(&mut d, "12x12 SA", m.apply([240.0, 316.0]), 6.0);
        let m_hi = d.handseed;
        let mut ms = placed("M-101", "M", [5_700.0, -300.0, 7_300.0, 900.0], (a_hi, m_hi));
        ms.level = "1".into();
        let sheets = vec![placed("A-101", "A", rect_of(&id), (lo, a_hi)), ms];
        let read = read_sheets(&d, &sheets, 12.0, None);
        let r = read.sheets.first().expect("M-101 read");
        assert!(!r.registered);
        assert!(r.warnings.iter().any(|w| w.contains("not lined up")), "{r:?}");
        assert!(r.registration.is_none());
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
        let (mep, read) =
            read_sheet(&titled_sheet("MECHANICAL DETAILS AND SCHEDULES"), &sh, std::slice::from_ref(&sh), MepKind::Duct, 12.0, None, None);
        assert!(mep.runs.is_empty(), "{read:?}");
        assert!(read.warnings.iter().any(|w| w.contains("schedule sheet, not a plan")), "{read:?}");
        let (mep, read) = read_sheet(&titled_sheet("MECHANICAL PLAN - LEVEL 1"), &sh, std::slice::from_ref(&sh), MepKind::Duct, 12.0, None, None);
        assert!(!mep.runs.is_empty(), "a plan's title reads its ducts: {read:?}");
        let (_, read) = read_sheet(&titled_sheet("PLUMBING RISER DIAGRAMS"), &sh, std::slice::from_ref(&sh), MepKind::Pipe, 12.0, None, None);
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
