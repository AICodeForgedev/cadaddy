//! Levels read from the prints: the level names and elevations the building sections,
//! elevations and level markers print (`LEVEL 2 = 14'-0"`, `T.O. ROOF EL. 42'-6"`), the level each
//! sheet's title block names (`SECOND FLOOR MECHANICAL PLAN` is on `LEVEL 2`), and the shafts and
//! risers that must carry on through the levels between where they start and end.
//!
//! Names are compared by their canonical form ([`canonical_level`]): `L2`, `Level 02`, `LVL 2`,
//! `SECOND FLOOR` and `2ND FLR` are all `LEVEL 2`; a mechanical penthouse is `PENTHOUSE`.
//!
//! Elevations read for a level the spec has update it; the others (the roof, a parapet, a
//! penthouse drawn on another drawing) are kept as [`Datum`]s, which set the height of the floor
//! under them and become IFC storeys, but place nothing on the sheet.

use serde::{Deserialize, Serialize};

use crate::spec::BuildingSpec;

/// Most notes stacked, sheets linked and shaft marks matched (hostile drawings).
pub const MAX_NOTES: usize = 5_000;
/// Longest text read as a level note or a sheet title.
const MAX_TEXT: usize = 120;

/// What a level or datum is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatumKind {
    #[default]
    Floor,
    Basement,
    Mezzanine,
    Penthouse,
    Roof,
    /// The top of a parapet: a height, not a storey.
    Parapet,
    /// A reference height of the site (`SITE`, finished `GRADE`, a survey benchmark): never a
    /// storey, never a level a sheet attaches to.
    Datum,
}

impl DatumKind {
    /// Whether the datum is a storey (IFC) and the top of the floor below it.
    pub fn is_storey(self) -> bool {
        !matches!(self, DatumKind::Parapet | DatumKind::Datum)
    }

    fn is_floor(&self) -> bool {
        *self == DatumKind::Floor
    }
}

/// An elevation the prints give for a level the spec has no plan for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Datum {
    pub name: String,
    pub elevation_ft: f64,
    #[serde(default, skip_serializing_if = "DatumKind::is_floor")]
    pub kind: DatumKind,
    /// Where its elevation came from: `model: <note>`, `layout:<name>: <note>`, `levels.set`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

/// A sheet of the set and the level it shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetLink {
    /// `M-201`, `A2.01`.
    pub number: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// From the number's prefix: `M` mechanical, `P` plumbing, `FP`, `E`, `A`, `S`…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discipline: Option<String>,
    /// The level's name in the spec (or its canonical name when the spec has none); none = a
    /// sheet of no one level (cover, schedules, details).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// Where it was found: `layout:<name>`, `file:<name>`, `model`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Where the sheet sits in model space `[x0, y0, x1, y1]` (a print set imported side by
    /// side): what lies there is this sheet's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<[f64; 4]>,
    /// Every level this sheet covers (typical-floor plans); empty = only `level`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<String>,
}

/// A level name and elevation read from one note.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelNote {
    /// Canonical: `LEVEL 2`, `ROOF`, `PENTHOUSE`.
    pub name: String,
    pub kind: DatumKind,
    pub elevation_ft: f64,
    /// Where it was read: `model: <note text>`, `layout:<name>: <note text>`; empty when unknown.
    pub source: String,
}

fn words(s: &str) -> Vec<String> {
    s.to_ascii_uppercase().replace(['.', ',', '(', ')', ':', '=', '/', '_'], " ").replace('-', " ").split_whitespace().map(str::to_string).collect()
}

/// `1`–`99` from `2`, `02`, `2ND`, `SECOND`.
fn floor_number(w: &str) -> Option<u32> {
    const ORD: [&str; 12] = ["FIRST", "SECOND", "THIRD", "FOURTH", "FIFTH", "SIXTH", "SEVENTH", "EIGHTH", "NINTH", "TENTH", "ELEVENTH", "TWELFTH"];
    if let Some(i) = ORD.iter().position(|o| *o == w) {
        return u32::try_from(i + 1).ok();
    }
    let digits = w.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let suffix = &w[digits.len()..];
    if digits.is_empty() || digits.len() > 2 || !digits.chars().all(|c| c.is_ascii_digit()) || !matches!(suffix, "" | "ST" | "ND" | "RD" | "TH") {
        return None;
    }
    digits.parse().ok().filter(|n| *n >= 1)
}

/// The canonical name of a level as prints write it, and what it is: `L2`, `Level 02`, `LVL 2`,
/// `SECOND FLOOR`, `2ND FLR` → `LEVEL 2`; `GROUND FLOOR` → `LEVEL 1`; `BASEMENT`, `B2` (after
/// `LEVEL`) → `BASEMENT 2`; `MAIN ROOF`, `T.O. ROOF` → `ROOF`; `MECHANICAL PENTHOUSE` →
/// `PENTHOUSE`; `TOP OF PARAPET` → `PARAPET`. None when the words name no level.
pub fn canonical_level(name: &str) -> Option<(String, DatumKind)> {
    if name.len() > MAX_TEXT {
        return None;
    }
    let w = words(name);
    let has = |k: &str| w.iter().any(|x| x == k);
    if has("PARAPET") {
        return Some(("PARAPET".into(), DatumKind::Parapet));
    }
    if let Some(name) = site_datum(&w) {
        return Some((name.into(), DatumKind::Datum));
    }
    if has("PENTHOUSE") || has("MPH") {
        return Some(if has("ROOF") { ("PENTHOUSE ROOF".into(), DatumKind::Roof) } else { ("PENTHOUSE".into(), DatumKind::Penthouse) });
    }
    if has("ROOF") {
        let q = ["LOW", "HIGH", "UPPER", "LOWER"].into_iter().find(|q| has(q));
        return Some((q.map_or_else(|| "ROOF".to_string(), |q| format!("{q} ROOF")), DatumKind::Roof));
    }
    if has("MEZZANINE") || has("MEZZ") {
        return Some(("MEZZANINE".into(), DatumKind::Mezzanine));
    }
    if has("BASEMENT") || has("CELLAR") {
        let n = w.iter().find_map(|x| floor_number(x));
        return Some((n.filter(|n| *n > 1).map_or_else(|| "BASEMENT".to_string(), |n| format!("BASEMENT {n}")), DatumKind::Basement));
    }
    if (has("GROUND") || has("GRADE")) && (has("FLOOR") || has("LEVEL") || has("FLR") || w.len() == 1) {
        return Some(("LEVEL 1".into(), DatumKind::Floor));
    }
    for (i, x) in w.iter().enumerate() {
        let next = w.get(i + 1).map(String::as_str).unwrap_or_default();
        // LEVEL 2, LVL 02, LEV 2, L 2; LEVEL B1 is a basement.
        if matches!(x.as_str(), "LEVEL" | "LVL" | "LEV" | "L") {
            if let Some(n) = floor_number(next) {
                return Some((format!("LEVEL {n}"), DatumKind::Floor));
            }
            if let Some(n) = next.strip_prefix('B').and_then(floor_number) {
                return Some((if n > 1 { format!("BASEMENT {n}") } else { "BASEMENT".into() }, DatumKind::Basement));
            }
        }
        // L2, L02.
        if let Some(n) = x.strip_prefix('L').filter(|r| r.len() <= 2 && r.chars().all(|c| c.is_ascii_digit())).and_then(floor_number) {
            return Some((format!("LEVEL {n}"), DatumKind::Floor));
        }
        // SECOND FLOOR, 2ND FLR, 2 FLOOR.
        if matches!(next, "FLOOR" | "FLR" | "STORY" | "STOREY")
            && let Some(n) = floor_number(x)
        {
            return Some((format!("LEVEL {n}"), DatumKind::Floor));
        }
    }
    // Bare sheet codes: "3" → LEVEL 3, "R" → ROOF, "B1" → BASEMENT.
    if w.len() == 1 {
        let c = &w[0];
        if c == "R" {
            return Some(("ROOF".into(), DatumKind::Roof));
        }
        if let Some(b) = c.strip_prefix('B').filter(|r| r.is_empty() || r.chars().all(|c| c.is_ascii_digit())) {
            let n: u32 = if b.is_empty() { 1 } else { b.parse().ok()? };
            return Some((if n > 1 { format!("BASEMENT {n}") } else { "BASEMENT".into() }, DatumKind::Basement));
        }
        if let Some(n) = floor_number(c) {
            return Some((format!("LEVEL {n}"), DatumKind::Floor));
        }
    }
    None
}

/// The site datum the words name (`SITE`, `FINISHED GRADE`, `SURVEY BENCHMARK`, `DATUM`): a
/// reference height, never a storey. `GRADE LEVEL` / `GROUND FLOOR` are the first floor, not
/// a datum.
fn site_datum(w: &[String]) -> Option<&'static str> {
    let has = |k: &str| w.iter().any(|x| x == k);
    if has("FLOOR") || has("FLR") || has("LEVEL") || has("STORY") || has("STOREY") {
        return None;
    }
    if has("SITE") {
        Some("SITE")
    } else if has("GRADE") {
        Some("GRADE")
    } else if has("SURVEY") || has("BENCHMARK") || has("DATUM") || (has("BM") && w.len() <= 3) {
        Some("DATUM")
    } else {
        None
    }
}

/// Whether a level name or code is a site datum (`SITE`, `GRADE`, `SURVEY DATUM`), never a
/// storey: no sheet attaches to it and nothing is built on it.
pub fn is_datum_name(name: &str) -> bool {
    canonical_level(name).is_some_and(|(_, k)| k == DatumKind::Datum)
}

/// The first elevation in `s` and the byte range it covers: feet and inches (`14'-0"`,
/// `14' 6 1/2"`, `-12'-0"`, `+42'-6"`), decimal feet (`14.5'`, `14.5 FT`), or a bare decimal
/// right after `EL`, `ELEV`, `FFE` or `T.O.S.` (`EL 114.50`).
pub fn parse_elevation(s: &str) -> Option<(f64, usize, usize)> {
    let b = s.as_bytes();
    let n = b.len();
    let digits = |mut i: usize| {
        while i < n && b.get(i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
            i += 1;
        }
        i
    };
    let skip_ws = |mut i: usize| {
        while i < n && b.get(i).is_some_and(|c| *c == b' ') {
            i += 1;
        }
        i
    };
    let num = |a: usize, z: usize| s.get(a..z).and_then(|t| t.parse::<f64>().ok());
    let mut i = 0;
    while i < n {
        let c = b.get(i).copied().unwrap_or(0);
        let starts = c.is_ascii_digit() && (i == 0 || b.get(i - 1).is_some_and(|p| !p.is_ascii_alphanumeric() && *p != b'.'));
        if !starts {
            i += 1;
            continue;
        }
        let sign_at = if i > 0 && matches!(b.get(i - 1), Some(b'-' | b'+')) { i - 1 } else { i };
        // A '-' between a word and the number is a dash, not a sign (`LEVEL 2 - 14'-0"`).
        let neg = b.get(sign_at) == Some(&b'-') && (sign_at == 0 || b.get(sign_at - 1).is_some_and(|p| *p == b' ' || *p == b'(' || *p == b'='));
        let sign = if neg { -1.0 } else { 1.0 };
        let start = if sign_at < i && (neg || b.get(sign_at) == Some(&b'+')) { sign_at } else { i };
        let e = digits(i);
        let Some(whole) = num(i, e) else {
            i = e.max(i + 1);
            continue;
        };
        let after = skip_ws(e);
        if b.get(after) == Some(&b'\'') {
            // Feet, then maybe inches: 14'-0", 14' 6", 14'-6 1/2".
            let mut j = after + 1;
            let mut end = j;
            let mut inch = 0.0;
            let k = skip_ws(if b.get(j) == Some(&b'-') { j + 1 } else { j });
            if b.get(k).is_some_and(u8::is_ascii_digit) {
                j = digits(k);
                if let Some(v) = num(k, j) {
                    inch = v;
                    let mut m = skip_ws(j);
                    // A fraction: 6 1/2.
                    if b.get(m).is_some_and(u8::is_ascii_digit) {
                        let p = digits(m);
                        if b.get(p) == Some(&b'/') {
                            let q = digits(p + 1);
                            if let (Some(a), Some(d)) = (num(m, p), num(p + 1, q))
                                && d > 0.0
                            {
                                inch += a / d;
                                m = q;
                            }
                        }
                    }
                    let m2 = skip_ws(m);
                    if b.get(m2) == Some(&b'"') {
                        end = m2 + 1;
                    } else if b.get(m2) != Some(&b'\'') {
                        end = m;
                    }
                }
            }
            if !(0.0..12.0 + 1e-9).contains(&inch) {
                i = e;
                continue;
            }
            let v = sign * (whole + inch / 12.0);
            return v.is_finite().then_some((v, start, end));
        }
        let rest = s.get(after..).unwrap_or_default().to_ascii_uppercase();
        if rest.starts_with("FT") || rest.starts_with("FEET") {
            let end = after + if rest.starts_with("FEET") { 4 } else { 2 };
            return Some((sign * whole, start, end));
        }
        let before = s.get(..start).unwrap_or_default().to_ascii_uppercase();
        let before = before.trim_end_matches([' ', '.', ':', '=', '+']);
        let last = before.rsplit(' ').next().unwrap_or_default();
        if ["EL", "ELEV", "ELEVATION", "FFE", "FF", "T.O.S", "TOS"].contains(&last) && b.get(after) != Some(&b'"') {
            return Some((sign * whole, start, e));
        }
        i = e;
    }
    None
}

/// A level note: a level's name and its elevation in one text (`LEVEL 2 = 14'-0"`,
/// `SECOND FLOOR EL. 14'-0"`, `T.O. ROOF +42'-6"`, `MECH. PENTHOUSE 56'-0" FFE`). None when the
/// text has no level name or no elevation, or is longer than a note.
pub fn parse_level_note(text: &str) -> Option<LevelNote> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_TEXT {
        return None;
    }
    let (elevation_ft, a, z) = parse_elevation(text)?;
    if !(-1000.0..=5000.0).contains(&elevation_ft) {
        return None;
    }
    // The name is what's left: before the elevation (or after it: `14'-0" LEVEL 2`).
    let before = text.get(..a).unwrap_or_default();
    let after = text.get(z..).unwrap_or_default();
    let (name, kind) = canonical_level(before).or_else(|| canonical_level(after))?;
    // A note about something else that mentions a level (`BOT @ 10'-0" ABOVE LEVEL 2`) is no
    // level marker: only level words and elevation keywords may be around the elevation.
    const FILL: [&str; 22] = [
        "EL",
        "ELEV",
        "ELEVATION",
        "FFE",
        "FF",
        "F",
        "T",
        "O",
        "TOS",
        "TOP",
        "OF",
        "SLAB",
        "FIN",
        "FINISH",
        "FINISHED",
        "FLOOR",
        "FLR",
        "STRUCTURE",
        "STRUCT",
        "DECK",
        "AFF",
        "STEEL",
    ];
    let level_words = words(before).into_iter().chain(words(after));
    let stray = level_words.filter(|w| canonical_level(w).is_none() && floor_number(w).is_none() && !FILL.contains(&w.as_str())).filter(|w| {
        !matches!(
            w.as_str(),
            "LEVEL"
                | "LVL"
                | "LEV"
                | "L"
                | "GROUND"
                | "GRADE"
                | "MAIN"
                | "MECHANICAL"
                | "MECH"
                | "LOW"
                | "HIGH"
                | "UPPER"
                | "LOWER"
                | "BEARING"
                | "B"
                | "STORY"
        ) && !w.starts_with('B')
    });
    if stray.count() > 0 {
        return None;
    }
    Some(LevelNote { name, kind, elevation_ft, source: String::new() })
}

/// The level a sheet's title names (`LEVEL 2 MECHANICAL PLAN`, `SECOND FLOOR PLAN - PLUMBING`,
/// `ROOF PLAN`), canonical; none for a title of no one level.
pub fn level_of_title(title: &str) -> Option<String> {
    let (name, kind) = canonical_level(title)?;
    (kind.is_storey()).then_some(name)
}

/// A sheet number in `text` (`M-201`, `FP-101`, `A2.01`, `E101`) and its discipline (the letters).
pub fn parse_sheet_number(text: &str) -> Option<(String, String)> {
    let t = text.trim();
    if t.len() > MAX_TEXT {
        return None;
    }
    for w in t.split(|c: char| c.is_whitespace() || c == ',' || c == '_') {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let letters: String = w.chars().take_while(char::is_ascii_alphabetic).collect();
        let rest = w.get(letters.len()..).unwrap_or_default();
        let rest = rest.strip_prefix(['-', ' ']).unwrap_or(rest);
        let digits = rest.chars().filter(char::is_ascii_digit).count();
        let ok_letters = (1..=2).contains(&letters.len()) && letters.chars().all(|c| c.is_ascii_uppercase());
        let ok_rest = (3..=5).contains(&digits)
            && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
            && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
            && rest.matches('.').count() <= 1;
        if ok_letters && ok_rest {
            return Some((w.to_string(), letters));
        }
    }
    None
}

/// Notes stacked into levels: one per name, lowest first; when notes disagree on a name's
/// elevation the most common value wins (the first read on a tie) and the disagreement is
/// reported.
pub fn stack(notes: &[LevelNote]) -> (Vec<LevelNote>, Vec<String>) {
    // Per name: each value read, how often, and the first note that gave it.
    let mut by_name: Vec<(String, DatumKind, Vec<(f64, usize, String)>)> = Vec::new();
    for n in notes.iter().take(MAX_NOTES) {
        let entry = match by_name.iter_mut().find(|(m, _, _)| *m == n.name) {
            Some(e) => e,
            None => {
                by_name.push((n.name.clone(), n.kind, Vec::new()));
                let Some(e) = by_name.last_mut() else { continue };
                e
            }
        };
        match entry.2.iter_mut().find(|(v, _, _)| (v - n.elevation_ft).abs() < 1.0 / 96.0) {
            Some(v) => v.1 += 1,
            None => entry.2.push((n.elevation_ft, 1, n.source.clone())),
        }
    }
    let mut out = Vec::new();
    let mut conflicts = Vec::new();
    for (name, kind, values) in by_name {
        let mut best: Option<(f64, usize, &str)> = None;
        for (v, c, src) in &values {
            if best.is_none_or(|(_, bc, _)| *c > bc) {
                best = Some((*v, *c, src.as_str()));
            }
        }
        let Some((elevation_ft, _, source)) = best else { continue };
        if values.len() > 1 {
            let all: Vec<String> = values.iter().map(|(v, c, _)| format!("{} ({c}×)", crate::spec::feet(*v))).collect();
            conflicts.push(format!("{name}: the prints disagree ({}); took {}", all.join(", "), crate::spec::feet(elevation_ft)));
        }
        out.push(LevelNote { name, kind, elevation_ft, source: source.to_string() });
    }
    out.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    (out, conflicts)
}

/// The datum the elevations are measured from: 100' when the prints put the first floor at
/// 100'-0" (a common convention), else 0.
pub fn auto_datum(notes: &[LevelNote]) -> f64 {
    let first = notes.iter().find(|n| n.name == "LEVEL 1").map(|n| n.elevation_ft);
    if first.is_some_and(|e| (e - 100.0).abs() < 1e-6) { 100.0 } else { 0.0 }
}

/// Where a level sits in the building's order, from its canonical name ([`canonical_level`]):
/// basements below the floors, `LEVEL n` in number order, the main `ROOF` over every floor. None
/// for what has no fixed place (a mezzanine, a low roof, a penthouse, a parapet).
pub fn level_rank(name: &str) -> Option<i32> {
    let (c, kind) = canonical_level(name)?;
    let n = |s: &str| s.trim().parse::<i32>().ok();
    match kind {
        DatumKind::Floor => c.strip_prefix("LEVEL ").and_then(n).map(|v| v * 10),
        DatumKind::Basement => Some(-10 * c.strip_prefix("BASEMENT ").and_then(n).unwrap_or(1)),
        DatumKind::Roof if c == "ROOF" => Some(100_000),
        _ => None,
    }
}

/// The level code a print set's index uses (`1`, `2`, `B1`, `R`) as a canonical level rank.
pub fn code_rank(code: &str) -> Option<i32> {
    let c = code.trim().to_ascii_uppercase();
    if c == "R" {
        return Some(100_000);
    }
    if let Some(b) = c.strip_prefix('B') {
        return Some(-10 * if b.is_empty() { 1 } else { b.parse::<i32>().ok()? });
    }
    c.parse::<i32>().ok().filter(|v| *v >= 1).map(|v| v * 10)
}

/// Which readings keep the building's order (elevations rise with the level number): `items`
/// are (rank, elevation, weight); the kept set is the heaviest one in order, a rank-less item is
/// always kept. A reading that breaks the order against heavier ones is dropped. Returns one
/// flag per item.
pub fn in_order(items: &[(Option<i32>, f64, u32)]) -> Vec<bool> {
    let n = items.len().min(MAX_NOTES);
    let mut keep: Vec<bool> = items.iter().map(|it| it.0.is_none()).collect();
    // Ranked items by rank, then the heaviest chain with strictly rising elevations.
    let mut idx: Vec<usize> = (0..n).filter(|&i| items.get(i).is_some_and(|it| it.0.is_some() && it.1.is_finite())).collect();
    idx.sort_by_key(|&i| items.get(i).and_then(|it| it.0));
    let mut best: Vec<(u64, Option<usize>)> = Vec::with_capacity(idx.len());
    for (a, &i) in idx.iter().enumerate() {
        let Some(&(ri, ei, wi)) = items.get(i) else { break };
        let mut b = (u64::from(wi), None);
        for (c, &j) in idx.iter().enumerate().take(a) {
            let (Some(&(rj, ej, _)), Some(&(w, _))) = (items.get(j), best.get(c)) else { continue };
            if rj < ri && ej < ei - 1e-6 && w + u64::from(wi) > b.0 {
                b = (w + u64::from(wi), Some(c));
            }
        }
        best.push(b);
    }
    let mut at = best.iter().enumerate().max_by_key(|(_, b)| b.0).map(|(c, _)| c);
    while let Some(c) = at {
        if let Some(f) = idx.get(c).and_then(|&i| keep.get_mut(i)) {
            *f = true;
        }
        at = best.get(c).and_then(|b| b.1);
    }
    keep
}

/// Whether a sheet is a civil, site, survey or landscape sheet (by its discipline letters or its
/// title): its elevations are site spot elevations (a finished floor on the site datum, a curb, a
/// grade), never the building's levels.
pub fn site_sheet(discipline: &str, title: &str) -> bool {
    let d = discipline.trim().to_ascii_uppercase();
    if matches!(d.as_str(), "C" | "CS" | "CG" | "CU" | "L" | "LS" | "LP" | "V" | "SV") {
        return true;
    }
    let w = words(title);
    w.iter().any(|x| matches!(x.as_str(), "SITE" | "CIVIL" | "GRADING" | "SURVEY" | "TOPOGRAPHIC" | "LANDSCAPE" | "PAVING" | "DRAINAGE"))
}

/// A shaft, chase, riser or elevator mark on one level's plan, in building feet.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaftMark {
    pub level: String,
    pub name: String,
    pub at: [f64; 2],
    /// `RISER UP` (+1), `DN` (−1): the level above or below must carry it on.
    pub dir: i8,
}

/// A vertical run through the levels: where it is, the levels it's marked on, and the levels
/// between its ends (and above an `UP` / below a `DN` at its ends) where it isn't.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaftRun {
    pub name: String,
    pub at: [f64; 2],
    pub levels: Vec<String>,
    pub missing: Vec<String>,
}

/// What a shaft mark says: its kind and direction, or none for a text that isn't one.
pub fn shaft_mark(text: &str) -> Option<(String, i8)> {
    if text.len() > 40 {
        return None;
    }
    let w = words(text);
    let kind = ["SHAFT", "CHASE", "RISER", "ELEVATOR", "ELEV", "SHAFTWAY"].into_iter().find(|k| w.iter().any(|x| x == k))?;
    let kind = match kind {
        "ELEV" | "ELEVATOR" => "ELEVATOR",
        "SHAFTWAY" => "SHAFT",
        k => k,
    };
    let dir = if w.iter().any(|x| x == "UP") {
        1
    } else if w.iter().any(|x| x == "DN" || x == "DOWN") {
        -1
    } else {
        0
    };
    Some((kind.to_string(), dir))
}

/// Shaft marks matched through the levels (`order`: the levels lowest first): marks of the same
/// kind within `tol_ft` of each other in plan are one run.
pub fn shaft_runs(marks: &[ShaftMark], order: &[String], tol_ft: f64) -> Vec<ShaftRun> {
    let idx = |l: &str| order.iter().position(|o| o.eq_ignore_ascii_case(l));
    let kind = |m: &ShaftMark| shaft_mark(&m.name).map(|k| k.0).unwrap_or_default();
    let mut groups: Vec<Vec<&ShaftMark>> = Vec::new();
    for m in marks.iter().take(MAX_NOTES) {
        if idx(&m.level).is_none() {
            continue;
        }
        let near = |g: &Vec<&ShaftMark>| g.iter().any(|o| kind(o) == kind(m) && (o.at[0] - m.at[0]).hypot(o.at[1] - m.at[1]) <= tol_ft);
        match groups.iter_mut().find(|g| near(g)) {
            Some(g) => g.push(m),
            None => groups.push(vec![m]),
        }
    }
    let mut out = Vec::new();
    for g in groups {
        let mut on: Vec<usize> = g.iter().filter_map(|m| idx(&m.level)).collect();
        on.sort_unstable();
        on.dedup();
        let (Some(&lo), Some(&hi)) = (on.first(), on.last()) else { continue };
        let up = g.iter().any(|m| m.dir > 0 && idx(&m.level) == Some(hi));
        let down = g.iter().any(|m| m.dir < 0 && idx(&m.level) == Some(lo));
        let top = if up { (hi + 1).min(order.len().saturating_sub(1)) } else { hi };
        let bottom = if down { lo.saturating_sub(1) } else { lo };
        let missing: Vec<String> = (bottom..=top).filter(|i| !on.contains(i)).filter_map(|i| order.get(i).cloned()).collect();
        let n = g.len() as f64;
        let at = [g.iter().map(|m| m.at[0]).sum::<f64>() / n, g.iter().map(|m| m.at[1]).sum::<f64>() / n];
        let name = g.first().map(|m| m.name.clone()).unwrap_or_default();
        out.push(ShaftRun {
            name,
            at: [(at[0] * 100.0).round() / 100.0, (at[1] * 100.0).round() / 100.0],
            levels: on.iter().filter_map(|i| order.get(*i).cloned()).collect(),
            missing,
        });
    }
    out
}

impl BuildingSpec {
    /// The spec's level of `name`, matched exactly, by canonical name (`L2`/`2`/`SECOND FLOOR`
    /// find `Level 2`) or by sheet attribute (`Level.attr`).
    pub fn level_matching(&self, name: &str) -> Option<usize> {
        // 1. Exact case-insensitive match on the display name.
        if let Some(i) = self.levels.iter().position(|l| l.name.eq_ignore_ascii_case(name)) {
            return Some(i);
        }
        // 2. Canonical name match (both sides normalised).
        if let Some(want) = canonical_level(name).map(|(c, _)| c)
            && let Some(i) = self.levels.iter().position(|l| canonical_level(&l.name).is_some_and(|(c, _)| c == want))
        {
            return Some(i);
        }
        // 3. Sheet attribute match (e.g. attr "3" on a level named "Level 3").
        let trimmed = name.trim();
        if !trimmed.is_empty()
            && let Some(i) = self.levels.iter().position(|l| l.attr.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(trimmed)))
        {
            return Some(i);
        }
        None
    }

    /// Stacked level notes into the spec: a level it has takes the elevation; the others become
    /// (or update) datums. Then every level's height reaches the next level or storey datum up.
    /// Returns what changed, in words, then the approved values it kept (see
    /// [`BuildingSpec::apply_levels_keeping`]).
    pub fn apply_levels(&mut self, stacked: &[LevelNote]) -> Vec<String> {
        let (mut changes, kept) = self.apply_levels_keeping(stacked);
        changes.extend(kept);
        changes
    }

    /// Whether a person approved the level or datum `name` (`levels/<name>`, any case).
    pub fn level_approved(&self, name: &str) -> bool {
        let key = format!("levels/{name}");
        self.approved.iter().any(|a| a.eq_ignore_ascii_case(&key))
    }

    /// Record where a level's or datum's value came from (`sources["levels/<name>"]`); it is no
    /// longer a guess.
    pub fn set_level_source(&mut self, name: &str, source: &str) {
        let key = format!("levels/{name}");
        self.sources.retain(|k, _| !k.eq_ignore_ascii_case(&key));
        self.guess.retain(|k, _| !k.eq_ignore_ascii_case(&key));
        if !source.is_empty() {
            self.sources.insert(key, source.chars().take(200).collect());
        }
    }

    /// Where a level's or datum's value came from, when it is known.
    pub fn level_source(&self, name: &str) -> Option<&str> {
        let key = format!("levels/{name}");
        self.sources.iter().find(|(k, _)| k.eq_ignore_ascii_case(&key)).map(|(_, v)| v.as_str())
    }

    /// [`BuildingSpec::apply_levels`], with the approved levels and datums kept apart: their
    /// elevations never change, and each one the prints disagree with is reported (`Level 2:
    /// kept 10'-0" (approved); prints say 14'-0"`). Every value applied records its note as its
    /// source, and stops being a guess. Returns (changes, kept).
    pub fn apply_levels_keeping(&mut self, stacked: &[LevelNote]) -> (Vec<String>, Vec<String>) {
        let mut changes = Vec::new();
        let mut kept = Vec::new();
        let differs = |a: f64, b: f64| (a - b).abs() > 1e-6;
        let kept_line =
            |name: &str, have: f64, says: f64| format!("{name}: kept {} (approved); prints say {}", crate::spec::feet(have), crate::spec::feet(says));
        for n in stacked.iter().take(MAX_NOTES) {
            if let Some(i) = self.level_matching(&n.name) {
                let Some((name, have)) = self.levels.get(i).map(|l| (l.name.clone(), l.elevation_ft)) else { continue };
                if self.level_approved(&name) {
                    if differs(have, n.elevation_ft) {
                        kept.push(kept_line(&name, have, n.elevation_ft));
                    }
                    continue;
                }
                if let Some(l) = self.levels.get_mut(i)
                    && differs(have, n.elevation_ft)
                {
                    changes.push(format!("{name}: elevation {} → {}", crate::spec::feet(have), crate::spec::feet(n.elevation_ft)));
                    l.elevation_ft = n.elevation_ft;
                }
                self.set_level_source(&name, &n.source);
                continue;
            }
            let approved = self.datums.iter().find(|d| d.name.eq_ignore_ascii_case(&n.name)).is_some_and(|d| self.level_approved(&d.name));
            match self.datums.iter_mut().find(|d| d.name.eq_ignore_ascii_case(&n.name)) {
                Some(d) if approved => {
                    if differs(d.elevation_ft, n.elevation_ft) {
                        kept.push(kept_line(&d.name, d.elevation_ft, n.elevation_ft));
                    }
                }
                Some(d) => {
                    if differs(d.elevation_ft, n.elevation_ft) {
                        changes.push(format!("{}: elevation {} → {}", d.name, crate::spec::feet(d.elevation_ft), crate::spec::feet(n.elevation_ft)));
                        d.elevation_ft = n.elevation_ft;
                    }
                    d.kind = n.kind;
                    if !n.source.is_empty() {
                        d.source = n.source.chars().take(200).collect();
                    }
                }
                None => {
                    changes.push(format!("{}: {} (no plan on this drawing)", n.name, crate::spec::feet(n.elevation_ft)));
                    self.datums.push(Datum {
                        name: n.name.clone(),
                        elevation_ft: n.elevation_ft,
                        kind: n.kind,
                        source: n.source.chars().take(200).collect(),
                    });
                }
            }
        }
        self.datums.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
        changes.extend(self.restack_heights());
        (changes, kept)
    }

    /// The readings that keep the building in order once applied (elevations rise with the
    /// level number, the main roof over every floor), and a warning for each one dropped. The
    /// spec's own levels and datums the readings don't name count too: approved ones always
    /// win, others weigh less than a reading, so one stray note (a site spot elevation) can't
    /// lift a floor over the floors above it, while readings that agree with each other replace
    /// guessed values.
    ///
    /// `fixed` are values that win like approved ones (the print set's level table).
    pub fn readings_in_order(&self, stacked: &[LevelNote], fixed: &[LevelNote]) -> (Vec<LevelNote>, Vec<String>) {
        const APPROVED: u32 = 1_000;
        let mut items: Vec<(Option<i32>, f64, u32)> = Vec::new();
        let mut reading: Vec<Option<usize>> = Vec::new();
        let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b) || canonical_level(a).zip(canonical_level(b)).is_some_and(|(x, y)| x.0 == y.0);
        let named = |name: &str| stacked.iter().chain(fixed).any(|n| same(&n.name, name));
        for n in fixed.iter().take(MAX_NOTES) {
            items.push((level_rank(&n.name), n.elevation_ft, APPROVED));
            reading.push(None);
        }
        for l in &self.levels {
            let approved = self.level_approved(&l.name);
            if approved || !named(&l.name) {
                items.push((level_rank(&l.name), l.elevation_ft, if approved { APPROVED } else { 1 }));
                reading.push(None);
            }
        }
        for d in &self.datums {
            let approved = self.level_approved(&d.name);
            if approved || !named(&d.name) {
                items.push((level_rank(&d.name), d.elevation_ft, if approved { APPROVED } else { 1 }));
                reading.push(None);
            }
        }
        for (k, n) in stacked.iter().enumerate().take(MAX_NOTES) {
            let approved = self.level_matching(&n.name).and_then(|i| self.levels.get(i)).is_some_and(|l| self.level_approved(&l.name))
                || self.datums.iter().any(|d| d.name.eq_ignore_ascii_case(&n.name) && self.level_approved(&d.name));
            // An approved value stays as it is (reported as kept), so the reading is no item.
            if !approved {
                items.push((level_rank(&n.name), n.elevation_ft, 2));
                reading.push(Some(k));
            }
        }
        let keep = in_order(&items);
        let mut dropped: Vec<usize> = Vec::new();
        for (r, f) in reading.iter().zip(&keep) {
            if let (Some(k), false) = (r, f) {
                dropped.push(*k);
            }
        }
        let mut out = Vec::new();
        let mut warnings = Vec::new();
        for (k, n) in stacked.iter().enumerate().take(MAX_NOTES) {
            if dropped.contains(&k) {
                let src = if n.source.is_empty() { String::new() } else { format!(" ({})", n.source) };
                warnings.push(format!(
                    "{}: {} rejected{src}: it would break the levels' order (elevations rise with the level number)",
                    n.name,
                    crate::spec::feet(n.elevation_ft)
                ));
            } else {
                out.push(n.clone());
            }
        }
        (out, warnings)
    }

    /// Every level reaches the next level or storey datum above it (floor to floor); the top
    /// level keeps its height. Returns what changed.
    pub fn restack_heights(&mut self) -> Vec<String> {
        let tops: Vec<f64> = self
            .levels
            .iter()
            .filter(|l| !is_datum_name(&l.name))
            .map(|l| l.elevation_ft)
            .chain(self.datums.iter().filter(|d| d.kind.is_storey()).map(|d| d.elevation_ft))
            .filter(|e| e.is_finite())
            .collect();
        let mut changes = Vec::new();
        for l in &mut self.levels {
            let next = tops.iter().copied().filter(|t| *t > l.elevation_ft + 1e-6).fold(f64::INFINITY, f64::min);
            if next.is_finite() && (next - l.elevation_ft - l.height_ft).abs() > 1e-6 {
                let h = next - l.elevation_ft;
                changes.push(format!("{}: floor to floor {} → {}", l.name, crate::spec::feet(l.height_ft), crate::spec::feet(h)));
                l.height_ft = h;
                if l.ceiling_ft > h {
                    l.ceiling_ft = h;
                }
            }
        }
        changes
    }

    /// The building's storeys, lowest first: its levels and its storey datums (the roof, a
    /// penthouse drawn elsewhere), by name and elevation in feet.
    pub fn storeys(&self) -> Vec<(String, f64)> {
        let mut out: Vec<(String, f64)> = self.levels.iter().filter(|l| !is_datum_name(&l.name)).map(|l| (l.name.clone(), l.elevation_ft)).collect();
        for d in self.datums.iter().filter(|d| d.kind.is_storey()) {
            if !out.iter().any(|(n, _)| n.eq_ignore_ascii_case(&d.name)) {
                out.push((d.name.clone(), d.elevation_ft));
            }
        }
        out.retain(|(_, e)| e.is_finite());
        out.sort_by(|a, b| a.1.total_cmp(&b.1));
        out
    }

    /// Rename a level everywhere the spec names it (rooms, runs, columns, stairs, RFIs, sheets,
    /// approvals). Returns how many references changed, or an error when the name is taken.
    pub fn rename_level(&mut self, old: &str, new: &str) -> Result<usize, String> {
        let new = new.trim();
        if new.is_empty() || new.len() > 64 {
            return Err("a level needs a name of 1 to 64 characters".into());
        }
        let Some(i) = self.levels.iter().position(|l| l.name.eq_ignore_ascii_case(old)) else {
            return Err(format!("there is no level named {old}"));
        };
        if self.levels.iter().enumerate().any(|(j, l)| j != i && l.name.eq_ignore_ascii_case(new)) {
            return Err(format!("there is already a level named {new}"));
        }
        let old = self.levels.get(i).map(|l| l.name.clone()).unwrap_or_default();
        let mut v = serde_json::to_value(&*self).map_err(|e| e.to_string())?;
        let mut n = 0;
        rename_in(&mut v, &old, new, &mut n);
        let key = format!("levels/{old}");
        if let Some(a) = v.get_mut("approved").and_then(|a| a.as_array_mut()) {
            for x in a.iter_mut() {
                if x.as_str() == Some(key.as_str()) {
                    *x = serde_json::json!(format!("levels/{new}"));
                }
            }
        }
        if let Some(m) = v.get_mut("sources").and_then(|g| g.as_object_mut())
            && let Some(src) = m.remove(&key)
        {
            m.insert(format!("levels/{new}"), src);
        }
        if let Some(g) = v.get_mut("guess").and_then(|g| g.as_object_mut())
            && let Some(why) = g.remove(&key)
        {
            g.insert(format!("levels/{new}"), why);
        }
        let mut sp: BuildingSpec = serde_json::from_value(v).map_err(|e| e.to_string())?;
        if let Some(l) = sp.levels.get_mut(i) {
            l.name = new.to_string();
        }
        *self = sp;
        Ok(n)
    }
}

/// Rename `level` / `to_level` values equal to `old` (any case) in a spec's JSON.
fn rename_in(v: &mut serde_json::Value, old: &str, new: &str, n: &mut usize) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if matches!(k.as_str(), "level" | "to_level") && x.as_str().is_some_and(|s| s.eq_ignore_ascii_case(old)) {
                    *x = serde_json::json!(new);
                    *n += 1;
                } else {
                    rename_in(x, old, new, n);
                }
            }
        }
        serde_json::Value::Array(a) => {
            for x in a {
                rename_in(x, old, new, n);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(t: &str) -> Option<(String, f64)> {
        parse_level_note(t).map(|n| (n.name, (n.elevation_ft * 1e6).round() / 1e6))
    }

    #[test]
    fn approved_levels_and_datums_are_kept_and_read_values_carry_their_source() {
        let lvl = |name: &str, e: f64| crate::rules::Level {
            name: name.into(),
            elevation_ft: e,
            height_ft: 10.0,
            ceiling_ft: 8.0,
            region: None,
            base: None,
            attr: None,
            sheet: None,
        };
        let mut sp = BuildingSpec {
            levels: vec![lvl("Level 1", 0.0), lvl("Level 2", 10.0)],
            datums: vec![Datum { name: "ROOF".into(), elevation_ft: 30.0, kind: DatumKind::Roof, source: String::new() }],
            approved: vec!["LEVELS/Level 2".into(), "levels/roof".into()],
            ..BuildingSpec::default()
        };
        sp.guess.insert("levels/Level 1".into(), "drafted".into());
        let note = |name: &str, kind, e: f64| LevelNote { name: name.into(), kind, elevation_ft: e, source: format!("layout:A-301: {name} = {e}") };
        let (changes, kept) = sp.apply_levels_keeping(&[
            note("LEVEL 1", DatumKind::Floor, 1.0),
            note("LEVEL 2", DatumKind::Floor, 14.0),
            note("ROOF", DatumKind::Roof, 42.5),
            note("PARAPET", DatumKind::Parapet, 46.0),
        ]);
        assert_eq!(kept, ["Level 2: kept 10'-0\" (approved); prints say 14'-0\"", "ROOF: kept 30'-0\" (approved); prints say 42'-6\""]);
        assert_eq!(sp.levels[1].elevation_ft, 10.0);
        assert_eq!(sp.datums.iter().find(|d| d.name == "ROOF").map(|d| d.elevation_ft), Some(30.0));
        assert_eq!(sp.levels[0].elevation_ft, 1.0);
        assert!(changes.contains(&"Level 1: elevation 0'-0\" → 1'-0\"".to_string()), "{changes:?}");
        assert_eq!(sp.level_source("level 1"), Some("layout:A-301: LEVEL 1 = 1"));
        assert_eq!(sp.level_source("Level 2"), None);
        assert!(sp.guess.is_empty());
        assert_eq!(sp.datums.iter().find(|d| d.name == "PARAPET").map(|d| d.source.as_str()), Some("layout:A-301: PARAPET = 46"));
        // The source follows a rename.
        sp.rename_level("Level 1", "Ground").unwrap();
        assert_eq!(sp.level_source("Ground"), Some("layout:A-301: LEVEL 1 = 1"));
        assert_eq!(sp.level_source("Level 1"), None);
    }

    #[test]
    fn a_reading_that_breaks_the_levels_order_is_rejected() {
        let lvl = |name: &str, e: f64| crate::rules::Level {
            name: name.into(),
            elevation_ft: e,
            height_ft: 10.0,
            ceiling_ft: 8.0,
            region: None,
            base: None,
            attr: None,
            sheet: None,
        };
        let note =
            |name: &str, e: f64| LevelNote { name: name.into(), kind: DatumKind::Floor, elevation_ft: e, source: format!("model: {name} {e}") };
        let sp = BuildingSpec {
            levels: vec![lvl("Level 1", 0.0), lvl("Level 2", 12.0), lvl("Level 3", 24.0), lvl("Level 4", 36.0)],
            ..BuildingSpec::default()
        };
        // A site spot elevation read as the first floor would lift it over Levels 3 and 4.
        let (kept, warnings) = sp.readings_in_order(&[note("LEVEL 1", 147.0)], &[]);
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("LEVEL 1: 147'-0\" rejected (model: LEVEL 1 147)"), "{warnings:?}");
        // Readings that agree with each other replace guessed values, even far from them.
        let all = [note("LEVEL 1", 100.0), note("LEVEL 2", 114.0), note("LEVEL 3", 128.0), note("LEVEL 4", 142.0)];
        let (kept, warnings) = sp.readings_in_order(&all, &[]);
        assert_eq!(kept.len(), 4);
        assert!(warnings.is_empty());
        // Of the readings, the one out of order goes, the rest stay.
        let (kept, warnings) = sp.readings_in_order(&[note("LEVEL 2", 14.0), note("LEVEL 3", 9.0), note("LEVEL 4", 42.0)], &[]);
        assert_eq!(kept.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["LEVEL 2", "LEVEL 4"]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        // An approved level always wins over a reading.
        let mut ap = sp.clone();
        ap.approved.push("levels/Level 3".into());
        let (kept, _) = ap.readings_in_order(&[note("LEVEL 2", 30.0)], &[]);
        assert!(kept.is_empty());
        // So does a fixed value (the print set's level table).
        let (kept, _) = sp.readings_in_order(&[note("LEVEL 2", 30.0), note("LEVEL 4", 40.0)], &[note("LEVEL 3", 26.0)]);
        assert_eq!(kept.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["LEVEL 4"]);
        // The main roof sits over every floor; a low roof or a mezzanine has no fixed place.
        assert_eq!(level_rank("T.O. ROOF"), Some(100_000));
        assert_eq!(level_rank("LOW ROOF"), None);
        assert_eq!(level_rank("MEZZANINE"), None);
        assert!(level_rank("LEVEL B2") < level_rank("BASEMENT") && level_rank("BASEMENT") < level_rank("GROUND FLOOR"));
        assert_eq!(code_rank("3"), level_rank("THIRD FLOOR"));
        assert_eq!(code_rank("R"), level_rank("ROOF"));
        assert_eq!(code_rank("B2"), level_rank("BASEMENT 2"));
        assert_eq!(in_order(&[(Some(10), 0.0, 1), (None, -5.0, 1), (Some(20), f64::NAN, 2)]), [true, true, false]);
    }

    #[test]
    fn site_grade_and_survey_are_datums_never_storeys() {
        for (s, want) in
            [("SITE", "SITE"), ("FINISHED GRADE", "GRADE"), ("GRADE", "GRADE"), ("SURVEY BENCHMARK", "DATUM"), ("PROJECT DATUM", "DATUM")]
        {
            assert_eq!(canonical_level(s), Some((want.to_string(), DatumKind::Datum)), "{s}");
            assert!(is_datum_name(s), "{s}");
        }
        // The first floor is no datum, however it is spelled.
        for s in ["GRADE LEVEL", "GROUND FLOOR", "GROUND", "LEVEL 1"] {
            assert_eq!(canonical_level(s), Some(("LEVEL 1".to_string(), DatumKind::Floor)), "{s}");
            assert!(!is_datum_name(s), "{s}");
        }
        assert!(!DatumKind::Datum.is_storey());
        // A site plan's title names no level of the building.
        assert_eq!(level_of_title("ARCHITECTURAL SITE PLAN"), None);
        let note = parse_level_note("FINISHED GRADE EL. 98'-6\"").expect("a grade note");
        assert_eq!((note.name.as_str(), note.kind), ("GRADE", DatumKind::Datum));
        // A grade reading goes in as a datum: no storey, no floor's height changes.
        let lvl = |name: &str, e: f64| crate::rules::Level {
            name: name.into(),
            elevation_ft: e,
            height_ft: 12.0,
            ceiling_ft: 8.0,
            region: None,
            base: None,
            attr: None,
            sheet: None,
        };
        let mut sp = BuildingSpec { levels: vec![lvl("Level 1", 0.0), lvl("Level 2", 12.0), lvl("SITE", 38.79)], ..BuildingSpec::default() };
        sp.apply_levels(&[LevelNote { name: "GRADE".into(), kind: DatumKind::Datum, elevation_ft: 6.0, source: String::new() }]);
        assert_eq!(sp.datums.iter().map(|d| (d.name.as_str(), d.kind)).collect::<Vec<_>>(), [("GRADE", DatumKind::Datum)]);
        assert_eq!(sp.levels.first().map(|l| l.height_ft), Some(12.0), "a datum is no storey's top");
        let storeys: Vec<String> = sp.storeys().into_iter().map(|(n, _)| n).collect();
        assert_eq!(storeys, ["Level 1", "Level 2"], "neither GRADE nor a level named SITE is a storey");
    }

    #[test]
    fn civil_and_site_sheets_are_told_apart() {
        assert!(site_sheet("C", "GRADING AND DRAINAGE PLAN"));
        assert!(site_sheet("A", "ARCHITECTURAL SITE PLAN"));
        assert!(site_sheet("E", "ELECTRICAL SITE PLAN"));
        assert!(site_sheet("L", "PLANTING PLAN"));
        assert!(!site_sheet("A", "FIRST FLOOR PLAN"));
        assert!(!site_sheet("A", "BUILDING SECTIONS"));
        assert!(!site_sheet("", ""));
    }

    #[test]
    fn level_names_in_every_spelling() {
        for (s, want) in [
            ("LEVEL 2", "LEVEL 2"),
            ("Level 02", "LEVEL 2"),
            ("LVL 2", "LEVEL 2"),
            ("L2", "LEVEL 2"),
            ("SECOND FLOOR", "LEVEL 2"),
            ("2ND FLR", "LEVEL 2"),
            ("GROUND FLOOR", "LEVEL 1"),
            ("FIRST FLOOR PLAN", "LEVEL 1"),
            ("T.O. ROOF", "ROOF"),
            ("MAIN ROOF", "ROOF"),
            ("LOW ROOF", "LOW ROOF"),
            ("MECH. PENTHOUSE", "PENTHOUSE"),
            ("PENTHOUSE ROOF", "PENTHOUSE ROOF"),
            ("TOP OF PARAPET", "PARAPET"),
            ("BASEMENT", "BASEMENT"),
            ("LEVEL B2", "BASEMENT 2"),
            ("MEZZANINE", "MEZZANINE"),
        ] {
            assert_eq!(canonical_level(s).map(|c| c.0).as_deref(), Some(want), "{s}");
        }
        for s in ["MECHANICAL PLAN", "LEGEND", "L200", "LEVELING NOTES", "SCHEDULE", ""] {
            assert_eq!(canonical_level(s), None, "{s}");
        }
    }

    #[test]
    fn level_notes_as_sections_and_markers_print_them() {
        assert_eq!(note("LEVEL 2 = 14'-0\""), Some(("LEVEL 2".into(), 14.0)));
        assert_eq!(note("LEVEL 2 EL. 14'-6\""), Some(("LEVEL 2".into(), 14.5)));
        assert_eq!(note("SECOND FLOOR 14' 6 1/2\""), Some(("LEVEL 2".into(), ((14.0 + 6.5 / 12.0) * 1e6_f64).round() / 1e6)));
        assert_eq!(note("T.O. ROOF +42'-6\""), Some(("ROOF".into(), 42.5)));
        assert_eq!(note("MECH. PENTHOUSE 56'-0\" FFE"), Some(("PENTHOUSE".into(), 56.0)));
        assert_eq!(note("BASEMENT -12'-0\""), Some(("BASEMENT".into(), -12.0)));
        assert_eq!(note("LEVEL 1 - 0'-0\""), Some(("LEVEL 1".into(), 0.0)));
        assert_eq!(note("LEVEL 3 EL 128.50"), Some(("LEVEL 3".into(), 128.5)));
        assert_eq!(note("ROOF 42.5'"), Some(("ROOF".into(), 42.5)));
        assert_eq!(note("14'-0\" LEVEL 2"), Some(("LEVEL 2".into(), 14.0)));
        assert_eq!(note("T.O. PARAPET 46'-0\""), Some(("PARAPET".into(), 46.0)));
        // Not level markers.
        assert_eq!(note("LEVEL 2 MECHANICAL PLAN"), None);
        assert_eq!(note("24x12 BOT @ 10'-0\" ABOVE LEVEL 2"), None);
        assert_eq!(note("2\" CW"), None);
        assert_eq!(note("SCALE: 1/8\" = 1'-0\""), None);
        assert_eq!(note(&"LEVEL 2 = 14'-0\" ".repeat(20)), None);
    }

    #[test]
    fn elevations_parse_and_never_panic() {
        assert_eq!(parse_elevation("EL 114.50").map(|e| e.0), Some(114.5));
        assert_eq!(parse_elevation("12 FT").map(|e| e.0), Some(12.0));
        assert_eq!(parse_elevation("10'-14\""), None);
        for s in ["'", "-'", "1'-", "1'-1/", "1'-1 1/0\"", "EL", "EL -", "+", "9999999999999999999999'", "1.2.3'", "€14'-0\"", "1'\u{2033}"] {
            let _ = parse_elevation(s);
            let _ = parse_level_note(s);
        }
    }

    #[test]
    fn sheet_titles_and_numbers() {
        assert_eq!(level_of_title("LEVEL 2 MECHANICAL PLAN").as_deref(), Some("LEVEL 2"));
        assert_eq!(level_of_title("SECOND FLOOR PLAN - PLUMBING").as_deref(), Some("LEVEL 2"));
        assert_eq!(level_of_title("ROOF PLAN").as_deref(), Some("ROOF"));
        assert_eq!(level_of_title("MECHANICAL SCHEDULES"), None);
        assert_eq!(parse_sheet_number("M-201"), Some(("M-201".into(), "M".into())));
        assert_eq!(parse_sheet_number("FP-101 LEVEL 1 FIRE PROTECTION PLAN"), Some(("FP-101".into(), "FP".into())));
        assert_eq!(parse_sheet_number("A2.01"), Some(("A2.01".into(), "A".into())));
        assert_eq!(parse_sheet_number("E101"), Some(("E101".into(), "E".into())));
        assert_eq!(parse_sheet_number("LEVEL 2"), None);
        assert_eq!(parse_sheet_number("24x12"), None);
    }

    #[test]
    fn stacking_takes_the_common_value_and_reports_the_odd_one() {
        let n = |name: &str, e: f64| LevelNote { name: name.into(), kind: DatumKind::Floor, elevation_ft: e, source: String::new() };
        let (s, c) = stack(&[n("LEVEL 2", 14.0), n("LEVEL 1", 0.0), n("LEVEL 2", 14.0), n("LEVEL 2", 14.5), n("ROOF", 28.0)]);
        let got: Vec<(&str, f64)> = s.iter().map(|x| (x.name.as_str(), x.elevation_ft)).collect();
        assert_eq!(got, [("LEVEL 1", 0.0), ("LEVEL 2", 14.0), ("ROOF", 28.0)]);
        assert_eq!(c.len(), 1, "{c:?}");
        assert!(c[0].contains("LEVEL 2") && c[0].contains("14'-6\""), "{c:?}");
        assert_eq!(auto_datum(&[n("LEVEL 1", 100.0)]), 100.0);
        assert_eq!(auto_datum(&s), 0.0);
    }

    #[test]
    fn applying_updates_levels_adds_datums_and_restacks() {
        let mut sp: BuildingSpec = BuildingSpec::from_json(
            &serde_json::json!({"levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10}, {"name": "Level 2", "elevation_ft": 10, "height_ft": 10}]}).to_string(),
        )
        .unwrap();
        let notes = [
            LevelNote { name: "LEVEL 1".into(), kind: DatumKind::Floor, elevation_ft: 0.0, source: String::new() },
            LevelNote { name: "LEVEL 2".into(), kind: DatumKind::Floor, elevation_ft: 14.0, source: String::new() },
            LevelNote { name: "ROOF".into(), kind: DatumKind::Roof, elevation_ft: 27.5, source: String::new() },
            LevelNote { name: "PARAPET".into(), kind: DatumKind::Parapet, elevation_ft: 31.0, source: String::new() },
        ];
        let changes = sp.apply_levels(&notes);
        assert!(!changes.is_empty());
        let l: Vec<(f64, f64)> = sp.levels.iter().map(|l| (l.elevation_ft, l.height_ft)).collect();
        assert_eq!(l, [(0.0, 14.0), (14.0, 13.5)]);
        assert_eq!(sp.datums.len(), 2);
        let st: Vec<String> = sp.storeys().into_iter().map(|s| s.0).collect();
        assert_eq!(st, ["L1", "Level 2", "ROOF"]);
        // Round trip, and applying again changes nothing.
        let back = BuildingSpec::from_json(&sp.to_json()).unwrap();
        assert_eq!(back, sp);
        assert!(sp.apply_levels(&notes).is_empty());
    }

    #[test]
    fn renaming_a_level_follows_every_reference() {
        let mut sp = BuildingSpec::from_json(
            &serde_json::json!({
                "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}, {"name": "2", "elevation_ft": 12, "height_ft": 12}],
                "rooms": [{"name": "Office", "level": "2"}],
                "stairs": [{"name": "S1", "level": "1", "to_level": "2", "outline": [[0,0],[1,0],[1,1]], "runs": [{"from": [0, 0], "to": [0, 10], "treads": 10}]}],
                "approved": ["levels/2"]
            })
            .to_string(),
        )
        .unwrap();
        let n = sp.rename_level("2", "LEVEL 2").unwrap();
        assert!(n >= 2, "{n}");
        assert_eq!(sp.levels[1].name, "LEVEL 2");
        assert_eq!(sp.rooms[0].level.as_deref(), Some("LEVEL 2"));
        assert_eq!(sp.approved, ["levels/LEVEL 2"]);
        assert!(sp.rename_level("1", "level 2").is_err());
        assert!(sp.rename_level("9", "x").is_err());
        assert_eq!(sp.level_matching("L2"), Some(1));
    }

    #[test]
    fn canonical_level_bare_codes() {
        // Bare digits → LEVEL N.
        assert_eq!(canonical_level("3"), Some(("LEVEL 3".into(), DatumKind::Floor)));
        assert_eq!(canonical_level("1"), Some(("LEVEL 1".into(), DatumKind::Floor)));
        assert_eq!(canonical_level("12"), Some(("LEVEL 12".into(), DatumKind::Floor)));
        // Bare R → ROOF.
        assert_eq!(canonical_level("R"), Some(("ROOF".into(), DatumKind::Roof)));
        // Bare B, B1, B2 → BASEMENT.
        assert_eq!(canonical_level("B"), Some(("BASEMENT".into(), DatumKind::Basement)));
        assert_eq!(canonical_level("B1"), Some(("BASEMENT".into(), DatumKind::Basement)));
        assert_eq!(canonical_level("B2"), Some(("BASEMENT 2".into(), DatumKind::Basement)));
        // Existing forms still work.
        assert_eq!(canonical_level("Level 3").unwrap().0, "LEVEL 3");
        assert_eq!(canonical_level("SECOND FLOOR").unwrap().0, "LEVEL 2");
        assert_eq!(canonical_level("L2").unwrap().0, "LEVEL 2");
        assert_eq!(canonical_level("ROOF").unwrap().0, "ROOF");
        // Not a level.
        assert_eq!(canonical_level("OFFICE"), None);
        assert_eq!(canonical_level(""), None);
    }

    #[test]
    fn level_matching_accepts_all_forms() {
        let sp = BuildingSpec::from_json(
            &serde_json::json!({
                "levels": [
                    {"name": "Level 1", "elevation_ft": 0, "height_ft": 14, "attr": "1"},
                    {"name": "Level 2", "elevation_ft": 14, "height_ft": 14, "attr": "2"},
                    {"name": "Level 3", "elevation_ft": 28, "height_ft": 12, "attr": "3"},
                ],
                "datums": [{"name": "Roof", "elevation_ft": 40, "kind": "roof"}]
            })
            .to_string(),
        )
        .unwrap();
        // Exact.
        assert_eq!(sp.level_matching("Level 2"), Some(1));
        assert_eq!(sp.level_matching("level 2"), Some(1));
        // Canonical aliases.
        assert_eq!(sp.level_matching("L2"), Some(1));
        assert_eq!(sp.level_matching("LVL 2"), Some(1));
        assert_eq!(sp.level_matching("SECOND FLOOR"), Some(1));
        assert_eq!(sp.level_matching("2ND FLR"), Some(1));
        // Bare digit (sheet code).
        assert_eq!(sp.level_matching("2"), Some(1));
        assert_eq!(sp.level_matching("3"), Some(2));
        // Sheet attribute match.
        assert_eq!(sp.level_matching("1"), Some(0));
        // level() also works (delegates to level_matching).
        assert!(sp.level("3").is_some_and(|l| l.name == "Level 3"));
        assert!(sp.level("THIRD FLOOR").is_some_and(|l| l.name == "Level 3"));
        assert!(sp.level("L1").is_some_and(|l| l.name == "Level 1"));
        // Unknown.
        assert_eq!(sp.level_matching("9"), None);
        assert_eq!(sp.level_matching("OFFICE"), None);
    }

    #[test]
    fn shafts_must_carry_on_between_their_ends() {
        let order: Vec<String> = ["LEVEL 1", "LEVEL 2", "LEVEL 3", "LEVEL 4"].map(String::from).to_vec();
        let m =
            |l: &str, name: &str, x: f64| ShaftMark { level: l.into(), name: name.into(), at: [x, 10.0], dir: shaft_mark(name).map_or(0, |k| k.1) };
        let runs = shaft_runs(
            &[
                m("LEVEL 1", "SHAFT", 20.0),
                m("LEVEL 2", "DUCT SHAFT", 21.0),
                m("LEVEL 4", "SHAFT", 20.5),
                m("LEVEL 2", "RISER UP", 60.0),
                m("LEVEL 1", "ELEV. 1", 90.0),
                m("LEVEL 2", "ELEVATOR", 90.0),
            ],
            &order,
            4.0,
        );
        let shaft = runs.iter().find(|r| r.name.contains("SHAFT")).unwrap();
        assert_eq!(shaft.missing, ["LEVEL 3"]);
        let riser = runs.iter().find(|r| r.name.contains("RISER")).unwrap();
        assert_eq!(riser.missing, ["LEVEL 3"]);
        let elev = runs.iter().find(|r| r.name.contains("ELEV")).unwrap();
        assert!(elev.missing.is_empty());
        assert_eq!(shaft_mark("OFFICE"), None);
    }
}
