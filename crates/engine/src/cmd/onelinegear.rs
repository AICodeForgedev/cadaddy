//! The service and distribution gear a one-line draws, built even with no gear submittal
//! (Q6359): engineers' sets often give the service only as a one-line (meter socket, service
//! disconnect, gutter, panel, grounding electrodes, the feeders between them tagged by a feeder
//! schedule) and a panel only as its schedule; the field still installs every piece.
//!
//! - [`panel_box`]: a panel its schedule names but no submittal sizes gets the typical enclosure
//!   for its rating and circuits from rules data, marked inferred (a submittal replaces it).
//! - [`read`]: on a sheet titled as a one-line, each label naming a kind of gear in
//!   `onelinegear.json` (short, not a note, not a pointer, not a schedule's cell), with the lines
//!   under it: its rating (amps or kVA), poles, fused or not and its fuses, service-entrance
//!   rating, NEMA type, voltage, how many, a size printed on it; a grounding electrode label its
//!   electrode kind and conductor.
//! - [`links`]: the one-line read top-down from the service: each piece's incoming feeder is the
//!   feeder tag just above it (as [`super::gearbuild::oneline_incoming`] reads a panel's), and it
//!   is fed from the next piece above that tag. A tag the feeder schedule leaves unsized (`XX`, a
//!   row with no conductors) is a review question, never a silent zero.
//! - [`layout`]: each piece proposed beside the gear it feeds, in the one-line's order, its box
//!   from its label or from rules data by its rating; the electrodes at the service.
//!
//! The alternative reading (a one-line drawn left to right) isn't taken: the top-down reading is
//! the one the panel tags already use. Every value says where it came from; nothing is known
//! about one job.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{BuildingSpec, Electrode, Feeder, MepEquipment, PanelSchedule};
use serde::Deserialize;

use super::gearbuild::{FeederType, SubmittedGear};

const RULES: &str = include_str!("onelinegear.json");
/// Texts of one sheet looked at, labels kept per sheet, and pieces one label may count
/// (hostile input).
const MAX_TEXTS: usize = 200_000;
const MAX_LABELS: usize = 200;
const MAX_COUNT: u32 = 50;
/// Links followed from a piece to the gear it feeds.
const MAX_HOPS: usize = 32;

/// One kind of gear a one-line labels.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct KindRule {
    pub kind: String,
    pub title: String,
    pub tag_prefix: String,
    pub words: Vec<String>,
    pub not_words: Vec<String>,
    /// `amps` (default) or `kva`: what its sizes go by.
    pub rating: String,
    /// Each row: the largest rating it holds, then height, width, depth (inches).
    pub sizes: Vec<[f64; 4]>,
    /// `wall` or `floor`.
    pub mount: String,
    pub center_in: Option<f64>,
    pub top_in: Option<f64>,
    pub bottom_in: Option<f64>,
    pub size_source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ElectrodeWords {
    pub kind: String,
    pub words: Vec<String>,
}

/// A panelboard enclosure by rating and circuits.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PanelSize {
    pub max_amps: f64,
    pub max_circuits: u32,
    pub height_in: f64,
    pub width_in: f64,
    pub depth_in: f64,
}

/// The rules data (`onelinegear.json`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub diagram_titles: Vec<String>,
    pub diagram_source: String,
    pub label_max_chars: usize,
    pub label_skip_prefixes: Vec<String>,
    /// A label's lines: how far from its first line (text heights), and how far across.
    pub label_reach_h: f64,
    pub label_width_h: f64,
    /// A piece's incoming tag: how far above it and how far to either side (text heights).
    pub incoming_reach_h: f64,
    pub incoming_side_h: f64,
    /// The piece feeding a tag: how far above it and to either side (text heights).
    pub source_reach_h: f64,
    pub source_side_h: f64,
    pub unsized_tags: Vec<String>,
    pub unsized_source: String,
    pub gap_in: f64,
    pub layout_source: String,
    pub kinds: Vec<KindRule>,
    pub electrode_words: Vec<ElectrodeWords>,
    pub electrode_label_words: Vec<String>,
    pub electrode_tag_prefix: String,
    pub rod_length_ft: f64,
    pub rod_source: String,
    pub ufer_length_ft: f64,
    pub ufer_source: String,
    /// Each row: the largest service conductor (circular mils) and its electrode conductor.
    pub gec_table: Vec<(f64, String)>,
    pub gec_rod_max: String,
    pub gec_ufer_max: String,
    pub gec_source: String,
    pub panel_sizes: Vec<PanelSize>,
    pub panel_default_amps: f64,
    pub panel_switchboard_over_amps: f64,
    pub panel_size_source: String,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// The `sources` key marking equipment a one-line called (its sheet and label).
pub(crate) const SOURCE_KEY: &str = "oneline";
/// The property holding the one-line's label as printed.
pub(crate) const LABEL_PROPERTY: &str = "ONE-LINE LABEL";

/// Whether a piece of equipment is one-line gear this module laid out.
pub(crate) fn is_oneline(e: &MepEquipment) -> bool {
    e.sources.contains_key(SOURCE_KEY)
}

/// Whether an electrode is one this module laid out (its tag `<prefix>-<n>`).
pub(crate) fn is_oneline_electrode(e: &Electrode) -> bool {
    let pre = format!("{}-", rules().electrode_tag_prefix);
    e.tag.strip_prefix(&pre).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Whether `w` stands in `up` as words of its own (not inside a longer word).
fn has_phrase(up: &str, w: &str) -> bool {
    if w.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(i) = up.get(from..).and_then(|s| s.find(w)).map(|i| i + from) {
        let before = up.get(..i).and_then(|s| s.chars().next_back());
        let after = up.get(i + w.len()..).and_then(|s| s.chars().next());
        let edge = |c: Option<char>| c.is_none_or(|c| !c.is_ascii_alphanumeric());
        // A plural counts (`GROUND RODS`, `METER SOCKETS`).
        let plural = after == Some('S') && edge(up.get(i + w.len() + 1..).and_then(|s| s.chars().next()));
        if edge(before) && (edge(after) || plural) {
            return true;
        }
        from = i + w.len().max(1);
    }
    false
}

/// A note's line: it starts with its number (`27.`, `3)`).
fn note_number(up: &str) -> bool {
    let digits = up.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && up.chars().nth(digits).is_some_and(|c| c == '.' || c == ')')
}

/// The first number in a text, within bounds.
fn first_number(t: &str, lo: f64, hi: f64) -> Option<f64> {
    let mut cur = String::new();
    for c in t.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() || (c == '.' && !cur.is_empty()) {
            cur.push(c);
        } else if !cur.is_empty() {
            if let Ok(v) = cur.trim_end_matches('.').parse::<f64>()
                && v.is_finite()
                && v >= lo
                && v <= hi
            {
                return Some(v);
            }
            cur.clear();
        }
    }
    None
}

/// The words of a text, split at anything but letters, digits, `/`, `.`, `#` and `-`.
fn words(up: &str) -> Vec<&str> {
    up.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '#' | '-'))).filter(|w| !w.is_empty()).collect()
}

/// Every amp rating in a text, in order: `200A`, `200AF`, `175AT`, `200 AMP`, `400AMPS`.
fn amps_all(up: &str) -> Vec<f64> {
    let ws = words(up);
    let mut out = Vec::new();
    for (i, w) in ws.iter().enumerate() {
        for part in w.split('/') {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            let rest = part.get(digits.len()..).unwrap_or_default();
            let Ok(n) = digits.parse::<f64>() else { continue };
            let unit = if rest.is_empty() { ws.get(i + 1).copied().unwrap_or_default() } else { rest };
            if matches!(unit, "A" | "AF" | "AT" | "AMP" | "AMPS" | "AMPERE" | "AMPERES" | "AS") && (1.0..=10_000.0).contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// `750 KVA`, `45KVA`, `112.5 KVA`.
fn kva_in(up: &str) -> Option<f64> {
    let at = up.find("KVA")?;
    let head = up.get(..at)?.trim_end();
    let num: String = head.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
    num.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0 && *v <= 100_000.0)
}

/// Poles: `3P`, `2P`, `200A/3P`, `3 POLE`.
fn poles_in(up: &str) -> Option<u32> {
    let ws = words(up);
    for (i, w) in ws.iter().enumerate() {
        for part in w.split('/') {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            let rest = part.get(digits.len()..).unwrap_or_default();
            let Ok(n) = digits.parse::<u32>() else { continue };
            let unit = if rest.is_empty() { ws.get(i + 1).copied().unwrap_or_default() } else { rest };
            if matches!(unit, "P" | "POLE" | "PL") && (1..=4).contains(&n) {
                return Some(n);
            }
        }
    }
    None
}

/// A voltage as printed: `120/240V`, `208Y/120V`, `480V`.
fn volts_in(up: &str) -> String {
    words(up)
        .into_iter()
        .find(|w| {
            let v = w.trim_end_matches('V');
            w.ends_with('V') && v.chars().filter(char::is_ascii_digit).count() >= 3 && v.chars().all(|c| c.is_ascii_digit() || c == '/' || c == 'Y')
        })
        .unwrap_or_default()
        .to_string()
}

/// `NEMA 3R`, `NEMA-1`, `NEMA 4X`.
fn nema_in(up: &str) -> String {
    let ws = words(up);
    ws.iter()
        .enumerate()
        .find_map(|(i, w)| {
            let t = w.strip_prefix("NEMA")?;
            let t = t.trim_start_matches('-');
            let ty = if t.is_empty() { ws.get(i + 1).copied()? } else { t };
            ty.chars().next().is_some_and(|c| c.is_ascii_digit()).then(|| format!("NEMA {ty}"))
        })
        .unwrap_or_default()
}

/// A conductor size: `#4`, `#2`, `1/0`, `3/0`, `250 KCMIL`.
fn conductor_in(up: &str) -> String {
    let ws = words(up);
    for (i, w) in ws.iter().enumerate() {
        if let Some(n) = w.strip_prefix('#') {
            let n: String = n.chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
            if !n.is_empty() {
                return if n.contains('/') { n } else { format!("#{n}") };
            }
        }
        if matches!(*w, "1/0" | "2/0" | "3/0" | "4/0") {
            return (*w).to_string();
        }
        if w.chars().all(|c| c.is_ascii_digit()) && matches!(ws.get(i + 1).copied(), Some("KCMIL" | "MCM")) {
            return format!("{w} kcmil");
        }
    }
    String::new()
}

/// A size printed on a label: `8"X8"X48"`, `24 X 18 X 8` (three numbers), sorted small to large.
fn printed_size(up: &str) -> Option<[f64; 3]> {
    let flat: String = up.replace('"', "").replace(' ', "");
    for w in flat.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == 'X')) {
        let parts: Vec<&str> = w.split('X').collect();
        if parts.len() != 3 {
            continue;
        }
        let mut v: Vec<f64> = parts.iter().filter_map(|p| p.parse::<f64>().ok()).filter(|x| x.is_finite() && *x >= 1.0 && *x <= 400.0).collect();
        if v.len() == 3 {
            v.sort_by(f64::total_cmp);
            return Some([v[0], v[1], v[2]]);
        }
    }
    None
}

/// How many: `(4) METER SOCKETS`, `(2) GROUND RODS`.
fn count_in(up: &str) -> Option<u32> {
    let t = up.trim_start();
    let inner = t.strip_prefix('(')?.split(')').next()?;
    inner.trim().parse::<u32>().ok().filter(|n| (1..=MAX_COUNT).contains(n))
}

/// Positions of one piece: `6-GANG`, `4 POSITION`, `6 METER` (a meter center's).
fn positions_in(up: &str) -> Option<u32> {
    let ws = words(up);
    for (i, w) in ws.iter().enumerate() {
        let (n, rest) = match w.split_once('-') {
            Some((a, b)) => (a, b),
            None => (*w, ws.get(i + 1).copied().unwrap_or_default()),
        };
        let Ok(n) = n.parse::<u32>() else { continue };
        if (2..=48).contains(&n) && (rest.starts_with("GANG") || rest.starts_with("POSITION")) {
            return Some(n);
        }
    }
    None
}

/// One piece of gear a one-line labels.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Labelled {
    /// A [`KindRule::kind`], or `electrode`.
    pub kind: String,
    /// An electrode's kind (`ground_rod`, `concrete_encased`…).
    pub electrode: String,
    pub sheet: String,
    /// Its lines as printed, top to bottom.
    pub lines: Vec<String>,
    /// Its first line's place and text height; the top of its lines.
    pub at: Vec2,
    pub h: f64,
    pub top: f64,
    pub amps: Option<f64>,
    pub kva: Option<f64>,
    pub fuse_amps: Option<f64>,
    pub poles: Option<u32>,
    pub fused: Option<bool>,
    pub se_rated: bool,
    pub nema: String,
    pub volts: String,
    /// Separate pieces (`(4) METER SOCKETS`) and positions of one piece (`6-GANG`).
    pub count: u32,
    pub positions: Option<u32>,
    /// A size printed on it (sorted small to large, inches).
    pub size_in: Option<[f64; 3]>,
    /// A conductor size printed on it (an electrode's).
    pub conductor: String,
}

impl Labelled {
    pub(crate) fn label(&self) -> String {
        self.lines.join(" / ")
    }
}

/// Whether a sheet's texts title a one-line diagram.
pub(crate) fn is_diagram(texts: &[TextItem]) -> bool {
    let r = rules();
    texts.iter().take(MAX_TEXTS).any(|t| {
        let up = t.value.trim().to_ascii_uppercase();
        up.len() <= 80 && r.diagram_titles.iter().any(|w| has_phrase(&up, w))
    })
}

/// The kind a label names (the longest of the rules' words in it), or `electrode` and its kind.
fn kind_of(up: &str, r: &Rules) -> Option<(String, usize)> {
    let mut best: Option<(String, usize)> = None;
    for k in &r.kinds {
        if k.not_words.iter().any(|w| has_phrase(up, w)) {
            continue;
        }
        for w in k.words.iter().filter(|w| has_phrase(up, w)) {
            if best.as_ref().is_none_or(|b| w.len() > b.1) {
                best = Some((k.kind.clone(), w.len()));
            }
        }
    }
    for w in r.electrode_label_words.iter().filter(|w| has_phrase(up, w)) {
        if best.as_ref().is_none_or(|b| w.len() > b.1) {
            best = Some(("electrode".into(), w.len()));
        }
    }
    best
}

/// The gear a one-line's texts label ([`Labelled`]). `skip`: texts (upper case) that are another
/// table's (a panel schedule row's description), a panel's tag or a feeder tag.
pub(crate) fn read(texts: &[TextItem], sheet: &str, skip: &BTreeSet<String>) -> Vec<Labelled> {
    let r = rules();
    let texts = texts.get(..texts.len().min(MAX_TEXTS)).unwrap_or(texts);
    let usable = |up: &str| {
        !up.is_empty()
            && up.len() <= r.label_max_chars.max(1)
            && !note_number(up)
            && !skip.contains(up)
            && !r.label_skip_prefixes.iter().any(|p| !p.is_empty() && up.starts_with(p.as_str()))
            && !r.diagram_titles.iter().any(|w| has_phrase(up, w))
    };
    let ups: Vec<String> = texts.iter().map(|t| t.value.trim().to_ascii_uppercase()).collect();
    let heads: Vec<(usize, String)> =
        ups.iter().enumerate().filter(|(_, up)| usable(up)).filter_map(|(i, up)| kind_of(up, r).map(|(k, _)| (i, k))).take(MAX_LABELS).collect();
    // Each other short line goes to the nearest label it is a line of (below it, or a rating
    // just above it).
    let mut lines_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (j, (t, up)) in texts.iter().zip(&ups).enumerate() {
        if !usable(up) || heads.iter().any(|(i, _)| *i == j) {
            continue;
        }
        let near = heads
            .iter()
            .filter_map(|(i, _)| {
                let l = texts.get(*i)?;
                let h = l.height.max(1e-9);
                let (dx, dy) = (t.at.x - l.at.x, t.at.y - l.at.y);
                (dy <= 1.2 * h && dy >= -r.label_reach_h.max(0.5) * h && dx.abs() <= r.label_width_h.max(1.0) * h)
                    .then(|| (*i, dx.abs() / r.label_width_h.max(1.0) + dy.abs() / h))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = near {
            lines_of.entry(i).or_default().push(j);
        }
    }
    let mut out = Vec::new();
    for (i, kind) in &heads {
        let Some(l) = texts.get(*i) else { continue };
        let mut members: Vec<usize> = lines_of.get(i).cloned().unwrap_or_default();
        members.push(*i);
        members.sort_by(|a, b| {
            let (ta, tb) = (texts.get(*a).map_or(0.0, |t| t.at.y), texts.get(*b).map_or(0.0, |t| t.at.y));
            tb.total_cmp(&ta).then(a.cmp(b))
        });
        let lines: Vec<String> = members.iter().filter_map(|m| texts.get(*m)).map(|t| t.value.trim().to_string()).collect();
        let up = lines.join(" ").to_ascii_uppercase();
        let amps = amps_all(&up);
        let electrode = if kind == "electrode" {
            r.electrode_words.iter().find(|e| e.words.iter().any(|w| has_phrase(&up, w))).map(|e| e.kind.clone()).unwrap_or_default()
        } else {
            String::new()
        };
        let non_fused = ["NON-FUSED", "NON FUSED", "NONFUSED", "UNFUSED", "NF"].iter().any(|w| has_phrase(&up, w));
        let fused = if non_fused {
            Some(false)
        } else if has_phrase(&up, "FUSED") || has_phrase(&up, "FUSES") || has_phrase(&up, "FUSE") {
            Some(true)
        } else {
            None
        };
        let se = ["SE RATED", "SE-RATED", "SERVICE ENTRANCE RATED", "SERVICE RATED", "SUSE", "SUITABLE FOR USE AS SERVICE", "SERVICE DISCONNECT"]
            .iter()
            .any(|w| has_phrase(&up, w));
        let top = members.iter().filter_map(|m| texts.get(*m)).map(|t| t.at.y + t.height.max(0.0)).fold(f64::NEG_INFINITY, f64::max);
        out.push(Labelled {
            kind: kind.clone(),
            electrode,
            sheet: sheet.to_string(),
            lines,
            at: l.at,
            h: l.height.max(1e-9),
            top: if top.is_finite() { top } else { l.at.y },
            amps: amps.first().copied(),
            kva: kva_in(&up),
            fuse_amps: (fused == Some(true)).then(|| amps.get(1).copied()).flatten(),
            poles: poles_in(&up),
            fused,
            se_rated: se,
            nema: nema_in(&up),
            volts: volts_in(&up),
            count: count_in(&up).unwrap_or(1),
            positions: positions_in(&up),
            size_in: printed_size(&up),
            conductor: conductor_in(&up),
        });
    }
    out
}

/// One connection on a one-line: `to` fed from `from` (gear tags), by a feeder tag or none (bolted
/// together); `no_size` when the tag is printed but the feeder schedule gives no conductors.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Link {
    pub from: String,
    pub to: String,
    pub tag: Option<String>,
    pub no_size: bool,
}

/// A piece of gear on the one-line: its tag and where its label is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Node {
    pub tag: String,
    pub at: Vec2,
    pub top: f64,
    pub h: f64,
}

/// The one-line's connections between `nodes`: each node's incoming tag (a feeder tag in `types`
/// or an unsized one in `no_size`, just above it) and the node above that tag that feeds it;
/// a node right under another with no tag between is bolted to it.
pub(crate) fn links(texts: &[TextItem], nodes: &[Node], types: &[String], no_size: &[String]) -> Vec<Link> {
    let r = rules();
    let texts = texts.get(..texts.len().min(MAX_TEXTS)).unwrap_or(texts);
    let is = |v: &str, list: &[String]| list.iter().any(|x| x.eq_ignore_ascii_case(v.trim()));
    let tags: Vec<&TextItem> = texts.iter().filter(|t| is(&t.value, types) || is(&t.value, no_size)).collect();
    let mut out = Vec::new();
    for n in nodes {
        let h = n.h.max(1e-9);
        let incoming = tags
            .iter()
            .filter(|t| {
                t.at.y > n.top - 0.2 * h
                    && t.at.y - n.top < r.incoming_reach_h.max(1.0) * h
                    && (t.at.x - n.at.x).abs() < r.incoming_side_h.max(0.5) * h
            })
            .min_by(|a, b| (a.at.y - n.top).total_cmp(&(b.at.y - n.top)));
        let from_y = incoming.map_or(n.top, |t| t.at.y);
        let feeder = nodes
            .iter()
            .filter(|o| {
                o.tag != n.tag
                    && o.at.y > from_y
                    && o.at.y - from_y < r.source_reach_h.max(1.0) * h
                    && (o.at.x - n.at.x).abs() < r.source_side_h.max(1.0) * h
            })
            .min_by(|a, b| (a.at.y - from_y).total_cmp(&(b.at.y - from_y)));
        let Some(f) = feeder else { continue };
        let tag = incoming.map(|t| t.value.trim().to_string());
        let unsized_tag = tag.as_deref().is_some_and(|v| is(v, no_size));
        out.push(Link { from: f.tag.clone(), to: n.tag.clone(), tag, no_size: unsized_tag });
    }
    out
}

/// The feeders of `links` whose tag the feeder schedule (`types`) sizes: phases by the fed gear's
/// voltage (`single_phase`), the rest as the schedule's row. Links to gear `fed` already has a
/// feeder to are left to it.
pub(crate) fn feeders(links: &[Link], types: &[FeederType], fed: &BTreeSet<String>, single_phase: &dyn Fn(&str) -> bool, sheet: &str) -> Vec<Feeder> {
    let mut out = Vec::new();
    for l in links {
        if fed.contains(&l.to.to_ascii_uppercase()) || l.no_size {
            continue;
        }
        let Some(ft) = l.tag.as_deref().and_then(|k| types.iter().find(|t| t.tag.eq_ignore_ascii_case(k))) else { continue };
        let phases = if single_phase(&l.to) { 2 } else { 3 }.min(ft.conductors.max(1));
        out.push(Feeder {
            tag: ft.tag.clone(),
            from: l.from.clone(),
            to: l.to.clone(),
            sets: ft.conduits.max(1),
            phases,
            size: ft.size.clone(),
            material: ft.material.clone(),
            neutrals: ft.conductors.saturating_sub(phases),
            grounds: u32::from(!ft.ground_size.is_empty()),
            ground_size: ft.ground_size.clone(),
            conduits: ft.conduits.max(1),
            conduit_in: ft.conduit_in,
            entry: "top".into(),
            source: format!("{}; one-line tag {} above {} ({sheet}), fed from {}", ft.source, ft.tag, l.to, l.from),
            ..Feeder::default()
        });
    }
    out
}

/// A panel's amp rating from its schedule's header (`bus amps`, `rating`, else its main).
pub(crate) fn panel_amps(p: &PanelSchedule) -> Option<f64> {
    let num = |v: &str| first_number(v, 15.0, 10_000.0);
    let by = |keys: &[&str]| p.header.iter().filter(|(k, _)| keys.iter().any(|w| k.contains(w))).find_map(|(_, v)| num(v));
    by(&["bus", "amp", "rating"]).or_else(|| by(&["main", "mcb", "mlo"]))
}

/// How many circuit spaces a panel's schedule uses (its highest pole).
pub(crate) fn panel_circuits(p: &PanelSchedule) -> u32 {
    let top = p
        .circuits
        .iter()
        .filter_map(|c| {
            let n: u32 = c.number.trim().chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()?;
            Some(n.saturating_add(c.poles.max(1).min(4) - 1))
        })
        .max()
        .unwrap_or(0);
    top.max(u32::try_from(p.circuits.len()).unwrap_or(u32::MAX)).min(10_000)
}

/// A panel's box when no submittal gives it (Q6359): the rules' typical enclosure for its rating
/// and circuits, marked inferred, its source saying so.
pub(crate) fn panel_box(p: &PanelSchedule) -> Option<SubmittedGear> {
    let r = rules();
    let given = panel_amps(p);
    let amps = given.unwrap_or(r.panel_default_amps.max(1.0));
    let circuits = panel_circuits(p);
    let row = r.panel_sizes.iter().find(|s| amps <= s.max_amps && circuits <= s.max_circuits).or_else(|| r.panel_sizes.last())?;
    let size = [row.height_in, row.width_in, row.depth_in];
    if !size.iter().all(|v| v.is_finite() && *v > 0.0) {
        return None;
    }
    let rating = given.map_or_else(|| format!("rating not printed, taken as {amps:.0} A"), |a| format!("{a:.0} A"));
    Some(SubmittedGear {
        size_in: size,
        kind: if amps > r.panel_switchboard_over_amps && r.panel_switchboard_over_amps > 0.0 { "switchboard".into() } else { String::new() },
        source: format!("{} ({rating}, {circuits} circuits)", r.panel_size_source),
        ..SubmittedGear::default()
    })
}

/// A piece's box (height, width along its wall, depth, inches) and its source: printed on its
/// label, else the rules' size for its rating.
fn box_of(g: &Labelled, k: &KindRule) -> ([f64; 3], String) {
    if let Some([a, b, c]) = g.size_in {
        // A wireway runs along the wall: its longest side.
        let s = if k.kind == "gutter" { [a, c, b] } else { [c, b, a] };
        return (s, format!("drawn: printed on the one-line's label ({})", g.sheet));
    }
    let rating = if k.rating.eq_ignore_ascii_case("kva") { g.kva } else { g.amps };
    let row = rating.and_then(|v| k.sizes.iter().find(|s| v <= s[0])).or_else(|| k.sizes.first());
    let size = row.map_or([12.0, 12.0, 6.0], |s| [s[1], s[2], s[3]]);
    let why = match rating {
        Some(v) => format!("inferred: {} (rating {v} {})", k.size_source, if k.rating.eq_ignore_ascii_case("kva") { "kVA" } else { "A" }),
        None => format!("inferred: {} (no rating printed: its smallest)", k.size_source),
    };
    (size, why)
}

/// What [`layout`] proposes: the gear, the electrodes, and what it could only flag.
#[derive(Clone, Debug, Default)]
pub(crate) struct Laid {
    pub equipment: Vec<MepEquipment>,
    pub electrodes: Vec<Electrode>,
    /// The gear each piece stands beside (its tag → that gear's), and pieces with nowhere to go.
    pub beside: BTreeMap<String, String>,
    pub review: Vec<String>,
}

/// The tag a piece gets: `<prefix>-<n>`, the first number no equipment has.
fn new_tag(prefix: &str, taken: &mut BTreeSet<String>) -> String {
    let pre = if prefix.is_empty() { "EQ" } else { prefix };
    for n in 1..10_000u32 {
        let t = format!("{pre}-{n}");
        if taken.insert(t.to_ascii_uppercase()) {
            return t;
        }
    }
    format!("{pre}-X")
}

/// The tags each labelled piece gets, in reading order (top to bottom, left to right); a label
/// counting several pieces (`(4) METER SOCKETS`) gets one tag per piece.
pub(crate) fn tags_for(gear: &[Labelled], sp: &BuildingSpec) -> Vec<Vec<String>> {
    let r = rules();
    let mut taken: BTreeSet<String> = sp.mep.equipment.iter().map(|e| e.tag.to_ascii_uppercase()).collect();
    taken.extend(sp.mep.panel_schedules.iter().map(|p| p.panel.to_ascii_uppercase()));
    if let Some(g) = &sp.mep.grounding {
        taken.extend(g.electrodes.iter().map(|e| e.tag.to_ascii_uppercase()));
    }
    let mut order: Vec<(usize, &Labelled)> = gear.iter().enumerate().collect();
    order.sort_by(|(_, a), (_, b)| a.sheet.cmp(&b.sheet).then(b.at.y.total_cmp(&a.at.y)).then(a.at.x.total_cmp(&b.at.x)));
    let mut out = vec![Vec::new(); gear.len()];
    for (i, g) in order {
        let prefix = if g.kind == "electrode" {
            r.electrode_tag_prefix.clone()
        } else {
            r.kinds.iter().find(|k| k.kind == g.kind).map(|k| k.tag_prefix.clone()).unwrap_or_default()
        };
        let n = if g.kind == "electrode" { 1 } else { g.count.clamp(1, MAX_COUNT) };
        if let Some(slot) = out.get_mut(i) {
            *slot = (0..n).map(|_| new_tag(&prefix, &mut taken)).collect();
        }
    }
    out
}

/// The grounding electrode conductor for a service conductor (`size`), and for an electrode kind
/// (a rod's and a concrete-encased electrode's need be no larger than the rules' caps).
pub(crate) fn gec_size(service_conductor: &str, electrode: &str) -> Option<String> {
    let r = rules();
    let c = super::gear::cmil(service_conductor)? as f64;
    let size = r.gec_table.iter().find(|(max, _)| c <= *max).map(|(_, s)| s.clone())?;
    let cap = match electrode {
        "ground_rod" => Some(r.gec_rod_max.as_str()),
        "concrete_encased" => Some(r.gec_ufer_max.as_str()),
        _ => None,
    };
    Some(match cap.and_then(|m| Some((m, super::gear::cmil(m)?))) {
        Some((m, mc)) if super::gear::cmil(&size).is_some_and(|s| s > mc) => m.to_string(),
        _ => size,
    })
}

/// Each labelled piece proposed beside the gear it feeds (following `links` down to gear the
/// model has), in the one-line's order along that gear's wall; the electrodes at the service
/// gear (`service`). `tags`: each label's tags ([`tags_for`]). Gear with nothing to stand beside
/// goes at the corner of the lowest level's plan, flagged for review.
pub(crate) fn layout(sp: &BuildingSpec, gear: &[Labelled], tags: &[Vec<String>], links: &[Link], service_conductor: &str) -> Laid {
    let r = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let inch = upf / 12.0;
    let mut out = Laid::default();
    let placed = |tag: &str| sp.mep.equipment.iter().find(|e| e.tag.eq_ignore_ascii_case(tag));
    // The gear a piece feeds, link by link, until one the model has.
    let downstream = |tag: &str| -> Option<String> {
        let mut at = tag.to_string();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for _ in 0..MAX_HOPS {
            if !seen.insert(at.to_ascii_uppercase()) {
                break;
            }
            let next = links.iter().find(|l| l.from.eq_ignore_ascii_case(&at))?;
            if placed(&next.to).is_some() {
                return Some(next.to.clone());
            }
            at = next.to.clone();
        }
        None
    };
    let slab = super::ugplan::slab_level(sp).unwrap_or_default();
    let fallback_anchor = super::ugplan::service_gear(sp, &slab)
        .or_else(|| {
            sp.mep.equipment.iter().find(|e| matches!(e.kind.as_str(), "panel" | "panelboard" | "switchboard") && e.level.eq_ignore_ascii_case(&slab))
        })
        .or_else(|| sp.mep.equipment.iter().find(|e| matches!(e.kind.as_str(), "panel" | "panelboard" | "switchboard")))
        .map(|e| e.tag.clone());
    // Per anchor, its pieces in the one-line's order (upstream, the higher label, first).
    let mut rows: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    let mut loose: Vec<(usize, usize)> = Vec::new();
    for (i, g) in gear.iter().enumerate() {
        if g.kind == "electrode" {
            continue;
        }
        for (j, tag) in tags.get(i).into_iter().flatten().enumerate() {
            match downstream(tag).or_else(|| fallback_anchor.clone()) {
                Some(a) => rows.entry(a).or_default().push((i, j)),
                None => loose.push((i, j)),
            }
        }
    }
    let order = |v: &mut Vec<(usize, usize)>| {
        let at = |i: usize| gear.get(i).map_or(Vec2::ZERO, |g| g.at);
        v.sort_by(|a, b| at(b.0).y.total_cmp(&at(a.0).y).then(at(a.0).x.total_cmp(&at(b.0).x)).then(a.1.cmp(&b.1)));
    };
    let gap = r.gap_in.max(0.0) * inch;
    let piece = |g: &Labelled, tag: &str, level: &str, lo: [f64; 2], along_x: bool, cursor: &mut f64, beside: Option<&str>| -> Option<MepEquipment> {
        let k = r.kinds.iter().find(|k| k.kind == g.kind)?;
        let ([h, w, dp], size_src) = box_of(g, k);
        let (min, max) = if along_x {
            ([*cursor, lo[1]], [*cursor + w * inch, lo[1] + dp * inch])
        } else {
            ([lo[0], *cursor], [lo[0] + dp * inch, *cursor + w * inch])
        };
        *cursor += w * inch + gap;
        let bottom_in = match k.mount.as_str() {
            "floor" => k.bottom_in.unwrap_or(0.0),
            _ => match (k.center_in, k.top_in) {
                (Some(c), _) => (c - h / 2.0).max(0.0),
                (None, Some(t)) => (t - h).max(0.0),
                _ => k.bottom_in.unwrap_or(48.0),
            },
        };
        let mut properties = BTreeMap::new();
        let mut put = |k: &str, v: String| {
            if !v.is_empty() {
                properties.insert(k.to_string(), v);
            }
        };
        put(LABEL_PROPERTY, g.label());
        put("RATING", g.amps.map(|a| format!("{a:.0} A")).or_else(|| g.kva.map(|v| format!("{v} kVA"))).unwrap_or_default());
        put("POLES", g.poles.map(|p| p.to_string()).unwrap_or_default());
        put(
            "FUSED",
            match (g.fused, g.fuse_amps) {
                (Some(true), Some(f)) => format!("fused, {f:.0} A fuses"),
                (Some(true), None) => "fused".into(),
                (Some(false), _) => "non-fused".into(),
                _ => String::new(),
            },
        );
        put("SERVICE ENTRANCE RATED", if g.se_rated { "yes (the one-line's label)".into() } else { String::new() });
        put("NEMA", g.nema.clone());
        put("VOLTAGE", g.volts.clone());
        put("POSITIONS", g.positions.map(|p| p.to_string()).unwrap_or_default());
        put("BOX", format!("{h} H x {w} W x {dp} D in ({size_src})"));
        let position = match beside {
            Some(b) => format!("proposed: beside {b}, the gear it feeds ({})", r.layout_source),
            None => "review: the model has no gear for it to stand beside; set at the lowest level's corner, its place to confirm".to_string(),
        };
        put("PLACEMENT", position.clone());
        let mut sources = BTreeMap::new();
        sources.insert("size".into(), size_src);
        sources.insert("position".into(), position);
        sources.insert("status".into(), "proposed".into());
        sources.insert(SOURCE_KEY.into(), format!("{}: {}", g.sheet, g.label()));
        let rating = g.amps.map(|a| format!(" {a:.0}A")).or_else(|| g.kva.map(|v| format!(" {v}kVA"))).unwrap_or_default();
        Some(MepEquipment {
            tag: tag.to_string(),
            kind: k.kind.clone(),
            level: level.to_string(),
            min,
            max,
            bottom_ft: bottom_in / 12.0,
            height_ft: h / 12.0,
            title: format!("{}{rating} {tag}", k.title.to_ascii_uppercase()),
            properties,
            sources,
            confidence: 0.5,
            ..MepEquipment::default()
        })
    };
    for (anchor, mut list) in rows {
        let Some(a) = placed(&anchor) else { continue };
        order(&mut list);
        let along_x = (a.max[0] - a.min[0]).abs() >= (a.max[1] - a.min[1]).abs();
        let lo = [a.min[0].min(a.max[0]), a.min[1].min(a.max[1])];
        let mut cursor = if along_x { a.min[0].max(a.max[0]) } else { a.min[1].max(a.max[1]) } + gap;
        for (i, j) in list {
            let (Some(g), Some(tag)) = (gear.get(i), tags.get(i).and_then(|t| t.get(j))) else { continue };
            if let Some(e) = piece(g, tag, &a.level, lo, along_x, &mut cursor, Some(&anchor)) {
                out.beside.insert(e.tag.clone(), anchor.clone());
                out.equipment.push(e);
            }
        }
    }
    if !loose.is_empty() {
        order(&mut loose);
        let level = sp.levels.iter().find(|l| l.name == slab).or_else(|| sp.levels.first());
        let lo = level.and_then(|l| l.region).map_or([0.0, 0.0], |rg| [rg[0].min(rg[2]), rg[1].min(rg[3])]);
        let name = level.map(|l| l.name.clone()).unwrap_or_default();
        let mut cursor = lo[0];
        for (i, j) in loose {
            let (Some(g), Some(tag)) = (gear.get(i), tags.get(i).and_then(|t| t.get(j))) else { continue };
            if let Some(e) = piece(g, tag, &name, lo, true, &mut cursor, None) {
                out.review.push(format!("{} ({}): no gear in the model to stand beside", e.tag, g.label()));
                out.equipment.push(e);
            }
        }
    }
    // The electrodes, at the service gear.
    let service = fallback_anchor.as_deref().and_then(placed).or_else(|| out.equipment.first());
    for (i, g) in gear.iter().enumerate().filter(|(_, g)| g.kind == "electrode") {
        let Some(tag) = tags.get(i).and_then(|t| t.first()) else { continue };
        let Some(svc) = service else {
            out.review.push(format!("{tag} ({}): no service gear to bond it to", g.label()));
            continue;
        };
        let elev = sp.level(&svc.level).map_or(0.0, |l| l.elevation_ft);
        let at = [(svc.min[0] + svc.max[0]) / 2.0, (svc.min[1] + svc.max[1]) / 2.0];
        // A label naming none: the concrete-encased electrode a new footing must have (NEC
        // 250.50); ground rods are the alternative.
        let kind = if g.electrode.is_empty() { "concrete_encased".to_string() } else { g.electrode.clone() };
        let printed = !g.conductor.is_empty();
        let gec = if printed { Some(g.conductor.clone()) } else { gec_size(service_conductor, &kind) };
        let mut e = Electrode {
            tag: tag.clone(),
            kind: kind.clone(),
            level: svc.level.clone(),
            at,
            z_ft: elev + if kind == "water_pipe" || kind == "building_steel" { 1.0 } else { -1.0 },
            gec_size: gec.clone().unwrap_or_default(),
            ..Electrode::default()
        };
        match kind.as_str() {
            "ground_rod" => {
                // One rod alone must prove 25 ohms (NEC 250.53(A)(2)); the field drives two.
                e.count = if g.count > 1 { g.count } else { 2 };
                e.rod_length_ft = r.rod_length_ft.max(1.0);
            }
            "concrete_encased" => {
                e.length_ft = r.ufer_length_ft.max(1.0);
                e.conductor = gec.clone().unwrap_or_else(|| e.conductor.clone());
                e.earth_contact = true;
                e.connection_accessible = true;
            }
            _ => {}
        }
        if gec.is_none() {
            out.review.push(format!("{tag} ({}): its electrode conductor isn't printed and the service conductor isn't known", g.label()));
        }
        out.beside.insert(tag.clone(), svc.tag.clone());
        out.electrodes.push(e);
    }
    out
}

/// What [`build_into`] did: the gear and electrodes it laid out, the feeders it added (and of
/// them, the untagged ones it gave the conductors coming in), the connections left unsized
/// (from, to, tag) and what only a review can place.
#[derive(Clone, Debug, Default)]
pub(crate) struct Outcome {
    pub sheets: Vec<String>,
    pub gear: Vec<String>,
    pub electrodes: Vec<String>,
    pub feeders: usize,
    pub continued: usize,
    pub no_size: Vec<(String, String, String)>,
    pub review: Vec<String>,
    pub by_kind: BTreeMap<String, usize>,
}

/// Whether a voltage text is single-phase (`120/240V`, `1PH`, `1 PH`).
fn single_phase_text(v: &str) -> bool {
    let u = v.to_ascii_uppercase().replace(' ', "");
    u.contains("1PH") || u.contains("1-PH") || u.contains("120/240")
}

/// The one-lines among `pages` read and their gear laid into `sp` ([`read`], [`links`],
/// [`layout`]): the gear beside what it feeds, the electrodes at the service, the feeders their
/// tags size (an untagged connection carries the conductors coming into the piece above it,
/// trade practice; an unsized one is left for review).
pub(crate) fn build_into(sp: &mut BuildingSpec, pages: &[(String, cadaddy_doc::Drawing)], types: &[FeederType], unsized_rows: &[String]) -> Outcome {
    let r = rules();
    let mut out = Outcome::default();
    let type_tags: Vec<String> = types.iter().map(|t| t.tag.clone()).collect();
    let mut no_size: Vec<String> = r.unsized_tags.clone();
    no_size.extend(unsized_rows.iter().cloned());
    // The gear the model and the schedules already name (one-line nodes, never new gear).
    let known: Vec<String> = sp
        .mep
        .panel_schedules
        .iter()
        .map(|p| p.panel.clone())
        .chain(sp.mep.equipment.iter().filter(|e| !e.tag.is_empty()).map(|e| e.tag.clone()))
        .collect();
    let mut skip: BTreeSet<String> = known.iter().map(|t| t.trim().to_ascii_uppercase()).collect();
    skip.extend(type_tags.iter().chain(&no_size).map(|t| t.trim().to_ascii_uppercase()));
    // A schedule's cells are its own (a row's description naming a photocell).
    for p in &sp.mep.panel_schedules {
        skip.extend(p.circuits.iter().map(|c| c.description.trim().to_ascii_uppercase()));
    }
    let mut gear: Vec<Labelled> = Vec::new();
    let mut page_of: Vec<usize> = Vec::new();
    let mut page_texts: Vec<(usize, Vec<TextItem>)> = Vec::new();
    for (pi, (number, d)) in pages.iter().enumerate() {
        let texts = cadaddy_mepfix::sheet_facts::texts_with_tags(d);
        if !is_diagram(&texts) {
            continue;
        }
        let found = read(&texts, number, &skip);
        if found.is_empty() {
            continue;
        }
        out.sheets.push(number.clone());
        page_of.extend(std::iter::repeat_n(pi, found.len()));
        gear.extend(found);
        page_texts.push((pi, texts));
        if gear.len() >= MAX_LABELS {
            break;
        }
    }
    if gear.is_empty() {
        return out;
    }
    let tags = tags_for(&gear, sp);
    // The connections, page by page: the labelled pieces and the known gear's tags as nodes.
    let mut all_links: Vec<Link> = Vec::new();
    for (pi, texts) in &page_texts {
        let mut nodes: Vec<Node> = gear
            .iter()
            .zip(&tags)
            .zip(&page_of)
            .filter(|((g, _), p)| **p == *pi && g.kind != "electrode")
            .filter_map(|((g, t), _)| Some(Node { tag: t.first()?.clone(), at: g.at, top: g.top, h: g.h }))
            .collect();
        for t in texts.iter().take(MAX_TEXTS) {
            if let Some(k) = known.iter().find(|k| k.eq_ignore_ascii_case(t.value.trim())) {
                nodes.push(Node { tag: k.clone(), at: t.at, top: t.at.y + t.height.max(0.0), h: t.height.max(1e-9) });
            }
        }
        for l in links(texts, &nodes, &type_tags, &no_size) {
            let dup = all_links.iter().any(|x| x.from.eq_ignore_ascii_case(&l.from) && x.to.eq_ignore_ascii_case(&l.to));
            if !l.from.eq_ignore_ascii_case(&l.to) && !dup {
                all_links.push(l);
            }
        }
    }
    // An untagged connection carries the conductors coming into the piece above it.
    let mut resolved = all_links.clone();
    let mut continued: BTreeSet<usize> = BTreeSet::new();
    for _ in 0..MAX_HOPS {
        let mut changed = false;
        for i in 0..resolved.len() {
            let Some(l) = resolved.get(i).cloned() else { continue };
            if l.tag.is_some() {
                continue;
            }
            let up = resolved.iter().find(|u| u.to.eq_ignore_ascii_case(&l.from) && u.tag.is_some()).map(|u| (u.tag.clone(), u.no_size));
            if let (Some((tag, unsized_up)), Some(x)) = (up, resolved.get_mut(i)) {
                x.tag = tag;
                x.no_size = unsized_up;
                continued.insert(i);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for l in &resolved {
        match &l.tag {
            Some(t) if l.no_size => out.no_size.push((l.from.clone(), l.to.clone(), t.clone())),
            None => out.no_size.push((l.from.clone(), l.to.clone(), String::new())),
            _ => {}
        }
    }
    // Each piece's phases: its label's voltage, or its schedule's; a piece printing none is on
    // the system feeding it (else the one it feeds).
    let mut single: BTreeMap<String, bool> = BTreeMap::new();
    for (g, t) in gear.iter().zip(&tags) {
        let label = g.label().to_ascii_uppercase();
        if !g.volts.is_empty() || label.contains("PH") {
            for tag in t {
                single.insert(tag.to_ascii_uppercase(), single_phase_text(&label));
            }
        }
    }
    for p in &sp.mep.panel_schedules {
        let v: String =
            p.header.iter().filter(|(k, _)| k.contains("volt") || k.contains("phase")).map(|(_, v)| v.as_str()).collect::<Vec<_>>().join(" ");
        if !v.trim().is_empty() {
            single.insert(p.panel.to_ascii_uppercase(), single_phase_text(&v));
        }
    }
    let fed: BTreeSet<String> = sp.mep.feeders.iter().map(|f| f.to.to_ascii_uppercase()).collect();
    let sheet = out.sheets.join(", ");
    let walk = |start: &str, up: bool| -> Option<bool> {
        let mut at = start.to_ascii_uppercase();
        for _ in 0..MAX_HOPS {
            if let Some(v) = single.get(&at) {
                return Some(*v);
            }
            let next = all_links.iter().find(|l| if up { l.to.eq_ignore_ascii_case(&at) } else { l.from.eq_ignore_ascii_case(&at) })?;
            at = if up { next.from.to_ascii_uppercase() } else { next.to.to_ascii_uppercase() };
        }
        None
    };
    let one_phase = |t: &str| walk(t, true).or_else(|| walk(t, false)).unwrap_or(false);
    let mut new_feeders = feeders(&resolved, types, &fed, &one_phase, &sheet);
    for f in &mut new_feeders {
        let was_continued = resolved
            .iter()
            .enumerate()
            .any(|(i, l)| continued.contains(&i) && l.from.eq_ignore_ascii_case(&f.from) && l.to.eq_ignore_ascii_case(&f.to));
        if was_continued {
            f.source = format!(
                "inferred: no tag on the one-line between {} and {}, so the conductors coming into {} continue (trade practice); {}",
                f.from, f.to, f.from, f.source
            );
            out.continued += 1;
        }
    }
    // The largest service conductor: the electrodes' conductor goes by it.
    let service_conductor = new_feeders
        .iter()
        .chain(sp.mep.feeders.iter())
        .max_by_key(|f| super::gear::cmil(&f.size).unwrap_or(0))
        .map(|f| f.size.clone())
        .unwrap_or_default();
    let laid = layout(sp, &gear, &tags, &all_links, &service_conductor);
    for e in &laid.equipment {
        *out.by_kind.entry(e.kind.clone()).or_default() += 1;
    }
    out.gear = laid.equipment.iter().map(|e| e.tag.clone()).collect();
    out.electrodes = laid.electrodes.iter().map(|e| e.tag.clone()).collect();
    out.review = laid.review.clone();
    out.feeders = new_feeders.len();
    sp.mep.equipment.extend(laid.equipment);
    sp.mep.feeders.extend(new_feeders);
    if !laid.electrodes.is_empty() {
        *out.by_kind.entry("grounding_electrode".into()).or_default() += laid.electrodes.len();
        let service = laid.electrodes.first().and_then(|e| laid.beside.get(&e.tag)).cloned().unwrap_or_default();
        let g = sp.mep.grounding.get_or_insert_with(cadaddy_walk3d::Grounding::default);
        if g.service.trim().is_empty() {
            g.service = service;
        }
        g.electrodes.extend(laid.electrodes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_walk3d::PanelCircuit;

    fn t(v: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: v.to_string(), at: Vec2::new(x, y), height: 1.0, rotation: 0.0 }
    }

    #[test]
    fn rules_data_loads() {
        let r = rules();
        assert!(r.kinds.len() >= 8 && !r.panel_sizes.is_empty() && !r.gec_table.is_empty() && !r.diagram_titles.is_empty());
        assert!(r.kinds.iter().all(|k| !k.sizes.is_empty() && !k.tag_prefix.is_empty() && !k.size_source.is_empty()));
    }

    /// A made-up one-line: a meter socket, a fused service disconnect, the panel and a ground
    /// rod, the feeder tags above each, a panel schedule's row and a note beside it.
    fn oneline() -> Vec<TextItem> {
        vec![
            t("SINGLE LINE DIAGRAM", 0.0, -10.0),
            t("F1", 1.0, 41.0),
            t("200A METER SOCKET", 2.0, 38.0),
            t("120/240V, 1PH, 3W", 2.0, 36.8),
            t("F1", 1.0, 31.0),
            t("200A/2P FUSED DISCONNECT", 2.0, 28.0),
            t("175A FUSES, NEMA 3R, SE RATED", 2.0, 26.8),
            t("XX", 1.0, 21.0),
            t("PX", 2.0, 18.0),
            t("(2) GROUND RODS, #6 CU", 30.0, 18.0),
            t("1. ALL DISCONNECTS SHALL BE HEAVY DUTY", 60.0, 30.0),
            t("EXTERIOR LIGHTING PHOTOCELL", 90.0, 30.0),
        ]
    }

    #[test]
    fn a_one_lines_labels_give_each_piece_its_kind_and_rating() {
        let texts = oneline();
        assert!(is_diagram(&texts));
        assert!(!is_diagram(&[t("PANEL SCHEDULE", 0.0, 0.0)]));
        let skip: BTreeSet<String> = ["PX".to_string(), "F1".into(), "XX".into(), "EXTERIOR LIGHTING PHOTOCELL".into()].into_iter().collect();
        let g = read(&texts, "E-9", &skip);
        let kinds: Vec<&str> = g.iter().map(|x| x.kind.as_str()).collect();
        assert_eq!(kinds, ["meter_socket", "disconnect", "electrode"], "{g:#?}");
        let m = &g[0];
        assert_eq!((m.amps, m.volts.as_str(), m.count), (Some(200.0), "120/240V", 1));
        let d = &g[1];
        assert_eq!(
            (d.amps, d.fuse_amps, d.poles, d.fused, d.se_rated, d.nema.as_str()),
            (Some(200.0), Some(175.0), Some(2), Some(true), true, "NEMA 3R")
        );
        assert_eq!(d.lines, ["200A/2P FUSED DISCONNECT", "175A FUSES, NEMA 3R, SE RATED"]);
        let e = &g[2];
        assert_eq!((e.electrode.as_str(), e.count, e.conductor.as_str()), ("ground_rod", 2, "#6"));
    }

    #[test]
    fn the_one_line_reads_top_down_its_tags_size_the_feeders_and_xx_is_unsized() {
        let texts = oneline();
        let node = |tag: &str, x: f64, y: f64| Node { tag: tag.into(), at: Vec2::new(x, y), top: y + 1.0, h: 1.0 };
        let nodes = [node("MTR-1", 2.0, 38.0), node("DISC-1", 2.0, 28.0), node("PX", 2.0, 18.0)];
        let no_size: Vec<String> = rules().unsized_tags.clone();
        let l = links(&texts, &nodes, &["F1".into()], &no_size);
        assert_eq!(
            l,
            vec![
                Link { from: "MTR-1".into(), to: "DISC-1".into(), tag: Some("F1".into()), no_size: false },
                Link { from: "DISC-1".into(), to: "PX".into(), tag: Some("XX".into()), no_size: true },
            ]
        );
        let ft = FeederType {
            tag: "F1".into(),
            conduits: 1,
            conduit_in: 2.0,
            conductors: 3,
            size: "3/0".into(),
            material: "cu".into(),
            ground_size: "6".into(),
            source: "FEEDER SCHEDULE E-9".into(),
        };
        let f = feeders(&l, &[ft], &BTreeSet::new(), &|_| true, "E-9");
        assert_eq!(f.len(), 1, "the unsized link builds no feeder: {f:?}");
        assert_eq!((f[0].from.as_str(), f[0].to.as_str(), f[0].phases, f[0].neutrals, f[0].size.as_str()), ("MTR-1", "DISC-1", 2, 1, "3/0"));
        assert!(f[0].source.contains("one-line tag F1"), "{}", f[0].source);
    }

    #[test]
    fn a_panel_with_no_submittal_takes_the_typical_box_for_its_rating_and_circuits() {
        let row = |n: &str, p: u32| PanelCircuit { number: n.into(), poles: p, ..PanelCircuit::default() };
        let mut p = PanelSchedule { panel: "PX".into(), ..PanelSchedule::default() };
        p.header.insert("bus amps".into(), "225A".into());
        p.circuits = (0..20).map(|i| row(&(2 * i + 1).to_string(), 1)).collect();
        p.circuits.push(row("41", 2));
        assert_eq!((panel_amps(&p), panel_circuits(&p)), (Some(225.0), 42));
        let g = panel_box(&p).expect("a box");
        assert_eq!(g.size_in, [50.0, 20.0, 5.75]);
        assert!(g.source.starts_with("inferred") && g.source.contains("225 A, 42 circuits"), "{}", g.source);
        // Its main when no bus rating is printed; no rating at all: the rules' default, said.
        let mut q = PanelSchedule { panel: "Q".into(), circuits: vec![row("1", 1)], ..PanelSchedule::default() };
        q.header.insert("mains".into(), "100A MCB".into());
        assert_eq!(panel_box(&q).map(|g| g.size_in), Some([30.0, 20.0, 5.75]));
        q.header.clear();
        assert!(panel_box(&q).is_some_and(|g| g.source.contains("rating not printed")));
    }

    #[test]
    fn the_electrode_conductor_goes_by_the_service_conductor_capped_for_a_rod() {
        assert_eq!(gec_size("3/0", "water_pipe").as_deref(), Some("#4"));
        assert_eq!(gec_size("500 kcmil", "water_pipe").as_deref(), Some("1/0"));
        assert_eq!(gec_size("500 kcmil", "ground_rod").as_deref(), Some("#6"));
        assert_eq!(gec_size("500 kcmil", "concrete_encased").as_deref(), Some("#4"));
        assert_eq!(gec_size("", "ground_rod"), None);
    }

    #[test]
    fn the_pieces_stand_beside_the_gear_they_feed_in_the_one_lines_order() {
        let texts = oneline();
        let skip: BTreeSet<String> = ["PX".to_string(), "F1".into(), "XX".into(), "EXTERIOR LIGHTING PHOTOCELL".into()].into_iter().collect();
        let g = read(&texts, "E-9", &skip);
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        sp.levels = serde_json::from_value(serde_json::json!([{"name": "L1", "elevation_ft": 0.0, "height_ft": 10.0}])).unwrap_or_default();
        sp.mep.equipment.push(MepEquipment {
            tag: "PX".into(),
            kind: "panel".into(),
            level: "L1".into(),
            min: [100.0, 6.0],
            max: [120.0, 11.75],
            height_ft: 4.0,
            ..MepEquipment::default()
        });
        let tags = tags_for(&g, &sp);
        assert_eq!(tags, vec![vec!["MTR-1".to_string()], vec!["DISC-1".to_string()], vec!["GE-1".to_string()]]);
        let l = vec![
            Link { from: "MTR-1".into(), to: "DISC-1".into(), tag: Some("F1".into()), no_size: false },
            Link { from: "DISC-1".into(), to: "PX".into(), tag: None, no_size: false },
        ];
        let laid = layout(&sp, &g, &tags, &l, "3/0");
        let e: Vec<(&str, &str)> = laid.equipment.iter().map(|e| (e.tag.as_str(), e.kind.as_str())).collect();
        assert_eq!(e, [("MTR-1", "meter_socket"), ("DISC-1", "disconnect")]);
        // Along the panel's wall past it, upstream first, 6 in apart; on its level.
        let (m, d) = (&laid.equipment[0], &laid.equipment[1]);
        assert!((m.min[0] - 126.0).abs() < 1e-9 && (m.min[1] - 6.0).abs() < 1e-9, "{m:?}");
        assert!((d.min[0] - (m.max[0] + 6.0)).abs() < 1e-9 && d.level == "L1", "{d:?}");
        assert_eq!(d.properties.get("FUSED").map(String::as_str), Some("fused, 175 A fuses"));
        assert!(d.properties.get("BOX").is_some_and(|b| b.starts_with("28 H x 17 W x 8 D in (inferred")), "{:?}", d.properties);
        assert!(d.sources.get(SOURCE_KEY).is_some_and(|s| s.starts_with("E-9: 200A/2P FUSED DISCONNECT")));
        assert_eq!(laid.beside.get("DISC-1").map(String::as_str), Some("PX"));
        // The rods at the service gear, two of them, their conductor as printed.
        let r = &laid.electrodes[0];
        assert_eq!((r.tag.as_str(), r.kind.as_str(), r.count, r.gec_size.as_str(), r.level.as_str()), ("GE-1", "ground_rod", 2, "#6", "L1"));
        assert!(is_oneline(m) && is_oneline_electrode(r));
        assert!(laid.review.is_empty(), "{:?}", laid.review);
    }

    #[test]
    fn hostile_labels_stay_bounded() {
        let mut texts: Vec<TextItem> = (0..5000).map(|i| t("(999) METER SOCKET 99999999999A", f64::from(i), 0.0)).collect();
        texts.push(t("ONE-LINE DIAGRAM", 0.0, -5.0));
        texts.push(t("(9999999999999999999) GROUND RODS #", f64::NAN, f64::INFINITY));
        let g = read(&texts, "X", &BTreeSet::new());
        assert!(g.len() <= MAX_LABELS && g.iter().all(|x| x.count <= MAX_COUNT && x.amps.is_none()));
        let sp = BuildingSpec::default();
        let tags = tags_for(&g, &sp);
        let laid = layout(&sp, &g, &tags, &[], "");
        assert!(laid.equipment.len() <= MAX_LABELS * MAX_COUNT as usize);
    }
}
