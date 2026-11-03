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
use cadaddy_walk3d::{BuildingSpec, Mep, MepEquipment, MepKind, MepRun, MepSystem};
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

/// A level's ceiling when the spec has none yet, feet; conduit runs this far above it.
const DEFAULT_CEILING_FT: f64 = 8.0;
const ABOVE_CEILING_FT: f64 = 0.25;
/// Lines at least this long (feet) can be drawn conduit.
const MIN_RUN_FT: f64 = 4.0;
/// Device to device on one branch circuit at most, feet: a longer step is two places printing the
/// same label.
const MAX_LEG_FT: f64 = 60.0;
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
/// A guest room's own panel (a load center recessed in its wall): wide, deep and high, inches.
const ROOM_PANEL_IN: [f64; 3] = [14.0, 4.0, 24.0];
/// A building panel lettered in a plan of its room (an MEP room's): a panelboard, inches.
const PANELBOARD_IN: [f64; 3] = [20.0, 6.0, 48.0];
/// The top of trim off the floor, inches: the highest breaker handle within 6'-7" (NEC 240.24(A)).
const PANEL_TOP_IN: f64 = 78.0;
/// The farthest the room's wall may be from where its plan letters the panel, feet.
const ROOM_PANEL_WALL_FT: f64 = 3.0;
/// Where a room's own panel stands, as its `sources.position` says.
pub(crate) const ROOM_PANEL_POSITION: &str = "typical-room plan lettering";

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
/// number (`E-101`: no panel has circuit 101), a dimension, or a zero-padded number (`S-01`: a
/// room's tag, circuits are never padded) is not one.
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
        && nums.iter().all(|n| {
            (1..=2).contains(&n.len())
                && !n.starts_with('0')
                && n.chars().all(|c| c.is_ascii_digit())
                && n.parse::<u32>().is_ok_and(|v| (1..=84).contains(&v))
        });
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
pub(crate) fn panel_lettering(s: &str) -> Option<String> {
    let u = s.trim().to_ascii_uppercase();
    let rest = ["PANELBOARD", "PANEL", "PNLBD", "PNL"].iter().find_map(|p| u.strip_prefix(p))?;
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
    /// Devices with no circuit printed beside them (no route can be estimated).
    pub no_circuit: usize,
    pub warnings: Vec<String>,
}

impl CondRead {
    pub fn drawn_ft(&self) -> f64 {
        self.sheets.iter().map(|s| s.conduit_ft).sum()
    }
    pub fn estimated_ft(&self) -> f64 {
        self.sheets.iter().map(|s| s.estimated_ft).sum()
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
            "unrouted": self.unrouted,
            "sheets": self.sheets.iter().map(|s| json!({
                "sheet": s.sheet, "level": s.level, "conduit_runs": s.conduits, "conduit_ft": r1(s.conduit_ft),
                "homeruns": s.homeruns, "panels": s.panels, "circuits": s.circuits, "estimated_ft": r1(s.estimated_ft),
            })).collect::<Vec<_>>(),
            "warnings": self.warnings,
        })
    }
}

pub(crate) fn run(system: &str, level: &str, sheet: &str, points: Vec<[f64; 2]>, size_in: f64, bottom: f64) -> MepRun {
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
        slope_in_per_ft: 0.0,
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

/// Whether `dv` was read on `sh`.
fn on_sheet(dv: &ReadDevice, sh: &SheetRef) -> bool {
    if dv.sheet.is_empty() {
        let [x0, y0, x1, y1] = sh.rect;
        dv.at[0] >= x0 && dv.at[0] <= x1 && dv.at[1] >= y0 && dv.at[1] <= y1
    } else {
        dv.sheet.eq_ignore_ascii_case(&sh.number)
    }
}

/// Read every electrical plan among `sheets` of `d` (`upf` drawing units per foot): drawn
/// conduit, homeruns and panels, and an estimated route for each circuit `devices` print.
pub(crate) fn read_sheets(d: &Drawing, sheets: &[SheetRef], upf: f64, devices: &[ReadDevice]) -> CondRead {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let mut out = CondRead::default();
    let plans: Vec<&SheetRef> = sheets.iter().filter(|s| s.discipline.eq_ignore_ascii_case("E") && s.builds()).take(MAX_SHEETS).collect();
    let devices = &devices[..devices.len().min(MAX_DEVICES)];
    let bottom = DEFAULT_CEILING_FT + ABOVE_CEILING_FT;
    // Every plan first (a circuit's panel can be on another plan of its level).
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
        let level = level_name(&sh.level);
        let mut read = CondSheet { sheet: sh.number.clone(), level: level.clone(), ..CondSheet::default() };
        read.panels = sc.panels.keys().cloned().collect();
        read.homeruns = sc.homeruns.len();
        for (i, (a, b, size, boc)) in sc.conduits.iter().enumerate() {
            if out.mep.runs.len() >= MAX_RUNS {
                break;
            }
            let mut r = run(DRAWN_SYSTEM, &level, &sh.number, vec![*a, *b], *size, boc.unwrap_or(bottom));
            r.id = format!("conduit:{}:{}#{i}", id_part(&level), id_part(&sh.number));
            r.size_source = "tag".into();
            r.height_source = if boc.is_some() { "note".into() } else { "default".into() };
            read.conduit_ft += dist(*a, *b) / upf;
            read.conduits += 1;
            out.mep.runs.push(r);
        }
        // This sheet's devices by circuit: the one the devices stage read, else the circuit
        // lettered beside the device (within six letter heights).
        let mut by_circuit: BTreeMap<(String, String), (String, Vec<&ReadDevice>)> = BTreeMap::new();
        for dv in devices.iter().filter(|dv| on_sheet(dv, sh)) {
            let beside = || {
                sc.marks
                    .iter()
                    .filter(|(_, at, h)| dist(*at, dv.at) <= 6.0 * h)
                    .min_by(|a, b| dist(a.1, dv.at).total_cmp(&dist(b.1, dv.at)))
                    .map(|(m, _, _)| m.trim().to_string())
            };
            let label = Some(dv.circuit.trim().to_string()).filter(|c| parse_circuit(c).is_some()).or_else(beside);
            match label.as_deref().and_then(parse_circuit) {
                Some(key) => by_circuit.entry(key).or_insert_with(|| (label.unwrap_or_default(), Vec::new())).1.push(dv),
                None => out.no_circuit += 1,
            }
        }
        let eps = 0.05 * upf;
        for ((panel, ckt), (label, devs)) in &by_circuit {
            // A circuit whose devices all sit on drawn conduit needs no estimate.
            let on_drawn = |p: [f64; 2]| sc.conduits.iter().any(|(a, b, ..)| beside(*a, *b, p).0 <= ON_CONDUIT_FT * upf);
            if !sc.conduits.is_empty() && devs.iter().all(|dv| on_drawn(dv.at)) {
                continue;
            }
            // The panel: on this plan, else on another plan of the level the same size (the same
            // plan placement), moved over by the sheets' corners.
            let size = |r: &[f64; 4]| (r[2] - r[0], r[3] - r[1]);
            let (w, h) = size(&sh.rect);
            let panel_at = sc.panels.get(panel).map(|p| (*p, sh.number.clone())).or_else(|| {
                scans.iter().filter(|(o, _)| o.number != sh.number && o.level.eq_ignore_ascii_case(&sh.level)).find_map(|(o, osc)| {
                    let (ow, oh) = size(&o.rect);
                    let same = w > 0.0 && h > 0.0 && ((ow - w) / w).abs() < 0.02 && ((oh - h) / h).abs() < 0.02;
                    let p = osc.panels.get(panel).filter(|_| same)?;
                    Some(([p[0] - o.rect[0] + sh.rect[0], p[1] - o.rect[1] + sh.rect[1]], o.number.clone()))
                })
            });
            let homerun = sc.homeruns.iter().filter(|h| h.key == (panel.clone(), ckt.clone())).min_by(|a, b| {
                let near = |h: &Homerun| devs.iter().map(|dv| dist(dv.at, h.origin)).fold(f64::INFINITY, f64::min);
                near(a).total_cmp(&near(b))
            });
            // The chain: from the homerun's device, else the one nearest the panel, else the
            // lowest-leftmost; then always the nearest one left.
            let mut left: Vec<&ReadDevice> = devs.clone();
            let start_at = homerun.map(|h| h.origin).or_else(|| panel_at.as_ref().map(|p| p.0));
            let first = match start_at {
                Some(p) => left.iter().enumerate().min_by(|a, b| dist(a.1.at, p).total_cmp(&dist(b.1.at, p))).map(|(i, _)| i),
                None => left
                    .iter()
                    .enumerate()
                    .min_by(|a, b| (a.1.at[1], a.1.at[0]).partial_cmp(&(b.1.at[1], b.1.at[0])).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i),
            };
            let Some(first) = first else { continue };
            let mut chain = vec![left.swap_remove(first)];
            while !left.is_empty() {
                let Some(last) = chain.last().map(|d| d.at) else { break };
                let Some((i, _)) = left.iter().enumerate().min_by(|a, b| dist(a.1.at, last).total_cmp(&dist(b.1.at, last))) else { break };
                chain.push(left.swap_remove(i));
            }
            let cid = format!("{CIRCUIT_TAG}{}:{}:{}", id_part(&level), id_part(&sh.number), id_part(label));
            let mut legs = 0usize;
            let mut ft = 0.0;
            let mut push = |out: &mut CondRead, points: Vec<[f64; 2]>, id: String, top: Option<(f64, f64)>| {
                if out.mep.runs.len() >= MAX_RUNS {
                    return;
                }
                let mut r = run(CIRCUIT_SYSTEM, &level, &sh.number, points, DEFAULT_SIZE_IN, bottom);
                if let Some((mount, top)) = top {
                    r.bottoms_ft = vec![mount];
                    r.top_ft = Some(top);
                    ft += top - mount;
                } else {
                    ft += super::bom::run_length_ft(&r.points, upf);
                }
                r.id = id;
                out.mep.runs.push(r);
            };
            // The home run, then device to device.
            if let (Some((p, _)), Some(dv)) = (&panel_at, chain.first()) {
                push(&mut out, square(dv.at, *p, eps), format!("{cid}#home"), None);
            }
            // A leg longer than a branch circuit's runs between its devices is two places that print
            // the same label (each guest room's own panel, a typical room's plan shown twice): no
            // conduit between them.
            let mut leaps = 0usize;
            for w in chain.windows(2) {
                if let [a, b] = w {
                    if dist(a.at, b.at) > MAX_LEG_FT * upf {
                        leaps += 1;
                        continue;
                    }
                    legs += 1;
                    push(&mut out, square(a.at, b.at, eps), format!("{cid}#{legs}"), None);
                }
            }
            // A drop to each wall device.
            let mut drops = 0usize;
            let mut no_height = 0usize;
            for (i, dv) in chain.iter().enumerate() {
                let kind = if dv.kind.is_empty() { super::readers::device_kind(&dv.tag) } else { dv.kind.as_str() };
                match mount_height_ft(kind) {
                    Some(m) if m < bottom => {
                        drops += 1;
                        push(&mut out, vec![dv.at], format!("{cid}#d{i}"), Some((m, bottom)));
                    }
                    Some(_) => {}
                    None => no_height += 1,
                }
            }
            let (conductors, source) =
                homerun.and_then(|h| h.conductors.clone()).map_or_else(|| (DEFAULT_CONDUCTORS.to_string(), "default"), |(c, s)| (c, s));
            let mut t = ElementTag {
                id: cid.clone(),
                kind: "circuit".into(),
                level: level.clone(),
                at_ft: [chain[0].at[0] / upf, chain[0].at[1] / upf],
                ..ElementTag::default()
            };
            t.set("circuit", label.clone(), "plan");
            t.set("panel", panel.clone(), "plan");
            t.set("sheet", sh.number.clone(), "plan");
            t.set("devices", chain.len().to_string(), "computed");
            t.set("route", ESTIMATED, "rule");
            t.set("conductors", conductors, source);
            t.set("conduit", "3/4\" EMT", "default");
            t.set("homerun", homerun.map_or("not drawn", |_| "read"), if homerun.is_some() { "plan" } else { "rule" });
            t.set("drops", drops.to_string(), "computed");
            if leaps > 0 {
                t.set("leaps", format!("{leaps} step(s) over {MAX_LEG_FT}' between its devices: not one run, no conduit counted there"), "rule");
            }
            if no_height > 0 {
                t.set("drops_unknown", format!("{no_height} device(s) of no known mounting height: no drop counted"), "rule");
            }
            match &panel_at {
                Some((_, on)) => t.set("panel_at", on.clone(), "plan"),
                None => {
                    t.issues.push("panel_not_found".into());
                    out.unrouted.push(json!({
                        "circuit": label, "sheet": sh.number, "level": level,
                        "reason": format!("panel {panel} is not shown on the level's electrical plans: no home run counted"),
                    }));
                }
            }
            out.circuits.push(t);
            read.circuits += 1;
            read.estimated_ft += ft;
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
/// levels: the legs at that height, each drop up to it. Drawn conduit keeps its own.
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
            Some(_) => {
                let mount = r.bottoms_ft.first().copied().unwrap_or(0.0);
                r.top_ft = Some(h.max(mount));
            }
            None => r.bottoms_ft.iter_mut().for_each(|b| *b = h),
        }
    }
}

/// Put the circuits into the spec's tags (an earlier read's go; the stamped rooms' own stay).
pub(crate) fn store_circuits(sp: &mut BuildingSpec, circuits: &[ElementTag]) {
    sp.tags.retain(|t| !t.id.starts_with(CIRCUIT_TAG) || t.id.contains(ROOM_PART));
    sp.tags.extend(circuits.iter().cloned());
}

/// What marks a circuit as a stamped room's own (its id).
const ROOM_PART: &str = ":room-";

/// The circuits of devices a typical room's plan stamped into rooms (`ReadDevice::room`): each
/// room's own (a guest room has its own panel), per room and circuit an estimated route in the
/// model's coordinates: the devices chained (no step over `MAX_LEG_FT`), a drop to each wall
/// device. No home run: the room's panel is not placed.
pub(crate) fn room_circuits(devices: &[ReadDevice], panels: &BTreeMap<String, Vec<(String, [f64; 2])>>, upf: f64) -> (Vec<MepRun>, Vec<ElementTag>) {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let bottom = DEFAULT_CEILING_FT + ABOVE_CEILING_FT;
    let eps = 0.05 * upf;
    let mut groups: BTreeMap<(String, String, String), Vec<&ReadDevice>> = BTreeMap::new();
    for dv in devices.iter().take(MAX_DEVICES).filter(|d| !d.room.is_empty() && d.at.iter().all(|v| v.is_finite())) {
        if parse_circuit(&dv.circuit).is_some() {
            groups.entry((dv.level.clone(), dv.room.clone(), dv.circuit.trim().to_string())).or_default().push(dv);
        }
    }
    let (mut runs, mut circuits) = (Vec::new(), Vec::new());
    for ((level, room, label), devs) in groups {
        if runs.len() >= MAX_RUNS {
            break;
        }
        let level = level_name(&level);
        let cid = format!("{CIRCUIT_TAG}{}{ROOM_PART}{}:{}", id_part(&level), id_part(&room), id_part(&label));
        // The room's own panel of the circuit, where its typical plan letters it.
        let panel = parse_circuit(&label).map(|(p, _)| p).unwrap_or_default();
        let panel_at = panels.get(&room).and_then(|ps| ps.iter().find(|(n, _)| n.eq_ignore_ascii_case(&panel))).map(|(_, at)| *at);
        // Chained from the device nearest the panel (else the lowest-leftmost), then always the
        // nearest one left.
        let mut left = devs.clone();
        left.sort_by(|a, b| (a.at[1], a.at[0]).partial_cmp(&(b.at[1], b.at[0])).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(p) = panel_at
            && let Some((i, _)) = left.iter().enumerate().min_by(|a, b| dist(a.1.at, p).total_cmp(&dist(b.1.at, p)))
        {
            let first = left.remove(i);
            left.insert(0, first);
        }
        let mut chain = vec![left.remove(0)];
        while !left.is_empty() {
            let Some(last) = chain.last().map(|d| d.at) else { break };
            let Some((i, _)) = left.iter().enumerate().min_by(|a, b| dist(a.1.at, last).total_cmp(&dist(b.1.at, last))) else { break };
            chain.push(left.swap_remove(i));
        }
        let (mut legs, mut drops) = (0usize, 0usize);
        if let Some(p) = panel_at {
            let mut r = run(CIRCUIT_SYSTEM, &level, &format!("room {room}"), square(chain[0].at, p, eps), DEFAULT_SIZE_IN, bottom);
            r.id = format!("{cid}#home");
            runs.push(r);
        }
        for w in chain.windows(2) {
            if let [a, b] = w
                && dist(a.at, b.at) <= MAX_LEG_FT * upf
            {
                legs += 1;
                let mut r = run(CIRCUIT_SYSTEM, &level, &format!("room {room}"), square(a.at, b.at, eps), DEFAULT_SIZE_IN, bottom);
                r.id = format!("{cid}#{legs}");
                runs.push(r);
            }
        }
        for (i, dv) in chain.iter().enumerate() {
            let kind = if dv.kind.is_empty() { super::readers::device_kind(&dv.tag) } else { dv.kind.as_str() };
            if let Some(m) = mount_height_ft(kind).filter(|m| *m < bottom) {
                drops += 1;
                let mut r = run(CIRCUIT_SYSTEM, &level, &format!("room {room}"), vec![dv.at], DEFAULT_SIZE_IN, bottom);
                r.bottoms_ft = vec![m];
                r.top_ft = Some(bottom);
                r.id = format!("{cid}#d{i}");
                runs.push(r);
            }
        }
        let mut t = ElementTag {
            id: cid,
            kind: "circuit".into(),
            level: level.clone(),
            at_ft: [chain[0].at[0] / upf, chain[0].at[1] / upf],
            ..ElementTag::default()
        };
        t.set("circuit", label.clone(), "plan");
        t.set("panel", panel.clone(), "plan");
        t.set("room", room.clone(), "computed");
        t.set("sheet", chain[0].sheet.clone(), "plan");
        t.set("devices", chain.len().to_string(), "computed");
        t.set("route", ESTIMATED, "rule");
        t.set("conductors", DEFAULT_CONDUCTORS, "default");
        t.set("conduit", "3/4\" EMT", "default");
        match panel_at {
            Some(_) => t.set("homerun", format!("to the room's own panel {panel}, where its typical plan letters it"), "rule"),
            None => t.set("homerun", "the room's own panel: not placed, no home run counted", "rule"),
        }
        t.set("drops", drops.to_string(), "computed");
        circuits.push(t);
    }
    (runs, circuits)
}

/// The panels the plans of rooms letter (`panels`: by the room) as equipment on the room's wall
/// (`rooms`: each room's level, outline, and whether a typical-room plan stamped it) at the
/// outline edge nearest the lettering: in a room a typical plan stamped, the room's own load
/// center (`G-<room>`, one per room) recessed in the wall, front flush; in a plan of one room (an
/// electrical room's), the building's panel of that name, a panelboard on the wall's face. With
/// the panels' places moved onto their fronts, for the circuits' home runs.
#[allow(clippy::type_complexity)]
pub(crate) fn room_panel_equipment(
    panels: &BTreeMap<String, Vec<(String, [f64; 2])>>,
    rooms: &BTreeMap<String, (String, Vec<cadaddy_geom::Vec2>, bool)>,
    upf: f64,
) -> (Vec<MepEquipment>, BTreeMap<String, Vec<(String, [f64; 2])>>) {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let (mut out, mut moved) = (Vec::new(), BTreeMap::new());
    for (room, ps) in panels.iter().take(MAX_DEVICES) {
        let Some((level, outline, typical)) = rooms.get(room) else {
            moved.insert(room.clone(), ps.clone());
            continue;
        };
        let size = if *typical { ROOM_PANEL_IN } else { PANELBOARD_IN };
        let [w, dp, _] = size.map(|v| v / 12.0 * upf);
        let mut at_front = Vec::new();
        for (name, at) in ps.iter().take(8) {
            let Some((front, along, out_n)) = on_wall(*at, outline, w, ROOM_PANEL_WALL_FT * upf) else {
                at_front.push((name.clone(), *at));
                continue;
            };
            // Recessed: its back in the wall; on the face: its front in the room.
            let (front, back) = if *typical {
                (front, [front[0] + out_n[0] * dp, front[1] + out_n[1] * dp])
            } else {
                ([front[0] - out_n[0] * dp, front[1] - out_n[1] * dp], front)
            };
            at_front.push((name.clone(), front));
            let half = [along[0] * w / 2.0, along[1] * w / 2.0];
            let corners = [
                [front[0] - half[0], front[1] - half[1]],
                [front[0] + half[0], front[1] + half[1]],
                [back[0] - half[0], back[1] - half[1]],
                [back[0] + half[0], back[1] + half[1]],
            ];
            let min = corners.iter().fold([f64::INFINITY; 2], |m, c| [m[0].min(c[0]), m[1].min(c[1])]);
            let max = corners.iter().fold([f64::NEG_INFINITY; 2], |m, c| [m[0].max(c[0]), m[1].max(c[1])]);
            let rule = if *typical { "rule: a guest room load center" } else { "rule: a panelboard" }.to_string();
            let sources: BTreeMap<String, String> = [
                ("size", rule.clone()),
                ("height", rule.clone()),
                ("elevation", "rule: top of trim 6'-6\", the highest breaker handle within 6'-7\"".to_string()),
                ("position", ROOM_PANEL_POSITION.to_string()),
                ("footprint", "rule: on the room's wall nearest the lettering".to_string()),
                ("status", "unconfirmed".to_string()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
            let properties: BTreeMap<String, String> = [
                ("PANEL", name.clone()),
                ("ROOM", room.clone()),
                ("MOUNTING", if *typical { "recessed, front flush with the wall" } else { "surface, on the wall" }.to_string()),
                ("SIZE", format!("{}\" W x {}\" D x {}\" H", size[0], size[1], size[2])),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
            out.push(MepEquipment {
                tag: if *typical { format!("{name}-{room}") } else { name.clone() },
                kind: "panel".into(),
                level: level_name(level),
                min,
                max,
                bottom_ft: (PANEL_TOP_IN - size[2]) / 12.0,
                height_ft: size[2] / 12.0,
                title: "typical-room plan".into(),
                properties,
                sources,
                confidence: 0.5,
                ..MepEquipment::default()
            });
        }
        moved.insert(room.clone(), at_front);
    }
    (out, moved)
}

/// Where a box `width` wide stands on the edge of `outline` nearest `p` (within `reach`): the
/// middle of its front on the edge, the edge's direction and the normal out of the room, each
/// along the axis nearest it (the box is square to the plan).
fn on_wall(p: [f64; 2], outline: &[cadaddy_geom::Vec2], width: f64, reach: f64) -> Option<([f64; 2], [f64; 2], [f64; 2])> {
    let n = outline.len();
    if n < 3 || !p.iter().all(|v| v.is_finite()) {
        return None;
    }
    let mut best: Option<(f64, [f64; 2], [f64; 2], f64, f64)> = None;
    for i in 0..n.min(MAX_ITEMS) {
        let (Some(a), Some(b)) = (outline.get(i), outline.get((i + 1) % n)) else { continue };
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = dx.hypot(dy);
        if !(len.is_finite() && len > 1e-9) {
            continue;
        }
        let u = [dx / len, dy / len];
        let t = ((p[0] - a.x) * u[0] + (p[1] - a.y) * u[1]).clamp(0.0, len);
        let d = dist(p, [a.x + u[0] * t, a.y + u[1] * t]);
        if d <= reach && best.is_none_or(|b| d < b.0) {
            best = Some((d, [a.x, a.y], u, len, t));
        }
    }
    let (_, a, u, len, t) = best?;
    // The whole box on the edge where it fits.
    let t = if len > width { t.clamp(width / 2.0, len - width / 2.0) } else { len / 2.0 };
    let front = [a[0] + u[0] * t, a[1] + u[1] * t];
    let u = if u[0].abs() >= u[1].abs() { [u[0].signum(), 0.0] } else { [0.0, u[1].signum()] };
    let mut out_n = [u[1], -u[0]];
    let probe = cadaddy_geom::Vec2::new(front[0] + out_n[0] * 1e-3 * len.max(1.0), front[1] + out_n[1] * 1e-3 * len.max(1.0));
    if cadaddy_geom::point_in_polygon(outline, probe) {
        out_n = [-out_n[0], -out_n[1]];
    }
    Some((front, u, out_n))
}

/// Put the rooms' panels into the spec (an earlier build's go); a panel the spec has from
/// elsewhere (a schedule) stays as it is.
pub(crate) fn store_room_panels(sp: &mut BuildingSpec, panels: Vec<MepEquipment>) {
    sp.mep.equipment.retain(|e| e.sources.get("position").map(String::as_str) != Some(ROOM_PANEL_POSITION));
    let new: Vec<MepEquipment> = panels.into_iter().filter(|p| !sp.mep.equipment.iter().any(|e| e.tag.eq_ignore_ascii_case(&p.tag))).collect();
    sp.mep.equipment.extend(new);
}

/// Put the stamped rooms' circuits and their runs into the spec (an earlier build's go), at the
/// levels' ceilings.
pub(crate) fn store_room_circuits(sp: &mut BuildingSpec, runs: Vec<MepRun>, circuits: &[ElementTag]) {
    let room = |id: &str| id.starts_with(CIRCUIT_TAG) && id.contains(ROOM_PART);
    sp.tags.retain(|t| !room(&t.id));
    sp.mep.runs.retain(|r| !room(&r.id));
    if !runs.is_empty() && !sp.mep.systems.iter().any(|s| s.name == CIRCUIT_SYSTEM) {
        sp.mep.systems.push(system(CIRCUIT_SYSTEM, DEFAULT_CEILING_FT + ABOVE_CEILING_FT));
    }
    sp.mep.runs.extend(runs);
    sp.tags.extend(circuits.iter().cloned());
    seat_heights(sp);
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
        for junk in ["E-101", "A-1A", "12-6", "3/4", "D-", "PANEL-1", "S-01", ""] {
            assert_eq!(parse_circuit(junk), None, "{junk}");
        }
        assert_eq!(conductor_note("2#12, 1#12G").as_deref(), Some("2#12 + 1#12G"));
        assert_eq!(conductor_note("3#10 + 1#10G").as_deref(), Some("3#10 + 1#10G"));
        assert_eq!(conductor_note("NOTE 3"), None);
        assert_eq!(panel_lettering("PANEL LA").as_deref(), Some("LA"));
        assert_eq!(panel_lettering("PNL 'D'").as_deref(), Some("D"));
        assert_eq!(panel_lettering("PNLBD 'G'").as_deref(), Some("G"));
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
    fn each_stamped_rooms_circuits_are_its_own() {
        let in_room = |tag: &str, x: f64, room: &str| ReadDevice { room: room.into(), ..dev(tag, x, 100.0, "G-2", "E-6") };
        // Rooms 101 and 102 side by side, each with its own panel G: two G-2 circuits.
        let devices = vec![
            in_room("REC", 0.0, "101"),
            in_room("REC", 120.0, "101"),
            in_room("S", 60.0, "101"),
            in_room("REC", 200.0, "102"),
            in_room("REC", 320.0, "102"),
        ];
        // Room 101's panel G lettered at its door; room 102's not.
        let panels: BTreeMap<String, Vec<(String, [f64; 2])>> = [("101".to_string(), vec![("G".to_string(), [0.0, 160.0])])].into_iter().collect();
        let (runs, circuits) = room_circuits(&devices, &panels, 12.0);
        let homes: Vec<&MepRun> = runs.iter().filter(|r| r.id.ends_with("#home")).collect();
        assert_eq!(homes.len(), 1, "room 101's circuit runs home to its panel");
        assert_eq!(homes[0].points.last(), Some(&[0.0, 160.0]));
        assert_eq!(circuits.len(), 2, "{circuits:?}");
        assert!(circuits.iter().all(|c| c.fields["homerun"].source == "rule"));
        for c in &circuits {
            let mine: Vec<&MepRun> = runs.iter().filter(|r| circuit_of_run(r) == Some(c.id.as_str())).collect();
            // No run reaches into the other room.
            let xs: Vec<f64> = mine.iter().flat_map(|r| r.points.iter().map(|p| p[0])).collect();
            assert!(xs.iter().all(|x| *x <= 120.0) || xs.iter().all(|x| *x >= 200.0), "{mine:#?}");
        }
        assert_eq!(runs.iter().filter(|r| r.top_ft.is_some()).count(), 1, "the switch's drop");
        // The takeoff wires them: the conductors over each room circuit's conduit.
        let mut s = Session::new();
        let mut spec = BuildingSpec::from_json(
            r#"{"version": 1, "units_per_foot": 12, "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}]}"#,
        )
        .expect("spec");
        store_room_circuits(&mut spec, runs.clone(), &circuits);
        s.execute("bspec.set", &json!({"replace": true, "spec": serde_json::from_str::<Value>(&spec.to_json()).expect("spec")})).expect("set");
        let bom = s.execute("bim.bom", &json!({})).expect("bom");
        let wire: f64 = bom["lines"].as_array().into_iter().flatten().filter(|l| l["item"] == "THHN copper").filter_map(|l| l["net"].as_f64()).sum();
        assert!(wire > 0.0, "{bom:#}");
        // The sheets' circuits stored again (the build does after the devices): the rooms' stay.
        store_circuits(&mut spec, &[]);
        assert_eq!(spec.tags.len(), 2);
        // Stored twice: the second replaces the first.
        let mut sp = BuildingSpec::default();
        store_room_circuits(&mut sp, runs.clone(), &circuits);
        store_room_circuits(&mut sp, runs.clone(), &circuits);
        assert_eq!((sp.mep.runs.len(), sp.tags.len()), (runs.len(), 2));
    }

    #[test]
    fn a_rooms_own_panel_is_recessed_in_the_wall_by_its_lettering() {
        let square = |x0: f64| -> Vec<cadaddy_geom::Vec2> {
            [[x0, 0.0], [x0 + 144.0, 0.0], [x0 + 144.0, 180.0], [x0, 180.0]].iter().map(|p| cadaddy_geom::Vec2::new(p[0], p[1])).collect()
        };
        // 101 and 102 stamped from a typical plan; 160 is an MEP room its own plan shows (a little
        // askew, as traced).
        let mut askew = square(400.0);
        if let Some(p) = askew.get_mut(3) {
            p.x += 3.0;
        }
        let rooms: BTreeMap<String, (String, Vec<cadaddy_geom::Vec2>, bool)> = [
            ("101".to_string(), ("Level 1".to_string(), square(0.0), true)),
            ("102".to_string(), ("Level 1".to_string(), square(200.0), true)),
            ("160".to_string(), ("Level 1".to_string(), askew, false)),
        ]
        .into_iter()
        .collect();
        // Room 101's G lettered 5" off its left wall; 102's right of its left wall; 103 has no
        // outline; the MEP room letters the building's panel A.
        let panels: BTreeMap<String, Vec<(String, [f64; 2])>> = [
            ("101".to_string(), vec![("G".to_string(), [5.0, 100.0])]),
            ("102".to_string(), vec![("G".to_string(), [206.0, 4.0])]),
            ("103".to_string(), vec![("G".to_string(), [500.0, 50.0])]),
            ("160".to_string(), vec![("A".to_string(), [406.0, 100.0])]),
        ]
        .into_iter()
        .collect();
        let (gear, moved) = room_panel_equipment(&panels, &rooms, 12.0);
        assert_eq!(gear.len(), 3, "{gear:#?}");
        // The MEP room's panel is the building's A, a panelboard square to the plan, on the face
        // of the wall (in the room).
        let a = gear.iter().find(|e| e.tag == "A").expect("A");
        let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
        assert_eq!((r3(a.max[0] - a.min[0]), r3(a.max[1] - a.min[1]), a.height_ft), (6.0, 20.0, 4.0), "{a:#?}");
        assert!(a.min[0] > 401.0 && a.max[0] < 408.0, "{a:#?}");
        assert_eq!(a.properties["MOUNTING"], "surface, on the wall");
        let g101 = gear.iter().find(|e| e.tag == "G-101").expect("G-101");
        // In the wall behind the lettering: 4" deep out of the room, 14" wide, top of trim 6'-6".
        assert_eq!((g101.min, g101.max), ([-4.0, 93.0], [0.0, 107.0]), "{g101:#?}");
        assert_eq!((g101.kind.as_str(), g101.bottom_ft, g101.height_ft), ("panel", 4.5, 2.0));
        assert_eq!(g101.sources["status"], "unconfirmed");
        assert_eq!(g101.properties["ROOM"], "101");
        // Lettered at a corner: the whole box on the nearest wall (the bottom one, 4" off it).
        let g102 = gear.iter().find(|e| e.tag == "G-102").expect("G-102");
        assert_eq!((g102.min, g102.max), ([200.0, -4.0], [214.0, 0.0]), "{g102:#?}");
        // The circuits run home to the panel's front; a room with no outline keeps its lettering.
        assert_eq!(moved["101"], vec![("G".to_string(), [0.0, 100.0])]);
        assert_eq!(moved["103"], vec![("G".to_string(), [500.0, 50.0])]);
        // Into the spec once however often stored; the takeoff counts one G per room with its trim.
        let mut spec = BuildingSpec::from_json(
            r#"{"version": 1, "units_per_foot": 12, "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}]}"#,
        )
        .expect("spec");
        // A scheduled panel A is the one the spec keeps.
        spec.mep.equipment.push(MepEquipment {
            tag: "A".into(),
            title: "ELECTRICAL EQUIPMENT SCHEDULE".into(),
            sources: BTreeMap::new(),
            properties: BTreeMap::new(),
            ..a.clone()
        });
        store_room_panels(&mut spec, gear.clone());
        store_room_panels(&mut spec, gear);
        assert_eq!(spec.mep.equipment.len(), 3);
        assert_eq!(spec.mep.equipment.iter().filter(|e| e.tag == "A").count(), 1);
        let mut s = Session::new();
        s.execute("bspec.set", &json!({"replace": true, "spec": serde_json::from_str::<Value>(&spec.to_json()).expect("spec")})).expect("set");
        let bom = s.execute("bim.bom", &json!({})).expect("bom");
        let net = |item: &str| -> f64 {
            bom["lines"].as_array().into_iter().flatten().filter(|l| l["item"] == item).filter_map(|l| l["net"].as_f64()).sum()
        };
        assert_eq!((net("panel G, one per room"), net("Panel cover/trim, flush: panel G")), (2.0, 2.0), "{bom:#}");
        // A room's load center takes no lug kit (the building's panel A does).
        let lug = |c: &&Value| c["item"] == "Lug kit" && c["detail"].as_str().is_some_and(|d| d.contains("panel G"));
        assert!(!bom["spec_check"].as_array().into_iter().flatten().any(|c| lug(&c)), "{bom:#}");
    }

    #[test]
    fn two_places_printing_one_label_get_no_conduit_between_them() {
        let mut s = Session::new();
        let mut devices = plan(&mut s);
        // LA-3 printed again 100' away (another room's own panel has a circuit 3 too).
        devices.push(dev("R", 1200.0 + 1200.0, 600.0, "LA-3", "E-1"));
        let d = s.doc().expect("doc").clone();
        let r = read_sheets(&d, &[sheet("E-1", "1", 0.0)], 12.0, &devices);
        let c3 = r.circuits.iter().find(|t| t.fields["circuit"].value == "LA-3").expect("LA-3");
        assert!(c3.fields.contains_key("leaps"), "{c3:?}");
        let longest = r
            .mep
            .runs
            .iter()
            .filter(|x| circuit_of_run(x) == Some(c3.id.as_str()) && x.top_ft.is_none() && !x.id.ends_with("#home"))
            .map(|x| super::super::bom::run_length_ft(&x.points, 12.0))
            .fold(0.0, f64::max);
        assert!(longest <= MAX_LEG_FT, "no leg across the leap: {longest}");
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
        assert_eq!(r.circuits.len(), 3);
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
}
