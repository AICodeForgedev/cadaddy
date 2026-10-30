//! Wall types from a partition type legend drawn as sections: each family's section
//! (`TYPE B PARTITION` under it) with its layers called out beside it on leaders, a schedule of
//! its marks under the title (`B6A | 5 1/2" | 1 HOUR | UL U327 | 50-54`), and the head condition
//! key a mark's last letter names (`A - PARTITION TO EXTEND TO UNDERSIDE OF DECK`).
//!
//! Each mark is a wall type with its layers per side: a callout `EACH SIDE` is on both faces, one
//! without is on the face its leader points at (left of the studs is side 1), and a face label
//! (`CORRIDOR SIDE`) names the face it is beside. So a wall the plans tag knows how much board is
//! on each face (two layers of 5/8", or 5/8" on a 1/2" resilient channel): the depth a device's
//! mud ring has to make up.

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{Assembly, AssemblyLayer, HeightRule, LayerSide, WallFrom, WallType};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::{CommandSpec, bad};
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("walls.legend", "Read Partition Type Legend", run_legend)
            .params("{region?: [x0, y0, x1, y1] (the legend sheet; default the whole drawing), sheet?: its number (for the source), apply?: bool (the wall types into the building spec, replacing those of the same names)} → {types: [{name, description, layers: [{side, material, thickness_in}], height, fire_rating, acoustic_rating, faces_in: [side 1, side 2] (board and channels each face, inches), notes}], report: [line per family], applied}")
            .noundo(),
    ]
}

fn run_legend(s: &mut Session, p: &Value) -> Result<Value> {
    let region = p.get("region").and_then(Value::as_array).and_then(|r| {
        let v: Vec<f64> = r.iter().filter_map(Value::as_f64).filter(|x| x.is_finite()).collect();
        (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
    });
    let sheet = p.get("sheet").and_then(Value::as_str).unwrap_or("legend").to_string();
    let lg = read_legend(s.doc()?, region, &sheet);
    let types: Vec<Value> = lg
        .types
        .iter()
        .map(|t| {
            let a = t.assembly.clone().unwrap_or_default();
            let face = |side| face_depth(t, side).map(|f| f.0);
            json!({
                "name": t.name,
                "description": a.description,
                "layers": a.layers.iter().map(|l| json!({"side": l.side, "material": l.material, "thickness_in": l.thickness_in})).collect::<Vec<_>>(),
                "height": a.height,
                "fire_rating": a.fire_rating,
                "acoustic_rating": a.acoustic_rating,
                "faces_in": [face(LayerSide::Side1), face(LayerSide::Side2)],
                "notes": a.notes,
            })
        })
        .collect();
    let mut applied = false;
    if p.get("apply").and_then(Value::as_bool).unwrap_or(false) && !lg.types.is_empty() {
        let (mut sp, _) = super::bspec::current(s, "walls.legend")?;
        sp.wall_types.retain(|w| !lg.types.iter().any(|t| t.name.eq_ignore_ascii_case(&w.name)));
        sp.wall_types.extend(lg.types.iter().cloned());
        s.doc_mut().map_err(|e| bad("walls.legend", e.to_string()))?.building_spec = Some(sp.to_json());
        applied = true;
    }
    Ok(json!({"types": types, "report": lg.report, "applied": applied}))
}

/// Most texts and leader segments looked at (hostile drawings).
const MAX_TEXTS: usize = 50_000;
const MAX_SEGS: usize = 200_000;

/// A mark: its family's letter, the framing depth digit(s), the head condition letter (`B6A`).
fn mark_parts(t: &str) -> Option<(char, char)> {
    let t = t.trim();
    let b = t.as_bytes();
    let n = b.len();
    let (first, last) = (b.first().copied()?, b.last().copied()?);
    let ok = (3..=4).contains(&n)
        && first.is_ascii_uppercase()
        && last.is_ascii_uppercase()
        && b.get(1..n.saturating_sub(1)).is_some_and(|m| m.iter().all(u8::is_ascii_digit));
    ok.then_some((first as char, last as char))
}

/// A family's title: `TYPE B PARTITION` → `B`.
fn family_title(t: &str) -> Option<char> {
    let u = t.trim().to_ascii_uppercase();
    let rest = u.strip_prefix("TYPE ")?.strip_suffix(" PARTITION")?.trim();
    let mut c = rest.chars();
    let letter = c.next().filter(char::is_ascii_uppercase)?;
    c.next().is_none().then_some(letter)
}

/// A head condition key line: `B - PARTITION TO EXTEND TO UNDERSIDE OF …` → (`B`, the words).
fn head_key(t: &str) -> Option<(char, String)> {
    let u = t.trim().to_ascii_uppercase();
    let mut c = u.chars();
    let letter = c.next().filter(char::is_ascii_uppercase)?;
    let rest = c.as_str().trim_start();
    let rest = rest.strip_prefix('-').or_else(|| rest.strip_prefix('\u{2013}'))?.trim();
    rest.starts_with("PARTITION").then(|| (letter, rest.to_string()))
}

/// The height a head condition's words give, and how far above the ceiling (inches) for one that
/// goes past it. None: a partial height wall (its elevations give it).
fn head_rule(words: &str) -> Option<(HeightRule, f64)> {
    let u = words.to_ascii_uppercase();
    if u.contains("PARTIAL") {
        None
    } else if u.contains("ABOVE") && u.contains("CEILING") {
        let past = inches_in(&u).first().copied().unwrap_or(6.0);
        Some((HeightRule::AboveCeiling, past))
    } else if u.contains("CEILING") {
        Some((HeightRule::Ceiling, 6.0))
    } else if u.contains("FULL") {
        Some((HeightRule::Full, 6.0))
    } else if u.contains("DECK") || u.contains("STRUCTURE") {
        Some((HeightRule::Deck, 6.0))
    } else {
        None
    }
}

/// Every inch dimension in a line (`5/8"`, `3 1/2"`, `3-1/2"`, `16"`), in order.
pub(crate) fn inches_in(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    for (k, c) in s.char_indices() {
        if c != '"' && c != '\u{2033}' {
            continue;
        }
        // Back over the number: digits, '/', '.', and one space or dash before a fraction.
        let mut j = k;
        let mut seps = 0;
        while let Some(p) = j.checked_sub(1).and_then(|i| b.get(i)).copied() {
            if p.is_ascii_digit() || p == b'/' || p == b'.' {
                j -= 1;
            } else if (p == b' ' || p == b'-')
                && seps == 0
                && j.checked_sub(2).and_then(|i| b.get(i)).is_some_and(u8::is_ascii_digit)
                && s.get(j..k).is_some_and(|r| r.contains('/'))
            {
                seps += 1;
                j -= 1;
            } else {
                break;
            }
        }
        let Some(num) = s.get(j..k).map(str::trim) else { continue };
        let frac = |f: &str| -> Option<f64> {
            let (a, b) = f.split_once('/')?;
            let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
            (b > 0.0).then(|| a / b)
        };
        let v = frac(num).or_else(|| {
            let (w, f) = num.split_once([' ', '-'])?;
            Some(w.parse::<f64>().ok()? + frac(f)?)
        });
        if let Some(v) = v.or_else(|| num.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0 && *v < 100.0) {
            out.push(v);
        }
    }
    out
}

/// A line of text on the legend.
#[derive(Clone, Debug)]
struct Line {
    s: String,
    at: Vec2,
    h: f64,
}

/// A callout: its lines joined, where it starts and its height.
#[derive(Clone, Debug)]
struct Callout {
    s: String,
    at: Vec2,
    h: f64,
}

/// What one callout says about the wall.
#[derive(Clone, Debug, PartialEq)]
enum Says {
    /// Layers on a face (outer first) and whether on both faces; marks it is for (`A5`, `A6`:
    /// those marks only, in place of the board on its face).
    Face { layers: Vec<AssemblyLayer>, both: bool, only: Vec<String> },
    /// The studs, masonry or cavity insulation.
    Core(AssemblyLayer),
    /// A face label: `CORRIDOR SIDE`.
    FaceName(String),
    /// What the wall is (`GUESTROOM DEMISING WALL`).
    Name(String),
}

fn layer(material: String, thickness_in: f64, category: &str) -> AssemblyLayer {
    AssemblyLayer { material, thickness_in, side: LayerSide::Core, category: category.into(), in_cavity: false, spacing_in: None }
}

/// The marks a callout is for: `… FOR PARTITION A5 & A6` → `A5`, `A6`.
fn only_for(u: &str) -> Vec<String> {
    let Some(rest) = u.find("FOR PARTITION").and_then(|at| u.get(at + "FOR PARTITION".len()..)) else { return Vec::new() };
    rest.trim_start_matches('S')
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| {
            (2..=4).contains(&w.len()) && w.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && w.chars().skip(1).any(|c| c.is_ascii_digit())
        })
        .map(str::to_string)
        .collect()
}

/// Read one callout.
fn says(text: &str) -> Option<Says> {
    let u = text.to_ascii_uppercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let first_in = inches_in(&u).first().copied();
    let both = u.contains("EACH SIDE") || u.contains("BOTH SIDES");
    if u.contains("GYP") || u.contains("GWB") || u.contains("GYPSUM") {
        let Some(th) = first_in else { return None };
        let count = if u.contains("(2)") || u.contains("2 LAYERS") || u.contains("TWO LAYERS") {
            2
        } else if u.contains("(3)") || u.contains("3 LAYERS") || u.contains("THREE LAYERS") {
            3
        } else {
            1
        };
        let kind = if u.contains("WATER RESISTANT") || u.contains("MOISTURE") {
            if u.contains("TYPE") && u.contains('X') { "water resistant Type X gypsum board" } else { "water resistant gypsum board" }
        } else if u.contains("TYPE-X") || u.contains("TYPE X") || u.contains("TYPE 'X'") || u.contains("TYPE \"X\"") {
            "Type X gypsum board"
        } else if u.contains("SHAFT") {
            "shaft liner"
        } else {
            "gypsum board"
        };
        let mut layers: Vec<AssemblyLayer> = (0..count).map(|_| layer(format!("{} {kind}", frac(th)), th, "board")).collect();
        // Board on channels (`… ON 7/8" FURRING CHANNELS`, `… ON 1/2" RESILIENT CHANNELS`): the
        // channel is between the board and the studs.
        if u.contains("CHANNEL") {
            let ch = inches_in(&u).get(1).copied().unwrap_or(if u.contains("RESILIENT") { 0.5 } else { 0.875 });
            let what = if u.contains("RESILIENT") { "resilient channel" } else { "furring channel" };
            layers.push(layer(format!("{} {what}", frac(ch)), ch, "furring"));
        }
        return Some(Says::Face { layers, both, only: only_for(&u) });
    }
    if u.contains("CHANNEL") || u.contains("FURRING") {
        let resilient = u.contains("RESILIENT");
        let th = first_in.filter(|v| *v <= 2.0).unwrap_or(if resilient { 0.5 } else { 0.875 });
        let what = if resilient { "resilient channel" } else { "furring channel" };
        return Some(Says::Face { layers: vec![layer(format!("{} {what}", frac(th)), th, "furring")], both, only: Vec::new() });
    }
    if u.contains("STUCCO") {
        return Some(Says::Face { layers: vec![layer("stucco".into(), first_in.unwrap_or(0.875), "finish")], both, only: Vec::new() });
    }
    if u.contains("SHEATHING") || u.contains("STRUCTURAL PANEL") {
        return Some(Says::Face {
            layers: vec![layer("wood structural panel sheathing".into(), first_in.unwrap_or(0.5), "sheathing")],
            both,
            only: Vec::new(),
        });
    }
    if u.contains("RIGID") && u.contains("INSUL") {
        // Continuous insulation outside the studs; its thickness is seldom printed (an R-value is).
        let mut l = layer("rigid insulation".into(), first_in.unwrap_or(1.0), "insulation");
        l.material =
            if first_in.is_some() { format!("{} rigid insulation", frac(l.thickness_in)) } else { "rigid insulation (thickness not given)".into() };
        return Some(Says::Face { layers: vec![l], both, only: Vec::new() });
    }
    if u.contains("STUD") {
        let lumber = nominal_lumber(&u);
        let mut l =
            layer(if u.contains("WOOD") || lumber.is_some() { "wood studs".into() } else { "metal studs".into() }, lumber.unwrap_or(0.0), "framing");
        l.spacing_in = u.find('@').and_then(|a| inches_in(&u[a..]).first().copied());
        return Some(Says::Core(l));
    }
    if u.contains("MASONRY") || u.contains("CMU") {
        return Some(Says::Core(layer("concrete masonry units".into(), 0.0, "masonry")));
    }
    if u.contains("BATT") || u.contains("INSULATION") || u.contains("GLASS FIBER") {
        let mut l = layer("sound batt insulation".into(), 0.0, "insulation");
        l.in_cavity = true;
        return Some(Says::Core(l));
    }
    let words: Vec<&str> = u.split(' ').collect();
    let first = words.first().copied().unwrap_or("");
    if (2..=3).contains(&words.len()) && words.last() == Some(&"SIDE") && !matches!(first, "EACH" | "BOTH" | "ONE" | "OTHER") {
        return Some(Says::FaceName(u));
    }
    if (u.ends_with(" WALL") || u.ends_with(" PARTITION")) && !u.starts_with("TYPE ") && words.len() <= 5 {
        return Some(Says::Name(u));
    }
    None
}

/// Sawn lumber's depth from its nominal size (`2X4` → 3 1/2", `2X6` → 5 1/2").
fn nominal_lumber(u: &str) -> Option<f64> {
    u.split(|c: char| !c.is_ascii_alphanumeric()).find_map(|w| {
        let (a, b) = w.split_once('X')?;
        let (a, b): (u32, u32) = (a.parse().ok()?, b.parse().ok()?);
        let depth = match b {
            4 => 3.5,
            6 => 5.5,
            8 => 7.25,
            10 => 9.25,
            12 => 11.25,
            _ => return None,
        };
        (1..=4).contains(&a).then_some(depth)
    })
}

/// A length in inches as a board callout writes it: `5/8"`, `1 1/4"`, `7/8"`.
pub(crate) fn frac(v: f64) -> String {
    let whole = v.trunc();
    let eighths = ((v - whole) * 16.0).round() as i64;
    let (mut n, mut d) = (eighths, 16i64);
    while n > 0 && n % 2 == 0 && d > 1 {
        n /= 2;
        d /= 2;
    }
    match (whole as i64, n) {
        (w, 0) => format!("{w}\""),
        (0, n) => format!("{n}/{d}\""),
        (w, n) => format!("{w} {n}/{d}\""),
    }
}

/// One mark's schedule row.
#[derive(Clone, Debug, Default, PartialEq)]
struct Row {
    mark: String,
    /// Framing depths (two for a double stud wall: `3 1/2" & 5 1/2"`).
    depths: Vec<f64>,
    fire: Option<String>,
    tested: Option<String>,
    stc: Option<String>,
    notes: Vec<String>,
}

fn row_of(mark: &str, cells: &[&str]) -> Row {
    let mut r = Row { mark: mark.to_string(), ..Row::default() };
    for c in cells.iter().map(|c| c.trim()).filter(|c| !c.is_empty() && *c != "-") {
        let u = c.to_ascii_uppercase();
        if r.depths.is_empty() && !inches_in(&u).is_empty() {
            r.depths = inches_in(&u);
        } else if u.contains("HOUR") || u.ends_with(" HR") {
            r.fire = u.split(|c: char| !c.is_ascii_digit()).find(|n| !n.is_empty()).map(|n| format!("{n} HR"));
        } else if u.split(' ').any(|w| w.len() >= 4 && w.starts_with('U') && w.chars().skip(1).all(|c| c.is_ascii_digit())) {
            r.tested = Some(u);
        } else if u.chars().all(|c| c.is_ascii_digit() || c == '-')
            && u.split('-').filter_map(|n| n.parse::<u32>().ok()).any(|n| (20..=90).contains(&n))
        {
            r.stc = Some(format!("STC {u}"));
        } else {
            r.notes.push(u);
        }
    }
    r
}

/// What a legend read gives: the wall types (a mark each) and what was read, per family.
#[derive(Clone, Debug, Default)]
pub(crate) struct Legend {
    pub types: Vec<WallType>,
    pub report: Vec<String>,
}

/// The straight segments of the drawing in `region` (leaders).
fn segments(d: &Drawing, inside: &dyn Fn(Vec2) -> bool) -> Vec<(Vec2, Vec2)> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        match &e.kind {
            EntityKind::Line(l) => {
                let (a, b) = (l.a.xy(), l.b.xy());
                if a.is_finite() && b.is_finite() && a.dist(b) > 1e-9 && (inside(a) || inside(b)) {
                    out.push((a, b));
                }
            }
            EntityKind::LwPolyline(p) if !p.closed => {
                for w in p.vertices.windows(2) {
                    let (a, b) = (w[0].p, w[1].p);
                    if a.is_finite() && b.is_finite() && a.dist(b) > 1e-9 && (inside(a) || inside(b)) {
                        out.push((a, b));
                    }
                }
            }
            _ => {}
        }
        if out.len() >= MAX_SEGS {
            break;
        }
    }
    out
}

/// Where a callout's leader points: from a segment end at the callout's left (its first line's
/// middle), along the segments joined end to end, to the last end.
fn leader_tip(c: &Callout, segs: &[(Vec2, Vec2)]) -> Option<Vec2> {
    let start = Vec2::new(c.at.x, c.at.y + c.h * 0.5);
    let near = |p: Vec2, q: Vec2, r: f64| p.dist(q) <= r;
    let (mut i, mut end) = segs
        .iter()
        .enumerate()
        .filter_map(|(i, (a, b))| {
            // The leader starts just left of the text and runs away from it (leftwards or down).
            let (from, to) = if near(*a, start, 1.5 * c.h) {
                (*a, *b)
            } else if near(*b, start, 1.5 * c.h) {
                (*b, *a)
            } else {
                return None;
            };
            (to.x < from.x - 1e-9).then_some((i, to, from.dist(start)))
        })
        .min_by(|x, y| x.2.total_cmp(&y.2))
        .map(|(i, to, _)| (i, to))?;
    for _ in 0..4 {
        let next = segs.iter().enumerate().find_map(|(j, (a, b))| {
            if j == i {
                None
            } else if near(*a, end, 0.2 * c.h) {
                Some((j, *b))
            } else if near(*b, end, 0.2 * c.h) {
                Some((j, *a))
            } else {
                None
            }
        });
        let Some((j, to)) = next else { break };
        i = j;
        end = to;
    }
    Some(end)
}

/// Read the partition type legend(s) of the drawing (within `region`, `[x0, y0, x1, y1]`, when
/// given): each family's marks as wall types with their layers per side, ratings and head rule.
pub(crate) fn read_legend(d: &Drawing, region: Option<[f64; 4]>, sheet: &str) -> Legend {
    let inside = |p: Vec2| region.is_none_or(|[x0, y0, x1, y1]| p.x >= x0.min(x1) && p.x <= x0.max(x1) && p.y >= y0.min(y1) && p.y <= y0.max(y1));
    let texts: Vec<TextItem> = cadaddy_mepfix::sheet_facts::texts(d);
    let lines: Vec<Line> = texts
        .iter()
        .filter(|t| t.rotation.sin().abs() < 0.05 && t.rotation.cos() > 0.0 && inside(t.at))
        .take(MAX_TEXTS)
        .map(|t| Line { s: t.value.trim().to_string(), at: t.at, h: t.height })
        .collect();
    let mut out = Legend::default();
    let families: Vec<(char, &Line)> = lines.iter().filter_map(|l| Some((family_title(&l.s)?, l))).collect();
    if families.is_empty() {
        return out;
    }
    // The head condition key, with each key's words run on onto the lines under it.
    let mut heads: BTreeMap<char, (HeightRule, f64)> = BTreeMap::new();
    for l in &lines {
        let Some((letter, mut words)) = head_key(&l.s) else { continue };
        let mut y = l.at.y;
        while let Some(next) = lines
            .iter()
            .filter(|n| {
                n.at.y < y && y - n.at.y <= 1.8 * l.h && n.at.x >= l.at.x - 0.5 * l.h && n.at.x <= l.at.x + 4.0 * l.h && head_key(&n.s).is_none()
            })
            .max_by(|a, b| a.at.y.total_cmp(&b.at.y))
        {
            words.push(' ');
            words.push_str(&next.s.to_ascii_uppercase());
            y = next.at.y;
        }
        if let Some(rule) = head_rule(&words) {
            heads.entry(letter).or_insert(rule);
        }
    }
    // Each family's schedule rows: marks of its letter under its title, and the cells on each
    // mark's line to its right.
    let mut used = vec![false; lines.len()];
    let mut rows: BTreeMap<usize, Vec<Row>> = BTreeMap::new();
    for (k, l) in lines.iter().enumerate() {
        let Some((letter, _)) = mark_parts(&l.s) else { continue };
        let Some(fi) = families
            .iter()
            .enumerate()
            .filter(|(_, (fl, f))| {
                *fl == letter && f.at.y > l.at.y && f.at.y - l.at.y <= 14.0 * f.h && l.at.x >= f.at.x - 6.0 * f.h && l.at.x <= f.at.x + 25.0 * f.h
            })
            .min_by(|a, b| (a.1.1.at.y - l.at.y).total_cmp(&(b.1.1.at.y - l.at.y)))
            .map(|(i, _)| i)
        else {
            continue;
        };
        let Some(fh) = families.get(fi).map(|f| f.1.h) else { continue };
        let mut cells: Vec<(usize, &Line)> = lines
            .iter()
            .enumerate()
            .filter(|(j, c)| {
                *j != k
                    && (c.at.y - l.at.y).abs() <= 0.5 * l.h.max(1e-9)
                    && c.at.x > l.at.x
                    && c.at.x - l.at.x <= 60.0 * fh
                    && mark_parts(&c.s).is_none()
            })
            .collect();
        cells.sort_by(|a, b| a.1.at.x.total_cmp(&b.1.at.x));
        for j in std::iter::once(k).chain(cells.iter().map(|(j, _)| *j)) {
            if let Some(u) = used.get_mut(j) {
                *u = true;
            }
        }
        let r = row_of(&l.s, &cells.iter().map(|(_, c)| c.s.as_str()).collect::<Vec<_>>());
        let list = rows.entry(fi).or_default();
        if !list.iter().any(|x| x.mark == r.mark) {
            list.push(r);
        }
    }
    let segs = segments(d, &inside);
    for (fi, (letter, title)) in families.iter().enumerate() {
        let Some(marks) = rows.get(&fi) else {
            out.report.push(format!("TYPE {letter} PARTITION: no marks in its schedule"));
            continue;
        };
        // Its callouts: lines above its title (the nearest title under them is its), beside it.
        let mut own: Vec<&Line> = lines
            .iter()
            .enumerate()
            .filter(|(k, l)| {
                !used.get(*k).copied().unwrap_or(true)
                    && family_title(&l.s).is_none()
                    && l.at.y > title.at.y
                    && l.at.x >= title.at.x - 12.0 * title.h
                    && l.at.x <= title.at.x + 50.0 * title.h
            })
            .filter(|(_, l)| {
                let below = families
                    .iter()
                    .filter(|(_, f)| f.at.y < l.at.y && l.at.x >= f.at.x - 12.0 * f.h && l.at.x <= f.at.x + 50.0 * f.h)
                    .min_by(|a, b| (l.at.y - a.1.at.y).total_cmp(&(l.at.y - b.1.at.y)));
                below.is_some_and(|(_, f)| std::ptr::eq(*f, *title)) && l.at.y - title.at.y <= 70.0 * title.h
            })
            .map(|(_, l)| l)
            .collect();
        own.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        // Lines run on into a callout: the same left edge, the next line down.
        let mut callouts: Vec<Callout> = Vec::new();
        let mut taken = vec![false; own.len()];
        for (i, first) in own.iter().enumerate() {
            if taken.get(i).copied().unwrap_or(true) {
                continue;
            }
            let mut c = Callout { s: first.s.clone(), at: first.at, h: first.h };
            let mut y = first.at.y;
            for (j, next) in own.iter().enumerate().skip(i + 1) {
                let Some(t) = taken.get_mut(j) else { break };
                if !*t && (next.at.x - c.at.x).abs() <= 0.8 * c.h && y - next.at.y > 0.0 && y - next.at.y <= 1.9 * c.h {
                    *t = true;
                    c.s.push(' ');
                    c.s.push_str(&next.s);
                    y = next.at.y;
                }
            }
            callouts.push(c);
        }
        let read: Vec<(Callout, Says, Option<Vec2>)> = callouts
            .into_iter()
            .filter_map(|c| {
                let s = says(&c.s)?;
                let tip = leader_tip(&c, &segs);
                Some((c, s, tip))
            })
            .collect();
        // The wall's middle: where the core's leaders point, else between the tips of a callout
        // on both faces, else between all the tips.
        let tips = |f: &dyn Fn(&Says) -> bool| -> Vec<f64> { read.iter().filter(|(_, s, _)| f(s)).filter_map(|(_, _, t)| t.map(|t| t.x)).collect() };
        let mean = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
        let middle = mean(&tips(&|s| matches!(s, Says::Core(_))))
            .or_else(|| mean(&tips(&|s| matches!(s, Says::Face { both: true, .. }))))
            .or_else(|| mean(&tips(&|_| true)));
        let mut name = String::new();
        let mut face_name: Option<(LayerSide, String)> = None;
        let mut core: Vec<AssemblyLayer> = Vec::new();
        // Each face's layers (outer first), with the marks a layer is only for.
        let mut faces: Vec<(LayerSide, AssemblyLayer, Vec<String>)> = Vec::new();
        let mut unsided: Vec<String> = Vec::new();
        for (c, s, tip) in &read {
            let side_of = |x: f64| match middle {
                Some(m) if x < m => Some(LayerSide::Side1),
                Some(_) => Some(LayerSide::Side2),
                None => None,
            };
            match s {
                Says::Name(n) => name = n.clone(),
                Says::FaceName(n) => {
                    if let Some(side) = side_of(c.at.x) {
                        face_name = Some((side, n.clone()));
                    }
                }
                Says::Core(l) => {
                    if !core.iter().any(|x| x.category == l.category) {
                        core.push(l.clone());
                    }
                }
                Says::Face { layers, both: true, only } => {
                    for side in [LayerSide::Side1, LayerSide::Side2] {
                        faces.extend(layers.iter().map(|l| (side, l.clone(), only.clone())));
                    }
                }
                Says::Face { layers, both: false, only } => match tip.and_then(|t| side_of(t.x)) {
                    Some(side) => faces.extend(layers.iter().map(|l| (side, l.clone(), only.clone()))),
                    None => unsided.push(c.s.clone()),
                },
            }
        }
        let mut made = Vec::new();
        for r in marks {
            let prefix: String = r.mark.chars().take(r.mark.len().saturating_sub(1)).collect();
            let mut layers: Vec<AssemblyLayer> = Vec::new();
            for side in [LayerSide::Side1, LayerSide::Side2] {
                let here: Vec<&(LayerSide, AssemblyLayer, Vec<String>)> = faces.iter().filter(|(s, _, _)| *s == side).collect();
                // A board only for this mark takes the place of the face's outer board.
                let special: Vec<AssemblyLayer> = here.iter().filter(|(_, _, o)| o.contains(&prefix)).map(|(_, l, _)| l.clone()).collect();
                let mut list: Vec<AssemblyLayer> = here.iter().filter(|(_, _, o)| o.is_empty()).map(|(_, l, _)| l.clone()).collect();
                for sp in special {
                    match list.iter_mut().find(|l| l.category == sp.category) {
                        Some(l) => *l = sp,
                        None => list.insert(0, sp),
                    }
                }
                // Outer face first: finishes, sheathing and insulation outside, then board, then
                // channels against the studs (as the callouts are listed top down, a face's
                // order is by what it is).
                let rank = |l: &AssemblyLayer| match l.category.as_str() {
                    "finish" => 0,
                    "insulation" => 1,
                    "sheathing" => 3,
                    "board" => 2,
                    _ => 4,
                };
                list.sort_by_key(|l| rank(l));
                if side == LayerSide::Side2 {
                    list.reverse();
                }
                let mut sided: Vec<AssemblyLayer> = list
                    .into_iter()
                    .map(|mut l| {
                        l.side = side;
                        l
                    })
                    .collect();
                if side == LayerSide::Side1 {
                    layers.append(&mut sided);
                    // The core between the faces, at the schedule's depth(s) (else the callout's
                    // lumber size).
                    for c in &core {
                        let depths: Vec<f64> = if c.category == "insulation" {
                            vec![0.0]
                        } else if !r.depths.is_empty() {
                            r.depths.clone()
                        } else {
                            vec![c.thickness_in]
                        };
                        for dp in depths {
                            let mut l = c.clone();
                            l.thickness_in = dp;
                            if c.category != "insulation" && dp > 0.0 {
                                l.material = format!("{} {}", frac(dp), c.material);
                            }
                            layers.push(l);
                        }
                    }
                } else {
                    layers.append(&mut sided);
                }
            }
            let mut notes: Vec<String> = Vec::new();
            if let Some(t) = &r.tested {
                notes.push(format!("tested assembly {t}"));
            }
            notes.extend(r.notes.iter().cloned());
            if let Some((side, n)) = &face_name {
                notes.push(format!("{}: {n}", if *side == LayerSide::Side1 { "side 1" } else { "side 2" }));
            }
            for u in &unsided {
                notes.push(format!("one face, which not read: {u}"));
            }
            let head = r.mark.chars().last().and_then(|c| heads.get(&c).copied());
            let layers_external = layers.iter().any(|l| l.category == "finish");
            let thickness: f64 = layers.iter().filter(|l| !l.in_cavity).map(|l| l.thickness_in).sum();
            made.push(WallType {
                name: r.mark.clone(),
                height_ft: None,
                thickness_in: if thickness > 0.0 { thickness } else { 6.0 },
                from: WallFrom::Pairs,
                assembly: Some(Assembly {
                    tag: r.mark.clone(),
                    description: if name.is_empty() { format!("TYPE {letter} PARTITION") } else { format!("TYPE {letter} PARTITION - {name}") },
                    layers,
                    height: head.map(|h| h.0),
                    above_ceiling_in: head.map_or(6.0, |h| h.1),
                    fire_rating: r.fire.clone(),
                    acoustic_rating: r.stc.clone(),
                    external: layers_external,
                    notes,
                    head: None,
                    source: format!("{sheet} partition type legend, mark {}", r.mark),
                }),
            });
        }
        out.report.push(format!(
            "TYPE {letter} PARTITION: {} marks ({}), {} callouts read{}{}",
            made.len(),
            made.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "),
            read.len(),
            if middle.is_none() { "; no leaders found: one-face layers not placed" } else { "" },
            if unsided.is_empty() { String::new() } else { format!("; {} one-face callouts without a leader", unsided.len()) }
        ));
        for t in made {
            if !out.types.iter().any(|x| x.name == t.name) {
                out.types.push(t);
            }
        }
    }
    out
}

/// The finish on one face of a wall type, inches: its board and the channels or furring it is
/// hung on (what a box on the studs has to reach through), and the words for it.
pub(crate) fn face_depth(t: &WallType, side: LayerSide) -> Option<(f64, String)> {
    let a = t.assembly.as_ref()?;
    let on: Vec<&AssemblyLayer> = a.layers.iter().filter(|l| l.side == side && matches!(l.category.as_str(), "board" | "furring")).collect();
    if on.is_empty() {
        return None;
    }
    let depth: f64 = on.iter().map(|l| l.thickness_in).sum();
    Some((depth, on.iter().map(|l| l.material.as_str()).collect::<Vec<_>>().join(" + ")))
}

/// Plaster (mud) ring depths stocked, inches.
const RING_DEPTHS: [f64; 8] = [0.25, 0.5, 0.625, 0.75, 1.0, 1.25, 1.5, 2.0];

/// The ring for a face's finish: the shallowest stocked ring at least as deep (a box may sit back
/// from a noncombustible face by no more than 1/4", NEC 314.20; one deeper stands proud).
pub(crate) fn ring_for(depth_in: f64) -> Option<f64> {
    RING_DEPTHS.iter().copied().find(|r| *r + 1e-6 >= depth_in)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, Line as DLine, Space, Text};
    use cadaddy_geom::Vec3;

    fn text(d: &mut Drawing, v: &str, x: f64, y: f64, h: f64) {
        let t = Text {
            insert: Vec3::new(x, y, 0.0),
            align_pt: None,
            height: h,
            value: v.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        };
        d.add(&Space::Model, Common::default(), EntityKind::Text(t)).ok();
    }

    fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
        d.add(&Space::Model, Common::default(), EntityKind::Line(DLine { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) })).ok();
    }

    /// A made-up legend: family Q (a wall between a hall and a suite: board each face, a
    /// resilient channel on the right face only, studs), family W (two layers each face; one
    /// mark's left face is moisture resistant), and the head key.
    fn legend() -> Drawing {
        let mut d = Drawing::default();
        let h = 2.0;
        // Q's section: the wall's faces at x 100 and 110; callouts at x 130 with leaders.
        text(&mut d, "5/8\" TYPE-X GYP. BD.", 130.0, 200.0, h);
        text(&mut d, "EACH SIDE", 130.0, 197.0, h);
        line(&mut d, (129.0, 201.0), (99.0, 190.0));
        line(&mut d, (129.0, 201.0), (111.0, 190.0));
        text(&mut d, "1/2\" RESILIENT CHANNEL", 130.0, 185.0, h);
        text(&mut d, "@ 16\" O.C.", 130.0, 182.0, h);
        line(&mut d, (129.0, 186.0), (125.0, 186.0));
        line(&mut d, (125.0, 186.0), (110.5, 180.0));
        text(&mut d, "WOOD STUDS", 130.0, 170.0, h);
        line(&mut d, (129.0, 171.0), (105.0, 165.0));
        text(&mut d, "SOUND BATT INSULATION", 130.0, 160.0, h);
        line(&mut d, (129.0, 161.0), (104.0, 150.0));
        text(&mut d, "HALL SIDE", 70.0, 175.0, h);
        text(&mut d, "SUITE SEPARATION WALL", 90.0, 135.0, h);
        text(&mut d, "TYPE Q PARTITION", 90.0, 128.0, 3.0);
        text(&mut d, "Q4B", 92.0, 118.0, h);
        text(&mut d, "3 1/2\"", 102.0, 118.0, h);
        text(&mut d, "1 HOUR", 115.0, 118.0, h);
        text(&mut d, "UL U999", 135.0, 118.0, h);
        text(&mut d, "52", 150.0, 118.0, h);
        // W's section further down: faces at x 100 and 112.
        text(&mut d, "(2) LAYERS 5/8\" TYPE-X", 130.0, 60.0, h);
        text(&mut d, "GYP. BD. EACH SIDE", 130.0, 57.0, h);
        line(&mut d, (129.0, 61.0), (99.0, 50.0));
        line(&mut d, (129.0, 61.0), (113.0, 50.0));
        text(&mut d, "5/8\" WATER RESISTANT", 130.0, 45.0, h);
        text(&mut d, "TYPE-X GYP. BD. FOR", 130.0, 42.0, h);
        text(&mut d, "PARTITION W6", 130.0, 39.0, h);
        line(&mut d, (129.0, 46.0), (99.5, 40.0));
        text(&mut d, "2X6 WOOD STUDS", 130.0, 30.0, h);
        line(&mut d, (129.0, 31.0), (106.0, 25.0));
        text(&mut d, "TYPE W PARTITION", 90.0, 10.0, 3.0);
        text(&mut d, "W4A", 92.0, 0.0, h);
        text(&mut d, "3 1/2\"", 102.0, 0.0, h);
        text(&mut d, "2 HOUR", 115.0, 0.0, h);
        text(&mut d, "W6A", 92.0, -4.0, h);
        text(&mut d, "5 1/2\"", 102.0, -4.0, h);
        text(&mut d, "2 HOUR", 115.0, -4.0, h);
        // The head key.
        text(&mut d, "A - PARTITION TO EXTEND TO UNDERSIDE OF DECK", 300.0, 100.0, h);
        text(&mut d, "B - PARTITION TO EXTEND TO UNDERSIDE OF FIRE", 300.0, 95.0, h);
        text(&mut d, "RATED CEILING ASSEMBLY", 304.0, 92.0, h);
        d
    }

    fn side(t: &WallType, s: LayerSide) -> Vec<String> {
        t.assembly.as_ref().unwrap().layers.iter().filter(|l| l.side == s).map(|l| l.material.clone()).collect()
    }

    #[test]
    fn each_mark_of_a_legend_family_is_a_wall_type_with_its_layers_on_the_faces_its_leaders_point_at() {
        let lg = read_legend(&legend(), None, "X000");
        let names: Vec<&str> = lg.types.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Q4B", "W4A", "W6A"], "{:?}", lg.report);
        let q = &lg.types[0];
        let a = q.assembly.as_ref().unwrap();
        // Board each face; the channel on the right face (side 2), under its board.
        assert_eq!(side(q, LayerSide::Side1), vec!["5/8\" Type X gypsum board"]);
        assert_eq!(side(q, LayerSide::Side2), vec!["1/2\" resilient channel", "5/8\" Type X gypsum board"]);
        assert_eq!(side(q, LayerSide::Core), vec!["3 1/2\" wood studs", "sound batt insulation"]);
        assert_eq!((a.fire_rating.as_deref(), a.acoustic_rating.as_deref()), (Some("1 HR"), Some("STC 52")));
        assert_eq!(a.height, Some(HeightRule::Ceiling), "B: up to the rated ceiling");
        assert!(a.notes.contains(&"side 1: HALL SIDE".to_string()) && a.notes.contains(&"tested assembly UL U999".to_string()), "{:?}", a.notes);
        assert_eq!(a.description, "TYPE Q PARTITION - SUITE SEPARATION WALL");
        // What a box has to reach through on each face.
        assert_eq!(face_depth(q, LayerSide::Side1).map(|f| f.0), Some(0.625));
        assert_eq!(face_depth(q, LayerSide::Side2).map(|f| f.0), Some(1.125));
        assert_eq!(ring_for(1.125), Some(1.25));
        assert_eq!(ring_for(0.625), Some(0.625));
        // Two layers each face; W6's left face is moisture resistant in place of its outer board.
        let (w4, w6) = (&lg.types[1], &lg.types[2]);
        assert_eq!(side(w4, LayerSide::Side1), vec!["5/8\" Type X gypsum board", "5/8\" Type X gypsum board"]);
        assert_eq!(side(w6, LayerSide::Side1), vec!["5/8\" water resistant Type X gypsum board", "5/8\" Type X gypsum board"]);
        assert_eq!(side(w6, LayerSide::Side2), side(w4, LayerSide::Side2));
        assert_eq!(side(w6, LayerSide::Core), vec!["5 1/2\" wood studs"], "2X6 is 5 1/2\"");
        assert_eq!(w4.assembly.as_ref().unwrap().height, Some(HeightRule::Deck));
        assert_eq!(face_depth(w4, LayerSide::Side2).map(|f| f.0), Some(1.25));
    }

    #[test]
    fn marks_titles_and_head_keys_are_read_and_notes_are_not() {
        assert_eq!(mark_parts("B6A"), Some(('B', 'A')));
        assert_eq!(mark_parts("L8A"), Some(('L', 'A')));
        assert_eq!(mark_parts("A111"), None);
        assert_eq!(mark_parts("U341"), None);
        assert_eq!(family_title("TYPE M PARTITION"), Some('M'));
        assert_eq!(family_title("PARTITION TYPE LEGEND"), None);
        assert_eq!(head_key("C - PARTITION TO EXTEND 6\" ABOVE SUSPENDED CEILING").map(|k| k.0), Some('C'));
        assert_eq!(head_rule("PARTITION TO EXTEND 6\" ABOVE SUSPENDED CEILING"), Some((HeightRule::AboveCeiling, 6.0)));
        assert_eq!(head_rule("PARTIAL HEIGHT PARTITION REF. ELEVATION"), None);
        assert_eq!(inches_in("3 1/2\" & 5 1/2\""), vec![3.5, 5.5]);
        assert_eq!(inches_in("5/8\" TYPE-X GYP. BD. ON 7/8\" FURRING CHANNELS 16\" O.C."), vec![0.625, 0.875, 16.0]);
        assert_eq!(says("ACOUSTICAL SEALANT @ EACH SIDE - ON TOP & BOTTOM OF WALL"), None);
        assert!(
            matches!(says("5/8\" TYPE-X GYP. BD. ON 7/8\" FURRING CHANNELS 16\" O.C."), Some(Says::Face { ref layers, both: false, .. }) if layers.len() == 2)
        );
        assert_eq!(frac(1.125), "1 1/8\"");
    }
}
