//! Element properties: the values a model element carries itself.
//!
//! Every element of the built model (a wall, a door, a room, a device, a duct…) holds one
//! property store: an ordered map of keys to [`Prop`]s — a value, its unit, where it came from
//! ([`Source`]), how sure the reader was and a note. Takeoff, schedules and checks group and
//! filter by these; a person's edit (source `user`) is kept through a rebuild of the model.
//!
//! The store is kept in the element's XDATA under the application [`APP`]: its JSON text in
//! `1000` strings short enough for the 255-byte group limit (every character outside ASCII and
//! every backslash and space written as a `\uXXXX` escape). So COPY carries it, and `.cdy`, DXF and DWG keep
//! it like any other extended data.
//!
//! Each kind of element has a standard property set ([`standard_keys`]) with stable keys; other
//! keys are allowed.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::RawTag;

/// The XDATA application name of an element's property store.
pub const APP: &str = "CADADDY_PROPS";
/// Most properties one element keeps (hostile files and calls).
pub const MAX_PROPS: usize = 256;
/// Longest key, in characters.
pub const MAX_KEY: usize = 64;
/// Longest text value, note or document name, in characters.
pub const MAX_TEXT: usize = 1000;
/// Most bytes of JSON one store is read from.
const MAX_JSON: usize = 256 * 1024;
/// Characters of JSON per `1000` string.
const CHUNK: usize = 200;

/// Where a property's value came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// Read from a sheet (the sheet and where on it).
    Print,
    /// Read from a symbol legend.
    Legend,
    /// Read from a schedule.
    Schedule,
    /// Read from the specifications.
    Spec,
    /// Read from an approved submittal (the product data sheet for the type).
    Submittal,
    /// Read from a general or keyed note.
    Note,
    /// Assumed from a rule of a standard or the project rules — an assumption, always marked.
    Rule,
    /// A default for the kind of element — an assumption, always marked.
    #[default]
    Default,
    /// Edited by a person (or an agent acting for one).
    User,
    /// Computed from the model (an area, a length, the room an element stands in).
    Computed,
}

impl SourceKind {
    pub const ALL: [SourceKind; 10] = [
        SourceKind::Print,
        SourceKind::Legend,
        SourceKind::Schedule,
        SourceKind::Spec,
        SourceKind::Submittal,
        SourceKind::Note,
        SourceKind::Rule,
        SourceKind::Default,
        SourceKind::User,
        SourceKind::Computed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Print => "print",
            SourceKind::Legend => "legend",
            SourceKind::Schedule => "schedule",
            SourceKind::Spec => "spec",
            SourceKind::Submittal => "submittal",
            SourceKind::Note => "note",
            SourceKind::Rule => "rule",
            SourceKind::Default => "default",
            SourceKind::User => "user",
            SourceKind::Computed => "computed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let t = s.trim();
        SourceKind::ALL.into_iter().find(|k| k.as_str().eq_ignore_ascii_case(t))
    }

    /// Whether a value from this source is an assumption rather than something read or edited.
    pub fn is_assumed(self) -> bool {
        matches!(self, SourceKind::Rule | SourceKind::Default)
    }
}

/// Where a property's value came from: the kind of source and, when it was read, the document
/// (a sheet number, a schedule's or spec section's name) and where on the sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Where on the sheet (sheet coordinates), for a value read from a print.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
}

impl Source {
    pub fn of(kind: SourceKind) -> Self {
        Source { kind, doc: None, at: None }
    }
    pub fn user() -> Self {
        Source::of(SourceKind::User)
    }
    pub fn computed() -> Self {
        Source::of(SourceKind::Computed)
    }
    /// Read from sheet `sheet` at `at`.
    pub fn print(sheet: &str, at: Option<[f64; 2]>) -> Self {
        Source {
            kind: SourceKind::Print,
            doc: Some(cap(sheet, MAX_TEXT)).filter(|s| !s.is_empty()),
            at: at.filter(|a| a.iter().all(|v| v.is_finite())),
        }
    }
    /// Read from document `doc` (a legend, schedule, spec section or note).
    pub fn from_doc(kind: SourceKind, doc: &str) -> Self {
        Source { kind, doc: Some(cap(doc, MAX_TEXT)).filter(|s| !s.is_empty()), at: None }
    }
}

/// One property of an element.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prop {
    /// A string, number or boolean.
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub source: Source,
    /// How sure the reader was, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Prop {
    pub fn new(value: impl Into<Value>, source: Source) -> Self {
        Prop { value: value.into(), unit: None, source, confidence: None, note: None }
    }
    pub fn with_unit(mut self, unit: &str) -> Self {
        self.unit = Some(cap(unit, MAX_KEY)).filter(|u| !u.is_empty());
        self
    }

    /// Whether the value says anything (not null, not an empty string).
    pub fn has_value(&self) -> bool {
        match &self.value {
            Value::Null => false,
            Value::String(s) => !s.trim().is_empty(),
            _ => true,
        }
    }

    /// The value as text: a string as it is, a number or boolean as JSON writes it.
    pub fn text(&self) -> String {
        match &self.value {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        }
    }

    /// The property made safe to keep: a scalar value, text, keys and numbers capped.
    pub fn sanitized(mut self) -> Option<Self> {
        self.value = match self.value {
            Value::String(s) => Value::String(cap(&s, MAX_TEXT)),
            Value::Number(n) if n.as_f64().is_some_and(f64::is_finite) => Value::Number(n),
            Value::Bool(b) => Value::Bool(b),
            Value::Null => Value::Null,
            _ => return None,
        };
        self.unit = self.unit.map(|u| cap(&u, MAX_KEY)).filter(|u| !u.is_empty());
        self.note = self.note.map(|n| cap(&n, MAX_TEXT)).filter(|n| !n.is_empty());
        self.confidence = self.confidence.filter(|c| c.is_finite()).map(|c| c.clamp(0.0, 1.0));
        self.source.doc = self.source.doc.map(|d| cap(&d, MAX_TEXT)).filter(|d| !d.is_empty());
        self.source.at = self.source.at.filter(|a| a.iter().all(|v| v.is_finite()));
        Some(self)
    }
}

/// One stored entry: a key and its property.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    #[serde(flatten)]
    pub prop: Prop,
}

/// An element's properties, in the order they were set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Props {
    items: Vec<(String, Prop)>,
}

impl Props {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Prop)> {
        self.items.iter().map(|(k, p)| (k.as_str(), p))
    }
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(|(k, _)| k.as_str())
    }
    /// The property `key` (keys match without regard to case).
    pub fn get(&self, key: &str) -> Option<&Prop> {
        self.items.iter().find(|(k, _)| k.eq_ignore_ascii_case(key.trim())).map(|(_, p)| p)
    }
    /// Set `key` (a new key goes last; an existing one keeps its place). False when the key is
    /// empty, the value isn't a scalar, or the store is full.
    pub fn set(&mut self, key: &str, prop: Prop) -> bool {
        let key = cap(key.trim(), MAX_KEY);
        let Some(prop) = prop.sanitized() else { return false };
        if key.is_empty() {
            return false;
        }
        if let Some(slot) = self.items.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            slot.1 = prop;
            return true;
        }
        if self.items.len() >= MAX_PROPS {
            return false;
        }
        self.items.push((key, prop));
        true
    }
    /// Set `key` only when it has no value yet. True when it was set.
    pub fn set_if_missing(&mut self, key: &str, prop: Prop) -> bool {
        if self.get(key).is_some_and(Prop::has_value) {
            return false;
        }
        self.set(key, prop)
    }
    pub fn remove(&mut self, key: &str) -> Option<Prop> {
        let i = self.items.iter().position(|(k, _)| k.eq_ignore_ascii_case(key.trim()))?;
        Some(self.items.remove(i).1)
    }
    /// The properties a person edited (source `user`).
    pub fn user_edits(&self) -> Props {
        Props { items: self.items.iter().filter(|(_, p)| p.source.kind == SourceKind::User).cloned().collect() }
    }

    /// The store as JSON: `{key: {value, unit?, source: {kind, doc?, at?}, confidence?, note?}}`
    /// in order (serde_json's map is sorted, so the order is also given as `order`).
    pub fn to_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        for (k, p) in &self.items {
            m.insert(k.clone(), serde_json::to_value(p).unwrap_or(Value::Null));
        }
        Value::Object(m)
    }

    /// The store's keys in order.
    pub fn order(&self) -> Vec<&str> {
        self.keys().collect()
    }

    fn to_text(&self) -> Option<String> {
        let entries: Vec<Entry> = self.items.iter().map(|(key, prop)| Entry { key: key.clone(), prop: prop.clone() }).collect();
        serde_json::to_string(&entries).ok()
    }

    fn from_text(text: &str) -> Props {
        let Ok(entries) = serde_json::from_str::<Vec<Entry>>(text) else { return Props::default() };
        let mut p = Props::default();
        for e in entries.into_iter().take(MAX_PROPS) {
            p.set(&e.key, e.prop);
        }
        p
    }
}

fn cap(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The range of `xdata` that is the store's: its `1001` group up to the next application's.
fn app_range(xdata: &[RawTag]) -> Option<std::ops::Range<usize>> {
    let start = xdata.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(APP))?;
    let len = xdata.get(start + 1..)?.iter().take_while(|t| t.code != 1001).count();
    Some(start..start + 1 + len)
}

/// The properties an element's XDATA holds (none: an empty store).
pub fn read(xdata: &[RawTag]) -> Props {
    let Some(r) = app_range(xdata) else { return Props::default() };
    let mut text = String::new();
    for t in xdata.get(r.start + 1..r.end).unwrap_or_default().iter().filter(|t| t.code == 1000) {
        if text.len() + t.value.len() > MAX_JSON {
            return Props::default();
        }
        text.push_str(&t.value);
    }
    Props::from_text(&text)
}

/// Whether an element's XDATA holds a property store.
pub fn has_store(xdata: &[RawTag]) -> bool {
    app_range(xdata).is_some()
}

/// `xdata` with its property store replaced by `props` (an empty store: none at all). Other
/// applications' data stays where it is.
pub fn write(xdata: &[RawTag], props: &Props) -> Vec<RawTag> {
    let mut out: Vec<RawTag> = xdata.to_vec();
    let at = app_range(&out).map(|r| {
        out.drain(r.clone());
        r.start
    });
    if props.is_empty() {
        return out;
    }
    let Some(text) = props.to_text() else { return out };
    let mut groups = vec![RawTag { code: 1001, value: APP.into() }];
    groups.extend(chunks(&text).into_iter().map(|value| RawTag { code: 1000, value }));
    let at = at.unwrap_or(out.len()).min(out.len());
    out.splice(at..at, groups);
    out
}

/// JSON text cut into `1000` strings: ASCII only (other characters and backslashes as `\uXXXX`
/// escapes, which JSON reads back as they were), at most [`CHUNK`] characters each.
fn chunks(json: &str) -> Vec<String> {
    let mut safe = String::with_capacity(json.len());
    let mut it = json.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            // An escape JSON wrote: keep it, except an escaped backslash (DXF reads `\U+` as a
            // character code), written as its code.
            match it.next() {
                Some('\\') => safe.push_str("\\u005c"),
                Some(n) => {
                    safe.push('\\');
                    safe.push(n);
                }
                None => {}
            }
        } else if c == ' ' {
            // A space could end a group, and readers may trim it.
            safe.push_str("\\u0020");
        } else if c.is_ascii() {
            safe.push(c);
        } else {
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                safe.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    let bytes = safe.as_bytes();
    bytes.chunks(CHUNK).map(|c| String::from_utf8_lossy(c).into_owned()).collect()
}

/// The kinds of element with a standard property set, and their keys (stable: takeoff, schedules,
/// checks and IFC export read them by name).
pub const STANDARD: &[(&str, &[&str])] = &[
    ("device", &["type", "type_mark", "mount_height", "circuit", "panel", "voltage", "room", "level", "host_wall", "sheet"]),
    ("door", &["number", "width", "height", "frame", "hardware_set", "fire_rating", "swing", "from_room", "to_room", "level"]),
    ("window", &["number", "type_mark", "width", "height", "sill_height", "fire_rating", "room", "level"]),
    ("room", &["number", "name", "area", "ceiling_height", "floor_finish", "wall_finish", "ceiling_finish", "level"]),
    ("wall", &["type", "assembly", "height", "fire_rating", "finish_a", "finish_b", "room_a", "room_b", "level"]),
    ("floor", &["type", "assembly", "thickness", "finish", "area", "level"]),
    ("ceiling", &["type", "height", "finish", "area", "room", "level"]),
    ("roof", &["type", "assembly", "slope", "area", "level"]),
    ("duct", &["size", "system", "shape", "insulation", "elevation", "level"]),
    ("pipe", &["size", "system", "material", "insulation", "elevation", "level"]),
    ("conduit", &["size", "system", "circuit", "elevation", "level"]),
    ("fixture", &["type_mark", "model", "circuit", "room", "level"]),
    ("light_fixture", &["type_mark", "model", "circuit", "mount_height", "room", "level"]),
    ("equipment", &["type_mark", "model", "circuit", "room", "level"]),
];

/// The standard keys of element kind `kind` (none for a kind without a standard set).
pub fn standard_keys(kind: &str) -> &'static [&'static str] {
    STANDARD.iter().find(|(k, _)| k.eq_ignore_ascii_case(kind.trim())).map_or(&[], |(_, keys)| keys)
}

/// The element kind an IFC class (`Wall`, `IfcDuctSegment`…) is, for properties read from IFC.
pub fn kind_of_ifc_class(class: &str) -> &'static str {
    let c = class.trim();
    let c = c.get(..3).filter(|p| p.eq_ignore_ascii_case("ifc")).map_or(c, |_| c.get(3..).unwrap_or(""));
    let c = c.to_ascii_lowercase();
    let starts = |xs: &[&str]| xs.iter().any(|x| c.starts_with(x));
    if starts(&["wall", "curtainwall"]) {
        "wall"
    } else if c.starts_with("door") {
        "door"
    } else if c.starts_with("window") {
        "window"
    } else if c.starts_with("space") {
        "room"
    } else if c.starts_with("slab") {
        "floor"
    } else if c.starts_with("covering") {
        "ceiling"
    } else if c.starts_with("roof") {
        "roof"
    } else if starts(&["duct", "damper"]) {
        "duct"
    } else if starts(&["pipe", "valve"]) {
        "pipe"
    } else if starts(&["cablecarrier", "cablesegment", "cablefitting", "conduit"]) {
        "conduit"
    } else if c.starts_with("lightfixture") || c.starts_with("lamp") {
        "light_fixture"
    } else if starts(&["sanitaryterminal", "airterminal", "firesuppressionterminal", "wasteterminal", "stackterminal"]) {
        "fixture"
    } else if starts(&[
        "outlet",
        "switchingdevice",
        "alarm",
        "sensor",
        "communicationsappliance",
        "audiovisualappliance",
        "junctionbox",
        "protectivedevice",
    ]) {
        "device"
    } else if starts(&[
        "electricdistributionboard",
        "electricappliance",
        "pump",
        "fan",
        "boiler",
        "chiller",
        "unitaryequipment",
        "airtoairheatrecovery",
        "coil",
        "tank",
        "transformer",
        "electricgenerator",
        "motor",
        "coolingtower",
        "heatexchanger",
        "spaceheater",
        "flowmeter",
    ]) {
        "equipment"
    } else {
        "element"
    }
}

/// The standard IFC property set and property name a key of an element kind is written as (and
/// read back from), where one exists.
pub fn standard_ifc(kind: &str, key: &str) -> Option<(&'static str, &'static str)> {
    let k = key.trim().to_ascii_lowercase();
    Some(match (kind, k.as_str()) {
        ("wall", "type") => ("Pset_WallCommon", "Reference"),
        ("wall", "fire_rating") => ("Pset_WallCommon", "FireRating"),
        ("door", "fire_rating") => ("Pset_DoorCommon", "FireRating"),
        ("door", "hardware_set") => ("Pset_DoorCommon", "Reference"),
        ("window", "fire_rating") => ("Pset_WindowCommon", "FireRating"),
        ("window", "type_mark") => ("Pset_WindowCommon", "Reference"),
        ("room", "number") => ("Pset_SpaceCommon", "Reference"),
        ("room", "floor_finish") => ("Pset_SpaceCoveringRequirements", "FloorCovering"),
        ("room", "wall_finish") => ("Pset_SpaceCoveringRequirements", "WallCovering"),
        ("room", "ceiling_finish") => ("Pset_SpaceCoveringRequirements", "CeilingCovering"),
        ("light_fixture" | "fixture" | "equipment" | "device", "type_mark") => ("Pset_ManufacturerTypeInformation", "ModelLabel"),
        ("light_fixture" | "fixture" | "equipment", "model") => ("Pset_ManufacturerTypeInformation", "ModelReference"),
        _ => return None,
    })
}

/// The key a standard IFC property of an element kind is read back as.
pub fn key_of_standard_ifc(kind: &str, set: &str, name: &str) -> Option<&'static str> {
    standard_keys(kind)
        .iter()
        .copied()
        .find(|k| standard_ifc(kind, k).is_some_and(|(s, n)| s.eq_ignore_ascii_case(set) && n.eq_ignore_ascii_case(name)))
}

/// The property set CADaddy writes every element's properties to in IFC.
pub const IFC_PSET: &str = "Pset_CADaddy";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Props {
        let mut p = Props::default();
        assert!(p.set("type", Prop::new("duplex", Source::from_doc(SourceKind::Legend, "E-001"))));
        assert!(p.set("mount_height", Prop::new(18, Source::of(SourceKind::Default)).with_unit("in")));
        assert!(p.set("circuit", Prop::new("G-12", Source::print("E-101", Some([120.5, 40.0])))));
        assert!(p.set("room", Prop::new("131", Source::computed())));
        p
    }

    #[test]
    fn props_round_trip_through_xdata_in_order() {
        let p = sample();
        let other = vec![RawTag { code: 1001, value: "OTHER".into() }, RawTag { code: 1000, value: "keep".into() }];
        let x = write(&other, &p);
        assert_eq!(x.first(), other.first(), "other applications stay first");
        assert!(x.iter().all(|t| t.value.len() <= 255 && t.value.is_ascii()), "{x:?}");
        let back = read(&x);
        assert_eq!(back, p);
        assert_eq!(back.order(), vec!["type", "mount_height", "circuit", "room"]);
        // Writing again replaces, never duplicates.
        let mut q = back.clone();
        q.remove("room");
        let y = write(&x, &q);
        assert_eq!(y.iter().filter(|t| t.code == 1001 && t.value == APP).count(), 1);
        assert_eq!(read(&y).len(), 3);
        assert!(y.iter().any(|t| t.value == "keep"));
        // An empty store is no store.
        assert!(!has_store(&write(&y, &Props::default())));
    }

    #[test]
    fn long_and_unicode_values_survive_the_group_limit() {
        let mut p = Props::default();
        let long = "Ü\\é \"quoted\" \\U+0041 ".repeat(40);
        assert!(p.set("note", Prop::new(long.clone(), Source::user())));
        let x = write(&[], &p);
        assert!(x.len() > 3, "chunked: {}", x.len());
        assert!(x.iter().all(|t| t.value.len() <= 255 && t.value.is_ascii() && !t.value.contains("\\U+")));
        assert_eq!(read(&x).get("NOTE").map(Prop::text), Some(cap(&long, MAX_TEXT)));
    }

    #[test]
    fn hostile_stores_and_values_are_refused_or_capped() {
        let mut p = Props::default();
        assert!(!p.set("", Prop::new("x", Source::user())));
        assert!(!p.set("obj", Prop::new(json!({"a": 1}), Source::user())));
        assert!(!p.set("list", Prop::new(json!([1, 2]), Source::user())));
        assert!(p.set(&"k".repeat(500), Prop::new("x", Source::user())));
        assert!(p.keys().all(|k| k.chars().count() <= MAX_KEY));
        let mut c = Prop::new(1.5, Source::user());
        c.confidence = Some(7.0);
        assert!(p.set("c", c));
        assert_eq!(p.get("c").and_then(|c| c.confidence), Some(1.0));
        for i in 0..(MAX_PROPS + 10) {
            p.set(&format!("k{i}"), Prop::new(i, Source::user()));
        }
        assert_eq!(p.len(), MAX_PROPS);
        // Garbage in the XDATA reads as an empty store.
        let bad = vec![RawTag { code: 1001, value: APP.into() }, RawTag { code: 1000, value: "[{\"key\": ".into() }];
        assert!(read(&bad).is_empty());
        let huge: Vec<RawTag> = std::iter::once(RawTag { code: 1001, value: APP.into() })
            .chain((0..3000).map(|_| RawTag { code: 1000, value: "x".repeat(200) }))
            .collect();
        assert!(read(&huge).is_empty());
    }

    #[test]
    fn standard_sets_and_ifc_names() {
        assert_eq!(
            standard_keys("device"),
            ["type", "type_mark", "mount_height", "circuit", "panel", "voltage", "room", "level", "host_wall", "sheet"]
        );
        assert!(standard_keys("door").contains(&"hardware_set"));
        assert!(standard_keys("nothing").is_empty());
        assert_eq!(standard_ifc("wall", "fire_rating"), Some(("Pset_WallCommon", "FireRating")));
        assert_eq!(key_of_standard_ifc("room", "Pset_SpaceCoveringRequirements", "FloorCovering"), Some("floor_finish"));
        assert_eq!(kind_of_ifc_class("IfcDuctSegment"), "duct");
        assert_eq!(kind_of_ifc_class("Space"), "room");
        assert_eq!(kind_of_ifc_class("WallStandardCase"), "wall");
        assert_eq!(kind_of_ifc_class("Outlet"), "device");
        assert_eq!(kind_of_ifc_class("BuildingElementProxy"), "element");
        assert!(SourceKind::Default.is_assumed() && !SourceKind::Print.is_assumed());
        assert_eq!(SourceKind::parse("Schedule"), Some(SourceKind::Schedule));
    }
}
