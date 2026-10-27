//! Tag every electrical element from the documents (Q299): what each device, luminaire,
//! homerun and conduit is, with where that came from, and what is unresolved.
//!
//! - **Luminaires**: the type tag on the plan → its row in the light fixture schedule. Issues:
//!   `untagged`, `tag_not_in_schedule`.
//! - **Devices**: subscripts beside the symbol → the device type (GFCI, WR, USB, IG, DED, TV,
//!   clock hanger; none: standard duplex). The circuit beside it (`P1-5`).
//! - **Protection is per circuit**: each breaker's type from the panel schedules (GFCI, AFCI,
//!   DF, GFEP). A device on a GFCI breaker's circuit is protected at the breaker and stays a
//!   standard device unless the drawings say otherwise. A wet location's receptacle with no
//!   protection is `missing_protection`; a GFCI device on a GFCI breaker is
//!   `double_protection` (wasted cost).
//! - **Homeruns**: the conductors from the note under the label (`2#12, 1#12G`, `3#8 THHN + #10G
//!   in 3/4" EMT`, `SER 2-2-2-4 AL`); without a note, from the breaker (`rule`) and
//!   `no_conductor_note`.
//! - **Conduit**: the wiring method from the project's rules by location (above a ceiling:
//!   `concealed`; in a room open to structure: `exposed`). A rule naming two methods is
//!   `two_rules_apply`.
//!
//! Every field records its source: `plan`, `schedule`, `note`, `rule`, `default`. The result
//! goes into the building spec ([`spec_patch`]) for review (`tags.list`, `tags.set`).

use std::collections::BTreeMap;

use crate::arch_reader::ReadTable;
use crate::conduit_reader::ConduitResult;
use crate::elec_reader::FoundDevice;
use crate::elecspec;
use crate::truth::Truth;
use cadaddy_doc::projrules::{ProjectRules, RuleDomain, builtin_wiring_method};

pub use cadaddy_walk3d::tags::{ElementTag, Field};

/// What the schedules say: luminaire types (tag → description) and breaker types by panel
/// tag and circuit.
#[derive(Clone, Debug, Default)]
pub struct Schedules {
    pub luminaires: BTreeMap<String, String>,
    pub breakers: BTreeMap<(String, String), String>,
    /// What each circuit serves, as the panel schedule describes it (`RECEP/DATA - OFFICE`).
    pub circuits: BTreeMap<(String, String), String>,
    /// The tags in the electrical equipment schedule (panels, disconnects, transformers): a symbol
    /// tagged with one is gear, not a luminaire.
    pub gear: std::collections::BTreeSet<String>,
}

/// The schedules from the electrical schedule sheet's tables (as [`crate::arch_reader`]
/// reads them: title, header, rows).
pub fn schedules_from(tables: &[ReadTable]) -> Schedules {
    let mut s = Schedules::default();
    for t in tables {
        let title = t.title.trim().to_ascii_uppercase();
        let col = |names: &[&str]| t.header.iter().position(|h| names.iter().any(|n| h.trim().eq_ignore_ascii_case(n)));
        let cell = |r: &Vec<String>, i: Option<usize>| i.and_then(|i| r.get(i)).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        if title.contains("FIXTURE SCHEDULE") || title.contains("LUMINAIRE SCHEDULE") {
            let (tag, desc) = (col(&["TYPE", "MARK"]), col(&["DESCRIPTION"]));
            for r in &t.rows {
                if let Some(tag) = cell(r, tag) {
                    s.luminaires.insert(tag, cell(r, desc).unwrap_or_default());
                }
            }
        } else if title.contains("EQUIPMENT SCHEDULE") {
            let tag = col(&["TAG"]);
            s.gear.extend(t.rows.iter().filter_map(|r| cell(r, tag)));
        } else if let Some(panel) = title.strip_prefix("PANEL ").and_then(|x| x.strip_suffix(" SCHEDULE")) {
            let (ckt, kind, desc) = (col(&["CKT", "CIRCUIT"]), col(&["TYPE"]), col(&["DESCRIPTION"]));
            for r in &t.rows {
                if let (Some(c), Some(d)) = (cell(r, ckt), cell(r, desc)) {
                    s.circuits.insert((panel.trim().to_string(), c), d);
                }
                let k = cell(r, kind).map(|k| k.to_ascii_uppercase()).filter(|k| k != "-");
                if let (Some(c), Some(k)) = (cell(r, ckt), k) {
                    s.breakers.insert((panel.trim().to_string(), c), k);
                }
            }
        }
    }
    s
}

/// What a set of subscripts makes a receptacle.
pub fn device_type(subscripts: &[String]) -> &'static str {
    let has = |w: &str| subscripts.iter().any(|s| s.eq_ignore_ascii_case(w));
    if has("GFCI") || has("GFI") {
        "GFCI receptacle"
    } else if has("WR") || has("WP") {
        "weather-resistant receptacle"
    } else if has("USB") {
        "USB receptacle"
    } else if has("IG") {
        "isolated-ground receptacle"
    } else if has("DED") {
        "dedicated receptacle"
    } else if has("TV") {
        "TV receptacle"
    } else if has("CH") {
        "clock hanger receptacle"
    } else {
        "standard duplex receptacle"
    }
}

/// Conductors as printed → a canonical make-up: `2#12 + 1#12G`, `3#8 THHN + 1#10G in 3/4" EMT`,
/// `SER 2-2-2-4 AL`. None for text that isn't a conductor note.
pub fn parse_conductors(note: &str) -> Option<String> {
    let u = note.trim().to_ascii_uppercase();
    if u.len() > 80 {
        return None;
    }
    if let Some(rest) = u.strip_prefix("SER ") {
        let sizes = rest.split_whitespace().next()?;
        let ok = sizes.split('-').count() >= 3 && sizes.split('-').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit() || c == '/'));
        let al = if rest.contains("AL") { " AL" } else { " CU" };
        return ok.then(|| format!("SER {sizes}{al}"));
    }
    let (wires, raceway) = match u.split_once(" IN ") {
        Some((w, r)) => (w, Some(r.trim().to_string())),
        None => (u.as_str(), None),
    };
    let mut parts: Vec<String> = Vec::new();
    for p in wires.split([',', '+']) {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let (count, rest) = p.split_once('#')?;
        let count = if count.trim().is_empty() { 1 } else { count.trim().parse::<u32>().ok()? };
        let size: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
        if size.is_empty() || count == 0 || count > 12 {
            return None;
        }
        let after = rest.get(size.len()..).unwrap_or_default().trim();
        let ground = after.starts_with('G');
        let insul = after.trim_start_matches('G').trim();
        let insul = if insul.is_empty() { String::new() } else { format!(" {insul}") };
        parts.push(format!("{count}#{size}{}{insul}", if ground { "G" } else { "" }));
    }
    if parts.is_empty() {
        return None;
    }
    Some(match raceway {
        Some(r) => format!("{} in {r}", parts.join(" + ")),
        None => parts.join(" + "),
    })
}

/// The wiring method for a location: the project's rule, else the built-in one.
pub fn wiring_method(rules: &ProjectRules, location: &str) -> Option<(String, &'static str)> {
    if let Some(v) = rules.resolve(RuleDomain::WiringMethod, location) {
        return Some((v.value.clone(), "rule"));
    }
    builtin_wiring_method(location).map(|v| (v.to_string(), "rule"))
}

/// A rule value naming more than one method (`MC cable or EMT`).
fn two_methods(v: &str) -> bool {
    v.to_ascii_lowercase().contains(" or ")
}

/// The smallest room holding a point (a reader can also report a box around a whole floor; a
/// run along a corridor is in the corridor, not in that).
pub fn room_at<'a, R>(rooms: &'a [R], p: [f64; 2], bounds: &dyn Fn(&R) -> ([f64; 2], [f64; 2])) -> Option<&'a R> {
    rooms
        .iter()
        .filter_map(|r| {
            let (lo, hi) = bounds(r);
            let inside = p[0] >= lo[0] - 1e-9 && p[0] <= hi[0] + 1e-9 && p[1] >= lo[1] - 1e-9 && p[1] <= hi[1] + 1e-9;
            inside.then_some((r, (hi[0] - lo[0]) * (hi[1] - lo[1])))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|x| x.0)
}

/// Whether the room (by name) is a wet location.
fn wet(room: &str) -> bool {
    elecspec::wet_room(room)
}

/// The panel tag and circuit of a `P1-5` reference.
fn split_ref(r: &str) -> Option<(String, String)> {
    let (p, c) = r.trim().split_once('-')?;
    Some((p.to_string(), c.to_string()))
}

/// Tag what the readers found. `locations(level, at)` says where a point is
/// (`concealed`/`exposed`), from the ceilings.
pub fn tag(
    devices: &[FoundDevice],
    runs: &ConduitResult,
    schedules: &Schedules,
    rules: &ProjectRules,
    locations: &dyn Fn(&str, [f64; 2]) -> &'static str,
) -> Vec<ElementTag> {
    let mut out = Vec::new();
    for d in devices {
        if d.kind == "light_fixture" && schedules.gear.contains(&d.type_tag) {
            continue;
        }
        let mut e = ElementTag {
            id: format!("device:{}:{:.1},{:.1}", d.level, d.drawn_ft[0], d.drawn_ft[1]),
            kind: if d.kind == "light_fixture" { "luminaire".into() } else { d.kind.clone() },
            level: d.level.clone(),
            at_ft: d.drawn_ft,
            ..ElementTag::default()
        };
        let subs: Vec<String> = d
            .notes
            .iter()
            .filter(|n| crate::elec_reader::is_subscript_or_circuit(n) && !n.contains('-'))
            .flat_map(|n| n.split_whitespace().map(str::to_string).collect::<Vec<_>>())
            .collect();
        let circuit = d.notes.iter().find(|n| n.contains('-') && crate::elec_reader::is_subscript_or_circuit(n)).cloned();
        if d.kind == "light_fixture" {
            if d.type_tag.is_empty() {
                e.issues.push("untagged".into());
            } else {
                e.set("type", d.type_tag.clone(), "plan");
                match schedules.luminaires.get(&d.type_tag) {
                    Some(desc) => e.set("luminaire", desc.clone(), "schedule"),
                    None => e.issues.push("tag_not_in_schedule".into()),
                }
            }
        }
        if d.kind == "receptacle" {
            let device_gfci = subs.iter().any(|s| s.eq_ignore_ascii_case("GFCI") || s.eq_ignore_ascii_case("GFI"));
            e.set("device_type", device_type(&subs), if subs.is_empty() { "default" } else { "plan" });
            let breaker = circuit.as_deref().and_then(split_ref).and_then(|k| schedules.breakers.get(&k).cloned());
            if let Some(c) = &circuit {
                e.set("circuit", c.clone(), "plan");
            }
            let breaker_gf = breaker.as_deref().is_some_and(|b| matches!(b, "GFCI" | "DF" | "GFEP"));
            let protection = match (breaker_gf, device_gfci) {
                (true, true) => "GFCI breaker and GFCI device",
                (true, false) => "GFCI breaker",
                (false, true) => "GFCI device",
                (false, false) => "none",
            };
            e.set(
                "protection",
                protection,
                if breaker.is_some() {
                    "schedule"
                } else if device_gfci {
                    "plan"
                } else {
                    "default"
                },
            );
            if breaker_gf && device_gfci {
                e.issues.push("double_protection".into());
            }
            // Where it is: the room its circuit serves (the schedule), else where it's drawn.
            let served = circuit.as_deref().and_then(split_ref).and_then(|k| schedules.circuits.get(&k).cloned());
            if !breaker_gf && !device_gfci && served.as_deref().map_or_else(|| wet(&d.room), wet) {
                e.issues.push("missing_protection".into());
            }
        }
        out.push(e);
    }
    for h in &runs.homeruns {
        let mut e = ElementTag {
            id: format!("homerun:{}:{}", h.level, h.label),
            kind: "homerun".into(),
            level: h.level.clone(),
            at_ft: h.origin_ft,
            ..ElementTag::default()
        };
        e.set("circuit", h.label.clone(), "plan");
        match h.note.as_deref().and_then(parse_conductors) {
            Some(c) => e.set("conductors", c, "note"),
            None => {
                e.set("conductors", "2#12 + 1#12G", "rule");
                e.issues.push("no_conductor_note".into());
            }
        }
        out.push(e);
    }
    for (i, c) in runs.conduits.iter().enumerate() {
        let mid = [(c.from_ft[0] + c.to_ft[0]) / 2.0, (c.from_ft[1] + c.to_ft[1]) / 2.0];
        let mut e = ElementTag {
            id: format!("conduit:{}:{i}", c.level),
            kind: "conduit".into(),
            level: c.level.clone(),
            at_ft: mid,
            ..ElementTag::default()
        };
        let loc = locations(&c.level, mid);
        e.set("location", loc, "rule");
        if let Some((m, src)) = wiring_method(rules, loc) {
            if two_methods(&m) {
                e.issues.push("two_rules_apply".into());
            }
            e.set("wiring_method", m, src);
        }
        e.set("size", format!("{}\"", crate::conduit_reader::fmt_size(c.size_in)), if c.size_from_tag { "plan" } else { "default" });
        out.push(e);
    }
    out
}

/// What the truth says each element is (the same rules over the truth's own facts), for
/// scoring.
pub fn expected(t: &Truth, rules: &ProjectRules) -> Vec<ElementTag> {
    let spec = &t.elec_spec;
    let panel_tag = |id: &str| t.elec_panels.iter().find(|p| p.id == id).map_or_else(|| id.to_string(), |p| p.tag.clone());
    let scheduled: Vec<String> =
        t.elec_devices.iter().filter(|d| d.kind == "light_fixture" && !spec.unscheduled.contains(&d.type_tag)).map(|d| d.type_tag.clone()).collect();
    let mut out = Vec::new();
    for d in t.elec_devices.iter().filter(|d| !d.sheet.is_empty()) {
        let mut e = ElementTag {
            id: d.id.clone(),
            kind: if d.kind == "light_fixture" { "luminaire".into() } else { d.kind.clone() },
            level: d.level.clone(),
            at_ft: d.drawn_ft,
            ..ElementTag::default()
        };
        let note = spec.note(&d.id);
        if d.kind == "light_fixture" {
            if spec.untagged.contains(&d.id) {
                e.issues.push("untagged".into());
            } else {
                e.set("type", d.type_tag.clone(), "plan");
                if !scheduled.contains(&d.type_tag) {
                    e.issues.push("tag_not_in_schedule".into());
                }
            }
        }
        if d.kind == "receptacle" {
            let subs = note.map(|n| n.subscripts.clone()).unwrap_or_default();
            e.set("device_type", device_type(&subs), "plan");
            let c = t.elec_circuits.iter().find(|c| c.id == d.circuit);
            if let Some(c) = c {
                e.set("circuit", format!("{}-{}", panel_tag(&c.panel), c.number), "plan");
            }
            let breaker_gf = c.and_then(|c| spec.breaker(&c.panel, &c.number)).is_some_and(|b| matches!(b, "GFCI" | "DF" | "GFEP"));
            let device_gfci = subs.iter().any(|s| s == "GFCI");
            let protection = match (breaker_gf, device_gfci) {
                (true, true) => "GFCI breaker and GFCI device",
                (true, false) => "GFCI breaker",
                (false, true) => "GFCI device",
                (false, false) => "none",
            };
            e.set("protection", protection, "schedule");
            if breaker_gf && device_gfci {
                e.issues.push("double_protection".into());
            }
            if !breaker_gf && !device_gfci && elecspec::device_wet(t, d) {
                e.issues.push("missing_protection".into());
            }
        }
        out.push(e);
    }
    for c in t.elec_circuits.iter().filter(|c| !c.sheet.is_empty()) {
        let mut e = ElementTag {
            id: format!("homerun:{}:{}", c.level, c.label),
            kind: "homerun".into(),
            level: c.level.clone(),
            at_ft: c.homerun_ft,
            ..ElementTag::default()
        };
        if let Some(note) = spec.conductors.iter().find(|(id, _)| *id == c.id).and_then(|(_, n)| parse_conductors(n)) {
            e.set("conductors", note, "note");
        }
        out.push(e);
    }
    for c in t.elec_conduits.iter().filter(|c| !c.sheet.is_empty()) {
        let mid = [(c.from_ft[0] + c.to_ft[0]) / 2.0, (c.from_ft[1] + c.to_ft[1]) / 2.0];
        let mut e =
            ElementTag { id: format!("conduit:{}", c.id), kind: "conduit".into(), level: c.level.clone(), at_ft: mid, ..ElementTag::default() };
        let level_rooms: Vec<&crate::truth::Room> = t.building.rooms.iter().filter(|r| r.level == c.level).collect();
        let room = room_at(&level_rooms, mid, &|r| (r.min_ft, r.max_ft)).copied();
        let loc = if room.is_some_and(|r| r.ceiling.is_none()) { "exposed" } else { "concealed" };
        e.set("location", loc, "rule");
        if let Some((m, _)) = wiring_method(rules, loc) {
            if two_methods(&m) {
                e.issues.push("two_rules_apply".into());
            }
            e.set("wiring_method", m, "rule");
        }
        out.push(e);
    }
    out
}

/// Precision and recall of one field (or issue kind) over the matched elements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldScore {
    pub truth: usize,
    pub found: usize,
    pub correct: usize,
}

impl FieldScore {
    pub fn precision(&self) -> f64 {
        if self.found == 0 { 1.0 } else { self.correct as f64 / self.found as f64 }
    }
    pub fn recall(&self) -> f64 {
        if self.truth == 0 { 1.0 } else { self.correct as f64 / self.truth as f64 }
    }
}

/// Score found tags against expected ones: elements matched by kind, level and position (within
/// 1'; homeruns by label), then each field's value and each issue kind compared.
pub fn score(found: &[ElementTag], truth: &[ElementTag]) -> BTreeMap<String, FieldScore> {
    let near = |a: &ElementTag, b: &ElementTag| {
        a.kind == b.kind
            && a.level == b.level
            && if a.kind == "homerun" { a.id == b.id } else { (a.at_ft[0] - b.at_ft[0]).hypot(a.at_ft[1] - b.at_ft[1]) <= 1.0 }
    };
    let mut out: BTreeMap<String, FieldScore> = BTreeMap::new();
    for t in truth {
        for k in t.fields.keys() {
            out.entry(k.clone()).or_default().truth += 1;
        }
        for i in &t.issues {
            out.entry(format!("issue:{i}")).or_default().truth += 1;
        }
    }
    for f in found {
        let m = truth.iter().find(|t| near(f, t));
        for (k, v) in &f.fields {
            if !m.is_some_and(|t| t.fields.contains_key(k)) {
                continue;
            }
            let s = out.entry(k.clone()).or_default();
            s.found += 1;
            if m.and_then(|t| t.fields.get(k)).is_some_and(|tv| tv.value.eq_ignore_ascii_case(&v.value)) {
                s.correct += 1;
            }
        }
        for i in &f.issues {
            let s = out.entry(format!("issue:{i}")).or_default();
            s.found += 1;
            if m.is_some_and(|t| t.issues.contains(i)) {
                s.correct += 1;
            }
        }
    }
    out
}

/// Read a fixture set and tag it: devices, homeruns and conduit from the electrical plans,
/// schedules from the schedule sheet, locations from the ceiling reader.
pub fn read_fixture_tags(seed: u64, rules: &ProjectRules) -> Result<(Truth, Vec<ElementTag>), String> {
    let set = crate::generate(seed)?;
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = crate::sheet_reader::recognise_fixture_set(&dxf);
    let (mut devices, _) = crate::elec_reader::read_devices(&dxf, &map, &set.truth);
    let runs = crate::conduit_reader::read_conduits(&dxf, &map);
    let mut tables = Vec::new();
    for (n, b) in &dxf {
        let stem = n.trim_end_matches(".dxf");
        if map.sheets.iter().any(|s| s.number == stem && s.discipline == "E" && s.title.to_ascii_uppercase().contains("SCHEDULE"))
            && let Ok(d) = cadaddy_io::read_dxf(b)
        {
            let shapes = crate::arch_reader::shapes(&d, &|p| p, 1.0);
            tables.extend(crate::arch_reader::tables(&shapes, &["SCHEDULE"]));
        }
    }
    let schedules = schedules_from(&tables);
    let ceil = crate::ceiling_reader::read_ceilings(&dxf, &map, None);
    // The legend's sample symbols sit outside the building: only devices within the grid count.
    let xs: Vec<f64> = map.grid_x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = map.grid_y.iter().map(|g| g.at_ft).collect();
    if !xs.is_empty() && !ys.is_empty() {
        let lo = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min) - 10.0;
        let hi = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max) + 10.0;
        let (x0, x1, y0, y1) = (lo(&xs), hi(&xs), lo(&ys), hi(&ys));
        devices.retain(|d| d.drawn_ft[0] >= x0 && d.drawn_ft[0] <= x1 && d.drawn_ft[1] >= y0 && d.drawn_ft[1] <= y1);
    }
    // Devices carry their room's number: the ceiling reader's rooms give the name.
    for d in &mut devices {
        if let Some(r) = ceil.rooms.iter().find(|r| r.level == d.level && r.number == d.room) {
            d.room = r.name.clone();
        }
    }
    let locate = |level: &str, p: [f64; 2]| -> &'static str {
        let level_rooms: Vec<&crate::ceiling_reader::FoundRoom> = ceil.rooms.iter().filter(|r| r.level == level).collect();
        let room = room_at(&level_rooms, p, &|r| (r.min_ft, r.max_ft)).copied();
        // Open only when every ceiling read for the room says so.
        let open = room.is_some_and(|r| {
            let cs: Vec<&crate::ceiling_reader::FoundCeiling> =
                ceil.ceilings.iter().filter(|c| c.level == level && c.room_number == r.number).collect();
            !cs.is_empty() && cs.iter().all(|c| c.open_to_structure)
        });
        if open { "exposed" } else { "concealed" }
    };
    Ok((set.truth, tag(&devices, &runs, &schedules, rules, &locate)))
}

/// The tags as the building spec's `tags` list (for `bspec.set`).
pub fn spec_patch(tags: &[ElementTag]) -> serde_json::Value {
    serde_json::to_value(tags).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conductor_notes_parse() {
        assert_eq!(parse_conductors("2#12, 1#12G").as_deref(), Some("2#12 + 1#12G"));
        assert_eq!(parse_conductors("3#8 THHN + #10G in 3/4\" EMT").as_deref(), Some("3#8 THHN + 1#10G in 3/4\" EMT"));
        assert_eq!(parse_conductors("SER 2-2-2-4 Al").as_deref(), Some("SER 2-2-2-4 AL"));
        assert_eq!(parse_conductors("L1/1"), None);
        assert_eq!(parse_conductors("#"), None);
        assert_eq!(parse_conductors("99#12"), None);
        assert_eq!(parse_conductors(&"2#12, ".repeat(40)), None);
    }

    #[test]
    fn subscripts_make_device_types() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(device_type(&s(&["GFCI"])), "GFCI receptacle");
        assert_eq!(device_type(&s(&["usb"])), "USB receptacle");
        assert_eq!(device_type(&[]), "standard duplex receptacle");
        assert!(two_methods("MC cable or EMT") && !two_methods("EMT"));
    }
}
