//! The utility's service transformer (Q6307): a one-line that names the utility transformer
//! feeding the service gear calls a pad-mounted transformer outside the building and the service
//! conductors from it to that gear, even when no plan draws it to scale.
//!
//! - [`read`]: on a one-line's texts, the label naming the utility's transformer (the words of
//!   `utilityxfmr.json`), the kVA and secondary its label gives, the feeder-type tag on the line
//!   between it and the service gear (its service entrance conductors) and a note's assumed length
//!   from it to the gear.
//! - [`propose`]: the transformer (its outline by its kVA from rules data; the kVA from its label,
//!   else the service gear's rating at its voltage rounded up to a standard size) on a concrete
//!   pad, on the slab level outside the slab edge nearest the service gear, marked proposed with
//!   why (the site plan isn't registered to the building); the pad decision; and the service
//!   feeder from it to the gear, its conduits and conductors the feeder schedule's for that tag.
//!   The job's feeder routes build that feeder in the ground ([`service_cover_ft`]) and the
//!   takeoff carries its conductors.
//!
//! Every value comes from the documents or from `utilityxfmr.json` (rules data, each with its
//! source); nothing is known about one job.

use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{BuildingSpec, Feeder, MepEquipment, PadDecision};
use serde::Deserialize;

use super::gearbuild::FeederType;

const RULES: &str = include_str!("utilityxfmr.json");

/// Texts of one sheet looked at, at most (hostile input).
const MAX_TEXTS: usize = 200_000;
/// Labels weighed per sheet, at most.
const MAX_LABELS: usize = 50;

/// The rules data (`utilityxfmr.json`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub label_words: Vec<String>,
    pub label_max_chars: usize,
    pub label_skip_prefixes: Vec<String>,
    pub label_source: String,
    pub tag: String,
    pub furnished_by: String,
    /// Lines under the label that still describe it, in text heights.
    pub line_reach_h: f64,
    /// How far beside the line from the label to the gear a feeder tag may sit, text heights.
    pub feeder_reach_h: f64,
    /// How far a note's lines run on from the line naming the transformer, text heights.
    pub note_reach_h: f64,
    pub kva_sizes: Vec<f64>,
    pub default_kva: f64,
    pub kva_source: String,
    /// Each row: the largest kVA it holds, then width, depth, height (inches).
    pub outlines: Vec<[f64; 4]>,
    pub outline_source: String,
    pub clearance_ft: f64,
    pub clearance_source: String,
    pub pad_owner: String,
    pub pad_margin_in: f64,
    pub pad_height_in: f64,
    pub pad_words: Vec<String>,
    pub pad_source: String,
    pub service_entry: String,
    pub service_entry_source: String,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// The `sources` key that marks equipment standing on the site, outside the building.
pub(crate) const SITE_SOURCE: &str = "site";
/// The property holding the least cover of the buried service from site gear, inches.
pub(crate) const COVER_PROPERTY: &str = "SERVICE COVER";

/// Whether a piece of equipment stands on the site (outside the building by design).
pub(crate) fn is_site(e: &MepEquipment) -> bool {
    e.sources.contains_key(SITE_SOURCE)
}

/// How deep a buried feeder from or to site gear runs below the floor, feet to the conduit's
/// bottom: its least cover (the site gear's [`COVER_PROPERTY`]) and the conduit. None: neither end
/// is site gear.
pub(crate) fn service_cover_ft(a: &MepEquipment, b: &MepEquipment, size_in: f64) -> Option<f64> {
    let cover = [a, b].into_iter().filter(|e| is_site(e)).find_map(|e| {
        e.properties.get(COVER_PROPERTY).and_then(|v| v.split_whitespace().next()).and_then(|n| n.parse::<f64>().ok())
    })?;
    let size = if size_in.is_finite() && size_in > 0.0 { size_in } else { 1.0 };
    Some((cover.clamp(0.0, 240.0) + size) / 12.0)
}

/// What a one-line says of the utility's transformer.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Called {
    pub sheet: String,
    /// The label as printed.
    pub label: String,
    pub kva: Option<f64>,
    /// Its secondary as the label's lines give it (`480Y/277V, 3PH SECONDARY`).
    pub secondary: String,
    /// The feeder-type tag on the line from it to the service gear.
    pub feeder_type: Option<String>,
    /// A note's assumed length from it to the service gear, feet, and the note.
    pub assumed_ft: Option<(f64, String)>,
}

fn upper(t: &TextItem) -> String {
    t.value.trim().to_ascii_uppercase()
}

fn names_it(up: &str, r: &Rules) -> bool {
    r.label_words.iter().any(|w| !w.is_empty() && up.contains(w.as_str()))
}

/// `750 KVA`, `750KVA`, `112.5 KVA` in a text.
fn kva_in(up: &str) -> Option<f64> {
    let at = up.find("KVA")?;
    let head = up.get(..at)?.trim_end();
    let num: String = head.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect::<Vec<_>>().into_iter().rev().collect();
    num.replace(',', "").parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0 && *v <= 100_000.0)
}

/// A voltage in a text (`208/120V`, `480Y/277 V`): the text itself when it has one.
fn has_voltage(up: &str) -> bool {
    up.split(|c: char| !c.is_ascii_alphanumeric() && c != '/').any(|w| {
        let w = w.trim_end_matches('V');
        let digits = w.chars().filter(char::is_ascii_digit).count();
        w.contains('/') && digits >= 5 && w.chars().all(|c| c.is_ascii_digit() || c == '/' || c == 'Y')
    })
}

/// A length in feet in a text after `from` (`50'-0"`, `75 FT`, `120'`).
fn feet_after(up: &str, from: usize) -> Option<f64> {
    let rest = up.get(from..)?;
    let bytes: Vec<char> = rest.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes.get(i).is_some_and(char::is_ascii_digit) {
            let start = i;
            while bytes.get(i).is_some_and(|c| c.is_ascii_digit() || *c == '.') {
                i += 1;
            }
            let n: String = bytes.get(start..i).map(|s| s.iter().collect()).unwrap_or_default();
            let tail: String = bytes.get(i..(i + 3).min(bytes.len())).map(|s| s.iter().collect()).unwrap_or_default();
            if tail.starts_with('\'') || tail.trim_start().starts_with("FT") {
                let mut ft = n.parse::<f64>().ok()?;
                // Inches after it: `50'-6"`.
                let after: String = bytes.get(i + 1..(i + 6).min(bytes.len())).map(|s| s.iter().collect()).unwrap_or_default();
                let inches: String = after.trim_start_matches(['-', ' ']).chars().take_while(char::is_ascii_digit).collect();
                if tail.starts_with('\'')
                    && let Ok(v) = inches.parse::<f64>()
                {
                    ft += v / 12.0;
                }
                return Some(ft).filter(|v| v.is_finite() && *v > 0.0 && *v < 10_000.0);
            }
        } else {
            i += 1;
        }
    }
    None
}

/// A note's line: it starts with its number (`27.`, `3)`).
fn note_number(up: &str) -> bool {
    let digits = up.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && up.chars().nth(digits).is_some_and(|c| c == '.' || c == ')')
}

/// The utility's transformer on a one-line's texts ([`Called`]): its label (short, naming it, not
/// a `TO …` pointer or a note's line), the lines under it, the feeder-type tag (`types`) on the line
/// from it to `gear_tag`'s label (else nearest it) and a note's assumed length to the gear.
pub(crate) fn read(texts: &[TextItem], sheet: &str, gear_tag: &str, types: &[String]) -> Option<Called> {
    let r = rules();
    let texts = texts.get(..texts.len().min(MAX_TEXTS)).unwrap_or(texts);
    let is_label = |t: &TextItem| {
        let up = upper(t);
        names_it(&up, r)
            && up.len() <= r.label_max_chars.max(1)
            && !r.label_skip_prefixes.iter().any(|p| !p.is_empty() && up.starts_with(p.as_str()))
            && !note_number(&up)
    };
    let labels: Vec<&TextItem> = texts.iter().filter(|t| is_label(t)).take(MAX_LABELS).collect();
    if labels.is_empty() {
        return None;
    }
    let gear = texts.iter().filter(|t| !gear_tag.is_empty() && t.value.trim().eq_ignore_ascii_case(gear_tag)).collect::<Vec<_>>();
    let tag_texts: Vec<&TextItem> = texts.iter().filter(|t| types.iter().any(|x| x.eq_ignore_ascii_case(t.value.trim()))).collect();
    // The feeder tag for one label: on the line between it and the gear's label (between them
    // along the longer way, within reach across it), nearest the gear; with no gear label, the
    // nearest within reach of the label.
    let feeder_for = |l: &TextItem| -> Option<(String, f64)> {
        let h = l.height.max(1e-9);
        let reach = r.feeder_reach_h.max(1.0) * h;
        let near_gear = gear
            .iter()
            .filter_map(|g| {
                let (dx, dy) = (g.at.x - l.at.x, g.at.y - l.at.y);
                let vertical = dy.abs() >= dx.abs();
                tag_texts
                    .iter()
                    .filter(|t| {
                        let (lo_y, hi_y) = (l.at.y.min(g.at.y), l.at.y.max(g.at.y));
                        let (lo_x, hi_x) = (l.at.x.min(g.at.x), l.at.x.max(g.at.x));
                        if vertical {
                            t.at.y > lo_y - h && t.at.y < hi_y + h && t.at.x > lo_x - reach && t.at.x < hi_x + reach
                        } else {
                            t.at.x > lo_x - h && t.at.x < hi_x + h && t.at.y > lo_y - reach && t.at.y < hi_y + reach
                        }
                    })
                    .map(|t| (t.value.trim().to_string(), (t.at - g.at).len()))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        near_gear.or_else(|| {
            tag_texts.iter().map(|t| (t.value.trim().to_string(), (t.at - l.at).len())).filter(|(_, d)| *d <= reach).min_by(|a, b| a.1.total_cmp(&b.1))
        })
    };
    // The label with a feeder tag by it (the one-line's, not a legend's or a riser's).
    let (label, feeder) = labels
        .iter()
        .map(|l| (*l, feeder_for(l)))
        .min_by(|a, b| {
            let da = a.1.as_ref().map_or(f64::INFINITY, |f| f.1);
            let db = b.1.as_ref().map_or(f64::INFINITY, |f| f.1);
            da.total_cmp(&db)
        })?;
    let h = label.height.max(1e-9);
    // The lines under the label: its kVA and secondary.
    let mut lines: Vec<&TextItem> = texts
        .iter()
        .filter(|t| t.at.y < label.at.y - 0.3 * h && t.at.y > label.at.y - r.line_reach_h.max(1.0) * h && (t.at.x - label.at.x).abs() < 2.0 * h)
        .collect();
    lines.sort_by(|a, b| b.at.y.total_cmp(&a.at.y));
    let up_label = upper(label);
    let kva = kva_in(&up_label).or_else(|| lines.iter().find_map(|t| kva_in(&upper(t))));
    let secondary = lines.iter().map(|t| t.value.trim().to_string()).find(|v| has_voltage(&v.to_ascii_uppercase())).unwrap_or_default();
    // A note's assumed length from it to the gear: the note's lines around one naming it.
    let mut assumed = None;
    for t in texts.iter().filter(|t| names_it(&upper(t), r)).take(MAX_LABELS) {
        let th = t.height.max(1e-9);
        let reach = r.note_reach_h.max(1.0) * th;
        let mut note: Vec<&TextItem> = texts.iter().filter(|o| (o.at.y - t.at.y).abs() <= reach && (o.at.x - t.at.x).abs() < 4.0 * th).collect();
        note.sort_by(|a, b| b.at.y.total_cmp(&a.at.y));
        let joined = note.iter().map(|o| upper(o)).collect::<Vec<_>>().join(" ");
        let Some(at) = r.label_words.iter().filter(|w| !w.is_empty()).filter_map(|w| joined.find(w.as_str())).min() else { continue };
        let Some(len_at) = joined.find("LENGTH") else { continue };
        let gear_named = gear_tag.is_empty() || joined.contains(&gear_tag.to_ascii_uppercase());
        if gear_named && let Some(ft) = feet_after(&joined, len_at.max(at)) {
            // From the sentence that gives the length.
            let start = joined.get(..len_at).and_then(|h| h.rfind(". ")).map_or(0, |i| i + 2);
            let said = joined.get(start..).unwrap_or(&joined);
            assumed = Some((ft, format!("{sheet} note: {}", said.chars().take(200).collect::<String>())));
            break;
        }
    }
    Some(Called {
        sheet: sheet.to_string(),
        label: label.value.trim().to_string(),
        kva,
        secondary,
        feeder_type: feeder.map(|f| f.0),
        assumed_ft: assumed,
    })
}

/// Amps in a gear property (`2000A MCB`, `2000A`, `1,200 A`).
fn amps(v: &str) -> Option<f64> {
    let digits: String = v.trim().chars().take_while(|c| c.is_ascii_digit() || *c == ',').filter(|c| *c != ',').collect();
    digits.parse::<f64>().ok().filter(|a| a.is_finite() && *a > 0.0 && *a < 100_000.0)
}

/// The line voltage and phases of a `VOLTS/PHASE` value (`208Y/120V, 3PH, 4W` → 208, 3).
fn volts_phases(v: &str) -> Option<(f64, u32)> {
    let u = v.to_ascii_uppercase();
    let line: f64 = u.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<f64>().ok()).find(|n| *n >= 100.0 && *n <= 50_000.0)?;
    let phases = if u.contains("1PH") || u.contains("1 PH") || u.contains("1Ø") || u.contains("1-PHASE") { 1 } else { 3 };
    Some((line, phases))
}

/// A property of the gear by any of the names a schedule gives it.
fn prop<'a>(e: &'a MepEquipment, names: &[&str]) -> Option<&'a str> {
    e.properties.iter().find(|(k, _)| names.iter().any(|n| k.eq_ignore_ascii_case(n))).map(|(_, v)| v.as_str())
}

/// The transformer's kVA and where it came from: its label's, else the service gear's rating at
/// its voltage rounded up to a standard size, else the rules' default.
fn kva_of(called: &Called, gear: &MepEquipment, r: &Rules) -> (f64, String) {
    if let Some(k) = called.kva {
        return (k, format!("one-line {}: \"{}\"", called.sheet, called.label));
    }
    let a = prop(gear, &["MAIN SIZE / TYPE", "MAIN", "MAINS", "MAIN BREAKER"]).and_then(amps).or_else(|| prop(gear, &["BUS AMPS", "BUS", "AMPS"]).and_then(amps));
    let vp = prop(gear, &["VOLTS/PHASE", "VOLTAGE", "VOLTS"]).and_then(volts_phases);
    match (a, vp) {
        (Some(a), Some((v, ph))) => {
            let need = a * v * if ph == 3 { 3f64.sqrt() } else { 1.0 } / 1000.0;
            let size = r.kva_sizes.iter().copied().filter(|s| s.is_finite() && *s >= need - 1e-6).fold(f64::INFINITY, f64::min);
            let size = if size.is_finite() { size } else { r.kva_sizes.iter().copied().fold(need, f64::max) };
            (size, format!("{a:.0} A at {v:.0} V {ph}-phase ({}) = {need:.0} kVA; {}", gear.tag, r.kva_source))
        }
        _ => (r.default_kva, format!("the service equipment's rating isn't read; {} kVA assumed ({})", r.default_kva, r.kva_source)),
    }
}

/// The unit's outline for a kVA: width, depth, height (inches).
fn outline(kva: f64, r: &Rules) -> [f64; 3] {
    let row = r.outlines.iter().find(|o| kva <= o[0] + 1e-6).or(r.outlines.last());
    row.map_or([74.0, 64.0, 60.0], |o| [o[1], o[2], o[3]])
}

/// What [`propose`] makes: the transformer, its pad decision and its service feeder (when the
/// one-line names the feeder's type and the schedule has it), and how it was placed.
#[derive(Clone, Debug, Default)]
pub(crate) struct Proposed {
    pub equipment: MepEquipment,
    pub pad: PadDecision,
    pub feeder: Option<Feeder>,
    /// Why no service feeder: no type tag on the line, or the schedule lacks it.
    pub no_feeder: Option<String>,
    /// About how long the service is as placed (feet, plan, gear to transformer).
    pub service_ft: f64,
}

/// The utility's transformer the one-line calls ([`read`]), placed outside the slab edge nearest
/// the service gear (proposed, the site plan isn't registered to the building), on its pad, with
/// its service feeder to the gear. `cover_in`: the least cover a note gives the service, else the
/// rules'. `pad_notes`: the documents' notes on the transformer's pad (its basis).
pub(crate) fn propose(sp: &BuildingSpec, called: &Called, types: &[FeederType], cover_in: Option<(f64, String)>, pad_notes: &[String]) -> Result<Proposed, String> {
    let r = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let slab = super::ugplan::slab_level(sp).ok_or("the model has no slab level")?;
    let gear = super::ugplan::service_gear(sp, &slab).ok_or_else(|| format!("no service gear on {slab} in the model"))?;
    let b = super::ugplan::slab_box(sp, &slab).ok_or_else(|| format!("{slab} has no outline"))?;
    let gc = [(gear.min[0] + gear.max[0]) / 2.0, (gear.min[1] + gear.max[1]) / 2.0];
    let (edge, n) = super::ugplan::nearest_edge(b, gc);
    let (kva, kva_src) = kva_of(called, gear, r);
    let [w, d, h] = outline(kva, r);
    let inch = upf / 12.0;
    let clear = r.clearance_ft.max(0.0) * upf;
    // Out from the edge: the clearance, the pad's margin, half the unit (its depth along the way out).
    let out = clear + r.pad_margin_in.max(0.0) * inch + d / 2.0 * inch;
    let c = [edge[0] + n[0] * out, edge[1] + n[1] * out];
    // Its width along the edge.
    let (hx, hy) = if n[0].abs() > 0.5 { (d / 2.0 * inch, w / 2.0 * inch) } else { (w / 2.0 * inch, d / 2.0 * inch) };
    let tag = if r.tag.trim().is_empty() { "UTILITY-XFMR".to_string() } else { r.tag.trim().to_string() };
    let service_ft = ((c[0] - gc[0]).abs() + (c[1] - gc[1]).abs()) / upf;
    let (cover, cover_src) = cover_in.unwrap_or_else(|| {
        let u = super::ugplan::rules();
        (u.depths.service_cover_in, u.depths.service_source.clone())
    });
    let mut properties = std::collections::BTreeMap::new();
    properties.insert("KVA".into(), format!("{kva} kVA ({kva_src})"));
    if !called.secondary.is_empty() {
        properties.insert("SECONDARY".into(), format!("{} (one-line {})", called.secondary, called.sheet));
    } else if let Some(v) = prop(gear, &["VOLTS/PHASE", "VOLTAGE", "VOLTS"]) {
        properties.insert("SECONDARY".into(), format!("{v} (the service equipment {}'s)", gear.tag));
    }
    properties.insert("SIZE".into(), format!("{w} W x {d} D x {h} H in ({})", r.outline_source));
    properties.insert("FURNISHED BY".into(), format!("{} ({})", r.furnished_by, r.label_source));
    properties.insert("MOUNTING".into(), "pad-mounted on grade, outdoors".into());
    properties.insert("FEEDS".into(), gear.tag.clone());
    properties.insert(COVER_PROPERTY.into(), format!("{cover} in to the service conduits ({cover_src})"));
    let dir = match (n[0] > 0.5, n[0] < -0.5, n[1] > 0.5) {
        (true, _, _) => "east (+x)",
        (_, true, _) => "west (-x)",
        (_, _, true) => "north (+y)",
        _ => "south (-y)",
    };
    properties.insert(
        "DESIGN_VALUE".into(),
        format!(
            "called by the one-line {} (\"{}\"); no plan locates it to scale: proposed {} ft out from the slab edge nearest {} on its {dir} side",
            called.sheet, called.label, r.clearance_ft, gear.tag
        ),
    );
    properties.insert("PLACEMENT".into(), format!("proposed: {}", r.clearance_source));
    let mut length = format!("about {service_ft:.0} ft from {} as placed", gear.tag);
    if let Some((ft, note)) = &called.assumed_ft {
        length.push_str(&format!("; the documents assume {ft:.0} ft ({})", note.chars().take(160).collect::<String>()));
    }
    properties.insert("SERVICE LENGTH".into(), length);
    let mut sources = std::collections::BTreeMap::new();
    sources.insert("called".into(), format!("one-line {}: \"{}\"", called.sheet, called.label));
    sources.insert("position".into(), format!("proposed outside the slab edge nearest {} (the site plan isn't registered to the building)", gear.tag));
    sources.insert("size".into(), r.outline_source.clone());
    sources.insert("status".into(), "proposed".into());
    sources.insert(SITE_SOURCE.into(), "outside the building".into());
    let equipment = MepEquipment {
        tag: tag.clone(),
        kind: "transformer".into(),
        level: slab.clone(),
        min: [c[0] - hx, c[1] - hy],
        max: [c[0] + hx, c[1] + hy],
        bottom_ft: 0.0,
        height_ft: h / 12.0,
        title: format!("{} (pad-mounted)", called.label.to_ascii_uppercase()),
        properties,
        sources,
        confidence: 0.5,
        ..MepEquipment::default()
    };
    let basis = if pad_notes.is_empty() {
        r.pad_source.clone()
    } else {
        format!("{} ({})", pad_notes.iter().take(3).cloned().collect::<Vec<_>>().join("; "), r.pad_source)
    };
    let pad = PadDecision {
        equipment: tag.clone(),
        required: Some(true),
        basis: basis.chars().take(400).collect(),
        owner: r.pad_owner.clone(),
        margin_in: Some(r.pad_margin_in),
        height_in: Some(r.pad_height_in),
        ..PadDecision::default()
    };
    // The service feeder: the one-line's type tag on the line, its conduits and conductors.
    let ft = called.feeder_type.as_ref().and_then(|k| types.iter().find(|t| t.tag.eq_ignore_ascii_case(k)));
    let (feeder, no_feeder) = match (ft, &called.feeder_type) {
        (Some(ft), _) => {
            let single = prop(gear, &["VOLTS/PHASE", "VOLTAGE"]).and_then(volts_phases).is_some_and(|(_, p)| p == 1);
            let phases = if single { 2 } else { 3 }.min(ft.conductors);
            let entry = prop(gear, &["INCOMING LOCATION"])
                .and_then(|v| v.split_whitespace().next())
                .map(str::to_ascii_lowercase)
                .filter(|v| v == "top" || v == "bottom")
                .unwrap_or_else(|| r.service_entry.clone());
            (
                Some(Feeder {
                    tag: ft.tag.clone(),
                    from: tag.clone(),
                    to: gear.tag.clone(),
                    from_point: "secondary".into(),
                    to_point: "main".into(),
                    sets: ft.conduits.max(1),
                    phases,
                    size: ft.size.clone(),
                    material: ft.material.clone(),
                    neutrals: ft.conductors.saturating_sub(phases),
                    grounds: u32::from(!ft.ground_size.is_empty()),
                    ground_size: ft.ground_size.clone(),
                    conduits: ft.conduits.max(1),
                    conduit_in: ft.conduit_in,
                    entry,
                    from_entry: "bottom".into(),
                    source: format!(
                        "{}; one-line tag {} between {} and {} (the service entrance conductors)",
                        ft.source, ft.tag, called.label, gear.tag
                    ),
                    ..Feeder::default()
                }),
                None,
            )
        }
        (None, Some(k)) => (None, Some(format!("the one-line tags the service {k}, and no feeder schedule row has it"))),
        (None, None) => (None, Some("no feeder-type tag on the one-line between the transformer and the service gear".into())),
    };
    Ok(Proposed { equipment, pad, feeder, no_feeder, service_ft })
}

/// The documents' notes on a transformer's pad (lines naming a pad and a transformer), at most a few.
pub(crate) fn pad_notes(sheet: &str, texts: &[TextItem]) -> Vec<String> {
    let r = rules();
    texts
        .iter()
        .take(MAX_TEXTS)
        .map(upper)
        .filter(|u| u.contains("TRANSFORMER") && r.pad_words.iter().any(|w| !w.is_empty() && u.contains(w.as_str())))
        .take(3)
        .map(|u| format!("{sheet}: {}", u.chars().take(120).collect::<String>()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_geom::Vec2;

    fn t(v: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: 0.1, rotation: 0.0 }
    }

    /// A made-up one-line: the utility's transformer over a CT cabinet over the service
    /// switchboard `SWBD-A`, the service feeder tag `SE1` on the line twice (once well to the side
    /// of the gear's label), a feeder tag `F4` under the switchboard to a panel, a `TO …` pointer
    /// and a note giving the length.
    fn oneline() -> Vec<TextItem> {
        vec![
            t("UTILITY TRANSFORMER", 10.0, 20.0),
            t("480Y/277V, 3PH SECONDARY", 10.0, 19.85),
            t("SE1", 9.4, 19.4),
            t("CT CABINET", 10.1, 19.2),
            t("SE1", 9.4, 18.2),
            t("SWBD-A", 10.0, 18.08),
            t("F4", 12.0, 17.2),
            t("PANEL P1", 12.0, 17.0),
            t("TO UTILITY TRANSFORMER", 30.0, 10.0),
            t("12.LENGTH OF SERVICE FROM UTILITY TRANSFORMER", 40.0, 10.0),
            t("TO 'SWBD-A' IS ASSUMED TO BE 75'-6\".", 40.2, 9.85),
        ]
    }

    #[test]
    fn the_one_line_names_the_transformer_its_service_feeder_and_assumed_length() {
        let c = read(&oneline(), "E-001", "SWBD-A", &["SE1".into(), "F4".into()]).expect("called");
        assert_eq!(c.label, "UTILITY TRANSFORMER");
        assert_eq!(c.secondary, "480Y/277V, 3PH SECONDARY");
        assert_eq!(c.feeder_type.as_deref(), Some("SE1"), "the tag on the line to the gear, not the panel's");
        assert_eq!(c.kva, None);
        let (ft, note) = c.assumed_ft.expect("note");
        assert!((ft - 75.5).abs() < 1e-9, "{ft}");
        assert!(note.starts_with("E-001 note"), "{note}");
    }

    #[test]
    fn a_pointer_or_a_note_is_not_the_label_and_no_label_is_no_call() {
        let only = vec![t("TO UTILITY TRANSFORMER", 1.0, 1.0), t("3.VERIFY UTILITY TRANSFORMER LOCATION", 1.0, 2.0)];
        assert_eq!(read(&only, "E-1", "SWBD-A", &[]), None);
        let kva = vec![t("1000 KVA UTILITY TRANSFORMER", 1.0, 1.0)];
        assert_eq!(read(&kva, "E-1", "", &[]).and_then(|c| c.kva), Some(1000.0));
        let under = vec![t("UTILITY XFMR", 1.0, 1.0), t("112.5KVA", 1.0, 0.85)];
        assert_eq!(read(&under, "E-1", "", &[]).and_then(|c| c.kva), Some(112.5));
    }

    #[test]
    fn a_horizontal_one_line_finds_the_tag_between_side_by_side_labels() {
        let v = vec![t("UTILITY TRANSFORMER", 1.0, 5.0), t("S-9", 3.0, 5.15), t("MSB", 5.0, 5.0), t("F2", 5.0, 7.5)];
        let c = read(&v, "E-2", "MSB", &["S-9".into(), "F2".into()]).expect("called");
        assert_eq!(c.feeder_type.as_deref(), Some("S-9"));
    }

    fn level(name: &str, region: [f64; 4]) -> cadaddy_walk3d::Level {
        serde_json::from_value(serde_json::json!({"name": name, "elevation_ft": 0.0, "height_ft": 12.0, "ceiling_ft": 10.0, "region": region}))
            .expect("level")
    }

    fn spec() -> BuildingSpec {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        sp.levels = vec![level("Ground", [0.0, 0.0, 1200.0, 600.0])];
        let mut props = std::collections::BTreeMap::new();
        props.insert("MAIN SIZE / TYPE".to_string(), "1600A MCB".to_string());
        props.insert("VOLTS/PHASE".to_string(), "480Y/277V, 3PH, 4W".to_string());
        props.insert("INCOMING LOCATION".to_string(), "bottom (submittal)".to_string());
        sp.mep.equipment.push(MepEquipment {
            tag: "SWBD-A".into(),
            kind: "switchboard".into(),
            level: "Ground".into(),
            min: [1100.0, 280.0],
            max: [1180.0, 310.0],
            height_ft: 7.5,
            properties: props,
            ..Default::default()
        });
        sp
    }

    fn se1() -> FeederType {
        FeederType {
            tag: "SE1".into(),
            conduits: 4,
            conduit_in: 4.0,
            conductors: 4,
            size: "500 kcmil".into(),
            material: "cu".into(),
            ground_size: String::new(),
            source: "FEEDER SCHEDULE E-001".into(),
        }
    }

    #[test]
    fn the_transformer_is_proposed_outside_the_nearest_edge_on_a_pad_with_its_service_feeder() {
        let sp = spec();
        let c = read(&oneline(), "E-001", "SWBD-A", &["SE1".into()]).expect("called");
        let p = propose(&sp, &c, &[se1()], Some((30.0, "E-002 note".into())), &["E-003: TRANSFORMER PAD DETAIL".into()]).expect("proposed");
        let e = &p.equipment;
        assert_eq!(e.kind, "transformer");
        assert_eq!(e.level, "Ground");
        assert!(is_site(e));
        // 1600 A at 480 V three-phase = 1330 kVA: the next standard size.
        assert!(e.properties.get("KVA").is_some_and(|v| v.starts_with("1500 kVA")), "{:?}", e.properties.get("KVA"));
        // East of the slab (the switchboard is by the east edge), 10 ft + 6 in + half its depth out.
        let cx = (e.min[0] + e.max[0]) / 2.0;
        assert!(e.min[0] > 1200.0 + 120.0 - 1e-6, "clear of the building: {}", e.min[0]);
        assert!((cx - (1200.0 + 120.0 + 6.0 + 45.0)).abs() < 1e-6, "{cx}");
        assert!(e.properties.get("DESIGN_VALUE").is_some_and(|v| v.contains("proposed") && v.contains("E-001")));
        assert!(e.properties.get("SERVICE LENGTH").is_some_and(|v| v.contains("assume 76 ft")), "{:?}", e.properties.get("SERVICE LENGTH"));
        assert!(e.properties.get(COVER_PROPERTY).is_some_and(|v| v.starts_with("30 in")));
        assert_eq!(p.pad.required, Some(true));
        assert!(p.pad.basis.contains("TRANSFORMER PAD DETAIL"));
        let f = p.feeder.expect("service feeder");
        assert_eq!((f.from.as_str(), f.to.as_str(), f.tag.as_str()), ("UTILITY-XFMR", "SWBD-A", "SE1"));
        assert_eq!((f.conduits, f.conduit_in, f.phases, f.neutrals, f.grounds), (4, 4.0, 3, 1, 0));
        assert_eq!((f.entry.as_str(), f.from_entry.as_str()), ("bottom", "bottom"));
        // The buried route: 30 in cover and the conduit.
        let gear = sp.mep.equipment.first().expect("gear");
        assert_eq!(service_cover_ft(e, gear, 4.0), Some(34.0 / 12.0));
        assert_eq!(service_cover_ft(gear, gear, 4.0), None);
    }

    #[test]
    fn no_service_gear_or_an_unscheduled_tag_is_said_not_guessed() {
        let mut sp = spec();
        let c = read(&oneline(), "E-001", "SWBD-A", &["SE1".into()]).expect("called");
        let p = propose(&sp, &c, &[], None, &[]).expect("proposed");
        assert!(p.feeder.is_none());
        assert!(p.no_feeder.is_some_and(|w| w.contains("SE1")));
        assert!(p.equipment.properties.get(COVER_PROPERTY).is_some_and(|v| v.contains("24")), "the rules' cover");
        sp.mep.equipment.clear();
        assert!(propose(&sp, &c, &[se1()], None, &[]).is_err_and(|e| e.contains("no service gear")));
    }

    #[test]
    fn rules_data_loads() {
        let r = rules();
        assert!(!r.label_words.is_empty() && !r.outlines.is_empty() && !r.kva_sizes.is_empty());
        assert!(r.clearance_ft > 0.0 && r.pad_height_in > 0.0);
    }
}
