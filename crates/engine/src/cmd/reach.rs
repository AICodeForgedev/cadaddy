//! Controls in an accessible room within reach (Q6555; code rule `A-REACH` in `reach.json`).
//!
//! A room the documents mark accessible (its name or number says `ADA`, `ACCESSIBLE`, `TYPE A`…,
//! or the typical unit plan its devices were copied from is titled so) has its operable parts
//! within the accessible reach range. A panel, thermostat, switch or other control there takes
//! the height its documents give it only while that height is within reach. Otherwise it is set
//! at the highest place within reach, and the design height stays beside the corrected one
//! (`design:mount_height`, `correction:mount_height`: the rule and the reason). The field then
//! installs the corrected height and the takeoff reads it, so nothing is changed silently.
//!
//! The limit is a printed accessible maximum first: a list note that names the item and gives
//! its `ADA … = 48" MAX.`, measured as the note measures (to the top, to its middle). Failing
//! that, it is the rule's reach (`max_in`, to the top of the box). A control set below the
//! rule's least reach (`min_in`, to its bottom) is raised to it. A height a person set stays and
//! is listed. A panel's spec record takes the same correction, so a rebuild and the conduit
//! routed to it follow. Nothing here panics; every scan is capped.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::{self, Prop, Source, SourceKind};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::BuildingSpec;
use serde::Deserialize;
use serde_json::{Value, json};

use super::mountlist::{self, Datum};
use super::provenance::{self, Correction};

/// Most elements looked at (hostile drawings), and most listed in a report.
const MAX_ELEMENTS: usize = 2_000_000;
const MAX_LISTED: usize = 200;
/// A height this little past its limit (inches) is at it.
const SLACK_IN: f64 = 0.05;
/// Gear standing within this of its floor stands on it (a switchboard), inches: no wall height.
const FLOOR_GEAR_IN: f64 = 6.0;
/// A note's or rule's limit outside this (inches) is no reach.
const LIMIT_RANGE: (f64, f64) = (6.0, 120.0);

/// The rule's numbers and words (`reach.json`).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub(crate) struct Rule {
    pub id: String,
    pub cite: String,
    /// The highest an operable part may be (to the top of its box) and the lowest (to its
    /// bottom), inches over the finished floor.
    pub max_in: f64,
    pub min_in: f64,
    pub rule: String,
    /// The words (or word runs) that mark a room accessible.
    pub room_words: Vec<String>,
    /// The kinds of device (spaces) and the words of a device's type that make it a control.
    pub device_kinds: Vec<String>,
    pub device_words: Vec<String>,
    /// Words that make a device no occupant's control (a disconnect, a sprinkler flow switch).
    pub except_words: Vec<String>,
    /// The kinds of equipment that are controls on a wall (a panelboard).
    pub equipment_kinds: Vec<String>,
}

impl Default for Rule {
    fn default() -> Rule {
        Rule {
            id: "A-REACH".into(),
            cite: String::new(),
            max_in: 48.0,
            min_in: 15.0,
            rule: String::new(),
            room_words: Vec::new(),
            device_kinds: Vec::new(),
            device_words: Vec::new(),
            except_words: Vec::new(),
            equipment_kinds: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
struct File {
    accessible_reach: Rule,
}

impl Rule {
    pub(crate) fn load() -> Rule {
        let mut r = serde_json::from_str::<File>(include_str!("reach.json")).map(|f| f.accessible_reach).unwrap_or_default();
        let ok = |v: f64| v.is_finite() && (LIMIT_RANGE.0..=LIMIT_RANGE.1).contains(&v);
        if !ok(r.max_in) {
            r.max_in = 48.0;
        }
        if !(r.min_in.is_finite() && r.min_in >= 0.0 && r.min_in < r.max_in) {
            r.min_in = 0.0;
        }
        r
    }

    /// The rule as an element's correction cites it.
    fn name(&self) -> String {
        if self.cite.trim().is_empty() { self.id.clone() } else { format!("{} ({})", self.id, self.cite) }
    }

    /// Whether words (a room's name and number, a typical view's title) mark a room accessible.
    pub(crate) fn accessible(&self, words: &str) -> bool {
        let toks = tokens(words);
        self.room_words.iter().take(64).any(|w| {
            let want = tokens(w);
            !want.is_empty() && toks.windows(want.len()).any(|x| x == want.as_slice())
        })
    }

    /// Whether a device of `kind` (spaces or underscores) and type words `words` is a control the
    /// rule keeps in reach.
    pub(crate) fn device_control(&self, kind: &str, words: &str) -> bool {
        let k = kind.replace('_', " ").trim().to_ascii_lowercase();
        let all = format!("{k} {words}");
        if self.except_words.iter().any(|w| has_words(&all, w)) {
            return false;
        }
        self.device_kinds.iter().any(|x| x.eq_ignore_ascii_case(&k)) || self.device_words.iter().any(|w| has_words(&all, w))
    }

    /// Whether a kind of equipment (`panel`, `load_center`) is a control on a wall.
    pub(crate) fn equipment_control(&self, kind: &str) -> bool {
        let k = kind.replace('_', " ").trim().to_ascii_lowercase();
        self.equipment_kinds.iter().any(|x| x.replace('_', " ").eq_ignore_ascii_case(&k))
    }
}

/// Upper-case words of a text (letters and digits only).
fn tokens(text: &str) -> Vec<String> {
    text.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(400).map(str::to_string).collect()
}

/// Whether `text` holds the words of `phrase` in a row.
fn has_words(text: &str, phrase: &str) -> bool {
    let (t, p) = (tokens(text), tokens(phrase));
    !p.is_empty() && t.windows(p.len()).any(|x| x == p.as_slice())
}

/// The highest an item may be in an accessible room: inches over its floor, what that is
/// measured to on it, and where it comes from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Limit {
    pub inches: f64,
    pub datum: Datum,
    pub from: String,
}

/// An item's limit: the least printed accessible maximum of the list notes naming it (`kind`,
/// type `name`, `mark`), measured as the note measures; else the rule's, to its top.
pub(crate) fn limit_for(rule: &Rule, notes: &[cadaddy_walk3d::MountNote], kind: &str, name: Option<&str>, mark: Option<&str>) -> Limit {
    notes
        .iter()
        .take(mountlist::MAX_ENTRIES)
        .filter(|n| mountlist::names_item(&n.subject, kind, name, mark))
        .filter_map(|n| {
            let inches = mountlist::ada_max(&n.text).filter(|v| (LIMIT_RANGE.0..=LIMIT_RANGE.1).contains(v))?;
            let sheet = if n.sheet.trim().is_empty() { String::new() } else { format!(" on {}", n.sheet.trim()) };
            Some(Limit {
                inches,
                datum: Datum::parse(&n.datum),
                from: format!("the accessible maximum its general notes print ({}{sheet})", n.subject.chars().take(60).collect::<String>()),
            })
        })
        .min_by(|a, b| a.inches.total_cmp(&b.inches))
        .unwrap_or_else(|| Limit {
            inches: rule.max_in,
            datum: Datum::Top,
            from: format!("{}: the accessible reach, {} in at most to the top", rule.name(), r2(rule.max_in)),
        })
}

/// One control in an accessible room read against its limit.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Measured {
    pub handle: Handle,
    pub level: String,
    pub kind: String,
    pub mark: Option<String>,
    pub room: String,
    /// Its bottom and top, inches over its floor.
    pub bottom_in: f64,
    pub top_in: f64,
    pub limit: Limit,
    /// Its equipment tag (a panel's) when it is equipment: its spec record follows.
    pub equipment: Option<String>,
    /// A person set its height: never moved by the rule.
    pub user: bool,
}

impl Measured {
    /// Its height where its limit measures it, inches over its floor.
    fn at_datum(&self) -> f64 {
        match self.limit.datum {
            Datum::Top => self.top_in,
            Datum::Bottom => self.bottom_in,
            Datum::Centre => (self.top_in + self.bottom_in) / 2.0,
        }
    }
    /// How far it must move (inches; up positive), 0 when it is within reach.
    pub(crate) fn move_in(&self, rule: &Rule) -> f64 {
        let over = self.at_datum() - self.limit.inches;
        if over > SLACK_IN {
            return -over;
        }
        let under = rule.min_in - self.bottom_in;
        // Raised to the least reach only where that keeps it under its limit.
        if rule.min_in > 0.0 && under > SLACK_IN && self.at_datum() + under <= self.limit.inches + SLACK_IN { under } else { 0.0 }
    }
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "handle": self.handle.hex(),
            "level": self.level,
            "kind": self.kind,
            "mark": self.mark,
            "room": self.room,
            "bottom_in": r2(self.bottom_in),
            "top_in": r2(self.top_in),
            "limit_in": r2(self.limit.inches),
            "limit_to": self.limit.datum.name(),
            "limit_from": self.limit.from,
        })
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// A room's floor outline where its elements stand (drawing units), its level, its label (its
/// number, else its name) and the words that say what it is: an element no room was given yet
/// (a panel before the properties are filled) finds its room here.
#[derive(Clone, Debug, Default)]
pub(crate) struct RoomArea {
    pub level: String,
    pub outline: Vec<Vec2>,
    pub label: String,
    pub words: String,
}

/// The room area holding an element of `level` at bounds `b`: its middle, else (set in a wall) a
/// point out past the face it serves (`face`, a unit normal) by up to 2 ft.
fn area_of<'a>(areas: &'a [RoomArea], level: &str, b: &cadaddy_geom::Bounds2, face: Option<Vec2>, upf: f64) -> Option<&'a RoomArea> {
    let c = b.center();
    let probes = std::iter::once(c).chain(face.into_iter().flat_map(|n| [0.5, 1.0, 2.0].map(|k| c + n * (k * upf))));
    probes.take(8).find_map(|p| {
        areas.iter().take(MAX_ELEMENTS).find(|a| a.level.trim().eq_ignore_ascii_case(level.trim()) && cadaddy_geom::point_in_polygon(&a.outline, p))
    })
}

/// The words that say what each room is, by its level (upper case) and its label (its number,
/// and its name): the room elements' names and numbers, the spec's rooms'.
fn room_words(d: &Drawing, sp: &BuildingSpec) -> HashMap<(String, String), String> {
    let mut out: HashMap<(String, String), String> = HashMap::new();
    let mut add = |level: &str, label: &str, words: &str| {
        let key = (level.trim().to_ascii_uppercase(), label.trim().to_ascii_uppercase());
        if key.1.is_empty() || out.len() >= MAX_ELEMENTS {
            return;
        }
        let w = out.entry(key).or_default();
        if w.len() < 1_000 {
            w.push(' ');
            w.push_str(words);
        }
    };
    for e in d.model.iter().filter(|e| super::wallgen::made_as(e).as_deref() == Some("room")).take(MAX_ELEMENTS) {
        let level = super::bspec::made_level(e).unwrap_or_default();
        let (number, name) = (super::bspec::room_value(e, "number").unwrap_or_default(), super::bspec::room_value(e, "name").unwrap_or_default());
        let words = format!("{name} {number}");
        add(&level, &number, &words);
        add(&level, &name, &words);
    }
    for r in sp.rooms.iter().take(MAX_ELEMENTS) {
        let level = r.level.clone().unwrap_or_default();
        let number = r.number.clone().unwrap_or_default();
        let words = format!("{} {number} {}", r.name, r.label.clone().unwrap_or_default());
        add(&level, &number, &words);
        add(&level, &r.name, &words);
        // A spec room of no level is any level's.
        if r.level.is_none() {
            add("", &number, &words);
        }
    }
    out
}

/// Every control (with `only`, those elements) in a room its documents mark accessible, against
/// its limit: wall-mounted devices of the rule's kinds and wall-hung equipment (a panelboard).
fn measure_with(d: &Drawing, sp: &BuildingSpec, only: Option<&BTreeSet<Handle>>, areas: &[RoomArea], rule: &Rule) -> Vec<Measured> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let rooms = room_words(d, sp);
    // The spec's devices by id: a device element built before its room was filled finds its room
    // and its typical view there.
    let tags: HashMap<&str, &cadaddy_walk3d::tags::ElementTag> =
        sp.tags.iter().filter(|t| t.id.starts_with("device:")).take(MAX_ELEMENTS).map(|t| (t.id.as_str(), t)).collect();
    let field = |t: Option<&&cadaddy_walk3d::tags::ElementTag>, k: &str| -> Option<String> {
        t.and_then(|t| t.fields.get(k)).map(|f| f.value.trim().to_string()).filter(|v| !v.is_empty())
    };
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS) {
        if only.is_some_and(|o| !o.contains(&e.handle)) || !matches!(e.kind, EntityKind::Mesh(_)) {
            continue;
        }
        let device = super::wallgen::made_as(e).as_deref() == Some("device");
        let equipment = !device && super::bimlook::made_tag(e, "kind") == Some("equipment");
        if !device && !equipment {
            continue;
        }
        let ps = props::read(&e.common.xdata);
        let text = |k: &str| ps.get(k).map(Prop::text).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let tag = super::bimlook::made_tag(e, super::propfill::DEVID).and_then(|id| tags.get(id));
        let (kind, mark, typical) = if device {
            if text("mounting").is_some_and(|m| m != "wall") {
                continue;
            }
            let kind = text("kind").unwrap_or_else(|| "device".into());
            let words = [text("type"), text("type_mark"), text("description"), field(tag, "fixture"), field(tag, "device_type")]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            if !rule.device_control(&kind, &words) {
                continue;
            }
            (kind, text("type_mark").or_else(|| field(tag, "type")), field(tag, "typical_view").unwrap_or_default())
        } else {
            let part = super::bimlook::made_tag(e, "part").unwrap_or("");
            let kind = part.strip_prefix("equipment:").unwrap_or(part).to_string();
            if !rule.equipment_control(&kind) {
                continue;
            }
            (kind, super::bimlook::made_tag(e, "tag").map(str::to_string), String::new())
        };
        let Some(level) = super::bspec::made_level(e).and_then(|l| sp.level(&l)) else { continue };
        let Some((b, z0, z1)) = super::propfill::extent(d, e) else { continue };
        if b.is_empty() {
            continue;
        }
        // Its room as filled, else as read, else the room area it stands in (or serves).
        let face = super::bimlook::made_prop(e, "HOST_FACE").and_then(|t| {
            let (a, c) = t.split_once(',')?;
            let v = Vec2::new(a.trim().parse().ok()?, c.trim().parse().ok()?);
            let len = v.len();
            (len.is_finite() && len > 1e-9).then(|| v * (1.0 / len))
        });
        let area = || area_of(areas, &level.name, &b, face, upf);
        let (room, area_words) = match text("room").or_else(|| field(tag, "room")) {
            Some(r) => (r, String::new()),
            None => match area() {
                Some(a) if !a.label.trim().is_empty() => (a.label.clone(), a.words.clone()),
                _ => continue,
            },
        };
        let key = |lv: &str| (lv.trim().to_ascii_uppercase(), room.trim().to_ascii_uppercase());
        let words = rooms.get(&key(&level.name)).or_else(|| rooms.get(&key(""))).map_or("", String::as_str);
        if !rule.accessible(&format!("{words} {area_words} {typical}")) {
            continue;
        }
        let floor = level.elevation_ft * upf;
        let (bottom_in, top_in) = ((z0 - floor) / upf * 12.0, (z1 - floor) / upf * 12.0);
        if !top_in.is_finite() || !bottom_in.is_finite() || top_in < bottom_in {
            continue;
        }
        // Gear on the floor (a switchboard) is no wall control; nor anything at or over its
        // storey's height (on the roof, on the floor over it).
        if equipment && bottom_in <= FLOOR_GEAR_IN {
            continue;
        }
        if level.height_ft.is_finite() && level.height_ft > 0.0 && bottom_in >= level.height_ft * 12.0 - SLACK_IN {
            continue;
        }
        let name = [text("type"), text("description")].into_iter().flatten().collect::<Vec<_>>().join(" ");
        let limit = limit_for(rule, &sp.mount_notes, &kind, Some(name.as_str()).filter(|n| !n.is_empty()), mark.as_deref());
        let user = ps.get("mount_height").is_some_and(|p| p.source.kind == SourceKind::User);
        out.push(Measured {
            handle: e.handle,
            level: level.name.clone(),
            kind,
            mark,
            room,
            bottom_in,
            top_in,
            limit,
            equipment: equipment.then(|| super::bimlook::made_tag(e, "tag").unwrap_or("").to_string()).filter(|t| !t.is_empty()),
            user,
        });
    }
    out
}

/// What [`keep_within`] did.
#[derive(Clone, Debug, Default)]
pub(crate) struct Kept {
    /// Controls in accessible rooms read against their limit.
    pub checked: usize,
    /// Set lower (their top, or where their limit measures them, at the limit) or raised to the
    /// least reach.
    pub lowered: usize,
    pub raised: usize,
    /// Out of reach but a person set their height: left, listed.
    pub user_kept: usize,
    /// The equipment (a panel's tag, its level) moved, inches (up positive), and why: its spec
    /// record follows ([`follow_records`]).
    pub record_moves: Vec<(String, String, f64, String)>,
    /// By kind, and the first ones.
    pub by_kind: BTreeMap<String, usize>,
    pub listed: Vec<Value>,
}

impl Kept {
    /// How many elements moved.
    pub(crate) fn moved(&self) -> usize {
        self.lowered + self.raised
    }
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "rule": Rule::load().id,
            "checked": self.checked,
            "lowered": self.lowered,
            "raised": self.raised,
            "user_kept": self.user_kept,
            "records": self.record_moves.len(),
            "by_kind": self.by_kind,
            "first": self.listed.iter().take(50).collect::<Vec<_>>(),
        })
    }
}

/// Every control in an accessible room (with `only`, those elements; `areas`, the rooms for an
/// element no room was given yet) out of reach set within it:
/// its element moves, its height takes the corrected value with the design value, the rule and
/// the reason beside it; a panel's move is listed for its spec record ([`follow_records`]). A
/// height a person set stays (listed).
pub(crate) fn keep_within(d: &mut Drawing, sp: &BuildingSpec, only: Option<&BTreeSet<Handle>>, areas: &[RoomArea]) -> Kept {
    let rule = Rule::load();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let all = measure_with(d, sp, only, areas, &rule);
    let mut out = Kept { checked: all.len(), ..Kept::default() };
    for m in &all {
        let dz_in = m.move_in(&rule);
        if dz_in.abs() <= SLACK_IN {
            continue;
        }
        if m.user {
            out.user_kept += 1;
            continue;
        }
        let Some(e) = d.model.get(m.handle) else { continue };
        let x = corrected_xdata(e, m, dz_in, &rule);
        let dz = dz_in / 12.0 * upf;
        let moved = d.modify_entity(m.handle, |e| {
            if let EntityKind::Mesh(mesh) = &mut e.kind {
                for v in mesh.vertices.iter_mut().take(1_000_000) {
                    v.z += dz;
                }
            }
            e.common.xdata = x.into();
        });
        if moved.is_err() {
            continue;
        }
        if dz_in < 0.0 {
            out.lowered += 1;
        } else {
            out.raised += 1;
        }
        *out.by_kind.entry(m.kind.clone()).or_default() += 1;
        if let Some(tag) = &m.equipment
            && out.record_moves.len() < MAX_ELEMENTS
        {
            out.record_moves.push((tag.clone(), m.level.clone(), dz_in, format!("{}: {}", rule.name(), reason(m, dz_in))));
        }
        if out.listed.len() < MAX_LISTED {
            let mut j = m.to_json();
            j["moved_in"] = json!(r2(dz_in));
            out.listed.push(j);
        }
    }
    out
}

/// The spec records of the equipment [`keep_within`] moved take the same move (their bottom, and
/// why beside it), so a rebuild and the conduit routed to them follow. How many changed.
pub(crate) fn follow_records(sp: &mut BuildingSpec, kept: &Kept) -> usize {
    let mut n = 0;
    for (tag, level, dz_in, why) in &kept.record_moves {
        let found = sp.mep.equipment.iter_mut().find(|r| {
            r.tag.trim().eq_ignore_ascii_case(tag.trim()) && (r.level.trim().eq_ignore_ascii_case(level.trim()) || r.level.trim().is_empty())
        });
        if let Some(r) = found {
            r.bottom_ft = ((r.bottom_ft + dz_in / 12.0) * 1e4).round() / 1e4;
            r.sources.insert("bottom".into(), why.chars().take(400).collect());
            n += 1;
        }
    }
    n
}

/// Why a control moved, as its correction says it.
fn reason(m: &Measured, dz_in: f64) -> String {
    let at = m.at_datum();
    if dz_in < 0.0 {
        format!(
            "in an accessible room ({}): measured {} it would be at {}, over {} ({}); set at {}",
            m.room,
            m.limit.datum.words(),
            ft_in(at),
            ft_in(m.limit.inches),
            m.limit.from,
            ft_in(at + dz_in)
        )
    } else {
        format!(
            "in an accessible room ({}): its bottom at {} is under the least reach; raised to {}",
            m.room,
            ft_in(m.bottom_in),
            ft_in(m.bottom_in + dz_in)
        )
    }
}

/// An element's properties with its height corrected by `dz_in` inches: `mount_height` (the
/// design value kept, the rule and the reason beside it), `offset` and `location`; one with no
/// height of its own takes its new top as it, corrected the same way.
fn corrected_xdata(e: &Entity, m: &Measured, dz_in: f64, rule: &Rule) -> Vec<cadaddy_doc::RawTag> {
    let mut ps = props::read(&e.common.xdata);
    let why = reason(m, dz_in);
    let rule_name = rule.name();
    let (old, h) = match ps.get("mount_height").cloned() {
        Some(old) => {
            let h = old.value.as_f64().or_else(|| old.text().parse().ok()).filter(|h| h.is_finite());
            (Some(old), h)
        }
        None => (None, None),
    };
    let (old, h) = match (old, h) {
        (Some(old), Some(h)) => (old, h),
        // No height of its own: its top as built is the design value.
        _ => {
            let mut p = Prop::new(r2(m.top_in), Source::from_doc(SourceKind::Computed, "its top as built")).with_unit("in");
            p.note = Some("measured to its top".into());
            ps.set("mount_height", p.clone());
            ps.set("mount_datum", Prop::new(Datum::Top.name(), Source::computed()));
            (p, m.top_in)
        }
    };
    let new = r2(h + dz_in);
    ps.set("mount_height", Prop { value: json!(new), ..old.clone() });
    let c = Correction { key: "mount_height".into(), design: old.text(), corrected: format!("{new}"), reason: why, rule: rule_name.clone() };
    provenance::correct(&mut ps, &c, old.source.clone());
    // The design height kept as the number it was (a first correction's only).
    let dkey = format!("{}mount_height", provenance::DESIGN_PREFIX);
    if let Some(kept) = ps.get(&dkey).cloned().filter(|p| p.value.is_string() && p.text().trim() == c.design.trim()) {
        ps.set(&dkey, Prop { value: json!(h), unit: old.unit.clone(), ..kept });
    }
    let src = Source::from_doc(SourceKind::Rule, &rule_name);
    if let Some(off) = ps.get("offset").cloned()
        && let Some(o) = off.value.as_f64().or_else(|| off.text().parse().ok()).filter(|o| o.is_finite())
    {
        ps.set("offset", Prop::new(r2(o + dz_in / 12.0), src).with_unit("ft"));
    }
    if let Some(loc) = ps.get("location").cloned() {
        let xyz: Vec<f64> = loc.text().split(',').filter_map(|v| v.trim().parse().ok()).collect();
        if let [x, y, z] = xyz[..] {
            ps.set("location", Prop { value: json!(format!("{x:.2}, {y:.2}, {:.2}", z + dz_in / 12.0)), ..loc });
        }
    }
    props::write(&e.common.xdata, &ps)
}

/// Inches as feet and inches (`4'-0"`, `3"`).
fn ft_in(inches: f64) -> String {
    let i = (inches * 4.0).round() / 4.0;
    if i.abs() < 12.0 {
        return super::walltypes::frac(i);
    }
    let ft = (i / 12.0).floor();
    format!("{ft:.0}'-{}", super::walltypes::frac(i - ft * 12.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(subject: &str, text: &str, datum: &str) -> cadaddy_walk3d::MountNote {
        cadaddy_walk3d::MountNote { sheet: "G-0".into(), subject: subject.into(), datum: datum.into(), text: text.into(), ..Default::default() }
    }

    #[test]
    fn the_rule_reads_its_data() {
        let r = Rule::load();
        assert_eq!((r.max_in, r.min_in), (48.0, 15.0));
        assert!(r.accessible("KING ADA 105") && r.accessible("UNIT PLAN - ACCESSIBLE SUITE") && r.accessible("TYPE A UNIT"));
        assert!(!r.accessible("DOUBLE QUEEN 204") && !r.accessible("CANADA ROOM") && !r.accessible("TYPE C UNIT"), "whole words only");
        assert!(r.device_control("thermostat", "T") && r.device_control("switch", "switch, 3-way S3") && r.device_control("device", "DIMMER"));
        assert!(!r.device_control("receptacle", "duplex receptacle"), "a receptacle is no control here");
        assert!(!r.device_control("switch", "SPRINKLER FLOW SWITCH"), "a flow switch is no occupant's control");
        assert!(!r.device_control("device", "FUSED DISCONNECT SWITCH"));
        assert!(r.equipment_control("panel") && r.equipment_control("load_center") && !r.equipment_control("switchboard"));
    }

    #[test]
    fn a_printed_accessible_maximum_comes_before_the_rule() {
        let r = Rule::load();
        let notes = vec![
            note("CONTROLS AND THERMOSTATS", "CONTROLS AND THERMOSTATS (TO TOP): ADA SIDE REACH = 46\" MAX.", "top"),
            note("SWITCHES", "SWITCHES (TO C.L.): ADA = 44\" MAX.", "centre"),
            note("RECEPTACLES", "RECEPTACLES (TO C.L.): STANDARD = 18\". ADA = 15\" MIN.", "centre"),
        ];
        let t = limit_for(&r, &notes, "thermostat", None, Some("T"));
        assert_eq!((t.inches, t.datum), (46.0, Datum::Top), "{t:?}");
        let s = limit_for(&r, &notes, "switch", Some("switch"), Some("S"));
        assert_eq!((s.inches, s.datum), (44.0, Datum::Centre), "{s:?}");
        let p = limit_for(&r, &notes, "panel", None, Some("LP-1"));
        assert_eq!((p.inches, p.datum), (48.0, Datum::Top), "no note names a panel: the rule's reach");
        assert!(p.from.starts_with("A-REACH"), "{p:?}");
    }

    #[test]
    fn a_control_out_of_reach_moves_by_its_limit() {
        let r = Rule::load();
        let m = Measured {
            handle: Handle(1),
            level: "L2".into(),
            kind: "panel".into(),
            mark: Some("LP-7".into()),
            room: "7".into(),
            bottom_in: 54.0,
            top_in: 78.0,
            limit: Limit { inches: 48.0, datum: Datum::Top, from: String::new() },
            equipment: None,
            user: false,
        };
        assert!((m.move_in(&r) + 30.0).abs() < 1e-9, "a panel topped at 6'-6\" comes down 30\"");
        let tstat = Measured { bottom_in: 46.0, top_in: 50.0, kind: "thermostat".into(), ..m.clone() };
        assert!((tstat.move_in(&r) + 2.0).abs() < 1e-9);
        let by_middle = Measured { limit: Limit { inches: 48.0, datum: Datum::Centre, from: String::new() }, ..tstat.clone() };
        assert!(by_middle.move_in(&r).abs() < 1e-9, "a limit to its middle: 48\" is within it");
        let low = Measured { bottom_in: 10.0, top_in: 14.0, ..m.clone() };
        assert!((low.move_in(&r) - 5.0).abs() < 1e-9, "raised to the least reach");
        let within = Measured { bottom_in: 40.0, top_in: 48.0, ..m };
        assert_eq!(within.move_in(&r), 0.0);
    }
}
