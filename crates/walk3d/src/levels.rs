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
}

impl DatumKind {
    /// Whether the datum is a storey (IFC) and the top of the floor below it.
    pub fn is_storey(self) -> bool {
        self != DatumKind::Parapet
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
    None
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
    (kind != DatumKind::Parapet).then_some(name)
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
    /// The spec's level of `name`, matched exactly or by canonical name (`L2` finds `Level 2`).
    pub fn level_matching(&self, name: &str) -> Option<usize> {
        if let Some(i) = self.levels.iter().position(|l| l.name.eq_ignore_ascii_case(name)) {
            return Some(i);
        }
        let want = canonical_level(name)?.0;
        self.levels.iter().position(|l| canonical_level(&l.name).is_some_and(|(c, _)| c == want))
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

    /// Every level reaches the next level or storey datum above it (floor to floor); the top
    /// level keeps its height. Returns what changed.
    pub fn restack_heights(&mut self) -> Vec<String> {
        let tops: Vec<f64> = self
            .levels
            .iter()
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
        let mut out: Vec<(String, f64)> = self.levels.iter().map(|l| (l.name.clone(), l.elevation_ft)).collect();
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
