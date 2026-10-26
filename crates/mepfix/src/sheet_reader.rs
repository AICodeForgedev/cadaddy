//! Sheet recognition and alignment: reading DXF sheets, identifying each sheet's discipline,
//! number, level and scale from its title block, detecting the structural grid, and computing
//! the building-coordinate transform so later pipeline steps work in a common frame.
//!
//! The reader is text-driven: it uses layer names (`TITLE`, `GRID`, `GRID-BUBBLE`, `TEXT`) and
//! the textual content of the entities, not their absolute positions or sizes.

use crate::layout;
use crate::truth::{GridLine, Sheet};
use cadaddy_doc::{Drawing, EntityKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- Data structures ----------------------------------------------------------------

/// A recognised sheet, carrying everything later pipeline steps need.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RecognisedSheet {
    /// The sheet number as read from the title block (`M-101`).
    pub number: String,
    /// The sheet title (`LEVEL 1 MECHANICAL PLAN`).
    pub title: String,
    /// Discipline code: `G`, `A`, `S`, `M`, `P`, `FP`.
    pub discipline: String,
    /// Level code (`1`, `2`, `R`) for plan sheets; `None` for covers and schedule sheets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// The scale text as printed (`1/8" = 1'-0"`), if found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_text: Option<String>,
    /// Paper inches per building foot.
    pub scale: f64,
    /// Detected grid lines of constant x (running north–south on the plan), in model inches.
    pub grid_x: Vec<DetectedGrid>,
    /// Detected grid lines of constant y, in model inches.
    pub grid_y: Vec<DetectedGrid>,
    /// The computed model-inches → building-feet transform, if alignment succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<Alignment>,
    /// Raw grid bubble data: (label, [model_x, model_y]) for each detected bubble.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bubbles: Vec<(String, [f64; 2])>,
    /// Every level the sheet (or this view of it) shows, `level` first: a typical-floor plan
    /// titled `THIRD & FOURTH FLOOR` shows two.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<String>,
    /// For one plan view of a sheet that prints several ([`recognise_views`]): the sheet number
    /// as printed. `number` is then `<printed number>/<level>`, so each view is its own sheet.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub printed_number: String,
}

/// A grid line detected in a DXF drawing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectedGrid {
    pub label: String,
    /// Position in model inches (x for x-lines, y for y-lines).
    pub at_in: f64,
}

/// The computed transform from model inches to building feet, derived from grid alignment.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Alignment {
    pub rotation_deg: f64,
    pub offset_in: [f64; 2],
}

/// The sheet map: the full recognition result for a set of sheets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetMap {
    pub sheets: Vec<RecognisedSheet>,
    /// The common grid, assembled from all sheets: labels and building-feet positions.
    pub grid_x: Vec<GridLine>,
    pub grid_y: Vec<GridLine>,
}

// ---- Title block parsing ------------------------------------------------------------

/// Known discipline prefixes: sheet number prefix → discipline code.
const DISCIPLINE_PREFIX: &[(&str, &str)] =
    &[("FP", "FP"), ("FA", "FA"), ("M", "M"), ("P", "P"), ("S", "S"), ("A", "A"), ("G", "G"), ("E", "E"), ("C", "C"), ("T", "T")];

/// The discipline a sheet number's letters name: `M-101`, `M101`, `M1.01` and `FP-201` alike.
fn discipline_of(number: &str) -> Option<&'static str> {
    let up = number.trim().to_ascii_uppercase();
    let letters: String = up.chars().take_while(char::is_ascii_alphabetic).collect();
    let next = up.chars().nth(letters.len())?;
    if !(next == '-' || next == '.' || next.is_ascii_digit()) {
        return None;
    }
    DISCIPLINE_PREFIX.iter().find(|(p, _)| *p == letters).map(|(_, d)| *d)
}

/// Whether a text reads as a sheet number: one or two letters, an optional `-` or `.`, then a
/// number with an optional decimal part and letter (`A-101`, `A101`, `C2.0`, `E-4`, `M-101A`).
fn looks_like_sheet_number(v: &str) -> bool {
    let v = v.trim();
    let b = v.as_bytes();
    let letters = b.iter().take_while(|c| c.is_ascii_uppercase()).count();
    if !(1..=2).contains(&letters) || v.len() > 8 {
        return false;
    }
    let mut rest = b.get(letters..).unwrap_or_default();
    if let Some((&c, r)) = rest.split_first()
        && (c == b'-' || c == b'.')
    {
        rest = r;
    }
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 4 {
        return false;
    }
    let tail = rest.get(digits..).unwrap_or_default();
    match tail {
        [] => true,
        [c] => c.is_ascii_uppercase(),
        [b'.', more @ ..] => (1..=2).contains(&more.len()) && more.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

/// Parse a scale string like `1/8" = 1'-0"` → paper inches per building foot.
pub fn parse_scale(s: &str) -> Option<f64> {
    let s = s.trim();
    for &(text, val) in layout::ENLARGED_SCALES.iter().chain(crate::style::SCALES.iter()) {
        if s == text {
            return Some(val);
        }
    }
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() == 2 {
        let left = parts[0].trim().trim_end_matches('"').trim();
        if let Some(val) = parse_fraction(left) {
            // `1" = 20'-0"`: paper inches per however many feet the right side says.
            let feet = parse_feet(parts[1]).filter(|f| *f > 1e-9).unwrap_or(1.0);
            return Some(val / feet).filter(|v| v.is_finite() && *v > 0.0);
        }
    }
    None
}

/// The right side of a scale note in feet: `1'-0"`, `20'`, `20'-0"`, `6"`.
fn parse_feet(s: &str) -> Option<f64> {
    let s = s.trim();
    let (ft, rest) = match s.split_once('\'') {
        Some((f, r)) => (f.trim().parse::<f64>().ok()?, r.trim().trim_start_matches('-').trim()),
        None => (0.0, s),
    };
    let inches = rest.trim_end_matches('"').trim();
    let inches = if inches.is_empty() { 0.0 } else { parse_fraction(inches)? };
    Some(ft + inches / 12.0)
}

fn parse_fraction(s: &str) -> Option<f64> {
    if let Some((n, d)) = s.split_once('/') {
        let n: f64 = n.trim().parse().ok()?;
        let d: f64 = d.trim().parse().ok()?;
        if d.abs() > 1e-12 {
            return Some(n / d);
        }
    }
    s.trim().parse::<f64>().ok()
}

fn detect_sheet_number(d: &Drawing) -> Option<String> {
    let mut best: Option<(f64, String)> = None;
    for e in d.model.iter() {
        let ly = &e.common.layer;
        // Original DXF layers (TITLE, BORDER) and content-class layer TITLEBLOCK from PDF
        // import. Not TEXT — that layer holds ALL text, so picking the largest there gives the
        // title instead of the number.
        let on_title = ly.eq_ignore_ascii_case("TITLE") || ly.eq_ignore_ascii_case("BORDER") || ly.eq_ignore_ascii_case("TITLEBLOCK");
        if !on_title {
            continue;
        }
        let (h, val) = match &e.kind {
            EntityKind::Text(t) => (t.height, t.value.clone()),
            EntityKind::MText(t) => (t.height, t.contents.clone()),
            _ => continue,
        };
        // Validate: SHX OCR can put garbled single-letter fragments on the TITLE layer;
        // reject text that doesn't look like a sheet number to avoid picking up OCR noise.
        if h > best.as_ref().map_or(0.0, |b| b.0) && looks_like_sheet_number(&val) {
            best = Some((h, val));
        }
    }
    best.map(|(_, v)| v).or_else(|| sheet_number_in_title_block(d))
}

/// The texts of a drawing with their heights and where they sit across the sheet's extents
/// (0 = left or bottom edge, 1 = right or top), for drawings whose layers say nothing (a PDF's
/// print, where everything comes in on one or a few pen layers).
fn texts_across(d: &Drawing) -> Vec<(String, f64, [f64; 2], [f64; 2])> {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut grow = |p: [f64; 2]| {
        if p[0].is_finite() && p[1].is_finite() {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
    };
    let mut texts = Vec::new();
    for e in d.model.iter() {
        match &e.kind {
            EntityKind::Line(l) => {
                grow([l.a.x, l.a.y]);
                grow([l.b.x, l.b.y]);
            }
            EntityKind::LwPolyline(pl) => pl.vertices.iter().for_each(|v| grow([v.p.x, v.p.y])),
            EntityKind::Text(t) => {
                grow([t.insert.x, t.insert.y]);
                texts.push((t.value.trim().to_string(), t.height, [t.insert.x, t.insert.y]));
            }
            EntityKind::MText(t) => {
                grow([t.insert.x, t.insert.y]);
                texts.push((t.contents.trim().to_string(), t.height, [t.insert.x, t.insert.y]));
            }
            _ => {}
        }
    }
    let size = [(hi[0] - lo[0]).max(1e-9), (hi[1] - lo[1]).max(1e-9)];
    texts.into_iter().map(|(v, h, p)| (v, h, [(p[0] - lo[0]) / size[0], (p[1] - lo[1]) / size[1]], p)).collect()
}

/// In the title block: along the right edge or the bottom of the sheet.
fn in_title_block(at: [f64; 2]) -> bool {
    at[0] > 0.8 || at[1] < 0.15
}

/// The sheet number without a `TITLE` layer: the largest text in the title block that reads as
/// one, the one nearest the bottom right corner when two are as large.
fn sheet_number_in_title_block(d: &Drawing) -> Option<String> {
    texts_across(d)
        .into_iter()
        .filter(|(v, _, at, _)| in_title_block(*at) && looks_like_sheet_number(v))
        .max_by(|a, b| a.1.total_cmp(&b.1).then((a.2[0] - a.2[1]).total_cmp(&(b.2[0] - b.2[1]))))
        .map(|(v, _, _, _)| v)
}

fn detect_scale_text(d: &Drawing) -> Option<(String, f64)> {
    for e in d.model.iter() {
        let on_title = e.common.layer.eq_ignore_ascii_case("TITLE") || e.common.layer.eq_ignore_ascii_case("TEXT");
        if !on_title {
            continue;
        }
        let val = match &e.kind {
            EntityKind::Text(t) => &t.value,
            EntityKind::MText(t) => &t.contents,
            _ => continue,
        };
        if let Some(rest) = val.strip_prefix("SCALE:").or_else(|| val.strip_prefix("SCALE :")) {
            let rest = rest.trim();
            if let Some(scale) = parse_scale(rest) {
                return Some((rest.to_string(), scale));
            }
        }
    }
    scale_note_anywhere(d)
}

/// The scale without `TITLE`/`TEXT` layers: the most common scale note on the sheet, with or
/// without a `SCALE:` label (a sheet of several views has one under each; the plan's repeats).
fn scale_note_anywhere(d: &Drawing) -> Option<(String, f64)> {
    let mut seen: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    for e in d.model.iter() {
        let val = match &e.kind {
            EntityKind::Text(t) => t.value.as_str(),
            EntityKind::MText(t) => t.contents.as_str(),
            _ => continue,
        };
        let up = val.trim().to_ascii_uppercase();
        let note = up.strip_prefix("SCALE").map_or(up.as_str(), |r| r.trim_start_matches([':', ' '])).trim();
        if !note.contains('=') || !(note.contains('\'') || note.contains('"')) || note.len() > 24 {
            continue;
        }
        if let Some(k) = parse_scale(note) {
            seen.entry(note.to_string()).or_insert((k, 0)).1 += 1;
        }
    }
    seen.into_iter().max_by(|a, b| a.1.1.cmp(&b.1.1).then(b.0.cmp(&a.0))).map(|(t, (k, _))| (t, k))
}

fn detect_discipline(number: &str, texts: &[(String, [f64; 2])]) -> String {
    if let Some(disc) = discipline_of(number) {
        return disc.to_string();
    }
    let keywords =
        [("MECHANICAL", "M"), ("PLUMBING", "P"), ("FIRE PROTECTION", "FP"), ("STRUCTURAL", "S"), ("ARCHITECTURAL", "A"), ("REFLECTED CEILING", "A")];
    for (text, _pos) in texts {
        let upper = text.to_uppercase();
        for &(kw, disc) in &keywords {
            if upper.contains(kw) {
                return disc.to_string();
            }
        }
    }
    "G".to_string()
}

/// `FIRST`, `1ST`, `2ND` … as a level number.
fn ordinal(word: &str) -> Option<u32> {
    const WORDS: [&str; 10] = ["FIRST", "SECOND", "THIRD", "FOURTH", "FIFTH", "SIXTH", "SEVENTH", "EIGHTH", "NINTH", "TENTH"];
    if let Some(i) = WORDS.iter().position(|w| *w == word) {
        return u32::try_from(i + 1).ok();
    }
    let digits: String = word.chars().take_while(char::is_ascii_digit).collect();
    let suffix = word.get(digits.len()..)?;
    if matches!(suffix, "ST" | "ND" | "RD" | "TH") { digits.parse().ok().filter(|n| *n > 0 && *n < 200) } else { None }
}

fn detect_level(title: &str) -> Option<String> {
    detect_levels(title).into_iter().next()
}

/// A level code after `LEVEL`: `2`, `02` (level 2), `B1`, `R`; not a word (`LEVEL MARKS ARE…`).
fn level_code(word: &str) -> Option<String> {
    let w = word.trim_end_matches([',', ':', '-']);
    if w.is_empty() || w.len() > 3 || !w.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if w.chars().all(|c| c.is_ascii_digit()) {
        let n = w.trim_start_matches('0');
        return (!n.is_empty()).then(|| n.to_string());
    }
    (w.chars().any(|c| c.is_ascii_digit()) || w.len() == 1).then(|| w.to_string())
}

/// A title's words, upper case, with `&` read as `AND` and a dash kept as `-` (`2ND-4TH`).
fn level_tokens(title: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    for c in title.chars().take(400) {
        if c.is_ascii_alphanumeric() {
            word.push(c.to_ascii_uppercase());
            continue;
        }
        if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
        match c {
            '&' => out.push("AND".into()),
            '-' | '\u{2013}' | '\u{2014}' => out.push("-".into()),
            _ => {}
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

fn is_range(t: &str) -> bool {
    matches!(t, "-" | "THRU" | "THROUGH" | "TO")
}

/// Levels `lo` up to `hi`, when a range of floors is a plausible one.
fn level_range(lo: u32, hi: u32) -> Vec<u32> {
    if hi > lo && hi - lo <= 60 { (lo..=hi).collect() } else { vec![lo, hi] }
}

/// Every level a title names, lowest first, roof last: `SECOND FLOOR PLAN`, `2ND FLOOR`,
/// `FLOOR 2`, `LEVEL 02`, and the lists and ranges of a typical-floor or two-view sheet
/// (`FIRST AND SECOND FLOOR`, `THIRD & FOURTH FLOOR`, `1ST AND 3RD FLOORS`, `2ND-4TH FLOOR`,
/// `SECOND THRU FOURTH FLOOR`, `LEVELS 2-4`).
pub fn detect_levels(title: &str) -> Vec<String> {
    let words = level_tokens(title);
    let mut found: Vec<String> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        match w.as_str() {
            "FLOOR" | "FLOORS" => {
                // Ordinals before it, joined by AND, `&` or a range: `FIRST AND SECOND FLOOR`.
                let mut before: Vec<u32> = Vec::new();
                let mut range = false;
                for p in words.get(..i).unwrap_or_default().iter().rev() {
                    if let Some(n) = ordinal(p) {
                        match before.last() {
                            Some(&hi) if range => before.extend(level_range(n, hi).into_iter().rev().skip(1)),
                            _ => before.push(n),
                        }
                        range = false;
                    } else if is_range(p) {
                        range = true;
                    } else if p != "AND" {
                        break;
                    }
                }
                if before.is_empty() {
                    // `FLOOR 2`, `FLOORS 3 AND 4`, `FLOORS 2-4`.
                    before = numbers_after(words.get(i + 1..).unwrap_or_default());
                }
                found.extend(before.iter().map(u32::to_string));
            }
            "LEVEL" | "LEVELS" => {
                let rest = words.get(i + 1..).unwrap_or_default();
                let numbers = numbers_after(rest);
                if numbers.is_empty() {
                    // `LEVEL B1`, `LEVEL R`.
                    found.extend(rest.first().and_then(|n| level_code(n)));
                } else {
                    found.extend(numbers.iter().map(u32::to_string));
                }
            }
            _ => {}
        }
    }
    // A roof plan (framing, drainage…), not a sheet of roof assemblies or details.
    if words.iter().any(|w| w == "ROOF") && words.iter().any(|w| w == "PLAN" || w == "PLANS") {
        found.push("R".to_string());
    }
    let key = |c: &String| (c == "R", c.parse::<u32>().unwrap_or(u32::MAX), c.clone());
    found.sort_by_key(key);
    found.dedup();
    found
}

/// Level numbers at the start of `words`: `2`, `02`, `3 AND 4`, `2-4`.
fn numbers_after(words: &[String]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let mut range = false;
    for w in words.iter().take(9) {
        let n = level_code(w).and_then(|c| c.parse::<u32>().ok()).filter(|n| *n > 0 && *n < 200);
        match n {
            Some(n) => {
                match out.last() {
                    Some(&lo) if range => out.extend(level_range(lo, n).into_iter().skip(1)),
                    _ => out.push(n),
                }
                range = false;
            }
            None if is_range(w) => range = true,
            None if w == "AND" => {}
            None => break,
        }
    }
    out
}

fn detect_title(d: &Drawing, number: &str) -> String {
    let mut candidates: Vec<(f64, String)> = Vec::new();
    let has_title_layer = d.model.iter().any(|e| e.common.layer.eq_ignore_ascii_case("TITLE"));
    for e in d.model.iter() {
        let ly = &e.common.layer;
        // TITLE layer (DXF), or when TITLE has text (OCR may garble it), also accept
        // PDF-HIDDEN-TEXT as an authoritative source.  Without TITLE text, let the
        // fallback (texts_across) handle it — hidden text heights are unreliable.
        let ok = ly.eq_ignore_ascii_case("TITLE") || (has_title_layer && ly.eq_ignore_ascii_case("PDF-HIDDEN-TEXT"));
        if !ok {
            continue;
        }
        let (h, val) = match &e.kind {
            EntityKind::Text(t) => (t.height, t.value.clone()),
            EntityKind::MText(t) => (t.height, t.contents.clone()),
            _ => continue,
        };
        if val == number || val.is_empty() {
            continue;
        }
        candidates.push((h, val));
    }
    if candidates.is_empty() {
        // No TITLE layer: the title block's texts, else the views' titles across the sheet.
        let texts: Vec<(String, f64, [f64; 2], [f64; 2])> = texts_across(d).into_iter().filter(|(v, _, _, _)| v != number && !v.is_empty()).collect();
        let titled = |v: &str| {
            let up = v.to_uppercase();
            ["PLAN", "SCHEDULE", "COVER", "FRAMING", "CEILING", "ASSEMBLIES", "DETAILS", "SECTIONS", "ELEVATIONS", "DIAGRAM", "LAYOUT"]
                .iter()
                .any(|k| up.contains(k))
        };
        // The largest titled lines, the title block's first; a heading over a block of notes
        // (`SHEET NOTES - FLOOR PLAN` at the top of the title column) isn't the title.
        let mut ranked: Vec<&(String, f64, [f64; 2], [f64; 2])> = texts.iter().filter(|(v, _, _, _)| titled(v)).collect();
        // The title column at the right edge first, then the foot strip, then the views.
        let rank = |at: [f64; 2]| (in_title_block(at), at[0] > 0.8);
        ranked.sort_by(|a, b| rank(b.2).cmp(&rank(a.2)).then(b.1.total_cmp(&a.1)));
        let not_notes = |t: &String| {
            let up = t.to_uppercase();
            !["NOTES", "LEGEND", "ABBREVIATIONS"].iter().any(|k| up.contains(k))
        };
        let mut first_title = None;
        for c in ranked.into_iter().take(40) {
            let t = title_lines(&texts, c);
            if not_notes(&t) {
                return t;
            }
            first_title.get_or_insert(t);
        }
        return first_title.unwrap_or_default();
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, val) in &candidates {
        let upper = val.to_uppercase();
        if upper.contains("PLAN") || upper.contains("SCHEDULE") || upper.contains("COVER") || upper.contains("FRAMING") || upper.contains("CEILING")
            // Assembly, detail and wall section sheets (Q337).
            || upper.contains("ASSEMBLIES")
            || upper.contains("DETAILS")
            || upper.contains("SECTIONS")
            || upper.contains("LAYOUT")
        {
            return val.clone();
        }
    }
    candidates.first().map_or_else(String::new, |(_, v)| v.clone())
}

/// A title printed over several lines (`SECOND FLOOR` above `POWER PLAN`): the lines of the
/// same lettering stacked with `first`, top to bottom, joined.
fn title_lines(texts: &[(String, f64, [f64; 2], [f64; 2])], first: &(String, f64, [f64; 2], [f64; 2])) -> String {
    let h = first.1.max(1e-9);
    // A title's lines are short; a note block's are long.
    let alike = |t: &&(String, f64, [f64; 2], [f64; 2])| (t.1 - h).abs() <= h * 0.25 && (t.3[0] - first.3[0]).abs() <= h * 12.0 && t.0.len() <= 40;
    let mut lines: Vec<&(String, f64, [f64; 2], [f64; 2])> = vec![first];
    // Grow the stack a line at a time, up and down (at most a few lines).
    for _ in 0..4 {
        let (top, bottom) = lines.iter().fold((f64::NEG_INFINITY, f64::INFINITY), |(t, b), l| (t.max(l.3[1]), b.min(l.3[1])));
        let next = texts.iter().filter(alike).find(|t| {
            let y = t.3[1];
            !lines.iter().any(|l| std::ptr::eq(*l, *t)) && ((y > top && y - top <= h * 2.0) || (y < bottom && bottom - y <= h * 2.0))
        });
        match next {
            Some(t) => lines.push(t),
            None => break,
        }
    }
    lines.sort_by(|a, b| b.3[1].total_cmp(&a.3[1]));
    lines.iter().map(|l| l.0.as_str()).collect::<Vec<_>>().join(" ")
}

// ---- Grid detection -----------------------------------------------------------------

/// Detect grid bubbles: circles on GRID-BUBBLE layer, with text labels inside.
fn detect_grid_bubbles(d: &Drawing) -> Vec<(String, [f64; 2])> {
    if !d.model.iter().any(|e| e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE")) {
        return bubbles_without_layers(d);
    }
    let mut circles: Vec<(f64, f64, f64)> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE") {
            continue;
        }
        if let Some((c, r)) = round_shape(&e.kind) {
            circles.push((c[0], c[1], r));
        }
    }
    let mut texts: Vec<(String, [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE") {
            continue;
        }
        match &e.kind {
            EntityKind::Text(t) => {
                let pt = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
                texts.push((t.value.clone(), pt));
            }
            EntityKind::MText(t) => {
                texts.push((t.contents.clone(), [t.insert.x, t.insert.y]));
            }
            _ => {}
        }
    }
    let mut bubbles: Vec<(String, [f64; 2])> = Vec::new();
    for (label, pt) in &texts {
        if label.trim().is_empty() {
            continue;
        }
        let mut best_dist = f64::INFINITY;
        let mut best_c: Option<[f64; 2]> = None;
        for &(cx, cy, r) in &circles {
            let dist = ((pt[0] - cx).powi(2) + (pt[1] - cy).powi(2)).sqrt();
            if dist < r * 1.5 && dist < best_dist {
                best_dist = dist;
                best_c = Some([cx, cy]);
            }
        }
        if let Some(centre) = best_c {
            bubbles.push((label.clone(), centre));
        }
    }
    // A print keeps the layer but puts the labels on its text layer.
    if bubbles.is_empty() { bubbles_without_layers(d) } else { bubbles }
}

/// Most round shapes and labels looked at on a sheet without layers (hostile input guard).
const MAX_ROUNDS: usize = 20_000;

/// A circle, or a closed polyline that is one (a PDF's print draws circles as many short
/// segments or as two arcs): its centre and radius.
fn round_shape(kind: &EntityKind) -> Option<([f64; 2], f64)> {
    match kind {
        EntityKind::Circle(c) => Some(([c.center.x, c.center.y], c.radius)),
        EntityKind::LwPolyline(pl) => {
            let n = pl.vertices.len();
            let arcs = pl.vertices.iter().filter(|v| v.bulge.abs() > 0.3).count();
            if !(2..=256).contains(&n) || (n < 8 && arcs < 2) {
                return None;
            }
            let pts: Vec<[f64; 2]> = pl.vertices.iter().map(|v| [v.p.x, v.p.y]).collect();
            let c = [pts.iter().map(|p| p[0]).sum::<f64>() / n as f64, pts.iter().map(|p| p[1]).sum::<f64>() / n as f64];
            let radii: Vec<f64> = pts.iter().map(|p| (p[0] - c[0]).hypot(p[1] - c[1])).collect();
            let r = radii.iter().sum::<f64>() / n as f64;
            let (first, last) = (pts.first()?, pts.last()?);
            let closed = pl.closed || (first[0] - last[0]).hypot(first[1] - last[1]) < r * 0.05;
            (closed && r > 1e-9 && r.is_finite() && radii.iter().all(|q| (q - r).abs() < r * 0.08)).then_some((c, r))
        }
        _ => None,
    }
}

/// Grid bubbles on a sheet whose layers say nothing: short labels (`1`, `12`, `A`, `C.5`) inside
/// round shapes, kept when their label is in two bubbles in line (the two ends of a grid line)
/// and their radius is the bubbles' usual one (not a keynote's or a tag's).
fn bubbles_without_layers(d: &Drawing) -> Vec<(String, [f64; 2])> {
    let rounds: Vec<([f64; 2], f64)> = d.model.iter().filter_map(|e| round_shape(&e.kind)).take(MAX_ROUNDS).collect();
    if rounds.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<(String, [f64; 2], f64)> = Vec::new();
    for e in d.model.iter() {
        let (label, at) = match &e.kind {
            EntityKind::Text(t) => {
                let v = t.value.trim();
                // A PDF's text sits on its baseline at its left end: its middle is up and right.
                let mid = t.align_pt.map_or([t.insert.x + t.height * 0.35 * v.len() as f64, t.insert.y + t.height * 0.5], |p| [p.x, p.y]);
                (v.to_string(), mid)
            }
            EntityKind::MText(t) => (t.contents.trim().to_string(), [t.insert.x, t.insert.y]),
            _ => continue,
        };
        let short = (1..=3).contains(&label.len())
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
            && label.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
        if !short {
            continue;
        }
        let inside = rounds.iter().filter(|(c, r)| (at[0] - c[0]).hypot(at[1] - c[1]) < r * 1.2).min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((c, r)) = inside {
            found.push((label, *c, *r));
        }
        if found.len() >= MAX_ROUNDS {
            break;
        }
    }
    // Labels in two bubbles in line, then the radius most of those share.
    // One bubble per shape (a label drawn twice over itself counts once).
    let mut by_label: BTreeMap<&str, Vec<([f64; 2], f64)>> = BTreeMap::new();
    for (l, c, r) in &found {
        let v = by_label.entry(l.as_str()).or_default();
        if !v.iter().any(|(p, q)| (p[0] - c[0]).hypot(p[1] - c[1]) < q * 0.5) {
            v.push((*c, *r));
        }
    }
    let mut paired: Vec<f64> = Vec::new();
    for v in by_label.values() {
        if let [(a, ra), (b, _), ..] = v.as_slice() {
            let (dx, dy) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
            if dx < dy * 0.3 || dy < dx * 0.3 {
                paired.push(*ra);
            }
        }
    }
    if paired.len() < 3 {
        // One bubble per line: the radius most labelled shapes share (their lines decide).
        paired = by_label.values().flatten().map(|(_, r)| *r).collect();
    }
    paired.sort_by(f64::total_cmp);
    let Some(&usual) = paired.get(paired.len() / 2) else { return Vec::new() };
    by_label
        .into_iter()
        .flat_map(|(l, v)| v.into_iter().filter(|(_, r)| (r - usual).abs() <= usual * 0.15).map(move |(c, _)| (l.to_string(), c)))
        .collect()
}

/// Most line pieces looked at for grid leaders (hostile input guard).
const MAX_LEADER_SEGS: usize = 2_000_000;

/// Grid lines from the line leaving each bubble (a print draws a dashed grid line as its
/// dashes, so the first dash is all there is next to the bubble): a bubble whose line leaves
/// it straight up or down is on a line of constant x, left or right one of constant y. A label
/// is classified by most of its bubbles and placed at the middle one.
fn grid_by_leaders(d: &Drawing, bubbles: &[(String, [f64; 2])]) -> (Vec<DetectedGrid>, Vec<DetectedGrid>) {
    if bubbles.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // The bubbles' radius: of the round shapes centred on them.
    let mut radii: Vec<f64> = d
        .model
        .iter()
        .filter_map(|e| round_shape(&e.kind))
        .take(MAX_ROUNDS)
        .filter(|(c, r)| bubbles.iter().any(|(_, b)| (b[0] - c[0]).hypot(b[1] - c[1]) < r * 0.2))
        .map(|(_, r)| r)
        .collect();
    radii.sort_by(f64::total_cmp);
    let Some(&r) = radii.get(radii.len() / 2) else { return (Vec::new(), Vec::new()) };
    let mut segs: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        match &e.kind {
            EntityKind::Line(l) => segs.push(([l.a.x, l.a.y], [l.b.x, l.b.y])),
            EntityKind::LwPolyline(pl) if round_shape(&e.kind).is_none() => {
                segs.extend(pl.vertices.windows(2).filter_map(|w| match w {
                    [a, b] => Some(([a.p.x, a.p.y], [b.p.x, b.p.y])),
                    _ => None,
                }));
            }
            _ => {}
        }
        if segs.len() >= MAX_LEADER_SEGS {
            break;
        }
    }
    // Per label: (votes for constant x, for constant y), and the positions.
    let mut votes: BTreeMap<&str, (usize, usize, Vec<[f64; 2]>)> = BTreeMap::new();
    for (label, c) in bubbles {
        let mut best: Option<(f64, [f64; 2])> = None;
        for (p, q) in &segs {
            for (near, far) in [(p, q), (q, p)] {
                let dn = (near[0] - c[0]).hypot(near[1] - c[1]);
                if dn < r * 0.7 || dn > r * 1.6 {
                    continue;
                }
                let v = [far[0] - near[0], far[1] - near[1]];
                let len = v[0].hypot(v[1]);
                if len < r * 0.4 {
                    continue;
                }
                let out = [(near[0] - c[0]) / dn, (near[1] - c[1]) / dn];
                let dir = [v[0] / len, v[1] / len];
                if dir[0] * out[0] + dir[1] * out[1] > 0.98 && best.is_none_or(|b| len > b.0) {
                    best = Some((len, dir));
                }
            }
        }
        let Some((_, dir)) = best else { continue };
        let e = votes.entry(label.as_str()).or_insert((0, 0, Vec::new()));
        if dir[0].abs() < 0.17 {
            e.0 += 1;
        } else if dir[1].abs() < 0.17 {
            e.1 += 1;
        } else {
            continue;
        }
        e.2.push(*c);
    }
    let (mut gx, mut gy) = (Vec::new(), Vec::new());
    for (label, (vx, vy, mut at)) in votes {
        let x = vx > vy;
        at.sort_by(|a, b| if x { a[0].total_cmp(&b[0]) } else { a[1].total_cmp(&b[1]) });
        let Some(mid) = at.get(at.len() / 2) else { continue };
        if x {
            gx.push(DetectedGrid { label: label.to_string(), at_in: mid[0] });
        } else if vy > 0 {
            gy.push(DetectedGrid { label: label.to_string(), at_in: mid[1] });
        }
    }
    gx.sort_by(|a, b| a.at_in.total_cmp(&b.at_in));
    gy.sort_by(|a, b| a.at_in.total_cmp(&b.at_in));
    (gx, gy)
}

/// Detect grid lines on the GRID layer.
fn detect_grid_lines(d: &Drawing) -> Vec<([f64; 2], [f64; 2])> {
    let mut lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for e in d.model.iter() {
        if !e.common.layer.eq_ignore_ascii_case("GRID") {
            continue;
        }
        if let EntityKind::Line(l) = &e.kind {
            lines.push(([l.a.x, l.a.y], [l.b.x, l.b.y]));
        }
    }
    lines
}

/// Classify detected grid lines into x-constant and y-constant, for unrotated plans.
fn group_grid_lines_simple(bubbles: &[(String, [f64; 2])]) -> (Vec<DetectedGrid>, Vec<DetectedGrid>) {
    let mut by_label: BTreeMap<String, Vec<[f64; 2]>> = BTreeMap::new();
    for (label, pos) in bubbles {
        by_label.entry(label.clone()).or_default().push(*pos);
    }

    let mut x_grids = Vec::new();
    let mut y_grids = Vec::new();

    for (label, positions) in &by_label {
        if positions.len() < 2 {
            continue;
        }
        let (a, b) = (positions[0], positions[1]);
        let dx = (b[0] - a[0]).abs();
        let dy = (b[1] - a[1]).abs();
        let mid_x = (a[0] + b[0]) / 2.0;
        let mid_y = (a[1] + b[1]) / 2.0;

        if dx < dy * 0.3 + 1e-6 {
            // Nearly same x → x-constant grid line.
            x_grids.push(DetectedGrid { label: label.clone(), at_in: mid_x });
        } else if dy < dx * 0.3 + 1e-6 {
            // Nearly same y → y-constant grid line.
            y_grids.push(DetectedGrid { label: label.clone(), at_in: mid_y });
        }
        // else: rotated plan, classification deferred to alignment.
    }

    x_grids.sort_by(|a, b| a.at_in.total_cmp(&b.at_in));
    y_grids.sort_by(|a, b| a.at_in.total_cmp(&b.at_in));

    (x_grids, y_grids)
}

// ---- Alignment computation ----------------------------------------------------------

/// Compute alignment using bubble pairs and truth grid.
///
/// Uses the direction of bubble pairs to determine rotation, then computes offset using
/// grid line intersections or perpendicular projections.
/// Largest misfit of a grid line, model inches (two feet), for [`alignment_from_lines`].
const LINE_FIT_TOLERANCE_IN: f64 = 24.0;

/// Most of `misfits` (two thirds, at least two) within [`LINE_FIT_TOLERANCE_IN`]: bubbles
/// staggered off lines too close together, and a label or two misread, don't spoil a fit.
fn mostly_fit(misfits: &[f64]) -> bool {
    let good = misfits.iter().filter(|m| m.abs() <= LINE_FIT_TOLERANCE_IN).count();
    good >= 2 && good * 3 >= misfits.len() * 2
}

/// An unrotated sheet's alignment from the grid lines it read: per direction, the offset most
/// of its lines found in the common grid agree on ([`mostly_fit`]).
fn alignment_from_lines(sx: &[DetectedGrid], sy: &[DetectedGrid], grid_x: &[GridLine], grid_y: &[GridLine]) -> Option<Alignment> {
    let fit = |found: &[DetectedGrid], common: &[GridLine]| -> Option<f64> {
        let mut offsets: Vec<f64> =
            found.iter().filter_map(|g| common.iter().find(|c| c.label == g.label).map(|c| g.at_in - c.at_ft * 12.0)).collect();
        offsets.sort_by(f64::total_cmp);
        let median = *offsets.get(offsets.len() / 2)?;
        let misfits: Vec<f64> = offsets.iter().map(|o| o - median).collect();
        if !mostly_fit(&misfits) {
            return None;
        }
        let good: Vec<f64> = offsets.iter().copied().filter(|o| (o - median).abs() <= LINE_FIT_TOLERANCE_IN).collect();
        Some(good.iter().sum::<f64>() / good.len().max(1) as f64)
    };
    let ox = fit(sx, grid_x)?;
    let oy = fit(sy, grid_y)?;
    Some(Alignment { rotation_deg: 0.0, offset_in: [ox, oy] })
}

/// Whether, under `a`, most bubbles of the common grid's labels sit on their lines.
fn bubbles_on_lines(a: &Alignment, bubbles: &[(String, [f64; 2])], grid_x: &[GridLine], grid_y: &[GridLine]) -> bool {
    let misfits: Vec<f64> = bubbles
        .iter()
        .filter_map(|(label, at)| {
            let b = building_from_model(a, *at);
            grid_x
                .iter()
                .find(|g| &g.label == label)
                .map(|g| (b[0] - g.at_ft) * 12.0)
                .or_else(|| grid_y.iter().find(|g| &g.label == label).map(|g| (b[1] - g.at_ft) * 12.0))
        })
        .collect();
    mostly_fit(&misfits)
}

/// Grid labels in their natural order: numbers by value (`2` before `10`, `1.5` between), else
/// shorter first (`Z` before `AA`), then alphabetically. Grids are labelled so these ascend
/// along building +x and +y.
fn label_key(label: &str) -> (u8, f64, usize, String) {
    let t = label.trim().to_uppercase();
    match t.parse::<f64>() {
        Ok(v) if v.is_finite() => (0, v, 0, t),
        _ => (1, 0.0, t.chars().count(), t),
    }
}

/// Whether lines read in model order (`at_in` ascending) carry their labels in natural order:
/// more neighbours ascend than descend ([`label_key`]). A sheet plotted turned 180° reads its
/// lines in reverse.
fn ascends(grids: &[DetectedGrid]) -> bool {
    let mut sorted: Vec<&DetectedGrid> = grids.iter().collect();
    sorted.sort_by(|a, b| a.at_in.total_cmp(&b.at_in));
    let (mut up, mut down) = (0usize, 0usize);
    for w in sorted.windows(2) {
        if let [a, b] = w {
            match label_key(&a.label).partial_cmp(&label_key(&b.label)) {
                Some(std::cmp::Ordering::Less) => up += 1,
                Some(std::cmp::Ordering::Greater) => down += 1,
                _ => {}
            }
        }
    }
    up > down
}

/// Largest turn off square, degrees, a sheet's bubble pairs may show and its read lines still
/// be taken as square (pair ends a print's 0.01" off show a few hundredths of a degree).
const SQUARE_DEG: f64 = 0.25;

/// How far a sheet's bubble pairs turn it off square, degrees in (-45, 45]: the circular mean
/// of each pair's direction taken modulo 90° (four times the angle, as unit vectors). `None`
/// with no pairs, or when the pairs don't agree on a turn.
fn pair_turn_deg(bubbles: &[(String, [f64; 2])]) -> Option<f64> {
    let (mut c, mut s, mut n) = (0.0, 0.0, 0usize);
    for d in pair_directions(bubbles) {
        let a = d[1].atan2(d[0]) * 4.0;
        c += a.cos();
        s += a.sin();
        n += 1;
    }
    if n == 0 || c.hypot(s) < 0.5 * n as f64 {
        return None;
    }
    Some(s.atan2(c).to_degrees() / 4.0)
}

/// The unit direction from a label's first bubble to its second, for every label with two.
fn pair_directions(bubbles: &[(String, [f64; 2])]) -> Vec<[f64; 2]> {
    let mut by_label: BTreeMap<&str, Vec<[f64; 2]>> = BTreeMap::new();
    for (label, pos) in bubbles {
        by_label.entry(label.as_str()).or_default().push(*pos);
    }
    by_label
        .values()
        .filter_map(|p| {
            let (a, b) = (p.first()?, p.get(1)?);
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = d[0].hypot(d[1]);
            (l > 1e-9 && l.is_finite()).then(|| [d[0] / l, d[1] / l])
        })
        .collect()
}

fn compute_alignment_from_bubbles(bubbles: &[(String, [f64; 2])], grid_x: &[GridLine], grid_y: &[GridLine]) -> Option<Alignment> {
    let x_map: BTreeMap<&str, f64> = grid_x.iter().map(|g| (g.label.as_str(), g.at_ft)).collect();
    let y_map: BTreeMap<&str, f64> = grid_y.iter().map(|g| (g.label.as_str(), g.at_ft)).collect();

    let mut by_label: BTreeMap<&str, Vec<[f64; 2]>> = BTreeMap::new();
    for (label, pos) in bubbles {
        by_label.entry(label.as_str()).or_default().push(*pos);
    }

    // Step 1: the rotation from bubble pair directions. A pair's direction gives the turn only
    // up to a half turn (which bubble is "first" is arbitrary), so the estimates are averaged
    // as axes: the mean of (cos 2θ, sin 2θ), halved. A plain mean of angles put in [0, π)
    // breaks at the seam: a square sheet whose pairs are a hair off either way (±ε, read as ε
    // and π − ε) averaged to 90°.
    // x-grid labels (constant x, runs along building y): direction = ±[-sin θ, cos θ]
    // y-grid labels (constant y, runs along building x): direction = ±[cos θ, sin θ]
    let (mut c2, mut s2, mut n) = (0.0, 0.0, 0usize);
    for (label, positions) in &by_label {
        let (Some(a), Some(b)) = (positions.first(), positions.get(1)) else { continue };
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = d[0].hypot(d[1]);
        if l < 1e-9 || !l.is_finite() {
            continue;
        }
        let dir = [d[0] / l, d[1] / l];
        let estimate = if x_map.contains_key(label) {
            (-dir[0]).atan2(dir[1])
        } else if y_map.contains_key(label) {
            dir[1].atan2(dir[0])
        } else {
            continue;
        };
        c2 += (2.0 * estimate).cos();
        s2 += (2.0 * estimate).sin();
        n += 1;
    }
    // Pairs that don't agree on an axis (a detail's few bubbles) give no rotation.
    if n == 0 || c2.hypot(s2) < 0.5 * n as f64 {
        return None;
    }
    let axis = s2.atan2(c2) / 2.0;

    // Which way along the axis: the grid's labels ascend along building +x and +y (the common
    // grid is built so, [`assemble_grid`]), so turned the right way each line's position across
    // the sheet rises with its place in the common grid; turned a half turn wrong, it falls.
    // This tells 0° from 180° (and 90° from 270°).
    let order = |theta: f64| -> f64 {
        let (sin_t, cos_t) = theta.sin_cos();
        let mut along: Vec<(f64, f64)> = Vec::new(); // (position across the sheet, common-grid feet)
        let mut across: Vec<(f64, f64)> = Vec::new();
        for (label, positions) in &by_label {
            let (Some(a), Some(b)) = (positions.first(), positions.get(1)) else { continue };
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            if let Some(&ft) = x_map.get(label) {
                along.push((mid[0] * cos_t + mid[1] * sin_t, ft));
            } else if let Some(&ft) = y_map.get(label) {
                across.push((-mid[0] * sin_t + mid[1] * cos_t, ft));
            }
        }
        let cov = |v: &[(f64, f64)]| -> f64 {
            if v.is_empty() {
                return 0.0;
            }
            let k = v.len() as f64;
            let (mp, mf) = (v.iter().map(|p| p.0).sum::<f64>() / k, v.iter().map(|p| p.1).sum::<f64>() / k);
            v.iter().map(|(p, f)| (p - mp) * (f - mf)).sum()
        };
        cov(&along) + cov(&across)
    };
    let theta = if order(axis) < 0.0 { axis + std::f64::consts::PI } else { axis };
    // In (-180°, 180°].
    let mut rotation_deg = theta.to_degrees();
    if rotation_deg > 180.0 {
        rotation_deg -= 360.0;
    }

    // Step 2: compute offset using grid line midpoints.
    // For an x-grid bubble pair with label L:
    //   midpoint_model = rotate([L_ft * 12, C_y * 12], θ) + offset
    // where C_y is the building y-centre of the grid.
    // We don't know C_y, but the perpendicular projection of the midpoint onto the building
    // x-axis gives:
    //   midpoint · [cos θ, sin θ] = L_ft * 12 + offset · [cos θ, sin θ]
    //
    // Similarly for y-grid:
    //   midpoint · [-sin θ, cos θ] = L_ft * 12 + offset · [-sin θ, cos θ]

    let (cos_t, sin_t) = theta.sin_cos();
    // Note: sin_cos returns (sin, cos), so swap.
    let (sin_t, cos_t) = (cos_t, sin_t);
    let bx_dir = [cos_t, sin_t]; // building x direction in model
    let by_dir = [-sin_t, cos_t]; // building y direction in model

    let mut off_bx_estimates = Vec::new(); // offset projected onto building-x direction
    let mut off_by_estimates = Vec::new();

    for (label, positions) in &by_label {
        if positions.len() < 2 {
            continue;
        }
        let mid = [(positions[0][0] + positions[1][0]) / 2.0, (positions[0][1] + positions[1][1]) / 2.0];

        if let Some(&ft) = x_map.get(label) {
            // x-grid line: building x = ft.
            // mid · bx_dir = ft * 12 + offset · bx_dir
            let proj = mid[0] * bx_dir[0] + mid[1] * bx_dir[1];
            off_bx_estimates.push(proj - ft * 12.0);
        }
        if let Some(&ft) = y_map.get(label) {
            // y-grid line: building y = ft.
            // mid · by_dir = ft * 12 + offset · by_dir
            let proj = mid[0] * by_dir[0] + mid[1] * by_dir[1];
            off_by_estimates.push(proj - ft * 12.0);
        }
    }

    if off_bx_estimates.is_empty() || off_by_estimates.is_empty() {
        return None;
    }

    let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let off_bx = avg(&off_bx_estimates);
    let off_by = avg(&off_by_estimates);

    // Reconstruct offset in model coordinates.
    let offset_in = [off_bx * bx_dir[0] + off_by * by_dir[0], off_bx * bx_dir[1] + off_by * by_dir[1]];

    Some(Alignment { rotation_deg, offset_in: [round(offset_in[0]), round(offset_in[1])] })
}

fn round(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

// ---- Public API ---------------------------------------------------------------------

fn all_texts(d: &Drawing) -> Vec<(String, [f64; 2])> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some((t.value.clone(), [t.insert.x, t.insert.y])),
            EntityKind::MText(t) => Some((t.contents.clone(), [t.insert.x, t.insert.y])),
            _ => None,
        })
        .collect()
}

/// Recognise one DXF sheet: title block, scale, grid.
pub fn recognise_dxf(d: &Drawing) -> RecognisedSheet {
    let number = detect_sheet_number(d).unwrap_or_default();
    let title = detect_title(d, &number);
    let texts = all_texts(d);
    let discipline = detect_discipline(&number, &texts);
    let level = detect_level(&title);
    let (scale_text, scale) = detect_scale_text(d).map(|(t, s)| (Some(t), s)).unwrap_or((None, 0.0));

    let (bubbles, grid_x, grid_y) = detect_grid(d);
    let mut levels = detect_levels(&title);
    let mut level = level;
    if level.is_none() && title.to_uppercase().contains("PLAN") {
        // A plan whose title block doesn't name a level: the drawing's own view title may
        // (`SECOND FLOOR PLAN - AREA B` under the plan).
        if let Some(v) = plan_views(d).into_iter().next() {
            level = v.levels.first().cloned();
            levels = v.levels;
        }
    }

    RecognisedSheet {
        number,
        title,
        discipline,
        level,
        scale_text,
        scale,
        grid_x,
        grid_y,
        alignment: None,
        bubbles,
        levels,
        printed_number: String::new(),
    }
}

/// The grid bubbles and the grid lines they label.
fn detect_grid(d: &Drawing) -> (Vec<(String, [f64; 2])>, Vec<DetectedGrid>, Vec<DetectedGrid>) {
    let bubbles = detect_grid_bubbles(d);
    let _grid_lines = detect_grid_lines(d);
    let (mut grid_x, mut grid_y) = group_grid_lines_simple(&bubbles);
    // One bubble per line, or a plan split over two views: each bubble's own line says which
    // way it runs. Pairs read off a few stray tags (two `1` keynotes in line) don't make a
    // grid, so the reading that finds more lines wins.
    let paired = if grid_x.is_empty() || grid_y.is_empty() { 0 } else { grid_x.len() + grid_y.len() };
    if paired < bubbles.len() / 3 {
        let (lx, ly) = grid_by_leaders(d, &bubbles);
        if !lx.is_empty() && !ly.is_empty() && lx.len() + ly.len() > paired {
            (grid_x, grid_y) = (lx, ly);
        }
    }
    // A plan printed with its grid lines off (one bubble per line, nothing leaving them): the
    // bubbles still stand in a row along one edge and a column along another.
    let read = if grid_x.is_empty() || grid_y.is_empty() { 0 } else { grid_x.len() + grid_y.len() };
    if read < bubbles.len() / 3 {
        let (rx, ry) = grid_by_rows(d, &bubbles);
        if !rx.is_empty() && !ry.is_empty() && rx.len() + ry.len() > read {
            (grid_x, grid_y) = (rx, ry);
        }
    }
    (bubbles, grid_x, grid_y)
}

/// Fewest bubbles in a row (or a column) that [`grid_by_rows`] takes for grid lines.
const MIN_BUBBLE_ROW: usize = 3;

/// Grid lines from where one-per-line bubbles stand: the longest row of bubbles (centres level
/// within three radii, as bubbles on close lines are staggered) are lines of constant x at
/// their x, the longest column lines of constant y at their y. Each needs
/// [`MIN_BUBBLE_ROW`] different labels; a label in both keeps the longer reading.
fn grid_by_rows(d: &Drawing, bubbles: &[(String, [f64; 2])]) -> (Vec<DetectedGrid>, Vec<DetectedGrid>) {
    let mut radii: Vec<f64> = d
        .model
        .iter()
        .filter_map(|e| round_shape(&e.kind))
        .take(MAX_ROUNDS)
        .filter(|(c, r)| bubbles.iter().any(|(_, b)| (b[0] - c[0]).hypot(b[1] - c[1]) < r * 0.2))
        .map(|(_, r)| r)
        .collect();
    radii.sort_by(f64::total_cmp);
    let Some(&r) = radii.get(radii.len() / 2) else { return (Vec::new(), Vec::new()) };
    let tol = r * 3.0;
    // The longest run of bubbles whose `across` coordinate stays within `tol` of the run's first,
    // one bubble per label, placed by `along`.
    let longest = |across: usize, along: usize| -> Vec<DetectedGrid> {
        let mut sorted: Vec<&(String, [f64; 2])> = bubbles.iter().collect();
        sorted.sort_by(|a, b| a.1.get(across).copied().unwrap_or(0.0).total_cmp(&b.1.get(across).copied().unwrap_or(0.0)));
        let mut best: Vec<DetectedGrid> = Vec::new();
        for (i, first) in sorted.iter().enumerate() {
            let at0 = first.1.get(across).copied().unwrap_or(0.0);
            let mut run: Vec<DetectedGrid> = Vec::new();
            for b in sorted.iter().skip(i).take_while(|b| b.1.get(across).copied().unwrap_or(0.0) - at0 <= tol) {
                if !run.iter().any(|g| g.label == b.0) {
                    run.push(DetectedGrid { label: b.0.clone(), at_in: b.1.get(along).copied().unwrap_or(0.0) });
                }
            }
            if run.len() > best.len() {
                best = run;
            }
        }
        if best.len() < MIN_BUBBLE_ROW {
            return Vec::new();
        }
        best.sort_by(|a, b| a.at_in.total_cmp(&b.at_in));
        best
    };
    let (mut gx, mut gy) = (longest(1, 0), longest(0, 1));
    if gx.len() >= gy.len() {
        gy.retain(|g| !gx.iter().any(|x| x.label == g.label));
    } else {
        gx.retain(|g| !gy.iter().any(|y| y.label == g.label));
    }
    if gx.len() < MIN_BUBBLE_ROW || gy.len() < MIN_BUBBLE_ROW {
        return (Vec::new(), Vec::new());
    }
    (gx, gy)
}

/// A plan view's title printed under it on the sheet (`2ND FLOOR LIGHTING PLAN`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanView {
    pub title: String,
    /// The levels it names, lowest first.
    pub levels: Vec<String>,
    /// Where the title is, model units.
    pub at: [f64; 2],
    pub height: f64,
}

/// The plan views' titles on a sheet: lettering well above the sheet's usual size, short, naming
/// a level, outside the title block's column. Bottom to top, then left to right.
pub fn plan_views(d: &Drawing) -> Vec<PlanView> {
    let texts = texts_across(d);
    let mut heights: Vec<f64> = texts.iter().map(|t| t.1).filter(|h| h.is_finite() && *h > 0.0).collect();
    if heights.is_empty() {
        return Vec::new();
    }
    heights.sort_by(f64::total_cmp);
    let median = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    let mut views: Vec<PlanView> = texts
        .iter()
        .filter(|(v, h, at, _)| at[0] <= 0.8 && *h >= median * 1.4 && v.len() <= 60 && !v.to_uppercase().contains("NOTE"))
        .filter_map(|(v, h, _, p)| {
            let levels = detect_levels(v);
            (!levels.is_empty()).then(|| PlanView { title: v.clone(), levels, at: *p, height: *h })
        })
        .take(16)
        .collect();
    // Two titles of one view (`1ST FLOOR LIGHTING PLAN` and `LEVEL 1 LIGHTING PLAN` under the
    // same plan): the same levels a few lines apart are one view, titled by the larger. Views
    // side by side (a floor's two halves, or its piping and its ductwork) title on one line and stay apart.
    // One title read twice at one spot (an imported PDF's hidden text and the OCR's reading of its
    // stroked lettering, at different heights) is one view too: kept twice, the first copy's
    // cell would end at the second copy, just under the title, and hold none of the plan.
    views.sort_by(|a, b| b.height.total_cmp(&a.height));
    let mut kept: Vec<PlanView> = Vec::new();
    for v in views {
        let one_view = |k: &PlanView| {
            let (dx, dy, h) = ((k.at[0] - v.at[0]).abs(), (k.at[1] - v.at[1]).abs(), k.height.max(v.height));
            k.levels == v.levels && ((dy > h * 0.5 && dy <= h * 8.0) || (dy <= h * 0.5 && dx <= h * 2.0))
        };
        if !kept.iter().any(one_view) {
            kept.push(v);
        }
    }
    kept.sort_by(|a, b| a.at[1].total_cmp(&b.at[1]).then(a.at[0].total_cmp(&b.at[0])));
    kept
}

/// The drawing with only the entities whose middle `keep` accepts.
fn crop(d: &Drawing, keep: &dyn Fn([f64; 2]) -> bool) -> Drawing {
    let mut sub = d.clone();
    sub.model = cadaddy_doc::EntityStore::new();
    let kept: Vec<cadaddy_doc::Entity> = d
        .model
        .iter()
        .filter(|e| {
            let b = cadaddy_doc::entity_bounds(d, e, 0);
            !b.is_empty() && keep([b.center().x, b.center().y])
        })
        .map(|e| (**e).clone())
        .collect();
    sub.model.push_many(kept);
    sub
}

/// Recognise a sheet that may print several plan views, each of its own level (`1ST FLOOR
/// LIGHTING PLAN` under one plan, `2ND FLOOR LIGHTING PLAN` under the other): one sheet per view,
/// numbered `<number>/<level>`, with that view's grid, so each aligns and reads on its own. A
/// sheet with one view (or views of the same levels) is recognised whole.
pub fn recognise_views(d: &Drawing) -> Vec<RecognisedSheet> {
    split_views(d).into_iter().map(|(s, _)| s).collect()
}

/// [`recognise_views`], each view with its part of the drawing (`None`: the whole sheet).
pub fn split_views(d: &Drawing) -> Vec<(RecognisedSheet, Option<Drawing>)> {
    let sheet = recognise_dxf(d);
    let cells = view_cells(d);
    if cells.is_empty() {
        return vec![(sheet, None)];
    }
    let mut out: Vec<(RecognisedSheet, Option<Drawing>)> = Vec::new();
    for (v, [x0, y0, x1, y1]) in cells {
        let sub = crop(d, &|p: [f64; 2]| p[0] >= x0 && p[0] < x1 && p[1] >= y0 && p[1] < y1);
        let (bubbles, grid_x, grid_y) = detect_grid(&sub);
        let level = v.levels.first().cloned();
        let mut number = match (&level, sheet.number.is_empty()) {
            (Some(l), false) => format!("{}/{l}", sheet.number),
            _ => sheet.number.clone(),
        };
        // Two views of one level (its piping and its ductwork): `/1`, `/1.2`.
        let base = number.clone();
        let mut k = 1;
        while out.iter().any(|(r, _)| r.number == number) && k < 64 {
            k += 1;
            number = format!("{base}.{k}");
        }
        let rec = RecognisedSheet {
            number,
            title: v.title.clone(),
            discipline: sheet.discipline.clone(),
            level,
            scale_text: sheet.scale_text.clone(),
            scale: sheet.scale,
            grid_x,
            grid_y,
            alignment: None,
            bubbles,
            levels: v.levels.clone(),
            printed_number: sheet.number.clone(),
        };
        out.push((rec, Some(sub)));
    }
    out
}

/// The plan views of a sheet that prints several, each of its own level (`3RD FLOOR PLAN` under
/// one plan, `4TH FLOOR PLAN` under the other), each with its cell `[x0, y0, x1, y1]` (model
/// units; open to infinity at the sheet's outer edges). Empty for a sheet of one view, or of
/// views of the same levels.
pub fn view_cells(d: &Drawing) -> Vec<(PlanView, [f64; 4])> {
    let views = plan_views(d);
    let mut distinct: Vec<&Vec<String>> = views.iter().map(|v| &v.levels).collect();
    distinct.sort();
    distinct.dedup();
    if distinct.len() < 2 || views.len() > 8 {
        return Vec::new();
    }
    // Each view's cell: the views stand in columns (titles at about the same x, a tenth of the
    // sheet or less apart), a column's views one above another. A title sits under its view:
    // a view starts just below its title and ends where the next one up starts; a column ends
    // halfway to the next column's titles.
    let width = texts_across(d).iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| (lo.min(t.3[0]), hi.max(t.3[0])));
    let gap = ((width.1 - width.0) * 0.1).max(1e-9);
    let mut views = views;
    views.sort_by(|a, b| a.at[0].total_cmp(&b.at[0]));
    let mut columns: Vec<Vec<PlanView>> = Vec::new();
    for v in views {
        match columns.last_mut() {
            Some(c) if c.last().is_some_and(|l| v.at[0] - l.at[0] <= gap) => c.push(v),
            _ => columns.push(vec![v]),
        }
    }
    let centre = |c: &Vec<PlanView>| c.iter().map(|v| v.at[0]).sum::<f64>() / c.len().max(1) as f64;
    let mut cells: Vec<(PlanView, [f64; 4])> = Vec::new();
    for (ci, col) in columns.iter().enumerate() {
        let x0 = if ci == 0 { f64::NEG_INFINITY } else { columns.get(ci - 1).map_or(f64::NEG_INFINITY, |p| (centre(p) + centre(col)) / 2.0) };
        let x1 = columns.get(ci + 1).map_or(f64::INFINITY, |n| (centre(col) + centre(n)) / 2.0);
        let mut col = col.clone();
        col.sort_by(|a, b| a.at[1].total_cmp(&b.at[1]));
        for (i, v) in col.iter().enumerate() {
            let y0 = if i == 0 { f64::NEG_INFINITY } else { v.at[1] - v.height };
            let y1 = col.get(i + 1).map_or(f64::INFINITY, |n| n.at[1] - n.height);
            cells.push((v.clone(), [x0, y0, x1, y1]));
        }
    }
    cells
}

/// Build a sheet map from a set of DXF drawings: recognise each sheet, assemble the common
/// grid, then compute each sheet's alignment.
pub fn build_sheet_map(drawings: &[(String, &Drawing)]) -> SheetMap {
    let mut sheets: Vec<RecognisedSheet> = Vec::new();

    // Phase 1: recognise each sheet.
    for (name, d) in drawings {
        let mut sh = recognise_dxf(d);
        if sh.number.is_empty() {
            sh.number = name.clone();
        }
        sheets.push(sh);
    }
    sheet_map_from(sheets)
}

/// The sheet map of sheets already recognised one at a time ([`recognise_dxf`]), so a large
/// set needn't hold every drawing at once: the common grid assembled, each sheet aligned.
pub fn sheet_map_from(mut sheets: Vec<RecognisedSheet>) -> SheetMap {
    // Phase 2: assemble the common grid from all sheets' detected grid lines.
    let (grid_x, grid_y) = assemble_grid(&sheets);

    // Phase 3: compute alignment for each sheet.
    for sh in &mut sheets {
        if sh.bubbles.is_empty() {
            continue;
        }
        // An unrotated plan: the lines it read fit the common grid square (this also aligns
        // one view of a two-view sheet, with one bubble per line and so no pairs to turn by).
        // Else bubble-based alignment (rotated plans), kept when the bubbles then sit on their
        // lines (a detail or an elevation with a few bubbles gives nonsense). A sheet whose
        // bubble pairs show it turned a little off square isn't read square: its lines fit
        // within the tolerance yet 2.5° puts the far end of a long building feet out.
        let turned = pair_turn_deg(&sh.bubbles).is_some_and(|t| t.abs() > SQUARE_DEG);
        let square = if turned { None } else { alignment_from_lines(&sh.grid_x, &sh.grid_y, &grid_x, &grid_y) };
        sh.alignment = square
            .or_else(|| compute_alignment_from_bubbles(&sh.bubbles, &grid_x, &grid_y).filter(|a| bubbles_on_lines(a, &sh.bubbles, &grid_x, &grid_y)));
    }

    SheetMap { sheets, grid_x, grid_y }
}

/// Compute a fingerprint of a sheet's grid label classification: which labels are in grid_x.
fn grid_x_labels(sh: &RecognisedSheet) -> Vec<String> {
    let mut labels: Vec<String> = sh.grid_x.iter().map(|g| g.label.clone()).collect();
    labels.sort();
    labels
}

/// Assemble the common grid from detected grids across all sheets.
/// Uses majority-vote label classification to handle rotated sheets correctly:
/// most sheets are unrotated, so the majority classification of labels into x/y is correct.
fn assemble_grid(sheets: &[RecognisedSheet]) -> (Vec<GridLine>, Vec<GridLine>) {
    // Count label classifications across all sheets. The majority determines the correct
    // assignment of labels to x-grid vs y-grid (rotated sheets are the minority).
    let valid: Vec<&RecognisedSheet> = sheets.iter().filter(|s| s.scale > 1e-9 && !s.grid_x.is_empty() && !s.grid_y.is_empty()).collect();
    if valid.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Group by label classification fingerprint, pick the most common one.
    let mut classification_counts: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    for s in &valid {
        *classification_counts.entry(grid_x_labels(s)).or_default() += 1;
    }
    let majority_x_labels = classification_counts.into_iter().max_by_key(|(_, count)| *count).map(|(labels, _)| labels);

    let Some(majority_x) = majority_x_labels else {
        return (Vec::new(), Vec::new());
    };

    // Pick the best sheet matching the majority classification: one plotted square and the
    // right way up (its lines in label order along +x and +y, [`ascends`]) if there is one, as
    // its lines become the building frame; then the one reading most lines.
    let upright_square = |s: &RecognisedSheet| -> (bool, bool) {
        let square = pair_turn_deg(&s.bubbles).is_none_or(|t| t.abs() <= SQUARE_DEG);
        (ascends(&s.grid_x) && ascends(&s.grid_y) && square, square)
    };
    let best = valid.iter().filter(|s| grid_x_labels(s) == majority_x).max_by_key(|s| {
        let (upright, square) = upright_square(s);
        (upright, square, s.grid_x.len() + s.grid_y.len())
    });

    let Some(ref_sheet) = best else {
        return (Vec::new(), Vec::new());
    };
    // Every sheet read the same way up as the reference adds its reading of each line, shifted
    // onto the reference: one sheet's bubbles a little off don't set the building frame alone.
    let (ref_upright, ref_square) = upright_square(ref_sheet);
    let agree: Vec<&RecognisedSheet> =
        valid.iter().copied().filter(|s| grid_x_labels(s) == majority_x && upright_square(s) == (ref_upright, ref_square)).collect();
    let consensus = |pick: &dyn Fn(&RecognisedSheet) -> &[DetectedGrid]| -> Vec<DetectedGrid> {
        let reference = pick(ref_sheet);
        let mut sums: BTreeMap<&str, (f64, usize)> = BTreeMap::new();
        for s in &agree {
            let pairs: Vec<(&str, f64)> =
                pick(s).iter().filter_map(|g| reference.iter().find(|r| r.label == g.label).map(|r| (r.label.as_str(), g.at_in - r.at_in))).collect();
            let mut shifts: Vec<f64> = pairs.iter().map(|p| p.1).collect();
            shifts.sort_by(f64::total_cmp);
            let Some(&median) = shifts.get(shifts.len() / 2) else { continue };
            let misfits: Vec<f64> = shifts.iter().map(|d| d - median).collect();
            if !mostly_fit(&misfits) {
                continue;
            }
            let fits: Vec<(&str, f64)> = pairs.into_iter().filter(|p| (p.1 - median).abs() <= LINE_FIT_TOLERANCE_IN).collect();
            let shift = fits.iter().map(|p| p.1).sum::<f64>() / fits.len().max(1) as f64;
            for (label, d) in fits {
                let e = sums.entry(label).or_default();
                e.0 += d - shift;
                e.1 += 1;
            }
        }
        reference
            .iter()
            .map(|r| {
                let mean = sums.get(r.label.as_str()).map_or(0.0, |(sum, n)| sum / (*n).max(1) as f64);
                DetectedGrid { label: r.label.clone(), at_in: r.at_in + mean }
            })
            .collect()
    };

    let to_ft = |grids: &[DetectedGrid]| -> Vec<GridLine> {
        // Building coordinates start at the reference sheet's first line each way (the lowest
        // label of an upright sheet) at 0.
        let Some(origin) = grids.first().map(|g| g.at_in) else { return Vec::new() };
        grids.iter().map(|g| GridLine { label: g.label.clone(), at_ft: (g.at_in - origin) / 12.0 }).collect()
    };

    (to_ft(&consensus(&|s| &s.grid_x)), to_ft(&consensus(&|s| &s.grid_y)))
}

/// Convert model inches to building feet using a sheet's alignment.
pub fn building_from_model(alignment: &Alignment, m: [f64; 2]) -> [f64; 2] {
    let p = [m[0] - alignment.offset_in[0], m[1] - alignment.offset_in[1]];
    let r = layout::rotate(p, -alignment.rotation_deg);
    [r[0] / 12.0, r[1] / 12.0]
}

// ---- Linework without layer names -------------------------------------------------------------

/// Most straight pieces [`drawn_segments`] takes from one drawing (hostile drawings).
const MAX_DRAWN_SEGMENTS: usize = 200_000;

/// Whether an entity is drawn in a screened (gray) pen: the background a trade's plan is drawn
/// over (the architecture under an electrical plan, the grid). Its colour, else its layer's, is
/// a gray between black and white; nothing is read from the layer's name, so a print's pen
/// layers (`PDF-GRAY50-0.18`) and a drawing's own (`ARCH`, colour 8) say the same.
pub fn screened(d: &Drawing, e: &cadaddy_doc::Entity) -> bool {
    let layer = d.layers.iter().find(|l| l.name.eq_ignore_ascii_case(&e.common.layer)).map_or(cadaddy_color::Color::Index(7), |l| l.color);
    let c = e.common.color.resolve(layer, cadaddy_color::Color::Index(7));
    let (lo, hi) = (c.0.min(c.1).min(c.2), c.0.max(c.1).max(c.2));
    hi - lo <= 16 && (40..=230).contains(&lo)
}

/// A polyline that is an outline: closed, or ending where it began (a print's closed path).
fn shut(p: &cadaddy_doc::LwPolyline) -> bool {
    p.closed || p.vertices.len() > 3 && p.vertices.first().zip(p.vertices.last()).is_some_and(|(a, b)| (a.p.x - b.p.x).hypot(a.p.y - b.p.y) < 1e-6)
}

/// The round shapes of a drawing (circles, and closed polylines as round as a printed circle):
/// centre and radius, drawing units.
/// A print draws a symbol in one path: a polyline that goes round and comes back to its start,
/// then on (a bubble and its divider) is round in that first loop.
pub fn round_shapes(d: &Drawing) -> Vec<([f64; 2], f64)> {
    let first_loop = |kind: &EntityKind| -> Option<([f64; 2], f64)> {
        let EntityKind::LwPolyline(pl) = kind else { return None };
        let v0 = pl.vertices.first()?.p;
        let j = pl.vertices.iter().enumerate().skip(7).find(|(_, v)| (v.p.x - v0.x).hypot(v.p.y - v0.y) < 1e-6)?.0;
        let ring = cadaddy_doc::LwPolyline { vertices: pl.vertices.get(..j)?.to_vec(), closed: true, ..pl.clone() };
        round_shape(&EntityKind::LwPolyline(ring))
    };
    d.model
        .iter()
        .filter_map(|e| round_shape(&e.kind).or_else(|| first_loop(&e.kind)))
        .filter(|(c, r)| c.iter().all(|v| v.is_finite()) && r.is_finite() && *r > 0.0)
        .take(MAX_ROUNDS)
        .collect()
}

/// The straight linework drawn in the drawing pens (not [`screened`]), in drawing units: lines,
/// and the straight sides of open polylines (a print often joins a run and its neighbours into
/// one path), and of closed ones too with `outlines` (a printed wall is often an outline; a
/// run never is). For readers that can't lean on layer names, as on a print. A piece across a
/// round shape (a callout bubble's divider, a symbol's bar) is the symbol's, not linework.
pub fn drawn_segments(d: &Drawing, outlines: bool) -> Vec<[[f64; 2]; 2]> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if out.len() >= MAX_DRAWN_SEGMENTS {
            break;
        }
        match &e.kind {
            EntityKind::Line(l) if !screened(d, e) => out.push([[l.a.x, l.a.y], [l.b.x, l.b.y]]),
            EntityKind::LwPolyline(p) if !screened(d, e) && (outlines || !shut(p)) && round_shape(&e.kind).is_none() => {
                let n = p.vertices.len();
                let sides = if p.closed { n } else { n.saturating_sub(1) };
                for i in 0..sides {
                    let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get((i + 1) % n)) else { continue };
                    if a.bulge.abs() < 1e-9 && out.len() < MAX_DRAWN_SEGMENTS {
                        out.push([[a.p.x, a.p.y], [b.p.x, b.p.y]]);
                    }
                }
            }
            _ => {}
        }
    }
    out.retain(|s| s.iter().flatten().all(|v| v.is_finite()));
    // Round shapes by cell (a cell as wide as the largest), to find the one a piece crosses.
    let rounds = round_shapes(d);
    let cell = rounds.iter().map(|r| r.1 * 2.0).fold(0.0, f64::max);
    if cell <= 0.0 {
        return out;
    }
    let key = |p: [f64; 2]| ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64);
    let mut cells: BTreeMap<(i64, i64), Vec<([f64; 2], f64)>> = BTreeMap::new();
    for r in &rounds {
        cells.entry(key(r.0)).or_default().push(*r);
    }
    let across = |s: &[[f64; 2]; 2]| {
        let m = [(s[0][0] + s[1][0]) / 2.0, (s[0][1] + s[1][1]) / 2.0];
        let (kx, ky) = key(m);
        (-1..=1).flat_map(|dx| (-1..=1).map(move |dy| (kx + dx, ky + dy))).filter_map(|k| cells.get(&k)).flatten().any(|(c, r)| {
            let on = |p: [f64; 2]| (p[0] - c[0]).hypot(p[1] - c[1]) <= r * 1.1;
            on(s[0]) && on(s[1]) && (m[0] - c[0]).hypot(m[1] - c[1]) < r * 0.7
        })
    };
    out.retain(|s| !across(s));
    out
}

/// The structural grid's extents in building feet, `margin` past it (none without a grid).
pub fn grid_box(map: &SheetMap, margin: f64) -> Option<([f64; 2], [f64; 2])> {
    let xs: Vec<f64> = map.grid_x.iter().map(|g| g.at_ft).filter(|v| v.is_finite()).collect();
    let ys: Vec<f64> = map.grid_y.iter().map(|g| g.at_ft).filter(|v| v.is_finite()).collect();
    let lo = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min) - margin;
    let hi = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max) + margin;
    (!xs.is_empty() && !ys.is_empty()).then(|| ([lo(&xs), lo(&ys)], [hi(&xs), hi(&ys)]))
}

/// Build a sheet map directly from a fixture set's DXF data.
pub fn recognise_fixture_set(dxf_data: &[(String, Vec<u8>)]) -> SheetMap {
    let mut drawings: Vec<(String, Drawing)> = Vec::new();
    for (name, bytes) in dxf_data {
        match cadaddy_io::read_dxf(bytes) {
            Ok(d) => drawings.push((name.clone(), d)),
            Err(_) => continue,
        }
    }
    let refs: Vec<(String, &Drawing)> = drawings.iter().map(|(n, d)| (n.clone(), d)).collect();
    build_sheet_map(&refs)
}

// ---- Scoring ------------------------------------------------------------------------

/// Score for recognising one sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetScore {
    pub number_correct: bool,
    pub discipline_correct: bool,
    pub level_correct: bool,
    pub scale_correct: bool,
    /// Grid alignment error in feet (mean absolute error of grid intersection positions).
    pub grid_error_ft: Option<f64>,
    /// The largest of those errors, feet: the worst grid intersection.
    pub grid_max_error_ft: Option<f64>,
}

/// Score the recognised sheet map against the truth.
pub fn score_sheet_map(map: &SheetMap, truth_sheets: &[Sheet], truth_grid_x: &[GridLine], truth_grid_y: &[GridLine]) -> Vec<SheetScore> {
    let mut scores = Vec::new();

    for ts in truth_sheets {
        let found = map.sheets.iter().find(|s| s.number == ts.number);
        let mut sc = SheetScore::default();
        if let Some(f) = found {
            sc.number_correct = true;
            sc.discipline_correct = f.discipline == ts.discipline;
            sc.level_correct = f.level == ts.level;
            sc.scale_correct =
                ts.scale_text.as_ref().is_some_and(|st| f.scale_text.as_ref().is_some_and(|ft| ft == st)) || (f.scale - ts.scale).abs() < 0.01;

            if let Some(alignment) = &f.alignment {
                let mut errors = Vec::new();
                for gx in truth_grid_x {
                    for gy in truth_grid_y {
                        let building_pt = [gx.at_ft, gy.at_ft];
                        let model_pt = layout::to_model(ts, building_pt);
                        let back = building_from_model(alignment, model_pt);
                        let err = ((back[0] - building_pt[0]).powi(2) + (back[1] - building_pt[1]).powi(2)).sqrt();
                        errors.push(err);
                    }
                }
                if !errors.is_empty() {
                    sc.grid_error_ft = Some(errors.iter().sum::<f64>() / errors.len() as f64);
                    sc.grid_max_error_ft = Some(errors.iter().copied().fold(0.0, f64::max));
                }
            }
        }
        scores.push(sc);
    }

    scores
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet_text(d: &mut Drawing, at: [f64; 2], value: &str, height: f64) {
        let t = cadaddy_doc::Text {
            insert: cadaddy_doc::geom::Vec3::new(at[0], at[1], 0.0),
            align_pt: None,
            height,
            value: value.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: cadaddy_doc::HAlign::Left,
            valign: cadaddy_doc::VAlign::Baseline,
        };
        d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common::default(), EntityKind::Text(t)).unwrap();
    }

    /// A sheet of two floor plans whose titles come twice, as an imported PDF gives them: the
    /// hidden text and the OCR's reading of the stroked lettering, at one spot, different heights.
    /// Each title is one view, and each view's cell holds its plan.
    #[test]
    fn a_title_read_twice_at_one_spot_is_one_view() {
        let mut d = Drawing::default();
        for (y0, title) in [(0.0, "3RD FLOOR PLAN"), (600.0, "4TH FLOOR PLAN")] {
            sheet_text(&mut d, [200.0, y0 - 60.0], title, 12.9);
            sheet_text(&mut d, [200.0, y0 - 60.0], title, 18.0);
            for (i, room) in ["OFFICE", "LOBBY", "STORAGE"].iter().enumerate() {
                sheet_text(&mut d, [90.0 + i as f64 * 240.0, y0 + 180.0], room, 6.4);
                sheet_text(&mut d, [90.0 + i as f64 * 240.0, y0 + 180.0], room, 9.0);
            }
        }
        sheet_text(&mut d, [0.0, -120.0], "SCALE: 1/8\" = 1'-0\"", 6.0);
        sheet_text(&mut d, [900.0, -120.0], "A101", 12.0);
        let views = plan_views(&d);
        assert_eq!(views.len(), 2, "{views:?}");
        assert!(views.iter().all(|v| v.height == 18.0), "the larger copy titles the view: {views:?}");
        let cells = view_cells(&d);
        assert_eq!(cells.len(), 2, "{cells:?}");
        for (v, [_, y0, _, y1]) in &cells {
            let plan_mid = if v.levels == ["3"] { 180.0 } else { 780.0 };
            assert!(*y0 < plan_mid && plan_mid < *y1, "{} cell {y0}..{y1} misses its plan", v.title);
        }
        // Two views side by side, titled on one line, stay two.
        let mut d = Drawing::default();
        sheet_text(&mut d, [0.0, 0.0], "3RD FLOOR PLAN - AREA A", 18.0);
        sheet_text(&mut d, [900.0, 0.0], "3RD FLOOR PLAN - AREA B", 18.0);
        for i in 0..6 {
            sheet_text(&mut d, [f64::from(i) * 300.0, 200.0], "ROOM", 9.0);
        }
        assert_eq!(plan_views(&d).len(), 2);
    }

    /// The bubbles of `printed_sheets::detail_sheet`: two wall sections, each with `1` and `A`
    /// at its corner.
    fn detail_bubbles() -> Vec<(String, [f64; 2])> {
        let mut b = Vec::new();
        for at in [[0.0, 0.0], [420.0, 300.0]] {
            b.push(("1".to_string(), [at[0] + 60.0, at[1] + 240.0]));
            b.push(("A".to_string(), [at[0], at[1] + 60.0]));
        }
        b
    }

    #[test]
    fn detail_bubble_pairs_give_a_rotation_their_bubbles_reject() {
        let gx = vec![GridLine { label: "1".into(), at_ft: 0.0 }, GridLine { label: "2".into(), at_ft: 24.0 }];
        let gy = vec![GridLine { label: "A".into(), at_ft: 0.0 }, GridLine { label: "B".into(), at_ft: 30.0 }];
        let b = detail_bubbles();
        // The pairs alone would turn the sheet (the nonsense rotations seen on details): either
        // they don't agree on an axis and give none, or the bubbles then sitting off their
        // lines reject the rotation they give.
        if let Some(a) = compute_alignment_from_bubbles(&b, &gx, &gy) {
            assert!(a.rotation_deg.abs() > 5.0, "rotation {}", a.rotation_deg);
            assert!(!bubbles_on_lines(&a, &b, &gx, &gy));
        }
        // Each pair alone turns it.
        let one = |label: &str| b.iter().filter(|(l, _)| l == label).cloned().collect::<Vec<_>>();
        for label in ["1", "A"] {
            let a = compute_alignment_from_bubbles(&one(label), &gx, &gy);
            assert!(a.as_ref().is_none_or(|a| a.rotation_deg.abs() > 5.0), "{label}: {a:?}");
        }
    }

    fn square_grid() -> (Vec<GridLine>, Vec<GridLine>) {
        let gx = (0..4).map(|i| GridLine { label: (i + 1).to_string(), at_ft: f64::from(i) * 24.0 }).collect();
        let gy = ["A", "B", "C"].iter().enumerate().map(|(i, l)| GridLine { label: (*l).into(), at_ft: i as f64 * 30.0 }).collect();
        (gx, gy)
    }

    /// Both bubbles of every line of `square_grid`, drawn turned `deg` and shifted, each moved by
    /// `jitter` (model inches) one way or the other.
    fn turned_bubbles(deg: f64, jitter: f64) -> Vec<(String, [f64; 2])> {
        let (gx, gy) = square_grid();
        let mut b = Vec::new();
        let mut k = 0usize;
        let mut push = |label: &str, p: [f64; 2]| {
            let r = layout::rotate([p[0] * 12.0, p[1] * 12.0], deg);
            let sign = |n: usize| if n.is_multiple_of(2) { 1.0 } else { -1.0 };
            b.push((label.to_string(), [r[0] + 500.0 + sign(k) * jitter, r[1] - 300.0 + sign(k / 2) * jitter]));
            k += 1;
        };
        for g in &gx {
            push(&g.label, [g.at_ft, -10.0]);
            push(&g.label, [g.at_ft, 70.0]);
        }
        for g in &gy {
            push(&g.label, [-10.0, g.at_ft]);
            push(&g.label, [82.0, g.at_ft]);
        }
        b
    }

    #[test]
    fn rotation_is_an_axial_mean_turned_by_label_order() {
        let (gx, gy) = square_grid();
        // 0.01" at 1/8" = 1'-0" is 0.96 model inches.
        for deg in [0.0, 90.0, 180.0, -90.0, 2.5, -2.5, 177.5] {
            for jitter in [0.0, 0.96] {
                let a = compute_alignment_from_bubbles(&turned_bubbles(deg, jitter), &gx, &gy).expect("aligned");
                let mut off = (a.rotation_deg - deg) % 360.0;
                if off > 180.0 {
                    off -= 360.0;
                } else if off < -180.0 {
                    off += 360.0;
                }
                assert!(off.abs() < 0.1, "{deg}° jitter {jitter}: read {}", a.rotation_deg);
            }
        }
        // The pairs of a square sheet a hair off read square, not turned.
        assert!(pair_turn_deg(&turned_bubbles(0.0, 0.96)).is_some_and(|t| t.abs() < SQUARE_DEG));
        assert!(pair_turn_deg(&turned_bubbles(90.0, 0.96)).is_some_and(|t| t.abs() < SQUARE_DEG));
        assert!(pair_turn_deg(&turned_bubbles(2.5, 0.96)).is_some_and(|t| (t - 2.5).abs() < 0.1));
    }

    #[test]
    fn labels_ascend_in_natural_order() {
        let g = |labels: &[&str]| labels.iter().enumerate().map(|(i, l)| DetectedGrid { label: (*l).into(), at_in: i as f64 }).collect::<Vec<_>>();
        assert!(ascends(&g(&["1", "2", "10", "11"])));
        assert!(ascends(&g(&["A", "B", "Z", "AA"])));
        assert!(ascends(&g(&["1", "1.5", "2"])));
        assert!(!ascends(&g(&["C", "B", "A"])));
        assert!(!ascends(&g(&["4", "3", "2", "1"])));
    }
}
