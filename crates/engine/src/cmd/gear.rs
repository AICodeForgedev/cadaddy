//! Electrical gear review: panels, switchboards, transformers and disconnects (`mep.equipment`
//! with an electrical kind) checked against the feeders that land on them (`mep.feeders`), before
//! the gear is ordered.
//!
//! - **Lugs**: every termination point records what its lugs take (lugs per phase or positions on
//!   a bar, wire range, CU/AL rating, mechanical or compression, supplied or ordered separately).
//!   Each feeder end is checked: conductors per phase against the lugs, its size against the wire
//!   range, aluminium on CU-only lugs; the neutral and ground bars against every neutral,
//!   equipment grounding conductor, grounding electrode conductor and bonding jumper landing there.
//! - **Lug kits**: lugs ordered separately (compression lugs on a transformer's pads) are listed
//!   with their bolt kits (bolts by diameter and length for the stack, nuts, flat and Belleville
//!   washers), so they are ordered with the gear; they go on the MEP takeoff as required items.
//! - **Installability**: the raceways coming in through each entry plate (count × size, with room
//!   for locknuts and bushings) against the plate; the conductors in the wiring gutter against its
//!   fill; the wire-bending space at each terminal against the table for its conductor size.
//!   Each failure says what would fix it (a wider or deeper enclosure, an auxiliary gutter or
//!   wireway, a pull or termination section, splitting the feed top and bottom).
//! - `gear.review` turns the failures into RFIs (submittal review, before release for
//!   manufacture); `gear.accept` records a failure accepted as it is, which the takeoff carries as
//!   a labor condition (a factor on the installation).
//!
//! No load, voltage-drop or code calculations: what lands where and whether it fits. The tables
//! (raceway sizes, conductor areas, bending space, gutter fill) are data with defaults; a project
//! gives its own in `mep.gear_rules`.

use std::collections::BTreeMap;

use cadaddy_walk3d::{BuildingSpec, Feeder, MepEquipment, Termination};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Most findings a check returns.
const MAX_FINDINGS: usize = 10_000;

// ---------------- sizes and tables ----------------

/// Circular mils of the AWG sizes, smallest first.
const AWG: [(&str, u64); 15] = [
    ("14", 4_110),
    ("12", 6_530),
    ("10", 10_380),
    ("8", 16_510),
    ("6", 26_240),
    ("4", 41_740),
    ("3", 52_620),
    ("2", 66_360),
    ("1", 83_690),
    ("1/0", 105_600),
    ("2/0", 133_100),
    ("3/0", 167_800),
    ("4/0", 211_600),
    ("5", 0),
    ("7", 0),
];

/// A conductor size as printed (`#2`, `2 AWG`, `3/0`, `500 kcmil`, `500 MCM`, `500`) in circular
/// mils.
pub fn cmil(size: &str) -> Option<u64> {
    let s = size.trim().to_ascii_lowercase().replace(['#', '"'], "");
    let kc = s.contains("kcmil") || s.contains("mcm");
    let bare = s.replace("kcmil", "").replace("mcm", "").replace("awg", "").replace("cu", "").replace("al", "");
    let bare = bare.trim();
    if bare.contains('/') {
        return AWG.iter().find(|(n, _)| *n == bare).map(|(_, c)| *c).filter(|c| *c > 0);
    }
    let n: f64 = bare.parse().ok().filter(|v: &f64| v.is_finite() && *v > 0.0)?;
    if kc || n >= 250.0 {
        return (n <= 5_000.0).then(|| (n * 1000.0).round() as u64);
    }
    AWG.iter().find(|(a, _)| a.parse::<f64>().ok() == Some(n)).map(|(_, c)| *c).filter(|c| *c > 0)
}

/// A size in words (`#2`, `3/0`, `500 kcmil`).
pub fn size_label(c: u64) -> String {
    match AWG.iter().find(|(_, x)| *x == c) {
        Some((n, _)) if n.contains('/') => n.to_string(),
        Some((n, _)) => format!("#{n}"),
        None => format!("{} kcmil", c / 1000),
    }
}

/// A wire range as printed (`#6 - 350 kcmil`, `1/0-750`, `#14 to 1/0`): smallest and largest.
pub fn range(r: &str) -> Option<(u64, u64)> {
    let t = r.trim().to_ascii_lowercase();
    let (a, b) = t.split_once(" to ").or_else(|| t.split_once('-'))?;
    let lo = cmil(a)?;
    let hi = cmil(b)?;
    (lo <= hi).then_some((lo, hi))
}

/// The tables gear is checked with.
#[derive(Clone, Debug)]
pub struct Rules {
    /// Raceway trade size → outside diameter, inches.
    pub conduit_od: Vec<(f64, f64)>,
    /// Room round each raceway at an entry plate (locknut, bushing, the punch), inches.
    pub spacing_in: f64,
    /// Conductor size (circular mils) → area with its insulation, square inches.
    pub areas: Vec<(u64, f64)>,
    /// Wire-bending space at terminals, inches, by the largest size and conductors per terminal
    /// (1 to 5): conductors not entering the wall opposite (`adjacent`) and entering it (`opposite`).
    pub bending_adjacent: Vec<(u64, [f64; 5])>,
    pub bending_opposite: Vec<(u64, [f64; 5])>,
    /// Share of a wiring gutter's cross-section the conductors may fill.
    pub gutter_fill: f64,
    /// Bolts for separately supplied lugs.
    pub bolt_grade: String,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            conduit_od: vec![
                (0.5, 0.84),
                (0.75, 1.05),
                (1.0, 1.315),
                (1.25, 1.66),
                (1.5, 1.9),
                (2.0, 2.375),
                (2.5, 2.875),
                (3.0, 3.5),
                (3.5, 4.0),
                (4.0, 4.5),
                (5.0, 5.563),
                (6.0, 6.625),
            ],
            spacing_in: 0.75,
            areas: vec![
                (4_110, 0.0097),
                (6_530, 0.0133),
                (10_380, 0.0211),
                (16_510, 0.0366),
                (26_240, 0.0507),
                (41_740, 0.0824),
                (52_620, 0.0973),
                (66_360, 0.1158),
                (83_690, 0.1562),
                (105_600, 0.1855),
                (133_100, 0.2223),
                (167_800, 0.2679),
                (211_600, 0.3237),
                (250_000, 0.397),
                (300_000, 0.4608),
                (350_000, 0.5242),
                (400_000, 0.5863),
                (500_000, 0.7073),
                (600_000, 0.8676),
                (700_000, 0.9887),
                (750_000, 1.0496),
                (1_000_000, 1.3478),
            ],
            bending_adjacent: vec![
                (16_510, [1.5, 0.0, 0.0, 0.0, 0.0]),
                (26_240, [1.5, 0.0, 0.0, 0.0, 0.0]),
                (52_620, [2.0, 0.0, 0.0, 0.0, 0.0]),
                (66_360, [2.5, 0.0, 0.0, 0.0, 0.0]),
                (83_690, [3.0, 0.0, 0.0, 0.0, 0.0]),
                (133_100, [3.5, 5.0, 7.0, 0.0, 0.0]),
                (211_600, [4.0, 6.0, 8.0, 0.0, 0.0]),
                (250_000, [4.5, 6.0, 8.0, 10.0, 0.0]),
                (350_000, [5.0, 8.0, 10.0, 12.0, 0.0]),
                (500_000, [6.0, 8.0, 10.0, 12.0, 0.0]),
                (700_000, [8.0, 10.0, 12.0, 14.0, 16.0]),
                (900_000, [8.0, 12.0, 14.0, 16.0, 18.0]),
                (1_250_000, [10.0, 0.0, 0.0, 0.0, 0.0]),
            ],
            bending_opposite: vec![
                (52_620, [3.0, 0.0, 0.0, 0.0, 0.0]),
                (66_360, [3.5, 0.0, 0.0, 0.0, 0.0]),
                (83_690, [4.5, 0.0, 0.0, 0.0, 0.0]),
                (105_600, [5.5, 5.5, 7.0, 0.0, 0.0]),
                (133_100, [6.0, 6.0, 7.5, 0.0, 0.0]),
                (167_800, [6.5, 6.5, 8.0, 0.0, 0.0]),
                (211_600, [7.0, 7.5, 8.5, 0.0, 0.0]),
                (250_000, [8.5, 8.5, 9.0, 10.0, 0.0]),
                (300_000, [10.0, 10.0, 11.0, 12.0, 0.0]),
                (350_000, [12.0, 12.0, 13.0, 14.0, 0.0]),
                (400_000, [13.0, 13.0, 14.0, 15.0, 0.0]),
                (500_000, [14.0, 14.0, 15.0, 16.0, 18.0]),
                (600_000, [15.0, 16.0, 18.0, 19.0, 22.0]),
                (700_000, [16.0, 18.0, 20.0, 22.0, 24.0]),
                (750_000, [17.0, 19.0, 22.0, 24.0, 24.0]),
                (900_000, [19.0, 21.0, 24.0, 24.0, 24.0]),
                (1_000_000, [20.0, 22.0, 24.0, 24.0, 24.0]),
            ],
            gutter_fill: 0.4,
            bolt_grade: "SAE grade 5".into(),
        }
    }
}

impl Rules {
    /// The defaults, with what the spec's `mep.gear_rules` gives instead: `spacing_in`,
    /// `gutter_fill`, `bolt_grade`, `conduit_od: [[trade, od]]`, `areas: [[size, in²]]`,
    /// `bending_adjacent` / `bending_opposite: [[size, [1..5 conductors]]]`.
    pub fn of(sp: &BuildingSpec) -> Rules {
        let mut r = Rules::default();
        let Some(g) = sp.mep.gear_rules.as_ref() else { return r };
        if let Some(v) = g.get("spacing_in").and_then(Value::as_f64).filter(|v| v.is_finite() && (0.0..=6.0).contains(v)) {
            r.spacing_in = v;
        }
        if let Some(v) = g.get("gutter_fill").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0 && *v <= 1.0) {
            r.gutter_fill = v;
        }
        if let Some(v) = g.get("bolt_grade").and_then(Value::as_str) {
            r.bolt_grade = v.chars().take(60).collect();
        }
        let pairs = |k: &str| -> Option<Vec<(f64, f64)>> {
            let a = g.get(k)?.as_array()?;
            let v: Vec<(f64, f64)> = a
                .iter()
                .take(200)
                .filter_map(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)))
                .filter(|(a, b)| a.is_finite() && b.is_finite() && *a > 0.0 && *b > 0.0)
                .collect();
            (!v.is_empty()).then_some(v)
        };
        if let Some(v) = pairs("conduit_od") {
            r.conduit_od = v;
        }
        if let Some(a) = g.get("areas").and_then(Value::as_array) {
            let v: Vec<(u64, f64)> = a
                .iter()
                .take(200)
                .filter_map(|p| Some((cmil(p.get(0)?.as_str()?)?, p.get(1)?.as_f64()?)))
                .filter(|(_, b)| b.is_finite() && *b > 0.0)
                .collect();
            if !v.is_empty() {
                r.areas = v;
            }
        }
        for (key, slot) in [("bending_adjacent", &mut r.bending_adjacent), ("bending_opposite", &mut r.bending_opposite)] {
            if let Some(a) = g.get(key).and_then(Value::as_array) {
                let v: Vec<(u64, [f64; 5])> = a
                    .iter()
                    .take(200)
                    .filter_map(|p| {
                        let c = cmil(p.get(0)?.as_str()?)?;
                        let row = p.get(1)?.as_array()?;
                        let mut out = [0.0; 5];
                        for (k, x) in row.iter().take(5).enumerate() {
                            if let Some(slot) = out.get_mut(k) {
                                *slot = x.as_f64().filter(|v| v.is_finite() && *v >= 0.0)?;
                            }
                        }
                        Some((c, out))
                    })
                    .collect();
                if !v.is_empty() {
                    *slot = v;
                }
            }
        }
        r.areas.sort_by_key(|(c, _)| *c);
        r.bending_adjacent.sort_by_key(|(c, _)| *c);
        r.bending_opposite.sort_by_key(|(c, _)| *c);
        r.conduit_od.sort_by(|a, b| a.0.total_cmp(&b.0));
        r
    }

    /// A raceway's outside diameter (its trade size's, else the next size up's).
    pub fn od(&self, trade: f64) -> f64 {
        self.conduit_od.iter().find(|(t, _)| *t + 1e-6 >= trade).map_or(trade * 1.15, |(_, od)| *od)
    }

    /// A conductor's area with insulation (its size's, else the next size up's).
    fn area(&self, c: u64) -> f64 {
        self.areas.iter().find(|(x, _)| *x >= c).or(self.areas.last()).map_or(0.0, |(_, a)| *a)
    }

    /// The wire-bending space a terminal needs for `c` circular mils, `n` per terminal.
    fn bending(&self, c: u64, n: u32, opposite: bool) -> Option<f64> {
        let table = if opposite { &self.bending_opposite } else { &self.bending_adjacent };
        let row = table.iter().find(|(x, _)| *x >= c)?;
        let k = usize::try_from(n.clamp(1, 5) - 1).unwrap_or(0);
        // Past the table's own columns, the last one given.
        row.1.iter().take(k + 1).rev().find(|v| **v > 0.0).copied()
    }
}

// ---------------- the gear and what lands on it ----------------

/// A count from the spec, kept to what a review can mean (hostile files ask for billions).
fn n(v: u32) -> u32 {
    v.min(1000)
}

/// Electrical gear: an equipment kind that is one, or anything with terminations.
pub fn is_gear(e: &MepEquipment) -> bool {
    matches!(e.kind.to_ascii_lowercase().as_str(), "panel" | "panelboard" | "switchboard" | "switchgear" | "transformer" | "disconnect")
        || !e.terminations.is_empty()
}

/// Where a feeder lands at its load end and leaves its source, by the gear's kind.
fn point_at(e: &MepEquipment, f: &Feeder, incoming: bool) -> String {
    let given = if incoming { &f.to_point } else { &f.from_point };
    if !given.trim().is_empty() {
        return given.trim().to_ascii_lowercase();
    }
    match (e.kind.to_ascii_lowercase().as_str(), incoming) {
        ("transformer", true) => "primary",
        ("transformer", false) => "secondary",
        ("disconnect", true) => "line",
        ("disconnect", false) => "load",
        (_, true) => "main",
        (_, false) => "breaker",
    }
    .into()
}

fn termination<'a>(e: &'a MepEquipment, point: &str) -> Option<&'a Termination> {
    e.terminations.iter().find(|t| t.point.trim().eq_ignore_ascii_case(point))
}

/// One thing the review found.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub gear: String,
    /// `lug count`, `lug range`, `lug material`, `neutral bar`, `ground bar`, `entry`, `gutter`,
    /// `bending`, `lug data`.
    pub check: &'static str,
    /// `error` (fails review), `warning`, `accepted` (an error accepted as it is).
    pub severity: &'static str,
    pub feeder: String,
    pub message: String,
    pub needed: String,
    pub available: String,
    pub fix: String,
}

impl Finding {
    fn json(&self) -> Value {
        json!({
            "gear": self.gear, "check": self.check, "severity": self.severity, "feeder": self.feeder,
            "message": self.message, "needed": self.needed, "available": self.available, "fix": self.fix,
        })
    }

    fn installability(&self) -> bool {
        matches!(self.check, "entry" | "gutter" | "bending")
    }
}

/// A lug kit to order with the gear: separately supplied lugs and their bolts.
#[derive(Clone, Debug, PartialEq)]
pub struct LugKit {
    pub gear: String,
    pub point: String,
    pub lugs: u32,
    pub lug: String,
    pub bolts: u32,
    pub bolt: String,
    pub nuts: u32,
    pub flat_washers: u32,
    pub belleville_washers: u32,
}

impl LugKit {
    fn json(&self) -> Value {
        json!({
            "gear": self.gear, "point": self.point, "lugs": self.lugs, "lug": self.lug, "bolts": self.bolts, "bolt": self.bolt,
            "nuts": self.nuts, "flat_washers": self.flat_washers, "belleville_washers": self.belleville_washers,
        })
    }
}

/// A fraction of an inch as a fitter writes it (`1/2`, `2-1/4`).
fn inches(v: f64) -> String {
    let q = (v * 16.0).round() as i64;
    let (whole, frac) = (q / 16, q % 16);
    let g = |mut a: i64, mut b: i64| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let d = g(frac, 16).max(1);
    match (whole, frac) {
        (w, 0) => format!("{w}\""),
        (0, f) => format!("{}/{}\"", f / d, 16 / d),
        (w, f) => format!("{w}-{}/{}\"", f / d, 16 / d),
    }
}

/// The bolt for a lug on a pad: through the stack, a flat washer each side, a Belleville
/// washer, the nut and two threads showing, up to the next quarter inch.
fn bolt_length(t: &Termination) -> f64 {
    let dia = if t.bolt_in.is_finite() && t.bolt_in > 0.0 { t.bolt_in } else { 0.5 };
    let stack = if t.stack_in.is_finite() && t.stack_in > 0.0 { t.stack_in } else { 0.75 };
    let raw = stack + 2.0 * 0.1 + 0.12 + dia + 2.0 / 13.0;
    (raw * 4.0).ceil() / 4.0
}

/// What the review finds on the spec's gear (one tag, or all), with the lug kits to order.
pub fn review(sp: &BuildingSpec, only: Option<&str>) -> (Vec<Finding>, Vec<LugKit>) {
    let rules = Rules::of(sp);
    let mut out: Vec<Finding> = Vec::new();
    let mut kits: Vec<LugKit> = Vec::new();
    for g in sp.mep.equipment.iter().filter(|e| is_gear(e)).filter(|e| only.is_none_or(|t| e.tag.eq_ignore_ascii_case(t))) {
        let mut add = |check: &'static str, error: bool, feeder: &str, message: String, needed: String, available: String, fix: &str| {
            if out.len() >= MAX_FINDINGS {
                return;
            }
            let severity = if !error {
                "warning"
            } else if g.accepted.iter().any(|a| a.eq_ignore_ascii_case(check)) {
                "accepted"
            } else {
                "error"
            };
            out.push(Finding { gear: g.tag.clone(), check, severity, feeder: feeder.into(), message, needed, available, fix: fix.into() });
        };
        // Every feeder end at this gear: (feeder, landing here as its load end?).
        let mut ends: Vec<(&Feeder, bool)> = Vec::new();
        for f in sp.mep.feeders.iter().take(MAX_FINDINGS) {
            if f.to.trim().eq_ignore_ascii_case(&g.tag) {
                ends.push((f, true));
            } else if f.from.trim().eq_ignore_ascii_case(&g.tag) {
                ends.push((f, false));
            }
        }
        if ends.is_empty() {
            continue;
        }
        if g.terminations.is_empty() {
            add(
                "lug data",
                false,
                "",
                format!("{} has no lug data: its lugs can't be checked", g.tag),
                String::new(),
                String::new(),
                "Get the lug provisions from the gear schedule or the submittal",
            );
        }
        // Phase conductors on their lugs.
        for (f, incoming) in &ends {
            let point = point_at(g, f, *incoming);
            let Some(t) = termination(g, &point) else {
                if !g.terminations.is_empty() {
                    add(
                        "lug data",
                        false,
                        &f.tag,
                        format!("{} lands on {}'s {point} lugs, which the lug data doesn't give", f.tag, g.tag),
                        String::new(),
                        String::new(),
                        "Get the lug provisions for this point",
                    );
                }
                continue;
            };
            let per = t.wires_per_lug.max(1);
            let need = n(f.sets).div_ceil(per);
            if need > t.lugs {
                add(
                    "lug count",
                    true,
                    &f.tag,
                    format!("{}: {} sets per phase need {need} lugs per phase on {}'s {point}; it has {}", f.tag, n(f.sets), g.tag, t.lugs),
                    format!("{need} per phase"),
                    format!("{} per phase", t.lugs),
                    "Order the gear with more lugs (or a lug kit for parallel sets), or a termination section",
                );
            }
            let size = cmil(&f.size);
            match (size, range(&t.range)) {
                (Some(c), Some((lo, hi))) if c < lo || c > hi => add(
                    "lug range",
                    true,
                    &f.tag,
                    format!("{}: {} {} is outside {}'s {point} lugs ({})", f.tag, size_label(c), f.material.to_uppercase(), g.tag, t.range),
                    size_label(c),
                    t.range.clone(),
                    "Order lugs for the conductor size, or change the conductors",
                ),
                (None, _) => add(
                    "lug range",
                    false,
                    &f.tag,
                    format!("{}: the size {} isn't one this check reads", f.tag, f.size),
                    f.size.clone(),
                    t.range.clone(),
                    "",
                ),
                (_, None) if !t.range.trim().is_empty() => add(
                    "lug range",
                    false,
                    &f.tag,
                    format!("{}'s {point} wire range {} isn't one this check reads", g.tag, t.range),
                    String::new(),
                    t.range.clone(),
                    "",
                ),
                _ => {}
            }
            if f.material.eq_ignore_ascii_case("al") && t.material.trim().eq_ignore_ascii_case("cu") {
                add(
                    "lug material",
                    true,
                    &f.tag,
                    format!("{}: aluminium conductors on {}'s CU-only {point} lugs", f.tag, g.tag),
                    "AL-rated (AL9CU) lugs".into(),
                    "CU only".into(),
                    "Order AL-rated lugs with the gear",
                );
            }
            if !t.supplied
                && let Some(c) = size
            {
                let lugs = n(f.phases.max(1)).saturating_mul(n(f.sets));
                let bolts = lugs.saturating_mul(n(t.holes.max(1)));
                kits.push(LugKit {
                    gear: g.tag.clone(),
                    point: point.clone(),
                    lugs,
                    lug: format!(
                        "{} {}, {}-hole, {}",
                        t.lug_type,
                        size_label(c),
                        t.holes.max(1),
                        if f.material.eq_ignore_ascii_case("al") { "AL" } else { "CU" }
                    ),
                    bolts,
                    bolt: format!("{} x {} {}", inches(t.bolt_in), inches(bolt_length(t)), rules.bolt_grade),
                    nuts: bolts,
                    flat_washers: bolts.saturating_mul(2),
                    belleville_washers: bolts,
                });
            }
        }
        // The neutral and ground bars: every conductor landing on them.
        for (bar, check) in [("neutral_bar", "neutral bar"), ("ground_bar", "ground bar")] {
            let mut sizes: Vec<(String, u64)> = Vec::new();
            for (f, incoming) in &ends {
                // A neutral per set; an equipment grounding conductor in every raceway.
                let count = if bar == "neutral_bar" {
                    n(f.neutrals).saturating_mul(n(f.sets))
                } else {
                    n(f.grounds).saturating_mul(n(if f.conduits > 0 { f.conduits } else { f.sets }))
                };
                let label =
                    if bar == "neutral_bar" { if f.neutral_size.trim().is_empty() { &f.size } else { &f.neutral_size } } else { &f.ground_size };
                for _ in 0..count.min(400) {
                    sizes.push((f.tag.clone(), cmil(label).unwrap_or(0)));
                }
                if bar == "ground_bar" && *incoming {
                    for x in f.extra_grounds.iter().take(50) {
                        sizes.push((format!("{} (GEC/bonding)", f.tag), cmil(x).unwrap_or(0)));
                    }
                }
            }
            if sizes.is_empty() {
                continue;
            }
            let Some(t) = termination(g, bar) else {
                if !g.terminations.is_empty() {
                    add(
                        check,
                        false,
                        "",
                        format!("{} conductors land on {}'s {check}, which the lug data doesn't give", sizes.len(), g.tag),
                        sizes.len().to_string(),
                        String::new(),
                        "Get the bar's positions and wire range",
                    );
                }
                continue;
            };
            let need = u32::try_from(sizes.len()).unwrap_or(u32::MAX).div_ceil(t.wires_per_lug.max(1));
            if need > t.lugs {
                add(
                    check,
                    true,
                    "",
                    format!("{} conductors land on {}'s {check}: {need} positions needed, it has {}", sizes.len(), g.tag, t.lugs),
                    format!("{need} positions"),
                    format!("{} positions", t.lugs),
                    if bar == "neutral_bar" {
                        "Order a larger or second neutral bar (or a neutral lug kit) with the gear"
                    } else {
                        "Order a ground bar kit with enough positions for the parallel grounds and bonding with the gear"
                    },
                );
            }
            if let Some((lo, hi)) = range(&t.range) {
                let bad: Vec<String> =
                    sizes.iter().filter(|(_, c)| *c > 0 && (*c < lo || *c > hi)).map(|(tag, c)| format!("{tag} {}", size_label(*c))).collect();
                if let Some(first) = bad.first() {
                    add(
                        check,
                        true,
                        "",
                        format!("{} conductors on {}'s {check} are outside its range ({}): {first}…", bad.len(), g.tag, t.range),
                        first.clone(),
                        t.range.clone(),
                        "Order bar lugs for those sizes",
                    );
                }
            }
        }
        // Installability: entry plates, gutter, bending space.
        if let Some(enc) = &g.enclosure {
            for side in ["top", "bottom"] {
                let plate = if side == "top" { enc.top_entry_in } else { enc.bottom_entry_in };
                let mut pitches: Vec<f64> = Vec::new();
                for (f, incoming) in &ends {
                    let e = if *incoming || f.from_entry.trim().is_empty() { &f.entry } else { &f.from_entry };
                    if e.trim().eq_ignore_ascii_case(side) && f.conduit_in > 0.0 {
                        for _ in 0..f.conduits.min(400) {
                            pitches.push(rules.od(f.conduit_in) + rules.spacing_in);
                        }
                    }
                }
                if pitches.is_empty() {
                    continue;
                }
                let (w, d) = (plate[0].max(0.0), plate[1].max(0.0));
                // Shelf packing: the largest first, rows across the plate's width.
                pitches.sort_by(|a, b| b.total_cmp(a));
                let (mut used_d, mut row_w, mut row_d, mut fits) = (0.0f64, 0.0f64, 0.0f64, true);
                for p in &pitches {
                    if *p > w || *p > d {
                        fits = false;
                        break;
                    }
                    if row_w + p > w {
                        used_d += row_d;
                        (row_w, row_d) = (0.0, 0.0);
                    }
                    row_w += p;
                    row_d = row_d.max(*p);
                }
                used_d += row_d;
                if !fits || used_d > d + 1e-9 {
                    let need: f64 = pitches.iter().map(|p| p * p).sum();
                    add(
                        "entry",
                        true,
                        "",
                        format!(
                            "{} raceways come in through {}'s {side}: about {:.0} sq in of plate needed, {:.0} sq in usable ({w:.0}\" x {d:.0}\")",
                            pitches.len(),
                            g.tag,
                            need,
                            w * d
                        ),
                        format!("{} raceways, {:.0} sq in", pitches.len(), need),
                        format!("{w:.0}\" x {d:.0}\""),
                        "A wider or deeper enclosure, a pull section, or split the feed between top and bottom",
                    );
                }
            }
            let gutter = enc.gutter_in[0].max(0.0) * enc.gutter_in[1].max(0.0);
            if gutter > 0.0 {
                let fill: f64 = ends
                    .iter()
                    .map(|(f, _)| {
                        let phase = rules.area(cmil(&f.size).unwrap_or(0)) * f64::from(n(f.phases).saturating_mul(n(f.sets)));
                        let neutral = rules.area(cmil(if f.neutral_size.is_empty() { &f.size } else { &f.neutral_size }).unwrap_or(0))
                            * f64::from(n(f.neutrals).saturating_mul(n(f.sets)));
                        let ground = rules.area(cmil(&f.ground_size).unwrap_or(0)) * f64::from(n(f.grounds).saturating_mul(n(f.sets)));
                        phase + neutral + ground
                    })
                    .sum();
                let allowed = gutter * rules.gutter_fill;
                if fill > allowed {
                    add(
                        "gutter",
                        true,
                        "",
                        format!(
                            "{}'s gutter: {fill:.1} sq in of conductors, {allowed:.1} sq in allowed ({:.0}% of {:.0}\" x {:.0}\")",
                            g.tag,
                            rules.gutter_fill * 100.0,
                            enc.gutter_in[0],
                            enc.gutter_in[1]
                        ),
                        format!("{fill:.1} sq in"),
                        format!("{allowed:.1} sq in"),
                        "An auxiliary gutter or wireway beside it, or a wider enclosure",
                    );
                }
            }
        }
        for (f, incoming) in &ends {
            let point = point_at(g, f, *incoming);
            let Some(t) = termination(g, &point).filter(|t| t.bending_in > 0.0) else { continue };
            let Some(c) = cmil(&f.size) else { continue };
            let per = t.wires_per_lug.max(1).min(n(f.sets).max(1));
            if let Some(need) = rules.bending(c, per, t.opposite)
                && need > t.bending_in + 1e-9
            {
                add(
                    "bending",
                    true,
                    &f.tag,
                    format!(
                        "{}: {} {} per terminal at {}'s {point} needs {need}\" to bend, {}\" given",
                        f.tag,
                        per,
                        size_label(c),
                        g.tag,
                        t.bending_in
                    ),
                    format!("{need}\""),
                    format!("{}\"", t.bending_in),
                    "A deeper enclosure or a pull/termination section at the lugs",
                );
            }
        }
    }
    (out, kits)
}

/// A labor condition: gear accepted with review failures (harder to install), and the factor.
pub fn labor(sp: &BuildingSpec) -> Vec<Value> {
    let (findings, _) = review(sp, None);
    let mut by: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    for f in findings.iter().filter(|f| f.severity == "accepted") {
        let v = by.entry(f.gear.clone()).or_default();
        if !v.contains(&f.check) {
            v.push(f.check);
        }
    }
    by.into_iter()
        .map(|(tag, checks)| {
            let factor: f64 = checks.iter().map(|c| if matches!(*c, "entry" | "gutter") { 1.15 } else { 1.1 }).product();
            json!({"gear": tag, "conditions": checks, "factor": (factor * 100.0).round() / 100.0})
        })
        .collect()
}

/// The lug kits the takeoff carries as required items.
pub fn lug_kits(sp: &BuildingSpec) -> Vec<Value> {
    review(sp, None).1.iter().map(LugKit::json).collect()
}

// ---------------- commands ----------------

fn spec(cmd: &str, s: &Session) -> Result<BuildingSpec> {
    let text = s.doc()?.building_spec.clone().ok_or_else(|| bad(cmd, "the drawing has no building spec (bspec.set)"))?;
    BuildingSpec::from_json(&text).map_err(|e| bad(cmd, e))
}

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let sp = spec("gear.list", s)?;
    let gear: Vec<Value> = sp
        .mep
        .equipment
        .iter()
        .filter(|e| is_gear(e))
        .map(|e| {
            let feeders_in: Vec<&str> = sp.mep.feeders.iter().filter(|f| f.to.eq_ignore_ascii_case(&e.tag)).map(|f| f.tag.as_str()).collect();
            let feeders_out: Vec<&str> = sp.mep.feeders.iter().filter(|f| f.from.eq_ignore_ascii_case(&e.tag)).map(|f| f.tag.as_str()).collect();
            json!({
                "tag": e.tag, "kind": e.kind, "level": e.level, "properties": e.properties, "terminations": e.terminations.len(),
                "enclosure": e.enclosure.is_some(), "feeders_in": feeders_in, "feeders_out": feeders_out, "accepted": e.accepted,
            })
        })
        .collect();
    Ok(json!({ "gear": gear, "feeders": sp.mep.feeders.len() }))
}

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("gear.check", s)?;
    let only = str_param(p, "tag").map(str::trim).filter(|t| !t.is_empty());
    if let Some(t) = only
        && !sp.mep.equipment.iter().any(|e| is_gear(e) && e.tag.eq_ignore_ascii_case(t))
    {
        return Err(bad("gear.check", format!("no electrical gear tagged {}", t.chars().take(40).collect::<String>())));
    }
    let (findings, kits) = review(&sp, only);
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    Ok(json!({
        "ok": errors == 0, "errors": errors, "findings": findings.iter().map(Finding::json).collect::<Vec<_>>(),
        "lug_kits": kits.iter().map(LugKit::json).collect::<Vec<_>>(), "labor": labor(&sp),
    }))
}

fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", h as u32)
}

fn run_review(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sp = spec("gear.review", s)?;
    let only = str_param(p, "tag").map(str::trim).filter(|t| !t.is_empty());
    let (findings, kits) = review(&sp, only);
    let mut added = Vec::new();
    for f in findings.iter().filter(|f| f.severity == "error") {
        let g = sp.mep.equipment.iter().find(|e| e.tag == f.gear);
        let at = g.map_or([0.0, 0.0], |e| [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0]);
        let level = g.map(|e| e.level.clone()).unwrap_or_default();
        let kind = if f.installability() { cadaddy_walk3d::RfiKind::GearInstall } else { cadaddy_walk3d::RfiKind::GearLugs };
        let id = format!("RFI-G{}", hash(&format!("{}|{}|{}", f.gear, f.check, f.message)));
        if sp.rfis.iter().any(|r| r.id == id) || sp.rfis.len() >= cadaddy_walk3d::rfi::MAX_RFIS {
            continue;
        }
        let mut r = cadaddy_walk3d::Rfi {
            id: id.clone(),
            kind: Some(kind),
            severity: kind.severity(),
            level,
            at,
            about: format!("{} ({})", f.gear, f.check),
            issue: format!("Submittal review: {}", f.message),
            assumed: "Held: the gear isn't released for manufacture until this is answered.".into(),
            proposed: (!f.fix.is_empty()).then(|| f.fix.clone()),
            ..cadaddy_walk3d::Rfi::default()
        };
        r.sheets = Vec::new();
        sp.rfis.push(r);
        added.push(id);
    }
    if !added.is_empty() {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    Ok(json!({
        "errors": errors, "rfis": added, "findings": findings.iter().map(Finding::json).collect::<Vec<_>>(),
        "lug_kits": kits.iter().map(LugKit::json).collect::<Vec<_>>(),
    }))
}

const CHECKS: [&str; 8] = ["lug count", "lug range", "lug material", "neutral bar", "ground bar", "entry", "gutter", "bending"];

fn run_accept(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sp = spec("gear.accept", s)?;
    let tag = str_param(p, "tag").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("gear.accept", "tag required"))?;
    let check = str_param(p, "check")
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| CHECKS.contains(&c.as_str()))
        .ok_or_else(|| bad("gear.accept", format!("check is one of {}", CHECKS.join(", "))))?;
    let undo = bool_or(p, "undo", false);
    let g = sp
        .mep
        .equipment
        .iter_mut()
        .find(|e| is_gear(e) && e.tag.eq_ignore_ascii_case(tag))
        .ok_or_else(|| bad("gear.accept", format!("no electrical gear tagged {}", tag.chars().take(40).collect::<String>())))?;
    let had = g.accepted.iter().any(|a| a.eq_ignore_ascii_case(&check));
    if undo {
        g.accepted.retain(|a| !a.eq_ignore_ascii_case(&check));
    } else if !had {
        g.accepted.push(check.clone());
    }
    let accepted = g.accepted.clone();
    let changed = had == undo;
    if changed {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({ "tag": tag, "accepted": accepted, "changed": changed, "labor": labor(&sp) }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("gear.list", "Electrical Gear", run_list)
            .params("{} → {gear: [{tag, kind, level, properties, terminations, enclosure, feeders_in, feeders_out, accepted}], feeders}")
            .noundo(),
        CommandSpec::new("gear.check", "Check Electrical Gear", run_check)
            .params("{tag?} → {ok, errors, findings: [{gear, check, severity, feeder, message, needed, available, fix}], lug_kits: [{gear, point, lugs, lug, bolts, bolt, nuts, flat_washers, belleville_washers}], labor}")
            .noundo(),
        CommandSpec::new("gear.review", "Submittal Review of Gear", run_review)
            .params("{tag?} → as gear.check, and every failure an RFI (gear_lugs, gear_install): {errors, rfis: [id], findings, lug_kits}"),
        CommandSpec::new("gear.accept", "Accept Gear as Submitted", run_accept)
            .params("{tag, check: lug count|lug range|lug material|neutral bar|ground bar|entry|gutter|bending, undo?: bool} → {tag, accepted, changed, labor: [{gear, conditions, factor}]}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_ranges_and_fractions() {
        assert_eq!(cmil("#2"), Some(66_360));
        assert_eq!(cmil("2 AWG"), Some(66_360));
        assert_eq!(cmil("3/0"), Some(167_800));
        assert_eq!(cmil("500 kcmil"), Some(500_000));
        assert_eq!(cmil("500 MCM AL"), Some(500_000));
        assert_eq!(cmil("350"), Some(350_000));
        assert_eq!(cmil("#5"), None);
        assert_eq!(cmil("junk"), None);
        assert_eq!(range("#6 - 350 kcmil"), Some((26_240, 350_000)));
        assert_eq!(range("1/0-750"), Some((105_600, 750_000)));
        assert_eq!(range("#14 to 1/0"), Some((4_110, 105_600)));
        assert_eq!(range("750-1/0"), None);
        assert_eq!((size_label(66_360), size_label(211_600), size_label(500_000)), ("#2".to_string(), "4/0".to_string(), "500 kcmil".to_string()));
        assert_eq!(
            (inches(0.5), inches(2.25), inches(1.0), inches(0.625)),
            ("1/2\"".to_string(), "2-1/4\"".to_string(), "1\"".to_string(), "5/8\"".to_string())
        );
        let t = Termination { bolt_in: 0.5, stack_in: 0.75, ..Termination::default() };
        assert_eq!(bolt_length(&t), 1.75);
    }

    #[test]
    fn tables_answer_by_the_next_size_up() {
        let r = Rules::default();
        assert_eq!(r.od(3.0), 3.5);
        assert_eq!(r.od(2.75), 3.5);
        assert!((r.area(500_000) - 0.7073).abs() < 1e-9);
        assert_eq!(r.bending(500_000, 1, true), Some(14.0));
        assert_eq!(r.bending(500_000, 2, true), Some(14.0));
        assert_eq!(r.bending(211_600, 4, true), Some(8.5), "past the row's columns: the last given");
        assert_eq!(r.bending(5_000_000, 1, true), None);
    }
}
