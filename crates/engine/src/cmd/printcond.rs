//! Print-set conduit and circuits (Q6007): what a job's electrical plans say about the wiring,
//! and an estimated route for every circuit they don't draw.
//!
//! Each electrical plan of the set is read on its own (cut out of the job's drawing as the
//! duct and pipe readers cut theirs, [`super::printmep::sheet_drawing`]):
//!
//! - **Drawn conduit**: a long drawn line (4' and up) with a conduit tag beside it (`3/4"C`,
//!   `1" EMT`, [`cadaddy_mepfix::conduit_reader::parse_conduit_tag`]); a `BOC @ 9'-8"` note by
//!   the tag gives its height. A print's lines carry no layer, so an untagged line (a wall, a
//!   grid line) is never taken for conduit.
//! - **Homeruns**: a short slanted line whose tip is at a circuit label (`D-7`, `LA-1,3,5`,
//!   `L1/1`): the circuit leaves the device at its other end for the panel the label names. A
//!   conductor note by the label (`2#12, 1#12G`) is its makeup; else the tick marks across the
//!   arrow count its conductors.
//! - **Panels**: `PANEL LA` lettering, or a panel's name lettered by a box on the plan.
//!
//! Where conduit is not drawn (typical for branch circuits) each circuit gets an **estimated
//! route**: the devices printing that circuit on a sheet, chained nearest-neighbour from the
//! homerun's device (else from the one nearest the panel), each leg run square (along x, then
//! y) just above the ceiling, a drop to each wall device at its usual mounting height, and the
//! home run from the first device to the panel when the level's plans show it. Every estimated
//! piece says so (`height_source`/`route`: `estimated route`), with the trade's usual size
//! (3/4" EMT, `2#12 + 1#12G`) marked as a default; nothing estimated is presented as read.
//!
//! The runs go into the building spec's MEP runs (system [`CIRCUIT_SYSTEM`] for estimated
//! routes, [`DRAWN_SYSTEM`] for drawn conduit) and each circuit is a `circuit` tag whose id the
//! runs carry (`<tag id>#<n>`), so the takeoff counts conduit, wire, boxes and fittings by level
//! and circuit ([`super::bom`]).

use std::collections::BTreeMap;

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_mepfix::conduit_reader::{DEFAULT_SIZE_IN, parse_boc, parse_conduit_tag};
use cadaddy_walk3d::tags::ElementTag;
use cadaddy_walk3d::{BuildingSpec, Mep, MepKind, MepRun, MepSystem};
use serde_json::{Value, json};

use super::printmep::{SheetRef, level_name, r1, sheet_drawing};
use super::readers::ReadDevice;

/// The system estimated circuit routes go into the spec as.
pub(crate) const CIRCUIT_SYSTEM: &str = "E-CIRCUIT-EST";
/// The system drawn (tagged) branch conduit goes into the spec as.
pub(crate) const DRAWN_SYSTEM: &str = cadaddy_mepfix::conduit_reader::CONDUIT_SYSTEM;
/// What an estimated piece says it is.
pub(crate) const ESTIMATED: &str = "estimated route";
/// A branch circuit's conductors when nothing on the plan says otherwise.
pub(crate) const DEFAULT_CONDUCTORS: &str = "2#12 + 1#12G";
/// Circuit tag ids start with this.
pub(crate) const CIRCUIT_TAG: &str = "circuit:";
/// A riser run's id ends with this (the home run up or down to a panel on another level).
pub(crate) const RISER_SUFFIX: &str = "#riser";
/// What a circuit no plan numbers is called (`unnumbered 3`).
pub(crate) const UNNUMBERED: &str = "unnumbered";
/// Devices on one estimated circuit when their circuit is not printed.
const DEVICES_PER_UNNUMBERED: usize = 8;
/// A floor-to-floor height until the spec has its levels (a riser's length a floor), feet.
const DEFAULT_FLOOR_FT: f64 = 10.0;

/// A level's ceiling when the spec has none yet, feet; conduit runs this far above it.
const DEFAULT_CEILING_FT: f64 = 8.0;
const ABOVE_CEILING_FT: f64 = 0.25;
/// Lines at least this long (feet) can be drawn conduit.
const MIN_RUN_FT: f64 = 4.0;
/// A conduit tag this close (feet) beside a line is its tag; a `BOC` note this close to the tag.
const TAG_FT: f64 = 1.5;
const BOC_FT: f64 = 2.0;
/// A homerun arrow's shaft, feet.
const ARROW_FT: std::ops::RangeInclusive<f64> = 0.25..=6.0;
/// A device this close (feet) to drawn conduit is on it (its circuit needs no estimate).
const ON_CONDUIT_FT: f64 = 1.5;
/// Most objects looked at per sheet, sheets read, devices routed, runs made (hostile sets).
const MAX_ITEMS: usize = 200_000;
const MAX_SHEETS: usize = 500;
const MAX_DEVICES: usize = 100_000;
const MAX_RUNS: usize = 50_000;

/// Where a wall device's box usually is, feet above the floor (none: at the ceiling, no drop).
pub(crate) fn mount_height_ft(kind: &str) -> Option<f64> {
    match kind.replace('_', " ").as_str() {
        "switch" => Some(4.0),
        "receptacle" | "low voltage outlet" | "data outlet" => Some(1.5),
        "fire alarm device" => Some(80.0 / 12.0),
        "door holder" => Some(7.0),
        _ => None,
    }
}

/// Words lettered before a dash that are no panel's name (`NOTE-1`, `TYPE-2`).
const NOT_PANELS: &[&str] =
    &["PANEL", "NOTE", "NOTES", "SHEET", "TYPE", "LEVEL", "ROOM", "DETAIL", "SEE", "KEY", "PLAN", "AREA", "ZONE", "UNIT", "DWG"];

/// A circuit label's panel and circuit numbers: `D-7`, `LA-1,3,5`, `LP1-12`, `L1/1`. A sheet
/// number (`E-101`: no panel has circuit 101) or a dimension is not one.
pub(crate) fn parse_circuit(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let (panel, ckt) = s.split_once('/').or_else(|| s.split_once('-'))?;
    let panel = panel.trim().to_ascii_uppercase();
    let ok_panel = (1..=6).contains(&panel.len())
        && panel.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && panel.chars().all(|c| c.is_ascii_alphanumeric())
        && !NOT_PANELS.contains(&panel.as_str());
    let nums: Vec<&str> = ckt.split(',').map(str::trim).collect();
    let ok_ckt = !nums.is_empty()
        && nums.len() <= 3
        && nums
            .iter()
            .all(|n| (1..=2).contains(&n.len()) && n.chars().all(|c| c.is_ascii_digit()) && n.parse::<u32>().is_ok_and(|v| (1..=84).contains(&v)));
    (ok_panel && ok_ckt).then(|| (panel, nums.join(",")))
}

/// A conductor note as the takeoff reads it: `2#12, 1#12G` → `2#12 + 1#12G`.
fn conductor_note(s: &str) -> Option<String> {
    let t = s.trim().to_ascii_uppercase();
    if !t.contains('#') || t.len() > 40 || !t.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let parts: Vec<String> = t.split([',', '+']).map(str::trim).filter(|p| p.contains('#')).map(str::to_string).collect();
    (!parts.is_empty()).then(|| parts.join(" + "))
}

/// A panel named by lettering: `PANEL LA`, `PNL 'D'`, `PANELBOARD LP1`.
fn panel_lettering(s: &str) -> Option<String> {
    let u = s.trim().to_ascii_uppercase();
    let rest = ["PANELBOARD", "PANEL", "PNL"].iter().find_map(|p| u.strip_prefix(p))?;
    let name: String = rest.trim().trim_start_matches(':').trim().trim_matches(['\'', '"']).to_string();
    let ok = (1..=6).contains(&name.len())
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    ok.then_some(name)
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Distance from `p` to segment `ab`, and where along it (0–1) the nearest point is.
fn beside(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> (f64, f64) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 1e-12 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (dist(p, [a[0] + t * d[0], a[1] + t * d[1]]), t)
}

/// Whether segments `ab` and `cd` cross.
fn crosses(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let side = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let (d1, d2) = (side(a, b, c), side(a, b, d));
    let (d3, d4) = (side(c, d, a), side(c, d, b));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// A text of a sheet: value, its middle (roughly, for unrotated lettering), height.
#[derive(Clone, Debug)]
struct Lettering {
    value: String,
    mid: [f64; 2],
    height: f64,
}

/// A homerun arrow read on a sheet.
#[derive(Clone, Debug)]
struct Homerun {
    key: (String, String),
    origin: [f64; 2],
    /// The makeup and where it came from (`note`, `rule`: counted ticks).
    conductors: Option<(String, &'static str)>,
}

/// What one electrical plan shows, drawing units.
#[derive(Debug, Default)]
struct SheetScan {
    /// Drawn conduit: ends, size (inches) from its tag, the bottom a `BOC` note gives (feet).
    conduits: Vec<([f64; 2], [f64; 2], f64, Option<f64>)>,
    homeruns: Vec<Homerun>,
    /// Circuit labels lettered on the plan: text, middle, height.
    marks: Vec<(String, [f64; 2], f64)>,
    /// Panels by name.
    panels: BTreeMap<String, [f64; 2]>,
}

/// The lettering of a sheet (deduplicated: a print's text can come twice, its hidden text and
/// a copy read off the strokes).
fn letterings(d: &Drawing) -> Vec<Lettering> {
    let mut out: Vec<Lettering> = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        let (value, at, height) = match &e.kind {
            EntityKind::Text(t) => (t.value.trim().to_string(), t.align_pt.unwrap_or(t.insert), t.height),
            EntityKind::MText(t) => (t.contents.trim().to_string(), t.insert, t.height),
            _ => continue,
        };
        if value.is_empty() || !(height.is_finite() && height > 0.0) || !(at.x.is_finite() && at.y.is_finite()) {
            continue;
        }
        let aligned = matches!(&e.kind, EntityKind::Text(t) if t.align_pt.is_some());
        // An unaligned text's insert is its lower left corner.
        let mid = if aligned { [at.x, at.y] } else { [at.x + 0.3 * height * value.chars().count() as f64, at.y + 0.5 * height] };
        if out.iter().any(|o| o.value == value && dist(o.mid, mid) < 0.75 * height) {
            continue;
        }
        out.push(Lettering { value, mid, height });
    }
    out
}

/// Closed four-sided outlines' centres and sizes (panels are drawn as boxes).
fn boxes(d: &Drawing) -> Vec<([f64; 2], f64)> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        let pts: Vec<[f64; 2]> = match &e.kind {
            EntityKind::LwPolyline(p) if p.closed || p.vertices.len() == 5 => p.vertices.iter().map(|v| [v.p.x, v.p.y]).collect(),
            EntityKind::Solid(s) => s.corners.iter().map(|c| [c.x, c.y]).collect(),
            EntityKind::Hatch(_) => continue,
            _ => continue,
        };
        if !(4..=5).contains(&pts.len()) {
            continue;
        }
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in &pts {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
        let size = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        if size.is_finite() && size > 0.0 {
            out.push(([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0], size));
        }
    }
    out
}

/// The lines of the symbol blocks a print's import made of its repeated drawings (`PDF-SYM-*`:
/// device symbols, homerun arrows), where each insert puts them.
fn symbol_segments(d: &Drawing) -> Vec<[[f64; 2]; 2]> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        let EntityKind::Insert(ins) = &e.kind else { continue };
        let Some(b) = d.blocks.get(&ins.block).filter(|_| ins.block.starts_with("PDF-SYM-")) else { continue };
        let (c, sn) = (ins.rotation.cos(), ins.rotation.sin());
        let place = |x: f64, y: f64| {
            let (x, y) = ((x - b.base.x) * ins.scale.x, (y - b.base.y) * ins.scale.y);
            [ins.insert.x + x * c - y * sn, ins.insert.y + x * sn + y * c]
        };
        for be in b.entities.iter().take(64) {
            match &be.kind {
                EntityKind::Line(l) => out.push([place(l.a.x, l.a.y), place(l.b.x, l.b.y)]),
                EntityKind::LwPolyline(pl) => {
                    let n = pl.vertices.len();
                    let sides = if pl.closed { n } else { n.saturating_sub(1) };
                    for i in 0..sides.min(64) {
                        if let (Some(a), Some(b)) = (pl.vertices.get(i), pl.vertices.get((i + 1) % n)) {
                            out.push([place(a.p.x, a.p.y), place(b.p.x, b.p.y)]);
                        }
                    }
                }
                _ => {}
            }
        }
        if out.len() >= MAX_ITEMS {
            break;
        }
    }
    out.retain(|s| s.iter().all(|p| p[0].is_finite() && p[1].is_finite()));
    out
}

/// Read one electrical plan (`upf` drawing units per foot): its drawn conduit, homerun arrows
/// and panels. `wanted_panels`: the panel names the devices' circuits on it name.
fn scan(d: &Drawing, upf: f64, wanted_panels: &[String]) -> SheetScan {
    let mut out = SheetScan::default();
    let texts = letterings(d);
    let mut segs: Vec<[[f64; 2]; 2]> = cadaddy_mepfix::sheet_reader::drawn_segments(d, true).into_iter().take(MAX_ITEMS).collect();
    // Conduit is drawn loose; arrows can come in as symbols (the same drawing repeated).
    let loose = segs.len();
    segs.extend(symbol_segments(d).into_iter().take(MAX_ITEMS));
    let len = |s: &[[f64; 2]; 2]| dist(s[0], s[1]);
    // Drawn conduit: a long line with a conduit tag beside it.
    let tags: Vec<(&Lettering, f64)> = texts.iter().filter_map(|t| parse_conduit_tag(&t.value).map(|s| (t, s))).collect();
    if !tags.is_empty() {
        for s in segs.iter().take(loose).filter(|s| len(s) >= MIN_RUN_FT * upf) {
            let tag = tags
                .iter()
                .map(|(t, size)| (*t, *size, beside(s[0], s[1], t.mid)))
                .filter(|(t, _, (dd, at))| *dd <= (TAG_FT * upf).max(2.0 * t.height) && *at > 0.0 && *at < 1.0)
                .min_by(|a, b| a.2.0.total_cmp(&b.2.0));
            let Some((t, size, _)) = tag else { continue };
            let boc = texts.iter().filter(|n| dist(n.mid, t.mid) <= BOC_FT * upf).find_map(|n| parse_boc(&n.value));
            out.conduits.push((s[0], s[1], size, boc));
        }
    }
    // Homeruns: a short slanted line whose tip is at a circuit label.
    for t in &texts {
        let Some(key) = parse_circuit(&t.value) else { continue };
        let reach = 0.35 * t.height * t.value.chars().count() as f64 + 1.5 * t.height;
        let arrow = segs
            .iter()
            .filter(|s| ARROW_FT.contains(&(len(s) / upf)))
            .filter(|s| {
                let (dx, dy) = ((s[1][0] - s[0][0]).abs(), (s[1][1] - s[0][1]).abs());
                dx.min(dy) > 0.18 * dx.max(dy)
            })
            .filter_map(|s| {
                let (ta, tb) = (dist(s[0], t.mid), dist(s[1], t.mid));
                let (tip, origin, near, far) = if ta <= tb { (s[0], s[1], ta, tb) } else { (s[1], s[0], tb, ta) };
                (near <= reach && far > near + 1.5 * t.height).then_some((near, tip, origin))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let note = texts.iter().filter(|n| dist(n.mid, t.mid) <= 3.0 * t.height).find_map(|n| conductor_note(&n.value)).map(|n| (n, "note"));
        // Tick marks: short strokes across the shaft.
        let ticks = |tip: [f64; 2], origin: [f64; 2]| {
            let n = segs.iter().filter(|s| (0.4 * t.height..=3.0 * t.height).contains(&len(s)) && crosses(s[0], s[1], tip, origin)).count();
            (n >= 2).then(|| (format!("{n}#12 + 1#12G"), "rule"))
        };
        let (origin, conductors) = match arrow {
            Some((_, tip, origin)) => (origin, note.or_else(|| ticks(tip, origin))),
            // No arrow (an import can take a short arrow by its label for lettering): a circuit
            // label with its conductor note under it is a homerun's all the same, by the label.
            None => match note {
                Some(n) => (t.mid, Some(n)),
                None => continue,
            },
        };
        if out.homeruns.iter().any(|h: &Homerun| h.key == key && dist(h.origin, origin) < 3.0 * t.height) {
            continue;
        }
        out.homeruns.push(Homerun { key, origin, conductors });
    }
    // Every circuit lettered on the plan (beside a device, at a homerun), for devices the
    // devices stage gave none.
    out.marks = texts.iter().filter(|t| parse_circuit(&t.value).is_some()).map(|t| (t.value.clone(), t.mid, t.height)).collect();
    // Panels: lettered `PANEL LA`, or a circuit's panel name (the devices' or a homerun's) by
    // a box.
    let bx = boxes(d);
    let lettered: Vec<String> = out.marks.iter().filter_map(|(m, _, _)| parse_circuit(m)).map(|(p, _)| p).collect();
    for t in &texts {
        let named = panel_lettering(&t.value).or_else(|| {
            let v = t.value.trim().to_ascii_uppercase();
            (wanted_panels.contains(&v) || lettered.contains(&v)).then_some(v)
        });
        let Some(name) = named else { continue };
        let lettered = panel_lettering(&t.value).is_some();
        let near_box = bx
            .iter()
            .filter(|(c, size)| *size <= 12.0 * upf && dist(*c, t.mid) <= (*size).max(3.0 * t.height))
            .min_by(|a, b| dist(a.0, t.mid).total_cmp(&dist(b.0, t.mid)));
        let at = match near_box {
            Some((c, _)) => *c,
            None if lettered => t.mid,
            None => continue,
        };
        out.panels.entry(name).or_insert(at);
    }
    out
}

/// The square route between two points: along x, then y.
fn square(a: [f64; 2], b: [f64; 2], eps: f64) -> Vec<[f64; 2]> {
    let turn = [b[0], a[1]];
    if dist(turn, a) < eps || dist(turn, b) < eps { vec![a, b] } else { vec![a, turn, b] }
}

/// Ids without the characters the model's run names split on.
fn id_part(s: &str) -> String {
    s.trim().chars().map(|c| if c == '/' || c == '#' || c == '@' { '-' } else { c }).collect()
}

/// What one electrical plan read.
#[derive(Clone, Debug, Default)]
pub(crate) struct CondSheet {
    pub sheet: String,
    pub level: String,
    pub conduits: usize,
    pub conduit_ft: f64,
    pub homeruns: usize,
    pub panels: Vec<String>,
    pub circuits: usize,
    pub estimated_ft: f64,
}

/// What a set's electrical plans read: the runs, one tag per circuit, sheet by sheet.
#[derive(Clone, Debug, Default)]
pub(crate) struct CondRead {
    pub mep: Mep,
    pub circuits: Vec<ElementTag>,
    pub sheets: Vec<CondSheet>,
    /// Circuits whose home run could not be routed: `{circuit, sheet, level, reason}`.
    pub unrouted: Vec<Value>,
    /// Devices with no circuit printed beside them (lighting and power ones are routed on
    /// unnumbered circuits).
    pub no_circuit: usize,
    /// Conduit feet on each level (drawn and estimated).
    pub by_level: BTreeMap<String, f64>,
    pub warnings: Vec<String>,
}

impl CondRead {
    pub fn drawn_ft(&self) -> f64 {
        self.sheets.iter().map(|s| s.conduit_ft).sum()
    }
    pub fn estimated_ft(&self) -> f64 {
        self.sheets.iter().map(|s| s.estimated_ft).sum()
    }
    /// How many levels have any conduit.
    pub fn levels_with_conduit(&self) -> usize {
        self.by_level.values().filter(|ft| **ft > 0.0).count()
    }
    pub fn to_json(&self) -> Value {
        let drops = self.mep.runs.iter().filter(|r| r.top_ft.is_some()).count();
        json!({
            "conduit_runs": self.sheets.iter().map(|s| s.conduits).sum::<usize>(),
            "conduit_ft": r1(self.drawn_ft()),
            "homeruns": self.sheets.iter().map(|s| s.homeruns).sum::<usize>(),
            "circuits": self.circuits.len(),
            "estimated_ft": r1(self.estimated_ft()),
            "drops": drops,
            "devices_without_circuit": self.no_circuit,
            "levels": self.by_level.iter().map(|(l, ft)| json!({"level": l, "conduit_ft": r1(*ft)})).collect::<Vec<_>>(),
            "levels_with_conduit": self.levels_with_conduit(),
            "unrouted": self.unrouted,
            "sheets": self.sheets.iter().map(|s| json!({
                "sheet": s.sheet, "level": s.level, "conduit_runs": s.conduits, "conduit_ft": r1(s.conduit_ft),
                "homeruns": s.homeruns, "panels": s.panels, "circuits": s.circuits, "estimated_ft": r1(s.estimated_ft),
            })).collect::<Vec<_>>(),
            "warnings": self.warnings,
        })
    }
}

fn run(system: &str, level: &str, sheet: &str, points: Vec<[f64; 2]>, size_in: f64, bottom: f64) -> MepRun {
    let segs = points.len().saturating_sub(1).max(1);
    MepRun {
        system: system.into(),
        level: level.into(),
        points,
        size_in: [size_in, 0.0],
        bottoms_ft: vec![bottom; segs],
        objects: Vec::new(),
        id: String::new(),
        encased: false,
        size_source: "default".into(),
        height_source: ESTIMATED.into(),
        reviewed: false,
        edited_by: String::new(),
        top_ft: None,
        source_sheet: sheet.into(),
    }
}

fn system(name: &str, bottom: f64) -> MepSystem {
    MepSystem {
        name: name.into(),
        layer: name.into(),
        kind: MepKind::Conduit,
        size_in: [DEFAULT_SIZE_IN, 0.0],
        bottom_ft: bottom,
        show: true,
        duct_type: None,
        color_index: 0,
    }
}

/// Whether `dv` was read on `sh`: named for it (a sheet split into one view per level keeps its
/// devices under the sheet's own number, `E-201` for `E-201/3`) and, on a view, inside it.
fn on_sheet(dv: &ReadDevice, sh: &SheetRef) -> bool {
    let inside = |r: &[f64; 4]| dv.at[0] >= r[0].min(r[2]) && dv.at[0] <= r[0].max(r[2]) && dv.at[1] >= r[1].min(r[3]) && dv.at[1] <= r[1].max(r[3]);
    let named = if dv.sheet.is_empty() {
        inside(&sh.rect)
    } else {
        let d = dv.sheet.trim();
        d.eq_ignore_ascii_case(&sh.number) || sh.number.split_once('/').is_some_and(|(base, _)| d.eq_ignore_ascii_case(base))
    };
    named && sh.view.as_ref().is_none_or(inside)
}

/// Every level a sheet's plan is on (a typical plan: each level it shows), by model name.
fn sheet_levels(sh: &SheetRef) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in std::iter::once(&sh.level).chain(sh.levels.iter()) {
        let n = level_name(l);
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&n)) {
            out.push(n);
        }
    }
    out
}

/// The level a device is on: its own when its sheet shows that level (a typical plan's device
/// is read once per level it shows), else its sheet's.
fn device_level(dv: &ReadDevice, sh: &SheetRef) -> String {
    let own = level_name(&dv.level);
    if !dv.level.trim().is_empty() && sheet_levels(sh).iter().any(|l| l.eq_ignore_ascii_case(&own)) { own } else { level_name(&sh.level) }
}

/// A level's number (`Level 3` → 3), for a riser's floors; none for a named level (`Roof`).
fn level_rank(name: &str) -> Option<f64> {
    let t = name.trim();
    let n = t.get(..6).filter(|p| p.eq_ignore_ascii_case("level ")).map_or(t, |_| t.get(6..).unwrap_or_default());
    n.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Where a sheet's plan is drawn: its view, else the sheet.
fn frame(sh: &SheetRef) -> [f64; 4] {
    let r = sh.view.unwrap_or(sh.rect);
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

/// Whether a device of this kind is on a lighting or power branch circuit (fire alarm, low
/// voltage and door holders are their own systems).
fn on_branch_circuit(kind: &str) -> bool {
    !matches!(kind.replace('_', " ").as_str(), "fire alarm device" | "low voltage outlet" | "data outlet" | "door holder")
}

/// A circuit's panel as found: where it is on the circuit's sheet (on another level's plan: the
/// same place, where the riser goes up or down to it), its name, the sheet it is lettered on,
/// and its level.
struct PanelAt {
    name: String,
    at: [f64; 2],
    sheet: String,
    level: String,
}

/// Find the panel `name` (none: the nearest panel to `near`) for a circuit on `level` of `sh`:
/// on this plan; else on another plan of the level drawn the same size (moved over by the
/// plans' corners); else the same way on another level's plan (a riser takes the home run there).
fn find_panel(scans: &[(&SheetRef, SheetScan)], sh: &SheetRef, sc: &SheetScan, level: &str, name: Option<&str>, near: [f64; 2]) -> Option<PanelAt> {
    let pick = |panels: &BTreeMap<String, [f64; 2]>, shift: [f64; 2]| -> Option<(String, [f64; 2])> {
        let moved = |p: &[f64; 2]| [p[0] + shift[0], p[1] + shift[1]];
        match name {
            Some(n) => panels.get(n).map(|p| (n.to_string(), moved(p))),
            None => panels.iter().map(|(k, p)| (k.clone(), moved(p))).min_by(|a, b| dist(a.1, near).total_cmp(&dist(b.1, near))),
        }
    };
    if let Some((n, at)) = pick(&sc.panels, [0.0, 0.0]) {
        return Some(PanelAt { name: n, at, sheet: sh.number.clone(), level: level.to_string() });
    }
    let f = frame(sh);
    let (w, h) = (f[2] - f[0], f[3] - f[1]);
    let same = |o: &SheetRef| {
        let of = frame(o);
        let (ow, oh) = (of[2] - of[0], of[3] - of[1]);
        (w > 0.0 && h > 0.0 && ((ow - w) / w).abs() < 0.02 && ((oh - h) / h).abs() < 0.02).then(|| [f[0] - of[0], f[1] - of[1]])
    };
    let others = || scans.iter().filter(|(o, _)| o.number != sh.number);
    // Another plan of this level first, then the other levels, nearest floor first.
    let here = others().filter(|(o, _)| sheet_levels(o).iter().any(|l| l.eq_ignore_ascii_case(level))).find_map(|(o, osc)| {
        let (n, at) = pick(&osc.panels, same(o)?)?;
        Some(PanelAt { name: n, at, sheet: o.number.clone(), level: level.to_string() })
    });
    if here.is_some() {
        return here;
    }
    let rank = level_rank(level);
    let floors = |l: &str| match (rank, level_rank(l)) {
        (Some(a), Some(b)) => (a - b).abs(),
        _ => f64::MAX,
    };
    others()
        .filter_map(|(o, osc)| {
            let (n, at) = pick(&osc.panels, same(o)?)?;
            let lv = sheet_levels(o).into_iter().filter(|l| !l.eq_ignore_ascii_case(level)).min_by(|a, b| floors(a).total_cmp(&floors(b)))?;
            Some(PanelAt { name: n, at, sheet: o.number.clone(), level: lv })
        })
        .min_by(|a, b| floors(&a.level).total_cmp(&floors(&b.level)).then(dist(a.at, near).total_cmp(&dist(b.at, near))))
}

/// The plan, level and reading a circuit is routed on.
struct RouteCtx<'a> {
    scans: &'a [(&'a SheetRef, SheetScan)],
    sh: &'a SheetRef,
    sc: &'a SheetScan,
    level: &'a str,
    upf: f64,
    bottom: f64,
}

/// Put one run into the read, counting its feet on its level; the feet it adds.
fn add_run(out: &mut CondRead, r: MepRun, upf: f64) -> f64 {
    if out.mep.runs.len() >= MAX_RUNS {
        return 0.0;
    }
    let ft = match r.top_ft {
        Some(top) => top - r.bottoms_ft.first().copied().unwrap_or(top),
        None => super::bom::run_length_ft(&r.points, upf),
    };
    let ft = if ft.is_finite() { ft.max(0.0) } else { 0.0 };
    *out.by_level.entry(r.level.clone()).or_default() += ft;
    out.mep.runs.push(r);
    ft
}

/// Route one circuit (`panel`: the panel its label names; none for an unnumbered one) through
/// `devs` on the context's plan and level: the chain, the home run to the panel (by a riser
/// when the panel is on another level), a drop to each wall device, and its tag. The feet added.
fn route_circuit(out: &mut CondRead, cx: &RouteCtx, panel: Option<&str>, label: &str, devs: &[&ReadDevice], homerun: Option<&Homerun>) -> f64 {
    let (level, sh, upf, bottom) = (cx.level, cx.sh, cx.upf, cx.bottom);
    let near = homerun.map(|h| h.origin).or_else(|| devs.first().map(|d| d.at)).unwrap_or_default();
    let panel_at = find_panel(cx.scans, sh, cx.sc, level, panel, near);
    // The chain: from the homerun's device, else the one nearest the panel, else the
    // lowest-leftmost; then always the nearest one left.
    let mut left: Vec<&ReadDevice> = devs.to_vec();
    let start_at = homerun.map(|h| h.origin).or_else(|| panel_at.as_ref().map(|p| p.at));
    let first = match start_at {
        Some(p) => left.iter().enumerate().min_by(|a, b| dist(a.1.at, p).total_cmp(&dist(b.1.at, p))).map(|(i, _)| i),
        None => left
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1.at[1], a.1.at[0]).partial_cmp(&(b.1.at[1], b.1.at[0])).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i),
    };
    let Some(first) = first else { return 0.0 };
    let mut chain = vec![left.swap_remove(first)];
    while !left.is_empty() {
        let Some(last) = chain.last().map(|d| d.at) else { break };
        let Some((i, _)) = left.iter().enumerate().min_by(|a, b| dist(a.1.at, last).total_cmp(&dist(b.1.at, last))) else { break };
        chain.push(left.swap_remove(i));
    }
    let Some(head) = chain.first().map(|d| d.at) else { return 0.0 };
    let eps = 0.05 * upf;
    let cid = format!("{CIRCUIT_TAG}{}:{}:{}", id_part(level), id_part(&sh.number), id_part(label));
    let mk = |points: Vec<[f64; 2]>, id: String| {
        let mut r = run(CIRCUIT_SYSTEM, level, &sh.number, points, DEFAULT_SIZE_IN, bottom);
        r.id = id;
        r
    };
    let mut ft = 0.0;
    // The home run (to where the riser is, when the panel is on another level), then the riser.
    let mut riser = None;
    if let Some(p) = &panel_at {
        ft += add_run(out, mk(square(head, p.at, eps), format!("{cid}#home")), upf);
        if !p.level.eq_ignore_ascii_case(level) {
            let floors = match (level_rank(level), level_rank(&p.level)) {
                (Some(a), Some(b)) => (a - b).abs().max(1.0),
                _ => 1.0,
            };
            let rise = floors * DEFAULT_FLOOR_FT;
            // It starts on the lower of the two levels and goes up.
            let below = if level_rank(&p.level).zip(level_rank(level)).is_some_and(|(a, b)| a < b) { p.level.as_str() } else { level };
            let mut r = run(CIRCUIT_SYSTEM, below, &sh.number, vec![p.at], DEFAULT_SIZE_IN, bottom);
            r.top_ft = Some(bottom + rise);
            r.id = format!("{cid}{RISER_SUFFIX}");
            ft += add_run(out, r, upf);
            riser = Some(rise);
        }
    }
    let mut legs = 0usize;
    for w in chain.windows(2) {
        if let [a, b] = w {
            legs += 1;
            ft += add_run(out, mk(square(a.at, b.at, eps), format!("{cid}#{legs}")), upf);
        }
    }
    // A drop to each wall device.
    let (mut drops, mut no_height) = (0usize, 0usize);
    for (i, dv) in chain.iter().enumerate() {
        let kind = if dv.kind.is_empty() { super::readers::device_kind(&dv.tag) } else { dv.kind.as_str() };
        match mount_height_ft(kind) {
            Some(m) if m < bottom => {
                drops += 1;
                let mut r = mk(vec![dv.at], format!("{cid}#d{i}"));
                r.bottoms_ft = vec![m];
                r.top_ft = Some(bottom);
                ft += add_run(out, r, upf);
            }
            Some(_) => {}
            None => no_height += 1,
        }
    }
    let (conductors, source) =
        homerun.and_then(|h| h.conductors.clone()).map_or_else(|| (DEFAULT_CONDUCTORS.to_string(), "default"), |(c, s)| (c, s));
    let mut t =
        ElementTag { id: cid, kind: "circuit".into(), level: level.to_string(), at_ft: [head[0] / upf, head[1] / upf], ..ElementTag::default() };
    t.set("circuit", label, if panel.is_some() { "plan" } else { "rule" });
    match (panel, &panel_at) {
        (Some(p), _) => t.set("panel", p, "plan"),
        (None, Some(p)) => t.set("panel", p.name.clone(), "rule"),
        (None, None) => {}
    }
    t.set("sheet", sh.number.clone(), "plan");
    t.set("devices", chain.len().to_string(), "computed");
    t.set("route", ESTIMATED, "rule");
    t.set("conductors", conductors, source);
    t.set("conduit", "3/4\" EMT", "default");
    t.set("homerun", homerun.map_or("not drawn", |_| "read"), if homerun.is_some() { "plan" } else { "rule" });
    t.set("drops", drops.to_string(), "computed");
    if no_height > 0 {
        t.set("drops_unknown", format!("{no_height} device(s) of no known mounting height: no drop counted"), "rule");
    }
    if panel.is_none() {
        t.issues.push("circuit_not_printed".into());
        t.set(
            "grouping",
            format!("no circuit printed: up to {DEVICES_PER_UNNUMBERED} neighbouring devices to a circuit, to the nearest panel"),
            "rule",
        );
    }
    match &panel_at {
        Some(p) => {
            t.set("panel_at", p.sheet.clone(), "plan");
            if let Some(rise) = riser {
                t.set("panel_level", p.level.clone(), "plan");
                t.set("riser", format!("{rise:.0} ft from {level} to {} ({DEFAULT_FLOOR_FT:.0} ft a floor)", p.level), "default");
            }
        }
        None => {
            t.issues.push("panel_not_found".into());
            let why = match panel {
                Some(p) => format!("panel {p} is not shown on the electrical plans: no home run counted"),
                None => "no panel is shown on the electrical plans: no home run counted".to_string(),
            };
            out.unrouted.push(json!({"circuit": label, "sheet": sh.number, "level": level, "reason": why}));
        }
    }
    out.circuits.push(t);
    ft
}

/// Read every electrical plan among `sheets` of `d` (`upf` drawing units per foot): drawn
/// conduit, homeruns and panels, and an estimated route for each circuit `devices` print, on
/// every level a plan's devices are on (a typical plan's on each level it shows). Devices whose
/// circuit is not printed (lighting and power) are routed too, a few neighbours to a circuit.
pub(crate) fn read_sheets(d: &Drawing, sheets: &[SheetRef], upf: f64, devices: &[ReadDevice]) -> CondRead {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let mut out = CondRead::default();
    let plans: Vec<&SheetRef> = sheets.iter().filter(|s| s.discipline.eq_ignore_ascii_case("E") && s.builds()).take(MAX_SHEETS).collect();
    let devices = &devices[..devices.len().min(MAX_DEVICES)];
    let bottom = DEFAULT_CEILING_FT + ABOVE_CEILING_FT;
    // Every plan first (a circuit's panel can be on another plan of its level, or another level).
    let mut scans: Vec<(&SheetRef, SheetScan)> = Vec::new();
    for sh in &plans {
        let sub = sheet_drawing(d, sh);
        if sub.model.is_empty() {
            out.warnings.push(format!("{}: no objects imported for this sheet", sh.number));
            continue;
        }
        if super::printmep::paper_units_per_foot(&sub, sh).is_some() {
            out.warnings.push(format!("{}: at paper size; its conduit and circuits were not read", sh.number));
            continue;
        }
        let mut wanted: Vec<String> =
            devices.iter().filter(|dv| on_sheet(dv, sh)).filter_map(|dv| parse_circuit(&dv.circuit)).map(|(p, _)| p).collect();
        wanted.sort();
        wanted.dedup();
        scans.push((sh, scan(&sub, upf, &wanted)));
    }
    for (sh, sc) in &scans {
        let primary = level_name(&sh.level);
        let levels = sheet_levels(sh);
        let mut read = CondSheet { sheet: sh.number.clone(), level: primary.clone(), ..CondSheet::default() };
        read.panels = sc.panels.keys().cloned().collect();
        read.homeruns = sc.homeruns.len();
        // Drawn conduit, on every level the plan shows.
        for level in &levels {
            for (i, (a, b, size, boc)) in sc.conduits.iter().enumerate() {
                let mut r = run(DRAWN_SYSTEM, level, &sh.number, vec![*a, *b], *size, boc.unwrap_or(bottom));
                r.id = format!("conduit:{}:{}#{i}", id_part(level), id_part(&sh.number));
                r.size_source = "tag".into();
                r.height_source = if boc.is_some() { "note".into() } else { "default".into() };
                read.conduit_ft += add_run(&mut out, r, upf);
                read.conduits += 1;
            }
        }
        // This sheet's devices by level, then by circuit: the one the devices stage read, else
        // the circuit lettered beside the device (within six letter heights).
        let mut by_level: BTreeMap<String, Vec<&ReadDevice>> = BTreeMap::new();
        for dv in devices.iter().filter(|dv| on_sheet(dv, sh)) {
            by_level.entry(device_level(dv, sh)).or_default().push(dv);
        }
        // A typical plan whose devices were read once: the same devices on each level it shows.
        if let Some(first) = by_level.get(&primary).cloned() {
            for l in levels.iter().skip(1) {
                by_level.entry(l.clone()).or_insert_with(|| first.clone());
            }
        }
        for (level, devs) in &by_level {
            let cx = RouteCtx { scans: &scans, sh, sc, level, upf, bottom };
            let mut by_circuit: BTreeMap<(String, String), (String, Vec<&ReadDevice>)> = BTreeMap::new();
            let mut unnumbered: Vec<&ReadDevice> = Vec::new();
            for dv in devs {
                let beside = || {
                    sc.marks
                        .iter()
                        .filter(|(_, at, h)| dist(*at, dv.at) <= 6.0 * h)
                        .min_by(|a, b| dist(a.1, dv.at).total_cmp(&dist(b.1, dv.at)))
                        .map(|(m, _, _)| m.trim().to_string())
                };
                let label = Some(dv.circuit.trim().to_string()).filter(|c| parse_circuit(c).is_some()).or_else(beside);
                match label.as_deref().and_then(parse_circuit) {
                    Some(key) => by_circuit.entry(key).or_insert_with(|| (label.unwrap_or_default(), Vec::new())).1.push(*dv),
                    None => {
                        out.no_circuit += 1;
                        let kind = if dv.kind.is_empty() { super::readers::device_kind(&dv.tag) } else { dv.kind.as_str() };
                        if on_branch_circuit(kind) {
                            unnumbered.push(*dv);
                        }
                    }
                }
            }
            for ((panel, ckt), (label, devs)) in &by_circuit {
                // A circuit whose devices all sit on drawn conduit needs no estimate.
                let on_drawn = |p: [f64; 2]| sc.conduits.iter().any(|(a, b, ..)| beside(*a, *b, p).0 <= ON_CONDUIT_FT * upf);
                if !sc.conduits.is_empty() && devs.iter().all(|dv| on_drawn(dv.at)) {
                    continue;
                }
                let homerun = sc.homeruns.iter().filter(|h| h.key == (panel.clone(), ckt.clone())).min_by(|a, b| {
                    let near = |h: &Homerun| devs.iter().map(|dv| dist(dv.at, h.origin)).fold(f64::INFINITY, f64::min);
                    near(a).total_cmp(&near(b))
                });
                read.estimated_ft += route_circuit(&mut out, &cx, Some(panel), label, devs, homerun);
                read.circuits += 1;
            }
            // Devices with no circuit printed: neighbours chained, a few to a circuit.
            let mut chain: Vec<&ReadDevice> = Vec::with_capacity(unnumbered.len());
            if let Some(i) = unnumbered
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1.at[1], a.1.at[0]).partial_cmp(&(b.1.at[1], b.1.at[0])).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
            {
                chain.push(unnumbered.swap_remove(i));
            }
            while !unnumbered.is_empty() {
                let Some(last) = chain.last().map(|d| d.at) else { break };
                let Some((i, _)) = unnumbered.iter().enumerate().min_by(|a, b| dist(a.1.at, last).total_cmp(&dist(b.1.at, last))) else { break };
                chain.push(unnumbered.swap_remove(i));
            }
            for (n, group) in chain.chunks(DEVICES_PER_UNNUMBERED).enumerate() {
                let label = format!("{UNNUMBERED} {}", n + 1);
                read.estimated_ft += route_circuit(&mut out, &cx, None, &label, group, None);
                read.circuits += 1;
            }
        }
        out.sheets.push(read);
    }
    if !out.mep.runs.is_empty() {
        for name in [DRAWN_SYSTEM, CIRCUIT_SYSTEM] {
            if out.mep.runs.iter().any(|r| r.system == name) {
                out.mep.systems.push(system(name, bottom));
            }
        }
    }
    out
}

/// Put the estimated routes at their level's ceiling (just above it), now the spec has its
/// levels: the legs at that height, each drop up to it, each riser from it as far as it went.
/// Drawn conduit keeps its own.
pub(crate) fn seat_heights(sp: &mut BuildingSpec) {
    let ceilings: BTreeMap<String, f64> = sp
        .levels
        .iter()
        .filter(|l| l.ceiling_ft.is_finite() && l.ceiling_ft > 0.0 && l.ceiling_ft + ABOVE_CEILING_FT < l.height_ft.max(l.ceiling_ft + 1.0))
        .map(|l| (l.name.to_ascii_lowercase(), l.ceiling_ft + ABOVE_CEILING_FT))
        .collect();
    for r in sp.mep.runs.iter_mut().filter(|r| r.system == CIRCUIT_SYSTEM) {
        let Some(&h) = ceilings.get(&r.level.to_ascii_lowercase()) else { continue };
        match r.top_ft {
            Some(top) if r.id.ends_with(RISER_SUFFIX) => {
                let rise = top - r.bottoms_ft.first().copied().unwrap_or(top);
                r.bottoms_ft = vec![h];
                r.top_ft = Some(h + rise.max(0.0));
            }
            Some(_) => {
                let mount = r.bottoms_ft.first().copied().unwrap_or(0.0);
                r.top_ft = Some(h.max(mount));
            }
            None => r.bottoms_ft.iter_mut().for_each(|b| *b = h),
        }
    }
}

/// Put the circuits into the spec's tags (an earlier read's go).
pub(crate) fn store_circuits(sp: &mut BuildingSpec, circuits: &[ElementTag]) {
    sp.tags.retain(|t| !t.id.starts_with(CIRCUIT_TAG));
    sp.tags.extend(circuits.iter().cloned());
}

/// The circuit a spec run belongs to: its tag id (`circuit:…`), from the run's id.
pub(crate) fn circuit_of_run(r: &MepRun) -> Option<&str> {
    r.id.strip_prefix(CIRCUIT_TAG).and(r.id.split_once('#').map(|(t, _)| t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;

    fn sheet(number: &str, level: &str, x: f64) -> SheetRef {
        SheetRef {
            number: number.into(),
            level: level.into(),
            discipline: "E".into(),
            kind: "plan".into(),
            rect: [x, 0.0, x + 2000.0, 1000.0],
            span: None,
            scale_text: None,
            levels: Vec::new(),
            view: None,
        }
    }

    fn dev(tag: &str, x: f64, y: f64, circuit: &str, sheet: &str) -> ReadDevice {
        ReadDevice { tag: tag.into(), at: [x, y], sheet: sheet.into(), level: "1".into(), circuit: circuit.into(), ..ReadDevice::default() }
    }

    fn text(s: &mut Session, at: [f64; 2], t: &str, h: f64) {
        s.execute("text", &json!({"at": at, "text": t, "height": h})).expect("text");
    }

    fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
        s.execute("line", &json!({"points": [a, b]})).expect("line");
    }

    #[test]
    fn labels_notes_and_panels_parse() {
        assert_eq!(parse_circuit("D-7"), Some(("D".into(), "7".into())));
        assert_eq!(parse_circuit("LA-1,3,5"), Some(("LA".into(), "1,3,5".into())));
        assert_eq!(parse_circuit("L1/1"), Some(("L1".into(), "1".into())));
        assert_eq!(parse_circuit("LP1-12"), Some(("LP1".into(), "12".into())));
        for junk in ["E-101", "A-1A", "12-6", "3/4", "D-", "PANEL-1", ""] {
            assert_eq!(parse_circuit(junk), None, "{junk}");
        }
        assert_eq!(conductor_note("2#12, 1#12G").as_deref(), Some("2#12 + 1#12G"));
        assert_eq!(conductor_note("3#10 + 1#10G").as_deref(), Some("3#10 + 1#10G"));
        assert_eq!(conductor_note("NOTE 3"), None);
        assert_eq!(panel_lettering("PANEL LA").as_deref(), Some("LA"));
        assert_eq!(panel_lettering("PNL 'D'").as_deref(), Some("D"));
        assert_eq!(panel_lettering("PANEL SCHEDULE"), None);
        assert_eq!(mount_height_ft("switch"), Some(4.0));
        assert_eq!(mount_height_ft("occupancy sensor"), None);
    }

    /// One E plan, inches: panel LA lettered by its box, circuit LA-1 on three devices with a
    /// homerun arrow and its conductor note at the device farthest from the panel, circuit
    /// LA-3 on two devices with no homerun, a tagged 3/4" conduit, and junk (an untagged long
    /// line, a sheet number by an arrow-like stroke).
    fn plan(s: &mut Session) -> Vec<ReadDevice> {
        // Panel LA: a 2' x 1' box at (100, 100) lettered above.
        s.execute("pline", &json!({"points": [[88, 94], [112, 94], [112, 106], [88, 106]], "closed": true})).expect("box");
        text(s, [96.0, 110.0], "LA", 6.0);
        // LA-1: switch at (600, 100), devices at (600, 400) and (900, 400); the homerun leaves
        // the one at (900, 400) for the label.
        line(s, [900.0, 400.0], [930.0, 430.0]);
        line(s, [912.0, 418.0], [918.0, 412.0]); // ticks across the shaft
        line(s, [917.0, 423.0], [923.0, 417.0]);
        line(s, [913.0, 411.0], [911.0, 425.0]);
        text(s, [932.0, 428.0], "LA-1", 6.0);
        text(s, [932.0, 418.0], "3#12, 1#12G", 4.0);
        // A tagged conduit along y = 800 and an untagged wall line.
        line(s, [100.0, 800.0], [700.0, 800.0]);
        text(s, [300.0, 804.0], "3/4\"C", 6.0);
        line(s, [100.0, 950.0], [1900.0, 950.0]);
        // LA-5: a homerun with three ticks and no note at (1500, 100).
        line(s, [1500.0, 100.0], [1470.0, 70.0]);
        for c in [1485.0, 1488.0, 1491.0] {
            line(s, [c - 3.0, c - 1400.0 + 3.0], [c + 3.0, c - 1400.0 - 3.0]);
        }
        text(s, [1460.0, 60.0], "LA-5", 6.0);
        // Junk: a sheet number at an arrow-like stroke.
        line(s, [1500.0, 100.0], [1530.0, 130.0]);
        text(s, [1532.0, 128.0], "E-101", 6.0);
        vec![
            dev("S", 600.0, 100.0, "LA-1", "E-1"),
            dev("A", 600.0, 400.0, "LA-1", "E-1"),
            dev("A", 900.0, 400.0, "LA-1", "E-1"),
            dev("R", 1200.0, 600.0, "LA-3", "E-1"),
            dev("R", 1200.0, 300.0, "LA-3", "E-1"),
            dev("A", 1600.0, 600.0, "", "E-1"),
            dev("R", 1200.0, 100.0, "LA-5", "E-1"),
            dev("R", 1500.0, 100.0, "LA-5", "E-1"),
        ]
    }

    #[test]
    fn an_electrical_plan_reads_its_conduit_homeruns_and_circuits() {
        let mut s = Session::new();
        let devices = plan(&mut s);
        let d = s.doc().expect("doc").clone();
        let sh = sheet("E-1", "1", 0.0);
        let r = read_sheets(&d, &[sh.clone(), SheetRef { discipline: "A".into(), ..sheet("A-1", "1", 0.0) }], 12.0, &devices);
        assert_eq!(r.sheets.len(), 1, "only the E plan: {:?}", r.sheets);
        let s0 = &r.sheets[0];
        assert_eq!((s0.conduits, s0.homeruns, s0.panels.clone()), (1, 2, vec!["LA".to_string()]), "{s0:?}");
        assert!((s0.conduit_ft - 50.0).abs() < 1e-6, "{s0:?}");
        assert_eq!(r.no_circuit, 1);
        // The device with no circuit printed is routed on an unnumbered circuit, to the
        // nearest panel, and says so.
        let u = r.circuits.iter().find(|t| t.fields["circuit"].value == format!("{UNNUMBERED} 1")).expect("unnumbered");
        assert_eq!(
            (u.fields["circuit"].source.as_str(), u.fields["panel"].value.as_str(), u.fields["panel"].source.as_str()),
            ("rule", "LA", "rule")
        );
        assert_eq!(u.issues, vec!["circuit_not_printed".to_string()]);
        assert!(r.mep.runs.iter().any(|x| x.id == format!("{}#home", u.id)), "its home run");
        // The drawn conduit: 3/4" from its tag.
        let drawn: Vec<&MepRun> = r.mep.runs.iter().filter(|x| x.system == DRAWN_SYSTEM).collect();
        assert_eq!(drawn.len(), 1);
        assert_eq!((drawn[0].size_in[0], drawn[0].size_source.as_str()), (0.75, "tag"));
        // LA-1: home run from the homerun's device (900, 400) to the panel, then the chain.
        let c1 = r.circuits.iter().find(|t| t.fields["circuit"].value == "LA-1").expect("LA-1");
        assert_eq!(c1.kind, "circuit");
        assert_eq!(c1.at_ft, [75.0, 400.0 / 12.0]);
        assert_eq!(c1.fields["conductors"].value, "3#12 + 1#12G");
        assert_eq!(c1.fields["conductors"].source, "note");
        assert_eq!(c1.fields["homerun"].value, "read");
        assert_eq!(c1.fields["route"].value, ESTIMATED);
        assert_eq!(c1.fields["devices"].value, "3");
        let runs1: Vec<&MepRun> = r.mep.runs.iter().filter(|x| circuit_of_run(x) == Some(c1.id.as_str())).collect();
        let home = runs1.iter().find(|x| x.id.ends_with("#home")).expect("home run");
        assert_eq!(home.points, vec![[900.0, 400.0], [100.0, 400.0], [100.0, 100.0]]);
        // Chain 900 → 600 (y 400) → 600, 100: 25' + 25'; the switch's drop 8.25 - 4 = 4.25'.
        let legs: f64 =
            runs1.iter().filter(|x| x.top_ft.is_none() && !x.id.ends_with("#home")).map(|x| super::super::bom::run_length_ft(&x.points, 12.0)).sum();
        assert!((legs - 50.0).abs() < 1e-6, "{runs1:#?}");
        let drops: Vec<&&MepRun> = runs1.iter().filter(|x| x.top_ft.is_some()).collect();
        assert_eq!(drops.len(), 1, "luminaire-type marks have no drop; the switch has one");
        assert_eq!((drops[0].bottoms_ft.clone(), drops[0].top_ft), (vec![4.0], Some(8.25)));
        assert!(runs1.iter().all(|x| x.height_source == ESTIMATED && x.system == CIRCUIT_SYSTEM));
        // LA-3: no homerun, defaults marked as such; it starts at the device nearest the panel.
        let c3 = r.circuits.iter().find(|t| t.fields["circuit"].value == "LA-3").expect("LA-3");
        assert_eq!((c3.fields["conductors"].value.as_str(), c3.fields["conductors"].source.as_str()), (DEFAULT_CONDUCTORS, "default"));
        assert_eq!(c3.fields["homerun"].value, "not drawn");
        assert_eq!(c3.at_ft, [100.0, 25.0]);
        assert!(r.unrouted.is_empty(), "{:?}", r.unrouted);
        // LA-5: three ticks across its arrow, no note: three #12 and a ground, by rule.
        let c5 = r.circuits.iter().find(|t| t.fields["circuit"].value == "LA-5").expect("LA-5");
        assert_eq!((c5.fields["conductors"].value.as_str(), c5.fields["conductors"].source.as_str()), ("3#12 + 1#12G", "rule"));
        assert_eq!(c5.at_ft, [125.0, 100.0 / 12.0], "it starts at the homerun's device");
        // The sheet number by a stroke is no homerun.
        assert_eq!(r.circuits.len(), 4);
        assert_eq!(r.levels_with_conduit(), 1);
        let total: f64 = r.by_level.values().sum();
        assert!((total - r.drawn_ft() - r.estimated_ft()).abs() < 1e-6, "{:?}", r.by_level);
    }

    #[test]
    fn a_circuit_without_its_panel_is_listed_unrouted_and_a_reread_replaces() {
        let mut s = Session::new();
        let devices = vec![dev("S", 100.0, 100.0, "Q-2", "E-1"), dev("S", 400.0, 100.0, "Q-2", "E-1")];
        text(&mut s, [0.0, 0.0], "NOTES", 6.0);
        line(&mut s, [0.0, 900.0], [1800.0, 900.0]);
        let d = s.doc().expect("doc").clone();
        let r = read_sheets(&d, &[sheet("E-1", "2", 0.0)], 12.0, &devices);
        assert_eq!(r.circuits.len(), 1);
        assert_eq!(r.circuits[0].issues, vec!["panel_not_found".to_string()]);
        assert_eq!(r.unrouted.len(), 1, "{:?}", r.unrouted);
        assert!(r.mep.runs.iter().all(|x| !x.id.ends_with("#home")));
        assert_eq!(r.circuits[0].level, "Level 2");
        // Into a spec: heights seat at the level's ceiling, a re-store replaces the circuits.
        let mut sp = BuildingSpec {
            levels: vec![cadaddy_walk3d::Level {
                name: "Level 2".into(),
                elevation_ft: 12.0,
                height_ft: 12.0,
                ceiling_ft: 9.0,
                region: None,
                base: None,
                attr: None,
                sheet: None,
            }],
            ..BuildingSpec::default()
        };
        super::super::printmep::merge_mep(&mut sp, r.mep.clone(), &["E-1".into()]);
        seat_heights(&mut sp);
        store_circuits(&mut sp, &r.circuits);
        store_circuits(&mut sp, &r.circuits);
        assert_eq!(sp.tags.iter().filter(|t| t.kind == "circuit").count(), 1);
        let leg = sp.mep.runs.iter().find(|x| x.top_ft.is_none()).expect("leg");
        assert_eq!(leg.bottoms_ft, vec![9.25]);
        assert!(sp.mep.runs.iter().filter(|x| x.top_ft.is_some()).all(|x| x.top_ft == Some(9.25) && x.bottoms_ft == vec![4.0]));
    }

    fn on_level(mut d: ReadDevice, level: &str) -> ReadDevice {
        d.level = level.into();
        d
    }

    fn panel_box(s: &mut Session, at: [f64; 2], name: &str) {
        let [x, y] = at;
        s.execute("pline", &json!({"points": [[x - 12.0, y - 6.0], [x + 12.0, y - 6.0], [x + 12.0, y + 6.0], [x - 12.0, y + 6.0]], "closed": true}))
            .expect("box");
        text(s, [x - 30.0, y + 10.0], &format!("PANEL {name}"), 6.0);
    }

    /// Q6036: a typical plan's circuits are on every level it shows (its devices read once per
    /// level, or only once), and a sheet split into one view per level routes each view's
    /// devices on its own level; the conduit is counted level by level.
    #[test]
    fn a_typical_plan_and_a_split_sheet_put_circuits_on_every_level() {
        let mut s = Session::new();
        panel_box(&mut s, [100.0, 100.0], "LA");
        text(&mut s, [1900.0, 900.0], "NOTES", 6.0);
        // E-5: two views, Level 5 on the left, Level 6 on the right, a panel in each.
        panel_box(&mut s, [3100.0, 100.0], "LB");
        panel_box(&mut s, [4100.0, 100.0], "LB");
        line(&mut s, [3050.0, 900.0], [3950.0, 900.0]);
        line(&mut s, [4050.0, 900.0], [4950.0, 900.0]);
        let d = s.doc().expect("doc").clone();
        let typical = SheetRef { levels: vec!["2".into(), "3".into(), "4".into()], ..sheet("E-2", "2", 0.0) };
        let mut v5 = SheetRef { number: "E-5/5".into(), view: Some([3000.0, 0.0, 4000.0, 1000.0]), ..sheet("E-5", "5", 3000.0) };
        v5.rect = [3000.0, 0.0, 5000.0, 1000.0];
        let v6 = SheetRef { number: "E-5/6".into(), level: "6".into(), view: Some([4000.0, 0.0, 5000.0, 1000.0]), ..v5.clone() };
        // The devices stage read the typical plan's devices for Levels 2 and 3, not 4; the
        // split sheet's devices carry the sheet's own number.
        let mut devices = Vec::new();
        for l in ["2", "3"] {
            devices.push(on_level(dev("R", 600.0, 100.0, "LA-1", "E-2"), l));
            devices.push(on_level(dev("R", 900.0, 400.0, "LA-1", "E-2"), l));
        }
        devices.push(on_level(dev("R", 3500.0, 300.0, "LB-2", "E-5"), "5"));
        devices.push(on_level(dev("R", 3700.0, 300.0, "LB-2", "E-5"), "5"));
        devices.push(on_level(dev("R", 4500.0, 300.0, "LB-4", "E-5"), "6"));
        let r = read_sheets(&d, &[typical, v5, v6], 12.0, &devices);
        let mut levels: Vec<&str> = r.circuits.iter().map(|t| t.level.as_str()).collect();
        levels.sort_unstable();
        assert_eq!(levels, ["Level 2", "Level 3", "Level 4", "Level 5", "Level 6"], "{:#?}", r.circuits);
        assert_eq!(r.levels_with_conduit(), 5, "{:?}", r.by_level);
        assert!(r.by_level.values().all(|ft| *ft > 0.0), "{:?}", r.by_level);
        // Each level's circuit is its own (its id names the level) and goes to its own panel.
        let ids: std::collections::BTreeSet<&str> = r.circuits.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.len(), 5);
        assert!(r.unrouted.is_empty(), "{:?}", r.unrouted);
        assert!(r.mep.runs.iter().all(|x| !x.id.ends_with(RISER_SUFFIX)), "every panel is on its level");
        let six = r.circuits.iter().find(|t| t.level == "Level 6").expect("Level 6");
        assert_eq!((six.fields["circuit"].value.as_str(), six.fields["panel_at"].value.as_str()), ("LB-4", "E-5/6"));
        let home = r.mep.runs.iter().find(|x| x.id == format!("{}#home", six.id)).expect("home");
        assert_eq!(home.points.last(), Some(&[4100.0, 100.0]), "the panel in its own view");
        // Levels 2–4 alike: the same plan, the same feet.
        let ft = |l: &str| r.by_level.get(l).copied().unwrap_or(0.0);
        assert!((ft("Level 2") - ft("Level 4")).abs() < 1e-6 && (ft("Level 3") - ft("Level 4")).abs() < 1e-6, "{:?}", r.by_level);
    }

    /// Q6036 MUST 2: a circuit whose panel is on another level's plan gets its home run to the
    /// panel's place and a riser to it (estimated: 10' a floor until the spec has its levels;
    /// then from the ceiling, as long).
    #[test]
    fn a_home_run_to_a_panel_on_another_level_goes_by_a_riser() {
        let mut s = Session::new();
        panel_box(&mut s, [100.0, 100.0], "LA");
        line(&mut s, [0.0, 900.0], [1900.0, 900.0]);
        line(&mut s, [2000.0, 900.0], [3900.0, 900.0]);
        let d = s.doc().expect("doc").clone();
        let devices = [dev("R", 2600.0, 400.0, "LA-7", "E-3"), dev("R", 2900.0, 400.0, "LA-7", "E-3")];
        let r = read_sheets(
            &d,
            &[sheet("E-1", "1", 0.0), sheet("E-3", "3", 2000.0)],
            12.0,
            &[on_level(devices[0].clone(), "3"), on_level(devices[1].clone(), "3")],
        );
        assert_eq!(r.circuits.len(), 1, "{:#?}", r.circuits);
        let t = &r.circuits[0];
        assert_eq!((t.level.as_str(), t.fields["panel_at"].value.as_str(), t.fields["panel_level"].value.as_str()), ("Level 3", "E-1", "Level 1"));
        assert!(t.fields["riser"].value.contains("20 ft"), "{:?}", t.fields["riser"]);
        assert!(r.unrouted.is_empty());
        // The home run goes to where the panel is (moved over from E-1 by the sheets' corners).
        let home = r.mep.runs.iter().find(|x| x.id == format!("{}#home", t.id)).expect("home");
        assert_eq!((home.level.as_str(), home.points.last()), ("Level 3", Some(&[2100.0, 100.0])));
        let riser = r.mep.runs.iter().find(|x| x.id == format!("{}{RISER_SUFFIX}", t.id)).expect("riser");
        assert_eq!((riser.level.as_str(), riser.points.clone()), ("Level 1", vec![[2100.0, 100.0]]), "it starts on the lower level");
        assert_eq!((riser.bottoms_ft.clone(), riser.top_ft), (vec![8.25], Some(28.25)));
        assert!(r.by_level.get("Level 1").is_some_and(|ft| (*ft - 20.0).abs() < 1e-6), "{:?}", r.by_level);
        // In the spec it starts at Level 1's ceiling and is as long.
        let level = |name: &str, el: f64| cadaddy_walk3d::Level {
            name: name.into(),
            elevation_ft: el,
            height_ft: 12.0,
            ceiling_ft: 9.0,
            region: None,
            base: None,
            attr: None,
            sheet: None,
        };
        let mut sp = BuildingSpec { levels: vec![level("Level 1", 0.0), level("Level 3", 24.0)], ..BuildingSpec::default() };
        super::super::printmep::merge_mep(&mut sp, r.mep.clone(), &["E-3".into()]);
        seat_heights(&mut sp);
        let riser = sp.mep.runs.iter().find(|x| x.id.ends_with(RISER_SUFFIX)).expect("riser");
        assert_eq!((riser.bottoms_ft.clone(), riser.top_ft), (vec![9.25], Some(29.25)));
    }

    /// Q6036 MUST 1: lighting and power devices with no circuit printed still get conduit, a few
    /// neighbours to an unnumbered circuit to the nearest panel; fire alarm and low voltage
    /// devices are their own systems and get none.
    #[test]
    fn devices_with_no_circuit_printed_are_routed_a_few_to_a_circuit() {
        let mut s = Session::new();
        panel_box(&mut s, [100.0, 100.0], "LC");
        line(&mut s, [0.0, 900.0], [1900.0, 900.0]);
        let d = s.doc().expect("doc").clone();
        let mut devices: Vec<ReadDevice> = (0..20).map(|i| dev("R", 200.0 + 60.0 * f64::from(i), 500.0, "", "E-4")).collect();
        devices.push(dev("SD", 900.0, 800.0, "", "E-4"));
        devices.push(dev("FA", 950.0, 800.0, "", "E-4"));
        devices.push(dev("WAP", 1000.0, 800.0, "", "E-4"));
        let r = read_sheets(&d, &[sheet("E-4", "1", 0.0)], 12.0, &devices);
        assert_eq!(r.no_circuit, 23, "{:?}", r.warnings);
        let counts: Vec<String> = r.circuits.iter().map(|t| t.fields["devices"].value.clone()).collect();
        assert_eq!(counts, ["8", "8", "5"], "21 branch devices (the switch too), 8 to a circuit");
        assert!(r.circuits.iter().all(|t| t.fields["panel"].value == "LC" && t.issues == vec!["circuit_not_printed".to_string()]));
        assert!(r.circuits.iter().all(|t| t.fields["route"].value == ESTIMATED));
        assert!(r.unrouted.is_empty());
        assert_eq!(r.levels_with_conduit(), 1);
        // Hostile: no panel anywhere, devices of no sheet outside every plan: nothing panics,
        // the circuit is listed unrouted.
        let bare = Session::new();
        let far = vec![dev("R", 1.0e300, -1.0e300, "", ""), dev("R", f64::NAN, 3.0, "Q-1", "")];
        let r = read_sheets(bare.doc().expect("doc"), &[sheet("E-9", "1", 0.0)], 12.0, &far);
        assert!(r.circuits.is_empty());
        let r = read_sheets(&d, &[sheet("E-4", "R", 0.0)], 12.0, &[dev("R", 300.0, 300.0, "Z-1", "E-4")]);
        assert_eq!(r.unrouted.len(), 1, "{:?}", r.unrouted);
    }
}
