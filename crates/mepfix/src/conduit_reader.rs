//! Branch conduit and homeruns from the electrical plans (Q289): the conduit runs drawn on the
//! lighting and power plans as 3D raceways, and each homerun arrow as an approximate route to
//! its panel.
//!
//! - **Conduit**: a long line on the device layer (the devices' own linework is short). Its size
//!   comes from the tag beside it (`3/4"C`, `1-1/4"C`, `1" EMT`); untagged, it is the smallest
//!   branch size (3/4") and the size is marked assumed. Q266's tray and feeder banks are on
//!   their own layers and are read by [`crate::tray_reader`].
//! - **Homerun**: a short diagonal line with its tip at a circuit label (`L1/1`): the circuit's
//!   wiring leaves the last device there for the panel the label names (`L1`). Its route is
//!   an assumption (the arrow only says "to the panel"): along the plan's x, then y, to the
//!   panel drawn on any electrical plan of the level.
//! - **Heights**: by Q257's rules ([`crate::heights::resolve_heights`]), with conduit as the
//!   lowest trade: a `BOC @` note beside the tag wins; else just above the ceiling of the room
//!   it is in; else stacked under the other trades below structure. A height from a rule is an
//!   assumption, and says so.
//!
//! - **Without layer names** (a print: its pen layers only): every straight piece drawn in a
//!   drawing pen (not the screened background) is a candidate, and a run is one drawn single: a
//!   long piece with a same-length parallel close beside it is a feeder bank or a tray's rail
//!   ([`crate::tray_reader`]'s), not branch conduit.
//!
//! E-sheet conduit and homerun routes are diagrammatic: every route this reader builds is an
//! assumed route between architecturally placed devices and panels, never a surveyed path.
//! Positions come from Q255's sheet map (each sheet aligned by its grid), not from the truth.

use crate::heights::{LevelContext, SegmentInput, TradeRules, resolve_heights};
use crate::sheet_reader::{Alignment, RecognisedSheet, SheetMap, building_from_model};
use crate::truth::{HeightSource, Truth};
use cadaddy_doc::{Drawing, EntityKind};

/// Most lines and texts read from one sheet (hostile drawings).
const MAX_ITEMS: usize = 100_000;
/// Device-layer lines at least this long (feet) are conduit; shorter ones are symbols.
const MIN_RUN_FT: f64 = 4.0;
/// A homerun arrow's shaft is between these lengths (feet): drawn at the text's size, so short.
const HOMERUN_FT: std::ops::RangeInclusive<f64> = 0.1..=MIN_RUN_FT;
/// How far past the structural grid a run may reach (feet); beyond it is the legend and the
/// title block, whose sample lines aren't runs.
const GRID_MARGIN_FT: f64 = 10.0;
/// The size an untagged branch conduit is taken as, inches.
pub const DEFAULT_SIZE_IN: f64 = 0.75;
/// The system names the runs go into the building spec as.
pub const CONDUIT_SYSTEM: &str = "E-BRANCH-COND";
pub const HOMERUN_SYSTEM: &str = "E-HOMERUN";

/// A conduit run found on a plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundConduit {
    pub level: String,
    pub sheet: String,
    pub from_ft: [f64; 2],
    pub to_ft: [f64; 2],
    pub size_in: f64,
    /// The tag as printed, when there is one.
    pub tag: Option<String>,
    /// Whether the size came from a tag (else the default, assumed).
    pub size_from_tag: bool,
    /// A `BOC @` note beside the tag, as printed.
    pub height_note: Option<String>,
    /// Bottom above the floor, feet, once [`assign_heights`] has run.
    pub bottom_ft: Option<f64>,
    pub height_source: Option<HeightSource>,
    /// The plan entity handles this run was read from (hex).
    pub objects: Vec<String>,
}

/// A homerun arrow and its assumed route to the panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundHomerun {
    pub level: String,
    pub sheet: String,
    /// `L1/1`.
    pub label: String,
    /// The panel's tag (the label before `/`).
    pub panel: String,
    /// Where the route starts, building feet (device position after anchoring, else E-sheet).
    pub origin_ft: [f64; 2],
    /// The original E-sheet position of the arrow (before anchoring), for scoring.
    pub drawn_origin_ft: [f64; 2],
    /// The panel, when one with that tag is drawn on the level's electrical plans.
    pub panel_ft: Option<[f64; 2]>,
    /// The assumed route: origin, the turn, the panel (empty without a panel).
    pub route_ft: Vec<[f64; 2]>,
    /// Bottom above the floor of each leg, feet, once [`assign_heights`] has run.
    pub bottoms_ft: Vec<f64>,
    /// The conductor note under the label (`2#12, 1#12G`), as printed.
    pub note: Option<String>,
    /// The plan entity handles this homerun was read from (hex): arrow line and label text.
    pub objects: Vec<String>,
    /// Where the origin came from: `"device"` (architecturally placed) or `"E-sheet"`.
    pub origin_source: String,
    /// Where the panel position came from: `"equipment"` (architecturally placed) or `"E-sheet"`.
    pub panel_source: String,
}

/// A panel drawn on an electrical plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundPanelAt {
    pub level: String,
    pub tag: String,
    pub at_ft: [f64; 2],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConduitResult {
    pub conduits: Vec<FoundConduit>,
    pub homeruns: Vec<FoundHomerun>,
    pub panels: Vec<FoundPanelAt>,
}

/// A trade size in inches: `3/4`, `1`, `1-1/4`, `1 1/4`, `2`.
fn trade_size(s: &str) -> Option<f64> {
    let s = s.trim();
    let frac = |f: &str| -> Option<f64> {
        let (n, d) = f.split_once('/')?;
        let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
        (d > 0.0).then_some(n / d)
    };
    let v = if let Some((whole, f)) = s.split_once(['-', ' ']).filter(|(_, f)| f.contains('/')) {
        whole.trim().parse::<f64>().ok()? + frac(f)?
    } else if s.contains('/') {
        frac(s)?
    } else {
        s.parse::<f64>().ok()?
    };
    (v > 0.0 && v <= 6.0).then_some(v)
}

/// A trade size as printed: `3/4`, `1`, `1-1/4`.
pub fn fmt_size(v: f64) -> String {
    let eighths = (v * 8.0).round() as i64;
    let (whole, rem) = (eighths / 8, eighths % 8);
    let frac = match rem {
        0 => String::new(),
        r if r % 4 == 0 => format!("{}/2", r / 4),
        r if r % 2 == 0 => format!("{}/4", r / 2),
        r => format!("{r}/8"),
    };
    match (whole, frac.is_empty()) {
        (0, false) => frac,
        (w, true) => w.to_string(),
        (w, false) => format!("{w}-{frac}"),
    }
}

/// A branch conduit tag's size, inches: `3/4"C`, `1"C`, `1-1/4"C`, `1" EMT`, `3/4" C`. A bank
/// (`(2) 3"C`, Q266's) or a pipe tag (`2" CW`) is not one.
pub fn parse_conduit_tag(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.starts_with('(') || s.len() > 24 {
        return None;
    }
    let (size, rest) = s.split_once('"')?;
    let rest = rest.trim().to_ascii_uppercase();
    let kind = rest.split_whitespace().next().unwrap_or_default();
    if !matches!(kind, "C" | "EMT" | "RGS" | "IMC" | "PVC" | "CONDUIT") {
        return None;
    }
    trade_size(size)
}

/// A `BOC @ 9'-8"` note: the conduit's bottom above the floor, feet.
pub fn parse_boc(s: &str) -> Option<f64> {
    let u = s.trim().to_ascii_uppercase();
    let rest = u.strip_prefix("BOC")?.trim_start().trim_start_matches('@').trim();
    let (ft, inch) = rest.split_once('\'')?;
    let inch = inch.trim_start_matches('-').trim().trim_end_matches('"');
    let v = ft.trim().parse::<f64>().ok()? + if inch.is_empty() { 0.0 } else { inch.parse::<f64>().ok()? / 12.0 };
    (0.0..100.0).contains(&v).then_some(v)
}

/// A circuit label: a panel tag, `/`, circuit numbers (`L1/1`, `LP2/12`, `P1/3,5`).
pub fn parse_circuit_label(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let (panel, ckt) = s.split_once('/')?;
    let ok_panel = (1..=6).contains(&panel.len())
        && panel.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && panel.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let ok_ckt = (1..=12).contains(&ckt.len())
        && ckt.chars().all(|c| c.is_ascii_digit() || c == ',')
        && ckt.chars().next().is_some_and(|c| c.is_ascii_digit());
    (ok_panel && ok_ckt).then(|| (panel.to_string(), ckt.to_string()))
}

#[derive(Clone, Copy, Debug)]
struct Line2 {
    a: [f64; 2],
    b: [f64; 2],
    handle: u64,
}

impl Line2 {
    fn len(&self) -> f64 {
        dist(self.a, self.b)
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Distance from `p` to the line's span, and where along it (0–1) the nearest point is.
fn beside(l: &Line2, p: [f64; 2]) -> (f64, f64) {
    let d = [l.b[0] - l.a[0], l.b[1] - l.a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 1e-12 { (((p[0] - l.a[0]) * d[0] + (p[1] - l.a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (dist(p, [l.a[0] + t * d[0], l.a[1] + t * d[1]]), t)
}

/// Is this sheet an electrical plan (lighting or power) with a level?
fn electrical_plan(sh: &RecognisedSheet) -> bool {
    let t = sh.title.to_ascii_uppercase();
    sh.discipline == "E" && sh.level.is_some() && t.contains("PLAN") && !t.contains("SCHEDULE")
}

/// Read every electrical plan's branch conduit, homerun arrows and panels.
pub fn read_conduits(dxf_data: &[(String, Vec<u8>)], map: &SheetMap) -> ConduitResult {
    let mut res = ConduitResult::default();
    for (name, bytes) in dxf_data {
        let stem = name.trim_end_matches(".dxf");
        let Some(sh) = map.sheets.iter().find(|s| s.number == stem) else { continue };
        if !electrical_plan(sh) {
            continue;
        }
        let (Some(level), Some(al)) = (&sh.level, &sh.alignment) else { continue };
        let Ok(d) = cadaddy_io::read_dxf(bytes) else { continue };
        read_sheet(&mut res, sh, &d, level, al, building_box(map));
    }
    // Each homerun to its panel, drawn on any of the level's electrical plans.
    for h in &mut res.homeruns {
        let Some(p) = res.panels.iter().find(|p| p.level == h.level && p.tag.eq_ignore_ascii_case(&h.panel)) else { continue };
        h.panel_ft = Some(p.at_ft);
        h.panel_source = "E-sheet".into();
        build_route(h);
    }
    res
}

/// Build the assumed L-shaped route from origin to panel.
fn build_route(h: &mut FoundHomerun) {
    let Some(panel) = h.panel_ft else { return };
    let turn = [panel[0], h.origin_ft[1]];
    h.route_ft = if dist(turn, h.origin_ft) < 0.1 || dist(turn, panel) < 0.1 {
        vec![h.origin_ft, panel]
    } else {
        vec![h.origin_ft, turn, panel]
    };
}

/// A device's position for anchoring homeruns (from Q288's placed devices).
pub struct DeviceAnchor {
    pub level: String,
    pub panel: String,
    pub circuit: String,
    pub at_ft: [f64; 2],
}

/// A panel / equipment position for anchoring homeruns (from the gear reader or mep.equipment).
pub struct EquipmentAnchor {
    pub level: String,
    pub tag: String,
    pub at_ft: [f64; 2],
}

/// An unrouted homerun that could not reach its panel (e.g. panel on another level).
#[derive(Clone, Debug, PartialEq)]
pub struct UnroutedHomerun {
    pub label: String,
    pub level: String,
    pub panel: String,
    pub reason: String,
}

/// Anchor homerun origins and endpoints to architecturally placed devices and equipment.
///
/// For each homerun, the origin is snapped to the nearest device on the same circuit and level
/// (from Q288's device reader); the endpoint is snapped to the panel's equipment position.
/// Where no matching device or equipment is found, the E-sheet position is kept and the source
/// is flagged as `"E-sheet position, not placed"`.
pub fn anchor_homeruns(
    res: &mut ConduitResult,
    devices: &[DeviceAnchor],
    equipment: &[EquipmentAnchor],
) -> Vec<UnroutedHomerun> {
    let mut unrouted = Vec::new();
    for h in &mut res.homeruns {
        // Anchor origin to the nearest device on this circuit, same level.
        let label_panel = &h.panel;
        let nearest_device = devices
            .iter()
            .filter(|d| d.level == h.level && d.panel.eq_ignore_ascii_case(label_panel))
            .min_by(|a, b| dist(a.at_ft, h.origin_ft).total_cmp(&dist(b.at_ft, h.origin_ft)));
        if let Some(dev) = nearest_device {
            h.origin_ft = dev.at_ft;
            h.origin_source = "device".into();
        } else {
            h.origin_source = "E-sheet position, not placed".into();
        }
        // Anchor panel endpoint to equipment position.
        let eq = equipment
            .iter()
            .find(|e| e.level == h.level && e.tag.eq_ignore_ascii_case(&h.panel));
        if let Some(eq) = eq {
            h.panel_ft = Some(eq.at_ft);
            h.panel_source = "equipment".into();
            build_route(h);
        } else if h.panel_ft.is_none() {
            // No E-sheet panel and no equipment: check other levels for the panel.
            let cross = equipment.iter().find(|e| e.tag.eq_ignore_ascii_case(&h.panel));
            if let Some(cl) = cross {
                // Panel on another level: unrouted.
                unrouted.push(UnroutedHomerun {
                    label: h.label.clone(),
                    level: h.level.clone(),
                    panel: h.panel.clone(),
                    reason: format!("panel {} on level {}, not this level", cl.tag, cl.level),
                });
            } else {
                unrouted.push(UnroutedHomerun {
                    label: h.label.clone(),
                    level: h.level.clone(),
                    panel: h.panel.clone(),
                    reason: format!("panel {} not found in equipment or on the plans", h.panel),
                });
            }
        } else if h.panel_source != "equipment" {
            h.panel_source = "E-sheet position, not placed".into();
        }
    }
    unrouted
}

/// The structural grid's extents with a margin (none without a grid: everything is in).
fn building_box(map: &SheetMap) -> Option<([f64; 2], [f64; 2])> {
    crate::sheet_reader::grid_box(map, GRID_MARGIN_FT)
}

/// How close (feet) a same-length parallel makes a long line part of a bank or a tray, not a
/// branch run (without layer names).
const BANKED_FT: f64 = 1.5;

/// Whether a long line has a parallel of about its extent within [`BANKED_FT`]: a feeder bank's
/// conduit or a tray's rail.
fn banked(l: &Line2, others: &[Line2]) -> bool {
    let len = l.len();
    if len <= 1e-9 {
        return false;
    }
    let u = [(l.b[0] - l.a[0]) / len, (l.b[1] - l.a[1]) / len];
    others.iter().any(|o| {
        let lo = o.len();
        if lo <= 1e-9 || (lo - len).abs() > 0.5 {
            return false;
        }
        let v = [(o.b[0] - o.a[0]) / lo, (o.b[1] - o.a[1]) / lo];
        let off = ((o.a[0] - l.a[0]) * -u[1] + (o.a[1] - l.a[1]) * u[0]).abs();
        let along = (o.a[0] - l.a[0]) * u[0] + (o.a[1] - l.a[1]) * u[1];
        (u[0] * v[1] - u[1] * v[0]).abs() < 0.01 && off > 0.01 && off <= BANKED_FT && (along.abs() < 0.5 || (along.abs() - len).abs() < 0.5)
    })
}

fn read_sheet(res: &mut ConduitResult, sh: &RecognisedSheet, d: &Drawing, level: &str, al: &Alignment, inside: Option<([f64; 2], [f64; 2])>) {
    let within = |p: [f64; 2]| inside.is_none_or(|(lo, hi)| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1]);
    let to_b = |x: f64, y: f64| building_from_model(al, [x, y]);
    let mut lines: Vec<Line2> = Vec::new();
    let mut texts: Vec<(String, [f64; 2], u64)> = Vec::new();
    // A drawing with the device layer reads its lines; a print (no such layer) its drawn pens.
    let layered = d.layers.iter().any(|l| l.name.eq_ignore_ascii_case("DEVICE"));
    for e in d.model.iter().take(MAX_ITEMS) {
        match &e.kind {
            EntityKind::Line(l) if layered && e.common.layer.eq_ignore_ascii_case("DEVICE") => {
                let ln = Line2 { a: to_b(l.a.x, l.a.y), b: to_b(l.b.x, l.b.y), handle: e.handle.0 };
                if within(ln.a) && within(ln.b) {
                    lines.push(ln);
                }
            }
            EntityKind::Text(t) => {
                let p = t.align_pt.unwrap_or(t.insert);
                texts.push((t.value.clone(), to_b(p.x, p.y), e.handle.0));
            }
            _ => {}
        }
    }
    if !layered {
        for [a, b] in crate::sheet_reader::drawn_segments(d, false).into_iter().take(MAX_ITEMS) {
            let ln = Line2 { a: to_b(a[0], a[1]), b: to_b(b[0], b[1]), handle: 0 };
            if within(ln.a) && within(ln.b) {
                lines.push(ln);
            }
        }
    }
    let long: Vec<Line2> = lines.iter().filter(|l| l.len() >= MIN_RUN_FT).copied().collect();
    // Conduit: the long device lines (drawn single, without layers), each with the nearest tag
    // beside it.
    for l in long.iter().filter(|l| layered || !banked(l, &long)) {
        let tag = texts
            .iter()
            .filter_map(|(t, p, _h)| parse_conduit_tag(t).map(|s| (t, s, p)))
            .map(|(t, s, p)| (t, s, p, beside(l, *p)))
            .filter(|(_, _, _, (dd, at))| *dd <= 1.5 && *at > 0.0 && *at < 1.0)
            .min_by(|a, b| a.3.0.total_cmp(&b.3.0));
        let height_note = tag.and_then(|(_, _, p, _)| {
            texts
                .iter()
                .filter(|(t, q, _)| parse_boc(t).is_some() && dist(*q, *p) < 2.0)
                .min_by(|a, b| dist(a.1, *p).total_cmp(&dist(b.1, *p)))
                .map(|x| x.0.clone())
        });
        let mut objects = Vec::new();
        if l.handle != 0 {
            objects.push(format!("{:X}", l.handle));
        }
        res.conduits.push(FoundConduit {
            level: level.to_string(),
            sheet: sh.number.clone(),
            from_ft: l.a,
            to_ft: l.b,
            size_in: tag.map_or(DEFAULT_SIZE_IN, |t| t.1),
            tag: tag.map(|t| t.0.clone()),
            size_from_tag: tag.is_some(),
            height_note,
            bottom_ft: None,
            height_source: None,
            objects,
        });
    }
    // Homeruns: a short diagonal device line whose tip is at a circuit label.
    for (t, p, text_handle) in &texts {
        let Some((panel, _)) = parse_circuit_label(t) else { continue };
        let arrow = lines
            .iter()
            .filter(|l| HOMERUN_FT.contains(&l.len()))
            .filter(|l| {
                let (dx, dy) = ((l.b[0] - l.a[0]).abs(), (l.b[1] - l.a[1]).abs());
                dx > 0.0 && dy > 0.0 && (dx / dy - 1.0).abs() < 0.25
            })
            .filter_map(|l| {
                let (ta, tb) = (dist(l.a, *p), dist(l.b, *p));
                // The tip is the end at the label; the origin the other end.
                if ta <= tb { (ta <= 1.0).then_some((ta, l.b, l.handle)) } else { (tb <= 1.0).then_some((tb, l.a, l.handle)) }
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, origin, arrow_handle)) = arrow else { continue };
        // The conductors noted just under the label.
        let note = texts
            .iter()
            .filter(|(n, q, _)| n.contains('#') && dist(*q, *p) < 1.5)
            .min_by(|a, b| dist(a.1, *p).total_cmp(&dist(b.1, *p)))
            .map(|(n, _, _)| n.trim().to_string());
        let mut objects = Vec::new();
        if arrow_handle != 0 {
            objects.push(format!("{:X}", arrow_handle));
        }
        if *text_handle != 0 {
            objects.push(format!("{:X}", text_handle));
        }
        res.homeruns.push(FoundHomerun {
            level: level.to_string(),
            sheet: sh.number.clone(),
            label: t.trim().to_string(),
            panel,
            origin_ft: origin,
            drawn_origin_ft: origin,
            note,
            objects,
            origin_source: "E-sheet".into(),
            panel_source: String::new(),
            ..FoundHomerun::default()
        });
    }
    for pnl in crate::elec_reader::detect_panels(d) {
        let at = to_b(pnl.at_in[0], pnl.at_in[1]);
        if !res.panels.iter().any(|q| q.level == level && q.tag == pnl.tag) {
            res.panels.push(FoundPanelAt { level: level.to_string(), tag: pnl.tag, at_ft: at });
        }
    }
}

/// Heights for every conduit and homerun leg by Q257's rules, from each level's context
/// (`ctx(level)`: its rooms and ceilings, structure; none leaves the level's heights unset).
pub fn assign_heights(res: &mut ConduitResult, ctx: &dyn Fn(&str) -> Option<LevelContext>) {
    let rules = TradeRules::standard();
    let mut levels: Vec<String> = res.conduits.iter().map(|c| c.level.clone()).chain(res.homeruns.iter().map(|h| h.level.clone())).collect();
    levels.sort();
    levels.dedup();
    for level in levels {
        let Some(cx) = ctx(&level) else { continue };
        let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let seg = |id: String, size: f64, at: [f64; 2], note: Option<String>| SegmentInput {
            id,
            system: CONDUIT_SYSTEM.into(),
            kind: "conduit".into(),
            shape: "round".into(),
            size_in: [size, 0.0],
            level: level.clone(),
            midpoint_ft: at,
            height_note: note,
            vertical: false,
        };
        let mut inputs = Vec::new();
        for (i, c) in res.conduits.iter().enumerate().filter(|(_, c)| c.level == level) {
            // A BOC note is a bottom: Q257's note parser reads it as `BOC @ …`.
            inputs.push(seg(format!("c{i}"), c.size_in, mid(c.from_ft, c.to_ft), c.height_note.clone()));
        }
        for (i, h) in res.homeruns.iter().enumerate().filter(|(_, h)| h.level == level) {
            for (j, w) in h.route_ft.windows(2).enumerate() {
                if let [a, b] = w {
                    inputs.push(seg(format!("h{i}-{j}"), 0.5, mid(*a, *b), None));
                }
            }
        }
        let resolved = resolve_heights(&inputs, &cx, &rules);
        let by_id = |id: &str| resolved.iter().find(|r| r.segment_id == id);
        for (i, c) in res.conduits.iter_mut().enumerate().filter(|(_, c)| c.level == level) {
            if let Some(r) = by_id(&format!("c{i}")) {
                c.bottom_ft = Some(r.bottom_ft);
                c.height_source = Some(r.source.clone());
            }
        }
        for (i, h) in res.homeruns.iter_mut().enumerate().filter(|(_, h)| h.level == level) {
            h.bottoms_ft = (0..h.route_ft.len().saturating_sub(1)).filter_map(|j| by_id(&format!("h{i}-{j}")).map(|r| r.bottom_ft)).collect();
        }
    }
}

/// A level's context from the readers: the ceiling reader's rooms (their ceilings by room
/// number) and the underside of structure above the floor.
pub fn context_from_ceilings(
    ceil: &crate::ceiling_reader::CeilingResult,
    level: &str,
    elevation_ft: f64,
    structure_underside_ft: f64,
) -> LevelContext {
    let rooms = ceil
        .rooms
        .iter()
        .filter(|r| r.level == level)
        .map(|r| {
            let c = ceil.ceilings.iter().find(|c| c.level == level && c.room_number == r.number);
            crate::truth::Room {
                id: format!("{level}-{}", r.number),
                level: level.to_string(),
                number: r.number.clone(),
                name: r.name.clone(),
                min_ft: r.min_ft,
                max_ft: r.max_ft,
                ceiling: c.filter(|c| !c.open_to_structure).and_then(|c| c.height_ft).map(|h| crate::truth::Ceiling {
                    height_ft: h,
                    kind: c.map(|c| c.kind.clone()).unwrap_or_default(),
                    ..Default::default()
                }),
                plenum_ft: None,
            }
        })
        .collect();
    LevelContext { level_code: level.to_string(), elevation_ft, structure_underside_ft, rooms, beams: Vec::new() }
}

/// Build device anchors from the truth's devices (each device knows its panel tag and level).
pub fn device_anchors_from_truth(t: &Truth) -> Vec<DeviceAnchor> {
    t.elec_devices
        .iter()
        .filter(|d| !d.panel.is_empty())
        .map(|d| {
            let panel_tag = t.elec_panels.iter().find(|p| p.id == d.panel).map(|p| p.tag.as_str()).unwrap_or(&d.panel);
            DeviceAnchor {
                level: d.level.clone(),
                panel: panel_tag.to_string(),
                circuit: d.circuit.clone(),
                at_ft: [d.at_ft[0], d.at_ft[1]],
            }
        })
        .collect()
}

/// Build equipment anchors from the truth's panels (their architectural position).
pub fn equipment_anchors_from_truth(t: &Truth) -> Vec<EquipmentAnchor> {
    t.elec_panels
        .iter()
        .map(|p| EquipmentAnchor { level: p.level.clone(), tag: p.tag.clone(), at_ft: [p.at_ft[0], p.at_ft[1]] })
        .collect()
}

/// Generate a seed's set and read its conduit and homeruns, heights from the ceiling reader
/// (and, for the structure above each floor, the truth's levels: floor to floor less 2').
/// Also anchors homeruns to architecturally placed devices and equipment from the truth.
pub fn read_fixture_conduits(seed: u64) -> Result<(Truth, ConduitResult, Vec<UnroutedHomerun>), String> {
    let set = crate::generate(seed)?;
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = crate::sheet_reader::recognise_fixture_set(&dxf);
    let mut res = read_conduits(&dxf, &map);
    // Anchor homeruns to placed devices and equipment.
    let dev_anchors = device_anchors_from_truth(&set.truth);
    let eq_anchors = equipment_anchors_from_truth(&set.truth);
    let unrouted = anchor_homeruns(&mut res, &dev_anchors, &eq_anchors);
    let ceil = crate::ceiling_reader::read_ceilings(&dxf, &map);
    let t = set.truth;
    let levels = t.building.levels.clone();
    assign_heights(&mut res, &|code| {
        let i = levels.iter().position(|l| l.code == code)?;
        let e = levels.get(i)?.elevation_ft;
        let next = levels.get(i + 1).map_or(e + 13.0, |l| l.elevation_ft);
        Some(context_from_ceilings(&ceil, code, e, next - e - 2.0))
    });
    Ok((t, res, unrouted))
}

/// How the reader did against the truth.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConduitScore {
    pub conduits_truth: usize,
    pub conduits_found: usize,
    /// Truth runs found (both ends within 1' on the same level).
    pub conduit_recall: f64,
    /// Found runs that are truth runs.
    pub conduit_precision: f64,
    /// Of the tagged truth runs found, the share whose size is right.
    pub size_accuracy: f64,
    pub homeruns_truth: usize,
    pub homeruns_found: usize,
    /// Truth homeruns found (label, level and origin within 1').
    pub homerun_recall: f64,
    pub homerun_precision: f64,
    /// Of the homeruns found, the share routed to the right panel.
    pub panel_accuracy: f64,
    /// Share of found conduit with a height (any source).
    pub heights_assigned: f64,
}

/// Score a result against the truth's drawn conduit and homeruns.
pub fn score(t: &Truth, res: &ConduitResult) -> ConduitScore {
    let tol = 1.0;
    let same_ends = |a: ([f64; 2], [f64; 2]), b: ([f64; 2], [f64; 2])| {
        (dist(a.0, b.0) <= tol && dist(a.1, b.1) <= tol) || (dist(a.0, b.1) <= tol && dist(a.1, b.0) <= tol)
    };
    let drawn: Vec<&crate::truth::ElecConduit> = t.elec_conduits.iter().filter(|c| !c.sheet.is_empty()).collect();
    let mut found_c = 0;
    let (mut tagged, mut sized) = (0, 0);
    for c in &drawn {
        if let Some(f) = res.conduits.iter().find(|f| f.level == c.level && same_ends((f.from_ft, f.to_ft), (c.from_ft, c.to_ft))) {
            found_c += 1;
            if c.tag.is_some() {
                tagged += 1;
                sized += usize::from((f.size_in - c.size_in).abs() < 1e-6);
            }
        }
    }
    let true_c =
        res.conduits.iter().filter(|f| drawn.iter().any(|c| f.level == c.level && same_ends((f.from_ft, f.to_ft), (c.from_ft, c.to_ft)))).count();
    let circuits: Vec<&crate::truth::ElecCircuit> = t.elec_circuits.iter().filter(|c| !c.sheet.is_empty()).collect();
    let panel_tag = |id: &str| t.elec_panels.iter().find(|p| p.id == id).map(|p| p.tag.clone()).unwrap_or_else(|| id.to_string());
    let matches =
        |h: &FoundHomerun, c: &crate::truth::ElecCircuit| h.level == c.level && h.label == c.label && dist(h.drawn_origin_ft, c.homerun_ft) <= tol;
    let found_h = circuits.iter().filter(|c| res.homeruns.iter().any(|h| matches(h, c))).count();
    let true_h: Vec<(&FoundHomerun, &&crate::truth::ElecCircuit)> =
        res.homeruns.iter().filter_map(|h| circuits.iter().find(|c| matches(h, c)).map(|c| (h, c))).collect();
    let routed = true_h.iter().filter(|(h, c)| h.panel_ft.is_some() && h.panel.eq_ignore_ascii_case(&panel_tag(&c.panel))).count();
    let share = |n: usize, d: usize| if d == 0 { 1.0 } else { n as f64 / d as f64 };
    ConduitScore {
        conduits_truth: drawn.len(),
        conduits_found: res.conduits.len(),
        conduit_recall: share(found_c, drawn.len()),
        conduit_precision: share(true_c, res.conduits.len()),
        size_accuracy: share(sized, tagged),
        homeruns_truth: circuits.len(),
        homeruns_found: res.homeruns.len(),
        homerun_recall: share(found_h, circuits.len()),
        homerun_precision: share(true_h.len(), res.homeruns.len()),
        panel_accuracy: share(routed, true_h.len()),
        heights_assigned: share(res.conduits.iter().filter(|c| c.bottom_ft.is_some()).count(), res.conduits.len()),
    }
}

/// The runs as building spec MEP systems and runs (`mep.systems` / `mep.runs` for `bspec.set`:
/// drawing inches, building feet × 12; `level_names` maps level codes to the spec's names),
/// and the guesses to record (`guess`): every homerun route, and every height or size a rule
/// chose, is an assumption. Runs without a height are left out.
///
/// Each run carries its plan `objects` (hex handles) and a per-run `source` describing where
/// its size and height came from (`"tag"` / `"default"` for size, `"Note"` / `"CeilingRule"` /
/// `"StackRule"` for height, `"assumed route"` for homeruns).
pub fn spec_runs(res: &ConduitResult, level_names: &[(&str, &str)]) -> (Vec<serde_json::Value>, Vec<serde_json::Value>, Vec<(String, String)>) {
    let systems = vec![
        serde_json::json!({"name": CONDUIT_SYSTEM, "layer": CONDUIT_SYSTEM, "kind": "conduit", "size_in": [DEFAULT_SIZE_IN, 0], "bottom_ft": 9}),
        serde_json::json!({"name": HOMERUN_SYSTEM, "layer": HOMERUN_SYSTEM, "kind": "cable", "size_in": [0.5, 0], "bottom_ft": 9}),
    ];
    let level_of = |code: &str| level_names.iter().find(|(c, _)| *c == code).map(|x| x.1);
    let pt = |p: [f64; 2]| serde_json::json!([p[0] * 12.0, p[1] * 12.0]);
    let height_src = |s: &Option<HeightSource>| match s {
        Some(HeightSource::Note) => "Note",
        Some(HeightSource::Ceiling) => "CeilingRule",
        Some(HeightSource::Structure) => "Structure",
        _ => "StackRule",
    };
    let mut runs = Vec::new();
    let (mut ruled, mut untagged) = (0, 0);
    for c in &res.conduits {
        let (Some(level), Some(b)) = (level_of(&c.level), c.bottom_ft) else { continue };
        ruled += usize::from(c.height_source != Some(HeightSource::Note));
        untagged += usize::from(!c.size_from_tag);
        let size_src = if c.size_from_tag { "tag" } else { "default" };
        let mut run = serde_json::json!({
            "system": CONDUIT_SYSTEM, "level": level,
            "points": [pt(c.from_ft), pt(c.to_ft)],
            "size_in": [c.size_in, 0], "bottoms_ft": [b],
            "source": format!("size {size_src}, height {}", height_src(&c.height_source)),
        });
        if !c.objects.is_empty() {
            run["objects"] = serde_json::json!(c.objects);
        }
        runs.push(run);
    }
    let mut homeruns = 0;
    for h in &res.homeruns {
        let Some(level) = level_of(&h.level) else { continue };
        if h.route_ft.len() < 2 || h.bottoms_ft.len() + 1 != h.route_ft.len() {
            continue;
        }
        homeruns += 1;
        let mut run = serde_json::json!({
            "system": HOMERUN_SYSTEM, "level": level,
            "points": h.route_ft.iter().map(|p| pt(*p)).collect::<Vec<_>>(),
            "size_in": [0.5, 0], "bottoms_ft": h.bottoms_ft,
            "source": format!(
                "assumed route, origin {}, panel {}",
                if h.origin_source.is_empty() { "E-sheet" } else { &h.origin_source },
                if h.panel_source.is_empty() { "E-sheet" } else { &h.panel_source },
            ),
        });
        if !h.objects.is_empty() {
            run["objects"] = serde_json::json!(h.objects);
        }
        runs.push(run);
    }
    let mut guesses = Vec::new();
    if homeruns > 0 {
        guesses.push((
            format!("mep/{HOMERUN_SYSTEM}"),
            format!("{homeruns} homerun routes assumed: the plans show only an arrow toward the panel, so each runs along x then y to it — not a surveyed path"),
        ));
    }
    if ruled > 0 || untagged > 0 {
        guesses.push((
            format!("mep/{CONDUIT_SYSTEM}"),
            format!("branch conduit is diagrammatic on the electrical plans: {ruled} heights set by the trade rules (above the ceiling), {untagged} untagged runs taken as {DEFAULT_SIZE_IN}\""),
        ));
    }
    (systems, runs, guesses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_and_labels_parse() {
        assert_eq!(parse_conduit_tag("3/4\"C"), Some(0.75));
        assert_eq!(parse_conduit_tag("1\"C"), Some(1.0));
        assert_eq!(parse_conduit_tag("1-1/4\"C"), Some(1.25));
        assert_eq!(parse_conduit_tag("1 1/4\" EMT"), Some(1.25));
        assert_eq!(parse_conduit_tag("2\" CW"), None);
        assert_eq!(parse_conduit_tag("(2) 3\"C"), None);
        assert_eq!(parse_conduit_tag("12x4 CT"), None);
        assert_eq!(parse_conduit_tag("0/0\"C"), None);
        assert_eq!(parse_conduit_tag("99\"C"), None);
        assert_eq!(parse_boc("BOC @ 9'-8\""), Some(9.0 + 8.0 / 12.0));
        assert_eq!(parse_boc("BOC @ 10'"), Some(10.0));
        assert_eq!(parse_boc("BOT @ 9'-8\""), None);
        assert_eq!(parse_circuit_label("L1/1"), Some(("L1".into(), "1".into())));
        assert_eq!(parse_circuit_label("LP2/12,14"), Some(("LP2".into(), "12,14".into())));
        assert_eq!(parse_circuit_label("1/2"), None);
        assert_eq!(parse_circuit_label("1/8\" = 1'-0\""), None);
        assert_eq!(parse_circuit_label("L1/"), None);
    }
}
