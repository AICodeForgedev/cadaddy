//! Electrical gear from the prints (Q290): the equipment, lug and feeder schedules on the
//! E-sheets read back into the building spec's gear (`mep.equipment` with its lug provisions and
//! enclosure) and feeders (`mep.feeders`); each piece of gear placed at its tag on the plan as a
//! box of its scheduled size at its mounting height, with the schedule's row as its properties
//! (as text: no load calculations). The light fixture schedule's columns go on every fixture of
//! that type ([`link_fixtures`]).

use std::collections::BTreeMap;

use cadaddy_doc::Drawing;
use cadaddy_walk3d::{Enclosure, Feeder, MepEquipment, Termination};

use crate::elec_reader::{FoundDevice, FoundPanel};
use crate::equip_reader::{Table, read_tables_keyed};

/// Tables keyed by a tag (gear) or a type (light fixtures).
fn key_column(n: &str) -> bool {
    matches!(n.trim().to_ascii_uppercase().as_str(), "TAG" | "TYPE" | "MARK")
}

/// What the reader found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GearRead {
    /// Placed gear (plan corners in drawing units, `units_per_foot` per building foot).
    pub equipment: Vec<MepEquipment>,
    pub feeders: Vec<Feeder>,
    /// Light fixture types: type → the schedule's columns.
    pub fixture_types: BTreeMap<String, BTreeMap<String, String>>,
    /// Scheduled gear whose tag no plan shows.
    pub unplaced: Vec<String>,
}

fn cell<'a>(row: &'a BTreeMap<String, String>, col: &str) -> &'a str {
    row.get(col).map_or("", |v| v.trim())
}

/// `-`, `NONE` and empty cells.
fn blank(v: &str) -> bool {
    let v = v.trim();
    v.is_empty() || v == "-" || v.eq_ignore_ascii_case("none")
}

/// A number of inches as printed: `4`, `3/8`, `1-1/2`, `0.75` (quotes and words ignored).
pub fn inches(v: &str) -> Option<f64> {
    let t: String =
        v.trim().trim_end_matches(|c: char| !c.is_ascii_digit()).chars().filter(|c| c.is_ascii_digit() || matches!(c, '/' | '-' | '.')).collect();
    let (whole, frac) = match t.split_once('-') {
        Some((w, f)) if f.contains('/') => (w, Some(f)),
        _ if t.contains('/') => ("0", Some(t.as_str())),
        _ => (t.as_str(), None),
    };
    let w: f64 = whole.parse().ok()?;
    let f = match frac {
        Some(f) => {
            let (n, d) = f.split_once('/')?;
            let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
            if d == 0.0 {
                return None;
            }
            n / d
        }
        None => 0.0,
    };
    Some(w + f).filter(|v| v.is_finite())
}

/// Feet and inches as printed (`6'-6"`, `6'`, `78"`) in inches.
fn feet_inches(v: &str) -> Option<f64> {
    let t = v.trim();
    match t.split_once('\'') {
        Some((ft, rest)) => {
            let ft: f64 = ft.trim().parse().ok()?;
            let rest = rest.trim().trim_start_matches('-');
            let i = if rest.trim_matches('"').trim().is_empty() { 0.0 } else { inches(rest)? };
            Some(ft * 12.0 + i)
        }
        None => inches(t),
    }
}

/// `72"W x 36"D x 90"H` → width, depth, height (inches).
fn size(v: &str) -> Option<[f64; 3]> {
    let parts: Vec<f64> = v.split(['x', 'X']).filter_map(inches).collect();
    match parts[..] {
        [w, d, h] if w > 0.0 && d > 0.0 && h > 0.0 => Some([w, d, h]),
        _ => None,
    }
}

/// An entry plate or gutter: `6" x 3"` → `[6, 3]`; `NONE` → `[0, 0]`.
fn plate(v: &str) -> [f64; 2] {
    if blank(v) {
        return [0.0, 0.0];
    }
    let p: Vec<f64> = v.split(['x', 'X']).filter_map(inches).collect();
    match p[..] {
        [a, b] => [a, b],
        _ => [0.0, 0.0],
    }
}

/// The bottom of gear above its floor (inches) from its mounting and height: `FLOOR`, `PAD`,
/// `SURFACE, TOP @ 6'-6"`, `BOTTOM @ 4'-0"`.
fn bottom(mounting: &str, height_in: f64) -> f64 {
    let m = mounting.to_uppercase();
    let at = m.split_once('@').and_then(|(_, v)| feet_inches(v));
    match at {
        Some(top) if m.contains("TOP") => (top - height_in).max(0.0),
        Some(b) => b,
        None => 0.0,
    }
}

fn kind_of(typ: &str) -> &'static str {
    let t = typ.to_uppercase();
    if t.contains("SWITCHBOARD") || t.contains("SWITCHGEAR") {
        "switchboard"
    } else if t.contains("TRANSFORMER") {
        "transformer"
    } else if t.contains("DISCONNECT") {
        "disconnect"
    } else {
        "panel"
    }
}

/// `600 KCMIL AL` → (`600 kcmil`, `al`).
fn conductor(v: &str) -> (String, String) {
    let low = v.trim().to_lowercase();
    let mut words: Vec<&str> = low.split_whitespace().collect();
    let material = match words.last() {
        Some(&m) if m == "al" || m == "cu" => {
            words.pop();
            m.to_string()
        }
        _ => String::new(),
    };
    (words.join(" "), material)
}

/// `1 x 600 KCMIL AL` → (1, `600 kcmil`).
fn counted(v: &str) -> (u32, String) {
    if blank(v) {
        return (0, String::new());
    }
    match v.split_once(['x', 'X']) {
        Some((n, rest)) => (n.trim().parse().unwrap_or(1), conductor(rest).0),
        None => (1, conductor(v).0),
    }
}

fn termination(row: &BTreeMap<String, String>) -> Option<Termination> {
    let mut t = Termination {
        point: cell(row, "POINT").to_lowercase().replace(' ', "_"),
        lugs: cell(row, "LUGS").parse().ok()?,
        wires_per_lug: cell(row, "WIRES").parse().unwrap_or(1),
        range: cell(row, "RANGE").to_lowercase(),
        material: cell(row, "RATING").to_lowercase(),
        lug_type: if cell(row, "TYPE").to_uppercase().starts_with("COMP") { "compression".into() } else { "mechanical".into() },
        supplied: !cell(row, "SUPPLIED").to_uppercase().contains("BY"),
        ..Termination::default()
    };
    // `3/8" 1-HOLE, 1/2" STACK`
    let bolt = cell(row, "BOLT");
    if !blank(bolt) {
        let (dia, rest) = bolt.split_once('"').unwrap_or((bolt, ""));
        t.bolt_in = inches(dia).unwrap_or(t.bolt_in);
        if let Some(h) = rest.to_uppercase().split("-HOLE").next().and_then(|x| x.trim().rsplit(' ').next().and_then(|n| n.parse().ok())) {
            t.holes = h;
        }
        if let Some(st) = rest.split_once(',').map(|(_, x)| x.trim()).and_then(|x| inches(x.split('"').next().unwrap_or(x))) {
            t.stack_in = st;
        }
    }
    // `8" OPP`
    let bend = cell(row, "BENDING");
    if !blank(bend) {
        t.bending_in = inches(bend.split('"').next().unwrap_or(bend)).unwrap_or(0.0);
        t.opposite = !bend.to_uppercase().contains("ADJ");
    }
    (!t.point.is_empty()).then_some(t)
}

fn feeder(row: &BTreeMap<String, String>) -> Option<Feeder> {
    let tag = cell(row, "TAG");
    if tag.is_empty() {
        return None;
    }
    let (size, material) = conductor(cell(row, "PHASE"));
    let (neutrals, neutral_size) = counted(cell(row, "NEUTRAL"));
    let (grounds, ground_size) = counted(cell(row, "GROUND"));
    let raceway = cell(row, "RACEWAY");
    let (conduits, conduit_in) = match raceway.split_once(['x', 'X']) {
        Some((n, s)) => (n.trim().parse().unwrap_or(1), inches(s).unwrap_or(0.0)),
        None => (1, inches(raceway).unwrap_or(0.0)),
    };
    let from = cell(row, "FROM");
    Some(Feeder {
        tag: tag.to_string(),
        from: if from.eq_ignore_ascii_case("UTILITY") { String::new() } else { from.to_string() },
        to: cell(row, "TO").to_string(),
        sets: cell(row, "SETS").parse().unwrap_or(1),
        neutral_size: if neutral_size == size { String::new() } else { neutral_size },
        size,
        material: if material.is_empty() { "cu".into() } else { material },
        neutrals,
        grounds,
        ground_size,
        extra_grounds: if blank(cell(row, "BONDING")) {
            Vec::new()
        } else {
            cell(row, "BONDING").split(',').map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect()
        },
        conduits,
        conduit_in,
        entry: cell(row, "ENTRY").to_lowercase(),
        ..Feeder::default()
    })
}

/// The gear on `drawings` (sheet number, drawing): its schedules, placed at the tags `panels`
/// found on the plans (building feet), in drawing units of `units_per_foot`.
pub fn read_gear(drawings: &[(&str, &Drawing)], panels: &[FoundPanel], units_per_foot: f64) -> GearRead {
    let tables: Vec<Table> = drawings.iter().flat_map(|(sheet, d)| read_tables_keyed(d, sheet, key_column)).collect();
    let titled = |t: &'static str| tables.iter().filter(move |x| x.title.trim().eq_ignore_ascii_case(t));
    let mut out = GearRead::default();
    let mut lugs: BTreeMap<String, Vec<Termination>> = BTreeMap::new();
    for row in titled("LUG SCHEDULE").flat_map(|t| &t.rows) {
        if let Some(t) = termination(row) {
            lugs.entry(cell(row, "TAG").to_string()).or_default().push(t);
        }
    }
    for row in titled("ELECTRICAL EQUIPMENT SCHEDULE").flat_map(|t| &t.rows) {
        let tag = cell(row, "TAG").to_string();
        let Some([w, d, h]) = size(cell(row, "SIZE")) else { continue };
        let Some(p) = panels.iter().find(|p| p.tag.eq_ignore_ascii_case(&tag)) else {
            out.unplaced.push(tag);
            continue;
        };
        let upf = if units_per_foot.is_finite() && units_per_foot > 0.0 { units_per_foot } else { 12.0 };
        let (hw, hd) = (w / 24.0, d / 24.0);
        let mut properties: BTreeMap<String, String> =
            row.iter().filter(|(k, _)| k.as_str() != "TAG").map(|(k, v)| (k.clone(), v.trim().to_string())).collect();
        properties.retain(|_, v| !blank(v));
        let (top, bot, gut) = (cell(row, "TOP ENTRY"), cell(row, "BOTTOM ENTRY"), cell(row, "GUTTER"));
        let enclosure = (!(blank(top) && blank(bot) && blank(gut))).then(|| Enclosure {
            top_entry_in: plate(top),
            bottom_entry_in: plate(bot),
            gutter_in: plate(gut),
        });
        let mut sources = BTreeMap::new();
        sources.insert("footprint".into(), "schedule".into());
        sources.insert("elevation".into(), "schedule".into());
        out.equipment.push(MepEquipment {
            kind: kind_of(cell(row, "TYPE")).into(),
            level: p.level.clone(),
            min: [(p.at_ft[0] - hw) * upf, (p.at_ft[1] - hd) * upf],
            max: [(p.at_ft[0] + hw) * upf, (p.at_ft[1] + hd) * upf],
            bottom_ft: bottom(cell(row, "MOUNTING"), h) / 12.0,
            height_ft: h / 12.0,
            title: "ELECTRICAL EQUIPMENT SCHEDULE".into(),
            properties,
            sources,
            confidence: 1.0,
            terminations: lugs.remove(&tag).unwrap_or_default(),
            enclosure,
            tag,
            ..MepEquipment::default()
        });
    }
    out.feeders = titled("FEEDER SCHEDULE").flat_map(|t| &t.rows).filter_map(feeder).collect();
    for row in titled("LIGHT FIXTURE SCHEDULE").flat_map(|t| &t.rows) {
        let ty = cell(row, "TYPE").to_string();
        if !ty.is_empty() {
            out.fixture_types.insert(
                ty,
                row.iter().filter(|(k, v)| k.as_str() != "TYPE" && !v.trim().is_empty()).map(|(k, v)| (k.clone(), v.trim().to_string())).collect(),
            );
        }
    }
    out
}

/// The light fixture schedule's columns on every fixture of its type; the number linked.
pub fn link_fixtures(devices: &mut [FoundDevice], types: &BTreeMap<String, BTreeMap<String, String>>) -> usize {
    let mut n = 0;
    for d in devices.iter_mut().filter(|d| d.kind == "light_fixture") {
        if let Some(p) = types.get(&d.type_tag) {
            d.properties = p.clone();
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printed_values_read_back() {
        assert_eq!(inches("3/8\""), Some(0.375));
        assert_eq!(inches("1-1/2\""), Some(1.5));
        assert_eq!(inches("4\""), Some(4.0));
        assert_eq!(inches("x"), None);
        assert_eq!(feet_inches("6'-6\""), Some(78.0));
        assert_eq!(size("72\"W x 36\"D x 90\"H"), Some([72.0, 36.0, 90.0]));
        assert_eq!(plate("6\" x 3\""), [6.0, 3.0]);
        assert_eq!(plate("NONE"), [0.0, 0.0]);
        assert_eq!(bottom("SURFACE, TOP @ 6'-6\"", 60.0), 18.0);
        assert_eq!(bottom("FLOOR", 90.0), 0.0);
        assert_eq!(conductor("600 KCMIL AL"), ("600 kcmil".to_string(), "al".to_string()));
        assert_eq!(counted("1 x #6 CU"), (1, "#6".to_string()));
        assert_eq!(counted("-"), (0, String::new()));
    }
}
