//! Occupancy sensor power packs (Q6352): the plans draw the sensors but rarely the packs that
//! power them and switch their lights. Each low-voltage sensor zone (a room, or each switched leg
//! in it) gets the pack its sensor family needs, a switching pack or a 0-10V dimming pack, set on a
//! box beside the controlled luminaire nearest its sensors, the zone's sensors wired to it with
//! low-voltage cable. A wall-box sensor switches line voltage itself and needs none; a pack the
//! documents draw in a room is that room's. Everything the rule fills in is inferred, with its
//! basis: trade practice `E-SENSOR-POWER-PACK` in `tradepractice.json` (its words and numbers).

use std::collections::BTreeMap;

use cadaddy_doc::Drawing;
use cadaddy_doc::props::{self, Prop};
use serde::Deserialize;

/// The trade practice rule the packs follow.
pub(crate) const RULE: &str = "E-SENSOR-POWER-PACK";
/// Most devices and luminaires looked at (hostile drawings), and zones made.
const MAX_ELEMENTS: usize = 2_000_000;
const MAX_ZONES: usize = 100_000;
/// The property a sensor carries what the specification says powers it (`power pack`), its
/// source the section and line.
pub(crate) const CONTROL_POWER: &str = "control_power";
/// Most sensors one zone lists.
const MAX_ZONE_SENSORS: usize = 500;

/// The rule's words and numbers (`tradepractice.json`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PackRule {
    pub id: String,
    pub basis: String,
    pub low_voltage_words: Vec<String>,
    pub line_voltage_words: Vec<String>,
    pub remote_words: Vec<String>,
    pub wallbox_words: Vec<String>,
    pub remote_mountings: Vec<String>,
    pub wallbox_max_height_in: f64,
    pub spec_context_lines: usize,
    pub dimming_words: Vec<String>,
    pub pack_words: Vec<String>,
    pub switching_item: String,
    pub dimming_item: String,
    pub box_item: String,
    pub box_size: String,
    pub cable_item: String,
    pub cable_size: String,
    pub cable_slack_ft: f64,
    pub default_cable_ft: f64,
    pub zone_reach_ft: f64,
}

/// The rule as its data has it (an empty one, which fills nothing, when the data has none).
pub(crate) fn rule() -> PackRule {
    serde_json::from_value(super::provenance::rule_data(RULE)).unwrap_or_default()
}

/// Words as whole words: upper case, anything but letters, digits, `-` and `/` a space, a space
/// each side (`LV` is not in `LEVEL`, `DIM` not in `DIMENSION`).
fn spaced(s: &str) -> String {
    let w: Vec<String> =
        s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '/')).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect();
    format!(" {} ", w.join(" "))
}

/// The first of `words` in `text` (whole words), if any.
fn said<'a>(text: &str, words: &'a [String]) -> Option<&'a str> {
    let up = spaced(text);
    words.iter().find(|w| !w.trim().is_empty() && up.contains(&spaced(w))).map(String::as_str)
}

/// One occupancy or vacancy sensor as the model has it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sensor {
    pub handle: String,
    pub level: String,
    /// Its room's number (or name), upper case; empty when none.
    pub room: String,
    /// Its switch leg letter, lower case; empty when none.
    pub leg: String,
    /// What its legend, type, product and catalog number say of it.
    pub words: String,
    /// How the built element is mounted (`wall`, `ceiling`), and its height (inches).
    pub mounting: String,
    pub height_in: Option<f64>,
    /// Where the specification says sensors are powered from a pack (`control_power`): its
    /// section and line; empty when it says nothing.
    pub spec: String,
    /// Where (feet, plan).
    pub at: Option<[f64; 2]>,
    /// The luminaires it controls (handles), as the controls step linked them.
    pub controls: Vec<String>,
}

/// A luminaire a pack may sit beside.
#[derive(Clone, Debug, Default)]
pub(crate) struct Load {
    pub handle: String,
    pub level: String,
    pub room: String,
    pub at: Option<[f64; 2]>,
}

/// What a sensor needs: a pack (dimming or not) and why, or none and why.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Need {
    Pack { dimming: bool, why: String },
    None(String),
}

/// What one sensor needs: its own words' voltage first (`LOW VOLTAGE`, `120/277V`), then where
/// they mount it (`CEILING`, `WALL SWITCH`), then what the specification says of sensors (a
/// separate power pack), then the built element's mounting (a ceiling sensor is never in a wall
/// box; nor is one on a wall above switch height); a sensor nothing places is a wall-box one.
pub(crate) fn need(r: &PackRule, s: &Sensor) -> Need {
    let dimming = said(&s.words, &r.dimming_words).is_some();
    let pack = |why: String| Need::Pack { dimming, why };
    if let Some(w) = said(&s.words, &r.low_voltage_words) {
        return pack(format!("its documents say {w}"));
    }
    if let Some(w) = said(&s.words, &r.line_voltage_words) {
        return Need::None(format!("its documents say {w}: it switches line voltage itself"));
    }
    if let Some(w) = said(&s.words, &r.remote_words) {
        return pack(format!("its documents mount it {w}: a low-voltage sensor"));
    }
    if let Some(w) = said(&s.words, &r.wallbox_words) {
        return Need::None(format!("its documents make it a {w} sensor: it switches line voltage itself"));
    }
    if !s.spec.trim().is_empty() {
        return pack(format!("the specification: {}", s.spec.trim()));
    }
    if r.remote_mountings.iter().any(|m| m.eq_ignore_ascii_case(s.mounting.trim())) {
        return pack(format!("mounted on the {}: a low-voltage sensor", s.mounting.trim()));
    }
    if r.wallbox_max_height_in > 0.0
        && s.mounting.trim().eq_ignore_ascii_case("wall")
        && let Some(h) = s.height_in.filter(|h| h.is_finite() && *h > r.wallbox_max_height_in)
    {
        return pack(format!("on the wall at {h:.0} in, above a wall box's {:.0} in: a high-wall low-voltage sensor", r.wallbox_max_height_in));
    }
    Need::None("a wall-box sensor (nothing says otherwise): it switches line voltage itself".into())
}

/// What the specification's lighting control section (`26 09 23`) says powers its sensors: the
/// first line naming a pack (`Separate power pack`, `Sensor is powered from the power pack`) within
/// `spec_context_lines` of a line naming a sensor, as `section: line`.
pub(crate) fn spec_says(r: &PackRule, lines: &[(String, String)]) -> Option<String> {
    let reach = r.spec_context_lines.min(1_000);
    let mut since_sensor: Option<usize> = None;
    for (line, doc) in lines.iter().take(50_000) {
        let up = spaced(line);
        if up.contains(" SENSOR ") || up.contains(" SENSORS ") {
            since_sensor = Some(0);
        } else {
            since_sensor = since_sensor.map(|n| n + 1).filter(|n| *n <= reach);
        }
        if since_sensor.is_some() && said(line, &r.pack_words).is_some() {
            return Some(format!("{doc}: {}", line.trim().chars().take(160).collect::<String>()));
        }
    }
    None
}

/// One sensor zone's pack.
#[derive(Clone, Debug, Default)]
pub(crate) struct Zone {
    pub level: String,
    pub room: String,
    pub leg: String,
    pub sensors: Vec<String>,
    pub dimming: bool,
    /// Why its sensors need a pack.
    pub why: String,
    /// The luminaire it sits beside, and where (feet, plan).
    pub beside: Option<String>,
    pub at: Option<[f64; 2]>,
    /// The low-voltage cable from its sensors to it, feet.
    pub cable_ft: f64,
}

impl Zone {
    /// The pack bought for it.
    pub(crate) fn item<'a>(&self, r: &'a PackRule) -> &'a str {
        if self.dimming { &r.dimming_item } else { &r.switching_item }
    }

    /// Its place in words (`Level 2 room 204 leg a`).
    pub(crate) fn place(&self) -> String {
        let mut s = self.level.clone();
        if !self.room.is_empty() {
            s.push_str(&format!(" room {}", self.room));
        }
        if !self.leg.is_empty() {
            s.push_str(&format!(" leg {}", self.leg));
        }
        s.trim().to_string()
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// The packs the sensors need: one per zone (level, room, leg) of sensors that need one, none
/// where the documents draw a pack in the room (`drawn`: level and room, upper case), each sensor
/// with no room a zone of its own. The pack sits beside the zone's controlled luminaire nearest
/// its sensors (one its sensors name, else one of its room within reach), else at its first
/// sensor; its sensors joined to it by the shortest tree along the plan's two axes, each adding its drops.
pub(crate) fn zones(r: &PackRule, sensors: &[Sensor], loads: &[Load], drawn: &[(String, String)]) -> Vec<Zone> {
    let key_level = |l: &str| l.trim().to_ascii_uppercase();
    let mut by_zone: BTreeMap<(String, String, String), (Vec<&Sensor>, bool, String)> = BTreeMap::new();
    for s in sensors.iter().take(MAX_ELEMENTS) {
        let Need::Pack { dimming, why } = need(r, s) else { continue };
        let room = s.room.trim().to_ascii_uppercase();
        if !room.is_empty() && drawn.iter().any(|(l, rm)| *l == key_level(&s.level) && *rm == room) {
            continue;
        }
        // A sensor with no room is its own zone.
        let room_key = if room.is_empty() { format!("\u{0}{}", s.handle) } else { room };
        let e = by_zone.entry((key_level(&s.level), room_key, s.leg.trim().to_ascii_lowercase())).or_insert_with(|| (Vec::new(), false, why));
        if e.0.len() < MAX_ZONE_SENSORS {
            e.0.push(s);
        }
        e.1 |= dimming;
        if by_zone.len() >= MAX_ZONES {
            break;
        }
    }
    let by_handle: BTreeMap<&str, &Load> = loads.iter().map(|l| (l.handle.as_str(), l)).collect();
    let reach = if r.zone_reach_ft.is_finite() && r.zone_reach_ft > 0.0 { r.zone_reach_ft } else { 0.0 };
    let slack = if r.cable_slack_ft.is_finite() { r.cable_slack_ft.max(0.0) } else { 0.0 };
    let default_ft = if r.default_cable_ft.is_finite() { r.default_cable_ft.max(0.0) } else { 0.0 };
    let mut out = Vec::new();
    for ((_, room_key, leg), (ss, dimming, why)) in by_zone {
        let Some(first) = ss.first() else { continue };
        let room = if room_key.starts_with('\u{0}') { String::new() } else { room_key };
        let spots: Vec<[f64; 2]> = ss.iter().filter_map(|s| s.at).collect();
        let centre = (!spots.is_empty()).then(|| {
            let n = spots.len() as f64;
            [spots.iter().map(|p| p[0]).sum::<f64>() / n, spots.iter().map(|p| p[1]).sum::<f64>() / n]
        });
        // The luminaires it may sit beside: those its sensors control, else its room's near them.
        let named: Vec<&Load> = ss.iter().flat_map(|s| s.controls.iter()).filter_map(|h| by_handle.get(h.as_str()).copied()).collect();
        let candidates: Vec<&Load> = if named.is_empty() && !room.is_empty() {
            loads
                .iter()
                .filter(|l| key_level(&l.level) == key_level(&first.level) && l.room.trim().eq_ignore_ascii_case(&room))
                .filter(|l| matches!((l.at, centre), (Some(a), Some(c)) if dist(a, c) <= reach))
                .collect()
        } else {
            named
        };
        let beside = centre
            .and_then(|c| candidates.iter().filter_map(|l| Some((l, dist(l.at?, c)))).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(l, _)| *l))
            .or_else(|| candidates.first().copied());
        let at = beside.and_then(|l| l.at).or(first.at);
        let cable_ft = chain_ft(&ss, at, slack, default_ft);
        out.push(Zone {
            level: first.level.clone(),
            room,
            leg,
            sensors: ss.iter().map(|s| s.handle.clone()).collect(),
            dimming,
            why,
            beside: beside.map(|l| l.handle.clone()),
            at,
            cable_ft,
        });
    }
    out
}

/// A zone's low-voltage cable, feet: the shortest tree joining the pack and its sensors along the
/// plan's two axes (the field runs from the pack to the nearest sensor and on, branching where
/// that is shorter: never longer than a home run to each), each sensor adding its drops and
/// makeup (`slack`); a sensor with no location, or a pack with none, takes `default_ft`.
fn chain_ft(ss: &[&Sensor], pack: Option<[f64; 2]>, slack: f64, default_ft: f64) -> f64 {
    let manhattan = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() + (a[1] - b[1]).abs();
    let mut left: Vec<[f64; 2]> = Vec::new();
    let mut total = 0.0;
    for s in ss {
        match (s.at, pack) {
            (Some(a), Some(_)) => left.push(a),
            _ => total += default_ft,
        }
    }
    let Some(pack) = pack else { return total };
    // Prim's tree from the pack: each sensor's distance to the tree so far.
    let mut best: Vec<f64> = left.iter().map(|p| manhattan(*p, pack)).collect();
    let mut done = vec![false; left.len()];
    for _ in 0..left.len() {
        let Some((i, d)) =
            best.iter().enumerate().filter(|(i, _)| !done.get(*i).copied().unwrap_or(true)).map(|(i, d)| (i, *d)).min_by(|a, b| a.1.total_cmp(&b.1))
        else {
            break;
        };
        if let Some(x) = done.get_mut(i) {
            *x = true;
        }
        if d.is_finite() {
            total += d + slack;
        }
        let Some(&p) = left.get(i) else { break };
        for (j, q) in left.iter().enumerate() {
            if let (Some(b), false) = (best.get_mut(j), done.get(j).copied().unwrap_or(true)) {
                *b = b.min(manhattan(*q, p));
            }
        }
    }
    total
}

/// The model's sensors, luminaires and drawn packs (level and room, upper case).
pub(crate) fn from_model(d: &Drawing) -> (Vec<Sensor>, Vec<Load>, Vec<(String, String)>) {
    let r = rule();
    let (mut sensors, mut loads, mut drawn) = (Vec::new(), Vec::new(), Vec::new());
    for (e, base) in super::elemprops::elements(d).take(MAX_ELEMENTS) {
        if !super::elemprops::is_device_kind(&base) {
            continue;
        }
        let kind = super::elemprops::reported_kind(e, base);
        let store = props::read(&e.common.xdata);
        let text = |k: &str| store.get(k).filter(|p| p.has_value()).map(Prop::text).map(|t| t.trim().to_string()).unwrap_or_default();
        let level = {
            let l = text("level");
            if l.is_empty() { super::bspec::made_level(e).unwrap_or_default() } else { l }
        };
        let room = text("room").to_ascii_uppercase();
        let at = {
            let v: Vec<f64> = text("location").split(',').filter_map(|x| x.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).collect();
            match v[..] {
                [x, y, ..] => Some([x, y]),
                _ => None,
            }
        };
        let words = ["type", "device_type", "product", "configuration", "required_features", "catalog_number", "description", "dimming"]
            .iter()
            .map(|k| text(k))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if matches!(kind.as_str(), "luminaire" | "wall_luminaire" | "light_fixture") {
            loads.push(Load { handle: e.handle.hex(), level, room, at });
            continue;
        }
        if said(&words, &r.pack_words).is_some() {
            drawn.push((level.to_ascii_uppercase(), room));
            continue;
        }
        let sensor_kind =
            matches!(kind.as_str(), "occupancy_sensor" | "vacancy_sensor") || super::propfill::kind_of(&text("type_mark")) == "occupancy sensor";
        if !sensor_kind {
            continue;
        }
        sensors.push(Sensor {
            handle: e.handle.hex(),
            level,
            room,
            leg: text("switch_leg").to_ascii_lowercase(),
            words,
            mounting: text("mounting"),
            height_in: text("mount_height").parse::<f64>().ok().filter(|v| v.is_finite()),
            spec: store
                .get(CONTROL_POWER)
                .filter(|p| p.has_value() && said(&p.text(), &r.pack_words).is_some())
                .map(|p| p.source.doc.clone().unwrap_or_else(|| "the specification".into()))
                .unwrap_or_default(),
            at,
            controls: text("controls").split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).take(200).collect(),
        });
    }
    (sensors, loads, drawn)
}

/// The spec's sensors, luminaires and drawn packs, read from its tags (a job whose devices the
/// model has not built).
pub(crate) fn from_tags(sp: &cadaddy_walk3d::BuildingSpec) -> (Vec<Sensor>, Vec<Load>, Vec<(String, String)>) {
    let r = rule();
    let (mut sensors, mut loads, mut drawn) = (Vec::new(), Vec::new(), Vec::new());
    for t in sp.tags.iter().take(MAX_ELEMENTS) {
        let f = |k: &str| t.fields.get(k).map(|v| v.value.trim().to_string()).unwrap_or_default();
        let at = Some(t.at_ft).filter(|p| p[0].is_finite() && p[1].is_finite());
        let room = f("room").to_ascii_uppercase();
        let words = ["type", "device_type", "product", "catalog_number", "dimming"]
            .iter()
            .map(|k| f(k))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if t.kind == "luminaire" {
            loads.push(Load { handle: t.id.clone(), level: t.level.clone(), room, at });
            continue;
        }
        if said(&words, &r.pack_words).is_some() {
            drawn.push((t.level.to_ascii_uppercase(), room));
            continue;
        }
        if !matches!(t.kind.as_str(), "occupancy_sensor" | "vacancy_sensor") {
            continue;
        }
        let leg = [f("switch_leg"), f("leg")].into_iter().find(|l| !l.is_empty()).unwrap_or_default().to_ascii_lowercase();
        sensors.push(Sensor {
            handle: t.id.clone(),
            level: t.level.clone(),
            room,
            leg,
            words,
            mounting: f("mounting"),
            height_in: f("mount_height_in").parse::<f64>().ok().filter(|v| v.is_finite()),
            spec: String::new(),
            at,
            controls: Vec::new(),
        });
    }
    (sensors, loads, drawn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor(h: &str, room: &str, leg: &str, words: &str, mounting: &str, at: [f64; 2]) -> Sensor {
        Sensor {
            handle: h.into(),
            level: "Level 1".into(),
            room: room.into(),
            leg: leg.into(),
            words: words.into(),
            mounting: mounting.into(),
            height_in: None,
            spec: String::new(),
            at: Some(at),
            controls: Vec::new(),
        }
    }

    #[test]
    fn the_rule_is_in_the_data() {
        let r = rule();
        assert_eq!(r.id, RULE);
        assert!(!r.switching_item.is_empty() && !r.dimming_item.is_empty() && !r.basis.is_empty());
        assert!(!r.low_voltage_words.is_empty() && !r.wallbox_words.is_empty() && !r.remote_mountings.is_empty());
    }

    #[test]
    fn low_voltage_sensors_need_a_pack_and_wall_box_ones_none() {
        let r = rule();
        let s = |w: &str, m: &str| need(&r, &sensor("1", "", "", w, m, [0.0, 0.0]));
        assert!(matches!(s("CEILING MOUNTED OCCUPANCY SENSOR", "wall"), Need::Pack { dimming: false, .. }));
        assert!(matches!(s("occupancy sensor", "ceiling"), Need::Pack { .. }));
        assert!(matches!(s("CORNER MOUNT OCCUPANCY SENSOR, LOW VOLTAGE", ""), Need::Pack { .. }));
        assert!(matches!(s("DUAL TECH SENSOR 0-10V DIMMING", "ceiling"), Need::Pack { dimming: true, .. }));
        assert!(matches!(s("WALL SWITCH OCCUPANCY SENSOR", "wall"), Need::None(_)));
        assert!(matches!(s("occupancy sensor", "wall"), Need::None(_)));
        // A ceiling sensor its documents call line voltage needs none; the voltage is read first.
        assert!(matches!(s("CEILING OCCUPANCY SENSOR 120/277V", "ceiling"), Need::None(_)));
        // Whole words: a level name is not `LV`.
        assert!(matches!(s("LEVEL SENSOR", "wall"), Need::None(_)));
        // A legend's wall-mounted sensor (a switch's replacement) needs none.
        assert!(matches!(s("WALL MOUNTED OCCUPANCY SENSOR AB-12", "wall"), Need::None(_)));
        // On a wall above switch height: a high-wall sensor; at switch height a wall-box one.
        let mut high = sensor("h", "", "", "occupancy sensor", "wall", [0.0, 0.0]);
        high.height_in = Some(96.0);
        assert!(matches!(need(&r, &high), Need::Pack { .. }));
        high.height_in = Some(48.0);
        assert!(matches!(need(&r, &high), Need::None(_)));
        // The specification's separate power pack governs a sensor its own words don't place, never
        // one its legend makes a wall-box sensor.
        high.spec = "26 09 23: Separate power pack.".into();
        assert!(matches!(need(&r, &high), Need::Pack { .. }));
        let mut wallbox = sensor("w", "", "", "WALL MOUNTED OCCUPANCY SENSOR AB-12", "wall", [0.0, 0.0]);
        wallbox.spec = high.spec.clone();
        assert!(matches!(need(&r, &wallbox), Need::None(_)));
    }

    #[test]
    fn a_corridors_cable_is_its_shortest_tree() {
        // A pack mid-corridor with sensors either side: out each way, not a home run to each.
        let at = |x: f64| Sensor { at: Some([x, 0.0]), ..Sensor::default() };
        let ss = [at(-20.0), at(-10.0), at(10.0), at(20.0)];
        let refs: Vec<&Sensor> = ss.iter().collect();
        assert!((chain_ft(&refs, Some([0.0, 0.0]), 1.0, 25.0) - (40.0 + 4.0)).abs() < 1e-9);
        // No pack location: the default each.
        assert!((chain_ft(&refs, None, 1.0, 25.0) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn the_specification_says_sensors_take_a_pack() {
        let r = rule();
        let lines = |v: &[&str]| v.iter().map(|l| (l.to_string(), "26 09 23".to_string())).collect::<Vec<_>>();
        let said = spec_says(
            &r,
            &lines(&["2.3 INDOOR OCCUPANCY SENSORS", "General Requirements for Sensors:", "Ultrasonic technology.", "Separate power pack."]),
        );
        assert_eq!(said.as_deref(), Some("26 09 23: Separate power pack."));
        // A pack far from any sensor text (another device's) is not the sensors'.
        let mut far = vec!["Occupancy sensors."];
        far.extend(std::iter::repeat_n("filler line", r.spec_context_lines + 1));
        far.push("Relay power pack for the contactor.");
        assert_eq!(spec_says(&r, &lines(&far)), None);
        assert_eq!(spec_says(&r, &lines(&["Wall-box sensors switch line voltage."])), None);
    }

    #[test]
    fn one_pack_per_room_and_leg_beside_the_nearest_luminaire() {
        let r = rule();
        let mut a = sensor("a", "105", "", "occupancy sensor", "ceiling", [10.0, 10.0]);
        a.controls = vec!["l2".into(), "l1".into()];
        let sensors = vec![
            a,
            sensor("b", "105", "", "occupancy sensor", "ceiling", [20.0, 10.0]),
            // Another leg of the room: its own zone.
            sensor("c", "105", "b", "occupancy sensor", "ceiling", [30.0, 10.0]),
            // A wall-box sensor: none.
            sensor("d", "106", "", "wall switch occupancy sensor", "wall", [50.0, 10.0]),
            // A room with a pack the plans draw: none added.
            sensor("e", "107", "", "occupancy sensor", "ceiling", [70.0, 10.0]),
            // No room: a zone of its own each.
            sensor("f", "", "", "occupancy sensor", "ceiling", [90.0, 10.0]),
            sensor("g", "", "", "occupancy sensor", "ceiling", [95.0, 10.0]),
        ];
        let loads = vec![
            Load { handle: "l1".into(), level: "Level 1".into(), room: "105".into(), at: Some([15.0, 12.0]) },
            Load { handle: "l2".into(), level: "Level 1".into(), room: "105".into(), at: Some([40.0, 12.0]) },
            Load { handle: "l3".into(), level: "Level 1".into(), room: "105".into(), at: Some([31.0, 11.0]) },
        ];
        let drawn = vec![("LEVEL 1".to_string(), "107".to_string())];
        let z = zones(&r, &sensors, &loads, &drawn);
        assert_eq!(z.len(), 4, "{z:?}");
        let room = z.iter().find(|z| z.room == "105" && z.leg.is_empty()).cloned().unwrap_or_default();
        assert_eq!(room.sensors, vec!["a".to_string(), "b".to_string()]);
        // Beside the controlled luminaire nearest the sensors (l1, not l2).
        assert_eq!(room.beside.as_deref(), Some("l1"));
        // Cable: the shortest tree from the pack along the axes (a branch to each here, 7' each, the
        // two sensors 10' apart), each sensor with its slack.
        let want = 2.0 * (5.0 + 2.0 + r.cable_slack_ft);
        assert!((room.cable_ft - want).abs() < 1e-6, "{} vs {want}", room.cable_ft);
        // The leg zone takes its room's luminaire in reach (none named).
        let leg = z.iter().find(|z| z.leg == "b").cloned().unwrap_or_default();
        assert_eq!(leg.beside.as_deref(), Some("l3"));
        assert_eq!(z.iter().filter(|z| z.room.is_empty()).count(), 2);
        assert!(z.iter().all(|z| !z.sensors.iter().any(|s| s == "d" || s == "e")));
        assert_eq!(room.item(&r), r.switching_item);
    }
}
