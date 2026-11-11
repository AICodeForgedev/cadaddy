//! A wall-mounted device sits on the wall below its room's finished ceiling (Q6520; trade
//! practice `E-WALL-UNDER-CEILING` in `tradepractice.json`).
//!
//! A wall device is installed where it can be seen, reached and serviced. Its top is at least its
//! own clearance under the ceiling of the room it serves: a legend's or schedule's `x" BELOW
//! CEILING` (an exit sign or emergency light), else the rule's least clearance. Some devices
//! take their height from a default, a rule or a document that doesn't fit the room (a wall
//! sensor's usual 9', a wall luminaire's 7'-6" under a 7'-6" ceiling). If that height puts any
//! part of the device above the ceiling, it is set at the highest allowed place, never left in
//! the plenum. The design height stays beside the corrected one (`design:mount_height`,
//! `correction:mount_height`: the rule and the reason), so the change is never silent.
//!
//! The ceiling over a device is the underside of the smallest built ceiling over the room side
//! of its wall face (its box's middle when it has no face). Failing that, it is its room
//! element's ceiling, then its level's. [`measure`] reads every wall device against it. The
//! build runs [`keep_under`], and the prescreen's [`CHECK`] uses [`measure`]. A device at or over
//! its storey's height (on the roof, on the floor over it) is in no room under this ceiling.
//! Nothing here panics; every scan is capped.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::{self, Prop, Source, SourceKind};
use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

use super::provenance::{self, Correction};

/// The trade practice rule this module follows, and the prescreen check that measures it.
pub(crate) const RULE: &str = "E-WALL-UNDER-CEILING";
/// The prescreen check: wall devices whose top is above their room's ceiling (= 0).
pub(crate) const CHECK: &str = "G-CEILING-CLEAR";
/// The verdicts the check gives.
pub(crate) const VERDICTS: [&str; 3] = ["pass", "fail", "unsure"];
/// The property a device's own clearance under the ceiling is kept in (inches), as a document
/// gives it.
pub(crate) const CLEARANCE_KEY: &str = "ceiling_clearance_in";
/// Most elements looked at (hostile drawings), and most listed in a report.
const MAX_ELEMENTS: usize = 500_000;
const MAX_LISTED: usize = 200;
/// A top this little over its allowed place (inches) is at it.
const SLACK_IN: f64 = 0.05;
/// How far past its wall face the ceiling over a wall device is looked for (inches).
const PROBE_IN: f64 = 6.0;
/// A ceiling lower than this over the floor (inches) is a soffit's or a cabinet's, not a room's.
const LOWEST_CEILING_IN: f64 = 72.0;

/// The rule's numbers and words (`tradepractice.json`).
#[derive(Clone, Debug)]
pub(crate) struct Rule {
    /// The least clearance from a device's top to the ceiling, inches.
    pub min_below_in: f64,
    /// A kind's own clearance: its kinds (spaces) or words, and the inches.
    pub below: Vec<(Vec<String>, Vec<String>, f64)>,
    /// The words after a document's `x"` that give a device's own clearance (`BELOW CEILING`).
    pub legend_words: Vec<String>,
    /// Kinds (spaces, lower case) whose place above a ceiling is their own: an equipment
    /// connection at its equipment, a junction box in the plenum, plumbing.
    pub skip_kinds: Vec<String>,
    /// Where a height comes from (`mount_height_from`) that puts a device somewhere other than on
    /// a wall: at its equipment, at its pole, in the ceiling, on the floor.
    pub skip_from: Vec<String>,
}

impl Rule {
    pub(crate) fn load() -> Rule {
        let v = provenance::rule_data(RULE);
        let list = |k: &str, up: bool| -> Vec<String> {
            v[k].as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|s| if up { s.to_ascii_uppercase() } else { s.to_ascii_lowercase() })
                .collect()
        };
        let below = v["below"]
            .as_array()
            .into_iter()
            .flatten()
            .take(100)
            .filter_map(|r| {
                let inches = r["below_in"].as_f64().filter(|x| x.is_finite() && *x >= 0.0 && *x <= 48.0)?;
                let ks = r["kinds"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|s| s.to_ascii_lowercase().replace('_', " "));
                let ws = r["words"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_ascii_uppercase);
                Some((ks.collect(), ws.collect(), inches))
            })
            .collect();
        Rule {
            min_below_in: v["min_below_in"].as_f64().filter(|x| x.is_finite() && *x >= 0.0 && *x <= 48.0).unwrap_or(2.0),
            below,
            legend_words: list("legend_words", true),
            skip_kinds: list("skip_kinds", false).into_iter().map(|k| k.replace('_', " ")).collect(),
            skip_from: list("skip_from", false),
        }
    }

    /// The clearance a document's words give a device (`EMERGENCY LIGHT 4" BELOW CEILING`),
    /// inches, with the words it was read from.
    pub(crate) fn clearance_in_words(&self, text: &str) -> Option<(f64, String)> {
        let u = super::mountlist::norm(text).to_ascii_uppercase();
        for w in &self.legend_words {
            let Some(i) = u.find(w.as_str()) else { continue };
            // The height written just before the words: the clause's last one.
            let before = u.get(..i).unwrap_or("");
            let clause_start = before.rfind([',', ';', '(']).map_or(0, |j| j + 1);
            let clause = before.get(clause_start..).unwrap_or(before);
            let inches = super::mountlist::dims(clause).last().map(|d| d.inches).filter(|x| x.is_finite() && *x >= 0.0 && *x <= 48.0)?;
            let said = format!("{}{w}", clause.trim_start()).chars().take(120).collect::<String>();
            return Some((inches, said));
        }
        None
    }

    /// A kind's own clearance by the rule (its kinds or words), else the least.
    fn clearance_for(&self, kind: &str, words: &str) -> f64 {
        let k = kind.replace('_', " ").to_ascii_lowercase();
        let up = words.to_ascii_uppercase();
        self.below
            .iter()
            .find(|(ks, ws, _)| ks.contains(&k) || ws.iter().any(|w| up.contains(w.as_str())))
            .map_or(self.min_below_in, |(_, _, i)| *i)
            .max(0.0)
    }

    fn skips(&self, kind: &str) -> bool {
        let k = kind.replace('_', " ").to_ascii_lowercase();
        self.skip_kinds.contains(&k)
    }
}

/// One wall device read against the ceiling over it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Measured {
    pub handle: Handle,
    pub level: String,
    pub kind: String,
    pub mark: Option<String>,
    /// Its top and the ceiling's underside, inches over its floor.
    pub top_in: f64,
    pub ceiling_in: Option<f64>,
    /// Which ceiling: `the ceiling over it`, `its room's ceiling`, `its level's ceiling`.
    pub ceiling_from: &'static str,
    /// Its clearance under the ceiling (inches) and where that came from.
    pub clearance_in: f64,
    pub clearance_from: String,
    /// A person set its height: never moved by the rule.
    pub user: bool,
}

impl Measured {
    /// The highest its top may be, inches over its floor.
    pub(crate) fn allowed_top_in(&self) -> Option<f64> {
        self.ceiling_in.map(|c| c - self.clearance_in)
    }
    /// How far its top is over its allowed place (inches; 0 when under it).
    pub(crate) fn over_in(&self) -> f64 {
        self.allowed_top_in().map_or(0.0, |a| (self.top_in - a).max(0.0))
    }
    /// Whether any part of it is above the ceiling itself.
    pub(crate) fn above_ceiling(&self) -> bool {
        self.ceiling_in.is_some_and(|c| self.top_in > c + SLACK_IN)
    }
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "handle": self.handle.hex(),
            "level": self.level,
            "kind": self.kind,
            "mark": self.mark,
            "top_in": r2(self.top_in),
            "ceiling_in": self.ceiling_in.map(r2),
            "ceiling_from": self.ceiling_from,
            "clearance_in": r2(self.clearance_in),
            "clearance_from": self.clearance_from,
            "over_in": r2(self.over_in()),
        })
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Every wall-mounted device element (its `mounting` wall) against the ceiling over it: its top,
/// the ceiling's underside and its clearance. With `only`, just those elements.
pub(crate) fn measure(d: &Drawing, sp: &BuildingSpec, only: Option<&BTreeSet<Handle>>) -> Vec<Measured> {
    let rule = Rule::load();
    measure_with(d, sp, only, &rule)
}

fn measure_with(d: &Drawing, sp: &BuildingSpec, only: Option<&BTreeSet<Handle>>, rule: &Rule) -> Vec<Measured> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let ceilings = super::propfill::built_ceilings(d);
    // Each room element's ceiling, by its level and number, for a device in a part of its room
    // no built ceiling covers.
    let room_ceiling: HashMap<(String, String), f64> = d
        .model
        .iter()
        .filter(|e| super::wallgen::made_as(e).as_deref() == Some("room"))
        .filter_map(|e| {
            let level = super::bspec::made_level(e)?.trim().to_ascii_uppercase();
            let number = super::bimlook::made_tag(e, "number")?.trim().to_ascii_uppercase();
            let ft = super::bimlook::made_tag(e, "ceiling_ft")?.parse::<f64>().ok().filter(|v| v.is_finite())?;
            Some(((level, number), ft))
        })
        .take(MAX_ELEMENTS)
        .collect();
    let mut out = Vec::new();
    for e in d.model.iter().filter(|e| super::wallgen::made_as(e).as_deref() == Some("device")).take(MAX_ELEMENTS) {
        if only.is_some_and(|o| !o.contains(&e.handle)) {
            continue;
        }
        let ps = props::read(&e.common.xdata);
        let text = |k: &str| ps.get(k).map(Prop::text).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        if text("mounting").as_deref() != Some("wall") {
            continue;
        }
        if text("mount_height_from").is_some_and(|f| rule.skip_from.iter().any(|x| x.eq_ignore_ascii_case(&f))) {
            continue;
        }
        let kind = text("kind").unwrap_or_else(|| "device".into());
        let device_type = text("type").unwrap_or_default();
        if rule.skips(&kind) || rule.skips(&device_type) {
            continue;
        }
        let Some(level) = super::bspec::made_level(e).and_then(|l| sp.level(&l)) else { continue };
        let Some((b, z0, z1)) = super::propfill::extent(d, e) else { continue };
        if !matches!(e.kind, EntityKind::Mesh(_)) || b.is_empty() {
            continue;
        }
        let floor = level.elevation_ft * upf;
        let (bottom_in, top_in) = ((z0 - floor) / upf * 12.0, (z1 - floor) / upf * 12.0);
        if !top_in.is_finite() || !bottom_in.is_finite() {
            continue;
        }
        // At or over its storey's height it stands on the roof or the floor over it (a roof
        // device on its curb or parapet), in no room under this ceiling.
        if level.height_ft.is_finite() && level.height_ft > 0.0 && bottom_in >= level.height_ft * 12.0 - SLACK_IN {
            continue;
        }
        // The room side of its wall face, else its middle; else round it (no face known).
        let c = b.center();
        let probe = PROBE_IN / 12.0 * upf;
        let face = text("face_normal").and_then(|t| {
            let (x, y) = t.split_once(',')?;
            let n = Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?);
            let len = n.len();
            (len.is_finite() && len > 1e-9).then(|| n * (1.0 / len))
        });
        let half = b.width().max(b.height()) / 2.0;
        let over = |p: Vec2| super::propfill::ceiling_over(&ceilings, &level.name, p, floor, upf);
        let found = match face {
            Some(n) => over(c + n * (half + probe)).or_else(|| over(c)),
            None => over(c).or_else(|| {
                [Vec2::new(1.0, 0.0), Vec2::new(-1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(0.0, -1.0)]
                    .iter()
                    .filter_map(|n| over(c + *n * (half + probe)))
                    .min_by(f64::total_cmp)
            }),
        };
        let in_room = || {
            let room = text("room")?.to_ascii_uppercase();
            room_ceiling.get(&(level.name.trim().to_ascii_uppercase(), room)).map(|ft| ft * 12.0).filter(|i| *i > LOWEST_CEILING_IN)
        };
        let (ceiling_in, ceiling_from) = match found {
            Some(i) => (Some(i), "the ceiling over it"),
            None => match in_room() {
                Some(i) => (Some(i), "its room's ceiling"),
                None if level.ceiling_ft.is_finite() && level.ceiling_ft * 12.0 > LOWEST_CEILING_IN => {
                    (Some(level.ceiling_ft * 12.0), "its level's ceiling")
                }
                None => (None, "no ceiling known"),
            },
        };
        let (clearance_in, clearance_from) = match ps.get(CLEARANCE_KEY).and_then(|p| p.value.as_f64().or_else(|| p.text().parse().ok())) {
            Some(i) if i.is_finite() && (0.0..=48.0).contains(&i) => {
                (i, ps.get(CLEARANCE_KEY).and_then(|p| p.source.doc.clone()).unwrap_or_else(|| "its documents".into()))
            }
            _ => (rule.clearance_for(&kind, &device_type), format!("{RULE}: the least clearance under the ceiling")),
        };
        let user = ps.get("mount_height").is_some_and(|p| p.source.kind == SourceKind::User);
        out.push(Measured {
            handle: e.handle,
            level: level.name.clone(),
            kind,
            mark: text("type_mark"),
            top_in,
            ceiling_in,
            ceiling_from,
            clearance_in,
            clearance_from,
            user,
        });
    }
    out
}

/// What [`keep_under`] did.
#[derive(Clone, Debug, Default)]
pub(crate) struct Lowered {
    /// Wall devices read against their ceiling.
    pub checked: usize,
    /// Set lower, their tops at their clearance under the ceiling.
    pub lowered: usize,
    /// Over their place but a person set their height: left, listed.
    pub user_kept: usize,
    /// By kind, and the first ones.
    pub by_kind: BTreeMap<String, usize>,
    pub listed: Vec<Value>,
}

impl Lowered {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "rule": RULE,
            "checked": self.checked,
            "lowered": self.lowered,
            "user_kept": self.user_kept,
            "by_kind": self.by_kind,
            "first": self.listed.iter().take(50).collect::<Vec<_>>(),
        })
    }
}

/// Every wall device (with `only`, those elements) whose top is over its allowed place under its
/// room's ceiling set there: its box moves down, its height takes the corrected value, and the
/// design value, the rule and the reason are kept beside it. A height a person set stays (listed).
pub(crate) fn keep_under(d: &mut Drawing, sp: &BuildingSpec, only: Option<&BTreeSet<Handle>>) -> Lowered {
    let rule = Rule::load();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let all = measure_with(d, sp, only, &rule);
    let mut out = Lowered { checked: all.len(), ..Lowered::default() };
    for m in all.iter().filter(|m| m.over_in() > SLACK_IN) {
        if m.user {
            out.user_kept += 1;
            continue;
        }
        let (Some(ceiling), Some(allowed)) = (m.ceiling_in, m.allowed_top_in()) else { continue };
        let dz_in = allowed - m.top_in;
        let dz = dz_in / 12.0 * upf;
        let Some(e) = d.model.get(m.handle) else { continue };
        let mut ps = props::read(&e.common.xdata);
        let reason = format!(
            "its top at {} would be {} {} its room's finished ceiling ({}, {}): set with its top {} under it ({})",
            ft_in(m.top_in),
            ft_in((m.top_in - ceiling).abs()),
            if m.top_in > ceiling { "above" } else { "short of its clearance under" },
            ft_in(ceiling),
            m.ceiling_from,
            ft_in(m.clearance_in),
            m.clearance_from
        );
        if let Some(old) = ps.get("mount_height").cloned()
            && let Some(h) = old.value.as_f64().or_else(|| old.text().parse().ok()).filter(|h| h.is_finite())
        {
            let new = r2(h + dz_in);
            let design = old.text();
            ps.set("mount_height", Prop { value: json!(new), ..old.clone() });
            let c = Correction { key: "mount_height".into(), design, corrected: format!("{new}"), reason: reason.clone(), rule: RULE.into() };
            provenance::correct(&mut ps, &c, old.source.clone());
            // The design height kept as the number it was (a first correction's only).
            let dkey = format!("{}mount_height", provenance::DESIGN_PREFIX);
            if let Some(kept) = ps.get(&dkey).cloned().filter(|p| p.value.is_string() && p.text().trim() == c.design.trim()) {
                ps.set(&dkey, Prop { value: json!(h), unit: old.unit.clone(), ..kept });
            }
            let src = Source::from_doc(SourceKind::Rule, RULE);
            ps.set("offset", Prop::new(r2(new / 12.0), src).with_unit("ft"));
        }
        if let Some(loc) = ps.get("location").cloned() {
            let xyz: Vec<f64> = loc.text().split(',').filter_map(|v| v.trim().parse().ok()).collect();
            if let [x, y, z] = xyz[..] {
                let p = Prop { value: json!(format!("{x:.2}, {y:.2}, {:.2}", z + dz / upf)), ..loc };
                ps.set("location", p);
            }
        }
        let x = props::write(&e.common.xdata, &ps);
        let moved = d.modify_entity(m.handle, |e| {
            if let EntityKind::Mesh(mesh) = &mut e.kind {
                for v in mesh.vertices.iter_mut().take(1_000_000) {
                    v.z += dz;
                }
            }
            e.common.xdata = x.into();
        });
        if moved.is_ok() {
            out.lowered += 1;
            *out.by_kind.entry(m.kind.clone()).or_default() += 1;
            if out.listed.len() < MAX_LISTED {
                let mut j = m.to_json();
                j["lowered_in"] = json!(r2(-dz_in));
                out.listed.push(j);
            }
        }
    }
    out
}

/// Inches as feet and inches (`7'-6"`, `3"`).
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

    #[test]
    fn a_documents_clearance_under_the_ceiling_is_read() {
        let r = Rule::load();
        assert!(r.min_below_in > 0.0, "the rule is in the trade practice data");
        let got = |t: &str| r.clearance_in_words(t).map(|x| x.0);
        assert_eq!(got("EMERGENCY LIGHT, WALL MOUNTED 4\" BELOW CEILING"), Some(4.0));
        assert_eq!(got("EXIT SIGN, MOUNT 6 IN BELOW FINISHED CEILING"), Some(6.0));
        assert_eq!(got("WALL PACK 1'-0\" BELOW CLG"), Some(12.0));
        // A height above the floor in another clause is not its clearance.
        assert_eq!(got("SCONCE 80\" AFF, BELOW CEILING"), None);
        assert_eq!(got("OCCUPANCY SENSOR, WALL MOUNTED"), None);
        assert_eq!(ft_in(90.0), "7'-6\"");
        assert_eq!(ft_in(3.0), "3\"");
    }

    #[test]
    fn a_device_over_its_place_is_measured_against_the_lowest_of_its_clearances() {
        let m = Measured {
            handle: Handle(1),
            level: "L1".into(),
            kind: "occupancy_sensor".into(),
            mark: None,
            top_in: 110.0,
            ceiling_in: Some(90.0),
            ceiling_from: "the ceiling over it",
            clearance_in: 2.0,
            clearance_from: String::new(),
            user: false,
        };
        assert_eq!(m.allowed_top_in(), Some(88.0));
        assert!((m.over_in() - 22.0).abs() < 1e-9 && m.above_ceiling());
        let under = Measured { top_in: 88.0, ..m.clone() };
        assert!(under.over_in() < 1e-9 && !under.above_ceiling());
        let unknown = Measured { ceiling_in: None, ..m };
        assert_eq!(unknown.over_in(), 0.0);
    }
}
