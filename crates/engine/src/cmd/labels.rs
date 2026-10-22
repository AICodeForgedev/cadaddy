//! Labels from the model (Q335): typed panel directories (circuit, description, room or area),
//! device plate circuit labels, equipment and disconnect labels, panel nameplates, and one arc
//! flash label per piece of labeled gear carrying the incident energy and PPE from the engineer's
//! arc flash study (imported: CADaddy doesn't calculate it). Generated from the building spec's
//! gear, panel circuits (`mep.equipment[].circuits`), devices (`mep.devices`) and feeders, every
//! time: a circuit that changes changes its labels, and `labels.list` says which changed since
//! they were last printed.
//!
//! - `labels.list {set?}`: every label, its text and whether it is new, changed or as printed.
//! - `labels.export {path, format?, set?, changed_only?, mark?}`: PDF sheets (Letter: directories
//!   a page each, 2" × 4" equipment labels and nameplates ten a sheet, ½" × 1¾" device labels
//!   eighty a sheet, 4" × 6" arc flash labels two a sheet), PNG pages, a CSV for label printers
//!   (12 mm tape and the rest), or DXF.
//! - `labels.arcflash {rows | path}`: the study's results (CSV or rows).
//! - `labels.check {review?}`: what's missing (study data for labeled gear, devices on circuits
//!   the schedule doesn't have); `review` makes each an RFI (a document request).
//! - `labels.bom`: the BOM lines the labels need: the label printer, tape cartridges for the
//!   device labels, label stock, directory cards and an arc flash label per labeled item (also on
//!   the MEP takeoff, part `labels`).
//!
//! The text follows the project rules (`naming/label_case`: `upper` or `as_is`;
//! `naming/room_label`: `RM {room}`; `naming/circuit_label`: `{panel}-{circuit}`;
//! `custom/label_tape_mm`, `custom/label_cartridge_m`).

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::projrules::RuleDomain;
use cadaddy_doc::{Common, Drawing, EntityKind, HAlign, Space, Text, VAlign};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::{ArcFlash, BuildingSpec, MepEquipment};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Most labels made at once.
const MAX_LABELS: usize = 100_000;
/// Most pages a sheet export draws.
const MAX_PAGES: usize = 2_000;

/// What a label set is printed on: width × height (inches) and labels per Letter sheet across × down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Media {
    pub name: &'static str,
    pub size_in: [f64; 2],
    pub grid: [usize; 2],
    /// Sheet margins: left and top, and the gap between labels across and down.
    pub margin_in: [f64; 2],
    pub gap_in: [f64; 2],
}

pub const DIRECTORY: Media = Media { name: "letter directory", size_in: [8.5, 11.0], grid: [1, 1], margin_in: [0.0, 0.0], gap_in: [0.0, 0.0] };
pub const EQUIPMENT_2X4: Media = Media { name: "2x4", size_in: [4.0, 2.0], grid: [2, 5], margin_in: [0.16, 0.5], gap_in: [0.19, 0.0] };
pub const DEVICE_HALF: Media = Media { name: "0.5x1.75", size_in: [1.75, 0.5], grid: [4, 20], margin_in: [0.28, 0.5], gap_in: [0.3, 0.0] };
pub const ARC_FLASH_4X6: Media = Media { name: "4x6", size_in: [6.0, 4.0], grid: [1, 2], margin_in: [1.25, 1.0], gap_in: [0.0, 1.0] };

/// One label.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// `dir:P1`, `dev:R-101-1`, `eq:DS-1`, `np:P1`, `af:MDP`.
    pub id: String,
    /// `directories`, `devices`, `equipment`, `nameplates`, `arc_flash`.
    pub set: &'static str,
    pub lines: Vec<String>,
    pub media: Media,
}

impl Label {
    fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// One row of a panel's directory.
#[derive(Clone, Debug, PartialEq)]
pub struct DirRow {
    pub circuit: String,
    pub description: String,
    pub room: String,
}

/// Something the labels can't be made right without.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub label: String,
    pub about: String,
    pub message: String,
    pub error: bool,
}

/// The text rules: from the project rules, else the defaults.
struct TextRules {
    upper: bool,
    room: String,
    circuit: String,
    tape_mm: f64,
    cartridge_m: f64,
}

impl TextRules {
    fn of(d: &Drawing) -> TextRules {
        let get = |dom: RuleDomain, key: &str| d.project_rules.resolve(dom, key).map(|v| v.value.trim().to_string()).filter(|v| !v.is_empty());
        let num = |dom: RuleDomain, key: &str, def: f64| {
            get(dom, key).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0 && *v < 1e4).unwrap_or(def)
        };
        TextRules {
            upper: get(RuleDomain::Naming, "label_case").is_none_or(|v| !v.eq_ignore_ascii_case("as_is")),
            room: get(RuleDomain::Naming, "room_label").unwrap_or_else(|| "RM {room}".into()),
            circuit: get(RuleDomain::Naming, "circuit_label").unwrap_or_else(|| "{panel}-{circuit}".into()),
            tape_mm: num(RuleDomain::Custom, "label_tape_mm", 12.0),
            cartridge_m: num(RuleDomain::Custom, "label_cartridge_m", 8.0),
        }
    }

    fn case(&self, s: &str) -> String {
        let s: String = s.trim().chars().take(200).collect();
        if self.upper { s.to_uppercase() } else { s }
    }

    fn room(&self, room: &str) -> String {
        if room.trim().is_empty() { String::new() } else { self.case(&self.room.replace("{room}", room.trim())) }
    }

    fn circuit(&self, panel: &str, circuit: &str) -> String {
        self.case(&self.circuit.replace("{panel}", panel.trim()).replace("{circuit}", circuit.trim()))
    }
}

fn is_gear(e: &MepEquipment) -> bool {
    super::gear::is_gear(e) || !e.circuits.is_empty()
}

/// Where gear is fed from (its incoming feeder's source; `UTILITY` for the service).
fn fed_from(sp: &BuildingSpec, tag: &str) -> Option<String> {
    sp.mep
        .feeders
        .iter()
        .find(|f| f.to.trim().eq_ignore_ascii_case(tag))
        .map(|f| if f.from.trim().is_empty() { "UTILITY".to_string() } else { f.from.trim().to_string() })
}

fn circuit_number(c: &str) -> (u64, String) {
    (c.trim().split(|ch: char| !ch.is_ascii_digit()).next().and_then(|n| n.parse().ok()).unwrap_or(u64::MAX), c.trim().to_string())
}

/// A panel's directory rows, by circuit number; a multi-pole circuit lists every pole it takes.
fn directory(e: &MepEquipment, r: &TextRules) -> Vec<DirRow> {
    let mut rows: Vec<DirRow> = e
        .circuits
        .iter()
        .take(2_000)
        .map(|c| {
            let poles = c.poles.clamp(1, 3);
            let circuit = match c.number.trim().parse::<u64>() {
                Ok(n) if poles > 1 => (0..u64::from(poles)).map(|k| (n + 2 * k).to_string()).collect::<Vec<_>>().join(","),
                _ => c.number.trim().to_string(),
            };
            DirRow { circuit, description: r.case(&c.description), room: r.room(&c.room) }
        })
        .collect();
    rows.sort_by_key(|r| circuit_number(&r.circuit));
    rows
}

/// The circuit numbers a panel's schedule has (every pole of a multi-pole breaker).
fn poles(e: &MepEquipment) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for c in e.circuits.iter().take(2_000) {
        match c.number.trim().parse::<u64>() {
            Ok(n) => {
                for k in 0..u64::from(c.poles.clamp(1, 3)) {
                    out.insert((n + 2 * k).to_string());
                }
            }
            Err(_) => {
                out.insert(c.number.trim().to_string());
            }
        }
    }
    out
}

fn prop<'a>(e: &'a MepEquipment, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| e.properties.iter().find(|(x, v)| x.eq_ignore_ascii_case(k) && !v.trim().is_empty()).map(|(_, v)| v.trim()))
}

fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let r = (v * 10.0).round() / 10.0;
    if (r - r.round()).abs() < 1e-9 { format!("{}", r.round() as i64) } else { format!("{r}") }
}

/// The arc flash label's lines, or `None` without study data.
fn arc_flash_lines(tag: &str, a: &ArcFlash, r: &TextRules) -> Vec<String> {
    let mut l = vec!["WARNING".to_string(), "ARC FLASH AND SHOCK HAZARD".into(), "APPROPRIATE PPE REQUIRED".into(), r.case(tag)];
    l.push(format!("INCIDENT ENERGY: {} CAL/CM² AT {} IN", fmt_num(a.incident_energy), fmt_num(a.working_distance_in)));
    l.push(format!("ARC FLASH BOUNDARY: {} IN", fmt_num(a.arc_flash_boundary_in)));
    if !a.ppe.trim().is_empty() {
        l.push(format!("PPE: {}", r.case(&a.ppe)));
    }
    if !a.voltage.trim().is_empty() {
        l.push(format!("NOMINAL VOLTAGE: {}", r.case(&a.voltage)));
    }
    if !a.limited_approach.trim().is_empty() {
        l.push(format!("LIMITED APPROACH: {}", r.case(&a.limited_approach)));
    }
    if !a.restricted_approach.trim().is_empty() {
        l.push(format!("RESTRICTED APPROACH: {}", r.case(&a.restricted_approach)));
    }
    let study = [a.study.trim(), a.date.trim()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(", ");
    if !study.is_empty() {
        l.push(format!("STUDY: {}", r.case(&study)));
    }
    l
}

/// Every label of the model, the panels' directories, and what's missing.
pub fn make(sp: &BuildingSpec, d: &Drawing) -> (Vec<Label>, BTreeMap<String, Vec<DirRow>>, Vec<Problem>) {
    let r = TextRules::of(d);
    let mut labels: Vec<Label> = Vec::new();
    let mut dirs = BTreeMap::new();
    let mut problems = Vec::new();
    let gear: Vec<&MepEquipment> = sp.mep.equipment.iter().filter(|e| is_gear(e)).collect();
    for e in &gear {
        let tag = e.tag.trim();
        let kind = e.kind.to_ascii_lowercase();
        let voltage = prop(e, &["VOLTAGE", "VOLTS"]).unwrap_or_default();
        let rating = prop(e, &["RATING", "AMPS", "KVA", "SIZE (A)"]).unwrap_or_default();
        let from = fed_from(sp, tag).map(|f| format!("FED FROM {}", r.case(&f)));
        if !e.circuits.is_empty() {
            let rows = directory(e, &r);
            let mut lines = vec![format!("PANEL {} DIRECTORY", r.case(tag))];
            lines.push([voltage, rating].iter().filter(|s| !s.is_empty()).map(|s| r.case(s)).collect::<Vec<_>>().join("  "));
            lines.extend(from.clone());
            lines.extend(rows.iter().map(|x| format!("{}\t{}\t{}", x.circuit, x.description, x.room)));
            labels.push(Label { id: format!("dir:{tag}"), set: "directories", lines, media: DIRECTORY });
            dirs.insert(tag.to_string(), rows);
        }
        let lines: Vec<String> =
            [r.case(tag), r.case(voltage), r.case(rating), from.clone().unwrap_or_default()].into_iter().filter(|s| !s.is_empty()).collect();
        if matches!(kind.as_str(), "panel" | "panelboard" | "switchboard" | "switchgear") || !e.circuits.is_empty() {
            labels.push(Label { id: format!("np:{tag}"), set: "nameplates", lines, media: EQUIPMENT_2X4 });
        } else {
            labels.push(Label { id: format!("eq:{tag}"), set: "equipment", lines, media: EQUIPMENT_2X4 });
        }
        // Every labeled piece of gear gets an arc flash label: from the study, or a request for it.
        match sp.mep.arc_flash.iter().find(|a| a.equipment.trim().eq_ignore_ascii_case(tag)) {
            Some(a) if a.incident_energy.is_finite() && a.incident_energy >= 0.0 && a.working_distance_in > 0.0 => {
                labels.push(Label { id: format!("af:{tag}"), set: "arc_flash", lines: arc_flash_lines(tag, a, &r), media: ARC_FLASH_4X6 });
            }
            Some(_) => problems.push(Problem {
                label: format!("af:{tag}"),
                about: tag.into(),
                message: format!("{tag}'s arc flash study data is incomplete (incident energy and working distance needed)"),
                error: true,
            }),
            None => problems.push(Problem {
                label: format!("af:{tag}"),
                about: tag.into(),
                message: format!("no arc flash study data for {tag}: its arc flash label can't be made (request the study's results)"),
                error: true,
            }),
        }
    }
    // Device plates: the circuit each is on.
    for dv in sp.mep.devices.iter().take(MAX_LABELS) {
        if dv.panel.trim().is_empty() || dv.circuit.trim().is_empty() {
            problems.push(Problem {
                label: format!("dev:{}", dv.tag),
                about: dv.tag.clone(),
                message: format!("{} has no panel or circuit", dv.tag),
                error: false,
            });
            continue;
        }
        match gear.iter().find(|e| e.tag.trim().eq_ignore_ascii_case(dv.panel.trim())) {
            Some(p) if !p.circuits.is_empty() => {
                let have = poles(p);
                let missing: Vec<&str> = dv.circuit.split(',').map(str::trim).filter(|c| !c.is_empty() && !have.contains(*c)).collect();
                if !missing.is_empty() {
                    problems.push(Problem {
                        label: format!("dev:{}", dv.tag),
                        about: dv.tag.clone(),
                        message: format!("{} is on {}-{}, which {}'s schedule doesn't have", dv.tag, dv.panel, missing.join(","), dv.panel),
                        error: true,
                    });
                }
            }
            Some(_) => {}
            None => problems.push(Problem {
                label: format!("dev:{}", dv.tag),
                about: dv.tag.clone(),
                message: format!("{} is fed from {}, which isn't in the equipment", dv.tag, dv.panel),
                error: true,
            }),
        }
        labels.push(Label { id: format!("dev:{}", dv.tag), set: "devices", lines: vec![r.circuit(&dv.panel, &dv.circuit)], media: DEVICE_HALF });
        if labels.len() >= MAX_LABELS {
            break;
        }
    }
    (labels, dirs, problems)
}

fn status(sp: &BuildingSpec, l: &Label) -> &'static str {
    match sp.mep.labels_printed.get(&l.id) {
        None => "new",
        Some(t) if *t == l.text() => "printed",
        Some(_) => "changed",
    }
}

const SETS: [&str; 5] = ["directories", "devices", "equipment", "nameplates", "arc_flash"];

fn spec(cmd: &str, s: &Session) -> Result<BuildingSpec> {
    let text = s.doc()?.building_spec.clone().ok_or_else(|| bad(cmd, "the drawing has no building spec (bspec.set)"))?;
    BuildingSpec::from_json(&text).map_err(|e| bad(cmd, e))
}

fn set_param(cmd: &str, p: &Value) -> Result<Option<&'static str>> {
    match str_param(p, "set").map(str::trim).filter(|v| !v.is_empty() && !v.eq_ignore_ascii_case("all")) {
        None => Ok(None),
        Some(v) => SETS
            .iter()
            .find(|s| s.eq_ignore_ascii_case(v) || s.trim_end_matches('s').eq_ignore_ascii_case(v))
            .copied()
            .map(Some)
            .ok_or_else(|| bad(cmd, format!("set is all, {}", SETS.join(", ")))),
    }
}

fn problems_json(p: &[Problem]) -> Vec<Value> {
    p.iter()
        .map(|x| json!({"label": x.label, "about": x.about, "message": x.message, "severity": if x.error { "error" } else { "warning" }}))
        .collect()
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("labels.list", s)?;
    let only = set_param("labels.list", p)?;
    let (labels, dirs, problems) = make(&sp, s.doc()?);
    let mut counts = serde_json::Map::new();
    for set in SETS {
        counts.insert(set.into(), json!(labels.iter().filter(|l| l.set == set).count()));
    }
    let list: Vec<Value> = labels
        .iter()
        .filter(|l| only.is_none_or(|o| l.set == o))
        .map(|l| json!({"id": l.id, "set": l.set, "media": l.media.name, "lines": l.lines, "status": status(&sp, l)}))
        .collect();
    let changed: Vec<&str> = labels.iter().filter(|l| status(&sp, l) == "changed").map(|l| l.id.as_str()).collect();
    let directories: serde_json::Map<String, Value> = dirs
        .iter()
        .map(|(k, rows)| {
            (k.clone(), Value::Array(rows.iter().map(|r| json!({"circuit": r.circuit, "description": r.description, "room": r.room})).collect()))
        })
        .collect();
    Ok(json!({"labels": list, "counts": counts, "changed": changed, "directories": directories, "problems": problems_json(&problems)}))
}

// ---------------- sheets ----------------

fn text(d: &mut Drawing, at: Vec2, h: f64, s: &str, align: HAlign) {
    let t = Text {
        insert: Vec3::new(at.x, at.y, 0.0),
        align_pt: (align != HAlign::Left).then(|| Vec3::new(at.x, at.y, 0.0)),
        height: h,
        value: s.chars().take(300).collect(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: align,
        valign: if align == HAlign::Left { VAlign::Baseline } else { VAlign::Middle },
    };
    let _ = d.add(&Space::Model, Common::default(), EntityKind::Text(t));
}

fn rect(d: &mut Drawing, lo: Vec2, hi: Vec2) {
    for (a, b) in [(lo, Vec2::new(hi.x, lo.y)), (Vec2::new(hi.x, lo.y), hi), (hi, Vec2::new(lo.x, hi.y)), (Vec2::new(lo.x, hi.y), lo)] {
        let _ =
            d.add(&Space::Model, Common::default(), EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(a.x, a.y, 0.0), b: Vec3::new(b.x, b.y, 0.0) }));
    }
}

/// A label's lines centred in its box, as large as they fit.
fn label_box(d: &mut Drawing, lo: Vec2, size: [f64; 2], lines: &[String]) {
    rect(d, lo, lo + Vec2::new(size[0], size[1]));
    let n = lines.len().max(1) as f64;
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1).max(1) as f64;
    // The stroke font's capitals run about 0.95 of the height wide, with their spacing.
    let h = (size[1] * 0.8 / (n * 1.35)).min((size[0] * 0.86) / (longest * 0.95)).min(0.5);
    let top = lo.y + size[1] / 2.0 + (n - 1.0) * h * 1.35 / 2.0;
    for (k, l) in lines.iter().enumerate() {
        text(d, Vec2::new(lo.x + size[0] / 2.0, top - k as f64 * h * 1.35), h, l, HAlign::Center);
    }
}

/// The labels laid out on Letter pages side by side: the drawing, and each page's window.
pub fn sheets(labels: &[Label], dirs: &BTreeMap<String, Vec<DirRow>>) -> (Drawing, Vec<[[f64; 2]; 2]>) {
    let mut d = Drawing::new_imperial();
    let mut windows: Vec<[[f64; 2]; 2]> = Vec::new();
    // A new Letter page beside the last: its lower left corner.
    let page = |windows: &mut Vec<[[f64; 2]; 2]>| -> Vec2 {
        let ox = windows.len() as f64 * 9.0;
        windows.push([[ox, 0.0], [ox + 8.5, 11.0]]);
        Vec2::new(ox, 0.0)
    };
    // Directories: a page each, odd circuits left, even right.
    for l in labels.iter().filter(|l| l.set == "directories") {
        if windows.len() >= MAX_PAGES {
            break;
        }
        let o = page(&mut windows);
        let panel = l.id.trim_start_matches("dir:");
        text(&mut d, o + Vec2::new(0.5, 10.3), 0.25, l.lines.first().map_or("", String::as_str), HAlign::Left);
        for (k, sub) in l.lines.iter().skip(1).take_while(|x| !x.contains('\t')).enumerate() {
            text(&mut d, o + Vec2::new(0.5, 9.95 - k as f64 * 0.22), 0.14, sub, HAlign::Left);
        }
        let rows = dirs.get(panel).cloned().unwrap_or_default();
        let (odd, even): (Vec<&DirRow>, Vec<&DirRow>) = rows.iter().partition(|r| circuit_number(&r.circuit).0 % 2 == 1);
        let (top, rh) = (9.3, 0.21);
        for (col, list) in [(0.0, odd), (3.9, even)] {
            let x = o.x + 0.5 + col;
            rect(&mut d, Vec2::new(x, top - rh * 41.0), Vec2::new(x + 3.6, top));
            for (i, r) in list.iter().take(40).enumerate() {
                let y = top - rh * (i as f64 + 1.0);
                let _ = d.add(
                    &Space::Model,
                    Common::default(),
                    EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 3.6, y, 0.0) }),
                );
                text(&mut d, Vec2::new(x + 0.05, y + 0.05), 0.1, &r.circuit, HAlign::Left);
                text(&mut d, Vec2::new(x + 0.5, y + 0.05), 0.1, &r.description.chars().take(26).collect::<String>(), HAlign::Left);
                text(&mut d, Vec2::new(x + 2.8, y + 0.05), 0.1, &r.room.chars().take(10).collect::<String>(), HAlign::Left);
            }
        }
    }
    // Label sheets: each medium's grid, page after page.
    // (Nameplates and equipment labels share the 2" × 4" stock.)
    for (sets, media) in [(&["nameplates", "equipment"][..], EQUIPMENT_2X4), (&["devices"][..], DEVICE_HALF), (&["arc_flash"][..], ARC_FLASH_4X6)] {
        let these: Vec<&Label> = labels.iter().filter(|l| sets.contains(&l.set)).collect();
        let per = media.grid[0] * media.grid[1];
        for chunk in these.chunks(per.max(1)) {
            if windows.len() >= MAX_PAGES {
                break;
            }
            let o = page(&mut windows);
            for (k, l) in chunk.iter().enumerate() {
                let (cx, cy) = (k % media.grid[0], k / media.grid[0]);
                let x = o.x + media.margin_in[0] + cx as f64 * (media.size_in[0] + media.gap_in[0]);
                let y = 11.0 - media.margin_in[1] - (cy as f64 + 1.0) * media.size_in[1] - cy as f64 * media.gap_in[1];
                label_box(&mut d, Vec2::new(x, y), media.size_in, &l.lines);
            }
        }
    }
    (d, windows)
}

/// A page of the sheets as a PNG, `dpi` pixels an inch.
pub fn page_png(d: &Drawing, window: [[f64; 2]; 2], dpi: f64) -> Option<Vec<u8>> {
    use cadaddy_render::raster::{RasterOptions, View};
    let world = cadaddy_geom::Bounds2::new(Vec2::new(window[0][0], window[0][1]), Vec2::new(window[1][0], window[1][1]));
    let (w, h) = (((world.width() * dpi).round() as u32).clamp(16, 8000), ((world.height() * dpi).round() as u32).clamp(16, 8000));
    let ropts = cadaddy_render::Options { tolerance: 0.5 / dpi, min_dash: 0.0, text: true, fill: true, ..Default::default() };
    let list = cadaddy_render::build_plot(d, &Space::Model, &ropts);
    let view = View::fit(&world, w, h, 0.0);
    let ro = RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 1.0, px_per_mm: 0.0, antialias: true };
    cadaddy_render::raster::render_png(&list, &view, &ro)
}

fn csv_field(t: &str) -> String {
    let formula = t.starts_with(['=', '+', '-', '@']) && t.parse::<f64>().is_err();
    let t = if formula { format!("'{t}") } else { t.to_string() };
    if t.contains([',', '"', '\n', '\r', '\t']) { format!("\"{}\"", t.replace('"', "\"\"")) } else { t }
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("labels.export", "path required"))?.to_string();
    let format = str_param(p, "format")
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| std::path::Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("pdf").to_ascii_lowercase());
    if !["pdf", "png", "csv", "dxf"].contains(&format.as_str()) {
        return Err(bad("labels.export", "format is pdf, png, csv or dxf"));
    }
    let only = set_param("labels.export", p)?;
    let mut sp = spec("labels.export", s)?;
    let (labels, dirs, problems) = make(&sp, s.doc()?);
    let changed_only = bool_or(p, "changed_only", false);
    let picked: Vec<Label> =
        labels.into_iter().filter(|l| only.is_none_or(|o| l.set == o)).filter(|l| !changed_only || status(&sp, l) != "printed").collect();
    if picked.is_empty() {
        return Err(bad(
            "labels.export",
            if changed_only {
                "no labels changed since they were printed"
            } else {
                "there are no labels to make (gear, circuits or devices in the building spec)"
            },
        ));
    }
    let mut out = json!({"format": format, "labels": picked.len(), "problems": problems_json(&problems)});
    let write = |path: &str, bytes: &[u8]| -> Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::writefile::write(path, bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (path, bytes);
            Err(bad("labels.export", "files can't be written on the web"))
        }
    };
    match format.as_str() {
        "csv" => {
            // One row a label: for label-printer software (the tape width for device labels).
            let mut csv = String::from("set,id,media,tape_mm,line1,line2,line3,line4,line5,line6,line7,line8,line9,line10,line11\n");
            let tape = TextRules::of(s.doc()?).tape_mm;
            for l in &picked {
                let lines: Vec<String> = if l.set == "directories" { l.lines.iter().take(3).cloned().collect() } else { l.lines.clone() };
                let mut row =
                    vec![l.set.to_string(), l.id.clone(), l.media.name.to_string(), if l.set == "devices" { fmt_num(tape) } else { String::new() }];
                row.extend(lines.into_iter().take(11));
                csv += &row.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(",");
                csv.push('\n');
            }
            write(&path, csv.as_bytes())?;
            out["path"] = json!(path);
        }
        _ => {
            let (d, windows) = sheets(&picked, &dirs);
            out["pages"] = json!(windows.len());
            match format.as_str() {
                "pdf" => {
                    let plot = super::file::io()
                        .and_then(|h| h.plot)
                        .ok_or_else(|| bad("labels.export", "PDF export isn't available in this build (export PNG or DXF)"))?;
                    let pages: Vec<Value> = windows.iter().map(|w| json!({"window": w, "paper": "Letter", "landscape": false})).collect();
                    let pdf =
                        plot(&d, &Space::Model, &json!({"pages": pages, "margins": [0.0, 0.0, 0.0, 0.0], "lineweights": false, "stamp": false}))
                            .map_err(|e| bad("labels.export", e))?;
                    write(&path, &pdf)?;
                    out["path"] = json!(path);
                }
                "png" => {
                    // A PNG a page: `name-1.png`, `name-2.png`… (one page: the name itself).
                    let stem = path.strip_suffix(".png").unwrap_or(&path).to_string();
                    let mut paths = Vec::new();
                    for (k, w) in windows.iter().enumerate() {
                        let png = page_png(&d, *w, 100.0).ok_or_else(|| bad("labels.export", "rendering the labels failed"))?;
                        let pth = if windows.len() == 1 { path.clone() } else { format!("{stem}-{}.png", k + 1) };
                        write(&pth, &png)?;
                        paths.push(pth);
                    }
                    out["paths"] = json!(paths);
                }
                _ => {
                    let hooks = super::file::io().ok_or_else(|| bad("labels.export", "file formats are not available in this build"))?;
                    let bytes = (hooks.write)(&d, "labels.dxf").map_err(|e| bad("labels.export", e))?;
                    write(&path, &bytes)?;
                    out["path"] = json!(path);
                }
            }
        }
    }
    // Printed: remember what each label said, so a change shows.
    if bool_or(p, "mark", true) {
        for l in &picked {
            sp.mep.labels_printed.insert(l.id.clone(), l.text());
        }
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(out)
}

// ---------------- arc flash data ----------------

/// A CSV line's fields (quoted fields, doubled quotes).
fn csv_line(l: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    let mut chars = l.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, q) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            ('"', _) => q = !q,
            (',', false) => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|f| f.trim().to_string()).collect()
}

fn arc_from(row: &BTreeMap<String, String>) -> Option<ArcFlash> {
    let get = |keys: &[&str]| {
        keys.iter().find_map(|k| row.iter().find(|(c, v)| c.contains(k) && !v.is_empty()).map(|(_, v)| v.clone())).unwrap_or_default()
    };
    let num = |keys: &[&str]| {
        get(keys).trim_end_matches(|c: char| !c.is_ascii_digit()).trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0 && *v < 1e6)
    };
    let equipment = get(&["equipment", "bus", "tag"]);
    if equipment.is_empty() {
        return None;
    }
    Some(ArcFlash {
        equipment,
        incident_energy: num(&["incident", "energy"]).unwrap_or(f64::NAN),
        working_distance_in: num(&["working"]).unwrap_or(0.0),
        arc_flash_boundary_in: num(&["boundary", "afb"]).unwrap_or(0.0),
        ppe: get(&["ppe"]),
        voltage: get(&["volt"]),
        limited_approach: get(&["limited"]),
        restricted_approach: get(&["restricted"]),
        study: get(&["study"]),
        date: get(&["date"]),
    })
}

fn run_arcflash(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sp = spec("labels.arcflash", s)?;
    let rows: Vec<BTreeMap<String, String>> = if let Some(a) = p.get("rows").and_then(Value::as_array) {
        a.iter()
            .take(10_000)
            .filter_map(Value::as_object)
            .map(|o| {
                o.iter()
                    .map(|(k, v)| (k.to_ascii_lowercase().replace([' ', '_', '-'], ""), v.as_str().map_or_else(|| v.to_string(), str::to_string)))
                    .collect()
            })
            .collect()
    } else if let Some(path) = str_param(p, "path") {
        let bytes = crate::readfile::read(path.trim()).map_err(|e| bad("labels.arcflash", format!("{path}: {e}")))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let header: Vec<String> =
            lines.next().map(csv_line).unwrap_or_default().into_iter().map(|h| h.to_ascii_lowercase().replace([' ', '_', '-'], "")).collect();
        lines.take(10_000).map(|l| header.iter().cloned().zip(csv_line(l)).collect()).collect()
    } else {
        return Err(bad("labels.arcflash", "rows or path (a CSV from the arc flash study) required"));
    };
    let known: BTreeSet<String> = sp.mep.equipment.iter().map(|e| e.tag.trim().to_ascii_uppercase()).collect();
    let (mut imported, mut replaced, mut unknown, mut incomplete) = (0usize, 0usize, Vec::new(), Vec::new());
    for row in &rows {
        let Some(a) = arc_from(row) else { continue };
        if !known.contains(&a.equipment.trim().to_ascii_uppercase()) {
            unknown.push(a.equipment.clone());
            continue;
        }
        if !(a.incident_energy.is_finite() && a.working_distance_in > 0.0) {
            incomplete.push(a.equipment.clone());
        }
        let a = ArcFlash { incident_energy: if a.incident_energy.is_finite() { a.incident_energy } else { -1.0 }, ..a };
        match sp.mep.arc_flash.iter_mut().find(|x| x.equipment.trim().eq_ignore_ascii_case(a.equipment.trim())) {
            Some(x) => {
                *x = a;
                replaced += 1;
            }
            None => {
                sp.mep.arc_flash.push(a);
                imported += 1;
            }
        }
    }
    if imported + replaced > 0 {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({"imported": imported, "replaced": replaced, "unknown_equipment": unknown, "incomplete": incomplete}))
}

fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", h as u32)
}

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sp = spec("labels.check", s)?;
    let (_, _, problems) = make(&sp, s.doc()?);
    let errors = problems.iter().filter(|x| x.error).count();
    let mut rfis = Vec::new();
    if bool_or(p, "review", false) {
        for x in problems.iter().filter(|x| x.error) {
            let id = format!("RFI-L{}", hash(&format!("{}|{}", x.label, x.message)));
            if sp.rfis.iter().any(|r| r.id == id) || sp.rfis.len() >= cadaddy_walk3d::rfi::MAX_RFIS {
                continue;
            }
            let g = sp.mep.equipment.iter().find(|e| e.tag.eq_ignore_ascii_case(&x.about));
            let kind = cadaddy_walk3d::RfiKind::LabelData;
            sp.rfis.push(cadaddy_walk3d::Rfi {
                id: id.clone(),
                kind: Some(kind),
                severity: kind.severity(),
                level: g.map(|e| e.level.clone()).unwrap_or_default(),
                at: g.map_or([0.0, 0.0], |e| [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0]),
                about: format!("{} (labels)", x.about),
                issue: format!("Document request: {}", x.message),
                assumed: "Labels for it are held until the data comes; they're ordered with the rest once it does.".into(),
                proposed: Some(if x.label.starts_with("af:") {
                    "Send the arc flash study's results for this equipment (incident energy, working distance, boundary, PPE)".into()
                } else {
                    "Confirm the circuit".into()
                }),
                ..cadaddy_walk3d::Rfi::default()
            });
            rfis.push(id);
        }
        if !rfis.is_empty() {
            s.doc_mut()?.building_spec = Some(sp.to_json());
        }
    }
    Ok(json!({"ok": errors == 0, "errors": errors, "problems": problems_json(&problems), "rfis": rfis}))
}

// ---------------- the BOM ----------------

/// The BOM lines the labels need (Q326's required items): `[{item, unit, qty, note}]`.
pub fn bom(sp: &BuildingSpec, d: &Drawing) -> Vec<Value> {
    let (labels, _, problems) = make(sp, d);
    if labels.is_empty() && problems.is_empty() {
        return Vec::new();
    }
    let r = TextRules::of(d);
    let count = |set: &str| labels.iter().filter(|l| l.set == set).count();
    let mut out = vec![json!({"item": "Label printer (tool)", "unit": "ea", "qty": 1, "note": "for the device and equipment labels"})];
    let devices: Vec<&Label> = labels.iter().filter(|l| l.set == "devices").collect();
    if !devices.is_empty() {
        // Each tape label: its text at about 0.12" a character, with 0.4" of lead and trail.
        let inches: f64 = devices.iter().map(|l| l.lines.first().map_or(0, |t| t.chars().count()) as f64 * 0.12 + 0.4).sum();
        let per_cartridge = r.cartridge_m * 39.37;
        let cartridges = (inches / per_cartridge).ceil().max(1.0);
        out.push(json!({"item": format!("Label tape cartridge, {} mm", fmt_num(r.tape_mm)), "unit": "ea", "qty": cartridges, "note": format!("{} device labels, {:.0} in of tape, {} m a cartridge", devices.len(), inches, fmt_num(r.cartridge_m))}));
    }
    let eq = count("equipment") + count("nameplates");
    if eq > 0 {
        out.push(json!({"item": "Equipment labels and nameplates, 2\" x 4\"", "unit": "ea", "qty": eq, "note": format!("{} sheets of 10", eq.div_ceil(10))}));
    }
    let dirs = count("directories");
    if dirs > 0 {
        out.push(json!({"item": "Panel directory card, typed", "unit": "ea", "qty": dirs, "note": ""}));
    }
    // One arc flash label per labeled item: the ones made, and the ones waiting for the study.
    let waiting = problems.iter().filter(|p| p.error && p.label.starts_with("af:")).count();
    let af = count("arc_flash") + waiting;
    if af > 0 {
        let note =
            if waiting > 0 { format!("with the study's ratings; {waiting} waiting for study data") } else { "with the study's ratings".to_string() };
        out.push(json!({"item": "Arc flash label, 4\" x 6\"", "unit": "ea", "qty": af, "note": note}));
    }
    out
}

fn run_bom(s: &mut Session, _p: &Value) -> Result<Value> {
    let sp = spec("labels.bom", s)?;
    Ok(json!({"items": bom(&sp, s.doc()?)}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("labels.list", "Labels", run_list)
            .params("{set?: all|directories|devices|equipment|nameplates|arc_flash} → {labels: [{id, set, media, lines, status: new|changed|printed}], counts, changed: [id], directories: {panel: [{circuit, description, room}]}, problems}")
            .noundo(),
        CommandSpec::new("labels.export", "Print Labels", run_export)
            .params("{path, format?: pdf|png|csv|dxf (default from the extension), set?, changed_only?: bool, mark?: bool (remember them as printed; default true)} → {path|paths, format, labels, pages?, problems}"),
        CommandSpec::new("labels.arcflash", "Import Arc Flash Data", run_arcflash)
            .params("{rows: [{equipment, incident_energy (cal/cm²), working_distance_in, arc_flash_boundary_in, ppe, voltage, limited_approach, restricted_approach, study, date}]} | {path: CSV with those columns} → {imported, replaced, unknown_equipment, incomplete}"),
        CommandSpec::new("labels.check", "Check Labels", run_check).params("{review?: bool (each failure an RFI: a document request)} → {ok, errors, problems, rfis}"),
        CommandSpec::new("labels.bom", "Label BOM Lines", run_bom).params("{} → {items: [{item, unit, qty, note}]}: the label printer, tape, label stock, directory cards, arc flash labels").noundo(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_lines_and_numbers() {
        assert_eq!(csv_line(r#"MDP,"8.2",18,"Cat 2, arc rated","480 V ""3PH""""#), vec!["MDP", "8.2", "18", "Cat 2, arc rated", "480 V \"3PH\""]);
        assert_eq!((fmt_num(8.25), fmt_num(18.0), fmt_num(f64::NAN)), ("8.3".to_string(), "18".to_string(), String::new()));
        assert_eq!(circuit_number("12"), (12, "12".to_string()));
        assert_eq!(circuit_number("1,3,5").0, 1);
    }
}
