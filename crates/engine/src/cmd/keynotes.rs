//! Keynotes as the sheets print them: a `KEYNOTES` heading (`ELECTRICAL KEYNOTES:`), numbered
//! notes under it (`3. BDA SYSTEM CEILING MOUNTED ANTENNA LOCATION. PROVIDE 8"X8"X6" JUNCTION
//! BOX …`), and on the plans each note's number in a small box (a square, or a hexagon) where it
//! applies. What a note asks to be built is read from its words ([`work`]): a box of a size, and
//! conduit of a size between each of its boxes (a riser), stubbed into a space, or run to its
//! system's riser.

use std::sync::OnceLock;

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{PanelCircuit, PanelSchedule};
use serde::Deserialize;

/// The systems keynotes build and the parts of each the documents must place
/// ([`SystemRules`]).
const RULES: &str = include_str!("keynotes.json");

/// A system keynotes build (`BDA`): its names, and the parts of it the job looks for in the
/// documents (a head-end and its power, a donor antenna) and leaves `undecided` when no
/// document places them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SystemRules {
    pub name: String,
    pub also: Vec<String>,
    pub title: String,
    pub source: String,
    pub parts: Vec<PartRule>,
}

/// One part of a system: what it is, the words a note or schedule names it by, and whether it
/// takes power (a branch circuit of its own).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PartRule {
    pub part: String,
    pub what: String,
    pub words: Vec<String>,
    pub powered: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    systems: Vec<SystemRules>,
}

/// The rules data's systems.
pub(crate) fn systems() -> &'static [SystemRules] {
    static R: OnceLock<Rules> = OnceLock::new();
    &R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default()).systems
}

/// The rules for a system a note names (`BDA`); a system the data doesn't know has no parts.
pub(crate) fn system_rules(name: &str) -> SystemRules {
    systems()
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name) || s.also.iter().any(|a| a.eq_ignore_ascii_case(name)))
        .cloned()
        .unwrap_or_else(|| SystemRules { name: name.to_ascii_uppercase(), ..SystemRules::default() })
}

/// Whether `text` holds `phrase` as whole words (`BDA` in `BDA SYSTEM`, not in `ABDAL`).
fn has_phrase(text: &str, phrase: &str) -> bool {
    let norm = |s: &str| {
        let s: String = s.to_ascii_uppercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect();
        format!(" {} ", s.split_whitespace().collect::<Vec<_>>().join(" "))
    };
    let p = norm(phrase);
    !p.trim().is_empty() && norm(text).contains(&p)
}

impl SystemRules {
    /// Whether a text names the system.
    pub fn named_in(&self, text: &str) -> bool {
        !self.name.is_empty() && (has_phrase(text, &self.name) || self.also.iter().any(|a| has_phrase(text, a)))
    }
}

impl PartRule {
    /// Whether a text names the part.
    pub fn named_in(&self, text: &str) -> bool {
        self.words.iter().any(|w| has_phrase(text, w))
    }
}

/// The panel schedules' circuits that feed a system: those whose description names it
/// (`BDA SYSTEM`), with their panel.
pub(crate) fn system_circuits<'a>(sys: &SystemRules, schedules: &'a [PanelSchedule]) -> Vec<(&'a PanelSchedule, &'a PanelCircuit)> {
    schedules.iter().flat_map(|p| p.circuits.iter().map(move |c| (p, c))).filter(|(_, c)| sys.named_in(&c.description)).take(MAX_TAGS).collect()
}

/// Most notes a list holds, lists a sheet holds, tags a sheet holds (hostile files).
const MAX_NOTES: usize = 60;
const MAX_LISTS: usize = 12;
const MAX_TAGS: usize = 2000;
/// How far below its heading a list's notes run, in the heading's text heights.
const LIST_REACH: f64 = 120.0;
/// A note's lines start this close to its number's column, in its text heights (text further
/// right on its line is something else: a title block, a logo's address).
const NOTE_INDENT: f64 = 4.0;

/// A list's subject: its heading's first word (`ELEVATOR` of `ELEVATOR KEYNOTES:`).
pub(crate) fn subject(l: &List) -> String {
    l.heading.split_whitespace().next().unwrap_or_default().trim_end_matches(':').to_ascii_uppercase()
}

/// One numbered note.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Note {
    pub number: String,
    pub text: String,
}

/// A keynote list: its heading, notes, where it is (to tell its own numbers from the plan's
/// tags) and its numbers' text height (a tag's number is printed about as tall).
#[derive(Clone, Debug, Default)]
pub(crate) struct List {
    pub heading: String,
    pub notes: Vec<Note>,
    pub area: [f64; 4],
    pub number_h: f64,
}

/// `3.` / `3. BDA SYSTEM…` → (`3`, the rest).
fn numbered(v: &str) -> Option<(String, String)> {
    let t = v.trim();
    let n: String = t.chars().take_while(char::is_ascii_digit).collect();
    let rest = t.get(n.len()..)?.strip_prefix('.')?;
    (!n.is_empty() && n.len() <= 2 && (rest.is_empty() || rest.starts_with(' ') || rest.starts_with(|c: char| c.is_ascii_uppercase())))
        .then(|| (n, rest.trim().to_string()))
}

/// The keynote lists on a sheet's texts.
pub(crate) fn lists(texts: &[TextItem]) -> Vec<List> {
    lists_under(texts, |u| u.contains("KEYNOTES") || u.contains("KEYED NOTES"))
}

/// The numbered lists under the headings `heading` takes (its upper-case text): a sheet's
/// general notes as well as its keynotes.
pub(crate) fn lists_under(texts: &[TextItem], heading: impl Fn(&str) -> bool) -> Vec<List> {
    let flat: Vec<&TextItem> =
        texts.iter().filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.x.is_finite() && t.at.y.is_finite()).collect();
    let mut out = Vec::new();
    for head in flat.iter().filter(|t| t.value.len() <= 80 && heading(&t.value.to_ascii_uppercase())) {
        if out.len() >= MAX_LISTS {
            break;
        }
        let h = head.height;
        let below = |t: &&&TextItem| t.at.y < head.at.y - 0.3 * h && t.at.y > head.at.y - LIST_REACH * h;
        // The numbers' column: the left-most `N.` under the heading, near its left edge.
        let firsts: Vec<&&TextItem> = flat
            .iter()
            .filter(below)
            .filter(|t| numbered(&t.value).is_some() && t.at.x > head.at.x - 3.0 * h && t.at.x < head.at.x + 10.0 * h)
            .collect();
        let Some(col) = firsts.iter().map(|t| t.at.x).min_by(f64::total_cmp) else { continue };
        let nh = firsts.iter().map(|t| t.height).fold(f64::INFINITY, f64::min);
        if !nh.is_finite() {
            continue;
        }
        let mut lines: Vec<&&TextItem> = flat.iter().filter(below).filter(|t| t.at.x >= col - nh && t.at.x <= col + NOTE_INDENT * nh).collect();
        lines.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        let mut notes: Vec<Note> = Vec::new();
        let mut last_y = head.at.y;
        let mut y_min = head.at.y;
        for t in lines {
            // A gap of several lines (or another heading) ends the list.
            if !notes.is_empty() && last_y - t.at.y > 4.0 * nh {
                break;
            }
            if t.value.to_ascii_uppercase().contains("NOTES") && t.height > 1.2 * nh {
                break;
            }
            match numbered(&t.value).filter(|_| (t.at.x - col).abs() < 1.5 * nh) {
                Some((n, rest)) => {
                    if notes.len() >= MAX_NOTES {
                        break;
                    }
                    notes.push(Note { number: n, text: rest });
                }
                None => {
                    let Some(n) = notes.last_mut() else { continue };
                    if !n.text.is_empty() {
                        n.text.push(' ');
                    }
                    n.text.push_str(t.value.trim());
                }
            }
            last_y = t.at.y;
            y_min = y_min.min(t.at.y);
        }
        if notes.is_empty() {
            continue;
        }
        let x1 = col + 80.0 * nh;
        out.push(List { heading: head.value.trim().to_string(), notes, area: [col - nh, y_min - nh, x1, head.at.y + h], number_h: nh });
    }
    out
}

/// Where a list's numbers are tagged on its sheet: a text that is one of its numbers, about as
/// tall, outside the list, in a box — four sides of `segs` around it, or one of `boxes` (a
/// keynote symbol the import made a block) holding it — or alone: as tall as the list's numbers,
/// no other text on its line beside it (a box the import didn't keep; a grid bubble's number is
/// taller, a room's has three digits, a circuit's is part of its label).
///
/// Not a number in a small circle (`rounds`: a callout bubble's; a radius below 0 is a symbol's
/// middle, the number right over it its own), nor one in a view whose title
/// names another list's subject (`others`: an `ELEVATOR` view's numbers are the elevator
/// keynotes').
pub(crate) fn tags(
    texts: &[TextItem],
    segs: &[(Vec2, Vec2)],
    boxes: &[[f64; 4]],
    rounds: &[(Vec2, f64)],
    others: &[String],
    list: &List,
) -> Vec<(String, [f64; 2])> {
    let [ax0, ay0, ax1, ay1] = list.area;
    let mut out = Vec::new();
    for t in texts.iter().filter(|t| t.rotation.abs() < 0.05) {
        if out.len() >= MAX_TAGS {
            break;
        }
        let v = t.value.trim();
        let Some(n) = list.notes.iter().find(|n| n.number == v) else { continue };
        if t.height < 0.6 * list.number_h || t.height > 1.6 * list.number_h {
            continue;
        }
        if t.at.x >= ax0 && t.at.x <= ax1 && t.at.y >= ay0 && t.at.y <= ay1 {
            continue;
        }
        let h = t.height;
        // The number's middle (its text anchors at its baseline's start).
        let c = Vec2::new(t.at.x + 0.3 * h * v.len() as f64, t.at.y + 0.5 * h);
        let owned = |(o, r): &(Vec2, f64)| {
            if *r < 0.0 {
                // A symbol's middle right under it: the upper half of a callout bubble.
                (o.x - c.x).abs() < 0.6 * h && c.y - o.y > 0.5 * h && c.y - o.y < 2.2 * h
            } else {
                (*o - c).len() < *r && *r < 4.0 * h
            }
        };
        if rounds.iter().any(owned) {
            continue;
        }
        // A callout's number: a sheet's number (`E-6`, `A501`) right under it.
        let sheet_no = |v: &str| {
            let v = v.trim();
            let letters = v.chars().take_while(char::is_ascii_alphabetic).count();
            (1..=2).contains(&letters)
                && v.len() <= 8
                && v.chars().any(|c| c.is_ascii_digit())
                && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        };
        if texts.iter().any(|o| o.at.y < t.at.y && t.at.y - o.at.y < 3.0 * h && (o.at.x - t.at.x).abs() < 2.0 * h && sheet_no(&o.value)) {
            continue;
        }
        // The view it is in: the nearest title under it (a `… PLAN` larger than its notes).
        let title = texts
            .iter()
            .filter(|o| o.at.y < t.at.y && o.height >= 1.2 * h && (o.at.x - t.at.x).abs() < VIEW_REACH * h && t.at.y - o.at.y < VIEW_REACH * h)
            .filter(|o| o.value.to_ascii_uppercase().contains("PLAN"))
            .min_by(|a, b| (t.at.y - a.at.y).total_cmp(&(t.at.y - b.at.y)));
        if title.is_some_and(|ti| {
            let u = ti.value.to_ascii_uppercase();
            others.iter().any(|w| u.contains(w.as_str()))
        }) {
            continue;
        }
        let in_box = boxes.iter().any(|b| c.x > b[0] && c.x < b[2] && c.y > b[1] && c.y < b[3] && b[2] - b[0] < 5.0 * h && b[3] - b[1] < 5.0 * h);
        let as_tall = (h - list.number_h).abs() <= LONE_HEIGHT * list.number_h;
        // A real text (mostly letters and digits: a dashed line's glyphs read as text aren't)
        // right by it: in its box (an equipment tag's `FCU` over its `4`).
        let crowded = texts.iter().any(|o| {
            let ov = o.value.trim();
            let alnum = ov.chars().filter(char::is_ascii_alphanumeric).count();
            !std::ptr::eq(o, t)
                && alnum > 0
                && 2 * alnum >= ov.chars().count()
                && (o.at.x - t.at.x).abs() < 1.5 * h
                && (o.at.y - t.at.y).abs() < 1.6 * h
        });
        // Beside it on its line, or right above or below it (a callout bubble's `3` over `E-6`):
        // a real text.
        let lone = as_tall
            && !texts.iter().any(|o| {
                let ov = o.value.trim();
                let alnum = ov.chars().filter(char::is_ascii_alphanumeric).count();
                let real = alnum > 0 && 2 * alnum >= ov.chars().count();
                let oh = o.height.min(2.0 * h);
                !std::ptr::eq(o, t)
                    && real
                    && (o.at.y - t.at.y).abs() < 1.8 * h
                    && o.at.x < t.at.x + (0.7 * v.len() as f64 + 1.0) * h
                    && o.at.x + 0.7 * ov.len() as f64 * oh > t.at.x - 1.0 * h
            });
        if lone || (!crowded && (in_box || boxed(c, h, segs))) {
            out.push((n.number.clone(), [c.x, c.y]));
        }
    }
    out
}

/// Whether straight sides close around `c` a box's width away (a text height or two): one above,
/// one below, crossing over it, and one at a side.
fn boxed(c: Vec2, h: f64, segs: &[(Vec2, Vec2)]) -> bool {
    let near = |s: &&(Vec2, Vec2)| (s.0.x - c.x).abs() < 3.0 * h && (s.0.y - c.y).abs() < 3.0 * h;
    let mut sides = [false; 4];
    for (a, b) in segs.iter().filter(near) {
        let (dx, dy) = ((b.x - a.x).abs(), (b.y - a.y).abs());
        let len = dx.hypot(dy);
        if len < 0.5 * h || len > 6.0 * h {
            continue;
        }
        let y = (a.y + b.y) / 2.0;
        let x = (a.x + b.x) / 2.0;
        let crosses_x = a.x.min(b.x) < c.x && a.x.max(b.x) > c.x;
        let crosses_y = a.y.min(b.y) < c.y && a.y.max(b.y) > c.y;
        if dy < 0.1 * len && crosses_x {
            if y > c.y + 0.4 * h && y < c.y + 2.0 * h {
                sides[0] = true;
            } else if y < c.y - 0.4 * h && y > c.y - 2.0 * h {
                sides[1] = true;
            }
        } else if dx < 0.1 * len && crosses_y && (x - c.x).abs() > 0.4 * h && (x - c.x).abs() < 2.5 * h {
            sides[if x < c.x { 2 } else { 3 }] = true;
        }
    }
    sides[0] && sides[1] && (sides[2] || sides[3])
}

/// Most entities of a block a tag's box may be drawn in.
const MAX_BLOCK_ENTITIES: usize = 400;

/// A sheet's straight pieces in `rect` (drawing units): its lines and polylines, and those of
/// the blocks inserted on it (a tag's box is often drawn in a symbol's block).
pub(crate) fn sheet_segments(d: &cadaddy_doc::Drawing, rect: [f64; 4]) -> Vec<(Vec2, Vec2)> {
    use cadaddy_doc::EntityKind;
    let inside = |p: Vec2| p.x >= rect[0] && p.x <= rect[2] && p.y >= rect[1] && p.y <= rect[3];
    let mut out: Vec<(Vec2, Vec2)> = super::gridtable::segments(d).into_iter().filter(|(a, _)| inside(*a)).collect();
    let pieces = |kind: &EntityKind, m: &cadaddy_geom::Mat3, out: &mut Vec<(Vec2, Vec2)>| match kind {
        EntityKind::Line(l) => out.push((m.apply(l.a.xy()), m.apply(l.b.xy()))),
        EntityKind::LwPolyline(p) => {
            let n = p.vertices.len();
            let last = if p.closed { n } else { n.saturating_sub(1) };
            for i in 0..last.min(MAX_BLOCK_ENTITIES) {
                if let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get((i + 1) % n.max(1)))
                    && a.bulge.abs() < 1e-9
                {
                    out.push((m.apply(a.p), m.apply(b.p)));
                }
            }
        }
        _ => {}
    };
    for e in d.model.iter() {
        let EntityKind::Insert(ins) = &e.kind else { continue };
        if !inside(ins.insert.xy()) {
            continue;
        }
        let Some(b) = d.block(&ins.block) else { continue };
        if b.entities.len() > MAX_BLOCK_ENTITIES {
            continue;
        }
        let m = ins.transform(b.base.xy());
        for be in b.entities.iter() {
            pieces(&be.kind, &m, &mut out);
        }
    }
    out
}

/// How far a view's title may be from a tag in it (below it), in the tag's text heights.
const VIEW_REACH: f64 = 60.0;
/// A lone tag's number is the list's numbers' height within this share.
const LONE_HEIGHT: f64 = 0.12;

/// How a note's conduit runs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Route {
    /// Between each of the note's boxes, floor to floor.
    Riser,
    /// From each box into the nearest room so named (`CORRIDOR`), above its ceiling.
    Stub(String),
    /// From each box to its system's riser box on its level.
    ToRiser,
    /// Under the slab from what its tag points at (a floor box) to the nearest wall of its room,
    /// stubbed up there.
    UnderSlab,
}

/// What a note asks to be built.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Work {
    /// The system it is part of (`BDA`), from `<NAME> SYSTEM`.
    pub system: String,
    /// Its box: height, width, depth, inches.
    pub box_in: Option<[f64; 3]>,
    /// Its conduit: trade size (inches) and how it runs.
    pub conduits: Vec<(f64, Route)>,
    /// The note puts it in the ground or under the slab (`UNDER SLAB`, `BELOW GRADE`).
    pub buried: bool,
    /// The note gives no conduit size: the smallest trade size is assumed.
    pub size_assumed: bool,
    /// Its box is an antenna's (`… ANTENNA LOCATION`) or the system's riser box (`RISER
    /// JUNCTION BOX`, or a box its conduit rises between).
    pub antenna: bool,
    pub riser: bool,
    /// The conduits (of [`Work::conduits`]) the note gives a pull tape (`WITH PULL TAPE`).
    pub pull_tape: Vec<(f64, Route)>,
    /// The space its conduit to the riser runs in or above (`ABOVE HARD LID PORTION OF
    /// CORRIDOR` → `CORRIDOR`), and whether over a hard (gypsum) ceiling there.
    pub via: Option<String>,
    pub hard_lid: bool,
    /// The fire rating the note keeps where its work passes (`MAINTAIN 2-HOUR FIRE RATING OF
    /// FLOORS AND WALLS`).
    pub rating: Option<Rating>,
}

/// A fire rating a note says to keep: its hours, and whether of floors, of walls (neither said:
/// both).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Rating {
    pub hours: f64,
    pub floors: bool,
    pub walls: bool,
}

impl Rating {
    /// `2-hour`.
    pub fn words(&self) -> String {
        format!("{}-hour", self.hours)
    }
}

impl Work {
    /// The kind its box is built as: `<system>_antenna_box`, `<system>_riser_box`, else a plain
    /// `junction_box` (a note that names no system, or no role for its box).
    pub fn box_kind(&self) -> String {
        let sys = self.system.to_ascii_lowercase();
        let role = if self.antenna {
            "antenna"
        } else if self.riser {
            "riser"
        } else {
            ""
        };
        if sys.is_empty() || role.is_empty() { "junction_box".to_string() } else { format!("{sys}_{role}_box") }
    }
}

/// The words a note gives a conduit's pull line with.
const PULL_WORDS: [&str; 6] = ["PULL TAPE", "PULL STRING", "PULL LINE", "PULL ROPE", "PULLWIRE", "PULL WIRE"];
/// The words that put a run over a hard (gypsum board) ceiling.
const HARD_LID_WORDS: [&str; 4] = ["HARD LID", "HARD-LID", "HARDLID", "GYP"];
/// The words before the space a run follows (`… PORTION OF CORRIDOR`, `ALONG THE CORRIDOR`).
const VIA_WORDS: [&str; 5] = ["OF", "IN", "ALONG", "THROUGH", "VIA"];

/// `2-HOUR`, `2 HOUR`, `TWO-HOUR`, `2 HR` → 2.
fn hours(sentence: &str) -> Option<f64> {
    let w: Vec<&str> = sentence.split(|c: char| c.is_whitespace() || c == '-').filter(|x| !x.is_empty()).collect();
    let i = w.iter().position(|x| matches!(x.trim_end_matches(['.', ',']), "HOUR" | "HOURS" | "HR" | "HRS"))?;
    let n = w.get(i.checked_sub(1)?)?;
    let v = match *n {
        "ONE" => 1.0,
        "TWO" => 2.0,
        "THREE" => 3.0,
        "FOUR" => 4.0,
        _ => n.trim_start_matches('(').trim_end_matches(')').parse::<f64>().ok()?,
    };
    (v > 0.0 && v <= 4.0).then_some(v)
}

/// The words a note uses for work in the ground or under the slab.
const BURIED_WORDS: [&str; 11] = [
    "UNDER SLAB",
    "UNDERSLAB",
    "UNDER-SLAB",
    "BELOW SLAB",
    "BELOW GRADE",
    "BELOW GROUND",
    "UNDERGROUND",
    "UNDER GROUND",
    "BURIED",
    "BURRIED",
    "IN GRADE",
];
/// The trade size assumed for buried conduit a note doesn't size, inches.
pub(crate) const BURIED_MIN_IN: f64 = 0.75;

/// Whether a text calls for work in the ground or under the slab.
pub(crate) fn buried(text: &str) -> bool {
    let up = text.to_ascii_uppercase();
    BURIED_WORDS.iter().any(|w| up.contains(w))
}

/// `12"X12"X6"` → [12, 12, 6].
fn dims(word: &str) -> Option<[f64; 3]> {
    let parts: Vec<f64> =
        word.split(['X', 'x']).map(|p| p.trim().trim_end_matches(['"', ',', '.', ';']).trim()).filter_map(|p| p.parse::<f64>().ok()).collect();
    match parts.as_slice() {
        [a, b, c] if word.contains('"') && [*a, *b, *c].iter().all(|v| *v > 0.0 && *v <= 96.0) => Some([*a, *b, *c]),
        _ => None,
    }
}

/// Lines a note of buried work is joined from, at most.
const MAX_NOTE_LINES: usize = 12;

/// The notes of a sheet that call for buried work, each whole: its numbered lists' notes (with
/// their numbers: `21`, `PROVIDE UNDER SLAB CONDUIT…`) and the paragraphs outside the lists, each
/// joined from the lines left-aligned with the one naming the work, one under the next (a site
/// plan's callout: `TWO (2) 5" SCH. 40 PVC CONDUIT … MIN. 48" BELOW GRADE …`).
pub(crate) fn buried_notes(texts: &[TextItem]) -> Vec<(Option<String>, String)> {
    notes_matching(texts, buried)
}

/// The notes of a sheet with a line `hit` says yes to, each whole (as [`buried_notes`] joins
/// them): its numbered lists' notes whose text is a hit, and the paragraphs outside the lists
/// joined round each line that is.
pub(crate) fn notes_matching(texts: &[TextItem], hit: impl Fn(&str) -> bool) -> Vec<(Option<String>, String)> {
    let ls = lists(texts);
    let mut out: Vec<(Option<String>, String)> =
        ls.iter().flat_map(|l| &l.notes).filter(|n| hit(&n.text)).map(|n| (Some(n.number.clone()), n.text.clone())).collect();
    let in_list = |p: Vec2| ls.iter().any(|l| p.x >= l.area[0] && p.x <= l.area[2] && p.y >= l.area[1] && p.y <= l.area[3]);
    let mut used = vec![false; texts.len()];
    for (i, t) in texts.iter().enumerate() {
        if used.get(i).copied().unwrap_or(true)
            || !hit(&t.value)
            || in_list(t.at)
            || t.rotation.abs() > 0.01
            || !t.height.is_finite()
            || t.height <= 0.0
        {
            continue;
        }
        let h = t.height;
        // The line next above (`up`) or below a line, left-aligned with it and as tall.
        let step = |cur: Vec2, up: bool, used: &[bool]| {
            texts
                .iter()
                .enumerate()
                .filter(|(j, o)| {
                    let dy = if up { o.at.y - cur.y } else { cur.y - o.at.y };
                    !used.get(*j).copied().unwrap_or(true)
                        && (o.at.x - cur.x).abs() < h
                        && (o.height - h).abs() < 0.25 * h
                        && dy > 0.5 * h
                        && dy < 2.2 * h
                        && o.rotation.abs() <= 0.01
                })
                .min_by(|a, b| (a.1.at.y - cur.y).abs().total_cmp(&(b.1.at.y - cur.y).abs()))
                .map(|(j, _)| j)
        };
        if let Some(u) = used.get_mut(i) {
            *u = true;
        }
        let mut lines: Vec<usize> = vec![i];
        // Up to the paragraph's first line (a numbered line starts one).
        while lines.len() < MAX_NOTE_LINES {
            let Some(&top) = lines.first() else { break };
            let Some(cur) = texts.get(top) else { break };
            if numbered(&cur.value).is_some() {
                break;
            }
            let Some(j) = step(cur.at, true, &used) else { break };
            if texts.get(j).is_some_and(|o| o.value.trim_end().ends_with('.') && numbered(&o.value).is_none()) {
                break;
            }
            if let Some(u) = used.get_mut(j) {
                *u = true;
            }
            lines.insert(0, j);
        }
        // Down to its last (the next numbered line starts another).
        while lines.len() < MAX_NOTE_LINES {
            let Some(&bottom) = lines.last() else { break };
            let Some(cur) = texts.get(bottom) else { break };
            let Some(j) = step(cur.at, false, &used) else { break };
            if texts.get(j).is_some_and(|o| numbered(&o.value).is_some()) {
                break;
            }
            if let Some(u) = used.get_mut(j) {
                *u = true;
            }
            lines.push(j);
        }
        let text = lines.iter().filter_map(|j| texts.get(*j)).map(|o| o.value.trim()).collect::<Vec<_>>().join(" ");
        if !out.iter().any(|(_, o)| *o == text) {
            out.push((None, text));
        }
    }
    out
}

/// What a note's words ask to be built (none: a note that builds nothing it can read).
pub(crate) fn work(text: &str) -> Option<Work> {
    let up = text.to_ascii_uppercase();
    let words: Vec<&str> = up.split_whitespace().collect();
    let mut w = Work::default();
    if let Some(i) = words.iter().position(|x| x.trim_end_matches(['.', ',']) == "SYSTEM").filter(|i| *i > 0) {
        w.system = words.get(i - 1).map(|s| s.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string()).unwrap_or_default();
    }
    let boxy = ["JUNCTION BOX", "PULL BOX", "J-BOX", "BACK BOX", "BACKBOX"].iter().any(|b| up.contains(b));
    if boxy {
        w.box_in = words.iter().find_map(|x| dims(x));
    }
    // Each sentence naming conduit: its size (the word before `CONDUIT`) and how it runs.
    for sentence in up.split(". ") {
        let sw: Vec<&str> = sentence.split_whitespace().collect();
        for (i, x) in sw.iter().enumerate() {
            if !x.starts_with("CONDUIT") || i == 0 {
                continue;
            }
            let in_ground = buried(sentence);
            let size = match sw.get(i - 1).and_then(|s| super::gearbuild::trade_in(s.trim_end_matches(','))) {
                Some(v) => v,
                None if in_ground => {
                    w.size_assumed = true;
                    BURIED_MIN_IN
                }
                None => continue,
            };
            let route = if in_ground {
                w.buried = true;
                Route::UnderSlab
            } else if sentence.contains("BETWEEN") {
                Route::Riser
            } else if let Some(j) = sw.iter().position(|y| y.starts_with("STUB")) {
                // `STUBBED INTO CORRIDOR`: the room named after it.
                let room = sw
                    .iter()
                    .skip(j + 1)
                    .find(|y| !matches!(**y, "INTO" | "TO" | "IN" | "THE" | "UP" | "OUT"))
                    .map(|y| y.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string());
                Route::Stub(room.unwrap_or_default())
            } else {
                Route::ToRiser
            };
            if PULL_WORDS.iter().any(|p| sentence.contains(p)) && !w.pull_tape.contains(&(size, route.clone())) {
                w.pull_tape.push((size, route.clone()));
            }
            if route == Route::ToRiser {
                w.hard_lid |= HARD_LID_WORDS.iter().any(|p| sentence.contains(p));
                // The space named last after `OF` / `IN` / `ALONG`: `… PORTION OF CORRIDOR`.
                let named = sw
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .filter(|(_, y)| VIA_WORDS.contains(&y.trim_end_matches([',', '.'])))
                    .filter_map(|(j, _)| sw.iter().skip(j + 1).find(|y| !matches!(**y, "THE" | "A" | "AN" | "EACH")))
                    .map(|y| y.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string())
                    .rfind(|y| !y.is_empty() && !y.chars().any(|c| c.is_ascii_digit()));
                if named.is_some() {
                    w.via = named;
                }
            }
            if route == Route::Riser {
                w.riser = true;
            }
            if !w.conduits.contains(&(size, route.clone())) {
                w.conduits.push((size, route));
            }
        }
    }
    // Its box's role: the first sentence's words (`… CEILING MOUNTED ANTENNA LOCATION`, `… RISER
    // JUNCTION BOX`).
    let first = up.split(". ").next().unwrap_or_default();
    w.antenna = first.contains("ANTENNA");
    w.riser |= !w.antenna && first.contains("RISER");
    // The rating it keeps: `MAINTAIN 2-HOUR FIRE RATING OF FLOORS AND WALLS`.
    w.rating = up.split(". ").filter(|s| s.contains("RATING") || s.contains("RATED")).find_map(|s| {
        let hours = hours(s)?;
        let (floors, walls) = (s.contains("FLOOR"), s.contains("WALL"));
        Some(Rating { hours, floors: floors || !walls, walls: walls || !floors })
    });
    (w.box_in.is_some() || !w.conduits.is_empty()).then_some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(x: f64, y: f64, v: &str, h: f64) -> TextItem {
        TextItem { value: v.to_string(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
    }

    /// A sheet's keynotes (as E-3 prints them, its lines one text each) and the plan's tags.
    fn sheet() -> (Vec<TextItem>, Vec<(Vec2, Vec2)>) {
        let mut v = vec![
            t(1000.0, 1000.0, "ELECTRICAL KEYNOTES: (     ,     ,     )", 15.0),
            t(1000.0, 970.0, "3.", 9.0),
            t(1020.0, 970.0, "BDA SYSTEM CEILING MOUNTED ANTENNA LOCATION.", 9.0),
            t(1020.0, 958.0, "PROVIDE 8\"X8\"X6\" JUNCTION BOX AT LOCATION AS DIRECTED", 9.0),
            t(1020.0, 946.0, "BY THE BDA SYSTEM VENDOR. PROVIDE 1\" CONDUIT WITH", 9.0),
            t(1020.0, 934.0, "PULL TAPE ABOVE HARD LID PORTION OF CORRIDOR.", 9.0),
            t(1000.0, 922.0, "6.", 9.0),
            t(1020.0, 922.0, "PROVIDE POWER TO FIRE SMOKE DAMPER.", 9.0),
            t(1000.0, 910.0, "7.", 9.0),
            t(1020.0, 910.0, "BDA SYSTEM RISER JUNCTION BOX. JUNCTION BOX SHALL BE", 9.0),
            t(1020.0, 898.0, "12\"X12\"X6\". CONFIRM ALL REQUIREMENTS WITH BDA SYSTEM", 9.0),
            t(1020.0, 886.0, "VENDOR. PROVIDE 2\" CONDUIT BETWEEN EACH RISER", 9.0),
            t(1020.0, 874.0, "JUNCTION BOX. PROVIDE 1\" CONDUIT STUBBED INTO", 9.0),
            t(1020.0, 862.0, "CORRIDOR ABOVE CEILING.", 9.0),
            t(1000.0, 840.0, "8.", 9.0),
            t(1020.0, 840.0, "CIRCUIT TO FACTORY MOUNTED DISCONNECT SWITCH.", 9.0),
            // Beside the list on its lines: a title block's text.
            t(1700.0, 958.0, "300 Sample Street N, Ste 100", 9.0),
            // On the plan: 7 in a box, a 7 that is a circuit number (no box), 3 in a symbol box.
            t(200.0, 200.0, "7", 9.0),
            t(400.0, 200.0, "7", 9.0),
            t(410.0, 200.0, "A-4", 9.0),
            // A lone 8 as tall as the list's numbers (its box not kept); a taller 8 (a grid bubble's).
            t(900.0, 100.0, "8", 9.0),
            t(950.0, 100.0, "8", 14.0),
            // An equipment tag: FCU over 7 in a hexagon (its box test below).
            t(1500.0, 312.0, "FCU", 9.0),
            t(1503.0, 300.0, "7", 9.0),
            // A callout bubble's 3 (in its circle) over its sheet, 2.2 heights down.
            t(1200.0, 100.0, "3", 9.0),
            t(1196.0, 80.0, "E-6", 9.0),
            // A 6 in an elevator view (another list's subject), a 6 in a plan.
            t(2000.0, 500.0, "6", 9.0),
            t(1990.0, 400.0, "ELEVATOR SHAFT ELECTRICAL PLAN", 12.0),
            t(3000.0, 500.0, "6", 9.0),
            t(2990.0, 400.0, "ELECTRICAL - 2ND FLOOR PLAN", 12.0),
            // Dashed line glyphs read as text on the lone 8's line.
            t(700.0, 101.0, "'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'.'", 9.0),
            t(600.0, 300.0, "3", 9.0),
        ];
        v.push(t(800.0, 800.0, "SOME OTHER NOTE", 9.0));
        // The box around the first 7 (its middle at 202.7, 204.5).
        let (x0, y0, x1, y1) = (194.0, 195.0, 212.0, 214.0);
        let segs = vec![
            (Vec2::new(x0, y1), Vec2::new(x1, y1)),
            (Vec2::new(x0, y0), Vec2::new(x1, y0)),
            (Vec2::new(x0, y0), Vec2::new(x0, y1)),
            (Vec2::new(x1, y0), Vec2::new(x1, y1)),
            // The hexagon around FCU / 7: flat top and bottom, slanted sides.
            (Vec2::new(1495.0, 325.0), Vec2::new(1520.0, 325.0)),
            (Vec2::new(1495.0, 296.0), Vec2::new(1520.0, 296.0)),
            (Vec2::new(1495.0, 325.0), Vec2::new(1488.0, 310.0)),
            (Vec2::new(1488.0, 310.0), Vec2::new(1495.0, 296.0)),
            (Vec2::new(1520.0, 325.0), Vec2::new(1527.0, 310.0)),
            (Vec2::new(1527.0, 310.0), Vec2::new(1520.0, 296.0)),
        ];
        (v, segs)
    }

    #[test]
    fn a_keynote_list_gives_its_numbered_notes_whole() {
        let (texts, _) = sheet();
        let l = lists(&texts);
        assert_eq!(l.len(), 1);
        let n: Vec<&str> = l[0].notes.iter().map(|n| n.number.as_str()).collect();
        assert_eq!(n, ["3", "6", "7", "8"]);
        assert!(l[0].notes[2].text.ends_with("CORRIDOR ABOVE CEILING."), "{:?}", l[0].notes[1]);
        assert!(!l[0].notes[0].text.contains("Street"), "a title block's text beside it is no note's");
    }

    #[test]
    fn a_tag_is_its_number_in_a_box_and_a_bare_number_is_not() {
        let (texts, segs) = sheet();
        let l = lists(&texts).remove(0);
        let boxes = [[596.0, 295.0, 614.0, 313.0]];
        let got = tags(&texts, &segs, &boxes, &[], &["ELEVATOR".to_string()], &l);
        let mut numbers: Vec<(&str, i64)> = got.iter().map(|(n, at)| (n.as_str(), at[0] as i64)).collect();
        numbers.sort();
        assert_eq!(numbers, [("3", 602), ("6", 3002), ("7", 202), ("8", 902)], "{got:?}");
    }

    #[test]
    fn a_notes_words_say_its_box_and_its_conduit() {
        let (texts, _) = sheet();
        let l = lists(&texts).remove(0);
        let riser = work(&l.notes[2].text).expect("riser");
        assert_eq!(riser.system, "BDA");
        assert_eq!(riser.box_in, Some([12.0, 12.0, 6.0]));
        assert_eq!(riser.conduits, [(2.0, Route::Riser), (1.0, Route::Stub("CORRIDOR".into()))]);
        let antenna = work(&l.notes[0].text).expect("antenna");
        assert_eq!((antenna.box_in, antenna.conduits.as_slice()), (Some([8.0, 8.0, 6.0]), [(1.0, Route::ToRiser)].as_slice()));
        assert_eq!(work(&l.notes[3].text), None, "a note that builds nothing it can read");
        assert!(!riser.buried);
        // Conduit under the slab, its size not given: the smallest trade size, assumed.
        let floor_box = work("PROVIDE UNDER SLAB CONDUIT FOR FLOOR BOX RECEPTACLE (HUBBLE OR EQUAL). VERIFY PARTS.").expect("floor box");
        assert_eq!(floor_box.conduits, [(BURIED_MIN_IN, Route::UnderSlab)]);
        assert!(floor_box.buried && floor_box.size_assumed);
        let sized = work("PROVIDE 1\" CONDUIT BELOW GRADE TO THE SIGN.").expect("sized");
        assert_eq!((sized.conduits.as_slice(), sized.size_assumed), ([(1.0, Route::UnderSlab)].as_slice(), false));
        assert!(buried("MIN. 48\" BELOW GRADE") && !buried("MICROWAVE BELOW"));
    }

    #[test]
    fn a_notes_words_say_its_box_kind_pull_tape_route_and_rating() {
        // The list's own wording: pull tape above the corridor's hard lid, the wall's rating kept.
        let (texts, _) = sheet();
        let l = lists(&texts).remove(0);
        let antenna = work(&l.notes[0].text).expect("antenna");
        assert!(antenna.antenna && !antenna.riser && antenna.hard_lid, "{antenna:?}");
        assert_eq!((antenna.box_kind().as_str(), antenna.via.as_deref()), ("bda_antenna_box", Some("CORRIDOR")));
        assert_eq!(antenna.pull_tape, [(1.0, Route::ToRiser)]);
        let riser = work(&l.notes[2].text).expect("riser");
        assert_eq!((riser.box_kind().as_str(), riser.pull_tape.len(), riser.rating.clone()), ("bda_riser_box", 0, None));
        // Other wordings: a pull string along a gypsum-lidded corridor, a 1-hour wall rating
        // in words; a riser box of another system, its floors 2 HR; a plain box, no system.
        let a = work("BDA SYSTEM CEILING ANTENNA. PROVIDE 6\"X6\"X4\" JUNCTION BOX WHERE THE SYSTEM VENDOR DIRECTS. RUN 3/4\" CONDUIT WITH PULL STRING ABOVE THE GYPSUM CEILING ALONG THE CORRIDOR. KEEP THE ONE-HOUR RATING OF WALLS.").expect("a");
        assert_eq!(
            (a.box_in, a.pull_tape.as_slice(), a.via.as_deref(), a.hard_lid),
            (Some([6.0, 6.0, 4.0]), [(0.75, Route::ToRiser)].as_slice(), Some("CORRIDOR"), true)
        );
        assert_eq!(a.rating, Some(Rating { hours: 1.0, floors: false, walls: true }));
        let r =
            work("DAS SYSTEM RISER JUNCTION BOX 10\"X10\"X4\". PROVIDE 1-1/4\" CONDUIT BETWEEN EACH RISER BOX. MAINTAIN 2 HR FIRE RATING OF FLOORS.")
                .expect("r");
        assert_eq!((r.system.as_str(), r.box_kind().as_str(), r.conduits.as_slice()), ("DAS", "das_riser_box", [(1.25, Route::Riser)].as_slice()));
        assert_eq!(r.rating, Some(Rating { hours: 2.0, floors: true, walls: false }));
        let plain = work("PROVIDE 4\"X4\"X2\" JUNCTION BOX FOR THE DOOR OPERATOR. KEEP THE (2) HOUR RATED ASSEMBLY.").expect("plain");
        assert_eq!((plain.box_kind().as_str(), plain.rating.clone()), ("junction_box", Some(Rating { hours: 2.0, floors: true, walls: true })));
        assert_eq!(Rating { hours: 2.0, floors: true, walls: true }.words(), "2-hour");
    }

    #[test]
    fn a_systems_power_and_parts_are_named_by_whole_words() {
        let bda = system_rules("bda");
        let parts: Vec<&str> = bda.parts.iter().map(|p| p.part.as_str()).collect();
        assert_eq!(parts, ["head_end", "donor_antenna"]);
        assert!(!bda.source.is_empty(), "rules data cite their source");
        assert!(bda.named_in("BDA SYSTEM") && bda.named_in("ERRCS PANEL") && !bda.named_in("ABDAL PUMP"));
        assert!(bda.parts[1].named_in("RUN IT TO THE ROOF.") && !bda.parts[1].named_in("ROOFTOP UNIT"));
        let c = |n: &str, d: &str| PanelCircuit { number: n.into(), description: d.into(), ..Default::default() };
        let schedules = vec![
            PanelSchedule { panel: "L1".into(), circuits: vec![c("1", "ABDAL PUMP"), c("3", "RCPT - LOBBY")], ..Default::default() },
            PanelSchedule { panel: "H2".into(), circuits: vec![c("12", "ERRCS / BDA HEAD-END")], ..Default::default() },
        ];
        let fed: Vec<(&str, &str)> = system_circuits(&bda, &schedules).into_iter().map(|(p, c)| (p.panel.as_str(), c.number.as_str())).collect();
        assert_eq!(fed, [("H2", "12")]);
        // A system the rules data doesn't know: its name, no parts to look for.
        let other = system_rules("Paging");
        assert_eq!((other.name.as_str(), other.parts.len()), ("PAGING", 0));
    }

    #[test]
    fn a_sheets_buried_work_notes_are_read_whole() {
        // A site plan's callout, its lines one text each, beside another callout; nothing buried in that one.
        let texts = vec![
            t(100.0, 500.0, "TWO (2) 5\" SCH. 40 PVC CONDUIT", 6.0),
            t(100.0, 492.0, "WITH STEEL ELLS AND RISERS,", 6.0),
            t(100.0, 484.0, "MIN. 48\" BELOW GRADE WITH", 6.0),
            t(100.0, 476.0, "PULLWIRE FOR PRIMARY", 6.0),
            t(100.0, 468.0, "CONDUCTORS.", 6.0),
            t(300.0, 500.0, "UTILITY CO. PAD MOUNTED", 6.0),
            t(300.0, 492.0, "TRANSFORMER.", 6.0),
        ];
        let notes = buried_notes(&texts);
        assert_eq!(
            notes,
            vec![(
                None,
                "TWO (2) 5\" SCH. 40 PVC CONDUIT WITH STEEL ELLS AND RISERS, MIN. 48\" BELOW GRADE WITH PULLWIRE FOR PRIMARY CONDUCTORS.".to_string()
            )]
        );
    }
}
