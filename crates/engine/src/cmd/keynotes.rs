//! Keynotes as the sheets print them: a `KEYNOTES` heading (`ELECTRICAL KEYNOTES:`), numbered
//! notes under it (`3. BDA SYSTEM CEILING MOUNTED ANTENNA LOCATION. PROVIDE 8"X8"X6" JUNCTION
//! BOX …`), and on the plans each note's number in a small box (a square, or a hexagon) where it
//! applies. What a note asks to be built is read from its words ([`work`]): a box of a size, and
//! conduit of a size between each of its boxes (a riser), stubbed into a space, or run to its
//! system's riser.

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;

/// Most notes a list holds, lists a sheet holds, tags a sheet holds (hostile files).
const MAX_NOTES: usize = 60;
const MAX_LISTS: usize = 12;
const MAX_TAGS: usize = 2000;
/// How far below its heading a list's notes run, in the heading's text heights.
const LIST_REACH: f64 = 120.0;
/// A note's lines start this close to its number's column, in its text heights (text further
/// right on its line is something else: a title block, a logo's address).
const NOTE_INDENT: f64 = 4.0;

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
    let flat: Vec<&TextItem> =
        texts.iter().filter(|t| t.rotation.abs() < 0.05 && t.height > 0.0 && t.at.x.is_finite() && t.at.y.is_finite()).collect();
    let mut out = Vec::new();
    for head in flat.iter().filter(|t| {
        let u = t.value.to_ascii_uppercase();
        u.contains("KEYNOTES") || u.contains("KEYED NOTES")
    }) {
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
pub(crate) fn tags(texts: &[TextItem], segs: &[(Vec2, Vec2)], boxes: &[[f64; 4]], list: &List) -> Vec<(String, [f64; 2])> {
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
        let in_box = boxes.iter().any(|b| c.x > b[0] && c.x < b[2] && c.y > b[1] && c.y < b[3] && b[2] - b[0] < 5.0 * h && b[3] - b[1] < 5.0 * h);
        let as_tall = (h - list.number_h).abs() <= LONE_HEIGHT * list.number_h;
        // Beside it on its line, or right above or below it (a callout bubble's `3` over `E-6`):
        // a real text (mostly letters and digits: a dashed line's glyphs read as text aren't).
        let lone = as_tall
            && !texts.iter().any(|o| {
                let ov = o.value.trim();
                let alnum = ov.chars().filter(char::is_ascii_alphanumeric).count();
                let real = alnum > 0 && 2 * alnum >= ov.chars().count();
                let oh = o.height.min(2.0 * h);
                !std::ptr::eq(o, t)
                    && real
                    && (o.at.y - t.at.y).abs() < 2.4 * h
                    && o.at.x < t.at.x + (0.7 * v.len() as f64 + 1.0) * h
                    && o.at.x + 0.7 * ov.len() as f64 * oh > t.at.x - 1.0 * h
            });
        if in_box || lone || boxed(c, h, segs) {
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
        } else if crosses_y && (x - c.x).abs() > 0.4 * h && (x - c.x).abs() < 2.5 * h {
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
            let Some(size) = sw.get(i - 1).and_then(|s| super::gearbuild::trade_in(s.trim_end_matches(','))) else { continue };
            let route = if sentence.contains("BETWEEN") {
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
            if !w.conduits.contains(&(size, route.clone())) {
                w.conduits.push((size, route));
            }
        }
    }
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
            t(1000.0, 910.0, "7.", 9.0),
            t(1020.0, 910.0, "BDA SYSTEM RISER JUNCTION BOX. JUNCTION BOX SHALL BE", 9.0),
            t(1020.0, 898.0, "12\"X12\"X6\". CONFIRM ALL REQUIREMENTS WITH BDA SYSTEM", 9.0),
            t(1020.0, 886.0, "VENDOR. PROVIDE 2\" CONDUIT BETWEEN EACH RISER", 9.0),
            t(1020.0, 874.0, "JUNCTION BOX. PROVIDE 1\" CONDUIT STUBBED INTO", 9.0),
            t(1020.0, 862.0, "CORRIDOR ABOVE CEILING.", 9.0),
            t(1000.0, 840.0, "8.", 9.0),
            t(1020.0, 840.0, "CIRCUIT TO FACTORY MOUNTED DISCONNECT SWITCH.", 9.0),
            // Beside the list on its lines: a title block's text.
            t(1700.0, 958.0, "300 20th Street N, Ste 100", 9.0),
            // On the plan: 7 in a box, a 7 that is a circuit number (no box), 3 in a symbol box.
            t(200.0, 200.0, "7", 9.0),
            t(400.0, 200.0, "7", 9.0),
            t(410.0, 200.0, "A-4", 9.0),
            // A lone 8 as tall as the list's numbers (its box not kept); a taller 8 (a grid bubble's).
            t(900.0, 100.0, "8", 9.0),
            t(950.0, 100.0, "8", 14.0),
            // A callout bubble's 3 over its sheet.
            t(1200.0, 100.0, "3", 9.0),
            t(1196.0, 88.0, "E-6", 9.0),
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
        ];
        (v, segs)
    }

    #[test]
    fn a_keynote_list_gives_its_numbered_notes_whole() {
        let (texts, _) = sheet();
        let l = lists(&texts);
        assert_eq!(l.len(), 1);
        let n: Vec<&str> = l[0].notes.iter().map(|n| n.number.as_str()).collect();
        assert_eq!(n, ["3", "7", "8"]);
        assert!(l[0].notes[1].text.ends_with("CORRIDOR ABOVE CEILING."), "{:?}", l[0].notes[1]);
        assert!(!l[0].notes[0].text.contains("Street"), "a title block's text beside it is no note's");
    }

    #[test]
    fn a_tag_is_its_number_in_a_box_and_a_bare_number_is_not() {
        let (texts, segs) = sheet();
        let l = lists(&texts).remove(0);
        let boxes = [[596.0, 295.0, 614.0, 313.0]];
        let got = tags(&texts, &segs, &boxes, &l);
        let numbers: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(numbers, ["7", "8", "3"], "{got:?}");
    }

    #[test]
    fn a_notes_words_say_its_box_and_its_conduit() {
        let (texts, _) = sheet();
        let l = lists(&texts).remove(0);
        let riser = work(&l.notes[1].text).expect("riser");
        assert_eq!(riser.system, "BDA");
        assert_eq!(riser.box_in, Some([12.0, 12.0, 6.0]));
        assert_eq!(riser.conduits, [(2.0, Route::Riser), (1.0, Route::Stub("CORRIDOR".into()))]);
        let antenna = work(&l.notes[0].text).expect("antenna");
        assert_eq!((antenna.box_in, antenna.conduits.as_slice()), (Some([8.0, 8.0, 6.0]), [(1.0, Route::ToRiser)].as_slice()));
        assert_eq!(work(&l.notes[2].text), None, "a note that builds nothing it can read");
    }
}
