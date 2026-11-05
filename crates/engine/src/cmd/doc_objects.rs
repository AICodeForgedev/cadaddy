//! Objects the documents define and mark that no other build step makes (Q6192): a duct smoke
//! detector a mechanical legend abbreviates and the mechanical plans tag, a door holder the fire
//! alarm legend gives a row, an occupancy sensor or an exit sign an electrical keynote calls for,
//! a pump or a charger only an electrical note names.
//!
//! Every class is rules data ([`doc_objects.json`]): the words the documents name it by, its
//! trade, how it mounts, its connection; nothing here names a job's sheet, room or tag. The
//! readers take a class's code from the set's own legends and abbreviation lists (the sheet's own
//! first, then its trade's, then the general sheets'), its marks from the plans' tags and keynote
//! markers, and, for a class no plan marks, its count from the note that names it (each one
//! proposed, its location not drawn).

use std::collections::BTreeMap;

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use serde::Deserialize;

use super::dampers::LegendEntry;

const RULES: &str = include_str!("doc_objects.json");

/// The most words of a note read for its objects.
const MAX_NOTE_WORDS: usize = 400;

/// A class of object the documents name.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct Class {
    /// The element's kind (`duct_smoke_detector`).
    pub kind: String,
    pub name: String,
    /// Word groups: a text naming the class holds every word of one group.
    pub words: Vec<Vec<String>>,
    /// Words that make a text name something else (a smoke detector's DAMPER).
    pub not_words: Vec<String>,
    /// `fire alarm`, `electrical`, `low voltage`, `plumbing`, `mechanical`, `general`.
    pub trade: String,
    /// The scope split's kind for it (`duct detector`): whose work it is.
    pub scope_kind: String,
    /// Its fire alarm device kind (the scope rules' `fa_kinds`).
    pub fa_kind: String,
    /// Other trades whose plans tag it (a duct detector on the mechanical plans).
    pub tag_trades: Vec<String>,
    /// `duct`, `wall`, `ceiling`, `ceiling_below`, `floor`, `roof`, `site`.
    pub mount: String,
    /// A wall-mounted class's centre above its floor, inches.
    pub height_in: f64,
    /// Width, depth, height, inches.
    pub size_in: [f64; 3],
    /// `power`, `fire_alarm`, `data`: what the electrical side gives it when no note says.
    pub connection: String,
    /// Fire alarm modules each one takes (a duct detector's shutdown relay).
    pub fa_modules: u32,
    /// A remote test station where the fire alarm drawings show them.
    pub remote_test: bool,
    /// Built only from the notes that name it (no plan marks it by a code of its own).
    pub note_only: bool,
    /// Read from the plans' code tags (false: a class another step builds from its legend
    /// symbols, an occupancy sensor; only its keynotes and notes are read here).
    #[serde(default = "yes")]
    pub from_tags: bool,
    /// Kinds of other steps' elements that are this class there (`_damper`: any damper kind;
    /// a leading `_` matches the end of the kind).
    pub also_kinds: Vec<String>,
    pub source: String,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Connection {
    pub via: String,
    pub words: Vec<String>,
}

/// A box the build adds for a connection (an equipment connection box, a module).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BoxRule {
    pub kind: String,
    pub name: String,
    pub box_in: [f64; 3],
    pub height_in: f64,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub source: String,
    pub classes: Vec<Class>,
    pub connections: Vec<Connection>,
    pub not_new_words: Vec<String>,
    /// `ALL`, `EACH`: a note naming a class after one speaks of every one there is (a rule), not
    /// of new ones.
    pub all_words: Vec<String>,
    /// Words that end a note's room type (`EACH GUEST ROOM SHALL …`: the type is `GUEST`).
    pub room_stop_words: Vec<String>,
    /// Words of a room type that name no type of their own (`ROOM`, `TYPE`): left out when the
    /// type's words are matched with the rooms' names.
    pub room_generic_words: Vec<String>,
    /// The most words of a room type read after `ALL` / `EACH`.
    pub room_type_words: usize,
    pub count_words: BTreeMap<String, u32>,
    /// Words a number after which is a circuit's, a panel's or a sheet's, no count.
    pub number_words: Vec<String>,
    pub max_note_count: u32,
    pub remote_test_words: Vec<Vec<String>>,
    pub power_box: BoxRule,
    pub data_outlet: BoxRule,
    pub fa_module: BoxRule,
    pub remote_test: BoxRule,
    pub tag_reach_ft: f64,
    /// How far from its marker a keynote's object may stand (a marker sits off it, on a leader).
    pub keynote_reach_ft: f64,
    /// The most characters of a device legend's code.
    pub legend_code_chars: usize,
    pub duct_reach_ft: f64,
    pub dedupe_ft: f64,
    pub existing_reach_ft: f64,
    pub max_sheets: usize,
    pub max_elements: usize,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// A text's words, upper case, split at spaces and punctuation (`FIRE/SMOKE` is two).
pub(crate) fn words(t: &str) -> Vec<String> {
    t.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(MAX_NOTE_WORDS).map(str::to_string).collect()
}

/// Whether `have` holds `phrase` (one word or several in a row).
fn holds(have: &[String], phrase: &str) -> bool {
    let p = words(phrase);
    // A word's plural holds it too (`CIRCUITS`).
    !p.is_empty() && have.windows(p.len()).any(|w| w.iter().zip(&p).all(|(a, b)| a == b || a.strip_suffix('S') == Some(b.as_str())))
}

/// The class a text names: every word of one of its groups (in the rules' order), none of its
/// `not_words`. `DUCT MOUNTED SMOKE DETECTOR` is a duct smoke detector; `SMOKE DAMPER` none.
pub(crate) fn class_of(text: &str) -> Option<&'static Class> {
    let w = words(text);
    class_of_words(&w)
}

fn class_of_words(w: &[String]) -> Option<&'static Class> {
    rules().classes.iter().find(|c| {
        c.words.iter().any(|g| !g.is_empty() && g.iter().all(|x| w.iter().any(|y| y.eq_ignore_ascii_case(x))))
            && !c.not_words.iter().any(|x| holds(w, x))
    })
}

/// Whether another step's element of `kind` (its words `words`) is one of the class's.
pub(crate) fn is_class(class: &Class, kind: &str, words: &str) -> bool {
    let k = kind.trim().to_ascii_lowercase();
    class.kind.eq_ignore_ascii_case(&k)
        || class.also_kinds.iter().any(|a| match a.strip_prefix('_') {
            Some(end) => k.ends_with(&format!("_{end}")),
            None => k == a.to_ascii_lowercase(),
        })
        || class_of(&format!("{} {words}", k.replace('_', " "))).is_some_and(|c| c.kind == class.kind)
}

/// Whether `meaning` (another tag's legend words) names the same kind of thing as `class`: the
/// last word of one of its word groups (`DAMPER` of `RATED DAMPER` in `FIRE/SMOKE DAMPER`).
pub(crate) fn same_noun(class: &Class, meaning: &str) -> bool {
    let w = words(meaning);
    class
        .words
        .iter()
        .filter_map(|g| g.last())
        .any(|n| w.iter().any(|x| x.eq_ignore_ascii_case(n) || x.strip_suffix('S').is_some_and(|y| y.eq_ignore_ascii_case(n))))
}

/// A class by its element kind.
pub(crate) fn class_by_kind(kind: &str) -> Option<&'static Class> {
    rules().classes.iter().find(|c| c.kind.eq_ignore_ascii_case(kind.trim()))
}

/// What a connection note gives the object: `fire_alarm`, `data` or `power` (the rules' order: the
/// specific words before the generic `CONNECT`).
pub(crate) fn connection_via(text: &str) -> Option<&'static str> {
    let w = words(text);
    rules().connections.iter().find(|c| c.words.iter().any(|x| holds(&w, x))).map(|c| c.via.as_str())
}

/// Whether a text speaks of work that is not new (existing, to be removed).
pub(crate) fn not_new(text: &str) -> bool {
    let w = words(text);
    rules().not_new_words.iter().any(|x| holds(&w, x))
}

/// The trade a sheet's discipline letter stands for, by the trade names the classes use.
pub(crate) fn trade_of_discipline(discipline: &str) -> &'static str {
    match discipline.trim().to_ascii_uppercase().as_str() {
        "E" => "electrical",
        "FA" | "FP" | "F" => "fire alarm",
        "T" | "LV" | "TC" | "TN" | "TY" => "low voltage",
        "P" | "PL" => "plumbing",
        "M" | "H" | "HVAC" => "mechanical",
        _ => "general",
    }
}

/// Whether two sheets' trades share their legends: the same trade, or electrical with fire alarm
/// or low voltage (an electrical plan tags the fire alarm legend's devices).
fn related(a: &str, b: &str) -> bool {
    let (ta, tb) = (trade_of_discipline(a), trade_of_discipline(b));
    let wired = |t: &str| matches!(t, "electrical" | "fire alarm" | "low voltage");
    ta == tb || (wired(ta) && wired(tb))
}

/// What `code` means on `sheet` (of `discipline`): its own legend row first, then one of its
/// trade's sheets (or a related trade's: [`related`]), then the general sheets'
/// (`rows` in order of preference); with the row it came from. Another trade's legend gives no
/// meaning: `SD` on a mechanical plan reads the mechanical abbreviations, `SP` on a life safety
/// plan isn't the plumbing legend's sump pump.
pub(crate) fn meaning<'a>(code: &str, sheet: &str, discipline: &str, rows: &'a [(String, LegendEntry)]) -> Option<&'a LegendEntry> {
    let c = code.trim();
    let hit = |e: &&(String, LegendEntry)| e.1.code.eq_ignore_ascii_case(c);
    let d = discipline.trim();
    rows.iter()
        .filter(hit)
        .find(|(_, e)| e.sheet == sheet)
        .or_else(|| rows.iter().filter(hit).find(|(di, _)| di.eq_ignore_ascii_case(d)))
        .or_else(|| rows.iter().filter(hit).find(|(di, _)| related(di, d)))
        .or_else(|| rows.iter().filter(hit).find(|(di, _)| trade_of_discipline(di) == "general"))
        .map(|(_, e)| e)
}

/// Whether a class's tags count on a sheet of `discipline`: its own trade's sheets and the ones
/// its rules list (`tag_trades`), a fire alarm or low-voltage class on the electrical ones too; a
/// general sheet (a life safety plan) only for the fire alarm and general classes; a general
/// class anywhere.
pub(crate) fn tagged_on(class: &Class, discipline: &str) -> bool {
    let t = trade_of_discipline(discipline);
    if class.tag_trades.iter().any(|x| x.eq_ignore_ascii_case(t)) {
        return true;
    }
    match class.trade.as_str() {
        "general" => true,
        "fire alarm" => matches!(t, "fire alarm" | "electrical" | "general"),
        "low voltage" => matches!(t, "low voltage" | "electrical"),
        own => t == own,
    }
}

/// A text that can be a legend's or a tag's code (`DH`, `KB`, `F/SD`): 2 to 6 characters,
/// upper-case letters (2 at least) and `/`.
pub(crate) fn code_like(v: &str) -> bool {
    let n = v.chars().count();
    let letters = v.chars().filter(char::is_ascii_uppercase).count();
    (2..=6).contains(&n) && letters >= 2 && v.chars().all(|c| c.is_ascii_uppercase() || c == '/')
}

/// The legend rows of a sheet whose words name one of the classes, though the code needn't
/// abbreviate them (a device legend's `DH` beside `ELECTROMAGNETIC DOOR HOLDER, FLUSH`): a code
/// with its row's texts to its right (on its line), in a column of codes (another code just above
/// or below it); or one text ending in its code in brackets (`MAGNETIC DOOR HOLDER (DH)`).
pub(crate) fn legend_rows(texts: &[TextItem], sheet: &str) -> Vec<LegendEntry> {
    const MAX_TEXTS: usize = 200_000;
    const MAX_ROWS: usize = 2_000;
    let flat: Vec<&TextItem> = texts
        .iter()
        .take(MAX_TEXTS)
        .filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.is_finite() && !t.value.trim().is_empty())
        .collect();
    let mut out: Vec<LegendEntry> = Vec::new();
    for t in &flat {
        if out.len() >= MAX_ROWS {
            break;
        }
        let v = t.value.trim();
        // `… (DH)` in one text.
        if let Some(open) = v.rfind('(').filter(|_| v.ends_with(')')) {
            let code = v.get(open + 1..v.len() - 1).unwrap_or("").trim();
            let what = v.get(..open).unwrap_or("").trim();
            if code_like(code) && class_of(what).is_some() {
                out.push(LegendEntry { code: code.to_string(), meaning: what.to_string(), sheet: sheet.to_string(), at: t.at });
            }
            continue;
        }
        let short = |c: &str| code_like(c) && c.chars().count() <= rules().legend_code_chars.clamp(2, 6);
        if !short(v) {
            continue;
        }
        let h = t.height;
        let column = flat
            .iter()
            .any(|o| !std::ptr::eq(*o, *t) && code_like(o.value.trim()) && (o.at.x - t.at.x).abs() < 1.0 * h && (o.at.y - t.at.y).abs() < 6.0 * h);
        if !column {
            continue;
        }
        let end = t.at.x + 0.6 * h * v.chars().count() as f64;
        let mut row: Vec<&&TextItem> =
            flat.iter().filter(|o| o.at.x > end && o.at.x < t.at.x + 40.0 * h && (o.at.y - t.at.y).abs() <= 0.6 * h).collect();
        row.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
        // The row's own cells: up to the next code (another row of a legend in columns) or a gap
        // wider than a column's.
        let mut cells: Vec<&str> = Vec::new();
        let mut reach = end;
        for o in row {
            let ov = o.value.trim();
            if code_like(ov) || o.at.x > reach + 4.0 * h {
                break;
            }
            cells.push(ov);
            reach = o.at.x + 0.6 * o.height * ov.chars().count() as f64;
        }
        let what: String = cells.join(" ");
        if what.chars().count() <= 80 && class_of(&what).is_some() {
            out.push(LegendEntry { code: v.to_string(), meaning: what, sheet: sheet.to_string(), at: t.at });
        }
    }
    out
}

/// How many objects a note gives (`(2) EV CHARGING STATIONS`, `TWO SITE CAMERAS`,
/// `… STATIONS (QTY 2)`), 1 when it doesn't say; capped by the rules.
pub(crate) fn note_count(text: &str, class: &Class) -> u32 {
    let r = rules();
    let cap = r.max_note_count.max(1);
    let w = words(text);
    // The class's words' first place in the note: a count is just before it.
    let at = class.words.iter().filter_map(|g| g.first()).filter_map(|first| w.iter().position(|x| x.eq_ignore_ascii_case(first))).min().unwrap_or(0);
    let lo = at.saturating_sub(5);
    let count_of =
        |x: &String| x.parse::<u32>().ok().filter(|n| (1..=cap).contains(n)).or_else(|| r.count_words.get(x.as_str()).map(|n| (*n).clamp(1, cap)));
    // A circuit's or a panel's number (`CIRCUIT 12`) is no count.
    let numbered = |i: usize| i.checked_sub(1).and_then(|p| w.get(p)).is_some_and(|p| r.number_words.iter().any(|x| x.eq_ignore_ascii_case(p)));
    let before = (lo..at).rev().filter(|i| !numbered(*i)).find_map(|i| w.get(i).and_then(count_of));
    if let Some(n) = before {
        // `ONE EXIT SIGN AT THE HEAD AND ONE AT THE FLOOR`: each `AND <count>` after it is more of it.
        let more: u32 = w.windows(2).skip(at).filter(|p| p.first().is_some_and(|x| x == "AND")).filter_map(|p| p.get(1).and_then(count_of)).sum();
        return n.saturating_add(more).min(cap);
    }
    // `(QTY 2)` / `QTY: 2` anywhere.
    if let Some(i) = w.iter().position(|x| x == "QTY" || x == "QUANTITY")
        && let Some(n) = w.get(i + 1).and_then(|x| x.parse::<u32>().ok()).filter(|n| (1..=cap).contains(n))
    {
        return n;
    }
    1
}

/// An object a note names.
#[derive(Clone, Debug)]
pub(crate) struct Named {
    pub class: &'static Class,
    pub count: u32,
    /// What connects it (`power`), empty when the note says nothing of it.
    pub via: &'static str,
    /// The note speaks of every one there is (`ALL ROOFTOP UNITS SHALL SHUT DOWN`): a rule, not a call
    /// for new ones.
    pub all: bool,
    /// The room type it is called for in every room of (`… IN EACH GUEST SUITE`: `GUEST SUITE`),
    /// upper case; empty when the note names none.
    pub rooms: Vec<String>,
}

/// The objects a note names (each sentence's first) with their count and what it connects them
/// by. A note about existing or removed work names none.
pub(crate) fn note_objects(text: &str) -> Vec<Named> {
    if not_new(text) {
        return Vec::new();
    }
    let r = rules();
    let via = connection_via(text).unwrap_or("");
    let mut out: Vec<Named> = Vec::new();
    for part in text.split(['.', ';']).filter(|p| !p.trim().is_empty()).take(20) {
        let w = words(part);
        let Some(c) = class_of_words(&w) else { continue };
        if out.iter().any(|o| o.class.kind == c.kind) {
            continue;
        }
        // `ALL` right before the class's words (or its trade word: `ALL HVAC ROOFTOP UNITS`).
        let first = c.words.iter().filter_map(|g| g.first()).filter_map(|f| w.iter().position(|x| x.eq_ignore_ascii_case(f))).min().unwrap_or(0);
        let all = w.get(first.saturating_sub(2)..first).unwrap_or(&[]).iter().any(|x| r.all_words.iter().any(|a| a.eq_ignore_ascii_case(x)));
        let rooms = if all { Vec::new() } else { room_type_after(&w, first) };
        out.push(Named { class: c, count: note_count(part, c), via: connection_via(part).unwrap_or(via), all, rooms });
    }
    out
}

/// The room type a note's words call an object for in each of, after the class's words (`PROVIDE
/// ONE RANGE IN EACH GUEST SUITE`: `GUEST SUITE`): the words after an `ALL` / `EACH` word up to a
/// stop word, at most the rules' `room_type_words`.
fn room_type_after(w: &[String], class_at: usize) -> Vec<String> {
    let r = rules();
    let is = |list: &[String], x: &str| list.iter().any(|a| a.eq_ignore_ascii_case(x));
    let Some(k) = w.iter().enumerate().skip(class_at + 1).find(|(_, x)| is(&r.all_words, x)).map(|(i, _)| i) else {
        return Vec::new();
    };
    w.iter().skip(k + 1).take_while(|x| !is(&r.room_stop_words, x) && !is(&r.all_words, x)).take(r.room_type_words.max(1)).cloned().collect()
}

/// The rooms of a note's room type: each room whose name holds every word of the type that names
/// one (`GUEST SUITE` names the rooms `GUEST SUITE 201`, `GUEST SUITES` too; `ROOM` alone names
/// none). The longest run of the type's first words that names any room is the type (`GUEST SUITE
/// DOOR`: `GUEST SUITE`). `rooms`: (level, name, outline).
pub(crate) fn rooms_of_type<T>(type_words: &[String], rooms: &[(String, String, T)]) -> Vec<usize> {
    let r = rules();
    let generic = |x: &str| r.room_generic_words.iter().any(|g| g.eq_ignore_ascii_case(x));
    let sing = |x: &str| -> String {
        let u = x.to_ascii_uppercase();
        match u.strip_suffix('S') {
            Some(b) if b.len() >= 3 && !b.ends_with('S') => b.to_string(),
            _ => u,
        }
    };
    let names: Vec<Vec<String>> = rooms.iter().map(|(_, n, _)| words(n).iter().map(|x| sing(x)).collect()).collect();
    for n in (1..=type_words.len()).rev() {
        let want: Vec<String> = type_words.iter().take(n).filter(|x| !generic(x)).map(|x| sing(x)).collect();
        if want.is_empty() {
            continue;
        }
        let hits: Vec<usize> = names.iter().enumerate().filter(|(_, have)| want.iter().all(|x| have.contains(x))).map(|(i, _)| i).collect();
        if !hits.is_empty() {
            return hits;
        }
    }
    Vec::new()
}

/// Each object read paired with the object already in the model that is it, nearest pairs first,
/// each one used once: `pairs` (distance, read, in the model) within reach. So a keynote calling
/// for two where the plan already draws one adds one. Returns, per object read, its pair.
pub(crate) fn pair_nearest(mut pairs: Vec<(f64, usize, usize)>, read: usize) -> Vec<Option<usize>> {
    pairs.retain(|p| p.0.is_finite());
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut out: Vec<Option<usize>> = vec![None; read];
    let mut used: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for (_, i, j) in pairs {
        if used.contains(&j) {
            continue;
        }
        if let Some(slot) = out.get_mut(i).filter(|s| s.is_none()) {
            *slot = Some(j);
            used.insert(j);
        }
    }
    out
}

/// Whether a sheet's texts show duct detector remote test stations (the rules' words).
pub(crate) fn shows_remote_tests(texts: &[String]) -> bool {
    let r = rules();
    texts.iter().any(|t| {
        let w = words(t);
        r.remote_test_words.iter().any(|g| !g.is_empty() && g.iter().all(|x| holds(&w, x)))
    })
}

/// How an object was read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum How {
    /// A code tag on a plan.
    #[default]
    Tag,
    /// A keynote marker on a plan.
    Keynote,
    /// A note naming it, its location not drawn.
    Note,
    /// A note calling for it in every room of a type (`IN EACH GUEST SUITE`): one in each such
    /// room, its place in the room not drawn.
    Rooms,
}

impl How {
    pub(crate) fn label(self) -> &'static str {
        match self {
            How::Tag => "tag",
            How::Keynote => "keynote",
            How::Note => "note",
            How::Rooms => "note_rooms",
        }
    }
}

/// An object read off the documents, put on its level's plan.
#[derive(Clone, Debug, Default)]
pub(crate) struct Found {
    pub kind: String,
    pub how: How,
    pub sheet: String,
    /// Read on an enlarged plan (its position wins over a floor plan's).
    pub enlarged: bool,
    pub level: String,
    /// The tag's code (`SD`), else empty.
    pub code: String,
    pub number: String,
    /// The documents' words for it (`DUCT MOUNTED SMOKE DETECTOR`).
    pub meaning: String,
    /// Where its kind came from (`M-001 abbreviations: SD = …`).
    pub source: String,
    /// Where it is on the plan (plan units).
    pub at: Vec2,
    /// The keynote it was read from or the one beside its tag (number, text).
    pub keynote: Option<(String, String)>,
    /// What connects it, from its note (`power`), with the note.
    pub via: String,
    pub via_source: String,
    /// The other sheets showing the same object (one count).
    pub also: Vec<String>,
    /// Its type mark, from the object the model already has beside it that the same keynote
    /// describes (`XA`, the schedule's exit sign); empty when nothing gives one.
    pub type_mark: String,
}

/// One object per place: the same kind on the same level within `reach` (plan units), read on
/// another sheet, is one, the enlarged plan's position kept, the other sheets listed in `also`.
pub(crate) fn dedupe(mut found: Vec<Found>, reach: f64) -> Vec<Found> {
    // Enlarged plans first, so their positions are the ones kept.
    found.sort_by_key(|f| !f.enlarged);
    let mut out: Vec<Found> = Vec::new();
    for f in found {
        // Two marks of one sheet are two objects (a keynote's one at the head and one at the floor).
        let same =
            out.iter_mut().find(|o| o.kind == f.kind && o.level == f.level && o.how == f.how && o.sheet != f.sheet && (o.at - f.at).len() <= reach);
        match same {
            Some(o) => {
                if o.sheet != f.sheet && !o.also.contains(&f.sheet) {
                    o.also.push(f.sheet);
                }
            }
            None => out.push(f),
        }
    }
    out
}

/// The nearest point of a polyline to `p` within `reach`: (segment, point, distance).
pub(crate) fn nearest_on_run(points: &[Vec2], p: Vec2, reach: f64) -> Option<(usize, Vec2, f64)> {
    points
        .windows(2)
        .enumerate()
        .filter_map(|(i, w)| {
            let (a, b) = (*w.first()?, *w.get(1)?);
            let ab = b - a;
            let l2 = ab.dot(ab);
            let t = if l2 > 1e-12 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let q = a + ab * t;
            let d = (p - q).len();
            (d.is_finite() && d <= reach).then_some((i, q, d))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: 2.0, rotation: 0.0 }
    }

    /// The legend rows of a sheet as the build reads them: abbreviations (the code abbreviating
    /// its meaning) and device legend rows (any words, a column of codes), with the sheet's
    /// discipline.
    fn rows_of(discipline: &str, sheet: &str, texts: &[TextItem]) -> Vec<(String, LegendEntry)> {
        super::super::dampers::legend(texts, sheet).into_iter().chain(legend_rows(texts, sheet)).map(|e| (discipline.to_string(), e)).collect()
    }

    #[test]
    fn device_legend_rows_need_no_abbreviation_and_any_column_order_reads() {
        // A fire alarm device legend: code, quantity, description (the description doesn't
        // abbreviate the code), in a column; a row of another kind of thing; a bracketed code.
        let fa = vec![
            t("DH", 0.0, 100.0),
            t("7", 8.0, 100.0),
            t("ELECTROMAGNETIC DOOR HOLDER, FLUSH, 24V", 14.0, 100.0),
            t("QS", 0.0, 96.0),
            t("2", 8.0, 96.0),
            t("SPEAKER STROBE, CEILING", 14.0, 96.0),
            t("MAGNETIC DOOR HOLDER (MH)", 0.0, 80.0),
            t("KNOX BOX (QK)", 0.0, 76.0),
        ];
        let rows = legend_rows(&fa, "FA-001");
        let got: Vec<(&str, &str)> = rows.iter().map(|e| (e.code.as_str(), e.meaning.as_str())).collect();
        assert_eq!(got, [("DH", "7 ELECTROMAGNETIC DOOR HOLDER, FLUSH, 24V"), ("MH", "MAGNETIC DOOR HOLDER"), ("QK", "KNOX BOX")], "{rows:?}");
        // A lone code beside words on a plan (no column) is no legend row.
        assert!(legend_rows(&[t("DH", 0.0, 0.0), t("DOOR HOLDER", 6.0, 0.0)], "E-101").is_empty());
        // Codes right of their words: the reader reads to the right only, so none.
        assert!(legend_rows(&[t("DOOR HOLDER", 0.0, 10.0), t("DH", 30.0, 10.0), t("QS", 30.0, 6.0)], "FA-002").is_empty());
    }

    #[test]
    fn tags_on_a_plan_become_objects_of_their_legends_class_on_their_trades_sheets() {
        // The mechanical abbreviations (another sheet) define SD as a duct detector and FSD as a
        // damper (the dampers step's); the plan tags SD twice (one with a bent leader), FSD once.
        let legend = vec![
            t("FSD", 0.0, 100.0),
            t("FIRE/SMOKE DAMPER", 12.0, 100.0),
            t("SD", 0.0, 96.0),
            t("DUCT MOUNTED SMOKE DETECTOR", 12.0, 96.0),
            t("VD", 0.0, 92.0),
            t("VOLUME DAMPER", 12.0, 92.0),
        ];
        let mut rows = rows_of("M", "M-001", &legend);
        // The electrical legend calls SD a smoke detector: not on a mechanical plan.
        rows.extend(rows_of("E", "E-001", &[t("SD", 0.0, 50.0), t("SMOKE DETECTOR", 12.0, 50.0), t("HS", 0.0, 46.0), t("HORN STROBE", 12.0, 46.0)]));
        let plan = vec![t("SD", 500.0, 500.0), t("SD-2", 700.0, 520.0), t("FSD", 900.0, 500.0), t("ROOM 901", 600.0, 600.0)];
        // SD-2's leader: out of its tag, bent once, to the duct.
        let segs = vec![(Vec2::new(705.0, 519.0), Vec2::new(720.0, 500.0)), (Vec2::new(720.0, 500.0), Vec2::new(740.0, 480.0))];
        let known = |c: &str| meaning(c, "M-101", "M", &rows).and_then(|e| class_of(&e.meaning)).is_some_and(|k| tagged_on(k, "M"));
        let tags = super::super::dampers::tags(&plan, &segs, &[], &[], &known);
        let got: Vec<(String, String)> = tags.iter().map(|g| (g.code.clone(), g.number.clone())).collect();
        assert_eq!(got, [("SD".to_string(), String::new()), ("SD".to_string(), "2".to_string())], "{tags:?}");
        let to = tags.get(1).and_then(|g| g.points_to).expect("its leader's end");
        assert!((to - Vec2::new(740.0, 480.0)).len() < 1e-6, "{to:?}");
        let e = meaning("SD", "M-101", "M", &rows).expect("SD");
        assert_eq!(class_of(&e.meaning).map(|c| c.kind.as_str()), Some("duct_smoke_detector"));
        // On an electrical plan SD is the electrical legend's smoke detector: none of ours.
        assert_eq!(meaning("SD", "E-101", "E", &rows).map(|e| e.meaning.as_str()), Some("SMOKE DETECTOR"));
        assert!(meaning("SD", "E-101", "E", &rows).and_then(|e| class_of(&e.meaning)).is_none());
    }

    #[test]
    fn keynote_markers_give_their_notes_objects_and_counts() {
        let h = 9.0;
        let k = |x: f64, y: f64, v: &str| TextItem { value: v.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 };
        let texts = vec![
            k(1000.0, 1000.0, "ELECTRICAL KEYNOTES:"),
            k(1000.0, 970.0, "1."),
            k(1020.0, 970.0, "PROVIDE ONE EXIT SIGN AT THE HEAD AND ONE AT THE FLOOR."),
            k(1000.0, 958.0, "2."),
            k(1020.0, 958.0, "PROVIDE CEILING OCCUPANCY SENSOR FOR THE CORRIDOR LIGHTING."),
            k(1000.0, 946.0, "3."),
            k(1020.0, 946.0, "EXISTING EXIT SIGN TO REMAIN."),
            k(1000.0, 934.0, "4."),
            k(1020.0, 934.0, "INTERLOCK ALL ROOFTOP UNITS WITH THE ALARM SYSTEM."),
            // The plan's markers, each alone in its box.
            k(200.0, 200.0, "1"),
            k(400.0, 300.0, "2"),
            k(600.0, 300.0, "3"),
        ];
        let box_round = |x: f64, y: f64| {
            let (x0, y0, x1, y1) = (x - 6.0, y - 5.0, x + 12.0, y + 14.0);
            vec![
                (Vec2::new(x0, y1), Vec2::new(x1, y1)),
                (Vec2::new(x0, y0), Vec2::new(x1, y0)),
                (Vec2::new(x0, y0), Vec2::new(x0, y1)),
                (Vec2::new(x1, y0), Vec2::new(x1, y1)),
            ]
        };
        let mut segs = box_round(200.0, 200.0);
        segs.extend(box_round(400.0, 300.0));
        segs.extend(box_round(600.0, 300.0));
        let list = super::super::keynotes::lists(&texts).remove(0);
        let markers = super::super::keynotes::tags(&texts, &segs, &[], &[], &[], &list);
        let mut objects: Vec<(String, String, u32)> = Vec::new();
        for (num, _) in &markers {
            let Some(n) = list.notes.iter().find(|n| n.number == *num) else { continue };
            if let Some(x) = note_objects(&n.text).into_iter().find(|x| !x.all && tagged_on(x.class, "E")) {
                objects.push((num.clone(), x.class.kind.clone(), x.count));
            }
        }
        objects.sort();
        assert_eq!(objects, [("1".to_string(), "exit_sign".to_string(), 2), ("2".to_string(), "occupancy_sensor".to_string(), 1)], "{markers:?}");
        // The rule over all rooftop units calls for none.
        assert!(list.notes.iter().filter(|n| n.number == "4").flat_map(|n| note_objects(&n.text)).all(|x| x.all));
    }

    #[test]
    fn classes_are_read_by_their_words() {
        assert_eq!(class_of("DUCT MOUNTED SMOKE DETECTOR").map(|c| c.kind.as_str()), Some("duct_smoke_detector"));
        assert_eq!(class_of("SMOKE DAMPER"), None);
        assert_eq!(class_of("Electromagnetic door holder, flush").map(|c| c.kind.as_str()), Some("door_holder"));
        assert_eq!(class_of("PROVIDE DUAL-TECHNOLOGY OCCUPANCY SENSOR").map(|c| c.kind.as_str()), Some("occupancy_sensor"));
        assert_eq!(class_of("BACK DRAFT DAMPER AT FAN DISCHARGE").map(|c| c.kind.as_str()), Some("backdraft_damper"));
        // A range hood or a mounting range is no range.
        assert_eq!(class_of("RANGE HOOD"), None);
        assert_eq!(class_of("MOUNTING HEIGHT RANGE 48 AFF"), None);
        assert_eq!(class_of("FIRE EXTINGUISHER CABINET"), None);
        assert!(rules().classes.iter().all(|c| !c.kind.is_empty() && !c.source.is_empty() && !c.words.is_empty()));
        // The fire alarm control panel is no annunciator (the panels step's).
        assert_eq!(class_of("FIRE ANNUNCIATOR CONTROL PANEL"), None);
        assert_eq!(class_of("REMOTE ANNUNCIATOR PANEL").map(|c| c.kind.as_str()), Some("fire_alarm_annunciator"));
        // A keynote's class and another tag's meaning: the same noun.
        let rated = class_by_kind("rated_damper").unwrap();
        assert!(same_noun(rated, "FIRE/SMOKE DAMPER") && same_noun(rated, "FIRE DAMPERS") && !same_noun(rated, "DUCT DETECTOR"));
    }

    #[test]
    fn a_note_gives_its_objects_their_count_and_connection() {
        let one = |t: &str| note_objects(t).into_iter().next().map(|n| (n.class.kind.clone(), n.count, n.via, n.all));
        let got = note_objects("PROVIDE (2) 50A CIRCUITS FOR EV CHARGING STATIONS. ROUTE TO PANEL QX.");
        assert_eq!(got.len(), 1);
        assert_eq!(
            one("PROVIDE (2) 50A CIRCUITS FOR EV CHARGING STATIONS. ROUTE TO PANEL QX."),
            Some(("ev_charging_station".into(), 2, "power", false))
        );
        assert_eq!(one("PROVIDE POWER FOR TWO SITE CAMERAS ON THE POLE"), Some(("site_camera".into(), 2, "power", false)));
        assert_eq!(one("CONNECT ROOF TOP UNIT TO FIRE ALARM FOR SHUTDOWN"), Some(("rooftop_unit".into(), 1, "fire_alarm", false)));
        // A rule over every one there is calls for none.
        assert_eq!(one("INTERLOCK ALL ROOFTOP UNITS WITH THE FIRE ALARM"), Some(("rooftop_unit".into(), 1, "fire_alarm", true)));
        assert!(note_objects("EXISTING SUMP PUMP TO REMAIN, RECONNECT").is_empty());
        assert_eq!(one("SUMP PUMP (QTY 3): 120V CONNECTION").map(|n| n.1), Some(3));
        // One here and one there: two.
        assert_eq!(one("PROVIDE ONE EXIT SIGN AT THE HEAD AND ONE AT THE FLOOR").map(|n| n.1), Some(2));
        // A bare number far from the words is no count.
        assert_eq!(note_count("CIRCUIT 12 TO PANEL; PROVIDE SUMP PUMP CONNECTION", class_by_kind("sump_pump").unwrap()), 1);
    }

    #[test]
    fn a_code_reads_its_own_sheet_then_its_trade_then_the_general_sheets() {
        let row = |d: &str, sheet: &str, code: &str, meaning: &str| {
            (d.to_string(), LegendEntry { code: code.into(), meaning: meaning.into(), sheet: sheet.into(), at: Vec2::ZERO })
        };
        let rows = vec![
            row("E", "E-001", "SD", "SMOKE DETECTOR"),
            row("M", "M-001", "SD", "DUCT MOUNTED SMOKE DETECTOR"),
            row("G", "G-001", "KB", "KNOX BOX"),
            row("M", "M-102", "SD", "SUPPLY DIFFUSER"),
            row("P", "P-001", "SP", "SUMP PUMP"),
            row("FA", "FA-001", "DH", "MAGNETIC DOOR HOLDER"),
        ];
        assert_eq!(meaning("SD", "M-101", "M", &rows).map(|e| e.meaning.as_str()), Some("DUCT MOUNTED SMOKE DETECTOR"));
        assert_eq!(meaning("SD", "E-101", "E", &rows).map(|e| e.meaning.as_str()), Some("SMOKE DETECTOR"));
        assert_eq!(meaning("SD", "M-102", "M", &rows).map(|e| e.meaning.as_str()), Some("SUPPLY DIFFUSER"));
        assert_eq!(meaning("KB", "C-101", "C", &rows).map(|e| e.meaning.as_str()), Some("KNOX BOX"));
        assert!(meaning("QQ", "M-101", "M", &rows).is_none());
        // Another trade's legend gives no meaning; a related trade's does.
        assert!(meaning("SP", "G-101", "G", &rows).is_none());
        assert_eq!(meaning("DH", "E-101", "E", &rows).map(|e| e.meaning.as_str()), Some("MAGNETIC DOOR HOLDER"));
        let class = |k: &str| class_by_kind(k).unwrap();
        assert!(tagged_on(class("door_holder"), "E") && tagged_on(class("door_holder"), "G") && !tagged_on(class("door_holder"), "M"));
        assert!(tagged_on(class("wall_hydrant"), "P") && !tagged_on(class("wall_hydrant"), "E") && !tagged_on(class("wall_hydrant"), "G"));
        assert!(tagged_on(class("knox_box"), "G"));
    }

    #[test]
    fn the_same_object_on_two_sheets_counts_once_at_the_enlarged_plans_place() {
        let f = |sheet: &str, enlarged: bool, x: f64| Found {
            kind: "duct_smoke_detector".into(),
            sheet: sheet.into(),
            enlarged,
            level: "Level 1".into(),
            at: Vec2::new(x, 0.0),
            ..Found::default()
        };
        let got = dedupe(vec![f("M-101", false, 10.0), f("M-501", true, 12.0), f("M-101", false, 100.0), f("M-501", true, 13.0)], 36.0);
        assert_eq!(got.len(), 3, "two marks of one sheet stay two: {got:?}");
        assert_eq!(got[0].sheet, "M-501");
        assert_eq!(got[0].also, vec!["M-101".to_string()]);
    }

    #[test]
    fn a_point_snaps_to_the_nearest_run_in_reach() {
        let run = [Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(100.0, 100.0)];
        let (seg, q, d) = nearest_on_run(&run, Vec2::new(50.0, 10.0), 48.0).unwrap();
        assert_eq!(seg, 0);
        assert!((q.x - 50.0).abs() < 1e-9 && q.y.abs() < 1e-9 && (d - 10.0).abs() < 1e-9);
        assert_eq!(nearest_on_run(&run, Vec2::new(60.0, 60.0), 48.0).map(|n| n.0), Some(1));
        assert!(nearest_on_run(&run, Vec2::new(-80.0, 80.0), 48.0).is_none());
        assert!(nearest_on_run(&[], Vec2::ZERO, 48.0).is_none());
    }
}
