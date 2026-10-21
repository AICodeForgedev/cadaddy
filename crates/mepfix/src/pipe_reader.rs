//! Pipe reader: reads single-line piping runs from DXF sheets (plumbing, fire protection,
//! hydronic) and returns [`FoundRun`] entries that can be scored against the truth.
//!
//! Piping on the generated fixtures is drawn as single lines on layers `PIPE`, `HIDDEN`
//! (below floor), `DASHED` (vent) and `HEAVY` (fire protection mains, sanitary). The system
//! is identified from inline text labels at the run's midpoint (e.g. `CW`, `HW`, `SAN`) or
//! from size tags that carry the system code (e.g. `2" CW`). Heights come from tags like
//! `BOP @ 10'-0"` or `CL @ 9'-6"`. Slopes come from `@ 1/4"/FT`. Risers are circles on the
//! PIPE layer with fill or mark symbols, annotated with notes like `3/4" CW UP`.

use crate::score::{building_from_model, FoundRun};
use crate::truth::Sheet;
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;

/// The pipe layers the generator draws on.
const PIPE_LAYERS: &[&str] = &["PIPE", "HIDDEN", "DASHED", "HEAVY"];

/// Maximum runs and segments to prevent hostile input from eating memory.
const MAX_RUNS: usize = 20_000;
const MAX_SEGS: usize = 200_000;

/// A raw line segment in model inches.
#[derive(Clone, Debug)]
struct RawSeg {
    a: Vec2,
    b: Vec2,
    layer: String,
}

/// A text entity's content and model-space position.
#[derive(Clone, Debug)]
struct TextItem {
    text: String,
    at: Vec2,
}

/// A detected riser symbol.
#[derive(Clone, Debug)]
pub struct DetectedRiser {
    pub at_model: Vec2,
    pub at_ft: [f64; 2],
    pub system: String,
    pub size_in: f64,
    pub direction: String, // "up", "down", "through"
    pub note: String,
}

/// Read piping runs from a DXF drawing using the sheet's coordinate transform.
///
/// Returns found runs in building feet, ready for scoring.
pub fn read_pipes(d: &Drawing, sh: &Sheet) -> (Vec<FoundRun>, Vec<DetectedRiser>) {
    let mut segs = Vec::new();
    let mut texts = Vec::new();
    let mut circles: Vec<(Vec2, f64, String)> = Vec::new();

    // Collect line segments on pipe layers.
    for e in d.model.iter() {
        let layer = e.common.layer.to_uppercase();
        let on_pipe = PIPE_LAYERS.iter().any(|l| layer.eq_ignore_ascii_case(l));
        if on_pipe {
            match &e.kind {
                EntityKind::Line(l) => {
                    if segs.len() < MAX_SEGS {
                        segs.push(RawSeg { a: l.a.xy(), b: l.b.xy(), layer: layer.clone() });
                    }
                }
                EntityKind::LwPolyline(p) => {
                    let pts: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
                    for w in pts.windows(2) {
                        if segs.len() >= MAX_SEGS {
                            break;
                        }
                        if let [a, b] = w {
                            segs.push(RawSeg { a: *a, b: *b, layer: layer.clone() });
                        }
                    }
                }
                EntityKind::Circle(c) => {
                    circles.push((Vec2::new(c.center.x, c.center.y), c.radius, layer.clone()));
                }
                _ => {}
            }
        }
        // Collect all text entities (on any layer — tags and labels may be on TEXT).
        match &e.kind {
            EntityKind::Text(t) => {
                let pt = t.align_pt.map_or(Vec2::new(t.insert.x, t.insert.y), |p| Vec2::new(p.x, p.y));
                texts.push(TextItem { text: t.value.clone(), at: pt });
            }
            EntityKind::MText(t) => {
                texts.push(TextItem { text: t.contents.clone(), at: Vec2::new(t.insert.x, t.insert.y) });
            }
            _ => {}
        }
    }

    // Chain segments end-to-end into runs.
    let runs = chain_segments(&segs);

    // For each run, identify system, size, height, slope from nearby text.
    let mut found = Vec::new();
    for (pts, layer) in &runs {
        if pts.len() < 2 {
            continue;
        }
        let Some(level) = &sh.level else { continue };

        // Run centreline in building feet.
        let from_ft = building_from_model(sh, [pts[0].x, pts[0].y]);
        let to_ft = building_from_model(sh, [pts[pts.len() - 1].x, pts[pts.len() - 1].y]);

        let total_len = run_length(pts);
        if total_len < 1e-9 {
            continue;
        }

        // Collect texts near the run: 4 building feet in model inches.
        let reach = 4.0 * 12.0; // 4 feet × 12 inches/foot = 48 model inches
        let near_texts: Vec<&TextItem> = texts.iter().filter(|t| seg_point_dist(&run_segments(pts), t.at) < reach).collect();

        // Identify system from inline text breaks or tags.
        let system = identify_system(&near_texts, layer);
        let (size_in, tag_system) = parse_size_from_texts(&near_texts);
        let system = tag_system.unwrap_or(system);
        let size = size_in.unwrap_or(default_size(&system));

        // Parse height.
        let section_ft = size / 12.0;
        let bottom_ft = parse_height_from_texts(&near_texts, section_ft);

        // Parse slope (for drains).
        let _slope = parse_slope_from_texts(&near_texts);

        // Determine if below floor (HIDDEN layer).
        let below_floor = layer.eq_ignore_ascii_case("HIDDEN");

        // Skip below-floor segments (the scorer's expected() excludes them).
        if below_floor {
            continue;
        }

        found.push(FoundRun {
            level: level.clone(),
            kind: "pipe".to_string(),
            system,
            shape: "pipe".to_string(),
            size_in: [size, size],
            from_ft,
            to_ft,
            bottom_ft,
        });

        if found.len() >= MAX_RUNS {
            break;
        }
    }

    // Detect risers from circles and their annotation text.
    let risers = detect_risers(&circles, &texts, sh);

    (found, risers)
}

/// Chain line segments sharing endpoints into runs. Returns (points, representative layer).
fn chain_segments(segs: &[RawSeg]) -> Vec<(Vec<Vec2>, String)> {
    let tol = 1e-3;
    let mut pieces: Vec<(Vec<Vec2>, String)> = segs.iter().map(|s| (vec![s.a, s.b], s.layer.clone())).collect();
    let mut out: Vec<(Vec<Vec2>, String)> = Vec::new();

    while let Some(mut cur) = (!pieces.is_empty()).then(|| pieces.remove(0)) {
        while let (Some(&head), Some(&tail)) = (cur.0.first(), cur.0.last()) {
            let found = pieces.iter().position(|(p, l)| {
                l == &cur.1
                    && (p.first().is_some_and(|q| q.dist(tail) < tol || q.dist(head) < tol)
                        || p.last().is_some_and(|q| q.dist(tail) < tol || q.dist(head) < tol))
            });
            let Some(i) = found else { break };
            let (mut p, _) = pieces.remove(i);
            if p.first().is_some_and(|q| q.dist(tail) < tol) {
                cur.0.extend(p.into_iter().skip(1));
            } else if p.last().is_some_and(|q| q.dist(tail) < tol) {
                p.reverse();
                cur.0.extend(p.into_iter().skip(1));
            } else if p.last().is_some_and(|q| q.dist(head) < tol) {
                p.pop();
                p.extend(cur.0);
                cur.0 = p;
            } else {
                p.reverse();
                p.pop();
                p.extend(cur.0);
                cur.0 = p;
            }
        }
        out.push(cur);
        if out.len() >= MAX_RUNS {
            break;
        }
    }
    out
}

fn run_length(pts: &[Vec2]) -> f64 {
    pts.windows(2).map(|w| w[0].dist(w[1])).sum()
}

fn run_segments(pts: &[Vec2]) -> Vec<(Vec2, Vec2)> {
    pts.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Distance from a point to the nearest segment in a run.
fn seg_point_dist(segs: &[(Vec2, Vec2)], p: Vec2) -> f64 {
    segs.iter()
        .map(|(a, b)| {
            let d = *b - *a;
            let l2 = d.dot(d);
            let t = if l2 > 1e-18 { ((p - *a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
            p.dist(*a + d * t)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Identify the piping system from nearby text or the layer.
fn identify_system(texts: &[&TextItem], layer: &str) -> String {
    // First, check for inline system labels (the generator puts them in line breaks).
    // These are short texts that are just the system abbreviation.
    for t in texts {
        let upper = t.text.to_uppercase();
        let trimmed = upper.trim();
        // Exact match on a system abbreviation.
        if let Some((_, code)) = SYS_ABBR.iter().find(|(abbr, _)| abbr.eq_ignore_ascii_case(trimmed)) {
            return code.to_string();
        }
    }
    // Infer from layer: DASHED = vent, HEAVY with no system text might be SAN.
    match layer.to_uppercase().as_str() {
        "DASHED" => "V".to_string(),
        _ => String::new(),
    }
}

/// Parse a pipe size and optional system from nearby text tags.
///
/// Tags look like `2" CW`, `3/4" HW`, `4" SAN`, `1" FP`. Returns (size_inches, system).
fn parse_size_from_texts(texts: &[&TextItem]) -> (Option<f64>, Option<String>) {
    for t in texts {
        let upper = t.text.to_uppercase().replace('\u{2033}', "\""); // typographic double prime
        let words: Vec<&str> = upper.split_whitespace().collect();
        for (i, w) in words.iter().enumerate() {
            if !w.contains('"') || w.contains('\'') {
                continue;
            }
            // Extract the numeric part before the inch mark.
            let digits: String = w.chars().take_while(|c| c.is_ascii_digit() || *c == '/' || *c == '.').collect();
            let value = parse_pipe_num(&digits);
            // Check for whole-number prefix: `1 1/2"`
            let value = match (i > 0 && digits.contains('/'), value) {
                (true, Some(frac)) => {
                    let whole = words.get(i.wrapping_sub(1)).and_then(|p| p.parse::<f64>().ok());
                    match whole {
                        Some(w) => Some(w + frac),
                        None => Some(frac),
                    }
                }
                (_, v) => v,
            };
            if let Some(v) = value.filter(|v| *v > 0.0 && *v <= 24.0) {
                // System code after the size: `2" CW`, `4" SS`, `1" F`.
                let sys = words.get(i + 1).and_then(|s| {
                    let s = s.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
                    SYS_ABBR.iter().find(|(abbr, _)| abbr.eq_ignore_ascii_case(s)).map(|(_, code)| code.to_string())
                });
                return (Some(v), sys);
            }
        }
    }
    (None, None)
}

fn parse_pipe_num(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('"');
    if let Some((a, b)) = s.split_once('/') {
        let (whole, a) = match a.trim().rsplit_once(' ') {
            Some((w, a)) => (w.trim().parse::<f64>().ok()?, a),
            None => (0.0, a),
        };
        Some(whole + a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok().filter(|b| *b != 0.0)?)
    } else {
        s.parse::<f64>().ok()
    }
}

/// Parse height from nearby text: `BOP @ 10'-6"`, `CL @ 9'-6"`, `B.O.P. 10'-0" AFF`.
fn parse_height_from_texts(texts: &[&TextItem], section_ft: f64) -> Option<f64> {
    for t in texts {
        let upper = t.text.to_uppercase();
        // Bottom of pipe.
        if let Some(v) = height_after(&upper, &["BOP", "B.O.P", "BOT", "BOD", "B.O."]) {
            return Some(v);
        }
        // Centreline.
        if let Some(v) = height_after(&upper, &["CL ", "CL@", "C.L.", "CTR", "CENTER", "CENTRE"]) {
            return Some(v - section_ft / 2.0);
        }
        // Top.
        if let Some(v) = height_after(&upper, &["TOP", "TOD", "T.O."]) {
            return Some(v - section_ft);
        }
        // Generic elevation.
        if let Some(v) = height_after(&upper, &["EL", "AFF", "A.F.F", "@"]) {
            // Skip if this is a slope note.
            if upper.contains("/FT") || upper.contains("PER FT") || upper.contains('%') {
                continue;
            }
            return Some(v);
        }
    }
    None
}

/// Extract feet-inches after the first occurrence of one of the keywords.
/// When multiple keys match at the same position, the longest match wins.
fn height_after(s: &str, keys: &[&str]) -> Option<f64> {
    let mut best: Option<(usize, usize)> = None; // (start, end)
    for k in keys {
        if let Some(start) = s.find(k) {
            let end = start + k.len();
            match best {
                None => best = Some((start, end)),
                Some((bs, be)) => {
                    if start < bs || (start == bs && end > be) {
                        best = Some((start, end));
                    }
                }
            }
        }
    }
    best.and_then(|(_, end)| s.get(end..)).and_then(parse_feet_inches).map(|v| v / 12.0)
}

/// Parse feet-inches text like `10'-6"`, `9'-0"`, `10'`, returning total inches.
fn parse_feet_inches(s: &str) -> Option<f64> {
    let s = s.trim().trim_start_matches(['@', ' ', ':', '.']);
    // Negative?
    let (neg, s) = if let Some(rest) = s.strip_prefix('-') { (true, rest.trim()) } else { (false, s) };
    // feet'inches"  or  feet'-inches"
    let sign = if neg { -1.0 } else { 1.0 };
    if let Some(ft_pos) = s.find('\'') {
        let ft: f64 = s[..ft_pos].trim().parse().ok()?;
        let rest = s[ft_pos + 1..].trim().trim_start_matches('-');
        let inches = if rest.is_empty() || !rest.contains('"') {
            0.0
        } else {
            // Strip trailing non-numeric text (like " AFF"), the inch mark, and leading/trailing spaces.
            let inch_str = rest
                .trim_end_matches(|c: char| c != '"')
                .trim_end_matches('"')
                .trim();
            parse_inch_value(inch_str)?
        };
        Some(sign * (ft * 12.0 + inches))
    } else {
        None
    }
}

fn parse_inch_value(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return Some(0.0);
    }
    // `7 1/2`
    if let Some((whole, frac)) = s.split_once(' ')
        && let (Ok(w), Some(f)) = (whole.trim().parse::<f64>(), parse_fraction(frac))
    {
        return Some(w + f);
    }
    // `1/2`
    if let Some(f) = parse_fraction(s) {
        return Some(f);
    }
    s.parse::<f64>().ok()
}

fn parse_fraction(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let n: f64 = n.trim().parse().ok()?;
    let d: f64 = d.trim().parse().ok().filter(|d| *d != 0.0)?;
    Some(n / d)
}

/// Parse slope notes: `@ 1/4"/FT`, `@ 1/4" PER FT`, `2%`.
fn parse_slope_from_texts(texts: &[&TextItem]) -> Option<String> {
    for t in texts {
        let upper = t.text.to_uppercase();
        if upper.contains("/FT") || upper.contains("PER FT") || upper.contains('%') {
            return Some(t.text.clone());
        }
    }
    None
}

/// Abbreviation → canonical system code. The style varies abbreviations per seed;
/// this table covers all generated variants.
const SYS_ABBR: &[(&str, &str)] = &[
    ("CW", "CW"),
    ("HW", "HW"),
    ("HWR", "HWR"),
    ("HWC", "HWR"),
    ("HRW", "HWR"),
    ("SAN", "SAN"),
    ("SS", "SAN"),
    ("V", "V"),
    ("G", "G"),
    ("GAS", "G"),
    ("NG", "G"),
    ("FP", "FP"),
    ("SP", "FP"),
    ("F", "FP"),
    ("CHW", "CHW"),
    ("CHWR", "CHWR"),
    ("ST", "ST"),
];

/// Identify a piping system from a riser note or other text, handling varied abbreviations.
fn identify_system_from_note(upper: &str) -> String {
    // Try to find a system abbreviation as a word boundary in the note.
    // Check longer abbreviations first to avoid false matches (e.g. "HWR" before "HW").
    let mut candidates: Vec<(&str, &str)> = SYS_ABBR.to_vec();
    candidates.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
    for (abbr, code) in &candidates {
        // Match as a whole word: surrounded by non-alphanumeric or at start/end.
        let a = abbr.to_uppercase();
        for (i, _) in upper.match_indices(&a) {
            let before_ok = i == 0 || !upper.as_bytes().get(i - 1).is_some_and(|b| b.is_ascii_alphanumeric());
            let after = i + a.len();
            let after_ok = after >= upper.len() || !upper.as_bytes().get(after).is_some_and(|b| b.is_ascii_alphanumeric());
            if before_ok && after_ok {
                return code.to_string();
            }
        }
    }
    String::new()
}

/// Default pipe size when no tag is found, inches.
fn default_size(system: &str) -> f64 {
    match system {
        "CW" => 1.0,
        "HW" | "HWR" => 0.75,
        "SAN" => 3.0,
        "V" => 3.0,
        "G" => 1.0,
        "FP" => 2.0,
        _ => 1.0,
    }
}

/// Detect riser symbols: circles on PIPE layer, with annotation text nearby.
fn detect_risers(circles: &[(Vec2, f64, String)], texts: &[TextItem], sh: &Sheet) -> Vec<DetectedRiser> {
    if sh.level.is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();

    // Riser circles are annotation-scale dependent. The generator draws them at
    // `pen.pl(0.07)` = `0.07 / paper_from_model.scale` model inches. Small drop circles
    // are at `pen.pl(0.035)`. We accept a generous range.
    let pms = sh.paper_from_model.scale;
    let k = if pms > 1e-9 { 1.0 / pms } else { 1.0 };
    let min_r = 0.02 * k; // smaller than drop circles
    let max_r = 0.15 * k; // larger than riser circles

    for (center, radius, _layer) in circles {
        if *radius < min_r || *radius > max_r {
            continue;
        }

        // Look for a riser note nearby: `3/4" CW UP`, `4" SAN DN`, `2" FP UP/DN`.
        let note_reach = 8.0 * k; // generous reach for leader-connected notes (8 paper inches)
        let mut best_note: Option<&TextItem> = None;
        let mut best_dist = f64::INFINITY;

        for t in texts {
            let upper = t.text.to_uppercase();
            let has_direction = upper.contains(" UP") || upper.contains(" DN") || upper.contains(" DOWN") || upper.contains("THRU");
            if !has_direction {
                continue;
            }
            let d = t.at.dist(*center);
            if d < note_reach && d < best_dist {
                best_dist = d;
                best_note = Some(t);
            }
        }

        let Some(note) = best_note else { continue };
        let upper = note.text.to_uppercase();

        // Parse system from the note, handling style-varied abbreviations.
        let system = identify_system_from_note(&upper);

        // Parse direction.
        let direction = if upper.contains("UP/DN") || upper.contains("UP/DOWN") || upper.contains("THRU") {
            "through"
        } else if upper.contains(" UP") {
            "up"
        } else {
            "down"
        };

        // Parse size from the note: `3/4" CW UP` → 0.75.
        let size = parse_riser_size(&upper);

        let at_ft = building_from_model(sh, [center.x, center.y]);

        out.push(DetectedRiser {
            at_model: *center,
            at_ft,
            system,
            size_in: size,
            direction: direction.to_string(),
            note: note.text.clone(),
        });
    }

    out
}

/// Parse a pipe size from a riser note: `3/4" CW UP` → 0.75.
fn parse_riser_size(s: &str) -> f64 {
    let words: Vec<&str> = s.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if !w.contains('"') {
            continue;
        }
        let digits: String = w.chars().take_while(|c| c.is_ascii_digit() || *c == '/' || *c == '.').collect();
        if let Some(v) = parse_pipe_num(&digits) {
            // Check for whole part: `1 1/2"`
            if i > 0 && digits.contains('/')
                && let Ok(whole) = words[i - 1].parse::<f64>()
            {
                return whole + v;
            }
            return v;
        }
    }
    1.0 // default
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_feet_inches_values() {
        assert_eq!(parse_feet_inches("10'-6\""), Some(126.0));
        assert_eq!(parse_feet_inches("9'-0\""), Some(108.0));
        assert_eq!(parse_feet_inches("8'-7 1/2\""), Some(103.5));
        assert_eq!(parse_feet_inches(" @ 10'-0\""), Some(120.0));
        assert_eq!(parse_feet_inches("-2'-0\""), Some(-24.0));
    }

    #[test]
    fn parse_pipe_sizes() {
        assert_eq!(parse_pipe_num("2"), Some(2.0));
        assert_eq!(parse_pipe_num("3/4"), Some(0.75));
        assert_eq!(parse_pipe_num("1"), Some(1.0));
    }

    #[test]
    fn parse_riser_sizes() {
        assert!((parse_riser_size("3/4\" CW UP") - 0.75).abs() < 1e-6);
        assert!((parse_riser_size("4\" SAN DN") - 4.0).abs() < 1e-6);
        assert!((parse_riser_size("2\" FP UP/DN") - 2.0).abs() < 1e-6);
    }

    #[test]
    fn system_from_text() {
        let mk = |s: &str| TextItem { text: s.to_string(), at: Vec2::ZERO };
        let cw = mk("CW");
        let hw = mk("HW");
        assert_eq!(identify_system(&[&cw], "PIPE"), "CW");
        assert_eq!(identify_system(&[&hw], "PIPE"), "HW");
        assert_eq!(identify_system(&[], "DASHED"), "V");
    }

    #[test]
    fn size_and_system_from_tag() {
        let mk = |s: &str| TextItem { text: s.to_string(), at: Vec2::ZERO };
        let t1 = mk("2\" CW");
        let (sz, sys) = parse_size_from_texts(&[&t1]);
        assert_eq!(sz, Some(2.0));
        assert_eq!(sys, Some("CW".to_string()));

        let t2 = mk("3/4\" HW");
        let (sz, sys) = parse_size_from_texts(&[&t2]);
        assert_eq!(sz, Some(0.75));
        assert_eq!(sys, Some("HW".to_string()));
    }

    #[test]
    fn height_parsing() {
        let mk = |s: &str| TextItem { text: s.to_string(), at: Vec2::ZERO };
        let t1 = mk("BOP @ 10'-0\"");
        assert_eq!(parse_height_from_texts(&[&t1], 0.25), Some(10.0));

        let t2 = mk("CL @ 10'-6\"");
        assert_eq!(parse_height_from_texts(&[&t2], 0.5), Some(10.5 - 0.25));

        let t3 = mk("B.O.P. 9'-6\" AFF");
        assert!((parse_height_from_texts(&[&t3], 0.25).unwrap_or(0.0) - 9.5).abs() < 1e-6);
    }

    #[test]
    fn slope_detection() {
        let mk = |s: &str| TextItem { text: s.to_string(), at: Vec2::ZERO };
        let t1 = mk("@ 1/4\"/FT");
        assert!(parse_slope_from_texts(&[&t1]).is_some());

        let t2 = mk("@ 1/4\" PER FT");
        assert!(parse_slope_from_texts(&[&t2]).is_some());

        let t3 = mk("2%");
        assert!(parse_slope_from_texts(&[&t3]).is_some());

        let t4 = mk("BOP @ 10'-0\"");
        assert!(parse_slope_from_texts(&[&t4]).is_none());
    }
}
