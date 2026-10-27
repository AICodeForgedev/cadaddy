//! The readers fill the elements' properties (`bim.props.fill`, Q6002).
//!
//! What the readers found on the prints goes on the model's elements, each value with where it
//! came from ([`cadaddy_doc::props`]):
//!
//! - **devices**: `model.build` makes one element per device the readers put in the building
//!   spec (a small box at its symbol, at its mounting height), carrying its type and type mark,
//!   the circuit printed beside it and the panel it names (`fed_from`), its sheet, its mounting
//!   height (the legend's or a general note's, else a marked default for its kind) and its 3D
//!   location; `bim.props.fill` adds the room it stands in and the wall it is on.
//! - **rooms** (`rooms.build`'s volumes): level, ceiling height and finishes as the room list
//!   reads them (schedule, spec, note, else a marked default).
//! - **doors**: the door number printed at the opening, width and height (computed), the rooms
//!   on its two sides.
//! - **walls**: the wall type the build gave it (from a wall tag on the plan, else the spec's
//!   type for its layer: a rule), its height and the rooms on its two sides.
//! - **ducts and pipes**: size, system, shape and elevation from their runs (a size read from a
//!   tag is `print`; a route height that was estimated is `default`).
//! - **air terminals**: their kind, neck size and the airflow printed beside them (`150 CFM`, or
//!   the number under the type in their tag), on the trade sheet they were read on.
//! - **equipment**: its tag, kind and the capacity on its schedule row.
//! - **plumbing fixtures**: placed at each scheduled fixture's tag on the plumbing plans (else the
//!   floor plans), with the fixture schedule's connection sizes (`schedules` parameter).
//!
//! Nothing that was not read is presented as read: assumptions are `default` or `rule`, what the
//! model gives is `computed`, and a person's value (`user`) is never replaced.

use std::collections::{BTreeMap, HashMap};

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, RawTag, Space};
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const FILL: &str = "bim.props.fill";

/// Most devices built as elements, and elements one fill looks at.
const MAX_DEVICES: usize = 100_000;
const MAX_ELEMENTS: usize = 500_000;
/// A device box: across the wall, along it and its height, inches.
const BOX_IN: [f64; 3] = [2.0, 4.0, 4.0];
/// How far a door number may be printed from its door, and a terminal's airflow from it, feet.
const DOOR_TAG_FT: f64 = 5.0;
const CFM_TAG_FT: f64 = 4.0;
/// How far from a terminal its tag (type over airflow) may be, and the airflow from the type, feet.
const TERM_TAG_FT: f64 = 3.0;
const TAG_LINE_FT: f64 = 1.5;
/// How far outside a room's outline a device, terminal or piece of equipment still belongs to it
/// (a device drawn on the room's wall), feet.
const NEAR_ROOM_FT: f64 = 2.0;
/// Sheets a point is looked for on, and schedule rows taken, at most.
const MAX_SCHEDULE_SHEETS: usize = 60;
const MAX_SCHEDULE_ROWS: usize = 5_000;
/// How far from a wall a device may stand and still be on it, feet.
const HOST_WALL_FT: f64 = 1.5;
/// How far beyond a wall's face the rooms on its sides are looked for, feet.
const SIDE_FT: f64 = 1.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(FILL, "Fill Element Properties", run_fill)
            .params("{schedules?: [{tag, sheet?, title?, row: {column: text}}], rooms?: [{number?, name?, level?, sheet?, at: [x, y], outline?: [[x, y]]}]} → {filled: {kind: {key: n}}, elements, devices, fixtures_placed}: what the readers found goes on the built model's elements (devices, rooms, doors, walls, ducts, pipes, terminals, equipment), each value with its source; plumbing fixtures are placed at their scheduled tags; `rooms` (the rooms a reading found on the plans, in plan coordinates) number the model's rooms and add the numbered ones it lacks; a person's values stay")
            .menu(&["Tools", "Building", "Fill Element Properties"]),
    ]
}

// ─── Devices: elements of their own ─────────────────────────────────────────

/// A device's mounting height and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Height {
    pub inches: f64,
    pub source: SourceKind,
    pub doc: String,
}

/// The kind of device a type mark names (`switch`, `fire alarm device`…; `device` when the mark
/// says nothing).
fn kind_of(mark: &str) -> &'static str {
    match mark.trim().to_ascii_uppercase().as_str() {
        "OS" | "OC" | "VS" | "PS" => "occupancy sensor",
        "DH" | "MH" => "door holder",
        "J" | "JB" => "junction box",
        "S" | "S3" | "S4" | "SD" | "DS" | "LV" => "switch",
        "FA" | "HS" | "SS" | "FS" | "ST" | "CM" => "fire alarm device",
        "TV" | "DA" | "TEL" | "WAP" | "CR" | "CAM" => "low voltage outlet",
        "EF" | "FCU" | "WH" | "PTAC" => "equipment connection",
        _ => "device",
    }
}

/// The device type a legend row's description names (`DUPLEX RECEPTACLE, +18" AFF` → `duplex
/// receptacle`); none when it names no device.
pub fn device_type_of(description: &str) -> Option<&'static str> {
    let u = description.to_ascii_uppercase();
    if u.chars().filter(char::is_ascii_alphabetic).count() < 4 {
        return None;
    }
    let has = |w: &str| u.contains(w);
    let word = |w: &str| u.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == w);
    Some(if has("GFCI") || word("GFI") || has("GROUND FAULT") {
        "GFCI receptacle"
    } else if has("QUAD") || has("FOURPLEX") || has("DOUBLE DUPLEX") {
        "quad receptacle"
    } else if has("RECEPTACLE") || has("CONVENIENCE OUTLET") || has("DUPLEX") {
        if has("SPECIAL") || has("RANGE") || has("DRYER") || has("30A") || has("50A") || has("WELDER") {
            "special receptacle"
        } else {
            "duplex receptacle"
        }
    } else if has("SMOKE") && has("DETECT") {
        "smoke detector"
    } else if has("HEAT DETECT") {
        "heat detector"
    } else if has("HORN") || has("STROBE") {
        "fire alarm horn/strobe"
    } else if has("PULL STATION") || has("MANUAL STATION") {
        "fire alarm pull station"
    } else if has("FIRE ALARM") {
        "fire alarm device"
    } else if has("OCCUPANCY") || has("VACANCY") || has("MOTION SENSOR") {
        "occupancy sensor"
    } else if has("DISCONNECT") {
        "disconnect switch"
    } else if has("DIMMER") {
        "dimmer switch"
    } else if has("SWITCH") && !has("SWITCHBOARD") && !has("SWITCHGEAR") {
        if has("3-WAY") || has("THREE WAY") || has("3 WAY") {
            "3-way switch"
        } else if has("4-WAY") || has("FOUR WAY") || has("4 WAY") {
            "4-way switch"
        } else {
            "switch"
        }
    } else if has("EXIT") && has("SIGN") {
        "exit sign"
    } else if has("EMERGENCY") && (has("LIGHT") || has("FIXTURE") || has("UNIT")) {
        "emergency light"
    } else if has("LUMINAIRE") || has("LIGHT FIXTURE") || has("DOWNLIGHT") || has("TROFFER") || has("SCONCE") || (has("LIGHT") && !has("LIGHTNING")) {
        "light fixture"
    } else if has("WIRELESS ACCESS") {
        "wireless access point"
    } else if has("TELEVISION") || word("CATV") || word("TV") {
        "TV outlet"
    } else if has("DATA") || has("TELEPHONE") || has("VOICE") || has("COMMUNICATION") || word("TEL") {
        "data/voice outlet"
    } else if has("CARD READER") {
        "card reader"
    } else if has("THERMOSTAT") {
        "thermostat"
    } else if has("JUNCTION BOX") || has("J-BOX") {
        "junction box"
    } else if has("EXHAUST FAN") {
        "exhaust fan"
    } else if has("SPEAKER") {
        "speaker"
    } else if has("CAMERA") {
        "camera"
    } else if has("DOOR HOLDER") {
        "door holder"
    } else {
        return None;
    })
}

/// The job's device class of a device type (`duplex_receptacle`, `gfci`, `switch`, `light`,
/// `data`, `fire_alarm`, `ef`; else the type itself, spaces as `_`).
pub fn device_class(device_type: &str) -> String {
    let t = device_type.to_ascii_lowercase();
    let class = if t.contains("gfci") {
        "gfci"
    } else if t.contains("receptacle") {
        "duplex_receptacle"
    } else if t.contains("smoke") || t.contains("heat detector") || t.contains("fire alarm") {
        "fire_alarm"
    } else if t.contains("switch") && !t.contains("disconnect") {
        "switch"
    } else if t.contains("light") || t.contains("exit sign") {
        "light"
    } else if t.contains("data") || t.contains("tv outlet") || t.contains("wireless") {
        "data"
    } else if t.contains("exhaust fan") {
        "ef"
    } else {
        return t.replace([' ', '/', '-'], "_");
    };
    class.to_string()
}

/// A mark's device type for takeoff and the element (`duplex receptacle`): a light fixture keeps
/// its mark, the fixture schedule's type (`light fixture A`).
pub(crate) fn typed(mark: &str, device_type: &str) -> String {
    if device_class(device_type) == "light" && device_type != "exit sign" {
        format!("{device_type} {}", mark.trim())
    } else {
        device_type.to_string()
    }
}

/// The kind a legend's device type is for the mounting-height rules ([`default_height`]).
fn kind_of_type(device_type: &str) -> Option<&'static str> {
    Some(match device_class(device_type).as_str() {
        "switch" => "switch",
        "fire_alarm" => "fire alarm device",
        "data" => "low voltage outlet",
        "occupancy_sensor" => "occupancy sensor",
        "door_holder" => "door holder",
        "junction_box" => "junction box",
        "duplex_receptacle" | "gfci" => "device",
        _ => return None,
    })
}

/// Each type mark's device type as the sheets' legends describe it: a text reading as the mark
/// with, on its line to its right, a description that names a device (`A` … `DUPLEX
/// RECEPTACLE`); the description most rows give wins. Mark (upper case) → (type, description).
pub(crate) fn legend_types(d: &Drawing, marks: &[String]) -> BTreeMap<String, (String, String)> {
    let wanted: std::collections::BTreeSet<String> = marks.iter().map(|m| m.trim().to_ascii_uppercase()).filter(|m| !m.is_empty()).collect();
    if wanted.is_empty() {
        return BTreeMap::new();
    }
    let ents = super::bspec::plan_text_objects(d);
    let texts: Vec<(String, Vec2, f64)> = super::perceive::texts_and_tags(&ents)
        .into_iter()
        .map(|(_, t, p, h)| (t, p, h))
        .filter(|(t, p, h)| !t.trim().is_empty() && p.is_finite() && h.is_finite() && *h > 0.0)
        .take(2_000_000)
        .collect();
    let cell = texts.iter().map(|(_, _, h)| *h).fold(0.0, f64::max).max(1.0) * 10.0;
    let grid = TextGrid::new(texts.clone(), cell);
    let mut votes: BTreeMap<String, BTreeMap<&'static str, (usize, String)>> = BTreeMap::new();
    for (t, p, h) in &texts {
        let mark = t.trim().to_ascii_uppercase();
        if !wanted.contains(&mark) {
            continue;
        }
        let row = grid
            .near(*p, 40.0 * h)
            .into_iter()
            .filter(|(w, q, _)| q.x > p.x && (q.y - p.y).abs() <= 0.75 * h && !w.trim().eq_ignore_ascii_case(&mark))
            .find_map(|(w, _, _)| device_type_of(w).map(|ty| (ty, w.trim().chars().take(120).collect::<String>())));
        if let Some((ty, desc)) = row {
            let e = votes.entry(mark).or_default().entry(ty).or_insert((0, desc));
            e.0 += 1;
        }
    }
    votes
        .into_iter()
        .filter_map(|(m, v)| v.into_iter().max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.0.cmp(a.0))).map(|(ty, (_, desc))| (m, (ty.to_string(), desc))))
        .collect()
}

/// The usual mounting height of a kind of device (a default: always marked), inches above the
/// floor.
fn default_height(kind: &str) -> f64 {
    match kind {
        "switch" | "equipment connection" => 48.0,
        "fire alarm device" | "door holder" => 80.0,
        "occupancy sensor" => 108.0,
        _ => 18.0,
    }
}

/// A mounting height written in `text` (`18" AFF`, `+48"`, `MOUNT AT 48" A.F.F.`, `4'-0" AFF`),
/// inches; only with a word that says it is a mounting height.
pub fn mount_height_in(text: &str) -> Option<f64> {
    let up = text.to_ascii_uppercase();
    let said = up.contains("AFF")
        || up.contains("A.F.F")
        || up.contains("MOUNT")
        || up.contains("ABOVE FIN")
        || up.contains("ABOVE FLOOR")
        || up.contains('+');
    if !said {
        return None;
    }
    let b = up.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b.get(i).is_some_and(u8::is_ascii_digit) && (i == 0 || !b.get(i - 1).is_some_and(|c| c.is_ascii_alphanumeric())) {
            let start = i;
            while b.get(i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                i += 1;
            }
            let n: f64 = up.get(start..i).and_then(|t| t.parse().ok()).unwrap_or(f64::NAN);
            let rest = up.get(i..).unwrap_or("");
            let inches = if let Some(r) = rest.strip_prefix('\'') {
                // Feet, maybe with inches after: 4'-0", 4' 6".
                let r = r.trim_start_matches(['-', ' ']);
                let m: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
                let more = if m.is_empty() || !r.get(m.len()..).is_some_and(|t| t.starts_with('"')) { 0.0 } else { m.parse().unwrap_or(0.0) };
                Some(n * 12.0 + more)
            } else if rest.starts_with('"') || rest.starts_with(" IN") || rest.starts_with("IN") {
                Some(n)
            } else {
                None
            };
            if let Some(v) = inches.filter(|v| v.is_finite() && (6.0..=240.0).contains(v)) {
                return Some(v);
            }
        }
        i += 1;
    }
    None
}

/// The words a general note uses for each kind of device.
fn note_kinds(up: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    if up.contains("RECEPT") || up.contains("OUTLET") && !up.contains("DATA") {
        out.push("device");
    }
    if up.contains("SWITCH") {
        out.push("switch");
    }
    if up.contains("FIRE ALARM") || up.contains("HORN") || up.contains("STROBE") || up.contains("PULL STATION") {
        out.push("fire alarm device");
    }
    if up.contains("DATA") || up.contains("TELE") || up.contains("LOW VOLTAGE") {
        out.push("low voltage outlet");
    }
    if up.contains("THERMOSTAT") || up.contains("DISCONNECT") {
        out.push("equipment connection");
    }
    out
}

/// Each device type's mounting height: a legend row's (the type mark and a height on one line
/// of the sheet), else a general note's for its kind of device, else the kind's usual height
/// (marked default).
pub fn mount_heights(d: &Drawing, marks: &[String]) -> BTreeMap<String, Height> {
    let ents = super::bspec::plan_text_objects(d);
    let texts = super::perceive::texts_and_tags(&ents);
    let wanted: Vec<String> = marks.iter().map(|m| m.trim().to_ascii_uppercase()).filter(|m| !m.is_empty()).collect();
    let mut legend: BTreeMap<String, Height> = BTreeMap::new();
    let mut by_note: BTreeMap<&'static str, Height> = BTreeMap::new();
    for (_, text, at, h) in texts.iter().take(2_000_000) {
        let Some(inches) = mount_height_in(text) else { continue };
        let up = text.to_ascii_uppercase();
        let h = if h.is_finite() && *h > 0.0 { *h } else { 1.0 };
        // A legend row: a type mark to the left on the same line.
        let row = texts
            .iter()
            .filter(|(_, _, p, _)| (p.y - at.y).abs() <= 0.75 * h && p.x < at.x && at.x - p.x <= 60.0 * h)
            .filter(|(_, t, _, _)| wanted.iter().any(|m| m.eq_ignore_ascii_case(t.trim())))
            .min_by(|a, b| (at.x - a.2.x).total_cmp(&(at.x - b.2.x)));
        if let Some((_, t, _, _)) = row {
            legend.entry(t.trim().to_ascii_uppercase()).or_insert(Height { inches, source: SourceKind::Legend, doc: "device legend".into() });
            continue;
        }
        for k in note_kinds(&up) {
            by_note.entry(k).or_insert_with(|| Height { inches, source: SourceKind::Note, doc: text.chars().take(120).collect() });
        }
    }
    wanted
        .iter()
        .map(|m| {
            let kind = kind_of(m);
            let h = legend.get(m).cloned().or_else(|| by_note.get(kind).cloned()).unwrap_or_else(|| Height {
                inches: default_height(kind),
                source: SourceKind::Default,
                doc: format!("usual height for a {kind}"),
            });
            (m.clone(), h)
        })
        .collect()
}

/// The panel a circuit tag names (`LP1-3` → `LP1`, `2LA-12` → `2LA`); none for a bare number.
pub fn panel_of(circuit: &str) -> Option<String> {
    let c = circuit.trim();
    let (panel, number) = c.rsplit_once(['-', '/', '.'])?;
    let panel = panel.trim();
    (panel.chars().any(|ch| ch.is_ascii_alphabetic()) && number.chars().any(|ch| ch.is_ascii_digit()) && panel.len() <= 12).then(|| panel.to_string())
}

/// Whether `e` is a device the readers put in the spec (and so one `model.build` builds).
fn read_device(t: &cadaddy_walk3d::tags::ElementTag) -> bool {
    t.id.starts_with("device:") && t.fields.contains_key("symbol") && t.at_ft.iter().all(|v| v.is_finite())
}

/// The WALLGEN tag key linking a built device to its spec tag.
const DEVID: &str = "devid";

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Build the spec's read devices as elements (a box at the symbol at its mounting height, on
/// 3D-Devices, part `device`) with what was read about each; how many. Levels drawn on another
/// sheet are skipped (their plans are not in this drawing).
pub(crate) fn build_devices(d: &mut Drawing, sp: &BuildingSpec) -> usize {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let devices: Vec<&cadaddy_walk3d::tags::ElementTag> = sp.tags.iter().filter(|t| read_device(t)).take(MAX_DEVICES).collect();
    if devices.is_empty() {
        return 0;
    }
    let field = |t: &cadaddy_walk3d::tags::ElementTag, k: &str| t.fields.get(k).map(|f| f.value.trim().to_string()).filter(|v| !v.is_empty());
    let marks: Vec<String> = devices.iter().filter_map(|t| field(t, "type")).collect();
    let heights = mount_heights(d, &marks);
    let layer = super::wallgen::layer_of(cadaddy_walk3d::Part::Device);
    if d.layer(layer).is_none() {
        d.layers.push(cadaddy_doc::Layer { name: layer.into(), color: cadaddy_doc::color::Color::Index(30), ..cadaddy_doc::Layer::default() });
    }
    let mut built = 0;
    for t in devices {
        let Some(level) = sp.level(&t.level).filter(|l| l.sheet.is_none()) else { continue };
        let mark = field(t, "type").unwrap_or_default();
        // The type the legend gives the mark (the readers put it in `device_type`, source legend).
        let legend_type = t.fields.get("device_type").filter(|f| f.source.eq_ignore_ascii_case("legend")).map(|f| f.value.trim().to_string());
        let kind = legend_type.as_deref().and_then(kind_of_type).unwrap_or_else(|| kind_of(&mark));
        let usual = Height { inches: default_height(kind), source: SourceKind::Default, doc: format!("usual height for a {kind}") };
        // A default for the mark's letter gives way to the default for the legend's type.
        let height =
            heights.get(&mark.to_ascii_uppercase()).cloned().filter(|h| h.source != SourceKind::Default || legend_type.is_none()).unwrap_or(usual);
        let read_at = Vec2::new(t.at_ft[0] * upf, t.at_ft[1] * upf);
        let sheet_name = field(t, "sheet").unwrap_or_default();
        let (placed, registered) = register(sp, &sheet_name, level, read_at);
        let (x, y) = (placed.x, placed.y);
        let z = (level.elevation_ft + height.inches / 12.0) * upf;
        let (w, l, h) = (BOX_IN[0] * upf / 12.0, BOX_IN[1] * upf / 12.0, BOX_IN[2] * upf / 12.0);
        let mut m = super::wallgen::empty_mesh();
        super::wallgen::add_box(&mut m, Vec2::new(x - l / 2.0, y), Vec2::new(x + l / 2.0, y), w, z - h / 2.0, z + h / 2.0);
        if m.faces.is_empty() {
            continue;
        }
        let symbol = field(t, "symbol").and_then(|h| Handle::parse_hex(&h)).unwrap_or(Handle(0));
        let mut xdata = super::wallgen::tag(cadaddy_walk3d::Part::Device, &level.name, symbol);
        xdata.push(RawTag { code: 1000, value: format!("{DEVID}:{}", t.id.chars().take(240).collect::<String>()) });
        // What was read about it.
        let sheet = sheet_name.clone();
        let at = Some([read_at.x, read_at.y]);
        let printed = || Source::print(&sheet, at);
        let mut store = Props::default();
        if let Some(desc) = t.fields.get("description").filter(|f| f.source.eq_ignore_ascii_case("legend")) {
            store.set("description", Prop::new(desc.value.trim().to_string(), Source::from_doc(SourceKind::Legend, &sheet)));
        }
        if !mark.is_empty() {
            let ty = match &legend_type {
                Some(ty) => {
                    Prop::new(ty.clone(), Source::from_doc(SourceKind::Legend, &sheet)).with_note("the device legend's description of its mark")
                }
                None => Prop::new(field(t, "device_type").unwrap_or_else(|| kind.to_string()), printed()),
            };
            store.set("type", ty);
            store.set("type_mark", Prop::new(mark.clone(), printed()));
        }
        if let Some(c) = field(t, "circuit") {
            store.set("circuit", Prop::new(c.clone(), printed()));
            match panel_of(&c) {
                Some(p) => {
                    store.set("panel", Prop::new(p.clone(), printed()));
                    store.set("fed_from", Prop::new(format!("panel {p}, circuit {c}"), printed()));
                }
                None => {
                    store.set("fed_from", Prop::new(format!("circuit {c}"), printed()).with_note("the panel is not printed with the circuit"));
                }
            }
        }
        let mut hp = Prop::new(round2(height.inches), Source::from_doc(height.source, &height.doc)).with_unit("in");
        if height.source == SourceKind::Default {
            hp.note = Some("not printed: the usual height for this kind of device".into());
        }
        store.set("mount_height", hp.clone());
        store.set("offset", Prop::new(round2(height.inches / 12.0), hp.source.clone()).with_unit("ft"));
        if !sheet.is_empty() {
            store.set("sheet", Prop::new(sheet.clone(), Source::print(&sheet, at)));
        }
        let mut loc = location_prop(x, y, z, upf);
        if registered {
            loc.note = Some("model coordinates: read on its sheet, moved onto its level's architectural plan by the sheet frame (same sheet size), at the level's elevation plus the offset".into());
        }
        store.set("location", loc);
        let xdata = props::write(&xdata, &store);
        if d.add(&Space::Model, Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            built += 1;
        }
    }
    built
}

/// Where a point read on sheet `sheet` lies on `level`'s architectural floor plan: moved by the
/// sheet frames when both sheets are the same size (the same title block and plan placement, as
/// the trades' runs are moved), else where it was read. Whether it was moved.
fn register(sp: &BuildingSpec, sheet: &str, level: &cadaddy_walk3d::Level, p: Vec2) -> (Vec2, bool) {
    let Some(region) = level.region else { return (p, false) };
    let inside = |r: &[f64; 4], q: Vec2| q.x >= r[0].min(r[2]) && q.x <= r[0].max(r[2]) && q.y >= r[1].min(r[3]) && q.y <= r[1].max(r[3]);
    let size = |r: &[f64; 4]| ((r[2] - r[0]).abs(), (r[3] - r[1]).abs());
    let plan_mid = Vec2::new((region[0] + region[2]) / 2.0, (region[1] + region[3]) / 2.0);
    let arch = sp
        .sheets
        .iter()
        .filter_map(|x| x.rect.filter(|_| x.discipline.as_deref().is_none_or(|d| d.eq_ignore_ascii_case("A"))))
        .find(|r| inside(r, plan_mid));
    let own = sp.sheets.iter().filter(|x| x.number.eq_ignore_ascii_case(sheet.trim())).filter_map(|x| x.rect).find(|r| inside(r, p));
    let (Some(a), Some(o)) = (arch, own) else { return (p, false) };
    if inside(&a, p) {
        return (p, false);
    }
    let ((aw, ah), (ow, oh)) = (size(&a), size(&o));
    let same = ow > 0.0 && oh > 0.0 && ((aw - ow) / ow).abs() < 0.02 && ((ah - oh) / oh).abs() < 0.02;
    if !same {
        return (p, false);
    }
    let q = Vec2::new(a[0].min(a[2]) + (p.x - o[0].min(o[2])), a[1].min(a[3]) + (p.y - o[1].min(o[3])));
    (if q.is_finite() { q } else { p }, q.is_finite())
}

/// An element's 3D location in the model (feet), as `x, y, z`: computed from where it was read.
fn location_prop(x: f64, y: f64, z: f64, upf: f64) -> Prop {
    Prop::new(format!("{:.2}, {:.2}, {:.2}", x / upf, y / upf, z / upf), Source::computed())
        .with_unit("ft")
        .with_note("model coordinates: the sheet's plan position, the level's elevation plus the offset")
}

/// What [`rebuild_devices`] did: device elements taken out and built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceRebuild {
    pub removed: usize,
    pub built: usize,
}

/// Build the spec's read devices in the open model again (a reader changed them): the device
/// elements built before go, every other element stays as it is (the plumbing fixtures the fill
/// placed too). A device on a level that `model.build` stacked moves as that level's built
/// objects did, and the people's overrides go back on.
pub(crate) fn rebuild_devices(s: &mut Session, cmd: &str) -> Result<DeviceRebuild> {
    let (sp, _) = super::bspec::current(s, cmd)?;
    let d = s.doc_mut()?;
    let read = |e: &Entity| super::wallgen::made_as(e).as_deref() == Some("device") && made(e, FIXID).is_none();
    // Each level's stacking move, as its built objects (not devices) carry it.
    let mut moved: HashMap<String, Vec2> = HashMap::new();
    for e in d.model.iter().filter(|e| super::wallgen::made_as(e).is_some_and(|p| p != "device")).take(MAX_ELEMENTS) {
        let Some(level) = super::bspec::made_level(e) else { continue };
        let off = super::bspec::stacked_by(e);
        if off.len() > 1e-9 {
            moved.entry(level.to_ascii_lowercase()).or_insert(off);
        }
    }
    let old: Vec<Handle> = d.model.iter().filter(|e| read(e)).map(|e| e.handle).collect();
    for h in &old {
        d.model.remove(*h);
    }
    let built = build_devices(d, &sp);
    if !moved.is_empty() {
        let new: Vec<(Handle, Vec2)> = d
            .model
            .iter()
            .filter(|e| read(e) && made(e, DEVID).is_some() && super::bspec::stacked_by(e).len() <= 1e-9)
            .filter_map(|e| super::bspec::made_level(e).and_then(|l| moved.get(&l.to_ascii_lowercase()).map(|off| (e.handle, *off))))
            .collect();
        for (h, off) in new {
            let m = cadaddy_geom::Mat3::translate(off);
            let _ = d.modify_entity(h, |e| {
                e.kind.transform(&m);
                e.common.xdata = super::bspec::with_stacked(&e.common.xdata, off).into();
            });
        }
    }
    // An automatic rebuild (the plan changed) builds from the spec it keeps: the same devices.
    if let Some(kept) = s.state_mut()?.wallgen.as_mut().and_then(|g| g.spec.as_mut()) {
        kept.tags.retain(|t| !read_device(t));
        kept.tags.extend(sp.tags.iter().filter(|t| read_device(t)).cloned());
    }
    super::overrides::after_build(s);
    Ok(DeviceRebuild { removed: old.len(), built })
}

trait WithNote {
    fn with_note(self, note: &str) -> Self;
}

impl WithNote for Prop {
    fn with_note(mut self, note: &str) -> Self {
        self.note = Some(note.chars().take(props::MAX_TEXT).collect());
        self
    }
}

// ─── The fill ───────────────────────────────────────────────────────────────

/// A room as the fill sees it: its outline (model coordinates), level, number and name.
struct RoomShape {
    outline: Vec<Vec2>,
    bounds: Bounds2,
    level: Option<String>,
    label: String,
}

impl RoomShape {
    fn holds(&self, p: Vec2) -> bool {
        self.bounds.contains(p) && cadaddy_geom::point_in_polygon(&self.outline, p)
    }
}

fn room_label(number: Option<&str>, name: &str) -> String {
    match number.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => name.trim().to_string(),
    }
}

fn same_level(r: &RoomShape, level: Option<&str>) -> bool {
    r.level.is_none() || level.is_none() || r.level.as_deref().zip(level).is_some_and(|(a, b)| level_key(a) == level_key(b))
}

/// A level's name as a key, so the readers' names for one level match: `Level 1`, `LEVEL 1`,
/// `1ST FLOOR`, `FIRST FLOOR`, `L1` and `1` are all `#1`; `ROOF` is `R`; anything else as written.
pub fn level_key(name: &str) -> String {
    let u = name.trim().to_ascii_uppercase();
    let digits: String = u.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    if let Ok(n) = digits.parse::<u32>() {
        return format!("#{n}");
    }
    const ORDINALS: [(&str, u32); 10] = [
        ("FIRST", 1),
        ("SECOND", 2),
        ("THIRD", 3),
        ("FOURTH", 4),
        ("FIFTH", 5),
        ("SIXTH", 6),
        ("SEVENTH", 7),
        ("EIGHTH", 8),
        ("NINTH", 9),
        ("TENTH", 10),
    ];
    if let Some((_, n)) = ORDINALS.iter().find(|(w, _)| u.split(|c: char| !c.is_ascii_alphabetic()).any(|x| x == *w)) {
        return format!("#{n}");
    }
    if u.contains("ROOF") {
        return "R".into();
    }
    u
}

/// The rooms devices and fixtures are put in: the model's room elements (their tag's number,
/// else the number the fill read inside them, else their name), else `rooms`.
fn room_shapes(d: &Drawing, rooms: &[super::rooms3d::Room3]) -> Vec<RoomShape> {
    let els = super::bspec::room_elements(d);
    if els.is_empty() {
        return rooms
            .iter()
            .filter(|r| r.outline.len() >= 3)
            .map(|r| RoomShape {
                bounds: Bounds2::from_points(r.outline.iter().copied()),
                outline: r.outline.clone(),
                level: r.level.clone(),
                label: room_label(r.number.as_deref(), &r.name),
            })
            .collect();
    }
    els.into_iter()
        .filter(|r| r.outline.len() >= 3)
        .map(|r| {
            let stored = d.model.get(r.handle).and_then(|e| props::read(&e.common.xdata).get("number").filter(|p| p.has_value()).map(Prop::text));
            let number = r.number.clone().filter(|n| !n.trim().is_empty()).or(stored);
            RoomShape {
                bounds: Bounds2::from_points(r.outline.iter().copied()),
                level: Some(r.level.clone()).filter(|l| !l.trim().is_empty()),
                label: room_label(number.as_deref(), &r.name),
                outline: r.outline,
            }
        })
        .collect()
}

/// A room the guided job's rooms stage read on a plan sheet (`rooms` parameter), in plan
/// coordinates: the drawing as imported, before the build stacked the levels.
pub(crate) struct ReadRoom {
    number: Option<String>,
    name: String,
    level: Option<String>,
    sheet: String,
    at: Vec2,
    outline: Vec<Vec2>,
}

/// Rooms read, and outline points per room, taken at most (hostile parameters).
const MAX_READ_ROOMS: usize = 20_000;
const MAX_READ_OUTLINE: usize = 512;

fn point_of(v: &Value) -> Option<Vec2> {
    let p = Vec2::new(v.get(0)?.as_f64()?, v.get(1)?.as_f64()?);
    p.is_finite().then_some(p)
}

/// The `rooms` parameter: `[{number?, name?, level?, sheet?, at: [x, y], outline?: [[x, y]…]}]`.
pub(crate) fn rooms_param(p: &Value) -> Vec<ReadRoom> {
    let cap = |v: &str| v.trim().chars().take(props::MAX_TEXT).collect::<String>();
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(cap).filter(|t| !t.is_empty());
    p.get("rooms")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_READ_ROOMS)
        .filter_map(|v| {
            let at = v.get("at").and_then(point_of)?;
            let outline: Vec<Vec2> =
                v.get("outline").and_then(Value::as_array).into_iter().flatten().take(MAX_READ_OUTLINE).filter_map(point_of).collect();
            Some(ReadRoom {
                number: text(v, "number").filter(|n| room_number_in(n).is_some()),
                name: text(v, "name").unwrap_or_default(),
                level: text(v, "level"),
                sheet: text(v, "sheet").unwrap_or_default(),
                at,
                outline,
            })
        })
        .collect()
}

/// The room number a text gives: the text itself (`101`, `101A`), else the one number word of a
/// one-line tag with its name (`OFFICE 101`, `101 OFFICE`).
pub fn room_number_in(t: &str) -> Option<String> {
    let t = t.trim();
    if super::perceive::geometry::room_number(t) {
        return Some(t.to_string());
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    if words.len() < 2 || words.len() > 4 || !words.iter().any(|w| w.chars().filter(char::is_ascii_alphabetic).count() >= 2) {
        return None;
    }
    let mut numbers = words.iter().filter(|w| super::perceive::geometry::room_number(w) && w.chars().any(|c| c.is_ascii_digit()));
    let n = numbers.next()?;
    // `ROOM 101 TO 103`: two numbers name no one room.
    numbers.next().is_none().then(|| n.to_string())
}

/// Whether two levels are one (by [`level_key`]); a missing level matches any.
fn level_matches(a: Option<&str>, b: Option<&str>) -> bool {
    a.zip(b).filter(|(a, b)| !a.trim().is_empty() && !b.trim().is_empty()).is_none_or(|(a, b)| level_key(a) == level_key(b))
}

/// The number the rooms reading gives the room whose plan outline is `plan` on `level`: the
/// numbered read room whose label stands inside it (nearest its middle), else the one whose
/// outline holds its middle and is about its size, else the one numbered room of its name on its
/// level. Numbers in `used` (level key, number) are taken already.
fn read_number<'a>(
    read: &'a [ReadRoom],
    plan: &[Vec2],
    level: &str,
    name: &str,
    used: &std::collections::HashSet<(String, String)>,
) -> Option<&'a ReadRoom> {
    let b = Bounds2::from_points(plan.iter().copied());
    let mid = b.center();
    let area = cadaddy_geom::shoelace(plan).abs();
    let free = |r: &&ReadRoom| {
        r.number.as_deref().is_some_and(|n| !used.contains(&(level_key(level), n.to_ascii_uppercase())))
            && level_matches(r.level.as_deref(), Some(level))
    };
    let inside = read
        .iter()
        .filter(free)
        .filter(|r| b.contains(r.at) && cadaddy_geom::point_in_polygon(plan, r.at))
        .min_by(|a, b| a.at.dist(mid).total_cmp(&b.at.dist(mid)));
    if inside.is_some() {
        return inside;
    }
    let holds = read.iter().filter(free).find(|r| {
        let a = cadaddy_geom::shoelace(&r.outline).abs();
        r.outline.len() >= 3 && a > 0.0 && area > 0.0 && (0.25..=4.0).contains(&(a / area)) && cadaddy_geom::point_in_polygon(&r.outline, mid)
    });
    if holds.is_some() {
        return holds;
    }
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let mut named = read.iter().filter(free).filter(|r| r.name.trim().eq_ignore_ascii_case(name));
    let one = named.next()?;
    named.next().is_none().then_some(one)
}

/// Give each room element whose tag has no number the room number printed inside it (on its
/// plan: the build moved stacked levels), nearest its middle (`print`), else the number the
/// job's rooms reading paired with its label (`read`); how many got one from each.
fn number_rooms(d: &mut Drawing, sp: &BuildingSpec, texts: &[(String, Vec2, f64)], read: &[ReadRoom]) -> (usize, usize) {
    let all = super::bspec::room_elements(d);
    let number_of = |r: &super::bspec::RoomElement| {
        r.number
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| d.model.get(r.handle).and_then(|e| props::read(&e.common.xdata).get("number").filter(|p| p.has_value()).map(Prop::text)))
    };
    let mut used: std::collections::HashSet<(String, String)> =
        all.iter().filter_map(|r| number_of(r).map(|n| (level_key(&r.level), n.trim().to_ascii_uppercase()))).collect();
    let els: Vec<&super::bspec::RoomElement> = all.iter().filter(|r| number_of(r).is_none() && r.outline.len() >= 3).collect();
    if els.is_empty() {
        return (0, 0);
    }
    let cell = texts.iter().map(|(_, _, h)| *h).filter(|h| h.is_finite()).fold(0.0, f64::max).max(1.0) * 20.0;
    let grid = TextGrid::new(texts.to_vec(), cell);
    let mut changes: Vec<(Handle, Props, bool)> = Vec::new();
    for r in els.into_iter().take(MAX_ELEMENTS) {
        let Some(e) = d.model.get(r.handle) else { continue };
        let mut store = props::read(&e.common.xdata);
        let shift = super::bspec::stacked_by(e);
        let plan: Vec<Vec2> = r.outline.iter().map(|p| *p - shift).collect();
        let b = Bounds2::from_points(plan.iter().copied());
        let mid = b.center();
        let reach = (b.width().hypot(b.height()) / 2.0).min(200.0 * sp.units_per_foot.max(1.0));
        let inside = |q: Vec2| b.contains(q) && cadaddy_geom::point_in_polygon(&plan, q);
        let near = grid.near(mid, reach);
        let printed = |whole: bool| {
            near.iter().find_map(|(t, q, _)| {
                let n = if whole { super::perceive::geometry::room_number(t).then(|| t.trim().to_string()) } else { room_number_in(t) }?;
                (!super::guided::is_circuit_tag(t) && inside(*q) && !used.contains(&(level_key(&r.level), n.to_ascii_uppercase()))).then_some((n, *q))
            })
        };
        let (number, src, note, from_read) = if let Some((n, q)) = printed(true).or_else(|| printed(false)) {
            (n, Source::print(&sheet_at(sp, q, Some(&r.level)), Some([q.x, q.y])), "the room number printed inside the room", false)
        } else if let Some(rr) = read_number(read, &plan, &r.level, &r.name, &used) {
            let Some(n) = rr.number.clone() else { continue };
            let sheet = if rr.sheet.is_empty() { sheet_at(sp, rr.at, Some(&r.level)) } else { rr.sheet.clone() };
            (n, Source::print(&sheet, Some([rr.at.x, rr.at.y])), "the room number the rooms reading paired with this room's label", true)
        } else {
            continue;
        };
        used.insert((level_key(&r.level), number.to_ascii_uppercase()));
        store.set("number", Prop::new(number, src).with_note(note));
        changes.push((r.handle, store, from_read));
    }
    let (mut printed, mut from_read) = (0, 0);
    for (h, store, read) in changes {
        if d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).is_ok() {
            if read {
                from_read += 1;
            } else {
                printed += 1;
            }
        }
    }
    (printed, from_read)
}

/// Smallest and largest read room made a room of the model (square feet), as the build's.
const MIN_READ_ROOM_SF: f64 = 16.0;
const MAX_READ_ROOM_SF: f64 = 50_000.0;

/// The numbered rooms the job read that the model has no room for (no room element or volume of
/// that number on the level, none holding its label): each a room volume where the build put
/// its level (the stacking move of the level's built objects), carrying its number and level as
/// read. The model's room elements come only from room tags inside the built walls; a set whose
/// walls leave its rooms open would otherwise have rooms in the reading and none in the model.
fn add_read_rooms(d: &mut Drawing, sp: &BuildingSpec, read: &[ReadRoom], upf: f64) -> (usize, Vec<RoomShape>) {
    if read.is_empty() {
        return (0, Vec::new());
    }
    // Each level's stacking move, from what the build made on it.
    let mut moved: HashMap<String, Vec2> = HashMap::new();
    let mut have: Vec<(Option<String>, Vec<Vec2>, Option<String>)> = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS * 4) {
        if let Some(l) = super::bspec::made_level(e) {
            moved.entry(level_key(&l)).or_insert_with(|| super::bspec::stacked_by(e));
        }
        if super::rooms3d::room_of(&e.common).is_some()
            && let EntityKind::Mesh(m) = &e.kind
        {
            let z0 = m.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
            let outline: Vec<Vec2> = m.vertices.iter().filter(|v| (v.z - z0).abs() < 1e-9).map(|v| v.xy()).collect();
            let store = props::read(&e.common.xdata);
            let level = store.get("level").filter(|p| p.has_value()).map(Prop::text);
            let number = volume_number(e).or_else(|| store.get("number").filter(|p| p.has_value()).map(Prop::text));
            have.push((level, outline, number));
        }
    }
    for r in super::bspec::room_elements(d) {
        let stored = d.model.get(r.handle).and_then(|e| props::read(&e.common.xdata).get("number").filter(|p| p.has_value()).map(Prop::text));
        have.push((Some(r.level.clone()), r.outline, r.number.filter(|n| !n.trim().is_empty()).or(stored)));
    }
    let mut out: Vec<RoomShape> = Vec::new();
    let mut added = 0;
    let (lo, hi) = (MIN_READ_ROOM_SF * upf * upf, MAX_READ_ROOM_SF * upf * upf);
    for r in read {
        let Some(number) = r.number.as_deref() else { continue };
        if r.outline.len() < 3 || added >= MAX_READ_ROOMS {
            continue;
        }
        let area = cadaddy_geom::shoelace(&r.outline).abs();
        if !(lo..=hi).contains(&area) {
            continue;
        }
        let shift = r.level.as_deref().and_then(|l| moved.get(&level_key(l))).copied().unwrap_or(Vec2::ZERO);
        let outline: Vec<Vec2> = r.outline.iter().map(|p| *p + shift).collect();
        let at = r.at + shift;
        let lvl = r.level.as_deref();
        let taken = have.iter().any(|(l, o, n)| {
            level_matches(l.as_deref(), lvl)
                && (n.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(number.trim()))
                    || (o.len() >= 3 && (cadaddy_geom::point_in_polygon(o, at) || cadaddy_geom::point_in_polygon(&outline, centroid(o)))))
        });
        if taken {
            continue;
        }
        let level = lvl.and_then(|l| sp.levels.iter().find(|x| level_key(&x.name) == level_key(l)));
        // The model's name for the level (the sheet may call it `1` or `FIRST FLOOR`).
        let level_name = level.map(|l| l.name.clone()).or_else(|| r.level.clone());
        let ceiling = level.map(|l| l.ceiling_ft * upf).filter(|c| c.is_finite() && *c > 0.0);
        let room = super::rooms3d::Room3 {
            name: r.name.clone(),
            number: Some(number.to_string()),
            at,
            label: Handle(0),
            area,
            perimeter: outline.iter().zip(outline.iter().cycle().skip(1)).map(|(a, b)| a.dist(*b)).sum(),
            elevation: level.map_or(0.0, |l| l.elevation_ft * upf),
            ceiling: ceiling.unwrap_or(super::rooms3d::DEFAULT_CEILING_IN * upf / 12.0),
            ceiling_source: if ceiling.is_some() { "level" } else { "default" }.into(),
            finishes: Vec::new(),
            finish_detail: Vec::new(),
            materials: Value::Null,
            level: level_name.clone(),
            outline: outline.clone(),
        };
        let src = Source::print(&r.sheet, Some([r.at.x, r.at.y]));
        let mut store = Props::default();
        store.set("number", Prop::new(number.to_string(), src.clone()).with_note("read on the plan; the build traced no room here"));
        if let Some(l) = &level_name {
            store.set("level", Prop::new(l.clone(), src));
        }
        if super::rooms3d::add_volume(d, &room, Some(&store)) {
            added += 1;
            have.push((level_name.clone(), outline.clone(), Some(number.to_string())));
            out.push(RoomShape {
                bounds: Bounds2::from_points(outline.iter().copied()),
                outline,
                level: level_name,
                label: room_label(Some(number), &r.name),
            });
        }
    }
    (added, out)
}

/// A room volume's number (its `rooms.build` tag).
fn volume_number(e: &Entity) -> Option<String> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value == super::rooms3d::ROOM_APP)?;
    x.get(at + 2).filter(|t| t.code == 1000 && !t.value.trim().is_empty()).map(|t| t.value.clone())
}

fn centroid(o: &[Vec2]) -> Vec2 {
    if o.is_empty() {
        return Vec2::ZERO;
    }
    o.iter().fold(Vec2::ZERO, |a, p| a + *p) * (1.0 / o.len() as f64)
}

/// The room holding `p` on `level` (any level when the room has none).
fn room_at<'a>(rooms: &'a [RoomShape], level: Option<&str>, p: Vec2) -> Option<&'a RoomShape> {
    rooms.iter().find(|r| same_level(r, level) && r.holds(p))
}

/// The room holding `p`, else the nearest one whose outline is within `max` of it (a device
/// drawn on the room's wall stands just outside the room's outline); whether it was inside.
fn room_near<'a>(rooms: &'a [RoomShape], level: Option<&str>, p: Vec2, max: f64) -> Option<(&'a RoomShape, bool)> {
    if let Some(r) = room_at(rooms, level, p) {
        return Some((r, true));
    }
    rooms
        .iter()
        .filter(|r| same_level(r, level) && dist_to(&r.bounds, p) <= max)
        .map(|r| (r, outline_dist(&r.outline, p)))
        .filter(|(_, d)| *d <= max)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(r, _)| (r, false))
}

/// The distance from `p` to a closed outline's edges.
fn outline_dist(outline: &[Vec2], p: Vec2) -> f64 {
    let n = outline.len();
    let mut best = f64::INFINITY;
    for i in 0..n {
        let (Some(a), Some(b)) = (outline.get(i), outline.get((i + 1) % n)) else { continue };
        let ab = *b - *a;
        let len2 = ab.x * ab.x + ab.y * ab.y;
        let t = if len2 > 0.0 { (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let q = *a + ab * t;
        best = best.min(q.dist(p));
    }
    best
}

/// Where a point of a level's architectural plan lies on the trades' sheets of that level (the
/// same title block and plan placement: the sheets are the same size), nearest first: the point
/// itself, then its place on each such sheet. The trades' readers moved what they read onto the
/// architectural plan this way (printmep), so what is printed beside it is found back there.
fn views(sp: &BuildingSpec, p: Vec2, level: Option<&str>) -> Vec<Vec2> {
    let mut out = vec![p];
    let inside = |r: &[f64; 4], q: Vec2| q.x >= r[0].min(r[2]) && q.x <= r[0].max(r[2]) && q.y >= r[1].min(r[3]) && q.y <= r[1].max(r[3]);
    let size = |r: &[f64; 4]| ((r[2] - r[0]).abs(), (r[3] - r[1]).abs());
    let Some(a) = sp.sheets.iter().filter_map(|x| x.rect).find(|r| inside(r, p)) else { return out };
    let (aw, ah) = size(&a);
    let on_level = |x: &cadaddy_walk3d::levels::SheetLink| {
        level.is_none_or(|l| x.level.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(l)) || x.levels.iter().any(|m| m.eq_ignore_ascii_case(l)))
    };
    for x in sp.sheets.iter().filter(|x| on_level(x)).take(MAX_SCHEDULE_SHEETS * 4) {
        let Some(o) = x.rect else { continue };
        let (ow, oh) = size(&o);
        if o == a || ow <= 0.0 || oh <= 0.0 || ((aw - ow) / ow).abs() >= 0.02 || ((ah - oh) / oh).abs() >= 0.02 {
            continue;
        }
        let q = Vec2::new(o[0].min(o[2]) + (p.x - a[0].min(a[2])), o[1].min(o[3]) + (p.y - a[1].min(a[3])));
        if q.is_finite() {
            out.push(q);
        }
    }
    out
}

/// Set `key` unless a person set it (their value stays) or the store already has a value read
/// from the prints that the fill would only replace with an assumption.
fn put(store: &mut Props, key: &str, prop: Prop, filled: &mut BTreeMap<String, usize>) {
    if !prop.has_value() {
        return;
    }
    if let Some(old) = store.get(key) {
        if old.source.kind == SourceKind::User {
            return;
        }
        if old.has_value() && !old.source.kind.is_assumed() && prop.source.kind.is_assumed() {
            return;
        }
    }
    if store.set(key, prop) {
        *filled.entry(key.to_string()).or_insert(0) += 1;
    }
}

/// The plan's texts by where they are (drawing units): a grid for nearby lookups.
struct TextGrid {
    cell: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
    texts: Vec<(String, Vec2, f64)>,
}

impl TextGrid {
    fn new(texts: Vec<(String, Vec2, f64)>, cell: f64) -> Self {
        let cell = if cell.is_finite() && cell > 0.0 { cell } else { 1.0 };
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, (_, p, _)) in texts.iter().enumerate() {
            if p.is_finite() {
                cells.entry(((p.x / cell).floor() as i64, (p.y / cell).floor() as i64)).or_default().push(i);
            }
        }
        TextGrid { cell, cells, texts }
    }

    /// The texts within `r` of `p`, nearest first.
    fn near(&self, p: Vec2, r: f64) -> Vec<(&str, Vec2, f64)> {
        if !(p.is_finite() && r.is_finite() && r > 0.0) {
            return Vec::new();
        }
        let n = (r / self.cell).ceil() as i64;
        let (cx, cy) = ((p.x / self.cell).floor() as i64, (p.y / self.cell).floor() as i64);
        let mut out: Vec<(&str, Vec2, f64)> = Vec::new();
        for gx in cx - n..=cx + n {
            for gy in cy - n..=cy + n {
                for i in self.cells.get(&(gx, gy)).into_iter().flatten() {
                    if let Some((t, q, h)) = self.texts.get(*i)
                        && q.dist(p) <= r
                    {
                        out.push((t.as_str(), *q, *h));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.1.dist(p).total_cmp(&b.1.dist(p)));
        out
    }
}

/// Whether `t` reads as a door number (`101`, `101A`, `D-12`, `1-104B`): short, with a digit,
/// letters and digits and at most two dashes; not a dimension or a size.
pub fn door_number(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty()
        && t.len() <= 7
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && t.matches('-').count() <= 2
        && !t.starts_with('-')
        && !t.ends_with('-')
        && t.chars().filter(|c| c.is_ascii_alphabetic()).count() <= 2
}

/// The airflow written in `t` (`150 CFM`, `CFM: 200`, `12x12 / 150CFM`).
pub fn cfm_in(t: &str) -> Option<f64> {
    let up = t.to_ascii_uppercase();
    let at = up.find("CFM")?;
    let before: String = up.get(..at).unwrap_or("").trim_end().chars().rev().take_while(|c| c.is_ascii_digit() || *c == ',').collect();
    let before: String = before.chars().rev().filter(|c| *c != ',').collect();
    let after: String = up.get(at + 3..).unwrap_or("").trim_start_matches([':', '=', ' ']).chars().take_while(char::is_ascii_digit).collect();
    let n: f64 = if before.is_empty() { after.parse().ok()? } else { before.parse().ok()? };
    (n.is_finite() && (10.0..=100_000.0).contains(&n)).then_some(n)
}

/// Whether `t` reads as an air terminal's type tag (`A`, `CD-1`, `SR2`, `RG-1`): one to three
/// letters, then at most two digits.
pub fn terminal_type_tag(t: &str) -> bool {
    let t = t.trim();
    let letters = t.chars().take_while(char::is_ascii_alphabetic).count();
    let rest = t.get(letters..).unwrap_or("").trim_start_matches('-');
    (1..=3).contains(&letters) && rest.len() <= 2 && rest.chars().all(|c| c.is_ascii_digit())
}

/// The airflow a terminal tag prints under its type (`150`, `1,200`): a whole number of cfm.
pub fn airflow_number(t: &str) -> Option<f64> {
    let t = t.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || c == ',') {
        return None;
    }
    let n: f64 = t.replace(',', "").parse().ok()?;
    (n.is_finite() && (25.0..=5_000.0).contains(&n)).then_some(n)
}

/// The airflow printed at a terminal at `p` (on any of its sheets' views): `150 CFM` beside it,
/// else the number under the type in its tag. The airflow, where it is printed and how it reads.
fn airflow_at(grid: &TextGrid, sp: &BuildingSpec, p: Vec2, level: Option<&str>, upf: f64) -> Option<(f64, Vec2, &'static str)> {
    for q in views(sp, p, level) {
        if let Some((cfm, at)) = grid.near(q, CFM_TAG_FT * upf).into_iter().find_map(|(t, at, _)| cfm_in(t).map(|c| (c, at))) {
            return Some((cfm, at, "the airflow printed beside the terminal"));
        }
        for (t, at, _) in grid.near(q, TERM_TAG_FT * upf) {
            if !terminal_type_tag(t) {
                continue;
            }
            if let Some((cfm, n_at)) = grid.near(at, TAG_LINE_FT * upf).into_iter().find_map(|(n, n_at, _)| airflow_number(n).map(|c| (c, n_at))) {
                return Some((cfm, n_at, "the number under the type in the terminal's tag"));
            }
        }
    }
    None
}

/// A terminal's airflow from its schedule when its plan prints none: the type mark beside it on
/// any of its sheets' views (`CD-1`) and that mark's row's CFM cell (`CFM`, `MAX CFM`, `AIRFLOW`).
/// The airflow, the mark and where the row is printed.
fn scheduled_airflow(grid: &TextGrid, sp: &BuildingSpec, schedules: &Rows, p: Vec2, level: Option<&str>, upf: f64) -> Option<(f64, String, String)> {
    if schedules.is_empty() {
        return None;
    }
    for q in views(sp, p, level) {
        for (t, _, _) in grid.near(q, TERM_TAG_FT * upf) {
            if !terminal_type_tag(t) {
                continue;
            }
            let mark = t.trim().to_ascii_uppercase();
            let Some((row, place)) = schedules.get(&mark) else { continue };
            let cfm = row.iter().find_map(|(k, v)| {
                let k = k.to_ascii_uppercase();
                (k.contains("CFM") || k.contains("AIRFLOW") || k.contains("AIR FLOW"))
                    .then(|| cfm_in(v).or_else(|| airflow_number(v.split(['-', '/', ' ']).next().unwrap_or(""))))
                    .flatten()
            });
            if let Some(c) = cfm {
                return Some((c, mark, place.clone()));
            }
        }
    }
    None
}

/// A schedule row given to the fill: its tag (upper case) → the row (column → text) and where
/// it is printed (`AIR HANDLING UNIT SCHEDULE M-601`).
pub(crate) type Rows = HashMap<String, (Vec<(String, String)>, String)>;

/// The `schedules` parameter: `[{tag, sheet?, title?, row: {column: text}}]` (the guided job
/// reads them off the schedule sheets).
fn schedules_param(p: &Value) -> Rows {
    let mut out: Rows = HashMap::new();
    let cap = |v: &str| v.chars().take(props::MAX_TEXT).collect::<String>();
    for v in p.get("schedules").and_then(Value::as_array).into_iter().flatten().take(MAX_SCHEDULE_ROWS) {
        let Some(tag) = v.get("tag").and_then(Value::as_str).map(|t| cap(t.trim()).to_ascii_uppercase()).filter(|t| !t.is_empty()) else { continue };
        let Some(row) = v.get("row").and_then(Value::as_object) else { continue };
        let cells: Vec<(String, String)> = row.iter().take(64).filter_map(|(k, x)| x.as_str().map(|x| (cap(k), cap(x)))).collect();
        let sheet = v.get("sheet").and_then(Value::as_str).unwrap_or("").trim();
        let title = v.get("title").and_then(Value::as_str).unwrap_or("").trim();
        let place = cap(format!("{title} {sheet}").trim());
        out.entry(tag).or_insert((cells, if place.is_empty() { "equipment schedule".into() } else { place }));
    }
    out
}

// ─── Plumbing fixtures: placed where a plan prints a scheduled fixture's tag ─

/// WALLGEN tag key of a plumbing fixture the fill placed (`fixid:<tag>@<x>,<y>`).
const FIXID: &str = "fixid";
/// The layer plumbing fixtures go on (its name makes them `fixture`s).
const FIXTURE_LAYER: &str = "3D-Plumbing Fixtures";
/// A fixture box: width, depth and height, inches.
const FIXTURE_IN: [f64; 3] = [20.0, 20.0, 30.0];
/// Two tags of one fixture read on two sheets (the plumbing plan and the floor plan) are within
/// this of each other, feet.
const SAME_FIXTURE_FT: f64 = 2.0;
/// Most fixtures placed.
const MAX_FIXTURES: usize = 20_000;

/// The property a plumbing fixture schedule's column gives: the connection sizes (`CW`, `HW`,
/// `SAN`/`WASTE`, `V`/`VENT`) and fixture units.
fn connection_key(column: &str) -> Option<&'static str> {
    let c = column.trim().to_ascii_uppercase().replace(['.', '(', ')'], " ");
    let words: Vec<&str> = c.split_whitespace().collect();
    let first = words.first().copied().unwrap_or("");
    if c.contains("WSFU") || (c.contains("FIXTURE UNIT") && !c.contains("DRAIN")) {
        return Some("fixture_units");
    }
    if c.contains("DFU") {
        return Some("drainage_units");
    }
    Some(match first {
        "CW" | "COLD" => "cw_size",
        "HW" | "HOT" => "hw_size",
        "SAN" | "SANITARY" | "WASTE" | "W" | "DRAIN" | "SOIL" => "waste_size",
        "V" | "VENT" => "vent_size",
        "TRAP" => "trap_size",
        _ => return None,
    })
}

/// The schedule rows that are plumbing fixtures': a fixture schedule's, or any row with
/// connection columns.
fn fixture_rows(schedules: &Rows) -> HashMap<&str, (&[(String, String)], &str)> {
    schedules
        .iter()
        .filter(|(_, (row, place))| {
            let conns = row.iter().filter(|(k, _)| connection_key(k).is_some_and(|c| c.ends_with("_size"))).count();
            conns >= 2 || (conns >= 1 && place.to_ascii_uppercase().contains("FIXTURE"))
        })
        .map(|(t, (row, place))| (t.as_str(), (row.as_slice(), place.as_str())))
        .collect()
}

/// Place one plumbing fixture element at each scheduled fixture's tag printed on a plumbing plan
/// (else the floor plan) of a built level, moved onto the level's architectural plan and its
/// stacking as the devices are; with its type mark, its schedule row's description and
/// connection sizes. Fixtures placed before (the same id) stay. How many were placed.
fn place_fixtures(d: &mut Drawing, sp: &BuildingSpec, schedules: &Rows, texts: &[(String, Vec2, f64)]) -> usize {
    let rows = fixture_rows(schedules);
    if rows.is_empty() {
        return 0;
    }
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let inside = |r: &[f64; 4], q: Vec2| q.x >= r[0].min(r[2]) && q.x <= r[0].max(r[2]) && q.y >= r[1].min(r[3]) && q.y <= r[1].max(r[3]);
    // Each built level's stacking move (its walls').
    let mut shift: HashMap<String, Vec2> = HashMap::new();
    let mut have: Vec<String> = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS * 4) {
        if let Some(id) = made(e, FIXID) {
            have.push(id);
        } else if super::wallgen::made_as(e).as_deref() == Some("wall")
            && let Some(l) = super::bspec::made_level(e)
        {
            shift.entry(l.to_ascii_uppercase()).or_insert_with(|| super::bspec::stacked_by(e));
        }
    }
    // The tags printed on a level's plumbing plans (first) and floor plans: a schedule's mark
    // (`WC-1`), or its letters when one row alone has them (`WC` for `WC-1`); not a legend's
    // entry (a word printed after it on its line).
    let by_letters = |t: &str| -> Option<String> {
        let mut hits = rows.keys().filter(|k| {
            k.strip_prefix(t).is_some_and(|rest| {
                let rest = rest.trim_start_matches('-');
                !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
            })
        });
        let one = hits.next()?;
        hits.next().is_none().then(|| (*one).to_string())
    };
    let grid = TextGrid::new(texts.to_vec(), 4.0 * upf);
    let legend = |p: Vec2, h: f64| {
        let h = if h.is_finite() && h > 0.0 { h } else { upf / 12.0 };
        grid.near(p, 15.0 * h)
            .into_iter()
            .any(|(w, q, _)| q.x > p.x && (q.y - p.y).abs() < 0.6 * h && w.chars().filter(char::is_ascii_alphabetic).count() >= 5)
    };
    let mut found: Vec<(u8, String, &str, &cadaddy_walk3d::Level, Vec2)> = Vec::new();
    for (t, p, h) in texts {
        let read = t.trim().to_ascii_uppercase();
        if read.is_empty() || read.len() > 12 {
            continue;
        }
        let Some(tag) = (if rows.contains_key(read.as_str()) { Some(read) } else { by_letters(&read) }) else { continue };
        if legend(*p, *h) {
            continue;
        }
        let Some(sheet) = sp.sheets.iter().find(|x| x.rect.is_some_and(|r| inside(&r, *p))) else { continue };
        let Some(level) = sheet.level.as_deref().and_then(|l| sp.level(l)).filter(|l| l.sheet.is_none()) else { continue };
        // The level's floor plan (the sheet holding its region), or a plumbing plan of its size
        // (moved onto it); not an enlarged or partial plan (they repeat what the plans show).
        let (Some(rect), Some(region)) = (sheet.rect, level.region) else { continue };
        let mid = Vec2::new((region[0] + region[2]) / 2.0, (region[1] + region[3]) / 2.0);
        let rank = if inside(&rect, mid) {
            1
        } else {
            let disc = sheet.discipline.clone().unwrap_or_else(|| sheet.number.chars().take_while(char::is_ascii_alphabetic).collect());
            let floor = sp.sheets.iter().filter_map(|x| x.rect).find(|r| inside(r, mid));
            let size = |r: &[f64; 4]| ((r[2] - r[0]).abs(), (r[3] - r[1]).abs());
            let same = floor.is_some_and(|f| {
                let ((fw, fh), (w, h)) = (size(&f), size(&rect));
                w > 0.0 && h > 0.0 && ((fw - w) / w).abs() < 0.02 && ((fh - h) / h).abs() < 0.02
            });
            if !(disc.eq_ignore_ascii_case("P") && same) {
                continue;
            }
            0
        };
        found.push((rank, tag, sheet.number.as_str(), level, *p));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut placed: Vec<(String, String, Vec2)> = Vec::new();
    let mut built = 0usize;
    if d.layer(FIXTURE_LAYER).is_none() {
        d.layers.push(cadaddy_doc::Layer {
            name: FIXTURE_LAYER.into(),
            color: cadaddy_doc::color::Color::Index(150),
            ..cadaddy_doc::Layer::default()
        });
    }
    for (_, tag, sheet, level, read_at) in found {
        if placed.len() >= MAX_FIXTURES {
            break;
        }
        let (at, registered) = register(sp, sheet, level, read_at);
        if placed.iter().any(|(l, t, q)| l.eq_ignore_ascii_case(&level.name) && *t == tag && q.dist(at) <= SAME_FIXTURE_FT * upf) {
            continue;
        }
        placed.push((level.name.clone(), tag.clone(), at));
        let id = format!("{tag}@{:.1},{:.1}", at.x / upf, at.y / upf);
        if have.contains(&id) {
            continue;
        }
        let Some((row, place)) = rows.get(tag.as_str()) else { continue };
        let off = shift.get(&level.name.to_ascii_uppercase()).copied().unwrap_or(Vec2::ZERO);
        let c = at + off;
        let (w, dp, h) = (FIXTURE_IN[0] * upf / 12.0, FIXTURE_IN[1] * upf / 12.0, FIXTURE_IN[2] * upf / 12.0);
        let z0 = level.elevation_ft * upf;
        let mut m = super::wallgen::empty_mesh();
        super::wallgen::add_box(&mut m, Vec2::new(c.x - w / 2.0, c.y), Vec2::new(c.x + w / 2.0, c.y), dp, z0, z0 + h);
        if m.faces.is_empty() {
            continue;
        }
        let mut xdata = super::wallgen::tag(cadaddy_walk3d::Part::Device, &level.name, Handle(0));
        xdata.push(RawTag { code: 1000, value: format!("{FIXID}:{}", id.chars().take(240).collect::<String>()) });
        if off != Vec2::ZERO {
            xdata = super::bspec::with_stacked(&xdata, off);
        }
        let printed = Source::print(sheet, Some([read_at.x, read_at.y]));
        let sched = Source::from_doc(SourceKind::Schedule, place);
        let mut store = Props::default();
        store.set("type_mark", Prop::new(tag.clone(), printed.clone()).with_note("the tag printed at the fixture"));
        if let Some((_, v)) = row.iter().find(|(k, v)| {
            let k = k.to_ascii_uppercase();
            (k.contains("FIXTURE") || k.contains("DESCRIPTION") || k == "TYPE" || k == "ITEM") && !v.trim().is_empty()
        }) {
            store.set("type", Prop::new(v.trim().to_string(), sched.clone()));
        }
        let mut conns: Vec<String> = Vec::new();
        for (k, v) in row.iter() {
            let v = v.trim();
            let Some(key) = connection_key(k) else { continue };
            if v.is_empty() || !v.chars().any(|ch| ch.is_ascii_digit()) {
                continue;
            }
            store.set(key, Prop::new(v.to_string(), sched.clone()).with_unit(if key.ends_with("_size") { "in" } else { "fu" }));
            if key.ends_with("_size") {
                conns.push(format!("{} {v}", k.trim()));
            }
        }
        if !conns.is_empty() {
            store.set("connections", Prop::new(conns.join(", "), sched.clone()));
        }
        store.set("sheet", Prop::new(sheet.to_string(), printed));
        let mut loc = location_prop(c.x, c.y, z0, upf);
        if registered {
            loc.note = Some("model coordinates: its tag read on a plumbing plan, moved onto its level's architectural plan by the sheet frame (same sheet size), at the level's floor".into());
        }
        store.set("location", loc);
        let xdata = props::write(&xdata, &store);
        if d.add(&Space::Model, Common { layer: FIXTURE_LAYER.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            built += 1;
        }
    }
    built
}

/// A built element's footprint, height range and centre, in model coordinates.
fn extent(d: &Drawing, e: &Entity) -> Option<(Bounds2, f64, f64)> {
    let EntityKind::Mesh(m) = &e.kind else {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        return (!b.is_empty()).then_some((b, 0.0, 0.0));
    };
    let mut b = Bounds2::EMPTY;
    let (mut z0, mut z1) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in m.vertices.iter().take(1_000_000) {
        if v.x.is_finite() && v.y.is_finite() && v.z.is_finite() {
            b = b.union(&Bounds2::from_points([Vec2::new(v.x, v.y)]));
            z0 = z0.min(v.z);
            z1 = z1.max(v.z);
        }
    }
    (!b.is_empty() && z0.is_finite()).then_some((b, z0, z1))
}

/// The two points a little past each long face of a footprint (a wall's sides; a door's).
fn sides(b: &Bounds2, out_by: f64) -> [Vec2; 2] {
    let c = b.center();
    if b.width() >= b.height() {
        let d = b.height() / 2.0 + out_by;
        [Vec2::new(c.x, c.y - d), Vec2::new(c.x, c.y + d)]
    } else {
        let d = b.width() / 2.0 + out_by;
        [Vec2::new(c.x - d, c.y), Vec2::new(c.x + d, c.y)]
    }
}

/// The distance from `p` to a footprint (0 inside).
fn dist_to(b: &Bounds2, p: Vec2) -> f64 {
    let dx = (b.min.x - p.x).max(p.x - b.max.x).max(0.0);
    let dy = (b.min.y - p.y).max(p.y - b.max.y).max(0.0);
    dx.hypot(dy)
}

/// The source of a value whose reader said how it got it (`tag`, `schedule`, `estimated-route`…).
fn source_said(how: &str, sheet: &str) -> Source {
    let h = how.trim().to_ascii_lowercase();
    if h.is_empty() || h.contains("estimat") || h.contains("default") || h.contains("typical") || h.contains("guess") || h.contains("system") {
        let mut s = Source::of(SourceKind::Default);
        s.doc = Some(if h.is_empty() { "not read: the system's typical value".to_string() } else { h }).filter(|d| !d.is_empty());
        s
    } else if h.contains("schedule") {
        Source::from_doc(SourceKind::Schedule, sheet)
    } else if h.contains("note") {
        Source::from_doc(SourceKind::Note, sheet)
    } else if h.contains("user") || h.contains("hand") {
        Source::user()
    } else {
        Source::print(sheet, None)
    }
}

/// The value of a WALLGEN tag key on a built element (`size`, `system`, `tag`…).
fn made(e: &Entity, key: &str) -> Option<String> {
    super::bimlook::made_tag(e, key).map(str::to_string).filter(|v| !v.trim().is_empty())
}

/// An equipment's capacity, from its schedule row's values (`cfm`, `tons`, `MBH`, `kW`, `gpm`,
/// `gal`): the first one there is, with its unit.
fn capacity_of(props: &[(String, String)]) -> Option<(String, String)> {
    let units: [(&str, &str); 9] = [
        ("CAPACITY", ""),
        ("CFM", "cfm"),
        ("TON", "tons"),
        ("MBH", "MBH"),
        ("BTU", "BTU/h"),
        ("KW", "kW"),
        ("GPM", "gpm"),
        ("GAL", "gal"),
        ("HP", "hp"),
    ];
    for (word, unit) in units {
        if let Some((_, v)) = props.iter().find(|(k, v)| k.to_ascii_uppercase().contains(word) && v.chars().any(|c| c.is_ascii_digit())) {
            return Some((v.trim().to_string(), unit.to_string()));
        }
    }
    None
}

/// Fill the built model's elements from what the readers found; per kind, how many values went
/// on each key.
pub(crate) fn fill(s: &mut Session, schedules: &Rows, read: &[ReadRoom]) -> Result<Value> {
    let (sp, stored) = super::bspec::current(s, FILL)?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    // The rooms (the build's, or the plan's), in model coordinates.
    // The plan's texts, for room and door numbers, airflows and fixture tags.
    let texts: Vec<(String, Vec2, f64)> = {
        let plan = super::bspec::plan_text_objects(s.doc()?);
        super::perceive::texts_and_tags(&plan).into_iter().map(|(_, t, p, h)| (t, p, h)).filter(|(t, _, _)| !t.is_empty()).collect()
    };
    // A room element whose tag gave no number: the room number printed inside it.
    let (rooms_numbered, rooms_numbered_from_reading) = number_rooms(s.doc_mut()?, &sp, &texts, read);
    // The numbered rooms the job read and the model has no room for.
    let (read_rooms_added, read_shapes) = add_read_rooms(s.doc_mut()?, &sp, read, upf);
    let (rooms3, rooms_error) = match super::rooms3d::rooms(s) {
        Ok(r) => (r, None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    let mut rooms: Vec<RoomShape> = room_shapes(s.doc()?, &rooms3);
    rooms.extend(read_shapes);
    let fixtures_placed = place_fixtures(s.doc_mut()?, &sp, schedules, &texts);
    let d = s.doc()?;
    let text_count = texts.len();
    let grid = TextGrid::new(texts, 4.0 * upf);
    // The runs and terminals by the ids their built objects carry.
    let runs: HashMap<String, &cadaddy_walk3d::MepRun> = sp.mep.runs.iter().map(|r| (super::mep::run_base(r), r)).collect();
    let equipment: HashMap<String, &cadaddy_walk3d::MepEquipment> = sp.mep.equipment.iter().map(|e| (e.tag.trim().to_ascii_uppercase(), e)).collect();
    // The walls, for the devices' host walls.
    let mut walls: Vec<(Handle, Option<String>, Bounds2)> = Vec::new();
    let mut work: Vec<(Handle, String)> = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS * 4) {
        let Some(kind) = super::elemprops::element_kind(e) else { continue };
        if kind == "wall"
            && let Some((b, _, _)) = extent(d, e)
        {
            walls.push((e.handle, super::bspec::made_level(e), b));
        }
        work.push((e.handle, kind));
        if work.len() >= MAX_ELEMENTS {
            break;
        }
    }
    // The panels and other equipment by tag, for the devices' fed_from.
    let gear: HashMap<String, Handle> = work
        .iter()
        .filter(|(_, k)| k == "equipment")
        .filter_map(|(h, _)| d.model.get(*h).and_then(|e| made(e, "tag")).map(|t| (t.trim().to_ascii_uppercase(), *h)))
        .collect();
    // The trades' sheets put onto their levels' architectural plans by the door and room numbers
    // both print (`bim.identity`'s registration), for the devices read on them.
    let registration = super::identity::register(d, &super::identity::sheets_of(Some(&sp)), upf);
    let placements: HashMap<String, &cadaddy_mepfix::tagid::SheetTransform> =
        registration.transforms.iter().filter(|t| t.registered && t.method == "tags").map(|t| (t.sheet.trim().to_ascii_uppercase(), t)).collect();
    let (mut devices_roomed_by_fit, mut devices_without_room) = (0usize, 0usize);
    // Doors with no tag beside them and the one numbered room each opens into; the numbers the
    // tagged doors have, per level.
    let mut doors_by_room: Vec<(Handle, String, String)> = Vec::new();
    let mut door_numbers_used: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut doors_untagged = 0usize;
    let room_level = |label: &str| rooms3.iter().find(|r| room_label(r.number.as_deref(), &r.name).eq_ignore_ascii_case(label));
    let mut filled: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut changes: Vec<(Handle, Props)> = Vec::new();
    let mut element_levels: BTreeMap<String, usize> = BTreeMap::new();
    let mut terminals = 0usize;
    for (h, kind) in &work {
        let Some(e) = d.model.get(*h) else { continue };
        let mut store = props::read(&e.common.xdata);
        let before = store.clone();
        let tally = filled.entry(kind.clone()).or_default();
        let level = super::bspec::made_level(e);
        let lvl = level.as_deref();
        if kind == "device" && element_levels.len() < 50 {
            *element_levels.entry(level.clone().unwrap_or_default()).or_default() += 1;
        }
        let ext = extent(d, e);
        let level_z = lvl.and_then(|l| sp.level(l)).map_or(0.0, |l| l.elevation_ft * upf);
        let shift = super::bspec::stacked_by(e);
        match kind.as_str() {
            "device" | "light_fixture" | "fixture" if made(e, DEVID).is_some() || made(e, FIXID).is_some() || kind == "device" => {
                let Some((b, z0, z1)) = ext else { continue };
                let c = b.center();
                // Where its sheet's shared door and room numbers put it on its level's plan (a
                // sheet whose plan sits elsewhere on the paper than the architectural one's), else
                // where it was built.
                let fitted = registered_at(&store, &placements).map(|q| q + shift);
                let found = match fitted.and_then(|q| room_near(&rooms, lvl, q, NEAR_ROOM_FT * upf)) {
                    Some((r, _)) => {
                        devices_roomed_by_fit += 1;
                        Some((
                            r,
                            "the room at its place on its level's architectural plan, by the door and room numbers its sheet shares with that plan",
                        ))
                    }
                    None => room_near(&rooms, lvl, c, NEAR_ROOM_FT * upf)
                        .map(|(r, inside)| (r, if inside { "the room the device stands in" } else { "the nearest room: the device is on its wall" })),
                };
                if let Some((r, note)) = found {
                    put(&mut store, "room", Prop::new(r.label.clone(), Source::computed()).with_note(note), tally);
                } else if kind == "device" {
                    devices_without_room += 1;
                }
                let host = walls
                    .iter()
                    .filter(|(_, l, _)| l.as_deref().zip(lvl).is_none_or(|(a, b)| level_key(a) == level_key(b)))
                    .map(|(wh, _, wb)| (wh, dist_to(wb, c)))
                    .filter(|(_, dd)| *dd <= HOST_WALL_FT * upf)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((wh, _)) = host {
                    put(&mut store, "host_wall", Prop::new(wh.hex(), Source::computed()).with_note("the nearest wall of its level"), tally);
                }
                // The panel it is fed from, as an element, and its breaker on the panel's schedule.
                let panel = store.get("panel").filter(|p| p.has_value()).map(Prop::text);
                let circuit = store.get("circuit").filter(|p| p.has_value()).map(Prop::text);
                if let Some(p) = &panel {
                    if let Some(gh) = gear.get(&p.trim().to_ascii_uppercase()) {
                        put(
                            &mut store,
                            "fed_from_handle",
                            Prop::new(gh.hex(), Source::computed()).with_note("the panel the circuit tag names"),
                            tally,
                        );
                    }
                    let number = circuit.as_deref().and_then(|c| c.rsplit_once(['-', '/', '.']).map(|(_, n)| n.trim().to_string()));
                    let row = equipment
                        .get(&p.trim().to_ascii_uppercase())
                        .and_then(|q| q.circuits.iter().find(|c| number.as_deref().is_some_and(|n| c.number.trim() == n)));
                    if let Some(c) = row {
                        let src = Source::from_doc(SourceKind::Schedule, if c.source.is_empty() { "panel schedule" } else { &c.source });
                        if !c.breaker.trim().is_empty() {
                            put(&mut store, "breaker", Prop::new(c.breaker.trim().to_string(), src.clone()), tally);
                        }
                        if c.poles > 0 {
                            put(&mut store, "poles", Prop::new(c.poles, src.clone()), tally);
                        }
                        if !c.description.trim().is_empty() {
                            put(&mut store, "circuit_description", Prop::new(c.description.trim().to_string(), src), tally);
                        }
                    }
                }
                if store.get("location").is_none_or(|p| !p.has_value()) {
                    put(&mut store, "location", location_prop(c.x, c.y, (z0 + z1) / 2.0, upf), tally);
                }
            }
            "room" => {
                let label = super::rooms3d::room_of(&e.common).unwrap_or_default();
                let number = e
                    .common
                    .xdata
                    .iter()
                    .position(|t| t.code == 1001 && t.value == super::rooms3d::ROOM_APP)
                    .and_then(|i| e.common.xdata.get(i + 2))
                    .map(|t| t.value.clone())
                    .unwrap_or_default();
                let Some(r) = room_level(&room_label(Some(&number), &label)) else { continue };
                if let Some(l) = &r.level {
                    put(&mut store, "level", Prop::new(l.clone(), Source::computed()), tally);
                }
                if r.ceiling.is_finite() && r.ceiling > 0.0 {
                    put(&mut store, "ceiling_height", Prop::new(round2(r.ceiling / upf), source_said(&r.ceiling_source, "")).with_unit("ft"), tally);
                }
                for f in &r.finish_detail {
                    let key = match f.slot.to_ascii_lowercase().as_str() {
                        "floor" => "floor_finish",
                        "wall" | "walls" => "wall_finish",
                        "ceiling" => "ceiling_finish",
                        "base" => "base_finish",
                        _ => continue,
                    };
                    put(&mut store, key, Prop::new(f.code.clone(), source_said(&f.source, "finish schedule")), tally);
                }
            }
            "door" => {
                let Some((b, z0, z1)) = ext else { continue };
                // The number printed at the opening (on the plan: the build moved stacked levels).
                let at_plan = b.center() - shift;
                if let Some((t, q, _)) =
                    grid.near(at_plan, DOOR_TAG_FT * upf).into_iter().find(|(t, _, _)| door_number(t) && !super::guided::is_circuit_tag(t))
                {
                    put(
                        &mut store,
                        "number",
                        Prop::new(t.trim().to_string(), Source::print(&sheet_at(&sp, q, lvl), Some([q.x, q.y])))
                            .with_note("the tag printed at the opening"),
                        tally,
                    );
                }
                let width = b.width().max(b.height()) / upf * 12.0;
                if width.is_finite() && width > 0.0 {
                    put(&mut store, "width", Prop::new(round2(width), Source::computed()).with_unit("in"), tally);
                }
                if (z1 - z0).is_finite() && z1 > z0 {
                    put(&mut store, "height", Prop::new(round2((z1 - z0) / upf * 12.0), Source::computed()).with_unit("in"), tally);
                }
                let mut near: Vec<&RoomShape> = Vec::new();
                for p in sides(&b, SIDE_FT * upf).into_iter().chain([b.center()]) {
                    if let Some(r) = room_at(&rooms, lvl, p)
                        && !near.iter().any(|x| std::ptr::eq(*x, r))
                    {
                        near.push(r);
                    }
                }
                // No tag at the opening: it may take the number of the one numbered room it opens
                // into (below, once every door's tag is known).
                let numbered: Vec<&str> = near.iter().map(|r| r.label.trim()).filter(|l| super::perceive::geometry::room_number(l)).collect();
                match store.get("number").filter(|p| p.has_value()) {
                    Some(n) => {
                        door_numbers_used.insert((level_key(lvl.unwrap_or("")), n.text().trim().to_ascii_uppercase()));
                    }
                    None => {
                        doors_untagged += 1;
                        if let [one] = numbered.as_slice() {
                            doors_by_room.push((*h, level_key(lvl.unwrap_or("")), one.to_ascii_uppercase()));
                        }
                    }
                }
                if let Some(r) = near.first() {
                    put(&mut store, "from_room", Prop::new(r.label.clone(), Source::computed()), tally);
                }
                if let Some(r) = near.get(1) {
                    put(&mut store, "to_room", Prop::new(r.label.clone(), Source::computed()), tally);
                }
            }
            "wall" => {
                let Some((b, z0, z1)) = ext else { continue };
                if let Some(t) = made(e, "type") {
                    let marked = sp.walls.iter().any(|m| {
                        m.wall_type.eq_ignore_ascii_case(&t)
                            && m.status() == cadaddy_walk3d::MarkStatus::Drawing
                            && lvl.is_some_and(|l| l.eq_ignore_ascii_case(&m.level))
                            && dist_to(&b, Vec2::new((m.a[0] + m.b[0]) / 2.0, (m.a[1] + m.b[1]) / 2.0) + shift) <= upf
                    });
                    let src = if marked {
                        Source::print(&sheet_at(&sp, b.center() - shift, lvl), None)
                    } else {
                        Source::from_doc(SourceKind::Rule, "the building spec's wall type for the wall's layer")
                    };
                    put(&mut store, "type", Prop::new(t.clone(), src), tally);
                    if let Some(wt) = sp.wall_types.iter().find(|w| w.name.eq_ignore_ascii_case(&t)) {
                        let src = Source::from_doc(SourceKind::Spec, &format!("wall type {}", wt.name));
                        put(&mut store, "thickness", Prop::new(round2(wt.thickness_in), src).with_unit("in"), tally);
                    }
                }
                if (z1 - z0).is_finite() && z1 > z0 {
                    put(&mut store, "height", Prop::new(round2((z1 - z0) / upf), Source::computed()).with_unit("ft"), tally);
                }
                let [a, c] = sides(&b, SIDE_FT * upf);
                if let Some(r) = room_at(&rooms, lvl, a) {
                    put(&mut store, "room_a", Prop::new(r.label.clone(), Source::computed()), tally);
                }
                if let Some(r) = room_at(&rooms, lvl, c) {
                    put(&mut store, "room_b", Prop::new(r.label.clone(), Source::computed()), tally);
                }
            }
            "duct" | "pipe" | "conduit" | "fixture" | "equipment" if super::wallgen::made_as(e).as_deref() == Some("mep") => {
                let part = made(e, "part").unwrap_or_default();
                let id = made(e, "mepid").unwrap_or_default();
                let run = id.rsplit_once('/').and_then(|(base, _)| runs.get(base)).copied();
                let sheet = run
                    .map(|r| r.source_sheet.clone())
                    .filter(|x| !x.is_empty())
                    .unwrap_or_else(|| ext.map_or_else(String::new, |(b, _, _)| sheet_at(&sp, b.center() - shift, lvl)));
                if let Some(sy) = made(e, "system") {
                    put(&mut store, "system", Prop::new(sy, Source::print(&sheet, None)), tally);
                }
                if let Some(size) = made(e, "size") {
                    let src = run.map_or_else(|| Source::print(&sheet, None), |r| source_said(&r.size_source, &sheet));
                    put(&mut store, "size", Prop::new(size.clone(), src.clone()).with_unit("in"), tally);
                    put(&mut store, "shape", Prop::new(if size.contains('x') { "rectangular" } else { "round" }, src), tally);
                }
                if let Some((b, z0, _)) = ext {
                    if kind == "duct" || kind == "pipe" || kind == "conduit" {
                        let src = run.map_or_else(|| Source::of(SourceKind::Default), |r| source_said(&r.height_source, &sheet));
                        put(
                            &mut store,
                            "elevation",
                            Prop::new(round2((z0 - level_z) / upf), src).with_unit("ft").with_note("bottom, above the level"),
                            tally,
                        );
                    }
                    if part.starts_with("terminal") {
                        terminals += 1;
                        let kind_word = part.trim_start_matches("terminal").trim_start_matches(':');
                        if !kind_word.is_empty() {
                            put(&mut store, "type", Prop::new(kind_word.to_string(), Source::print(&sheet, None)), tally);
                        }
                        let at_plan = b.center() - shift;
                        if let Some((cfm, q, how)) = airflow_at(&grid, &sp, at_plan, lvl, upf) {
                            let src = Source::print(&sheet_at(&sp, q, lvl), Some([q.x, q.y]));
                            put(&mut store, "cfm", Prop::new(cfm, src).with_unit("cfm").with_note(how), tally);
                        } else if let Some((cfm, mark, place)) = scheduled_airflow(&grid, &sp, schedules, at_plan, lvl, upf) {
                            let note = format!("the airflow on the schedule's {mark} row (the type printed beside the terminal)");
                            put(
                                &mut store,
                                "cfm",
                                Prop::new(cfm, Source::from_doc(SourceKind::Schedule, &place)).with_unit("cfm").with_note(&note),
                                tally,
                            );
                        }
                        if let Some((r, _)) = room_near(&rooms, lvl, b.center(), NEAR_ROOM_FT * upf) {
                            put(&mut store, "room", Prop::new(r.label.clone(), Source::computed()), tally);
                        }
                    }
                    if part.starts_with("equipment") {
                        let tag = made(e, "tag").unwrap_or_default();
                        if !tag.is_empty() {
                            put(&mut store, "type_mark", Prop::new(tag.clone(), Source::print(&sheet, None)), tally);
                        }
                        let rows: Vec<(String, String)> = super::bimlook::made_tags(e)
                            .into_iter()
                            .flatten()
                            .filter_map(|t| {
                                t.value.strip_prefix("prop:").and_then(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string()))
                            })
                            .filter(|(k, _)| !k.starts_with("SOURCE "))
                            .collect();
                        let scheduled = schedules.get(&tag.trim().to_ascii_uppercase());
                        let capacity = capacity_of(&rows)
                            .map(|c| {
                                let sched = equipment
                                    .get(&tag.to_ascii_uppercase())
                                    .and_then(|q| q.sources.get("schedule").or_else(|| q.sources.values().next()).cloned())
                                    .unwrap_or_else(|| "equipment schedule".into());
                                (c, sched)
                            })
                            .or_else(|| scheduled.and_then(|(row, place)| capacity_of(row).map(|c| (c, place.clone()))));
                        if let Some(((v, unit), sched)) = capacity {
                            let mut p = Prop::new(v, Source::from_doc(SourceKind::Schedule, &sched));
                            if !unit.is_empty() {
                                p = p.with_unit(&unit);
                            }
                            put(&mut store, "capacity", p, tally);
                        }
                        if let Some((r, _)) = room_near(&rooms, lvl, b.center(), NEAR_ROOM_FT * upf) {
                            put(&mut store, "room", Prop::new(r.label.clone(), Source::computed()), tally);
                        }
                    }
                }
            }
            _ => {}
        }
        if store != before {
            changes.push((*h, store));
        }
    }
    let doors_numbered_by_room = number_doors_by_room(d, &mut changes, &doors_by_room, &door_numbers_used, filled.entry("door".into()).or_default());
    let d = s.doc_mut()?;
    let mut changed = 0usize;
    for (h, store) in changes {
        if d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).is_ok() {
            changed += 1;
        }
    }
    filled.retain(|_, m| !m.is_empty());
    if changed + fixtures_placed + rooms_numbered + rooms_numbered_from_reading + read_rooms_added > 0 {
        s.touch();
    }
    // What the fill had to go on, for a run on a set no one can see.
    let mut room_levels: BTreeMap<String, usize> = BTreeMap::new();
    for r in &rooms {
        *room_levels.entry(r.level.clone().unwrap_or_default()).or_default() += 1;
    }
    let mut diagnostics = json!({
        "rooms": rooms.len(),
        "rooms_labelled": rooms.iter().filter(|r| !r.label.trim().is_empty()).count(),
        "rooms_numbered_from_text": rooms_numbered,
        "rooms_numbered_from_reading": rooms_numbered_from_reading,
        "read_rooms": read.len(),
        "read_rooms_numbered": read.iter().filter(|r| r.number.is_some()).count(),
        "read_rooms_with_outline": read.iter().filter(|r| r.outline.len() >= 3).count(),
        "read_rooms_added": read_rooms_added,
        "room_levels": room_levels,
        "element_levels": element_levels,
        "schedule_rows": schedules.len(),
        "fixture_rows": fixture_rows(schedules).len(),
        "terminals": terminals,
        "doors_untagged": doors_untagged,
        "doors_numbered_by_room": doors_numbered_by_room,
        "sheets_registered_by_tags": placements.len(),
        "devices_roomed_by_registration": devices_roomed_by_fit,
        "devices_without_room": devices_without_room,
        "texts": text_count,
        "room_elements": super::bspec::room_elements(s.doc()?).len(),
        "drawing_objects": s.doc()?.model.len(),
    });
    if let Some(e) = rooms_error {
        diagnostics["rooms_error"] = json!(e);
    }
    Ok(json!({ "filled": filled, "elements": changed, "fixtures_placed": fixtures_placed, "spec": stored, "diagnostics": diagnostics }))
}

/// Doors are numbered after the room they open into (door 101 into room 101). A door with no tag
/// printed beside it that opens into one numbered room (the other side a corridor or outside)
/// takes that room's number when it is the only such door of that room on its level and no tagged
/// door of the level has the number already. The number is the convention's (`rule`), never
/// presented as read. How many doors took one.
fn number_doors_by_room(
    d: &Drawing,
    changes: &mut Vec<(Handle, Props)>,
    doors: &[(Handle, String, String)],
    used: &std::collections::HashSet<(String, String)>,
    tally: &mut BTreeMap<String, usize>,
) -> usize {
    let mut wanting: HashMap<(&str, &str), usize> = HashMap::new();
    for (_, level, number) in doors {
        *wanting.entry((level.as_str(), number.as_str())).or_default() += 1;
    }
    let mut numbered = 0;
    for (h, level, number) in doors {
        if wanting.get(&(level.as_str(), number.as_str())) != Some(&1) || used.contains(&(level.clone(), number.clone())) {
            continue;
        }
        let prop = Prop::new(number.clone(), Source::from_doc(SourceKind::Rule, "doors are numbered after the room they open into"))
            .with_note("no tag printed at the opening: the number of the one numbered room it opens into");
        match changes.iter_mut().find(|(x, _)| x == h) {
            Some((_, store)) => put(store, "number", prop, tally),
            None => {
                let Some(e) = d.model.get(*h) else { continue };
                let mut store = props::read(&e.common.xdata);
                put(&mut store, "number", prop, tally);
                changes.push((*h, store));
            }
        }
        numbered += 1;
    }
    numbered
}

/// Where a device read on a registered sheet lies on its level's architectural plan: its read
/// point (its `sheet` value's) through its sheet's registration; none when the sheet has none.
fn registered_at(store: &Props, placements: &HashMap<String, &cadaddy_mepfix::tagid::SheetTransform>) -> Option<Vec2> {
    let src = &store.get("sheet")?.source;
    let t = placements.get(&src.doc.as_deref()?.trim().to_ascii_uppercase())?;
    let [x, y] = t.place(src.at?)?;
    let q = Vec2::new(x, y);
    q.is_finite().then_some(q)
}

/// The sheet a plan point lies on (the set's sheet rectangles), else the level's name.
fn sheet_at(sp: &BuildingSpec, p: Vec2, level: Option<&str>) -> String {
    sp.sheets
        .iter()
        .find(|x| x.rect.is_some_and(|[x0, y0, x1, y1]| p.x >= x0.min(x1) && p.x <= x0.max(x1) && p.y >= y0.min(y1) && p.y <= y0.max(y1)))
        .map(|x| x.number.clone())
        .or_else(|| level.map(str::to_string))
        .unwrap_or_default()
}

fn run_fill(s: &mut Session, p: &Value) -> Result<Value> {
    let mut r = fill(s, &schedules_param(p), &rooms_param(p))?;
    let devices = s.doc().map_or(0, |d| d.model.iter().filter(|e| made(e, DEVID).is_some()).count());
    r["devices"] = json!(devices);
    let n = r["elements"].as_u64().unwrap_or(0);
    s.echo(format!("Filled the properties of {n} element(s)."));
    Ok(r)
}
