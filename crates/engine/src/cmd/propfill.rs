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
//! - **air terminals**: their kind, neck size and the airflow printed beside them (`150 CFM`).
//! - **equipment**: its tag, kind and the capacity on its schedule row.
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
const DOOR_TAG_FT: f64 = 3.0;
const CFM_TAG_FT: f64 = 4.0;
/// How far from a wall a device may stand and still be on it, feet.
const HOST_WALL_FT: f64 = 1.5;
/// A luminaire on a wall with no height printed (an exit sign over a door, a sconce), inches.
const WALL_LUMINAIRE_IN: f64 = 90.0;
/// A switch is always on a wall: read at its designation beside the symbol (or stamped from a
/// typical room's plan), it is on the nearest wall this far, feet.
const SWITCH_WALL_FT: f64 = 3.0;

/// A wall device's room is looked for this far past the wall's face (feet).
const FACE_PROBE_FT: f64 = 0.5;

/// A device in no room's outline takes a room this close (feet).
const ROOM_NEAR_FT: f64 = 1.0;
/// A device whose box starts below this height (feet off its floor) is in the wall, not the
/// ceiling: its box gets a plaster ring.
const WALL_BOX_FT: f64 = 7.0;

/// A wall device goes on the face of a wall this close to where it was read (feet); none that
/// close: it stays, for review.
pub(crate) const HOST_SNAP_FT: f64 = 3.0;

/// A device box this low (inches over the floor) is in the floor, not on a wall.
const FLOOR_BOX_IN: f64 = 2.0;
/// How far beyond a wall's face the rooms on its sides are looked for, feet.
const SIDE_FT: f64 = 1.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(FILL, "Fill Element Properties", run_fill)
            .params("{} → {filled: {kind: {key: n}}, elements, devices, mount_heights: {type_mark: {inches, source, doc?}}}: what the readers found goes on the built model's elements (devices, rooms, doors, walls, ducts, pipes, terminals, equipment), each value with its source; a person's values stay")
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
pub(crate) fn kind_of(mark: &str) -> &'static str {
    match mark.trim().to_ascii_uppercase().as_str() {
        "OS" | "OS1" | "OS2" | "OC" | "VS" | "PS" => "occupancy sensor",
        "DH" | "MH" => "door holder",
        "J" | "JB" => "junction box",
        "S" | "S2" | "S3" | "S4" | "SD" | "DS" | "LV" | "SP" | "SM" | "SH" | "SK" | "SOR" | "SWT" => "switch",
        "FA" | "HS" | "SS" | "FS" | "ST" | "CM" => "fire alarm device",
        "TV" | "DA" | "TEL" | "WAP" | "CR" | "CAM" => "low voltage outlet",
        "EF" | "FCU" | "WH" | "PTAC" => "equipment connection",
        "T" | "TSTAT" | "TH" => "thermostat",
        _ => "device",
    }
}

/// The usual mounting height of a kind of device (a default: always marked), inches above the
/// floor.
fn default_height(kind: &str) -> f64 {
    match kind {
        "switch" | "equipment connection" | "thermostat" => 48.0,
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
/// Whether a legend row's words (upper case) can describe a device of `kind`: a switch's row
/// says switch or dimmer, a sensor's sensor, a fire alarm device's what it is; a device of no
/// known kind takes any row.
fn row_fits(kind: &str, words: &str) -> bool {
    let any = |ws: &[&str]| ws.iter().any(|w| words.contains(w));
    match kind {
        "switch" => any(&["SWITCH", "DIMMER"]),
        "occupancy sensor" => any(&["SENSOR", "OCCUPANCY", "VACANCY"]),
        "fire alarm device" => any(&["FIRE", "STROBE", "HORN", "SMOKE", "PULL", "DETECTOR", "SPEAKER", "ALARM"]),
        _ => true,
    }
}

pub fn mount_heights(d: &Drawing, marks: &[String]) -> BTreeMap<String, Height> {
    let ents = super::bspec::plan_objects(d);
    let texts = super::perceive::texts(&ents);
    let wanted: Vec<String> = marks.iter().map(|m| m.trim().to_ascii_uppercase()).filter(|m| !m.is_empty()).collect();
    // Each mark's legend height and the row's words.
    let mut legend: BTreeMap<String, (Height, String)> = BTreeMap::new();
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
            legend
                .entry(t.trim().to_ascii_uppercase())
                .or_insert((Height { inches, source: SourceKind::Legend, doc: "device legend".into() }, up.clone()));
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
            // A legend row of another kind of device under the same mark is not this one's.
            let row = legend.get(m).filter(|(_, words)| row_fits(kind, words)).map(|(h, _)| h.clone());
            let h = row.or_else(|| by_note.get(kind).cloned()).unwrap_or_else(|| Height {
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

/// A circuit tag and the switch leg printed with it: `A-1,a` is circuit `A-1` on switch leg `a`
/// (a lower-case letter after a comma); `LP-3,5` is two circuits and no leg.
pub fn split_switch_leg(tag: &str) -> (String, Option<String>) {
    let tag = tag.trim();
    match tag.rsplit_once(',') {
        Some((circuit, leg)) if leg.trim().len() == 1 && leg.trim().chars().all(|c| c.is_ascii_lowercase()) => {
            (circuit.trim().to_string(), Some(leg.trim().to_string()))
        }
        _ => (tag.to_string(), None),
    }
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
    // The built ceilings (rooms', the reflected ceiling plans'), for what hangs at the ceiling: a
    // luminaire, a smoke detector. Their level, plan extent and underside.
    let wants_ceiling = devices.iter().any(|t| field(t, "mount").as_deref() == Some("ceiling"));
    let ceilings: Vec<(Option<String>, Bounds2, f64)> = if wants_ceiling {
        d.model
            .iter()
            .filter(|e| super::elemprops::element_kind(e).as_deref() == Some("ceiling"))
            .filter_map(|e| extent(d, e).map(|(b, z0, _)| (super::bspec::made_level(e), b, z0)))
            .filter(|(_, b, z)| !b.is_empty() && z.is_finite())
            .take(MAX_ELEMENTS)
            .collect()
    } else {
        Vec::new()
    };
    // The built walls (their plan footprints), for putting a wall device on its wall's face.
    let walls = wall_footprints(d);
    let mut built = 0;
    for t in devices {
        let Some(level) = sp.level(&t.level).filter(|l| l.sheet.is_none()) else { continue };
        let mark = field(t, "type").unwrap_or_default();
        // What its sheet made it (a fire alarm plan's `S`), else what its mark says.
        let kind = match t.kind.replace('_', " ").as_str() {
            "fire alarm device" => "fire alarm device",
            _ => kind_of(&mark),
        };
        let read_at = Vec2::new(t.at_ft[0] * upf, t.at_ft[1] * upf);
        let sheet_name = field(t, "sheet").unwrap_or_default();
        let (placed, registered) = register(sp, &sheet_name, level, read_at);
        let (x, y) = (placed.x, placed.y);
        // A printed height (its legend's, a note's) first; else one that hangs at the ceiling is
        // at the underside of the ceiling over it (the smallest: a soffit in a room), else its
        // level's ceiling; else the usual height for its kind.
        let ceiling = (field(t, "mount").as_deref() == Some("ceiling")).then(|| -> Option<Height> {
            let p = Vec2::new(x, y);
            let floor = level.elevation_ft * upf;
            let over = ceilings
                .iter()
                .filter(|(l, b, _)| l.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(&level.name)) && b.contains(p))
                .min_by(|a, b| (a.1.width() * a.1.height()).total_cmp(&(b.1.width() * b.1.height())))
                .map(|(_, _, z)| (z - floor) / upf * 12.0)
                .filter(|inches| inches.is_finite() && *inches > 72.0);
            let what = if kind == "fire alarm device" { "a detector" } else { "a luminaire" };
            match over {
                Some(inches) => Some(Height { inches, source: SourceKind::Rule, doc: format!("{what}: at the ceiling over it") }),
                None => (level.ceiling_ft.is_finite() && level.ceiling_ft > 6.0).then(|| Height {
                    inches: level.ceiling_ft * 12.0,
                    source: SourceKind::Rule,
                    doc: format!("{what}: at its level's ceiling"),
                }),
            }
        });
        // A luminaire on a wall (an exit sign, a sconce, a wall pack): 7'-6" unless printed.
        let wall_light = (field(t, "mount").as_deref() == Some("wall")).then(|| Height {
            inches: WALL_LUMINAIRE_IN,
            source: SourceKind::Rule,
            doc: "a wall luminaire (exit sign, sconce): 7'-6\" over the floor".into(),
        });
        let known = heights.get(&mark.to_ascii_uppercase());
        // A printed height under 7' is no ceiling device's: another device of the legend under the
        // same mark (a receptacle's +44"); 7' and up is a wall-mounted one's (a sconce, an exit). A
        // wall luminaire's is 6' and up.
        let ceiling_mount = ceiling.as_ref().is_some_and(Option::is_some);
        let on_a_wall = wall_light.is_some();
        let lowest = if ceiling_mount {
            WALL_BOX_FT * 12.0
        } else if wall_light.is_some() {
            72.0
        } else {
            0.0
        };
        let printed = known.filter(|h| h.source != SourceKind::Default && h.inches >= lowest).cloned();
        // Its own height printed by it on its typical room's plan ("44\" AFF"), before any other.
        let noted = field(t, "mount_height_in").and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0).map(|inches| Height {
            inches,
            source: SourceKind::Note,
            doc: field(t, "mount_height_note").unwrap_or_else(|| "printed by it".into()),
        });
        let height = noted.or(printed).or(ceiling.flatten()).or(wall_light).or_else(|| known.cloned()).unwrap_or(Height {
            inches: default_height(kind),
            source: SourceKind::Default,
            doc: format!("usual height for a {kind}"),
        });
        let z = (level.elevation_ft + height.inches / 12.0) * upf;
        let (w, l, h) = (BOX_IN[0] * upf / 12.0, BOX_IN[1] * upf / 12.0, BOX_IN[2] * upf / 12.0);
        // A wall device (not one at the ceiling, nor on the floor) on the face of its wall that
        // faces its room, where it is along the wall, turned with it; none within reach: it stays
        // where it was read, for review.
        let wall_kind = matches!(kind, "device" | "switch" | "low voltage outlet" | "fire alarm device" | "thermostat") || on_a_wall;
        let on_wall = wall_kind && !ceiling_mount && height.inches > FLOOR_BOX_IN && height.inches < WALL_BOX_FT * 12.0 + 12.0;
        let face = on_wall.then(|| wall_face(&walls, &level.name, Vec2::new(x, y), HOST_SNAP_FT * upf)).flatten();
        let (centre, along) = match &face {
            Some(f) => (f.at + f.out * (w / 2.0), f.along),
            None => (Vec2::new(x, y), Vec2::new(1.0, 0.0)),
        };
        let (x, y) = (centre.x, centre.y);
        let mut m = super::wallgen::empty_mesh();
        super::wallgen::add_box(&mut m, centre - along * (l / 2.0), centre + along * (l / 2.0), w, z - h / 2.0, z + h / 2.0);
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
        if !mark.is_empty() {
            store.set("type", Prop::new(field(t, "device_type").unwrap_or_else(|| kind.to_string()), printed()));
            store.set("type_mark", Prop::new(mark.clone(), printed()));
        }
        // A switch's or sensor's own leg (its designation), else the fixture's after its circuit.
        if let Some(leg) = field(t, "switch_leg") {
            store.set("switch_leg", Prop::new(leg, printed()).with_note("the lower-case letter of the fixtures it controls"));
        }
        if let Some(c) = field(t, "circuit") {
            let (c, leg) = split_switch_leg(&c);
            if let Some(leg) = leg {
                store.set("switch_leg", Prop::new(leg, printed()).with_note("the lower-case letter after its circuit: the switch that controls it"));
            }
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
        // What it is: its element's kind as read (its legend row's, its sheet's), else its mark's.
        let own = t.kind.replace('_', " ");
        let what = if own.trim().is_empty() || own == "device" { field(t, "device_kind").unwrap_or_else(|| kind.to_string()) } else { own };
        let what_src = field(t, "kind_source").map_or_else(|| Source::print(&sheet, at), |row| Source::print(&row, at));
        store.set("kind", Prop::new(what.replace(' ', "_"), what_src.clone()));
        if let Some(ifc) = field(t, "ifc_class") {
            store.set("ifc_class", Prop::new(ifc, what_src));
        }
        store.set("offset", Prop::new(round2(height.inches / 12.0), hp.source.clone()).with_unit("ft"));
        if !sheet.is_empty() {
            store.set("sheet", Prop::new(sheet.clone(), Source::print(&sheet, at)));
        }
        let mut loc = location_prop(x, y, z, upf);
        if registered {
            loc.note = Some("model coordinates: read on its sheet, moved onto its level's architectural plan by the sheet frame (same sheet size), at the level's elevation plus the offset".into());
        }
        store.set("location", loc);
        match &face {
            Some(f) => {
                store.set("host_wall", Prop::new(f.wall.hex(), Source::computed()).with_note("the wall whose face it is on"));
                store.set(
                    "host_face",
                    Prop::new(format!("{:.3},{:.3}", f.out.x, f.out.y), Source::computed())
                        .with_note("the face's outward direction (plan): toward its room"),
                );
                if f.moved > 0.0 {
                    store.set(
                        "placement",
                        Prop::new(
                            format!("on its wall's face, {} from where its symbol was read", super::walltypes::frac(f.moved / upf * 12.0)),
                            Source::computed(),
                        ),
                    );
                }
            }
            None if on_wall => {
                store.set("placement", Prop::new("review: no wall within 3' of where it was read", Source::computed()));
            }
            None => {}
        }
        let xdata = props::write(&xdata, &store);
        if d.add(&Space::Model, Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            built += 1;
        }
    }
    built
}

/// The built walls' plan footprints (their meshes' outlines), with their levels and handles.
pub(crate) fn wall_footprints(d: &Drawing) -> Vec<(Option<String>, Handle, Vec<Vec2>)> {
    d.model
        .iter()
        .filter(|e| super::wallgen::made_as(e).as_deref() == Some("wall"))
        .filter_map(|e| {
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let h = super::mep::hull(&m.vertices.iter().map(|v| v.xy()).collect::<Vec<_>>());
            (h.len() >= 3).then(|| (super::bspec::made_level(e), e.handle, h))
        })
        .take(MAX_ELEMENTS)
        .collect()
}

/// A wall face a device goes on: the wall, the point on its face, the face's direction and its
/// outward normal (toward the room), and how far the device moved to it.
pub(crate) struct WallFace {
    pub(crate) wall: Handle,
    pub(crate) at: Vec2,
    pub(crate) along: Vec2,
    pub(crate) out: Vec2,
    pub(crate) moved: f64,
}

/// The face of `level`'s nearest wall within `reach` of `p`: one of each footprint's long sides
/// (not a wall's end), the point on it nearest `p`. A point inside a wall goes to the face it is
/// nearer.
pub(crate) fn wall_face(walls: &[(Option<String>, Handle, Vec<Vec2>)], level: &str, p: Vec2, reach: f64) -> Option<WallFace> {
    let mut best: Option<(f64, WallFace)> = None;
    for (_, h, poly) in walls.iter().filter(|(l, _, _)| l.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(level))) {
        let n = poly.len();
        let edges: Vec<(Vec2, Vec2)> = (0..n).filter_map(|i| Some((*poly.get(i)?, *poly.get((i + 1) % n)?))).collect();
        let longest = edges.iter().map(|(a, b)| a.dist(*b)).fold(0.0, f64::max);
        if longest <= 1e-9 || longest.is_nan() {
            continue;
        }
        let ccw = cadaddy_geom::shoelace(poly) > 0.0;
        for (a, b) in edges.iter().filter(|(a, b)| a.dist(*b) >= 0.5 * longest) {
            let ab = *b - *a;
            let len = ab.len();
            let u = ab * (1.0 / len);
            let t = ((p - *a).x * u.x + (p - *a).y * u.y).clamp(0.0, len);
            let q = *a + u * t;
            let dd = q.dist(p);
            if dd > reach || best.as_ref().is_some_and(|(bd, _)| *bd <= dd) {
                continue;
            }
            // Outward: right of the edge on a counter-clockwise outline.
            let out = if ccw { Vec2::new(u.y, -u.x) } else { Vec2::new(-u.y, u.x) };
            best = Some((dd, WallFace { wall: *h, at: q, along: u, out, moved: dd }));
        }
    }
    best.map(|(_, f)| f)
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
/// elements built before go, every other element stays as it is. A device on a level that
/// `model.build` stacked moves as that level's built objects did, and the people's overrides go
/// back on.
pub(crate) fn rebuild_devices(s: &mut Session, cmd: &str) -> Result<DeviceRebuild> {
    let (sp, _) = super::bspec::current(s, cmd)?;
    let d = s.doc_mut()?;
    // Each level's stacking move, as its built objects (not devices) carry it.
    let mut moved: HashMap<String, Vec2> = HashMap::new();
    for e in d.model.iter().filter(|e| super::wallgen::made_as(e).is_some_and(|p| p != "device")).take(MAX_ELEMENTS) {
        let Some(level) = super::bspec::made_level(e) else { continue };
        let off = super::bspec::stacked_by(e);
        if off.len() > 1e-9 {
            moved.entry(level.to_ascii_lowercase()).or_insert(off);
        }
    }
    let old: Vec<Handle> = d.model.iter().filter(|e| super::wallgen::made_as(e).as_deref() == Some("device")).map(|e| e.handle).collect();
    for h in &old {
        d.model.remove(*h);
    }
    let built = build_devices(d, &sp);
    if !moved.is_empty() {
        let new: Vec<(Handle, Vec2)> = d
            .model
            .iter()
            .filter(|e| super::wallgen::made_as(e).as_deref() == Some("device"))
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

pub(crate) trait WithNote {
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

/// The room holding `p` on `level` (any level when the room has none).
fn room_at<'a>(rooms: &'a [RoomShape], level: Option<&str>, p: Vec2) -> Option<&'a RoomShape> {
    rooms.iter().find(|r| {
        (r.level.is_none() || level.is_none() || r.level.as_deref().zip(level).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))) && r.holds(p)
    })
}

/// The room on the side of the wall `wall` (its box) a device at `p` is drawn on: a point just
/// past the wall's face on that side, else on the other (a device right on the wall's line).
fn room_by_face<'a>(rooms: &'a [RoomShape], level: Option<&str>, wall: &Bounds2, p: Vec2, upf: f64) -> Option<&'a RoomShape> {
    let (w, h) = (wall.width(), wall.height());
    if !(w.is_finite() && h.is_finite()) {
        return None;
    }
    // Across the wall: its short side.
    let (n, half, off) =
        if w >= h { (Vec2::new(0.0, 1.0), h / 2.0, p.y - wall.center().y) } else { (Vec2::new(1.0, 0.0), w / 2.0, p.x - wall.center().x) };
    let past = half + FACE_PROBE_FT * upf;
    let side = if off < 0.0 { -1.0 } else { 1.0 };
    let probe = |k: f64| room_at(rooms, level, p - n * off + n * (k * past));
    probe(side).or_else(|| probe(-side))
}

/// The room of `level` whose outline comes within `within` of `p`, the nearest.
fn room_near<'a>(rooms: &'a [RoomShape], level: Option<&str>, p: Vec2, within: f64) -> Option<&'a RoomShape> {
    rooms
        .iter()
        .filter(|r| r.level.is_none() || level.is_none() || r.level.as_deref().zip(level).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)))
        .filter(|r| dist_to(&r.bounds, p) <= within)
        .map(|r| (r, edge_dist(&r.outline, p)))
        .filter(|(_, dd)| *dd <= within)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(r, _)| r)
}

/// How far `p` is from the nearest edge of the closed `outline`.
fn edge_dist(outline: &[Vec2], p: Vec2) -> f64 {
    let n = outline.len();
    (0..n)
        .filter_map(|i| Some((*outline.get(i)?, *outline.get((i + 1) % n)?)))
        .map(|(a, b)| {
            let ab = b - a;
            let len2 = ab.x * ab.x + ab.y * ab.y;
            let t = if len2 > 0.0 { ((p - a).x * ab.x + (p - a).y * ab.y) / len2 } else { 0.0 };
            p.dist(a + ab * t.clamp(0.0, 1.0))
        })
        .fold(f64::INFINITY, f64::min)
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
/// The plaster ring each built wall device's box takes (its `mud_ring`, inches), by the id of
/// the spec device it was built from.
pub(crate) fn ring_depths(d: &Drawing) -> HashMap<String, f64> {
    d.model
        .iter()
        .filter_map(|e| {
            let id = made(e, DEVID)?;
            let depth = props::read(&e.common.xdata).get("mud_ring").map(Prop::text).and_then(|t| t.trim().parse::<f64>().ok());
            Some((id, depth.filter(|v| v.is_finite() && *v > 0.0)?))
        })
        .take(MAX_ELEMENTS)
        .collect()
}

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
pub(crate) fn fill(s: &mut Session) -> Result<Value> {
    let (sp, stored) = super::bspec::current(s, FILL)?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    // The rooms (the build's, or the plan's), in model coordinates.
    let rooms3 = super::rooms3d::rooms(s).unwrap_or_default();
    let rooms: Vec<RoomShape> = rooms3
        .iter()
        .filter(|r| r.outline.len() >= 3)
        .map(|r| RoomShape {
            bounds: Bounds2::from_points(r.outline.iter().copied()),
            outline: r.outline.clone(),
            level: r.level.clone(),
            label: room_label(r.number.as_deref(), &r.name),
        })
        .collect();
    let d = s.doc()?;
    // The plan's texts, for door numbers and airflows.
    let plan = super::bspec::plan_objects(d);
    let texts: Vec<(String, Vec2, f64)> =
        super::perceive::texts(&plan).into_iter().map(|(_, t, p, h)| (t, p, h)).filter(|(t, _, _)| !t.is_empty()).collect();
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
    let room_level = |label: &str| rooms3.iter().find(|r| room_label(r.number.as_deref(), &r.name).eq_ignore_ascii_case(label));
    let mut filled: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut changes: Vec<(Handle, Props)> = Vec::new();
    // Wall boxes on walls of no known build-up (the spec has wall types with build-ups).
    let mut untyped_boxes = 0usize;
    // Devices in no room, per level.
    let mut no_room: BTreeMap<String, usize> = BTreeMap::new();
    for (h, kind) in &work {
        let Some(e) = d.model.get(*h) else { continue };
        let mut store = props::read(&e.common.xdata);
        let before = store.clone();
        let tally = filled.entry(kind.clone()).or_default();
        let level = super::bspec::made_level(e);
        let lvl = level.as_deref();
        let ext = extent(d, e);
        let level_z = lvl.and_then(|l| sp.level(l)).map_or(0.0, |l| l.elevation_ft * upf);
        let shift = super::bspec::stacked_by(e);
        match kind.as_str() {
            "device" | "light_fixture" | "fixture" if made(e, DEVID).is_some() || kind == "device" => {
                let Some((b, z0, z1)) = ext else { continue };
                let c = b.center();
                let switch = store.get("type_mark").map(Prop::text).is_some_and(|m| kind_of(&m) == "switch");
                let reach = if switch { SWITCH_WALL_FT } else { HOST_WALL_FT };
                let host = walls
                    .iter()
                    .filter(|(_, l, _)| l.as_deref().zip(lvl).is_none_or(|(a, b)| a.eq_ignore_ascii_case(b)))
                    .map(|(wh, _, wb)| (wh, dist_to(wb, c)))
                    .filter(|(_, dd)| *dd <= reach * upf)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                // The room it stands in; a device on a wall line (a receptacle drawn on the wall)
                // the room on the face it serves, the side of the wall it is drawn on; else a room
                // within a foot of it.
                let host_bounds = host.and_then(|(wh, _)| walls.iter().find(|(w, _, _)| w == wh)).map(|(_, _, wb)| *wb);
                let served = room_at(&rooms, lvl, c)
                    .map(|r| (r, "the room the device stands in"))
                    .or_else(|| {
                        host_bounds.and_then(|wb| room_by_face(&rooms, lvl, &wb, c, upf)).map(|r| (r, "the room on the face of the wall it serves"))
                    })
                    .or_else(|| room_near(&rooms, lvl, c, ROOM_NEAR_FT * upf).map(|r| (r, "the room within a foot of it")));
                // A typical room's device the room it was stamped into (its bath is that room's).
                let stamped = made(e, DEVID)
                    .and_then(|id| sp.tags.iter().find(|t| t.id == id))
                    .and_then(|t| t.fields.get("room"))
                    .map(|f| f.value.trim().to_string())
                    .filter(|r| !r.is_empty());
                match (served, stamped) {
                    (Some((r, note)), _) => put(&mut store, "room", Prop::new(r.label.clone(), Source::computed()).with_note(note), tally),
                    (None, Some(r)) => {
                        put(&mut store, "room", Prop::new(r, Source::computed()).with_note("the typical room it was stamped into"), tally)
                    }
                    (None, None) => *no_room.entry(lvl.unwrap_or("").to_string()).or_default() += 1,
                }
                if let Some((wh, _)) = host {
                    put(&mut store, "host_wall", Prop::new(wh.hex(), Source::computed()).with_note("the nearest wall of its level"), tally);
                    // A box in the wall (below 7' off the floor): the finish on its face, from the
                    // wall's type (the legend's layers), and the plaster ring that reaches it.
                    let in_wall = (z0 - level_z) / upf < WALL_BOX_FT;
                    let wall_type =
                        d.model.get(*wh).and_then(|w| made(w, "type")).and_then(|t| sp.wall_types.iter().find(|x| x.name.eq_ignore_ascii_case(&t)));
                    // A box in a wall whose build-up nothing says: no ring can be given.
                    if in_wall && sp.wall_types.iter().any(|t| t.assembly.is_some()) && wall_type.and_then(|wt| wt.assembly.as_ref()).is_none() {
                        untyped_boxes += 1;
                    }
                    if in_wall && let Some(wt) = wall_type {
                        let room = room_at(&rooms, lvl, c).map_or("", |r| r.label.as_str());
                        if let Some((depth, words, note)) = super::walltypes::box_face(wt, room) {
                            let src = Source::from_doc(SourceKind::Spec, &format!("wall type {}", wt.name));
                            put(&mut store, "wall_type", Prop::new(wt.name.clone(), src.clone()), tally);
                            put(&mut store, "wall_finish", Prop::new(words, src.clone()).with_note("board and channels on the box's face"), tally);
                            if let Some(ring) = super::walltypes::ring_for(depth) {
                                // A wall no tag typed: its type is a guess, and the ring with it.
                                let guessed = d.model.get(*wh).and_then(|w| extent(d, w)).and_then(|(wb, _, _)| {
                                    sp.walls.iter().find(|m| {
                                        m.status() == cadaddy_walk3d::MarkStatus::Guess
                                            && m.wall_type.eq_ignore_ascii_case(&wt.name)
                                            && lvl.is_some_and(|l| l.eq_ignore_ascii_case(&m.level))
                                            && dist_to(&wb, Vec2::new((m.a[0] + m.b[0]) / 2.0, (m.a[1] + m.b[1]) / 2.0) + shift) <= upf
                                    })
                                });
                                let mut text = note.unwrap_or_else(|| format!("reaches {} of finish", super::walltypes::frac(depth)));
                                if let Some(m) = guessed {
                                    text = format!("{text}; the wall's type is a guess ({})", m.guess.as_deref().unwrap_or("no tag"));
                                }
                                put(&mut store, "mud_ring", Prop::new(round2(ring), src).with_unit("in").with_note(&text), tally);
                            }
                        }
                    }
                }
                // The panel it is fed from, as an element, and its breaker on the panel's schedule.
                let panel = store.get("panel").filter(|p| p.has_value()).map(Prop::text);
                let circuit = store.get("circuit").filter(|p| p.has_value()).map(Prop::text);
                if let Some(p) = &panel {
                    // The room's own panel of that letter (a guest room's G) before one elsewhere.
                    let own = store.get("room").filter(|r| r.has_value()).map(|r| format!("{}-{}", p.trim(), r.text().trim()).to_ascii_uppercase());
                    let key = own.filter(|k| gear.contains_key(k)).unwrap_or_else(|| p.trim().to_ascii_uppercase());
                    if let Some(gh) = gear.get(&key) {
                        put(
                            &mut store,
                            "fed_from_handle",
                            Prop::new(gh.hex(), Source::computed()).with_note("the panel the circuit tag names"),
                            tally,
                        );
                    }
                    let number = circuit.as_deref().and_then(|c| c.rsplit_once(['-', '/', '.']).map(|(_, n)| n.trim().to_string()));
                    let row = equipment.get(&key).and_then(|q| q.circuits.iter().find(|c| number.as_deref().is_some_and(|n| c.number.trim() == n)));
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
                        let kind_word = part.trim_start_matches("terminal").trim_start_matches(':');
                        if !kind_word.is_empty() {
                            put(&mut store, "type", Prop::new(kind_word.to_string(), Source::print(&sheet, None)), tally);
                        }
                        let at_plan = b.center() - shift;
                        if let Some((cfm, q)) = grid.near(at_plan, CFM_TAG_FT * upf).into_iter().find_map(|(t, q, _)| cfm_in(t).map(|c| (c, q))) {
                            put(&mut store, "cfm", Prop::new(cfm, Source::print(&sheet, Some([q.x, q.y]))).with_unit("cfm"), tally);
                        }
                        if let Some(r) = room_at(&rooms, lvl, b.center()) {
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
                        if let Some((v, unit)) = capacity_of(&rows) {
                            let sched = equipment
                                .get(&tag.to_ascii_uppercase())
                                .and_then(|q| q.sources.get("schedule").or_else(|| q.sources.values().next()).cloned())
                                .unwrap_or_else(|| "equipment schedule".into());
                            let mut p = Prop::new(v, Source::from_doc(SourceKind::Schedule, &sched));
                            if !unit.is_empty() {
                                p = p.with_unit(&unit);
                            }
                            put(&mut store, "capacity", p, tally);
                        }
                        if let Some(r) = room_at(&rooms, lvl, b.center()) {
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
    let d = s.doc_mut()?;
    let mut changed = 0usize;
    for (h, store) in changes {
        if d.modify_entity(h, |e| e.common.xdata = props::write(&e.common.xdata, &store).into()).is_ok() {
            changed += 1;
        }
    }
    filled.retain(|_, m| !m.is_empty());
    if changed > 0 {
        s.touch();
    }
    Ok(json!({ "filled": filled, "elements": changed, "spec": stored, "wall_boxes_untyped": untyped_boxes, "devices_in_no_room": no_room }))
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

fn run_fill(s: &mut Session, _p: &Value) -> Result<Value> {
    let mut r = fill(s)?;
    let devices = s.doc().map_or(0, |d| d.model.iter().filter(|e| made(e, DEVID).is_some()).count());
    r["devices"] = json!(devices);
    let n = r["elements"].as_u64().unwrap_or(0);
    s.echo(format!("Filled the properties of {n} element(s)."));
    Ok(r)
}
