//! Underground services from the civil sheets (Q348): the water, sewer, storm, gas and
//! electric/telecom duct bank routes drawn on `C-UNDR`, read with what their labels print (kind,
//! size, depth, invert), so that `mep.services` (and the underground sheets, `ugsheets`) get them
//! from the prints instead of by hand.
//!
//! A route is the lines and open polylines of `C-UNDR` joined end to end (the short ticks at a
//! run's ends left out). Each label on the layer belongs to the route nearest it. A label that
//! names no kind leaves the route `unknown`: nothing is guessed from where a line runs. The
//! building is the `C-SITE` rectangle labelled `BUILDING`; a route is ordered from its site end
//! to the end nearest the building, which is where it enters. Each service keeps the sheet and
//! the box it was read from, and a confidence.

use serde_json::{Value, json};

use crate::layout::{SITE_LAYER, UNDERGROUND_LAYER};
use crate::score::building_from_model;
use crate::truth::Sheet;
use cadaddy_doc::{Drawing, EntityKind};

/// Pieces of a sheet looked at, at most (hostile drawings).
const MAX_PIECES: usize = 20_000;
/// Points of one route, at most.
const MAX_ROUTE: usize = 1_000;
/// A piece shorter than this share of the layer's average is a tick, not a run.
const TICK_SHARE: f64 = 0.3;
/// Two routes the same within this (feet) are one service seen on two sheets.
const SAME_FT: f64 = 1.0;
/// A route's building end must be within this many drawing units of the BUILDING box to count as
/// entering. Civil sheets are typically in inches, so 120 = 10 ft.
const BUILDING_TOLERANCE: f64 = 120.0;

/// An underground service read from a sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadService {
    /// `water`, `sewer`, `storm`, `gas`, `power`, `telephone`, `catv`, or `unknown`.
    pub kind: String,
    /// The labels that went with it, as printed.
    pub labels: Vec<String>,
    /// Pipe or duct size, inches, when printed (`4" WATER`).
    pub size_in: Option<f64>,
    /// Conduits in a duct bank, when printed (`4-4" PVC`).
    pub conduits: Option<u32>,
    /// Building feet, from the site end to the building end when `enters`.
    pub route_ft: Vec<[f64; 2]>,
    /// The route's last point is at the building (a building outline was found on the sheet).
    pub enters: bool,
    pub depth_ft: Option<f64>,
    pub invert_ft: Option<f64>,
    /// The sheet number and the box (the sheet's drawing coordinates) it was read from.
    pub sheet: String,
    pub found_in: [[f64; 2]; 2],
    /// 0 … 1: a printed kind 0.7, and 0.1 each for a size, a depth or invert, and where it enters.
    /// An unknown route 0.2.
    pub confidence: f64,
}

/// The underground services on one sheet.
pub fn read_services(d: &Drawing, sh: &Sheet) -> Vec<ReadService> {
    let mut pieces: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut labels: Vec<(String, [f64; 2])> = Vec::new();
    let mut boxes: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut site_texts: Vec<(String, [f64; 2])> = Vec::new();
    for e in d.model.iter().take(MAX_PIECES) {
        let layer = e.common.layer.as_str();
        let undr = layer.eq_ignore_ascii_case(UNDERGROUND_LAYER);
        let site = layer.eq_ignore_ascii_case(SITE_LAYER);
        if !undr && !site {
            continue;
        }
        match &e.kind {
            EntityKind::Line(l) if undr => pieces.push(vec![[l.a.x, l.a.y], [l.b.x, l.b.y]]),
            EntityKind::LwPolyline(p) if undr && !p.closed => pieces.push(p.vertices.iter().take(MAX_ROUTE).map(|v| [v.p.x, v.p.y]).collect()),
            EntityKind::LwPolyline(p) if site && p.closed && p.vertices.len() == 4 => {
                let (mut mn, mut mx) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
                for v in &p.vertices {
                    mn = [mn[0].min(v.p.x), mn[1].min(v.p.y)];
                    mx = [mx[0].max(v.p.x), mx[1].max(v.p.y)];
                }
                boxes.push((mn, mx));
            }
            EntityKind::Text(t) => {
                let at = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                if undr { labels.push((t.value.clone(), at)) } else { site_texts.push((t.value.clone(), at)) }
            }
            EntityKind::MText(t) => {
                let at = [t.insert.x, t.insert.y];
                if undr { labels.push((t.contents.clone(), at)) } else { site_texts.push((t.contents.clone(), at)) }
            }
            _ => {}
        }
    }
    let pieces: Vec<Vec<[f64; 2]>> = pieces.into_iter().filter(|p| p.len() >= 2 && p.iter().all(|q| q[0].is_finite() && q[1].is_finite())).collect();
    if pieces.is_empty() {
        return Vec::new();
    }
    // The ticks at a run's ends are short against the runs.
    let average = pieces.iter().map(|p| length(p)).sum::<f64>() / pieces.len() as f64;
    let runs: Vec<Vec<[f64; 2]>> = join(pieces.into_iter().filter(|p| length(p) >= average * TICK_SHARE).collect());
    // The building: a site rectangle with BUILDING written inside it.
    let building =
        boxes.iter().copied().find(|(mn, mx)| site_texts.iter().any(|(t, p)| t.trim().eq_ignore_ascii_case("BUILDING") && inside(*p, *mn, *mx)));
    // Each label to the route nearest it, within half that route's length.
    let mut by_run: Vec<Vec<&str>> = vec![Vec::new(); runs.len()];
    for (text, at) in &labels {
        let best = runs.iter().enumerate().map(|(i, r)| (i, distance(*at, r))).min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, dist)) = best
            && runs.get(i).is_some_and(|r| dist <= length(r) * 0.5)
            && let Some(v) = by_run.get_mut(i)
        {
            v.push(text.as_str());
        }
    }
    runs.into_iter()
        .zip(by_run)
        .map(|(mut run, texts)| {
            let mut s = ReadService { sheet: sh.number.clone(), ..ReadService::default() };
            for t in &texts {
                let words = t.to_ascii_uppercase();
                if s.kind.is_empty() {
                    s.kind = kind_of(&words).unwrap_or_default().to_string();
                }
                s.size_in = s.size_in.or_else(|| size_of(&words));
                s.conduits = s.conduits.or_else(|| conduits_of(&words));
                s.depth_ft = s.depth_ft.or_else(|| depth_of(&words));
                s.invert_ft = s.invert_ft.or_else(|| invert_of(&words));
                s.labels.push(t.trim().to_string());
            }
            if let Some((mn, mx)) = building {
                let gap = |p: Option<&[f64; 2]>| p.map_or(f64::INFINITY, |p| box_distance(*p, mn, mx));
                if gap(run.first()) < gap(run.last()) {
                    run.reverse();
                }
                // Only mark "enters" when the building end is actually near the building.
                s.enters = gap(run.last()) <= BUILDING_TOLERANCE;
            }
            let (mut mn, mut mx) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for p in &run {
                mn = [mn[0].min(p[0]), mn[1].min(p[1])];
                mx = [mx[0].max(p[0]), mx[1].max(p[1])];
            }
            s.found_in = [mn, mx];
            s.route_ft = run.iter().map(|p| building_from_model(sh, *p)).collect();
            s.confidence = if s.kind.is_empty() {
                s.kind = "unknown".into();
                0.2
            } else {
                0.7 + [s.size_in.is_some(), s.depth_ft.is_some() || s.invert_ft.is_some(), s.enters].iter().filter(|b| **b).count() as f64 * 0.1
            };
            s
        })
        .collect()
}

/// The services of several sheets, each once: a route drawn on two sheets (the site plan and the
/// site utility plan) is kept from the first, with what the second printed that it didn't.
pub fn merge(sheets: Vec<Vec<ReadService>>) -> Vec<ReadService> {
    let mut out: Vec<ReadService> = Vec::new();
    for s in sheets.into_iter().flatten() {
        let same = out.iter_mut().find(|o| (o.kind == s.kind || o.kind == "unknown" || s.kind == "unknown") && same_route(&o.route_ft, &s.route_ft));
        match same {
            Some(o) => {
                if o.kind == "unknown" && s.kind != "unknown" {
                    o.kind = s.kind.clone();
                }
                o.size_in = o.size_in.or(s.size_in);
                o.conduits = o.conduits.or(s.conduits);
                o.depth_ft = o.depth_ft.or(s.depth_ft);
                o.invert_ft = o.invert_ft.or(s.invert_ft);
                o.confidence = o.confidence.max(s.confidence);
            }
            None => out.push(s),
        }
    }
    out
}

/// The building spec patch: `mep.services`, each on `level`. `to_plan` maps the level's building
/// feet to its plan's coordinates (none: the level isn't built). A route that doesn't reach a
/// building has no `at` (the drawings don't show where it enters).
pub fn spec_patch(services: &[ReadService], level: &str, to_plan: &dyn Fn(&str, [f64; 2]) -> Option<[f64; 2]>) -> Value {
    let mut tags: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let out: Vec<Value> = services
        .iter()
        .filter_map(|s| {
            let route: Vec<[f64; 2]> = s.route_ft.iter().map(|p| to_plan(level, *p)).collect::<Option<_>>()?;
            let n = tags.entry(s.kind.clone()).or_default();
            *n += 1;
            let is_unknown = s.kind == "unknown";
            let mut v = json!({
                "kind": s.kind,
                "tag": format!("SVC-{}{}", s.kind.to_ascii_uppercase(), if *n > 1 { format!("-{n}") } else { String::new() }),
                "level": level,
                "from": route.first(),
                "route": route,
                "shown": !is_unknown,
                "source": s.sheet,
                "found_in": s.found_in,
                "confidence": (s.confidence * 100.0).round() / 100.0,
            });
            if s.enters && !is_unknown {
                v["at"] = json!(route.last());
            }
            if let Some(size) = s.size_in {
                v["size_in"] = json!(size);
                if matches!(s.kind.as_str(), "power" | "telephone" | "catv") {
                    v["conduit_in"] = json!(size);
                }
            }
            if let Some(n) = s.conduits {
                v["conduits"] = json!(n);
            }
            if let Some(dep) = s.depth_ft {
                v["depth_ft"] = json!(dep);
                v["cover_in"] = json!(dep * 12.0);
            }
            if let Some(inv) = s.invert_ft {
                v["invert_ft"] = json!(inv);
            }
            Some(v)
        })
        .collect();
    json!({ "services": out })
}

/// The service a label names, by its words; none when it names none.
fn kind_of(words: &str) -> Option<&'static str> {
    let tokens: Vec<&str> = words.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &[&str]| tokens.iter().any(|t| w.contains(t));
    if has(&["WATER", "CW", "DW", "DOMESTIC"]) {
        Some("water")
    } else if has(&["SEWER", "SAN", "SANITARY", "SS"]) {
        Some("sewer")
    } else if has(&["STORM", "SD", "STM"]) {
        Some("storm")
    } else if has(&["GAS"]) {
        Some("gas")
    } else if has(&["TELECOM", "TEL", "TELEPHONE", "COMM", "COMMUNICATIONS", "FIBER", "FO"]) {
        Some("telephone")
    } else if has(&["CATV", "CABLE"]) {
        Some("catv")
    } else if has(&["ELECTRIC", "ELECTRICAL", "ELEC", "POWER", "PRIMARY", "SECONDARY"]) {
        Some("power")
    } else {
        None
    }
}

/// A number and what follows it in `words`, for each number.
fn numbers(words: &str) -> Vec<(f64, &str)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut prev: Option<char> = None;
    for (i, c) in words.char_indices().chain([(words.len(), ' ')]) {
        let digit = c.is_ascii_digit() || (c == '.' && start.is_some());
        match (start, digit) {
            (None, true) if prev.is_none_or(|p| !(p.is_ascii_alphanumeric() || p == '.')) => start = Some(i),
            (Some(s), false) => {
                if let (Some(n), Some(rest)) = (words.get(s..i).and_then(|t| t.parse::<f64>().ok()), words.get(i..)) {
                    out.push((n, rest));
                }
                start = None;
            }
            _ => {}
        }
        prev = Some(c);
    }
    out
}

/// `4" WATER`, `8 IN SAN`, `6"Ø`: the first number marked as inches.
fn size_of(words: &str) -> Option<f64> {
    numbers(words)
        .into_iter()
        .find(|(n, rest)| *n > 0.0 && (rest.starts_with(['"', 'Ø']) || rest.starts_with(" IN") || rest.starts_with("IN")))
        .map(|(n, _)| n)
}

/// `4-4" PVC`: conduits before a dash and a size.
fn conduits_of(words: &str) -> Option<u32> {
    let (count, rest) = words.split_once('-')?;
    let count = count.trim().rsplit(' ').next()?.parse::<u32>().ok()?;
    (size_of(rest).is_some() && (1..=64).contains(&count)).then_some(count)
}

/// `3' DEEP`, `4 FT DEEP`, `DEPTH 3'`, `36" COVER` (→ 3.0 ft), `COVER 30 IN` (→ 2.5 ft).
/// Returns feet: a number before DEEP/DEPTH/COVER, or right after, with its unit (`'`/FT = feet,
/// `"`/IN = inches → feet; bare = feet).
fn depth_of(words: &str) -> Option<f64> {
    let kw_pos = ["DEEP", "DEPTH", "COVER"]
        .iter()
        .filter_map(|k| {
            let mut from = 0;
            while let Some(i) = words.get(from..).and_then(|w| w.find(k)) {
                let i = from + i;
                let before = words.get(..i).and_then(|w| w.chars().last());
                if before.is_none_or(|c| !c.is_ascii_alphanumeric()) {
                    return Some((i, k.len()));
                }
                from = i + k.len();
            }
            None
        })
        .next()?;
    let (kw_start, kw_len) = kw_pos;
    // Look for a number right after the keyword first, then before it.
    let after = words.get(kw_start + kw_len..).and_then(|rest| numbers(rest).into_iter().next().filter(|(n, _)| *n > 0.0 && *n < 100.0));
    let before = || numbers(words.get(..kw_start)?).into_iter().next_back().filter(|(n, _)| *n > 0.0 && *n < 100.0);
    let (n, rest) = after.or_else(before)?;
    // Unit: inches → feet; feet or bare → feet.
    if rest.starts_with('"') || rest.starts_with(" IN") || rest.starts_with("IN") { Some(n / 12.0) } else { Some(n) }
}

/// `INV 95.50`, `INV. EL. 95.5`, `IE=95.5`, `INVERT 95.5`.
fn invert_of(words: &str) -> Option<f64> {
    let at = ["INVERT", "INV", "IE"].iter().find_map(|k| {
        let mut from = 0;
        while let Some(i) = words.get(from..).and_then(|w| w.find(k)) {
            let i = from + i;
            let before = words.get(..i).and_then(|w| w.chars().last());
            if before.is_none_or(|c| !c.is_ascii_alphanumeric()) {
                return Some(i + k.len());
            }
            from = i + k.len();
        }
        None
    })?;
    numbers(words.get(at..)?).first().map(|(n, _)| *n)
}

/// Pieces joined where one ends at another's end (in order, either way round).
fn join(mut pieces: Vec<Vec<[f64; 2]>>) -> Vec<Vec<[f64; 2]>> {
    let close = |a: &[f64; 2], b: &[f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-6 * (1.0 + a[0].abs().max(a[1].abs()));
    let mut out: Vec<Vec<[f64; 2]>> = Vec::new();
    while let Some(mut run) = pieces.pop() {
        while let (Some(head), Some(tail)) = (run.first().copied(), run.last().copied()) {
            let Some(i) = pieces.iter().position(|p| {
                p.first().is_some_and(|q| close(q, &tail) || close(q, &head)) || p.last().is_some_and(|q| close(q, &tail) || close(q, &head))
            }) else {
                break;
            };
            let mut p = pieces.swap_remove(i);
            if p.first().is_some_and(|q| close(q, &tail)) {
                run.extend(p.into_iter().skip(1));
            } else if p.last().is_some_and(|q| close(q, &tail)) {
                p.reverse();
                run.extend(p.into_iter().skip(1));
            } else if p.last().is_some_and(|q| close(q, &head)) {
                p.extend(run.into_iter().skip(1));
                run = p;
            } else {
                p.reverse();
                p.extend(run.into_iter().skip(1));
                run = p;
            }
            if run.len() > MAX_ROUTE {
                run.truncate(MAX_ROUTE);
                break;
            }
        }
        out.push(run);
    }
    out.reverse();
    out
}

fn length(p: &[[f64; 2]]) -> f64 {
    p.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum()
}

/// From `p` to the nearest point of the polyline `r`.
fn distance(p: [f64; 2], r: &[[f64; 2]]) -> f64 {
    r.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
        })
        .fold(f64::INFINITY, f64::min)
}

fn inside(p: [f64; 2], mn: [f64; 2], mx: [f64; 2]) -> bool {
    p[0] >= mn[0] && p[0] <= mx[0] && p[1] >= mn[1] && p[1] <= mx[1]
}

fn box_distance(p: [f64; 2], mn: [f64; 2], mx: [f64; 2]) -> f64 {
    let dx = (mn[0] - p[0]).max(p[0] - mx[0]).max(0.0);
    let dy = (mn[1] - p[1]).max(p[1] - mx[1]).max(0.0);
    dx.hypot(dy)
}

/// The same route: the same ends (either way round) within [`SAME_FT`].
fn same_route(a: &[[f64; 2]], b: &[[f64; 2]]) -> bool {
    let near = |p: Option<&[f64; 2]>, q: Option<&[f64; 2]>| matches!((p, q), (Some(p), Some(q)) if (p[0] - q[0]).hypot(p[1] - q[1]) <= SAME_FT);
    (near(a.first(), b.first()) && near(a.last(), b.last())) || (near(a.first(), b.last()) && near(a.last(), b.first()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_say_what_they_print_and_nothing_more() {
        assert_eq!(kind_of("4\" WATER"), Some("water"));
        assert_eq!(kind_of("8\" SAN"), Some("sewer"));
        assert_eq!(kind_of("TELECOM"), Some("telephone"));
        assert_eq!(kind_of("ELECTRIC"), Some("power"));
        assert_eq!(kind_of("GASKET"), None);
        assert_eq!(kind_of("XYZ"), None);
        assert_eq!(size_of("4\" WATER"), Some(4.0));
        assert_eq!(size_of("8 IN SAN"), Some(8.0));
        assert_eq!(size_of("WATER"), None);
        assert_eq!(size_of("3' DEEP"), None);
        assert_eq!(conduits_of("4-4\" PVC ELECTRIC"), Some(4));
        assert_eq!(depth_of("3' DEEP"), Some(3.0));
        assert_eq!(depth_of("4-4\" PVC ELECTRIC 3' DEEP"), Some(3.0));
        assert_eq!(depth_of("36\" COVER"), Some(3.0));
        assert_eq!(depth_of("COVER 30 IN"), Some(2.5));
        assert_eq!(depth_of("WATER"), None);
        assert_eq!(invert_of("INV 95.50"), Some(95.5));
        assert_eq!(invert_of("IE=94.2"), Some(94.2));
        assert_eq!(invert_of("PRIVATE"), None);
    }
}
