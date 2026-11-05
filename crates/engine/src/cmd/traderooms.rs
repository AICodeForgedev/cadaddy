//! Rooms another trade's plans name (Q6180): the telecom, low-voltage, electrical and fire alarm
//! plans name rooms their systems need (an MDF, an IDF, a telecom room) that the architectural
//! plans leave unnamed or call something else.
//!
//! - A room label on another trade's plan registered to a level ([`name_rooms`], from the job's
//!   placer: the sheet point onto the level's architectural plan): inside a room of the model,
//!   it names a room the architectural plans left unnamed; a room they named the same keeps its
//!   name; a room they named otherwise keeps both (the architectural name stays the room's name,
//!   the other trade's goes beside it with its sheet) and is listed for review. A label in no
//!   room of the model with a closed outline round it on its own sheet is a room of its own
//!   (a telecom room only the trade plan draws); one in no closed outline stays unplaced, listed
//!   for review.
//! - An enlarged room plan of a trade (`LOW VOLTAGE - ROOM #105`) whose sheet's notes are the
//!   requirements of one kind of room (`REQUIREMENTS FOR IDF ROOMS:`): the room of that number is
//!   that room.
//! - Each room named by the rules' names ([`function_of`], `traderooms.json`) gets its function
//!   (`mdf`, `idf`, `telecom`), so the routing that looks for a telecom room finds it whatever its
//!   architectural name.
//!
//! Every value comes from the documents or from `traderooms.json` (rules data with its source).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use cadaddy_doc::{EntityKind, Handle};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::tagid::Sim;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Result, Session};

const RULES: &str = include_str!("traderooms.json");

/// Texts looked at, at most (hostile input).
const MAX_TEXTS: usize = 400_000;
/// Labels taken from one sheet, at most.
const MAX_LABELS_PER_SHEET: usize = 2_000;
/// Outcomes listed in the result, at most (all are counted).
const MAX_LISTED: usize = 200;
/// Sheets looked at, at most.
const MAX_SHEETS: usize = 2_000;

/// One kind of room the trades name, and the names it goes by.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FunctionRule {
    /// `mdf`, `idf`, `telecom`.
    pub function: String,
    /// The name a room of it is given when the documents give none of their own (`MDF`).
    pub label: String,
    /// Whole names of such a room (a room word and a number may go with them).
    pub names: Vec<String>,
    /// Names that are a system's or a device's too (`DATA`): a room's only with a room word.
    pub with_room_word: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub functions: Vec<FunctionRule>,
    pub room_words: Vec<String>,
    /// The disciplines whose floor plans may name rooms (`T`, `LV`, `E`, `FA`).
    pub trade_disciplines: Vec<String>,
    /// Words of a view title that make it a trade's view (`LOW VOLTAGE`).
    pub trade_view_words: Vec<String>,
    /// Words of a notes heading (`NOTES`, `REQUIREMENTS`).
    pub heading_words: Vec<String>,
    pub max_label_chars: usize,
    pub min_room_sqft: f64,
    pub max_room_sqft: f64,
    pub source: String,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// A text's words, upper case: split at spaces and punctuation (a `/` joins, `#` and `.` split).
fn words(t: &str) -> Vec<String> {
    t.to_ascii_uppercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '/'))
        .map(|w| w.trim_matches('/'))
        .filter(|w| !w.is_empty())
        .take(64)
        .map(str::to_string)
        .collect()
}

fn room_word(w: &str, r: &Rules) -> bool {
    r.room_words.iter().any(|x| x.trim_end_matches('.').eq_ignore_ascii_case(w))
}

/// A label's name words: its words less room words and numbers (`MDF ROOM 105` is `MDF`), and
/// whether it had a room word.
fn core(t: &str, r: &Rules) -> (Vec<String>, bool) {
    let all = words(t);
    let had = all.iter().any(|w| room_word(w, r));
    (all.into_iter().filter(|w| !room_word(w, r) && !w.chars().any(|c| c.is_ascii_digit())).collect(), had)
}

/// The kind of room a label names by the rules' names: the whole label is the name, give or take
/// a room word and a number (`MDF`, `MDF ROOM 201`, `DATA ROOM`; not `RUN TO MDF`).
pub(crate) fn function_of(t: &str) -> Option<&'static FunctionRule> {
    let r = rules();
    let (core, had) = core(t, r);
    if core.is_empty() {
        return None;
    }
    let name = core.join(" ");
    let is = |n: &String| words(n).join(" ") == name;
    r.functions.iter().find(|f| f.names.iter().any(is) || (had && f.with_room_word.iter().any(is)))
}

/// The function a label names, by its `function` (`mdf`).
pub(crate) fn function_rule(function: &str) -> Option<&'static FunctionRule> {
    rules().functions.iter().find(|f| f.function.eq_ignore_ascii_case(function.trim()))
}

/// Whether `want` (words) is in `have` (words) as a run.
fn has_run(have: &[String], want: &[String]) -> Option<usize> {
    if want.is_empty() || want.len() > have.len() {
        return None;
    }
    (0..=have.len() - want.len()).find(|&i| have.get(i..i + want.len()).is_some_and(|s| s == want))
}

/// The kind of room a notes heading is about: a heading (a heading word, or ending in `:`) naming
/// one kind of room followed by a room word (`IDF ROOM NOTES`, `REQUIREMENTS FOR TELECOM
/// ROOMS:`). None for a heading naming none or two kinds.
pub(crate) fn heading_function(t: &str) -> Option<&'static FunctionRule> {
    let r = rules();
    let w = words(t);
    let heading = t.trim_end().ends_with(':') || w.iter().any(|x| r.heading_words.iter().any(|h| h.eq_ignore_ascii_case(x)));
    if !heading || t.chars().count() > 2 * r.max_label_chars.max(1) {
        return None;
    }
    let mut found: Vec<&FunctionRule> = Vec::new();
    for f in &r.functions {
        let named = f.names.iter().chain(&f.with_room_word).any(|n| {
            let nw = words(n);
            has_run(&w, &nw).is_some_and(|i| w.get(i + nw.len()).is_some_and(|x| room_word(x, r)))
        });
        if named {
            found.push(f);
        }
    }
    match found.as_slice() {
        [one] => Some(one),
        _ => None,
    }
}

/// A trade's enlarged room plan's title: the room's number and the name it gives (`LOW VOLTAGE -
/// STORAGE ROOM #145`: `145`, `STORAGE`). A trade's view (a trade word in the title) or a room named by the
/// rules' names (`ENLARGED MDF ROOM 105`). None for a title naming no numbered room.
pub(crate) fn view_room(title: &str) -> Option<(String, String)> {
    let r = rules();
    let up = title.to_ascii_uppercase();
    if title.chars().count() > 2 * r.max_label_chars.max(1) {
        return None;
    }
    let w = words(&up);
    let at = w.iter().position(|x| room_word(x, r))?;
    let mut k = at + 1;
    if w.get(k).is_some_and(|x| x == "NO") {
        k += 1;
    }
    let number = w.get(k).filter(|x| x.chars().any(|c| c.is_ascii_digit()) && x.chars().count() <= 8)?.clone();
    // The name: the words before the room word after the title's last separator (` - `, `:`).
    let head = up.rsplit_once(" - ").map_or(up.as_str(), |(_, b)| b);
    let head = head.rsplit_once(':').map_or(head, |(_, b)| b);
    let hw = words(head);
    let stop = hw.iter().position(|x| room_word(x, r)).unwrap_or(hw.len());
    let skip = ["ENLARGED", "PLAN", "PARTIAL", "LAYOUT"];
    let name: Vec<&String> = hw.iter().take(stop).filter(|x| !skip.contains(&x.as_str())).collect();
    let name = name.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
    let trade_view = r.trade_view_words.iter().any(|t| up.contains(&t.to_ascii_uppercase()));
    (trade_view || function_of(&name).is_some()).then_some((number, name))
}

/// Whether two room names are one name: the same words (less room words and numbers), each word
/// of one the other's or its abbreviation (`STOR.` for `STORAGE`), or the same kind of room.
pub(crate) fn same_name(a: &str, b: &str) -> bool {
    let r = rules();
    let (a, _) = core(a, r);
    let (b, _) = core(b, r);
    if a.is_empty() || b.is_empty() {
        return a == b;
    }
    let abbrev = |x: &String, y: &String| x == y || (x.len().min(y.len()) >= 3 && (x.starts_with(y.as_str()) || y.starts_with(x.as_str())));
    if a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| abbrev(x, y)) {
        return true;
    }
    matches!((function_of(&a.join(" ")), function_of(&b.join(" "))), (Some(f), Some(g)) if f.function == g.function)
}

/// A room element's name the architectural plans didn't give: `Room`, `Room <number>` or none.
fn unnamed(name: &str, number: Option<&str>) -> bool {
    let n = name.trim();
    n.is_empty()
        || n.eq_ignore_ascii_case("Room")
        || number.is_some_and(|k| n.eq_ignore_ascii_case(k.trim()) || n.eq_ignore_ascii_case(&format!("Room {}", k.trim())))
}

/// A sheet another trade's rooms may be read on.
#[derive(Clone, Debug, Default)]
pub(crate) struct TradeSheet {
    pub number: String,
    pub discipline: String,
    /// The level its plan is of (empty: not a floor plan of a level).
    pub level: String,
    /// The sheet's rectangle (`[x0, y0, x1, y1]`).
    pub rect: [f64; 4],
    /// Where its plan is (its view, else its drawing area): labels are read inside it.
    pub plan: [f64; 4],
}

fn in_rect(r: &[f64; 4], p: Vec2) -> bool {
    p.x >= r[0].min(r[2]) && p.x <= r[0].max(r[2]) && p.y >= r[1].min(r[3]) && p.y <= r[1].max(r[3])
}

fn level_key(l: &str) -> String {
    l.chars().filter(char::is_ascii_digit).collect()
}

fn same_level(a: &str, b: &str) -> bool {
    let (ka, kb) = (level_key(a), level_key(b));
    if ka.is_empty() || kb.is_empty() { a.trim().eq_ignore_ascii_case(b.trim()) } else { ka == kb }
}

/// What became of one trade label.
#[derive(Clone, Debug, Default)]
struct Outcome {
    /// `named`, `agrees`, `conflict`, `added`, `unplaced`.
    how: &'static str,
    level: String,
    /// The model's room: its name and number as the model has them.
    room: String,
    number: Option<String>,
    /// The trade's name for it.
    name: String,
    function: Option<String>,
    source: String,
    why: String,
}

impl Outcome {
    fn json(&self) -> Value {
        let mut v = json!({
            "how": self.how, "level": self.level, "name": self.name, "source": self.source,
        });
        if !self.room.is_empty() {
            v["room"] = json!(self.room);
        }
        if let Some(n) = &self.number {
            v["number"] = json!(n);
        }
        if let Some(f) = &self.function {
            v["function"] = json!(f);
        }
        if !self.why.is_empty() {
            v["why"] = json!(self.why);
        }
        v
    }
}

/// A room element's tags as this step reads them.
struct ModelRoom {
    handle: Handle,
    level: String,
    name: String,
    number: Option<String>,
    function: Option<String>,
    other: Option<String>,
    outline: Vec<Vec2>,
}

fn model_rooms(d: &cadaddy_doc::Drawing) -> Vec<ModelRoom> {
    super::bspec::room_elements(d)
        .into_iter()
        .map(|r| {
            let e = d.model.get(r.handle);
            let tag = |k: &str| e.and_then(|e| super::bspec::room_tag(e, k)).filter(|v| !v.is_empty()).map(str::to_string);
            ModelRoom {
                handle: r.handle,
                function: tag("function:"),
                other: tag("other_name:"),
                level: r.level,
                name: r.name,
                number: r.number,
                outline: r.outline,
            }
        })
        .collect()
}

/// Write `tags` (`key:` → value) on a room element.
fn set_tags(s: &mut Session, h: Handle, tags: &[(&str, String)]) -> Result<()> {
    s.doc_mut()?.modify_entity(h, |e| {
        let mut x = e.common.xdata.to_vec();
        for (k, v) in tags {
            super::bspec::set_made_tag(&mut x, k, v);
        }
        e.common.xdata = x.into();
    })?;
    Ok(())
}

/// Apply a trade's name for model room `i`: name it when unnamed, add its function, or keep both
/// names when they differ. The outcome.
fn apply(s: &mut Session, room: &ModelRoom, name: &str, function: Option<&FunctionRule>, source: &str) -> Result<Outcome> {
    let function = function.or_else(|| function_of(&room.name));
    let mut out = Outcome {
        level: room.level.clone(),
        room: room.name.clone(),
        number: room.number.clone(),
        name: name.to_string(),
        function: function.map(|f| f.function.clone()),
        source: source.to_string(),
        ..Outcome::default()
    };
    let mut tags: Vec<(&str, String)> = Vec::new();
    if let Some(f) = function
        && room.function.is_none()
    {
        tags.push(("function:", f.function.clone()));
        tags.push(("function_source:", format!("{source} (named {name}; {})", rules().source)));
    }
    if unnamed(&room.name, room.number.as_deref()) {
        tags.push(("name:", name.to_string()));
        tags.push(("type:", name.to_string()));
        tags.push(("name_source:", source.to_string()));
        out.how = "named";
    } else if same_name(&room.name, name) || room.other.as_deref().is_some_and(|o| o.split("; ").any(|o| same_name(o, name))) {
        // The same name, or one the trades gave it already.
        out.how = "agrees";
    } else {
        let other = match &room.other {
            Some(o) => format!("{o}; {name}"),
            None => name.to_string(),
        };
        tags.push(("other_name:", other));
        tags.push(("other_name_source:", source.to_string()));
        out.how = "conflict";
    }
    if !tags.is_empty() {
        set_tags(s, room.handle, &tags)?;
    }
    Ok(out)
}

/// The rooms other trades' sheets name, put on the model's rooms (after the build). `sheets`: the
/// set's sheets (their floor plans' labels are read when they have a level and a trade
/// discipline; every sheet's enlarged room views); `place`: a sheet point of a level onto its
/// architectural plan. The result: counts, each room named, added, in conflict or unplaced.
pub(crate) fn name_rooms(s: &mut Session, sheets: &[TradeSheet], place: &dyn Fn([f64; 2], &str) -> Option<Sim>) -> Result<Value> {
    let r = rules();
    let Ok((sp, true)) = super::bspec::current(s, "bim_build") else { return Ok(json!({"named": 0})) };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    // Each level's plan corner; the stack moved each level's model by the lowest's minus its own.
    let corner = |l: &cadaddy_walk3d::Level| l.region.map(|g| Vec2::new(g[0].min(g[2]), g[1].min(g[3])));
    let lowest = sp.levels.iter().filter(|l| corner(l).is_some()).min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)).and_then(corner);
    let stack_off = |level: &str| -> Option<Vec2> {
        let lv = sp.levels.iter().find(|l| same_level(&l.name, level))?;
        Some(match (lowest, corner(lv)) {
            (Some(a), Some(b)) if sp.stack => a - b,
            _ => Vec2::ZERO,
        })
    };
    // The drawing's texts (not what a build made), once.
    let texts: Vec<(String, Vec2, f64)> = {
        let d = s.doc()?;
        let ents: Vec<&std::sync::Arc<cadaddy_doc::Entity>> = super::bspec::plan_objects(d)
            .into_iter()
            .filter(|e| matches!(e.kind, EntityKind::Text(_) | EntityKind::MText(_)))
            .take(MAX_TEXTS)
            .collect();
        super::perceive::texts(&ents)
            .into_iter()
            .filter(|(_, t, at, _)| at.is_finite() && !t.trim().is_empty())
            .map(|(_, t, at, h)| (t, at, h))
            .collect()
    };
    let mut outcomes: Vec<Outcome> = Vec::new();
    let mut not_placeable: BTreeMap<String, usize> = BTreeMap::new();
    let mut done: HashSet<(Handle, String)> = HashSet::new();
    // The model's rooms, read again after a change.
    let mut rooms: Option<Vec<ModelRoom>> = None;
    let trade = |sh: &TradeSheet| r.trade_disciplines.iter().any(|t| t.eq_ignore_ascii_case(sh.discipline.trim()));
    // (1) Room labels on the trades' floor plans.
    for sh in sheets.iter().filter(|sh| trade(sh) && !sh.level.trim().is_empty()).take(MAX_SHEETS) {
        let labels: Vec<(&String, Vec2, Option<&FunctionRule>)> = texts
            .iter()
            .filter(|(t, at, _)| in_rect(&sh.plan, *at) && t.chars().count() <= r.max_label_chars)
            .filter_map(|(t, at, _)| {
                let f = function_of(t);
                // A label of another kind names a room only with a room word (`OFFICE`, `CORRIDOR`):
                // a trade's plan is full of system and device words.
                let ok = f.is_some() || (super::perceive::geometry::room_name_score(t) >= 3 && !super::perceive::geometry::note_like(t));
                ok.then_some((t, *at, f))
            })
            .take(MAX_LABELS_PER_SHEET)
            .collect();
        if labels.is_empty() {
            continue;
        }
        let Some(off) = stack_off(&sh.level) else {
            *not_placeable.entry(format!("{}: no level {}", sh.number, sh.level)).or_default() += labels.len();
            continue;
        };
        let mut plan: Option<Option<Session>> = None;
        let mut cache = None;
        for (label, at, f) in labels {
            let p = [at.x, at.y];
            let Some(m) = place(p, &sh.level) else {
                *not_placeable.entry(format!("{}: not registered to {}", sh.number, sh.level)).or_default() += 1;
                continue;
            };
            let q = m.apply(p);
            let q = Vec2::new(q[0], q[1]) + off;
            if rooms.is_none() {
                rooms = Some(model_rooms(s.doc()?));
            }
            let inside = rooms
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|x| same_level(&x.level, &sh.level) && x.outline.len() >= 3 && cadaddy_geom::point_in_polygon(&x.outline, q))
                .min_by(|a, b| cadaddy_geom::shoelace(&a.outline).abs().total_cmp(&cadaddy_geom::shoelace(&b.outline).abs()));
            let source = format!("{} ({} plan of {})", sh.number, sh.discipline.to_ascii_uppercase(), sh.level);
            if let Some(room) = inside {
                // A room label of no trade's kind (a unit type, CORRIDOR) is the architectural
                // plan's label printed again, misregistered by a few feet as often as not: it
                // names a room the architectural plans left unnamed, never one they named.
                if f.is_none() && !unnamed(&room.name, room.number.as_deref()) {
                    let how = if same_name(&room.name, label) { "agrees" } else { "kept" };
                    outcomes.push(Outcome {
                        how,
                        level: room.level.clone(),
                        room: room.name.clone(),
                        name: label.trim().to_string(),
                        source,
                        ..Outcome::default()
                    });
                    continue;
                }
                if done.insert((room.handle, label.trim().to_ascii_uppercase())) {
                    let o = apply(s, room, label.trim(), f, &source)?;
                    if o.how != "agrees" || o.function.is_some() {
                        rooms = None;
                    }
                    outcomes.push(o);
                }
                continue;
            }
            // In no room of the model: a closed outline round it on its own sheet is a room of its
            // own when the label names a trade's room; another label there (a stair's, a note's
            // first line) is no room the trade adds.
            let Some(f) = f else {
                outcomes.push(Outcome { how: "unmatched", level: sh.level.clone(), name: label.trim().to_string(), source, ..Outcome::default() });
                continue;
            };
            let mut out = Outcome {
                how: "unplaced",
                level: sh.level.clone(),
                name: label.trim().to_string(),
                function: Some(f.function.clone()),
                source: source.clone(),
                ..Outcome::default()
            };
            let scale = m.scale();
            let outline = if scale.is_finite() && scale > 0.0 {
                if plan.is_none() {
                    let keep: HashSet<Handle> = {
                        let d = s.doc()?;
                        super::bspec::plan_objects(d)
                            .into_iter()
                            .filter(|e| super::bspec::centre(d, e).is_some_and(|c| in_rect(&sh.plan, c)))
                            .map(|e| e.handle)
                            .collect()
                    };
                    plan = Some(super::bspec::room_plan_of(s, &keep)?);
                }
                let min_area = r.min_room_sqft * upf * upf / (scale * scale);
                plan.as_ref().and_then(Option::as_ref).and_then(|p| super::bspec::outline_round_label(p, at, &mut cache, min_area))
            } else {
                None
            };
            let sqft = |o: &[Vec2]| cadaddy_geom::shoelace(o).abs() * scale * scale / (upf * upf);
            match outline {
                Some(o) if sqft(&o) <= r.max_room_sqft => {
                    let mapped: Vec<Vec2> = o
                        .iter()
                        .map(|v| {
                            let w = m.apply([v.x, v.y]);
                            Vec2::new(w[0], w[1])
                        })
                        .collect();
                    let extra = [
                        format!("name_source:{source}"),
                        format!("function:{}", f.function),
                        format!("function_source:{source} (named {}; {})", out.name, r.source),
                    ];
                    match super::bspec::add_room_element(s.doc_mut()?, &sp, &sh.level, &mapped, &out.name, off, &extra) {
                        Some(_) => {
                            out.how = "added";
                            rooms = None;
                        }
                        None => out.why = "its outline could not be made a room of the level".into(),
                    }
                }
                Some(o) => out.why = format!("the closed outline round it on {} is {:.0} sq ft, more than a room", sh.number, sqft(&o)),
                None => out.why = format!("in no room of the model, and nothing closed round it on {}", sh.number),
            }
            outcomes.push(out);
        }
    }
    // (2) A trade's enlarged room plans: the room their title numbers is the room their sheet's
    // notes are about.
    for sh in sheets.iter().take(MAX_SHEETS) {
        let in_sheet: Vec<&(String, Vec2, f64)> = texts.iter().filter(|(_, at, _)| in_rect(&sh.rect, *at)).collect();
        let views: Vec<(&String, (String, String))> = in_sheet.iter().filter_map(|(t, _, _)| view_room(t).map(|v| (t, v))).collect();
        let numbers: BTreeSet<String> = views.iter().map(|(_, (n, _))| n.clone()).collect();
        if numbers.len() != 1 {
            continue;
        }
        let Some((title, (number, vname))) = views.first().map(|(t, v)| (*t, v.clone())) else { continue };
        let headings: Vec<(&String, &FunctionRule)> = in_sheet.iter().filter_map(|(t, _, _)| heading_function(t).map(|f| (t, f))).collect();
        let kinds: BTreeSet<&str> = headings.iter().map(|(_, f)| f.function.as_str()).collect();
        let (function, heading) = match function_of(&vname) {
            Some(f) => (f, None),
            None if kinds.len() == 1 => match headings.first() {
                Some((h, f)) => (*f, Some(*h)),
                None => continue,
            },
            None => continue,
        };
        let name = if function_of(&vname).is_some() { vname.clone() } else { function.label.clone() };
        let mut source = format!("{} view \"{}\"", sh.number, title.trim());
        if let Some(h) = heading {
            source.push_str(&format!(" and its notes \"{}\"", h.trim()));
        }
        if rooms.is_none() {
            rooms = Some(model_rooms(s.doc()?));
        }
        let numbered: Vec<&ModelRoom> = rooms
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|x| x.number.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(&number)))
            .collect();
        let on_level: Vec<&&ModelRoom> = numbered.iter().filter(|x| !sh.level.is_empty() && same_level(&x.level, &sh.level)).collect();
        let room = match (on_level.as_slice(), numbered.as_slice()) {
            ([one], _) => Some(**one),
            (_, [one]) => Some(*one),
            _ => None,
        };
        let Some(room) = room else {
            outcomes.push(Outcome {
                how: "unplaced",
                level: sh.level.clone(),
                number: Some(number.clone()),
                name,
                function: Some(function.function.clone()),
                source,
                why: if numbered.is_empty() {
                    format!("no room numbered {number} in the model")
                } else {
                    format!("{} rooms numbered {number} in the model", numbered.len())
                },
                ..Outcome::default()
            });
            continue;
        };
        if done.insert((room.handle, name.to_ascii_uppercase())) {
            outcomes.push(apply(s, room, &name, Some(function), &source)?);
            rooms = None;
        }
    }
    let count = |how: &str| outcomes.iter().filter(|o| o.how == how).count();
    let mut functions: BTreeMap<String, usize> = BTreeMap::new();
    for f in model_rooms(s.doc()?).iter().filter_map(|x| x.function.clone()) {
        *functions.entry(f).or_default() += 1;
    }
    let listed: Vec<Value> =
        outcomes.iter().filter(|o| !matches!(o.how, "agrees" | "kept" | "unmatched")).take(MAX_LISTED).map(Outcome::json).collect();
    Ok(json!({
        "named": count("named"),
        "added": count("added"),
        "agrees": count("agrees"),
        "conflicts": count("conflict"),
        "unplaced": count("unplaced"),
        // Other room labels of the trades' plans: in a room the architectural plans named
        // otherwise (their name kept), and in no room.
        "kept_architectural": count("kept"),
        "unmatched": count("unmatched"),
        "functions": functions,
        "not_placeable": not_placeable,
        "rooms": listed,
        "source": r.source,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_load_with_their_source() {
        let r = rules();
        assert!(r.functions.len() >= 3 && !r.source.is_empty() && r.max_room_sqft > r.min_room_sqft && r.min_room_sqft > 0.0);
    }

    #[test]
    fn a_label_names_a_kind_of_room_only_as_a_whole_name() {
        let f = |t: &str| function_of(t).map(|f| f.function.as_str());
        assert_eq!(f("MDF"), Some("mdf"));
        assert_eq!(f("mdf room 201"), Some("mdf"));
        assert_eq!(f("IDF 3"), Some("idf"));
        assert_eq!(f("TELECOM ROOM"), Some("telecom"));
        assert_eq!(f("DATA ROOM 105"), Some("telecom"));
        // A system's or device's word alone, and a note naming the room, name none.
        assert_eq!(f("DATA"), None);
        assert_eq!(f("RUN TO MDF."), None);
        assert_eq!(f("OFFICE 101"), None);
        assert_eq!(f(""), None);
    }

    #[test]
    fn headings_name_the_room_their_notes_are_about() {
        let f = |t: &str| heading_function(t).map(|f| f.function.as_str());
        assert_eq!(f("IDF ROOM NOTES"), Some("idf"));
        assert_eq!(f("REQUIREMENTS FOR MDF ROOMS:"), Some("mdf"));
        // A sentence, a heading naming no room, and one naming two kinds.
        assert_eq!(f("1. PROVIDE CONDUIT TO THE MDF ROOM."), None);
        assert_eq!(f("GENERAL NOTES:"), None);
        assert_eq!(f("MDF ROOM AND IDF ROOM NOTES"), None);
    }

    #[test]
    fn a_trade_view_title_numbers_its_room() {
        assert_eq!(view_room("LOW VOLTAGE - STORAGE ROOM #145"), Some(("145".into(), "STORAGE".into())));
        assert_eq!(view_room("ENLARGED TELECOM ROOM 3105 PLAN"), Some(("3105".into(), "TELECOM".into())));
        assert_eq!(view_room("TECHNOLOGY: RM. NO. 12B"), Some(("12B".into(), String::new())));
        // An architectural enlarged plan, and a trade view numbering no room.
        assert_eq!(view_room("ENLARGED STORAGE ROOM #145"), None);
        assert_eq!(view_room("LOW VOLTAGE RISER DIAGRAM"), None);
        let _ = view_room(&"ROOM # ".repeat(5000));
    }

    // ---- synthetic sets: an architectural floor, and a trade's plan of it drawn elsewhere ----

    fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
        s.execute("line", &json!({"points": [a, b]})).unwrap();
    }

    fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
        line(s, a, [b[0], a[1]]);
        line(s, [b[0], a[1]], b);
        line(s, b, [a[0], b[1]]);
        line(s, [a[0], b[1]], a);
    }

    fn text(s: &mut Session, at: [f64; 2], t: &str, h: f64) {
        s.execute("text", &json!({"at": at, "text": t, "height": h})).unwrap();
    }

    /// A 40' × 30' floor (inches) at `o`, `k` times: 6" outside walls and 4" partitions at 160"
    /// and 320", so three closed rooms (west, middle, east); `labels[i]` written in room `i`.
    fn floor(s: &mut Session, o: [f64; 2], k: f64, labels: [&[&str]; 3]) {
        let p = |x: f64, y: f64| [o[0] + x * k, o[1] + y * k];
        s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
        rect(s, p(0.0, 0.0), p(480.0, 360.0));
        rect(s, p(6.0, 6.0), p(474.0, 354.0));
        for x in [158.0, 162.0, 318.0, 322.0] {
            line(s, p(x, 6.0), p(x, 354.0));
        }
        s.execute("layer.current", &json!({"name": "A-AREA-IDEN"})).unwrap();
        for (i, room) in labels.iter().enumerate() {
            for (j, t) in room.iter().enumerate() {
                text(s, p(40.0 + 160.0 * i as f64, 220.0 - 30.0 * j as f64), t, 6.0 * k);
            }
        }
    }

    /// The architectural floor at the origin (its labels `arch`, its spec rooms `rooms`), built.
    fn built(arch: [&[&str]; 3], rooms: Value) -> Session {
        let mut s = Session::new();
        s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
        s.execute("layer.new", &json!({"name": "A-AREA-IDEN"})).unwrap();
        floor(&mut s, [0.0, 0.0], 1.0, arch);
        let levels =
            json!([{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 10.0, "ceiling_ft": 9.0, "region": [-200.0, -200.0, 2000.0, 1200.0]}]);
        s.execute("bspec.draft", &json!({"store": true, "levels": levels, "stack": true})).unwrap();
        let mut spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
        spec["rooms"] = rooms;
        s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
        s.execute("model.build", &json!({})).unwrap();
        s
    }

    /// The trade's plan: the same floor at half scale at x 3000 (its labels `trade`), and the
    /// sheet it is on.
    fn trade_plan(s: &mut Session, discipline: &str, trade: [&[&str]; 3]) -> TradeSheet {
        floor(s, [3000.0, 0.0], 0.5, trade);
        let r = [2900.0, -100.0, 3400.0, 300.0];
        TradeSheet { number: "T-101".into(), discipline: discipline.into(), level: "Level 1".into(), rect: r, plan: r }
    }

    /// The trade's plan onto the architectural one (twice its size, 6000 to the west).
    fn place(p: [f64; 2], level: &str) -> Option<Sim> {
        (level == "Level 1" && p[0] > 2800.0).then(|| Sim::scale_offset(2.0, [-6000.0, 0.0]))
    }

    fn room<'a>(s: &'a Session, number: Option<&str>, name: &str) -> &'a cadaddy_doc::Entity {
        let d = s.doc().unwrap();
        let h = super::super::bspec::room_elements(d)
            .into_iter()
            .find(|r| r.number.as_deref() == number && (number.is_some() || r.name == name))
            .unwrap_or_else(|| {
                panic!("room {number:?} {name}: {:?}", super::super::bspec::room_elements(d).iter().map(|r| (&r.name, &r.number)).collect::<Vec<_>>())
            })
            .handle;
        d.model.get(h).unwrap()
    }

    fn tag<'a>(e: &'a cadaddy_doc::Entity, k: &str) -> Option<&'a str> {
        super::super::bspec::room_tag(e, k)
    }

    /// (1) Only the low-voltage plan names the rooms: the one the architectural plan numbers but
    /// doesn't name is named MDF (its function mdf, the sheet its source); the one it doesn't
    /// draw a label in at all is a room of its own, IDF, traced on the trade's sheet; a device
    /// word in the third (DATA) names nothing. A fire alarm plan's room name in the same set
    /// names the same way.
    #[test]
    fn rooms_only_a_trade_plan_names_are_named_in_the_model_with_their_sheet() {
        let mut s = built([&["OFFICE", "101"], &["102"], &[]], json!([{"name": "OFFICE", "number": "101"}, {"name": "", "number": "102"}]));
        assert_eq!(super::super::bspec::room_elements(s.doc().unwrap()).len(), 2);
        let sheet = trade_plan(&mut s, "T", [&["DATA"], &["MDF"], &["IDF ROOM"]]);
        let out = name_rooms(&mut s, &[sheet], &place).unwrap();
        assert_eq!((out["named"].as_u64(), out["added"].as_u64(), out["conflicts"].as_u64()), (Some(1), Some(1), Some(0)), "{out}");
        let mdf = room(&s, Some("102"), "");
        assert_eq!((tag(mdf, "name:"), tag(mdf, "function:")), (Some("MDF"), Some("mdf")));
        assert!(tag(mdf, "name_source:").is_some_and(|v| v.contains("T-101")), "{:?}", tag(mdf, "name_source:"));
        let idf = room(&s, None, "IDF ROOM");
        assert_eq!(tag(idf, "function:"), Some("idf"));
        // Traced on the trade's sheet, put where the room is on the architectural plan.
        let east = super::super::bspec::room_elements(s.doc().unwrap()).into_iter().find(|r| r.name == "IDF ROOM").unwrap();
        let b = cadaddy_geom::Bounds2::from_points(east.outline.iter().copied());
        assert!(b.min.x > 318.0 && b.max.x < 480.0 && (b.width() - 152.0).abs() < 2.0, "{b:?}");
        assert_eq!(tag(room(&s, Some("101"), ""), "name:"), Some("OFFICE"));
        // Rerun: nothing named twice, nothing added twice.
        let sheet = TradeSheet {
            number: "T-101".into(),
            discipline: "T".into(),
            level: "Level 1".into(),
            rect: [2900.0, -100.0, 3400.0, 300.0],
            plan: [2900.0, -100.0, 3400.0, 300.0],
        };
        let again = name_rooms(&mut s, std::slice::from_ref(&sheet), &place).unwrap();
        assert_eq!((again["named"].as_u64(), again["added"].as_u64(), again["agrees"].as_u64()), (Some(0), Some(0), Some(2)), "{again}");
        // An architectural sheet's labels are never read as a trade's.
        let arch = TradeSheet { discipline: "A".into(), ..sheet };
        assert_eq!(name_rooms(&mut s, &[arch], &place).unwrap()["agrees"].as_u64(), Some(0));
    }

    /// (2) The architectural plan and the low-voltage plan disagree: the architectural name stays,
    /// the trade's goes beside it with its sheet (never overwritten), listed for review; a name
    /// that is the same one abbreviated agrees.
    #[test]
    fn a_name_the_trade_gives_otherwise_is_kept_beside_the_architectural_one_for_review() {
        let mut s = built(
            [&["STORAGE", "101"], &["ELECTRICAL", "102"], &["OFFICE", "103"]],
            json!([{"name": "STORAGE", "number": "101"}, {"name": "ELECTRICAL", "number": "102"}, {"name": "OFFICE", "number": "103"}]),
        );
        // The east room's label, another room's name printed in it (a trade's plan misregistered
        // by a few feet), is no claim on the room: the architectural name is kept, unlisted.
        let sheet = trade_plan(&mut s, "LV", [&["MDF"], &["ELEC. ROOM"], &["BREAK ROOM"]]);
        let out = name_rooms(&mut s, &[sheet], &place).unwrap();
        assert_eq!((out["conflicts"].as_u64(), out["agrees"].as_u64(), out["named"].as_u64()), (Some(1), Some(1), Some(0)), "{out}");
        assert_eq!(out["kept_architectural"].as_u64(), Some(1), "{out}");
        assert_eq!(out["rooms"].as_array().map(Vec::len), Some(1), "{out}");
        assert_eq!(tag(room(&s, Some("103"), ""), "other_name:"), None);
        let st = room(&s, Some("101"), "");
        assert_eq!((tag(st, "name:"), tag(st, "other_name:"), tag(st, "function:")), (Some("STORAGE"), Some("MDF"), Some("mdf")));
        assert!(tag(st, "other_name_source:").is_some_and(|v| v.contains("T-101")));
        let listed = out["rooms"].as_array().unwrap();
        assert!(listed.iter().any(|v| v["how"] == "conflict" && v["room"] == "STORAGE" && v["name"] == "MDF"), "{out}");
        assert_eq!(tag(room(&s, Some("102"), ""), "other_name:"), None);
    }

    /// (3) A name label outside any closed area (off the floor on the trade's sheet): no room
    /// takes it and none is made; it is listed unplaced, with why.
    #[test]
    fn a_name_label_in_no_closed_area_stays_unplaced_for_review() {
        let mut s = built(
            [&["OFFICE", "101"], &["STORAGE", "102"], &["OFFICE", "103"]],
            json!([{"name": "OFFICE", "number": "101"}, {"name": "STORAGE", "number": "102"}, {"name": "OFFICE", "number": "103"}]),
        );
        let sheet = trade_plan(&mut s, "T", [&[], &[], &[]]);
        s.execute("layer.current", &json!({"name": "A-AREA-IDEN"})).unwrap();
        text(&mut s, [3260.0, 250.0], "TELECOM ROOM", 3.0);
        // Another room label off the floor (a stair's, beside the plan) is counted, not listed.
        text(&mut s, [3260.0, 100.0], "STAIR ROOM", 3.0);
        let before = super::super::bspec::room_elements(s.doc().unwrap()).len();
        let out = name_rooms(&mut s, &[sheet], &place).unwrap();
        assert_eq!((out["unplaced"].as_u64(), out["added"].as_u64(), out["named"].as_u64()), (Some(1), Some(0), Some(0)), "{out}");
        assert_eq!((out["unmatched"].as_u64(), out["rooms"].as_array().map(Vec::len)), (Some(1), Some(1)), "{out}");
        let u = &out["rooms"][0];
        assert_eq!(
            (u["how"].as_str(), u["name"].as_str(), u["function"].as_str()),
            (Some("unplaced"), Some("TELECOM ROOM"), Some("telecom")),
            "{out}"
        );
        assert!(u["why"].as_str().is_some_and(|w| w.contains("nothing closed round it on T-101")), "{out}");
        assert_eq!(super::super::bspec::room_elements(s.doc().unwrap()).len(), before);
    }

    /// An enlarged low-voltage room plan, on a sheet of no level (a site sheet's number), titled
    /// with the room's number, whose sheet's notes are an IDF room's: that room is the IDF. A
    /// sheet with two numbered room views, or notes naming two kinds of room, names none.
    #[test]
    fn an_enlarged_trade_room_view_and_its_notes_name_the_numbered_room() {
        let mut s = built(
            [&["STORAGE", "101"], &["OFFICE", "102"], &["OFFICE", "103"]],
            json!([{"name": "STORAGE", "number": "101"}, {"name": "OFFICE", "number": "102"}, {"name": "OFFICE", "number": "103"}]),
        );
        s.execute("layer.current", &json!({"name": "A-AREA-IDEN"})).unwrap();
        let sheet = |n: &str, x: f64| TradeSheet {
            number: n.into(),
            discipline: "L".into(),
            level: String::new(),
            rect: [x, 5000.0, x + 1000.0, 5800.0],
            plan: [x, 5000.0, x + 1000.0, 5800.0],
        };
        text(&mut s, [100.0, 5100.0], "LOW VOLTAGE - STORAGE ROOM #101", 4.0);
        text(&mut s, [600.0, 5500.0], "REQUIREMENTS FOR IDF ROOMS:", 4.0);
        text(&mut s, [600.0, 5490.0], "1. PROVIDE A PLYWOOD BACKBOARD IN THE IDF ROOM.", 3.0);
        // Two numbered views.
        text(&mut s, [2100.0, 5100.0], "LOW VOLTAGE - OFFICE ROOM #102", 4.0);
        text(&mut s, [2500.0, 5100.0], "LOW VOLTAGE - OFFICE ROOM #103", 4.0);
        text(&mut s, [2600.0, 5500.0], "REQUIREMENTS FOR MDF ROOMS:", 4.0);
        // Notes of two kinds.
        text(&mut s, [4100.0, 5100.0], "TELECOM - OFFICE ROOM #103", 4.0);
        text(&mut s, [4600.0, 5500.0], "MDF ROOM NOTES", 4.0);
        text(&mut s, [4600.0, 5400.0], "IDF ROOM NOTES", 4.0);
        let out = name_rooms(&mut s, &[sheet("L-9", 0.0), sheet("L-10", 2000.0), sheet("L-11", 4000.0)], &place).unwrap();
        assert_eq!((out["conflicts"].as_u64(), out["named"].as_u64(), out["unplaced"].as_u64()), (Some(1), Some(0), Some(0)), "{out}");
        let st = room(&s, Some("101"), "");
        assert_eq!((tag(st, "name:"), tag(st, "other_name:"), tag(st, "function:")), (Some("STORAGE"), Some("IDF"), Some("idf")));
        let src = tag(st, "other_name_source:").unwrap_or_default();
        assert!(src.contains("L-9") && src.contains("LOW VOLTAGE - STORAGE ROOM #101") && src.contains("REQUIREMENTS FOR IDF ROOMS:"), "{src}");
        for n in ["102", "103"] {
            assert_eq!(tag(room(&s, Some(n), ""), "function:"), None, "{n}");
        }
        // A view of a room the model doesn't have: unplaced.
        text(&mut s, [6100.0, 5100.0], "LOW VOLTAGE - ROOM #999", 4.0);
        text(&mut s, [6600.0, 5500.0], "REQUIREMENTS FOR MDF ROOMS:", 4.0);
        let out = name_rooms(&mut s, &[sheet("L-12", 6000.0)], &place).unwrap();
        assert_eq!(out["unplaced"].as_u64(), Some(1), "{out}");
        assert!(out["rooms"][0]["why"].as_str().is_some_and(|w| w.contains("no room numbered 999")), "{out}");
    }

    #[test]
    fn names_are_the_same_by_words_abbreviations_or_kind() {
        assert!(same_name("STORAGE 101", "STOR."));
        assert!(same_name("Electrical Room", "ELEC RM"));
        assert!(same_name("MDF", "MAIN DISTRIBUTION FRAME ROOM"));
        assert!(!same_name("STORAGE", "MDF"));
        assert!(!same_name("BREAK ROOM", "OFFICE"));
    }
}
