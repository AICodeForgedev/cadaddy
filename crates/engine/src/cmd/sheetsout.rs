//! Construction drawings from the built model (Q6016): `sheetset.from_model` makes the drawing
//! set a built BIM prints as, each sheet an ordinary layout (editable before export) with the
//! title block and the views drawn on it to scale.
//!
//! - **Views from the model:** floor plans per level per discipline (architectural; electrical
//!   power, lighting and systems; mechanical; plumbing; fire protection), reflected ceiling
//!   plans, the roof plan, enlarged plans (wet rooms, the print set's enlarged-plan regions and
//!   any asked for), building sections at chosen cut lines (one each way through the building's
//!   middle by default), the four exterior elevations and a 3D view (hidden lines removed).
//! - **Annotation from the elements' properties:** door, room, wall type, window, device,
//!   fixture and equipment tags; wall-to-wall, opening, overall and grid-to-grid dimension
//!   strings; circuit numbers with the wiring and home runs; duct and pipe sizes; level markers;
//!   keynotes (fire ratings, GFCI, dampers, an element's own `keynote`); north arrow and scale bar.
//! - **Schedules as table objects** from `bim.schedule`: door, window, room finish, panel (one per
//!   panel), lighting fixture, equipment, plumbing fixture and air distribution; the takeoff
//!   summary on a sheet of its own when asked.
//! - **Sheets:** the drawing's title block (else a library one at the sheet size) filled from the
//!   project's properties; numbered by discipline (`A101`, `E201`, `M101`, `P101`, `FP101`); the
//!   cover's sheet index lists them all.
//! - **Export:** one multi-page PDF (or one per sheet) through `exportpdf`, vector, with a PDF
//!   layer per drawing layer and searchable text, at 1:1 on the sheet's paper size; the same
//!   sheets to DWG through `export`.
//!
//! The views are drawn straight on paper (a scale per view, the model to paper), so each sheet
//! shows exactly its discipline and level without viewport layer freezes; every entity carries
//! the `CADADDY_SHEETSOUT` tag, so building the set again replaces the sheets it made before.

mod model;
mod pen;
mod views;

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{RawTag, Space, TableStyle};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use self::model::{Disc, Item, bounds};
use self::pen::{Pen, Shape, ok};
use self::views::{Frame, GridLines, PlanCtx, PlanKind, Upright, fit_scale, list, put_upright, view_title};
use super::*;
use crate::{Result, Session};

const CMD: &str = "sheetset.from_model";
/// The tag on every entity the set draws.
pub const APP: &str = "CADADDY_SHEETSOUT";
/// Most levels with plans.
const MAX_LEVELS: usize = 50;
/// Most sheets in a set (a PDF holds at most 256 pages).
const MAX_SHEETS: usize = 240;
/// Most rows a schedule puts on sheets.
const MAX_SCHEDULE_ROWS: usize = 2_000;
/// Most sections and enlarged plans asked for.
const MAX_VIEWS: usize = 24;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Drawing Set from Model", run)
            .menu(&["Draw", "BIM", "Drawing Set from Model"])
            .params(
                "{disciplines?: [A|E|M|P|FP or architectural, electrical, mechanical, plumbing, fire] (default every one the model has), levels?: [name] (default all), scale?: \"1/8\\\"=1'-0\\\"\" | \"1:96\" | paper inches per foot (default the largest that fits), size?: ARCH C|ARCH D|ARCH E1|ARCH E|ANSI D|ANSI E (default ARCH D), sections?: [{from: [x,y], to: [x,y], name?}] (drawing units; default one each way through the middle), depth?: feet seen beyond a section (40), enlarged?: [{level, rect: [x0,y0,x1,y1], title?}] (added to the wet rooms and the print set's enlarged plans), schedules?: bool (true), views3d?: bool (elevations and the 3D view; true), takeoff?: bool (a takeoff summary sheet; false), project?: {name, number, firm, client, address, date} (default the drawing's properties), title_block?: block name (default the drawing's, else the library's at the sheet size), pdf?: path (one PDF of every sheet), split?: bool (one PDF per sheet, numbered), dwg?: path} → {sheets: [{number, title, layout, discipline, level?, view, scale?, counts}], index: [[number, title]], files, removed, notes}",
            ),
    ]
}

// ─── Paper ──────────────────────────────────────────────────────────────────

struct Paper {
    w: f64,
    h: f64,
    block: &'static str,
    /// Where the views go: inside the border, left of the title strip.
    area: [f64; 4],
}

fn paper(p: &Value) -> Result<Paper> {
    let name = str_param(p, "size").unwrap_or("ARCH D");
    let key: String = name.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase();
    let (w, h, block) = match key.as_str() {
        "ARCHC" | "C" => (24.0, 18.0, "G_TITLE_ARCH_C"),
        "ARCHD" | "D" | "" => (36.0, 24.0, "G_TITLE_ARCH_D"),
        "ARCHE1" | "E1" => (42.0, 30.0, "G_TITLE_ARCH_E1"),
        "ARCHE" | "E" => (48.0, 36.0, "G_TITLE_ARCH_E"),
        "ANSID" => (34.0, 22.0, "G_TITLE_ANSI_D"),
        "ANSIE" => (44.0, 34.0, "G_TITLE_ANSI_E"),
        _ => return Err(bad(CMD, format!("no sheet size `{name}`: ARCH C, ARCH D, ARCH E1, ARCH E, ANSI D or ANSI E"))),
    };
    // The library title block: a 1/2" border (1" at the binding edge) and a title strip 3" wide
    // (4" on sheets 40" and wider) down the right side.
    let strip = if w >= 40.0 { 4.0 } else { 3.0 };
    Ok(Paper { w, h, block, area: [1.25, 0.75, w - 0.5 - strip - 0.25, h - 0.75] })
}

/// Paper inches per foot from `scale`.
fn scale_param(p: &Value) -> Result<Option<f64>> {
    let Some(v) = p.get("scale").filter(|v| !v.is_null()) else { return Ok(None) };
    let k = match v {
        Value::Number(n) => n.as_f64(),
        Value::String(t) => {
            let t = t.trim();
            if let Some((a, b)) = t.split_once(':') {
                match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
                    (Ok(a), Ok(b)) if a > 0.0 && b > 0.0 => Some(12.0 * a / b),
                    _ => None,
                }
            } else {
                // `1/8"=1'-0"`, `3/16"`, `0.125`.
                let lead = t.split(['"', '=']).next().unwrap_or("").trim();
                match lead.split_once('/') {
                    Some((a, b)) => match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
                        (Ok(a), Ok(b)) if b > 0.0 => Some(a / b),
                        _ => None,
                    },
                    None => lead.parse::<f64>().ok(),
                }
            }
        }
        _ => None,
    };
    match k {
        Some(k) if k.is_finite() && (1.0 / 1024.0..=12.0).contains(&k) => Ok(Some(k)),
        _ => Err(bad(CMD, "`scale` is like \"1/8\\\"=1'-0\\\"\", \"1:96\" or paper inches per foot (0.125)")),
    }
}

fn disciplines(p: &Value) -> Result<Option<BTreeSet<Disc>>> {
    let Some(v) = p.get("disciplines").filter(|v| !v.is_null()) else { return Ok(None) };
    let words: Vec<&str> = match v {
        Value::String(t) => t.split([',', ' ']).filter(|w| !w.trim().is_empty()).collect(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        _ => return Err(bad(CMD, "`disciplines` is a list: A, E, M, P, FP")),
    };
    let mut out = BTreeSet::new();
    for w in words.into_iter().take(16) {
        out.insert(Disc::parse(w).ok_or_else(|| bad(CMD, format!("no discipline `{w}`: A, E, M, P or FP")))?);
    }
    Ok(Some(out))
}

fn point(v: Option<&Value>) -> Option<[f64; 2]> {
    let p = v.and_then(point_value)?;
    ok([p.x, p.y]).then_some([p.x, p.y])
}

// ─── The project ────────────────────────────────────────────────────────────

#[derive(Default)]
struct Info {
    name: String,
    number: String,
    firm: String,
    client: String,
    address: String,
    date: String,
}

fn info(s: &mut Session, p: &Value) -> Result<Info> {
    let given =
        |k: &str| p.get("project").and_then(|v| v.get(k)).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    let d = s.doc()?;
    let props = &d.properties;
    let custom = |names: &[&str]| names.iter().find_map(|n| props.custom(n)).map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    let mut i = Info {
        name: given("name")
            .or_else(|| custom(&["Project", "Project Name"]))
            .or_else(|| Some(props.title.trim().to_string()).filter(|t| !t.is_empty()))
            .unwrap_or_default(),
        number: given("number").or_else(|| custom(&["Project Number", "Project No", "Job Number"])).unwrap_or_default(),
        firm: given("firm").or_else(|| custom(&["Firm", "Architect", "Company"])).unwrap_or_default(),
        client: given("client").or_else(|| custom(&["Client", "Owner"])).unwrap_or_default(),
        address: given("address").or_else(|| custom(&["Address", "Site", "Location"])).unwrap_or_default(),
        date: given("date").or_else(|| custom(&["Date", "Issue Date"])).unwrap_or_default(),
    };
    if i.name.is_empty()
        && find_command("project.info").is_some()
        && let Ok(v) = s.execute("project.info", &json!({}))
    {
        i.name = v["project"]["name"].as_str().unwrap_or("").trim().to_string();
    }
    if i.name.is_empty() {
        i.name = "PROJECT".into();
    }
    Ok(i)
}

// ─── Sheets ─────────────────────────────────────────────────────────────────

struct Sheet {
    number: String,
    title: String,
    disc: Option<Disc>,
    level: Option<String>,
    view: &'static str,
    scale: Option<f64>,
    pen: Pen,
}

/// Sheet numbers by discipline and series: `A101`, `A102`, `E201`.
#[derive(Default)]
struct Numbers(BTreeMap<String, usize>);

impl Numbers {
    fn next(&mut self, code: &str, series: u32) -> String {
        let n = self.0.entry(format!("{code}{series}")).or_default();
        *n += 1;
        format!("{code}{series}{:02}", *n)
    }
}

/// A level of the model: its name as the elements carry it, the name printed, its floor
/// elevation (drawing units).
struct Lvl {
    tag: String,
    name: String,
    z: f64,
}

fn levels_of(items: &[Item], sp: Option<&cadaddy_walk3d::BuildingSpec>, upf: f64) -> Vec<Lvl> {
    let mut by: BTreeMap<String, f64> = BTreeMap::new();
    for it in items {
        let z = by.entry(it.level.clone()).or_insert(f64::INFINITY);
        if it.z[0].is_finite() {
            *z = z.min(it.z[0]);
        }
    }
    let mut out: Vec<Lvl> = by
        .into_iter()
        .take(MAX_LEVELS)
        .map(|(tag, z)| {
            let spec = sp.and_then(|sp| sp.levels.iter().find(|l| l.name.eq_ignore_ascii_case(&tag)));
            let z = spec.map(|l| l.elevation_ft * upf).filter(|z| z.is_finite()).unwrap_or(if z.is_finite() { z } else { 0.0 });
            let name = if tag.is_empty() { "LEVEL".to_string() } else { spec.map(|l| l.name.clone()).unwrap_or_else(|| tag.clone()) };
            Lvl { tag, name, z }
        })
        .collect();
    out.sort_by(|a, b| a.z.total_cmp(&b.z).then_with(|| a.name.cmp(&b.name)));
    out
}

fn grid_lines(sp: Option<&cadaddy_walk3d::BuildingSpec>) -> GridLines {
    let Some(sp) = sp else { return GridLines::default() };
    let g = super::ugsheets::grids(sp);
    GridLines { x: g.x, y: g.y }
}

/// The plan box on a sheet: inside the area, room for the dimension strings round it, the view
/// title under it and the keynote column on the right.
fn plan_box(a: [f64; 4]) -> [f64; 4] {
    [a[0] + 1.9, a[1] + 2.6, a[2] - 4.6, a[3] - 2.0]
}

/// The legend column right of a plan: north arrow, keynotes, then the legend of the marks.
fn legend(pen: &mut Pen, a: [f64; 4], notes: &[String], marks: &[String], north: bool) {
    let x = a[2] - 4.2;
    let mut y = a[3] - 0.4;
    if north {
        views::north_arrow(pen, [x + 0.5, y - 0.4]);
        y -= 1.2;
    }
    pen.layer("G-ANNO-TEXT");
    y = list(pen, x, y, "KEYNOTES", notes, 44, a[1] + 1.2);
    let _ = list(pen, x, y, "LEGEND", marks, 44, a[1] + 1.2);
}

/// The marks a plan's elements carry with what they are: `W1 - 1 HR`, `A - 2X4 TROFFER`.
fn marks(items: &[&Item], kind: PlanKind) -> Vec<String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for it in items.iter().filter(|it| kind.shows(it)) {
        let (mark, what) = match it.kind.as_str() {
            "wall" => (it.first(&["type", "type_mark"]), it.first(&["description", "assembly", "fire_rating"])),
            "room" | "door" | "ceiling" => continue,
            _ => (it.p("type_mark"), it.first(&["type", "description", "model"])),
        };
        if let Some(m) = mark
            && out.len() < 40
        {
            let e = out.entry(m.to_string()).or_default();
            if e.is_empty() {
                *e = what.unwrap_or("").to_string();
            }
        }
    }
    out.into_iter().map(|(m, w)| if w.is_empty() { m } else { format!("{m} - {}", w.to_ascii_uppercase()) }).collect()
}

struct PlanJob<'a> {
    kind: PlanKind,
    lvl: &'a Lvl,
    items: Vec<&'a Item>,
    lo: [f64; 2],
    hi: [f64; 2],
}

fn plan_sheet(job: &PlanJob, number: String, per_ft: f64, upf: f64, paper: &Paper, grids: &GridLines, panels: &BTreeMap<String, [f64; 2]>) -> Sheet {
    let a = paper.area;
    let f = Frame::centred(job.lo, job.hi, plan_box(a), per_ft / upf);
    let ctx = PlanCtx { upf, f, grids, panels, floor_z: job.lvl.z, mid: [(job.lo[0] + job.hi[0]) / 2.0, (job.lo[1] + job.hi[1]) / 2.0] };
    let mut pen = Pen::default();
    let notes = views::plan(&mut pen, &ctx, &job.items, job.kind, job.lo, job.hi);
    let title = format!("{} - {}", job.kind.title(), job.lvl.name.to_ascii_uppercase());
    view_title(&mut pen, [a[0] + 0.2, a[1] + 0.15], 1, &title, Some(per_ft));
    legend(&mut pen, a, &notes, &marks(&job.items, job.kind), true);
    Sheet { number, title, disc: Some(job.kind.disc()), level: Some(job.lvl.name.clone()), view: "plan", scale: Some(per_ft), pen }
}

/// Every level's extents: its walls', else all its elements'.
fn extents(items: &[&Item]) -> ([f64; 2], [f64; 2]) {
    let walls: Vec<[f64; 2]> = items.iter().filter(|it| it.kind == "wall").flat_map(|it| it.hull.iter().copied()).collect();
    let (lo, hi) = bounds(&walls);
    if ok(lo) && ok(hi) {
        return (lo, hi);
    }
    let all: Vec<[f64; 2]> = items.iter().flat_map(|it| it.hull.iter().copied()).collect();
    bounds(&all)
}

// ─── Schedules ──────────────────────────────────────────────────────────────

struct Sched {
    disc: Disc,
    title: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(cell).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// The schedules from `bim.schedule` the disciplines asked for have, each non-empty one.
fn schedules(s: &mut Session, discs: &BTreeSet<Disc>, notes: &mut Vec<String>) -> Vec<Sched> {
    let kinds: [(Disc, &str, &str); 8] = [
        (Disc::Arch, "door", "DOOR SCHEDULE"),
        (Disc::Arch, "window", "WINDOW SCHEDULE"),
        (Disc::Arch, "room_finish", "ROOM FINISH SCHEDULE"),
        (Disc::Plumb, "plumbing_fixture", "PLUMBING FIXTURE SCHEDULE"),
        (Disc::Mech, "equipment", "EQUIPMENT SCHEDULE"),
        (Disc::Mech, "air_distribution", "AIR DISTRIBUTION SCHEDULE"),
        (Disc::Elec, "light_fixture", "LIGHTING FIXTURE SCHEDULE"),
        (Disc::Elec, "panel", "PANEL SCHEDULE"),
    ];
    let mut out = Vec::new();
    for (disc, kind, title) in kinds {
        if !discs.contains(&disc) {
            continue;
        }
        let r = match s.execute(super::propuse::SCHEDULE, &json!({ "kind": kind })) {
            Ok(r) => r,
            Err(e) => {
                notes.push(format!("{title}: {e}"));
                continue;
            }
        };
        let columns: Vec<String> = r["columns"]
            .as_array()
            .map(|c| c.iter().filter_map(Value::as_str).filter(|c| *c != "handle").map(str::to_string).collect())
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = r["rows"]
            .as_array()
            .map(|rows| rows.iter().take(MAX_SCHEDULE_ROWS).map(|row| columns.iter().map(|c| cell(&row[c.as_str()])).collect()).collect())
            .unwrap_or_default();
        if rows.is_empty() || columns.is_empty() {
            continue;
        }
        if kind == "panel" {
            // One schedule a panel.
            let at = columns.iter().position(|c| c == "panel");
            let mut by: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
            for row in rows {
                let p = at.and_then(|i| row.get(i)).cloned().unwrap_or_default();
                by.entry(p).or_default().push(row);
            }
            for (panel, rows) in by {
                let load: f64 = columns
                    .iter()
                    .position(|c| c == "load_va")
                    .map(|i| rows.iter().filter_map(|r| r.get(i).and_then(|v| v.parse::<f64>().ok())).sum())
                    .unwrap_or(0.0);
                let title = if panel.is_empty() { "PANEL SCHEDULE".to_string() } else { format!("PANEL {panel} - CONNECTED {load:.0} VA") };
                out.push(Sched { disc, title, columns: columns.clone(), rows });
            }
        } else {
            out.push(Sched { disc, title: title.to_string(), columns, rows });
        }
    }
    out
}

fn takeoff(s: &mut Session, notes: &mut Vec<String>) -> Option<Sched> {
    let r = match s.execute("bim.takeoff", &json!({ "group_by": "trade", "keep": false })) {
        Ok(r) => r,
        Err(e) => {
            notes.push(format!("takeoff: {e}"));
            return None;
        }
    };
    let columns: Vec<String> = ["trade", "item", "size", "qty", "unit", "level"].iter().map(|c| (*c).to_string()).collect();
    let rows: Vec<Vec<String>> = r["lines"]
        .as_array()?
        .iter()
        .take(MAX_SCHEDULE_ROWS)
        .map(|l| {
            columns
                .iter()
                .map(|c| if c == "qty" { l[c.as_str()].as_f64().map(|q| format!("{q:.1}")).unwrap_or_default() } else { cell(&l[c.as_str()]) })
                .collect()
        })
        .collect();
    (!rows.is_empty()).then(|| Sched { disc: Disc::Arch, title: "TAKEOFF SUMMARY".into(), columns, rows })
}

const TABLE_TEXT: f64 = 0.09;

fn style() -> TableStyle {
    TableStyle { name: "Standard".into(), text_height: TABLE_TEXT, margin: 0.04, title: true, header: true, cell_styles: Vec::new() }
}

/// A schedule's rows `rows` as a table at `at` (its top left); its width and height.
fn table(title: &str, columns: &[String], rows: &[Vec<String>], at: [f64; 2]) -> (cadaddy_doc::Table, f64, f64) {
    let st = style();
    let header: Vec<String> = columns.iter().map(|c| c.replace('_', " ").to_ascii_uppercase()).collect();
    let mut t = super::table::make_table(&st, Vec2::new(at[0], at[1]), rows.len(), header.len(), None, None, Some(title), Some(header.clone()), rows);
    let char_w = TABLE_TEXT * 0.85;
    t.col_widths = header
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let longest = rows.iter().filter_map(|r| r.get(i)).map(|c| c.chars().count()).chain([h.chars().count()]).max().unwrap_or(4).min(36);
            (longest as f64 * char_w + 2.0 * st.margin + 0.06).max(0.5)
        })
        .collect();
    // The title spans the table; a title longer than the columns widens the first.
    let short = title.chars().count() as f64 * TABLE_TEXT * 1.4 * 0.85 + 0.2 - t.col_widths.iter().sum::<f64>();
    if let Some(first) = t.col_widths.first_mut() {
        *first += short.max(0.0);
    }
    let w: f64 = t.col_widths.iter().sum();
    let h: f64 = t.row_heights.iter().sum();
    (t, w, h)
}

/// Schedules flowed onto sheets in columns, top to bottom; a schedule too long for the sheet
/// goes on in the next column (or sheet) under its title with "(CONT.)".
fn schedule_sheets(scheds: &[Sched], code: &str, disc: Disc, paper: &Paper, numbers: &mut Numbers, series: u32, title: &str) -> Vec<Sheet> {
    let a = paper.area;
    let (top, bottom) = (a[3] - 0.4, a[1] + 1.0);
    let row_h = TABLE_TEXT * 5.0 / 3.0 + 0.08;
    let mut sheets: Vec<Sheet> = Vec::new();
    let mut pen = Pen::default();
    let (mut x, mut y, mut col_w) = (a[0] + 0.3, top, 0.0_f64);
    let mut on_sheet = 0;
    let flush = |pen: &mut Pen, sheets: &mut Vec<Sheet>, numbers: &mut Numbers| {
        let mut p = std::mem::take(pen);
        let n = sheets.len() + 1;
        let t = if n == 1 { title.to_string() } else { format!("{title} ({n})") };
        view_title(&mut p, [a[0] + 0.2, a[1] + 0.15], 1, &t, None);
        sheets.push(Sheet { number: numbers.next(code, series), title: t, disc: Some(disc), level: None, view: "schedules", scale: None, pen: p });
    };
    let start_x = a[0] + 0.3;
    for sc in scheds {
        let mut rest: &[Vec<String>] = &sc.rows;
        let mut part = 0;
        while !rest.is_empty() {
            let fresh_column = (y - top).abs() < 1e-9;
            let fits = (((y - bottom) / row_h).floor() as i64 - 3).max(0) as usize;
            if fits < 2 && !fresh_column {
                // The next column.
                (x, y, col_w) = (x + col_w + 0.4, top, 0.0);
                continue;
            }
            let (now, later) = rest.split_at(fits.max(1).min(rest.len()));
            let t = if part == 0 { sc.title.clone() } else { format!("{} (CONT.)", sc.title) };
            let (tb, w, h) = table(&t, &sc.columns, now, [x, y]);
            if x + w > a[2] && x > start_x + 1e-9 {
                // The next sheet.
                flush(&mut pen, &mut sheets, numbers);
                (x, y, col_w, on_sheet) = (start_x, top, 0.0, 0);
                if sheets.len() >= MAX_SHEETS {
                    return sheets;
                }
                continue;
            }
            pen.layer(&format!("{}-ANNO-SCHD", disc.prefix()));
            pen.table(tb);
            pen.count("schedules");
            *pen.counts.entry("schedule_rows".into()).or_default() += now.len();
            on_sheet += 1;
            col_w = col_w.max(w);
            y -= h + 0.5;
            rest = later;
            part += 1;
        }
    }
    if on_sheet > 0 {
        flush(&mut pen, &mut sheets, numbers);
    }
    sheets
}

// ─── The command ────────────────────────────────────────────────────────────

/// What the set is made of, worked out before anything is drawn.
struct Ask {
    discs: Option<BTreeSet<Disc>>,
    levels: Option<Vec<String>>,
    scale: Option<f64>,
    paper: Paper,
    sections: Vec<(String, [f64; 2], [f64; 2])>,
    depth_ft: f64,
    enlarged: Vec<(String, [f64; 4], String)>,
    schedules: bool,
    views3d: bool,
    takeoff: bool,
}

fn ask(p: &Value) -> Result<Ask> {
    let levels = match p.get("levels") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => {
            Some(a.iter().filter_map(Value::as_str).map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).take(MAX_LEVELS).collect())
        }
        Some(Value::String(l)) => Some(vec![l.trim().to_string()]),
        Some(_) => return Err(bad(CMD, "`levels` is a list of level names")),
    };
    let mut sections = Vec::new();
    if let Some(list) = p.get("sections").and_then(Value::as_array) {
        for (i, v) in list.iter().take(MAX_VIEWS).enumerate() {
            let (Some(a), Some(b)) = (point(v.get("from")), point(v.get("to"))) else {
                return Err(bad(CMD, "each of `sections` is {from: [x,y], to: [x,y], name?}"));
            };
            let name = v.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("SECTION {}", i + 1));
            sections.push((name, a, b));
        }
    }
    let mut enlarged = Vec::new();
    if let Some(list) = p.get("enlarged").and_then(Value::as_array) {
        for v in list.iter().take(MAX_VIEWS) {
            let r: Vec<f64> = v.get("rect").and_then(Value::as_array).map(|r| r.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            let ([x0, y0, x1, y1], Some(level)) =
                (<[f64; 4]>::try_from(r.as_slice()).unwrap_or([f64::NAN; 4]), v.get("level").and_then(Value::as_str))
            else {
                return Err(bad(CMD, "each of `enlarged` is {level, rect: [x0,y0,x1,y1], title?}"));
            };
            if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
                return Err(bad(CMD, "an `enlarged` rect is [x0, y0, x1, y1] with x1 > x0 and y1 > y0"));
            }
            let title = v.get("title").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| "ENLARGED PLAN".into());
            enlarged.push((level.to_string(), [x0, y0, x1, y1], title));
        }
    }
    let depth_ft = p.get("depth").and_then(Value::as_f64).filter(|d| d.is_finite() && *d > 0.0).unwrap_or(40.0).min(10_000.0);
    Ok(Ask {
        discs: disciplines(p)?,
        levels,
        scale: scale_param(p)?,
        paper: paper(p)?,
        sections,
        depth_ft,
        enlarged,
        schedules: bool_or(p, "schedules", true),
        views3d: bool_or(p, "views3d", true),
        takeoff: bool_or(p, "takeoff", false),
    })
}

/// Wet rooms worth an enlarged plan, and the print set's enlarged plans placed on their levels.
fn enlarged_regions(items: &[Item], sp: Option<&cadaddy_walk3d::BuildingSpec>, upf: f64) -> Vec<(String, [f64; 4], String)> {
    const WET: &[&str] = &["TOILET", "RESTROOM", "BATH", "WOMEN", "MEN", "LAV", "KITCHEN", "JANITOR", "SHOWER"];
    let mut out = Vec::new();
    for it in items.iter().filter(|it| it.kind == "room") {
        let name = it.p("name").unwrap_or("").to_ascii_uppercase();
        if WET.iter().any(|w| name.split(|c: char| !c.is_ascii_alphanumeric()).any(|word| word == *w || word.starts_with(w))) {
            let (lo, hi) = it.bounds();
            let m = 2.0 * upf;
            let title = format!("ENLARGED PLAN - {}{}", name, it.p("number").map(|n| format!(" {n}")).unwrap_or_default());
            out.push((it.level.clone(), [lo[0] - m, lo[1] - m, hi[0] + m, hi[1] + m], title));
        }
    }
    if let Some(sp) = sp {
        for sh in sp.sheets.iter().filter(|sh| sh.title.to_ascii_uppercase().contains("ENLARGED")) {
            let (Some(level), Some(r), Some(reg)) = (&sh.level, sh.rect, &sh.registration) else { continue };
            let t = |x: f64, y: f64| [reg.a * x - reg.b * y + reg.tx, reg.b * x + reg.a * y + reg.ty];
            let (lo, hi) = bounds(&[t(r[0], r[1]), t(r[2], r[1]), t(r[2], r[3]), t(r[0], r[3])]);
            if ok(lo) && ok(hi) {
                out.push((level.clone(), [lo[0], lo[1], hi[0], hi[1]], format!("ENLARGED PLAN - {}", sh.title.to_ascii_uppercase())));
            }
        }
    }
    out.truncate(MAX_VIEWS * 2);
    out
}

/// One undo step for the set, however many entities draw it (a step of each kept a copy of the
/// drawing).
fn run(s: &mut Session, p: &Value) -> Result<Value> {
    s.as_one_undo_step(|s| run_inner(s, p))
}

fn run_inner(s: &mut Session, p: &Value) -> Result<Value> {
    let ask = ask(p)?;
    let mut notes: Vec<String> = Vec::new();
    let spec = super::bspec::current(s, CMD).ok().map(|(sp, _)| sp);
    let (count, has) = {
        let (items, _) = model::items(s.doc()?);
        let has: BTreeSet<Disc> = items.iter().map(|it| it.disc).collect();
        (items.len(), has)
    };
    if count == 0 {
        return Err(crate::EngineError::Other("The model has no building elements to draw: build it first (bim.build, wallgen).".into()));
    }
    // Architectural always (the plans the others sit on); the others when the model has them.
    let discs: BTreeSet<Disc> = match &ask.discs {
        Some(d) => d.clone(),
        None => Disc::ALL.into_iter().filter(|d| *d == Disc::Arch || has.contains(d)).collect(),
    };
    let info = info(s, p)?;
    let scheds = if ask.schedules { schedules(s, &discs, &mut notes) } else { Vec::new() };
    let takeoff = if ask.takeoff { takeoff(s, &mut notes) } else { None };
    let tb = p.get("title_block").and_then(Value::as_str).map(str::to_string).or_else(|| own_title_block(s));
    if let Some(b) = &tb
        && !s.doc()?.blocks.contains_key(b.as_str())
    {
        return Err(bad(CMD, format!("no block `{b}` for the title block")));
    }
    let mut sheets: Vec<Sheet> = Vec::new();
    let mut numbers = Numbers::default();
    {
        let d = s.doc()?;
        let (items, upf) = model::items(d);
        let sp = spec.as_ref();
        let lvls = levels_of(&items, sp, upf);
        let grids = grid_lines(sp);
        let panels: BTreeMap<String, [f64; 2]> = items
            .iter()
            .filter(|it| it.panel || it.kind == "equipment")
            .filter_map(|it| it.tag.as_ref().map(|t| (t.to_ascii_uppercase(), it.center())))
            .collect();
        let wanted =
            |l: &Lvl| ask.levels.as_ref().is_none_or(|w| w.iter().any(|x| x.eq_ignore_ascii_case(&l.tag) || x.eq_ignore_ascii_case(&l.name)));
        let on = |l: &Lvl| -> Vec<&Item> { items.iter().filter(|it| it.level == l.tag).collect() };
        if let Some(w) = &ask.levels {
            for x in w {
                if !lvls.iter().any(|l| x.eq_ignore_ascii_case(&l.tag) || x.eq_ignore_ascii_case(&l.name)) {
                    notes.push(format!("no level `{x}` in the model"));
                }
            }
        }
        // One scale for every plan: the largest that fits the largest level.
        let pb = plan_box(ask.paper.area);
        let (pw, ph) = (pb[2] - pb[0], pb[3] - pb[1]);
        let per_ft = ask.scale.unwrap_or_else(|| {
            let (mut w, mut h) = (1.0_f64, 1.0_f64);
            for l in &lvls {
                let (lo, hi) = extents(&on(l));
                if ok(lo) && ok(hi) {
                    w = w.max((hi[0] - lo[0]) / upf);
                    h = h.max((hi[1] - lo[1]) / upf);
                }
            }
            fit_scale(w, h, pw, ph, 0.25)
        });
        let (all_lo, all_hi) = extents(&items.iter().collect::<Vec<_>>());
        let (blo, bhi) = (all_lo, all_hi);
        if ask.scale.is_some() && ok(blo) && ok(bhi) && ((bhi[0] - blo[0]) / upf * per_ft > pw || (bhi[1] - blo[1]) / upf * per_ft > ph) {
            notes.push(format!("at {} the plans are larger than the sheet's plan area", views::scale_label(per_ft)));
        }
        let plans_of = |kinds: &[PlanKind], series: u32, numbers: &mut Numbers, sheets: &mut Vec<Sheet>| {
            for kind in kinds {
                for l in lvls.iter().filter(|l| wanted(l)) {
                    if sheets.len() >= MAX_SHEETS {
                        return;
                    }
                    let its = on(l);
                    if kind.disc() != Disc::Arch && !its.iter().any(|it| kind.shows(it)) {
                        continue;
                    }
                    if *kind == PlanKind::Ceiling && !its.iter().any(|it| kind.shows(it) && it.kind != "wall") {
                        continue;
                    }
                    let (lo, hi) = extents(&its);
                    if !ok(lo) || !ok(hi) {
                        continue;
                    }
                    let job = PlanJob { kind: *kind, lvl: l, items: its, lo, hi };
                    let series = if kind.disc() == Disc::Elec {
                        match kind {
                            PlanKind::Lighting => 2,
                            PlanKind::Systems => 3,
                            _ => 1,
                        }
                    } else {
                        series
                    };
                    let number = numbers.next(kind.disc().code(), series);
                    sheets.push(plan_sheet(&job, number, per_ft, upf, &ask.paper, &grids, &panels));
                }
            }
        };
        for disc in &discs {
            match disc {
                Disc::Arch => {
                    plans_of(&[PlanKind::Floor, PlanKind::Ceiling], 1, &mut numbers, &mut sheets);
                    // The roof plan: the top level's outline with the roof and what's on it.
                    if let Some(top) = lvls.iter().rev().find(|l| on(l).iter().any(|it| it.kind == "wall")) {
                        let mut its = on(top);
                        its.extend(items.iter().filter(|it| it.kind == "roof" && it.level != top.tag));
                        let (lo, hi) = extents(&its);
                        if ok(lo) && ok(hi) && sheets.len() < MAX_SHEETS {
                            let roof_lvl = Lvl { tag: top.tag.clone(), name: "ROOF".into(), z: top.z };
                            let job = PlanJob { kind: PlanKind::Roof, lvl: &roof_lvl, items: its, lo, hi };
                            let number = numbers.next("A", 1);
                            sheets.push(plan_sheet(&job, number, per_ft, upf, &ask.paper, &grids, &panels));
                        }
                    }
                    if ask.views3d && ok(blo) && ok(bhi) {
                        sheets.push(elevations_sheet(d, &items, &lvls, blo, bhi, upf, &ask.paper, numbers.next("A", 2)));
                    }
                    sheets.push(sections_sheet(d, &items, &lvls, blo, bhi, upf, &ask, numbers.next("A", 3), &mut notes));
                    let mut regions = enlarged_regions(&items, sp, upf);
                    regions.extend(ask.enlarged.iter().cloned());
                    for sh in enlarged_sheets(&items, &lvls, &regions, upf, &ask.paper, &mut numbers, &grids, &panels) {
                        sheets.push(sh);
                    }
                    let mine: Vec<&Sched> = scheds.iter().filter(|s| s.disc == Disc::Arch).collect();
                    sheets.extend(schedule_sheets(
                        &mine.into_iter().map(clone_sched).collect::<Vec<_>>(),
                        "A",
                        Disc::Arch,
                        &ask.paper,
                        &mut numbers,
                        6,
                        "SCHEDULES",
                    ));
                    if ask.views3d {
                        sheets.push(axon_sheet(d, &items, &ask.paper, numbers.next("A", 9)));
                    }
                }
                Disc::Elec => plans_of(&[PlanKind::Power, PlanKind::Lighting, PlanKind::Systems], 1, &mut numbers, &mut sheets),
                Disc::Mech => plans_of(&[PlanKind::Mechanical], 1, &mut numbers, &mut sheets),
                Disc::Plumb => plans_of(&[PlanKind::Plumbing], 1, &mut numbers, &mut sheets),
                Disc::Fire => plans_of(&[PlanKind::Fire], 1, &mut numbers, &mut sheets),
            }
            if *disc != Disc::Arch {
                let mine: Vec<Sched> = scheds.iter().filter(|s| s.disc == *disc).map(clone_sched).collect();
                sheets.extend(schedule_sheets(&mine, disc.code(), *disc, &ask.paper, &mut numbers, 6, "SCHEDULES"));
            }
        }
    }
    if let Some(t) = takeoff {
        let mut n = Numbers::default();
        let _ = n.next("G0", 0);
        let mut more = schedule_sheets(&[t], "G", Disc::Arch, &ask.paper, &mut n, 0, "TAKEOFF SUMMARY");
        for sh in &mut more {
            sh.disc = None;
        }
        sheets.splice(0..0, more);
    }
    sheets.truncate(MAX_SHEETS - 1);
    let cover = cover_sheet(&info, &sheets, &ask.paper, &discs);
    sheets.insert(0, cover);
    if sheets.iter().any(|sh| sh.pen.full) {
        notes.push(format!("a sheet reached its {} shapes and was cut short", pen::MAX_SHAPES));
    }
    place_all(s, p, sheets, &ask.paper, tb.as_deref(), &info, notes)
}

fn clone_sched(s: &Sched) -> Sched {
    Sched { disc: s.disc, title: s.title.clone(), columns: s.columns.clone(), rows: s.rows.clone() }
}

/// The drawing's own title block: a block named like one that isn't the library's.
fn own_title_block(s: &Session) -> Option<String> {
    let d = s.doc().ok()?;
    d.blocks
        .keys()
        .find(|n| {
            let up = n.to_ascii_uppercase();
            !n.starts_with('*') && !up.starts_with("G_TITLE_") && (up.contains("TITLE") || up.starts_with("TB") || up.contains("BORDER"))
        })
        .cloned()
}

/// Up to `n` boxes in a grid filling the area above the view titles.
fn cells(a: [f64; 4], cols: usize, rows: usize) -> Vec<[f64; 4]> {
    let (w, h) = ((a[2] - a[0]) / cols as f64, (a[3] - a[1]) / rows as f64);
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let (x0, y1) = (a[0] + c as f64 * w, a[3] - r as f64 * h);
            out.push([x0, y1 - h, x0 + w, y1]);
        }
    }
    out
}

/// The view box in a cell: margins, the title band under it, room for level marks on the right.
fn inner(c: [f64; 4]) -> [f64; 4] {
    [c[0] + 0.5, c[1] + 1.0, c[2] - 1.6, c[3] - 0.4]
}

#[allow(clippy::too_many_arguments)]
fn elevations_sheet(
    d: &cadaddy_doc::Drawing,
    items: &[Item],
    lvls: &[Lvl],
    lo: [f64; 2],
    hi: [f64; 2],
    upf: f64,
    paper: &Paper,
    number: String,
) -> Sheet {
    let m = 10.0 * upf;
    let lines = [
        ("SOUTH ELEVATION", [lo[0], lo[1] - m], [hi[0], lo[1] - m]),
        ("NORTH ELEVATION", [hi[0], hi[1] + m], [lo[0], hi[1] + m]),
        ("EAST ELEVATION", [hi[0] + m, lo[1]], [hi[0] + m, hi[1]]),
        ("WEST ELEVATION", [lo[0] - m, hi[1]], [lo[0] - m, lo[1]]),
    ];
    let drawn: Vec<(&str, views::UprightDrawing)> =
        lines.iter().filter_map(|(t, a, b)| Upright::new(*a, *b).map(|u| (*t, views::elevation(d, items, &u)))).collect();
    upright_sheet(drawn, lvls, upf, paper, number, "BUILDING ELEVATIONS", "A-ELEV", "elevations")
}

#[allow(clippy::too_many_arguments)]
fn sections_sheet(
    d: &cadaddy_doc::Drawing,
    items: &[Item],
    lvls: &[Lvl],
    lo: [f64; 2],
    hi: [f64; 2],
    upf: f64,
    ask: &Ask,
    number: String,
    notes: &mut Vec<String>,
) -> Sheet {
    let m = 2.0 * upf;
    let mut lines = ask.sections.clone();
    if lines.is_empty() && ok(lo) && ok(hi) {
        let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
        lines.push(("BUILDING SECTION A".into(), [lo[0] - m, cy], [hi[0] + m, cy]));
        lines.push(("BUILDING SECTION B".into(), [cx, lo[1] - m], [cx, hi[1] + m]));
    }
    let depth = ask.depth_ft * upf;
    let mut drawn = Vec::new();
    for (t, a, b) in lines.iter().take(4) {
        match Upright::new(*a, *b) {
            Some(u) => drawn.push((t.as_str(), views::section(d, items, &u, depth))),
            None => notes.push(format!("{t}: its two points are the same")),
        }
    }
    if lines.len() > 4 {
        notes.push(format!("{} sections asked; the sheet shows the first 4", lines.len()));
    }
    upright_sheet(drawn, lvls, upf, &ask.paper, number, "BUILDING SECTIONS", "A-SECT-BYND", "sections")
}

#[allow(clippy::too_many_arguments)]
fn upright_sheet(
    drawn: Vec<(&str, views::UprightDrawing)>,
    lvls: &[Lvl],
    upf: f64,
    paper: &Paper,
    number: String,
    title: &str,
    beyond: &str,
    view: &'static str,
) -> Sheet {
    let a = paper.area;
    let boxes = cells(a, 2, 2);
    // One scale for them all, the largest that fits every one.
    let per_ft = drawn
        .iter()
        .zip(&boxes)
        .map(|((_, v), c)| {
            let (lo, hi) = v.bounds();
            let ib = inner(*c);
            if ok(lo) && ok(hi) { fit_scale((hi[0] - lo[0]) / upf, (hi[1] - lo[1]) / upf, ib[2] - ib[0], ib[3] - ib[1], 0.25) } else { 0.25 }
        })
        .fold(0.25, f64::min);
    let levels: Vec<(String, f64)> = lvls.iter().map(|l| (l.name.to_ascii_uppercase(), l.z)).collect();
    let mut pen = Pen::default();
    for (i, ((t, v), c)) in drawn.iter().zip(&boxes).enumerate() {
        let (lo, hi) = v.bounds();
        if !ok(lo) || !ok(hi) {
            continue;
        }
        let f = Frame::centred(lo, hi, inner(*c), per_ft / upf);
        put_upright(&mut pen, &f, v, &levels, upf, beyond);
        view_title(&mut pen, [c[0] + 0.2, c[1] + 0.15], i + 1, t, Some(per_ft));
        pen.count("views");
    }
    Sheet { number, title: title.into(), disc: Some(Disc::Arch), level: None, view, scale: Some(per_ft), pen }
}

#[allow(clippy::too_many_arguments)]
fn enlarged_sheets(
    items: &[Item],
    lvls: &[Lvl],
    regions: &[(String, [f64; 4], String)],
    upf: f64,
    paper: &Paper,
    numbers: &mut Numbers,
    grids: &GridLines,
    panels: &BTreeMap<String, [f64; 2]>,
) -> Vec<Sheet> {
    let mut out = Vec::new();
    for chunk in regions.chunks(4) {
        let a = paper.area;
        let boxes = cells(a, 2, 2);
        let mut pen = Pen::default();
        let mut scale = None;
        for (i, ((level, r, title), c)) in chunk.iter().zip(&boxes).enumerate() {
            let Some(l) = lvls.iter().find(|l| l.tag.eq_ignore_ascii_case(level) || l.name.eq_ignore_ascii_case(level)) else { continue };
            let its: Vec<&Item> = items
                .iter()
                .filter(|it| it.level == l.tag)
                .filter(|it| {
                    let (lo, hi) = it.bounds();
                    lo[0] < r[2] && hi[0] > r[0] && lo[1] < r[3] && hi[1] > r[1]
                })
                .collect();
            let ib = inner(*c);
            let per_ft = fit_scale((r[2] - r[0]) / upf, (r[3] - r[1]) / upf, ib[2] - ib[0], ib[3] - ib[1], 0.5);
            scale = Some(scale.map_or(per_ft, |s: f64| s.min(per_ft)));
            let f = Frame::centred([r[0], r[1]], [r[2], r[3]], ib, per_ft / upf);
            let ctx = PlanCtx { upf, f, grids, panels, floor_z: l.z, mid: [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0] };
            let _ = views::plan(&mut pen, &ctx, &its, PlanKind::Enlarged, [r[0], r[1]], [r[2], r[3]]);
            view_title(&mut pen, [c[0] + 0.2, c[1] + 0.15], i + 1, &format!("{title} - {}", l.name.to_ascii_uppercase()), Some(per_ft));
            pen.count("views");
        }
        if pen.counts.get("views").copied().unwrap_or(0) > 0 {
            out.push(Sheet {
                number: numbers.next("A", 4),
                title: "ENLARGED PLANS".into(),
                disc: Some(Disc::Arch),
                level: None,
                view: "enlarged",
                scale,
                pen,
            });
        }
    }
    out
}

fn axon_sheet(d: &cadaddy_doc::Drawing, items: &[Item], paper: &Paper, number: String) -> Sheet {
    let a = paper.area;
    let lines = views::axon(d, items);
    let pts: Vec<[f64; 2]> = lines.iter().flat_map(|l| [l[0], l[1]]).collect();
    let (lo, hi) = bounds(&pts);
    let mut pen = Pen::default();
    let ib = [a[0] + 0.5, a[1] + 1.2, a[2] - 0.5, a[3] - 0.5];
    if ok(lo) && ok(hi) {
        let k = ((ib[2] - ib[0]) / (hi[0] - lo[0]).max(1e-9)).min((ib[3] - ib[1]) / (hi[1] - lo[1]).max(1e-9));
        let f = Frame::centred(lo, hi, ib, k);
        pen.layer("A-ISO");
        for [p, q] in &lines {
            pen.line(f.p(*p), f.p(*q));
        }
        pen.count("views");
    }
    view_title(&mut pen, [a[0] + 0.2, a[1] + 0.15], 1, "3D VIEW FROM SOUTH-EAST", None);
    Sheet { number, title: "3D VIEWS".into(), disc: Some(Disc::Arch), level: None, view: "3d", scale: None, pen }
}

fn cover_sheet(info: &Info, sheets: &[Sheet], paper: &Paper, discs: &BTreeSet<Disc>) -> Sheet {
    let a = paper.area;
    let mut pen = Pen::default();
    pen.layer("G-ANNO-TEXT");
    let x = a[0] + 0.6;
    let mut y = a[3] - 1.2;
    pen.text([x, y], &info.name.to_ascii_uppercase(), 0.6);
    y -= 0.6;
    for line in [&info.address, &info.client, &info.firm].into_iter().filter(|t| !t.is_empty()) {
        pen.text([x, y], &line.to_ascii_uppercase(), 0.2);
        y -= 0.35;
    }
    if !info.number.is_empty() {
        pen.text([x, y], &format!("PROJECT NO. {}", info.number), 0.16);
        y -= 0.3;
    }
    if !info.date.is_empty() {
        pen.text([x, y], &format!("DATE: {}", info.date), 0.16);
        y -= 0.3;
    }
    let general = [
        "THESE DRAWINGS ARE GENERATED FROM THE BUILDING MODEL; THE MODEL'S ELEMENTS AND THEIR PROPERTIES ARE THE SOURCE OF EVERY TAG, DIMENSION AND SCHEDULE.".to_string(),
        "DIMENSIONS ARE TO WALL CENTERLINES UNLESS NOTED.".into(),
        "VERIFY ALL DIMENSIONS AND CONDITIONS IN THE FIELD BEFORE STARTING WORK.".into(),
        format!("DISCIPLINES: {}", discs.iter().map(|d| d.name().to_ascii_uppercase()).collect::<Vec<_>>().join(", ")),
    ];
    let _ = list(&mut pen, x, y - 0.5, "GENERAL NOTES", &general, 70, a[1] + 1.2);
    // The sheet index: this sheet first, then every other.
    let mut rows = vec![vec!["G001".to_string(), "COVER SHEET AND SHEET INDEX".to_string()]];
    rows.extend(sheets.iter().map(|sh| vec![sh.number.clone(), sh.title.clone()]));
    let cols = vec!["sheet".to_string(), "title".to_string()];
    let ix = (a[0] + a[2]) / 2.0 + 0.5;
    let row_h = TABLE_TEXT * 5.0 / 3.0 + 0.08;
    let per = ((((a[3] - 0.5) - (a[1] + 1.0)) / row_h).floor() as usize).saturating_sub(3).max(5);
    let mut tx = ix;
    for (i, chunk) in rows.chunks(per).enumerate() {
        let title = if i == 0 { "SHEET INDEX" } else { "SHEET INDEX (CONT.)" };
        let (t, w, _) = table(title, &cols, chunk, [tx, a[3] - 0.5]);
        pen.layer("G-ANNO-SCHD");
        pen.table(t);
        pen.count("index_tables");
        tx += w + 0.3;
    }
    pen.count("index_rows");
    view_title(&mut pen, [a[0] + 0.2, a[1] + 0.15], 1, "COVER SHEET AND SHEET INDEX", None);
    Sheet { number: "G001".into(), title: "COVER SHEET AND SHEET INDEX".into(), disc: None, level: None, view: "cover", scale: None, pen }
}

// ─── On the layouts ─────────────────────────────────────────────────────────

fn layer_color(name: &str) -> u8 {
    let up = name.to_ascii_uppercase();
    if up.contains("BKGD") {
        return 8;
    }
    if up.contains("-IDEN") || up.contains("-DIMS") || up.contains("-KEYN") || up.contains("-SCHD") || up.starts_with("G-") {
        return 7;
    }
    match up.split('-').next().unwrap_or("") {
        "A" => 7,
        "S" => 8,
        "E" if up.starts_with("E-LITE") => 2,
        "E" if up.starts_with("E-SYST") => 3,
        "E" => 1,
        "M" => 4,
        "P" => 5,
        "F" => 1,
        _ => 7,
    }
}

fn layout_name(number: &str, title: &str) -> String {
    let n: String = format!("{number} {title}")
        .chars()
        .map(|c| if super::layout::BAD_CHARS.contains(&c) || c.is_control() { '-' } else { c })
        .take(120)
        .collect();
    n.trim().to_string()
}

/// The layouts a set drew before: any with an entity tagged [`APP`].
fn made_before(d: &cadaddy_doc::Drawing) -> Vec<String> {
    d.layouts
        .iter()
        .filter(|l| l.entities.iter().take(50).any(|e| e.common.xdata.iter().any(|t| t.code == 1001 && t.value == APP)))
        .map(|l| l.name.clone())
        .collect()
}

fn place_all(s: &mut Session, p: &Value, sheets: Vec<Sheet>, paper: &Paper, tb: Option<&str>, info: &Info, mut notes: Vec<String>) -> Result<Value> {
    let before = made_before(s.doc()?);
    let mut out = Vec::new();
    let mut names = Vec::new();
    for sh in &sheets {
        let name = layout_name(&sh.number, &sh.title);
        let layout = place(s, sh, &name, paper, tb, info, &mut notes)?;
        out.push(json!({
            "number": sh.number, "title": sh.title, "layout": layout, "discipline": sh.disc.map(Disc::code), "level": sh.level,
            "view": sh.view, "scale": sh.scale.map(views::scale_label), "counts": sh.pen.counts,
        }));
        names.push(layout);
    }
    // The sheets the set drew before and doesn't now.
    let mut removed = Vec::new();
    for old in before.iter().filter(|o| !names.contains(o)) {
        if s.execute("layout.delete", &json!({ "name": old })).is_ok() {
            removed.push(old.clone());
        }
    }
    if let Some(first) = names.first() {
        s.execute("layout.set", &json!({ "name": first }))?;
    }
    let mut files = Vec::new();
    if let Some(path) = str_param(p, "pdf").map(str::trim).filter(|x| !x.is_empty()) {
        if bool_or(p, "split", false) {
            for (sh, layout) in sheets.iter().zip(&names) {
                let one = numbered(path, &sh.number);
                s.execute("exportpdf", &json!({ "path": one, "layouts": [layout] }))?;
                files.push(one);
            }
        } else {
            s.execute("exportpdf", &json!({ "path": path, "layouts": names }))?;
            files.push(path.to_string());
        }
    }
    if let Some(path) = str_param(p, "dwg").map(str::trim).filter(|x| !x.is_empty()) {
        s.execute("export", &json!({ "path": path, "format": "dwg" }))?;
        files.push(path.to_string());
    }
    let index: Vec<Value> = sheets.iter().map(|sh| json!([sh.number, sh.title])).collect();
    s.echo(format!("Drawing set: {} sheets{}.", sheets.len(), if files.is_empty() { String::new() } else { format!(", {}", files.join(", ")) }));
    notes.dedup();
    Ok(json!({ "sheets": out, "index": index, "files": files, "removed": removed, "notes": notes }))
}

/// `set.pdf` → `set-A101.pdf`.
fn numbered(path: &str, number: &str) -> String {
    let p = std::path::Path::new(path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("sheet");
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("pdf");
    let file = format!("{stem}-{number}.{ext}");
    match p.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => dir.join(file).to_string_lossy().into_owned(),
        None => file,
    }
}

fn place(s: &mut Session, sh: &Sheet, name: &str, paper: &Paper, tb: Option<&str>, info: &Info, notes: &mut Vec<String>) -> Result<String> {
    let existing = s.doc()?.layout(name).map(|l| l.name.clone());
    let layout = match existing {
        Some(old) => {
            if let Some(l) = s.doc_mut()?.layouts.iter_mut().find(|l| l.name == old) {
                l.entities = cadaddy_doc::EntityStore::default();
            }
            s.state_mut()?.selection.clear();
            old
        }
        None => {
            let made = s.execute("layout.new", &json!({ "name": name, "viewport": false }))?;
            made["name"].as_str().unwrap_or(name).to_string()
        }
    };
    s.execute("layout.set", &json!({ "name": layout }))?;
    s.execute(
        "pagesetup",
        &json!({ "layout": layout, "width": paper.w * 25.4, "height": paper.h * 25.4, "margins": [0, 0, 0, 0], "scale": 1, "center": false, "landscape": paper.w > paper.h }),
    )?;
    let tag = vec![RawTag { code: 1001, value: APP.into() }, RawTag { code: 1000, value: sh.number.clone() }];
    {
        let d = s.doc_mut()?;
        let layers: BTreeSet<&str> = sh.pen.shapes.iter().map(|(l, _)| l.as_str()).chain(["G-ANNO-TTLB"]).collect();
        for l in layers {
            if d.ensure_layer(l)
                && let Some(layer) = d.layer_mut(l)
            {
                layer.color = cadaddy_doc::color::Color::Index(layer_color(l));
            }
        }
    }
    // The title block, filled in.
    s.execute("layer.current", &json!({ "name": "G-ANNO-TTLB" }))?;
    let attribs = json!({
        "FIRM": info.firm, "PROJECT": info.name, "SHEET_TITLE": sh.title, "PROJECT_NO": info.number, "DATE": info.date,
        "SCALE": sh.scale.map(views::scale_label).unwrap_or_else(|| "AS NOTED".into()), "SHEET_NO": sh.number,
    });
    let placed = match tb {
        Some(b) => s.execute("insert", &json!({ "name": b, "at": [0, 0], "attribs": attribs })),
        None => s.execute("part", &json!({ "name": paper.block, "at": [0, 0], "scale": 1, "attribs": attribs, "layer": "current" })),
    };
    let tb_handle = match placed {
        Ok(v) => v["handle"].as_str().map(str::to_string),
        Err(e) => {
            notes.push(format!("{}: no title block ({e})", sh.number));
            None
        }
    };
    {
        let space = Space::Paper(layout.clone());
        let d = s.doc_mut()?;
        // The title block carries the tag too, so a layout is known as the set's by any entity.
        if let Some(h) = tb_handle.as_deref().and_then(cadaddy_doc::Handle::parse_hex)
            && let Some(l) = d.layouts.iter_mut().find(|l| l.name == layout)
        {
            l.entities.modify(h, |e| e.common.xdata = tag.clone().into());
        }
        // Sheet number and title in the strip, also as plain text (a title block without
        // those fields still says which sheet it is).
        let mut extra = Pen::default();
        extra.layer("G-ANNO-TTLB");
        if tb.is_some() {
            extra.text([paper.area[2] + 0.5, 1.0], &sh.number, 0.4);
            extra.text([paper.area[2] + 0.5, 1.6], &sh.title, 0.12);
        }
        let mut shapes: Vec<(String, Shape)> = sh.pen.shapes.clone();
        shapes.extend(extra.shapes);
        // The dimensions it leaves go through DIMLINEAR below.
        let _ = pen::put(d, &space, &shapes, &tag);
    }
    // The dimensions, through DIMLINEAR (its style, its block).
    let dims: Vec<&(String, Shape)> = sh.pen.shapes.iter().filter(|(_, s)| matches!(s, Shape::Dim { .. })).collect();
    let mut current = String::new();
    for (layer, d) in dims {
        let Shape::Dim { a, b, at, text, vertical } = d else { continue };
        if *layer != current {
            s.execute("layer.current", &json!({ "name": layer }))?;
            layer.clone_into(&mut current);
        }
        if let Ok(r) = s.execute("dimlinear", &json!({ "p1": a, "p2": b, "at": at, "text": text, "rotation": if *vertical { 90 } else { 0 } }))
            && let Some(h) = r["handle"].as_str().and_then(cadaddy_doc::Handle::parse_hex)
            && let Some(l) = s.doc_mut()?.layouts.iter_mut().find(|l| l.name == layout)
        {
            l.entities.modify(h, |e| e.common.xdata = tag.clone().into());
        }
    }
    s.execute("layer.current", &json!({ "name": "0" }))?;
    Ok(layout)
}
