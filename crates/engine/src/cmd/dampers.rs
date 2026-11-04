//! Dampers (Q6182): the fire, smoke, combination fire/smoke, motorized, backdraft, volume and
//! ceiling radiation dampers the mechanical plans tag, as elements in their ducts, with the
//! electrical and fire alarm work they bring.
//!
//! - A damper tag is a short text on a mechanical plan or enlarged plan whose code the set's own
//!   legend or abbreviations define as a damper ([`legend`], [`meaning`]); a code the legend gives
//!   another meaning (`SD` = duct smoke detector) is no damper. A code no legend defines takes the
//!   rules data's meaning (`dampers.json`). Each reading says where its meaning came from.
//! - Each tag is put on its level's plan (the job's placer) and into its duct ([`place`]): the
//!   duct run its leader (or the tag itself) is nearest, within the rules' reach, at the nearest
//!   place that duct (or a duct beside it) crosses a wall, a rated wall first; sized to the duct,
//!   at its height. A tag with no duct in reach is a review item, never dropped. The same damper
//!   on a floor plan and an enlarged plan counts once, at the enlarged plan's position.
//! - Powered dampers (fire/smoke, smoke, motorized) take the circuit an electrical note names
//!   ([`circuit_notes`]: `FEED THE SMOKE DAMPERS FROM CIRCUIT LA-12`),
//!   else the panel schedule's circuit that names dampers; none named: a review item and an RFI.
//! - Dampers the fire alarm sequence releases or monitors ([`fa_actions`]) take a control (relay)
//!   module each, a monitor module each when monitored.
//! - A duct crossing a rated wall with no damper there ([`unprotected`]) is a code finding.
//!
//! Every value comes from the documents or from `dampers.json` (rules data with its source).

use std::collections::BTreeMap;

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use serde::Deserialize;

const RULES: &str = include_str!("dampers.json");

/// Tags read on one sheet, at most (hostile input).
const MAX_TAGS_PER_SHEET: usize = 2_000;
/// Legend rows read on one sheet, at most.
const MAX_LEGEND: usize = 2_000;
/// Texts looked at on one sheet, at most.
const MAX_TEXTS: usize = 200_000;
/// Segments a leader is followed through, at most.
const MAX_LEADER_HOPS: usize = 4;
/// Wall crossings looked at on one level, at most.
const MAX_CROSSINGS: usize = 50_000;

/// A code the rules data knows, and the kind of damper it is.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CodeRule {
    pub code: String,
    pub kind: String,
}

/// A kind of damper: its name, the words a legend names it by, and what it needs.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct KindRule {
    /// `fire_smoke`, `fire`, `smoke`, `motorized`, `backdraft`, `volume`, `ceiling_radiation`.
    pub kind: String,
    pub name: String,
    /// Groups of words: a meaning with every word of one group names this kind.
    pub words: Vec<Vec<String>>,
    /// Listed for a rated assembly (fire, smoke).
    pub rated: bool,
    /// Its actuator takes power.
    pub powered: bool,
    /// The fire alarm releases it.
    pub fire_alarm: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PowerRules {
    pub device: String,
    pub name: String,
    pub box_in: [f64; 3],
    pub whip_ft: f64,
    pub whip: String,
    pub conduit_in: f64,
    pub conductors: u32,
    pub wire: String,
    pub circuit_words: Vec<String>,
    pub power_words: Vec<String>,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FaRules {
    pub control: String,
    pub control_name: String,
    pub monitor: String,
    pub monitor_name: String,
    pub release_words: Vec<String>,
    pub monitor_words: Vec<String>,
    pub matrix_words: Vec<String>,
    pub box_in: [f64; 3],
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub source: String,
    pub codes: Vec<CodeRule>,
    pub kinds: Vec<KindRule>,
    pub damper_word: String,
    pub max_code_chars: usize,
    pub legend_column_rows: usize,
    pub duct_reach_ft: f64,
    pub leader_reach_ft: f64,
    pub wall_reach_ft: f64,
    pub dedupe_ft: f64,
    pub body_in: f64,
    pub default_size_in: [f64; 2],
    pub power: PowerRules,
    pub fire_alarm: FaRules,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// A kind by its name (`fire_smoke`).
pub(crate) fn kind_rule(kind: &str) -> Option<&'static KindRule> {
    rules().kinds.iter().find(|k| k.kind.eq_ignore_ascii_case(kind.trim()))
}

/// A text's words, upper case (split at spaces and punctuation: `FIRE/SMOKE` is two).
fn words(t: &str) -> Vec<String> {
    t.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(200).map(str::to_string).collect()
}

/// Whether `w` is the damper word or its plural (`DAMPERS`).
fn damper_word(w: &str) -> bool {
    let d = rules().damper_word.as_str();
    !d.is_empty() && (w == d || w.strip_prefix(d).is_some_and(|rest| rest == "S"))
}

/// The kind of damper a text names (`FIRE/SMOKE DAMPER`: `fire_smoke`): it says damper, and every
/// word of one of a kind's word groups (the rules' kinds in order: the combination before fire).
/// None for a text that names no damper (`DUCT SMOKE DETECTOR`).
pub(crate) fn kind_of_words(t: &str) -> Option<&'static KindRule> {
    let w = words(t);
    if !w.iter().any(|x| damper_word(x)) {
        return None;
    }
    rules().kinds.iter().find(|k| k.words.iter().any(|g| !g.is_empty() && g.iter().all(|x| w.iter().any(|y| y.eq_ignore_ascii_case(x)))))
}

/// Whether two kinds are the same for a note or a sequence that names one (a smoke damper's note
/// powers the combination fire/smoke dampers too, and the other way round).
pub(crate) fn same_duty(a: &str, b: &str) -> bool {
    let smokes = |k: &str| matches!(k, "smoke" | "fire_smoke");
    a == b || (smokes(a) && smokes(b))
}

/// A text that can be a code (`FSD`, `F/SD`): 2 to the rules' most letters, upper case.
fn code_like(v: &str) -> bool {
    let n = v.chars().count();
    let letters = v.chars().filter(char::is_ascii_uppercase).count();
    (2..=rules().max_code_chars.max(2) + 1).contains(&n) && letters >= 2 && v.chars().all(|c| c.is_ascii_uppercase() || c == '/')
}

/// A text that reads as a legend's meaning: words, no digits, not a code itself.
fn meaning_like(v: &str) -> bool {
    let t = v.trim();
    let letters = t.chars().filter(char::is_ascii_alphabetic).count();
    letters >= 4 && t.chars().count() <= 60 && !t.chars().any(|c| c.is_ascii_digit()) && !code_like(t)
}

/// Whether `code` abbreviates `meaning`: its first letter starts a word of it and its letters come
/// in the meaning's order (`FSD` FIRE/SMOKE DAMPER, `SD` DUCT SMOKE DETECTOR, `BDD`
/// BACKDRAFT DAMPER; not `FIRE` SMOKE DAMPERS, a schedule's cells side by side).
fn abbreviates(code: &str, meaning: &str) -> bool {
    let w = words(meaning);
    let letters: Vec<char> = code.chars().filter(char::is_ascii_alphabetic).map(|c| c.to_ascii_uppercase()).collect();
    let Some(first) = letters.first() else { return false };
    let Some(start) = w.iter().position(|x| x.starts_with(*first)) else { return false };
    let mut rest = w.iter().skip(start).flat_map(|x| x.chars()).skip(1);
    letters.iter().skip(1).all(|c| rest.any(|m| m == *c))
}

/// One row of a legend or abbreviations list: a code and what it means, on its sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LegendEntry {
    pub code: String,
    pub meaning: String,
    pub sheet: String,
    /// Where the code is printed (so the legend's own row isn't read as a tag).
    pub at: Vec2,
}

/// The legend and abbreviation rows of a sheet's texts: a code with its meaning just right of it
/// (on its line, give or take a line), in a column of such rows (the rules' rows at least besides
/// it: a tag beside a room name on a plan is no legend).
pub(crate) fn legend(texts: &[TextItem], sheet: &str) -> Vec<LegendEntry> {
    let flat: Vec<&TextItem> =
        texts.iter().take(MAX_TEXTS).filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.x.is_finite() && t.at.y.is_finite()).collect();
    let mut rows: Vec<(LegendEntry, f64)> = Vec::new();
    // A row printed as one text: the code, a gap, its meaning (`FSD    FIRE/SMOKE DAMPER`).
    for c in flat.iter() {
        let v = c.value.trim();
        let Some((code, rest)) = v.split_once(char::is_whitespace) else { continue };
        if code_like(code)
            && meaning_like(rest)
            && abbreviates(code, rest)
            && (rest.split_whitespace().count() >= 2 || rest.trim().chars().count() >= 6)
        {
            rows.push((LegendEntry { code: code.to_string(), meaning: rest.trim().to_string(), sheet: sheet.to_string(), at: c.at }, c.height));
        }
        if rows.len() >= MAX_LEGEND {
            break;
        }
    }
    for c in flat.iter().filter(|t| code_like(t.value.trim())) {
        let v = c.value.trim();
        let h = c.height;
        let end = c.at.x + 0.6 * h * v.chars().count() as f64;
        let best = flat
            .iter()
            .filter(|m| {
                m.at.x > end && m.at.x < c.at.x + 25.0 * h && (m.at.y - c.at.y).abs() <= 1.2 * h && meaning_like(&m.value) && abbreviates(v, &m.value)
            })
            .min_by(|a, b| {
                let k = |m: &&&TextItem| ((m.at.y - c.at.y).abs() / h).round() * 100.0 + (m.at.x - c.at.x) / h;
                k(a).total_cmp(&k(b))
            });
        if let Some(m) = best {
            rows.push((LegendEntry { code: v.to_string(), meaning: m.value.trim().to_string(), sheet: sheet.to_string(), at: c.at }, h));
        }
        if rows.len() >= MAX_LEGEND {
            break;
        }
    }
    let need = rules().legend_column_rows;
    rows.iter()
        .filter(|(r, h)| {
            rows.iter().filter(|(o, _)| !std::ptr::eq(o, r) && (o.at.x - r.at.x).abs() < 1.0 * h && (o.at.y - r.at.y).abs() < 30.0 * h).count()
                >= need
        })
        .map(|(r, _)| r.clone())
        .collect()
}

/// What a damper code means here.
#[derive(Clone, Debug)]
pub(crate) struct Meaning {
    /// The damper kind; none: the legend gives the code another meaning (not a damper).
    pub kind: Option<&'static KindRule>,
    pub source: String,
}

/// What `code` means on `sheet`: its own legend's row, else another sheet's of the set (`set` in
/// order of preference), else the rules data. None when no legend and no rule knows the code.
pub(crate) fn meaning(code: &str, sheet: &str, set: &[LegendEntry]) -> Option<Meaning> {
    let c = code.trim().to_ascii_uppercase();
    let row =
        set.iter().find(|e| e.sheet == sheet && e.code.eq_ignore_ascii_case(&c)).or_else(|| set.iter().find(|e| e.code.eq_ignore_ascii_case(&c)));
    if let Some(e) = row {
        return Some(Meaning { kind: kind_of_words(&e.meaning), source: format!("{} legend: {} = {}", e.sheet, e.code, e.meaning) });
    }
    let r = rules();
    let rule = r.codes.iter().find(|x| x.code.eq_ignore_ascii_case(&c))?;
    let kind = kind_rule(&rule.kind)?;
    Some(Meaning { kind: Some(kind), source: format!("rules data: {c} = {} ({})", kind.name, r.source) })
}

/// A tag's code and number (`FSD-2` → `FSD`, `2`; `FD` → `FD`, ``).
fn split_tag(v: &str) -> Option<(String, String)> {
    let t = v.trim().to_ascii_uppercase();
    let letters: String = t.chars().take_while(|c| c.is_ascii_uppercase() || *c == '/').collect();
    let rest = t.get(letters.len()..)?.trim_start_matches(['-', ' ']);
    (code_like(&letters) && rest.chars().count() <= 4 && rest.chars().all(|c| c.is_ascii_digit())).then(|| (letters, rest.to_string()))
}

/// A damper tag read on a sheet (drawing units of the sheet).
#[derive(Clone, Debug, Default)]
pub(crate) struct Tag {
    pub code: String,
    pub number: String,
    /// The tag's middle.
    pub at: Vec2,
    pub height: f64,
    /// Where its leader ends, when it has one.
    pub points_to: Option<Vec2>,
    /// In a circle or a closed outline (a hexagon) round it.
    pub enclosed: bool,
}

/// The damper tags on a sheet's texts: a text that is a code the set's legends or the rules
/// data know (`known`), not a legend row, with its leader followed through the sheet's straight
/// pieces `segs` and its enclosure from them or from `rounds` (circles: centre, radius).
pub(crate) fn tags(
    texts: &[TextItem],
    segs: &[(Vec2, Vec2)],
    rounds: &[(Vec2, f64)],
    rows: &[LegendEntry],
    known: &dyn Fn(&str) -> bool,
) -> Vec<Tag> {
    let mut out = Vec::new();
    for t in texts.iter().take(MAX_TEXTS).filter(|t| t.height > 0.0 && t.at.is_finite() && t.rotation.is_finite()) {
        if out.len() >= MAX_TAGS_PER_SHEET {
            break;
        }
        let Some((code, number)) = split_tag(&t.value) else { continue };
        if !known(&code) {
            continue;
        }
        let h = t.height;
        if rows.iter().any(|r| (r.at - t.at).len() < 0.5 * h) {
            continue;
        }
        let n = t.value.trim().chars().count() as f64;
        // Its middle, along its baseline and up from it (a tag may be printed turned).
        let (dir, up) = (Vec2::new(t.rotation.cos(), t.rotation.sin()), Vec2::new(-t.rotation.sin(), t.rotation.cos()));
        let c = t.at + dir * (0.3 * h * n) + up * (0.5 * h);
        let near = |p: Vec2, r: f64| (p - c).len() < r;
        let enclosure = segs.iter().filter(|(a, b)| near(*a, 3.0 * h) && near(*b, 3.0 * h) && (*a - *b).len() > 0.3 * h).count();
        let enclosed = enclosure >= 4 || rounds.iter().any(|(o, r)| (*o - c).len() < 0.8 * h && *r > 0.5 * h && *r < 4.0 * h);
        // The leader: a piece starting at the tag's edge and leaving it, followed through the
        // pieces joined to its far end.
        let start = segs
            .iter()
            .filter_map(|(a, b)| {
                let (da, db) = ((*a - c).len(), (*b - c).len());
                let (n0, far, dn, df) = if da <= db { (*a, *b, da, db) } else { (*b, *a, db, da) };
                (dn < 3.0 * h && df > 3.5 * h && (far - n0).len() < 30.0 * h).then_some((dn, far, n0))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let points_to = start.map(|(_, far, from)| {
            let (mut end, mut prev) = (far, from);
            for _ in 0..MAX_LEADER_HOPS {
                let next = segs.iter().find_map(|(a, b)| {
                    if (*a - end).len() < 0.1 * h && (*b - prev).len() > 0.1 * h && (*b - end).len() > 0.1 * h {
                        Some(*b)
                    } else if (*b - end).len() < 0.1 * h && (*a - prev).len() > 0.1 * h && (*a - end).len() > 0.1 * h {
                        Some(*a)
                    } else {
                        None
                    }
                });
                match next {
                    Some(n) if (n - c).len() > (end - c).len() && (n - c).len() < 40.0 * h => {
                        prev = end;
                        end = n;
                    }
                    _ => break,
                }
            }
            end
        });
        out.push(Tag { code, number, at: c, height: h, points_to, enclosed });
    }
    out
}

/// A duct run on a level's plan (the spec's run, plan units).
#[derive(Clone, Debug, Default)]
pub(crate) struct Duct {
    /// Its index in the spec's runs.
    pub run: usize,
    pub level: String,
    pub points: Vec<Vec2>,
    pub size_in: [f64; 2],
    pub bottoms_ft: Vec<f64>,
    pub system: String,
    pub sheet: String,
}

impl Duct {
    fn bottom(&self, seg: usize) -> f64 {
        self.bottoms_ft.get(seg).or(self.bottoms_ft.last()).copied().filter(|v| v.is_finite()).unwrap_or(9.0)
    }
}

/// A wall on a level's plan: its outline, how high it goes over its floor (none: to the deck)
/// and its type's fire rating with where that came from.
#[derive(Clone, Debug, Default)]
pub(crate) struct Wall {
    pub level: String,
    pub hull: Vec<Vec2>,
    pub top_ft: Option<f64>,
    pub wall_type: String,
    pub rating: Option<String>,
    pub rating_source: String,
}

/// A damper read off a sheet and put on its level's plan.
#[derive(Clone, Debug, Default)]
pub(crate) struct Found {
    pub sheet: String,
    /// Read on an enlarged plan (its position wins over a floor plan's).
    pub enlarged: bool,
    pub level: String,
    pub code: String,
    pub number: String,
    pub kind: String,
    pub kind_source: String,
    /// Where its leader points (else the tag), on the plan.
    pub at: Vec2,
    /// The tag itself, on the plan.
    pub tag_at: Vec2,
    pub enclosed: bool,
    /// The keynote beside it (number, text).
    pub keynote: Option<(String, String)>,
}

/// A damper in its duct.
#[derive(Clone, Debug, Default)]
pub(crate) struct Placed {
    pub found: Found,
    /// The duct it is in ([`Duct`] index in the list given).
    pub duct: usize,
    pub segment: usize,
    pub at: Vec2,
    /// The duct's direction there (unit).
    pub along: Vec2,
    pub size_in: [f64; 2],
    pub bottom_ft: f64,
    /// The wall it is set in ([`Wall`] index), none when no wall crosses the duct in reach.
    pub wall: Option<usize>,
    pub why: String,
    /// The other sheets showing the same damper (one count).
    pub also: Vec<String>,
}

fn same_level(a: &str, b: &str) -> bool {
    let key = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let (ka, kb) = (key(a), key(b));
    if ka.is_empty() || kb.is_empty() { a.trim().eq_ignore_ascii_case(b.trim()) } else { ka == kb }
}

/// The point of segment `a`–`b` nearest `p`, and its distance.
fn nearest_on(a: Vec2, b: Vec2, p: Vec2) -> (Vec2, f64) {
    let ab = b - a;
    let l2 = ab.x * ab.x + ab.y * ab.y;
    let t = if l2 > 1e-12 { (((p - a).x * ab.x + (p - a).y * ab.y) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let q = a + ab * t;
    (q, (p - q).len())
}

/// Where segments `a`–`b` and `c`–`d` cross: the parameter on the first.
fn cross_t(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<f64> {
    let r = b - a;
    let s = d - c;
    let den = r.x * s.y - r.y * s.x;
    if den.abs() < 1e-12 {
        return None;
    }
    let q = c - a;
    let t = (q.x * s.y - q.y * s.x) / den;
    let u = (q.x * r.y - q.y * r.x) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some(t)
}

/// A duct crossing a wall: the duct, its segment, the point (the middle of where it goes through)
/// and the wall.
#[derive(Clone, Debug)]
pub(crate) struct Crossing {
    pub duct: usize,
    pub segment: usize,
    pub at: Vec2,
    pub wall: usize,
}

/// Every place a duct of the list crosses a wall of its level, below the wall's top.
pub(crate) fn crossings(ducts: &[Duct], walls: &[Wall]) -> Vec<Crossing> {
    let mut out = Vec::new();
    for (di, d) in ducts.iter().enumerate() {
        for (si, w) in d.points.windows(2).enumerate() {
            let [a, b] = [w[0], w[1]];
            let lo = (a.x.min(b.x), a.y.min(b.y));
            let hi = (a.x.max(b.x), a.y.max(b.y));
            for (wi, wall) in walls.iter().enumerate() {
                if out.len() >= MAX_CROSSINGS {
                    return out;
                }
                if wall.hull.len() < 3 || !same_level(&wall.level, &d.level) {
                    continue;
                }
                if wall.top_ft.is_some_and(|t| d.bottom(si) >= t) {
                    continue;
                }
                let (wl, wh) = wall.hull.iter().fold(((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY)), |(l, h), p| {
                    ((l.0.min(p.x), l.1.min(p.y)), (h.0.max(p.x), h.1.max(p.y)))
                });
                if wh.0 < lo.0 || wl.0 > hi.0 || wh.1 < lo.1 || wl.1 > hi.1 {
                    continue;
                }
                let n = wall.hull.len();
                let ts: Vec<f64> = (0..n)
                    .filter_map(|i| {
                        let (p, q) = (*wall.hull.get(i)?, *wall.hull.get((i + 1) % n)?);
                        cross_t(a, b, p, q)
                    })
                    .collect();
                let (t0, t1) = ts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), t| (l.min(*t), h.max(*t)));
                let t = match ts.len() {
                    0 => continue,
                    1 => {
                        // One end inside the wall: the middle of the part inside.
                        if cadaddy_geom::point_in_polygon(&wall.hull, a) { t0 / 2.0 } else { (t0 + 1.0) / 2.0 }
                    }
                    _ => (t0 + t1) / 2.0,
                };
                out.push(Crossing { duct: di, segment: si, at: a + (b - a) * t, wall: wi });
            }
        }
    }
    out
}

/// The dampers in their ducts, and the tags put in none (each with why). `upf`: plan units per
/// foot. Same damper on two sheets (same level and kind, within the rules' distance): one, at
/// the enlarged plan's position.
pub(crate) fn place(found: Vec<Found>, ducts: &[Duct], walls: &[Wall], upf: f64) -> (Vec<Placed>, Vec<(Found, String)>) {
    let r = rules();
    let all = crossings(ducts, walls);
    let mut placed: Vec<Placed> = Vec::new();
    let mut none: Vec<(Found, String)> = Vec::new();
    let mut found = found;
    // Enlarged plans first: their dampers are kept, a floor plan's same one merged into them.
    found.sort_by_key(|f| !f.enlarged);
    for f in found {
        let nearest = |p: Vec2| {
            ducts
                .iter()
                .enumerate()
                .filter(|(_, d)| same_level(&d.level, &f.level))
                .flat_map(|(di, d)| d.points.windows(2).enumerate().map(move |(si, w)| (di, si, w[0], w[1], d.size_in[0])))
                .map(|(di, si, a, b, w)| {
                    let (q, dist) = nearest_on(a, b, p);
                    (di, si, q, (dist - w / 24.0 * upf).max(0.0))
                })
                .filter(|x| x.3 <= r.duct_reach_ft * upf)
                .min_by(|a, b| a.3.total_cmp(&b.3))
        };
        // The leader's end first (it touches the duct), else the tag beside it.
        let hit = nearest(f.at).filter(|x| x.3 <= r.leader_reach_ft.max(r.duct_reach_ft) * upf).or_else(|| nearest(f.tag_at));
        let Some((di, si, q, _)) = hit else {
            let why = format!("no duct within {} ft of the tag on its level's plan", r.duct_reach_ft);
            none.push((f, why));
            continue;
        };
        // The wall the duct goes through there: a rated one in reach first, else any.
        let reach = r.wall_reach_ft * upf;
        let in_reach: Vec<&Crossing> = all
            .iter()
            .filter(|c| same_level(ducts.get(c.duct).map(|d| d.level.as_str()).unwrap_or(""), &f.level) && (c.at - q).len() <= reach)
            .collect();
        let pick = |rated: bool| {
            in_reach
                .iter()
                .filter(|c| !rated || walls.get(c.wall).is_some_and(|w| w.rating.is_some()))
                .min_by(|a, b| {
                    ((a.at - q).len() + if a.duct == di { 0.0 } else { 0.5 * upf })
                        .total_cmp(&((b.at - q).len() + if b.duct == di { 0.0 } else { 0.5 * upf }))
                })
                .copied()
        };
        let wants_rated = kind_rule(&f.kind).is_some_and(|k| k.rated);
        let cross = if wants_rated { pick(true).or_else(|| pick(false)) } else { pick(false) };
        let (duct, segment, at, wall, why) = match cross {
            Some(c) => (c.duct, c.segment, c.at, Some(c.wall), String::new()),
            None => (di, si, q, None, format!("no wall crosses the duct within {} ft: set where the tag points", r.wall_reach_ft)),
        };
        let Some(d) = ducts.get(duct) else { continue };
        let along = match d.points.get(segment).zip(d.points.get(segment + 1)) {
            Some((a, b)) if (*b - *a).len() > 1e-9 => (*b - *a) * (1.0 / (*b - *a).len()),
            _ => Vec2::new(1.0, 0.0),
        };
        let size_in = if d.size_in[0] > 0.0 { d.size_in } else { r.default_size_in };
        // Merged into one kept already: the same kind at the same place.
        if let Some(k) =
            placed.iter_mut().find(|p| same_level(&p.found.level, &f.level) && p.found.kind == f.kind && (p.at - at).len() < r.dedupe_ft * upf)
        {
            if !k.also.contains(&f.sheet) && k.found.sheet != f.sheet {
                k.also.push(f.sheet.clone());
            }
            if k.found.keynote.is_none() {
                k.found.keynote = f.keynote.clone();
            }
            continue;
        }
        placed.push(Placed { bottom_ft: d.bottom(segment), found: f, duct, segment, at, along, size_in, wall, why, also: Vec::new() });
    }
    (placed, none)
}

/// The places a duct crosses a rated wall with no damper of `placed` there: one per duct and wall,
/// and one per spot (duct pieces drawn over each other, a wall in two pieces: the rules' dedupe
/// distance).
pub(crate) fn unprotected<'a>(ducts: &[Duct], walls: &[Wall], placed: &[Placed], upf: f64, all: &'a [Crossing]) -> Vec<&'a Crossing> {
    let r = rules();
    let reach = r.wall_reach_ft * upf;
    let level = |c: &Crossing| ducts.get(c.duct).map(|d| d.level.clone()).unwrap_or_default();
    let mut seen: std::collections::BTreeSet<(usize, usize)> = std::collections::BTreeSet::new();
    let mut out: Vec<&Crossing> = Vec::new();
    for c in all.iter().filter(|c| walls.get(c.wall).is_some_and(|w| w.rating.is_some())) {
        let l = level(c);
        if placed.iter().any(|p| same_level(&p.found.level, &l) && (p.at - c.at).len() <= reach) {
            continue;
        }
        if out.iter().any(|o| same_level(&level(o), &l) && (o.at - c.at).len() < r.dedupe_ft * upf) {
            continue;
        }
        if seen.insert((c.duct, c.wall)) {
            out.push(c);
        }
    }
    out
}

/// A note naming the circuit powered dampers go on.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CircuitNote {
    /// The kind of damper it names.
    pub kind: String,
    /// The panel (empty: the note names none) and the circuit number.
    pub panel: String,
    pub circuit: String,
    pub sheet: String,
    /// The level whose plan it is on (empty: a sheet of no level).
    pub level: String,
    pub text: String,
    pub source: String,
}

fn clean(tok: &str) -> &str {
    tok.trim_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '(' | ')' | '"'))
}

/// The panel and circuit a note names after its circuit word: `CIRCUIT H-31` (panel H, 31),
/// `CKT 12` with `PANEL LA` elsewhere in it, `PANEL 2HA CIRCUIT 5,7`.
fn circuit_in(text: &str, panels: &[String]) -> Option<(String, String)> {
    let p = &rules().power;
    let toks: Vec<&str> = text.split_whitespace().take(400).collect();
    let up = |t: &str| clean(t).to_ascii_uppercase();
    let is_number =
        |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == ',') && t.chars().next().is_some_and(|c| c.is_ascii_digit());
    let panel_word = toks.iter().position(|t| up(t) == "PANEL" || up(t) == "PANELBOARD").and_then(|i| toks.get(i + 1)).map(|t| up(t));
    let at = toks.iter().rposition(|t| p.circuit_words.iter().any(|w| w.eq_ignore_ascii_case(clean(t))))?;
    for t in toks.iter().skip(at + 1).take(4).map(|t| up(t)) {
        if !t.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Some((pa, n)) = t.rsplit_once('-')
            && is_number(n)
            && !pa.is_empty()
            && pa.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return Some((pa.to_string(), n.to_string()));
        }
        if is_number(&t) {
            // The panel the note names, else one the schedules know written just before the word.
            let before = at.checked_sub(1).and_then(|i| toks.get(i)).map(|t| up(t)).filter(|b| panels.iter().any(|x| x.eq_ignore_ascii_case(b)));
            return Some((panel_word.clone().or(before).unwrap_or_default(), t));
        }
        break;
    }
    None
}

/// The notes (`(sheet, level, text)`: an electrical sheet's keynotes and general notes) that put
/// dampers on a circuit: they name a damper kind, a power word and a circuit (`FEED THE SMOKE
/// DAMPERS FROM CIRCUIT LA-12`). `panels`: the panel
/// schedules' names, for a circuit written without its panel.
pub(crate) fn circuit_notes(notes: &[(String, String, String)], panels: &[String]) -> Vec<CircuitNote> {
    let p = &rules().power;
    let mut out = Vec::new();
    for (sheet, level, text) in notes.iter().take(20_000) {
        let w = words(text);
        if !w.iter().any(|x| p.power_words.iter().any(|pw| pw.eq_ignore_ascii_case(x))) {
            continue;
        }
        let Some(kind) = kind_of_words(text) else { continue };
        if !kind.powered {
            continue;
        }
        let Some((panel, circuit)) = circuit_in(text, panels) else { continue };
        let source = format!("{sheet} note: {}", text.trim());
        let n = CircuitNote {
            kind: kind.kind.clone(),
            panel,
            circuit,
            sheet: sheet.clone(),
            level: level.clone(),
            text: text.trim().to_string(),
            source,
        };
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// What the fire alarm does to dampers, as its sequence of operations (matrix) says.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FaAction {
    pub release: bool,
    pub monitor: bool,
    /// The kind it names; empty: every damper the fire alarm controls.
    pub kind: String,
    pub sheet: String,
    pub text: String,
}

/// The fire alarm sequence rows about dampers on `pages` (`(sheet, texts)`): a text naming
/// dampers with a release word (`CLOSE SMOKE DAMPERS`) or a monitor word, on a page
/// whose texts name a sequence or matrix.
pub(crate) fn fa_actions(pages: &[(String, Vec<TextItem>)]) -> Vec<FaAction> {
    let f = &rules().fire_alarm;
    let has = |w: &[String], list: &[String]| w.iter().any(|x| list.iter().any(|l| l.eq_ignore_ascii_case(x)));
    let mut out = Vec::new();
    for (sheet, texts) in pages {
        let matrix = texts.iter().take(MAX_TEXTS).any(|t| has(&words(&t.value), &f.matrix_words));
        if !matrix {
            continue;
        }
        for t in texts.iter().take(MAX_TEXTS) {
            if t.value.chars().count() > 120 {
                continue;
            }
            let w = words(&t.value);
            if !w.iter().any(|x| damper_word(x)) {
                continue;
            }
            let (release, monitor) = (has(&w, &f.release_words), has(&w, &f.monitor_words));
            if !release && !monitor {
                continue;
            }
            let a = FaAction {
                release,
                monitor,
                kind: kind_of_words(&t.value).map(|k| k.kind.clone()).unwrap_or_default(),
                sheet: sheet.clone(),
                text: t.value.trim().to_string(),
            };
            if !out.contains(&a) {
                out.push(a);
            }
            if out.len() >= 200 {
                return out;
            }
        }
    }
    out
}

/// The fire alarm modules a damper of `kind` takes from `actions`: (control, monitor), with the
/// sequence rows that call for them.
pub(crate) fn fa_modules(kind: &str, actions: &[FaAction]) -> (u32, u32, Vec<String>) {
    let Some(k) = kind_rule(kind) else { return (0, 0, Vec::new()) };
    let applies: Vec<&FaAction> = actions
        .iter()
        .filter(|a| if a.kind.is_empty() { k.fire_alarm } else { same_duty(&a.kind, kind) && kind_rule(&a.kind).is_some_and(|x| x.fire_alarm) })
        .collect();
    let control = u32::from(applies.iter().any(|a| a.release));
    let monitor = u32::from(applies.iter().any(|a| a.monitor));
    (control, monitor, applies.iter().map(|a| format!("{}: {}", a.sheet, a.text)).collect())
}

/// Counts by level and kind (`Level 2` → `fire_smoke` → n).
pub(crate) fn by_level_kind(placed: &[Placed]) -> BTreeMap<String, BTreeMap<String, usize>> {
    let mut m: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for p in placed {
        *m.entry(p.found.level.clone()).or_default().entry(p.found.kind.clone()).or_default() += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: 2.0, rotation: 0.0 }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Vec2> {
        vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]
    }

    /// A legend column of abbreviations (codes left, meanings right).
    fn legend_sheet(x: f64, y: f64, rows: &[(&str, &str)]) -> Vec<TextItem> {
        rows.iter().enumerate().flat_map(|(i, (c, m))| [t(c, x, y - 4.0 * i as f64), t(m, x + 12.0, y - 4.0 * i as f64)]).collect()
    }

    #[test]
    fn a_legend_defines_codes_and_overrides_the_rules() {
        let texts = legend_sheet(
            0.0,
            100.0,
            &[
                ("FSD", "FIRE/SMOKE DAMPER"),
                ("FD", "FIRE DAMPER"),
                ("SD", "DUCT SMOKE DETECTOR"),
                ("OBD", "OPPOSED BLADE DAMPER"),
                ("MD", "MOTOR OPERATED DAMPER"),
            ],
        );
        let rows = legend(&texts, "M-001");
        assert_eq!(rows.len(), 5, "{rows:?}");
        let k = |c: &str, sheet: &str| meaning(c, sheet, &rows).and_then(|m| m.kind).map(|k| k.kind.clone());
        assert_eq!(k("FSD", "M-101").as_deref(), Some("fire_smoke"));
        assert_eq!(k("FD", "M-101").as_deref(), Some("fire"));
        assert_eq!(k("MD", "M-101").as_deref(), Some("motorized"));
        assert_eq!(k("OBD", "M-101").as_deref(), Some("volume"));
        // The legend says SD is a smoke detector: no damper, whatever the rules data say.
        let sd = meaning("SD", "M-101", &rows);
        assert!(sd.as_ref().is_some_and(|m| m.kind.is_none() && m.source.contains("M-001 legend")), "{sd:?}");
        // A code no legend gives: the rules data, with their source.
        let bdd = meaning("BDD", "M-101", &[]);
        assert!(bdd.as_ref().is_some_and(|m| m.kind.is_some_and(|k| k.kind == "backdraft") && m.source.starts_with("rules data")), "{bdd:?}");
        assert!(meaning("XYZ", "M-101", &[]).is_none());
        assert!(abbreviates("SD", "DUCT SMOKE DETECTOR") && abbreviates("BDD", "BACKDRAFT DAMPER") && abbreviates("FSD", "FIRE/SMOKE DAMPER"));
        assert!(!abbreviates("FIRE", "SMOKE DAMPERS") && !abbreviates("CD", "REV DESCRIPTION DATE") && !abbreviates("PTAC", "SINGLE ROOM"));
        // Rows printed as one text each, in a column.
        let one = vec![t("FSD    FIRE/SMOKE DAMPER", 0.0, 50.0), t("SD  DUCT SMOKE DETECTOR", 0.0, 46.0), t("VD VOLUME DAMPER", 0.0, 42.0)];
        let rows1 = legend(&one, "M-001");
        assert_eq!(rows1.len(), 3, "{rows1:?}");
        assert!(meaning("SD", "M-001", &rows1).is_some_and(|m| m.kind.is_none()));
        // A lone tag beside a room name on a plan is no legend row.
        let plan = vec![t("FSD", 0.0, 0.0), t("OFFICE", 10.0, 0.0)];
        assert!(legend(&plan, "M-101").is_empty());
        // The sheet's own legend first.
        let own = vec![
            LegendEntry { code: "FD".into(), meaning: "FIRE DAMPER".into(), sheet: "M-001".into(), at: Vec2::ZERO },
            LegendEntry { code: "FD".into(), meaning: "FLOOR DRAIN DAMPER".into(), sheet: "M-102".into(), at: Vec2::ZERO },
        ];
        assert!(meaning("FD", "M-102", &own).is_some_and(|m| m.source.starts_with("M-102")));
    }

    #[test]
    fn tags_are_read_with_their_leaders_and_never_from_the_legend() {
        let rows =
            legend(&legend_sheet(500.0, 500.0, &[("FSD", "FIRE/SMOKE DAMPER"), ("FD", "FIRE DAMPER"), ("CRD", "CEILING RADIATION DAMPER")]), "M-002");
        let mut texts = legend_sheet(500.0, 500.0, &[("FSD", "FIRE/SMOKE DAMPER"), ("FD", "FIRE DAMPER"), ("CRD", "CEILING RADIATION DAMPER")]);
        texts.push(t("FSD", 100.0, 100.0));
        texts.push(t("FD-2", 200.0, 100.0));
        texts.push(t("AHU", 300.0, 100.0));
        // FSD's hexagon (6 short sides round it) and a bent leader down to the duct.
        let c = Vec2::new(101.8, 101.0);
        let hex: Vec<(Vec2, Vec2)> = (0..6)
            .map(|i| {
                let a = std::f64::consts::PI / 3.0 * i as f64;
                let b = a + std::f64::consts::PI / 3.0;
                (c + Vec2::new(4.0 * a.cos(), 4.0 * a.sin()), c + Vec2::new(4.0 * b.cos(), 4.0 * b.sin()))
            })
            .collect();
        let mut segs = hex;
        segs.push((Vec2::new(101.8, 96.5), Vec2::new(101.8, 80.0)));
        segs.push((Vec2::new(101.8, 80.0), Vec2::new(110.0, 70.0)));
        let known = |c: &str| meaning(c, "M-101", &rows).is_some_and(|m| m.kind.is_some());
        let tags = tags(&texts, &segs, &[], &rows, &known);
        assert_eq!(tags.len(), 2, "{tags:?}");
        let fsd = tags.iter().find(|t| t.code == "FSD").expect("fsd");
        assert!(fsd.enclosed);
        assert!(fsd.points_to.is_some_and(|p| (p - Vec2::new(110.0, 70.0)).len() < 1e-6), "{fsd:?}");
        let mut turned = texts.clone();
        turned.push(TextItem { value: "FSD".into(), at: Vec2::new(300.0, 300.0), height: 2.0, rotation: std::f64::consts::FRAC_PI_2 });
        assert_eq!(super::tags(&turned, &[], &[], &rows, &known).len(), 3, "a turned tag is read too");
        let fd = tags.iter().find(|t| t.code == "FD").expect("fd");
        assert_eq!(fd.number, "2");
        assert!(fd.points_to.is_none() && !fd.enclosed);
    }

    /// A level's plan at 12 units per foot: a supply duct along y = 100 through a 1 HR rated
    /// corridor wall at x = 240 and an unrated wall at x = 480; a return duct along x = 600
    /// through the rated wall's continuation.
    fn plan() -> (Vec<Duct>, Vec<Wall>) {
        let ducts = vec![
            Duct {
                run: 0,
                level: "Level 2".into(),
                points: vec![Vec2::new(0.0, 100.0), Vec2::new(720.0, 100.0)],
                size_in: [24.0, 12.0],
                bottoms_ft: vec![9.5],
                system: "SA".into(),
                sheet: "M-101".into(),
            },
            Duct {
                run: 1,
                level: "Level 2".into(),
                points: vec![Vec2::new(600.0, 300.0), Vec2::new(600.0, 500.0)],
                size_in: [12.0, 10.0],
                bottoms_ft: vec![10.0],
                system: "RA".into(),
                sheet: "M-101".into(),
            },
        ];
        let walls = vec![
            Wall {
                level: "Level 2".into(),
                hull: rect(236.0, 0.0, 244.0, 400.0),
                top_ft: None,
                wall_type: "1B".into(),
                rating: Some("1 HR".into()),
                rating_source: "A-001 wall type legend, tag 1B".into(),
            },
            Wall {
                level: "Level 2".into(),
                hull: rect(477.0, 0.0, 483.0, 400.0),
                top_ft: None,
                wall_type: "3A".into(),
                rating: None,
                rating_source: String::new(),
            },
            Wall {
                level: "Level 2".into(),
                hull: rect(500.0, 397.0, 900.0, 403.0),
                top_ft: None,
                wall_type: "1B".into(),
                rating: Some("1 HR".into()),
                rating_source: "A-001 wall type legend, tag 1B".into(),
            },
        ];
        (ducts, walls)
    }

    fn found(kind: &str, code: &str, sheet: &str, at: Vec2, enlarged: bool) -> Found {
        Found {
            sheet: sheet.into(),
            enlarged,
            level: "Level 2".into(),
            code: code.into(),
            kind: kind.into(),
            kind_source: "test".into(),
            at,
            tag_at: at,
            ..Found::default()
        }
    }

    /// Test (1): FSD and FD tags on ducts at a rated wall become dampers in the duct at the wall.
    #[test]
    fn fsd_and_fd_tags_become_dampers_in_the_duct_at_the_rated_wall() {
        let (ducts, walls) = plan();
        let tags = vec![
            // The FSD's leader ends on the supply duct a foot and a half off the rated wall.
            found("fire_smoke", "FSD", "M-101", Vec2::new(258.0, 104.0), false),
            // The FD's tag sits beside the return duct, near where it meets the rated wall.
            found("fire", "FD", "M-101", Vec2::new(630.0, 380.0), false),
            // A motorized damper near the unrated wall stays at that wall.
            found("motorized", "MD", "M-101", Vec2::new(470.0, 110.0), false),
        ];
        let (placed, none) = place(tags, &ducts, &walls, 12.0);
        assert!(none.is_empty(), "{none:?}");
        assert_eq!(placed.len(), 3);
        let fsd = placed.iter().find(|p| p.found.code == "FSD").expect("fsd");
        assert_eq!((fsd.duct, fsd.wall), (0, Some(0)));
        assert!((fsd.at - Vec2::new(240.0, 100.0)).len() < 1e-6, "in the duct at the wall: {:?}", fsd.at);
        assert_eq!(fsd.size_in, [24.0, 12.0]);
        assert!((fsd.bottom_ft - 9.5).abs() < 1e-9);
        assert!((fsd.along - Vec2::new(1.0, 0.0)).len() < 1e-9);
        let fd = placed.iter().find(|p| p.found.code == "FD").expect("fd");
        assert_eq!((fd.duct, fd.wall), (1, Some(2)));
        assert!((fd.at - Vec2::new(600.0, 400.0)).len() < 1e-6, "{:?}", fd.at);
        assert_eq!(fd.size_in, [12.0, 10.0]);
        let md = placed.iter().find(|p| p.found.code == "MD").expect("md");
        assert_eq!(md.wall, Some(1));
        let counts = by_level_kind(&placed);
        assert_eq!(counts.get("Level 2").and_then(|m| m.get("fire_smoke")), Some(&1));
    }

    /// Test (2): a tag with no duct in reach becomes a review item.
    #[test]
    fn a_tag_with_no_duct_in_reach_is_a_review_item() {
        let (ducts, walls) = plan();
        let far = found("fire_smoke", "FSD", "M-101", Vec2::new(300.0, 250.0), false);
        let other_level = Found { level: "Level 3".into(), ..found("fire", "FD", "M-102", Vec2::new(258.0, 104.0), false) };
        let (placed, none) = place(vec![far, other_level], &ducts, &walls, 12.0);
        assert!(placed.is_empty());
        assert_eq!(none.len(), 2);
        assert!(none.iter().all(|(_, why)| why.contains("no duct within")));
    }

    /// Test (3): the same damper on a floor plan and an enlarged plan counts once, at the
    /// enlarged plan's position.
    #[test]
    fn the_same_damper_on_a_floor_plan_and_an_enlarged_plan_counts_once() {
        let (ducts, walls) = plan();
        let floor = found("fire_smoke", "FSD", "M-101", Vec2::new(250.0, 108.0), false);
        let mut enlarged = found("fire_smoke", "FSD", "M-501", Vec2::new(236.0, 98.0), true);
        enlarged.keynote = Some(("4".into(), "FIRE DAMPER AT RATED PARTITION".into()));
        // An FD drawn at the same wall is another damper (another kind).
        let fd = found("fire", "FD", "M-101", Vec2::new(244.0, 103.0), false);
        let (placed, _) = place(vec![floor, enlarged, fd], &ducts, &walls, 12.0);
        let fsds: Vec<&Placed> = placed.iter().filter(|p| p.found.kind == "fire_smoke").collect();
        assert_eq!(fsds.len(), 1, "{placed:?}");
        let one = fsds.first().expect("one");
        assert_eq!(one.found.sheet, "M-501", "the enlarged plan's is kept");
        assert_eq!(one.also, vec!["M-101".to_string()]);
        assert!(one.found.keynote.is_some());
        assert_eq!(placed.len(), 2);
    }

    /// Test (4): an electrical keynote naming a damper circuit powers the fire/smoke dampers on
    /// that circuit.
    #[test]
    fn an_electrical_note_naming_a_damper_circuit_powers_the_fire_smoke_dampers() {
        let panels = vec!["LA".to_string(), "H".to_string()];
        let notes = vec![
            (
                "E-101".to_string(),
                "2".to_string(),
                "FURNISH 120V TO EACH COMBINATION FIRE/SMOKE DAMPER ACTUATOR, CONNECT TO CIRCUIT H-31.".to_string(),
            ),
            ("E-102".to_string(), "3".to_string(), "MOTORIZED DAMPER: FEED FROM PANEL LA CIRCUIT 12,14.".to_string()),
            ("E-102".to_string(), "3".to_string(), "CONNECT EXHAUST FAN TO CIRCUIT 5.".to_string()),
            ("E-103".to_string(), "".to_string(), "BACKDRAFT DAMPER BY DIVISION 23. NO POWER CIRCUIT REQUIRED.".to_string()),
            ("E-103".to_string(), "".to_string(), "SMOKE DAMPER ON LA CKT 9".to_string()),
        ];
        let got = circuit_notes(&notes, &panels);
        assert_eq!(got.len(), 3, "{got:?}");
        let fs = got.iter().find(|n| n.kind == "fire_smoke").expect("fire smoke");
        assert_eq!((fs.panel.as_str(), fs.circuit.as_str(), fs.sheet.as_str()), ("H", "31", "E-101"));
        let md = got.iter().find(|n| n.kind == "motorized").expect("motorized");
        assert_eq!((md.panel.as_str(), md.circuit.as_str()), ("LA", "12,14"));
        let sd = got.iter().find(|n| n.kind == "smoke").expect("smoke");
        assert_eq!((sd.panel.as_str(), sd.circuit.as_str()), ("LA", "9"));
        assert!(same_duty("smoke", "fire_smoke") && !same_duty("fire", "fire_smoke"));
        assert!(kind_rule("fire_smoke").is_some_and(|k| k.powered) && kind_rule("fire").is_some_and(|k| !k.powered));
    }

    /// Test (5): a fire alarm matrix "release dampers" row gives each such damper a module.
    #[test]
    fn a_fire_alarm_matrix_release_row_gives_each_damper_a_module() {
        let page =
            vec![t("SEQUENCE OF OPERATIONS", 0.0, 100.0), t("Close fire/smoke dampers on alarm", 0.0, 80.0), t("Activate Horn Strobes", 0.0, 76.0)];
        let note_only = vec![t("FIRE/SMOKE DAMPER BY MECHANICAL", 0.0, 10.0), t("CLOSE DAMPER", 0.0, 5.0)];
        let acts = fa_actions(&[("FA-101".into(), page), ("FA-109".into(), note_only)]);
        assert_eq!(acts.len(), 1, "{acts:?}");
        let a = acts.first().expect("a");
        assert!(a.release && !a.monitor && a.kind == "fire_smoke" && a.sheet == "FA-101");
        assert_eq!(fa_modules("fire_smoke", &acts).0, 1);
        assert_eq!(fa_modules("smoke", &acts).0, 1);
        assert_eq!(fa_modules("fire", &acts).0, 0, "a fire damper (fusible link) takes no module");
        assert_eq!(fa_modules("motorized", &acts).0, 0);
        let mon = fa_actions(&[("FA-102".into(), vec![t("FIRE ALARM MATRIX", 0.0, 9.0), t("MONITOR DAMPER POSITION", 0.0, 1.0)])]);
        assert_eq!(fa_modules("smoke", &mon), (0, 1, vec!["FA-102: MONITOR DAMPER POSITION".to_string()]));
    }

    /// Test (6): a duct through a rated wall with no damper is a finding (the code check).
    #[test]
    fn a_duct_through_a_rated_wall_with_no_damper_is_a_finding() {
        let (ducts, walls) = plan();
        let all = crossings(&ducts, &walls);
        assert_eq!(all.len(), 3, "{all:?}");
        let fsd = found("fire_smoke", "FSD", "M-101", Vec2::new(258.0, 104.0), false);
        let (placed, _) = place(vec![fsd], &ducts, &walls, 12.0);
        let open = unprotected(&ducts, &walls, &placed, 12.0, &all);
        // The return duct through the rated wall has no damper; the unrated wall needs none.
        assert_eq!(open.len(), 1);
        let c = open.first().expect("open");
        assert_eq!((c.duct, c.wall), (1, 2));
        let checks = vec![cadaddy_walk3d::codebook::MechCheck {
            id: "RA".into(),
            level: Some("Level 2".into()),
            location: Some("1 HR wall 1B".into()),
            has_damper: false,
            clearance_in: None,
        }];
        let findings = cadaddy_walk3d::codebook::starter_book().check_mechanical(&checks);
        assert!(findings.iter().any(|f| f.rule_id.contains("fire_damper")), "{findings:?}");
        // The same return duct drawn twice over itself: still one place.
        let mut twice = ducts.clone();
        if let Some(d) = ducts.get(1) {
            twice.push(Duct { run: 2, points: d.points.iter().map(|p| *p + Vec2::new(1.0, 0.0)).collect(), ..d.clone() });
        }
        let all2 = crossings(&twice, &walls);
        assert_eq!(unprotected(&twice, &walls, &placed, 12.0, &all2).len(), 1);
        // A duct over a wall that stops below it doesn't go through it.
        let mut low = walls.clone();
        if let Some(w) = low.get_mut(2) {
            w.top_ft = Some(9.0);
        }
        assert_eq!(crossings(&ducts, &low).len(), 2);
    }
}
