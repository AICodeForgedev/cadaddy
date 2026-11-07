//! The documents that decide an area's heights, types and ratings, for its evidence packet
//! (Q6261, [`super::validate_packet`]).
//!
//! A judge can only check what it can see. A packet carries, as `deciding` pictures that the
//! picture cap (`max_images`) never drops:
//! - every sheet the area's prescreen findings cite ([`cited`]): a finding names a sheet in a
//!   `sheet` / `source_sheet` / `document` field (with the point it read, `at` / `sheet_at`, when
//!   it has one), as the keys of `by_sheet`, or in its words after "on", "sheet", "note", "see",
//!   "from", "per" ("note E-001 general note", "44\" AFF on A-501"). The crop is round the cited
//!   points; a sheet cited with no point is shown whole (or, when the packet already shows it,
//!   those pictures are marked);
//! - every schedule, legend and notes sheet of the area's trades ([`sheets`]), whole: the set's
//!   sheet links and the job's imported sheets that are no plan (the job keeps their rectangles,
//!   `sheet_regions`), named by their titles (the job's sheet index) or the kind the job gave
//!   them;
//! - the keynote lists of the sheets that mark the area, the gear's schedules and riser, and the
//!   specification and submittal pages the area's product records cite.
//!
//! The packet also states the area's ceiling height and where it comes from ([`ceiling`]): the
//! reflected ceiling plan over the area, else the room's record (its source), else the level's
//! height (which no document read for this room gives).

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_geom::Vec2;
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

use super::validate::Frame;
use super::validate_packet::{Area, inside, norm_rect, point_in, r2, sheet_kind, trade_of_discipline};

/// Sheets the prescreen's findings may cite in one packet, and the points kept per sheet.
const MAX_CITED: usize = 60;
const MAX_POINTS: usize = 200;
/// How deep a finding's JSON is walked, and the strings read in one finding.
const MAX_DEPTH: usize = 8;
const MAX_STRINGS: usize = 400;
/// Schedule, legend and notes sheets one packet shows whole, at most.
const MAX_SHEETS: usize = 40;

/// The kinds of sheet that decide what an element is and where it goes (a sheet's kind as
/// [`sheet_kind`] reads it off its title).
pub(crate) const DECIDING_KINDS: [&str; 5] = ["schedule", "legend", "general_notes", "spec", "partition_legend"];

/// The words that, just before a sheet number in a finding's text, make it a citation of that
/// sheet ("note E-001 general note", "on A-501", "see P-001").
const CITE_WORDS: [&str; 9] = ["ON", "SHEET", "SHEETS", "NOTE", "NOTES", "SEE", "FROM", "PER", "OF"];

/// A sheet the area's prescreen cites: the rules citing it and the sheet points they read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Cites {
    pub rules: BTreeSet<String>,
    pub points: Vec<[f64; 2]>,
    pub findings: usize,
}

/// The sheet numbers a packet can name: upper case → as the set writes it.
pub(crate) fn numbers<'a>(all: impl Iterator<Item = &'a str>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for n in all.map(str::trim).filter(|n| !n.is_empty() && n.len() <= 40 && n.chars().any(|c| c.is_ascii_digit())) {
        out.entry(n.to_ascii_uppercase()).or_insert_with(|| n.to_string());
    }
    out
}

fn point(v: Option<&Value>) -> Option<[f64; 2]> {
    let a = v?.as_array()?;
    let (x, y) = (a.first()?.as_f64()?, a.get(1)?.as_f64()?);
    (a.len() == 2 && x.is_finite() && y.is_finite()).then_some([x, y])
}

/// The words of a text, upper case, split at spaces and punctuation (a sheet number keeps its
/// `-`, `.` and `#`).
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '(' | ')' | '[' | ']' | '"' | '\'' | '/' | '='))
        .map(|w| w.trim_end_matches('.').to_ascii_uppercase())
        .filter(|w| !w.is_empty())
        .collect()
}

struct Walk<'a> {
    numbers: &'a BTreeMap<String, String>,
    found: Vec<(String, Option<[f64; 2]>)>,
    strings: usize,
}

impl Walk<'_> {
    fn sheet(&self, s: &str) -> Option<String> {
        self.numbers.get(&s.trim().to_ascii_uppercase()).cloned()
    }

    fn text(&mut self, s: &str) {
        if self.strings >= MAX_STRINGS {
            return;
        }
        self.strings += 1;
        let w = words(s);
        for (i, t) in w.iter().enumerate() {
            let Some(n) = self.numbers.get(t) else { continue };
            if i > 0 && w.get(i - 1).is_some_and(|p| CITE_WORDS.contains(&p.as_str())) {
                self.found.push((n.clone(), None));
            }
        }
    }

    fn value(&mut self, v: &Value, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match v {
            Value::Object(o) => {
                for key in ["sheet", "source_sheet", "document"] {
                    if let Some(n) = o.get(key).and_then(Value::as_str).and_then(|s| self.sheet(s)) {
                        let at = point(o.get("sheet_at")).or_else(|| point(o.get("at")));
                        self.found.push((n, at));
                    }
                }
                if let Some(Value::Object(by)) = o.get("by_sheet") {
                    for k in by.keys().take(50) {
                        if let Some(n) = self.sheet(k) {
                            self.found.push((n, None));
                        }
                    }
                }
                for (k, x) in o.iter().take(200) {
                    // A sheet field is read above; its words are the sheet number itself.
                    if !matches!(k.as_str(), "sheet" | "source_sheet" | "document") {
                        self.value(x, depth + 1);
                    }
                }
            }
            Value::Array(a) => a.iter().take(200).for_each(|x| self.value(x, depth + 1)),
            Value::String(s) => self.text(s),
            _ => {}
        }
    }
}

/// The sheets the prescreen's findings cite (`numbers`: the sheets the set has), with the rules
/// citing each and the sheet points they read (a point off its sheet, `rects`, is dropped: it
/// is no sheet point).
pub(crate) fn cited<'v>(
    findings: impl Iterator<Item = &'v Value>,
    numbers: &BTreeMap<String, String>,
    rects: &dyn Fn(&str) -> Vec<[f64; 4]>,
) -> BTreeMap<String, Cites> {
    let mut out: BTreeMap<String, Cites> = BTreeMap::new();
    for f in findings {
        let mut w = Walk { numbers, found: Vec::new(), strings: 0 };
        w.value(f, 0);
        let rule = f.get("rule").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut here: BTreeSet<String> = BTreeSet::new();
        for (n, at) in w.found {
            if out.len() >= MAX_CITED && !out.contains_key(&n) {
                continue;
            }
            let c = out.entry(n.clone()).or_default();
            if !rule.is_empty() {
                c.rules.insert(rule.clone());
            }
            if let Some(p) = at.filter(|p| rects(&n).iter().any(|r| inside(*r, *p)))
                && c.points.len() < MAX_POINTS
                && !c.points.iter().any(|q| (q[0] - p[0]).abs() < 1e-6 && (q[1] - p[1]).abs() < 1e-6)
            {
                c.points.push(p);
            }
            here.insert(n);
        }
        for n in here {
            if let Some(c) = out.get_mut(&n) {
                c.findings += 1;
            }
        }
    }
    out
}

/// A schedule, legend or notes sheet a packet shows whole.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DocSheet {
    pub number: String,
    pub title: String,
    pub kind: &'static str,
    pub trade: &'static str,
    /// Its drawing area (off its title block when the job found one), drawing units; none for a
    /// sheet the drawing doesn't hold (shown as its PDF page).
    pub rect: Option<[f64; 4]>,
}

/// The kind of a sheet that decides (a schedule, a legend, notes, specifications), from its
/// title words, else from the kind the job gave it.
pub(crate) fn deciding_kind(title: &str, job_kind: &str) -> Option<&'static str> {
    let k = sheet_kind(title, false);
    if let Some(k) = DECIDING_KINDS.iter().find(|d| **d == k) {
        return Some(k);
    }
    let j = job_kind.trim().to_ascii_lowercase();
    DECIDING_KINDS.iter().find(|d| **d == j.as_str()).copied()
}

/// One sheet as a packet may show it: number, title words, the kind the job gave it, discipline,
/// rectangle, and whether it is a plan of a level.
pub(crate) struct SheetOf {
    pub number: String,
    pub title: String,
    pub job_kind: String,
    pub discipline: String,
    /// Its rectangle in the drawing; none: only the job's sheet index has it (its PDF page).
    pub rect: Option<[f64; 4]>,
    pub has_level: bool,
}

/// The schedule, legend and notes sheets of `trades` (and the set's general ones and the
/// architectural ones) among `all`, one per number, in sheet order. A plan of a level is never one, whatever its
/// title says beside it (a plan sheet's general notes come in with the plan).
pub(crate) fn sheets(all: &[SheetOf], trades: &BTreeSet<String>) -> (Vec<DocSheet>, Vec<(String, String)>) {
    let mut out: Vec<DocSheet> = Vec::new();
    let mut over: Vec<(String, String)> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut list: Vec<&SheetOf> = all.iter().filter(|s| !s.has_level && s.rect.is_none_or(|r| r.iter().all(|v| v.is_finite()))).collect();
    // A sheet in the drawing before its PDF page alone, then by number.
    list.sort_by(|a, b| a.number.cmp(&b.number).then(a.rect.is_none().cmp(&b.rect.is_none())));
    for s in list {
        let Some(kind) = deciding_kind(&s.title, &s.job_kind) else { continue };
        let trade = trade_of_discipline(&s.discipline);
        // A general sheet is the set's own (G, or no discipline): a structural, civil or
        // landscape sheet is no trade of a room's.
        let general = trade == "general" && matches!(s.discipline.trim().to_ascii_uppercase().as_str(), "" | "G" | "GI" | "GN" | "GEN");
        if !((trade != "general" && trades.contains(trade)) || trade == "architecture" || general) || !seen.insert(s.number.to_ascii_uppercase()) {
            continue;
        }
        if out.len() >= MAX_SHEETS {
            over.push((format!("sheet {}", s.number), format!("over the packet's {MAX_SHEETS} schedule, legend and notes sheets")));
            continue;
        }
        out.push(DocSheet { number: s.number.clone(), title: s.title.clone(), kind, trade, rect: s.rect.map(norm_rect) });
    }
    (out, over)
}

/// The area's ceiling height above its floor and where it comes from: the reflected ceiling
/// plan's ceiling over the area's middle, else the room's record (the sheet it was read on),
/// else the level's ceiling (no document read gives this room's). The other heights over the
/// area (soffits, other ceiling areas) are listed with it.
pub(crate) fn ceiling(sp: &BuildingSpec, frame: &Frame, area: &Area) -> Value {
    let middle = {
        let c = [(area.bbox[0] + area.bbox[2]) / 2.0, (area.bbox[1] + area.bbox[3]) / 2.0];
        if area.outline.len() < 3 || point_in(&area.outline, c) {
            c
        } else {
            // A middle off an L-shaped room: its outline's first corner nudged inside.
            area.outline.iter().map(|p| [p[0] * 0.9 + c[0] * 0.1, p[1] * 0.9 + c[1] * 0.1]).find(|p| point_in(&area.outline, *p)).unwrap_or(c)
        }
    };
    let level = area.level.as_str();
    let on_ft = |o: &[[f64; 2]]| -> Vec<[f64; 2]> { o.iter().take(10_000).map(|p| frame.to_ft_on(level, Vec2::new(p[0], p[1]))).collect() };
    let touches = |o: &[[f64; 2]]| o.iter().any(|p| inside(area.bbox, *p)) || area.outline.iter().any(|p| point_in(o, *p));
    let mut also: Vec<Value> = Vec::new();
    let mut rcp: Option<Value> = None;
    for c in sp.rcp.ceilings.iter().filter(|c| c.level.eq_ignore_ascii_case(level) && c.outline.len() >= 3).take(5_000) {
        if !(c.height_ft.is_finite() && c.height_ft > 0.0) {
            continue;
        }
        let o = on_ft(&c.outline);
        let source = c.source.clone().unwrap_or_else(|| "the reflected ceiling plan".into());
        if rcp.is_none() && point_in(&o, middle) {
            rcp = Some(json!({"height_ft": r2(c.height_ft), "source": source, "type": c.ceiling_type, "room": c.room}));
        } else if touches(&o) && also.len() < 12 {
            also.push(json!({"what": "ceiling", "height_ft": r2(c.height_ft), "source": source, "type": c.ceiling_type}));
        }
    }
    for s in sp.rcp.soffits.iter().filter(|s| s.level.eq_ignore_ascii_case(level) && s.outline.len() >= 3).take(5_000) {
        let o = on_ft(&s.outline);
        if s.height_ft.is_finite() && s.height_ft > 0.0 && touches(&o) && also.len() < 12 {
            also.push(
                json!({"what": "soffit", "height_ft": r2(s.height_ft), "type": s.soffit_type, "source": "the reflected ceiling plan (soffit)"}),
            );
        }
    }
    let names = |r: &&cadaddy_walk3d::SpecRoom| {
        let num = r.number.as_deref().map(str::trim).filter(|n| !n.is_empty());
        let same_level = r.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(level));
        same_level
            && match (num, area.number.as_deref()) {
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b.trim()),
                (None, _) => area.name.as_deref().is_some_and(|n| !r.name.trim().is_empty() && n.trim().eq_ignore_ascii_case(r.name.trim())),
                _ => false,
            }
    };
    let room = sp.rooms.iter().take(50_000).find(|r| names(r) && r.ceiling_ft.is_some_and(|h| h.is_finite() && h > 0.0));
    let room_json = room.map(|r| {
        json!({"height_ft": r2(r.ceiling_ft.unwrap_or_default()), "source": r.ceiling_source.clone().unwrap_or_else(|| format!("room {}'s record", r.number.as_deref().unwrap_or(&r.name)))})
    });
    let level_ft = sp.level(level).map(|l| l.ceiling_ft).filter(|h| h.is_finite() && *h > 0.0);
    let (from, chosen) = match (&rcp, &room_json, level_ft) {
        (Some(c), _, _) => ("rcp", Some(c.clone())),
        (None, Some(r), _) => ("room", Some(r.clone())),
        (None, None, Some(h)) => (
            "level",
            Some(
                json!({"height_ft": r2(h), "source": format!("level {level}'s ceiling height: no reflected ceiling plan or room record read gives this room's")}),
            ),
        ),
        (None, None, None) => ("none", None),
    };
    let Some(c) = chosen else {
        return json!({"from": "none", "source": "no reflected ceiling plan, room record or level gives a ceiling height here", "also": also});
    };
    let h = c["height_ft"].as_f64().unwrap_or_default();
    let mut out = json!({
        "height_ft": r2(h),
        "height_in": r2(h * 12.0),
        "from": from,
        "source": c["source"],
        "also": also,
    });
    if let Some(t) = c.get("type").filter(|t| !t.is_null()) {
        out["type"] = t.clone();
    }
    // A room record that disagrees with the ceiling plan: both, so a judge sees it.
    if from == "rcp"
        && let Some(r) = &room_json
        && (r["height_ft"].as_f64().unwrap_or(h) - h).abs() > 0.05
    {
        out["room_record"] = r.clone();
    }
    out
}

/// A height in feet and inches (`7'-10"`).
pub(crate) fn feet_inches(ft: f64) -> String {
    let total = (ft * 12.0).round();
    let f = (total / 12.0).floor();
    let i = total - f * 12.0;
    format!("{f:.0}'-{i:.0}\"")
}

/// The packet prompt's section on its deciding documents and the ceiling height.
pub(crate) fn prompt_section(deciding: &[(String, String)], ceiling: &Value, not_shown: &[(String, String)]) -> String {
    let mut s = String::from("## The documents that decide\n\n");
    s.push_str("The pictures below decide heights, types and ratings: the sheets the prescreen cites, the schedules, legends and notes of this area's trades (whole), the keynote lists, and the specification and submittal pages. They are evidence like any print picture: cite them (a specification or submittal page too) for a finding they settle.\n\n");
    match ceiling["height_ft"].as_f64() {
        Some(h) => s.push_str(&format!(
            "Ceiling height here: {} ({} in) above the floor, from {}. Measure heights against it; never scale a ceiling height off a picture.{}\n\n",
            feet_inches(h),
            r2(h * 12.0),
            ceiling["source"].as_str().unwrap_or_default(),
            match ceiling["room_record"]["height_ft"].as_f64() {
                Some(r) => format!(
                    " The room's record says {} ({}): where the two disagree, say so.",
                    feet_inches(r),
                    ceiling["room_record"]["source"].as_str().unwrap_or_default()
                ),
                None => String::new(),
            }
        )),
        None => s.push_str(
            "Ceiling height here: no document in this job gives it. Say UNSURE for what it would decide; never scale it off a picture.\n\n",
        ),
    }
    if deciding.is_empty() {
        s.push_str("No deciding document of this area was found in the job.\n");
    }
    for (file, why) in deciding.iter().take(300) {
        s.push_str(&format!("- `{file}`: {why}\n"));
    }
    if !not_shown.is_empty() {
        s.push_str("\nCited or needed, but not in this packet (say UNSURE for what they would decide):\n");
        for (what, why) in not_shown.iter().take(60) {
            s.push_str(&format!("- {what}: {why}\n"));
        }
    }
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nums() -> BTreeMap<String, String> {
        numbers(["E-001", "E-101", "A-501", "P-001", "G-002", "M-201"].into_iter())
    }

    #[test]
    fn a_finding_cites_sheets_by_field_by_key_and_by_its_words() {
        let f = [
            json!({"rule": "G-HEIGHT", "expected_source": {"sheet": "E-101", "at": [5.0, 6.0], "what": "its record: note E-001 general note: MOUNT OUTLETS AT 18 IN"}}),
            json!({"rule": "G-PRESENT", "measured": {"by_sheet": {"M-201": 4}, "text": "44\" AFF on A-501; sink height on G-002"}}),
            // A circuit tag is no citation; nor is a sheet number with no word before it.
            json!({"rule": "E-SWITCH-LEG", "measured": {"circuit": "E-101", "tag": "P-001 E-101"}}),
            // A point off its sheet is no sheet point.
            json!({"rule": "G-HOST", "measured": {"sheet": "A-501", "sheet_at": [999.0, 999.0]}}),
        ];
        let rects = |n: &str| if n == "E-101" { vec![[0.0, 0.0, 10.0, 10.0]] } else { vec![[0.0, 0.0, 100.0, 100.0]] };
        let c = cited(f.iter(), &nums(), &rects);
        assert_eq!(c.keys().cloned().collect::<Vec<_>>(), vec!["A-501", "E-001", "E-101", "G-002", "M-201"], "{c:?}");
        assert_eq!(c["E-101"].points, vec![[5.0, 6.0]]);
        assert_eq!(c["E-101"].rules.iter().cloned().collect::<Vec<_>>(), vec!["G-HEIGHT"]);
        assert!(c["E-001"].points.is_empty() && c["E-001"].rules.contains("G-HEIGHT"));
        assert!(c["A-501"].points.is_empty() && c["A-501"].rules.contains("G-HOST") && c["A-501"].rules.contains("G-PRESENT"), "{c:?}");
        assert!(!c.contains_key("P-001"), "a tag is no citation: {c:?}");
    }

    #[test]
    fn deciding_sheets_are_schedules_legends_and_notes_of_the_trades_present() {
        let s = |n: &str, t: &str, k: &str, d: &str, lv: bool| SheetOf {
            number: n.into(),
            title: t.into(),
            job_kind: k.into(),
            discipline: d.into(),
            rect: Some([0.0, 0.0, 42.0, 30.0]),
            has_level: lv,
        };
        let all = vec![
            s("E-001", "ELECTRICAL SYMBOLS LEGEND AND GENERAL NOTES", "cover", "E", false),
            s("E-901", "PANEL SCHEDULES", "schedule", "E", false),
            s("M-601", "MECHANICAL SCHEDULES", "schedule", "M", false),
            s("FP-001", "FIRE PROTECTION NOTES", "", "FP", false),
            s("A-601", "", "schedule", "A", false),
            s("E-101", "FIRST FLOOR POWER PLAN AND GENERAL NOTES", "plan", "E", true),
            s("A-700", "DETAILS", "detail", "A", false),
            s("G-002", "GENERAL NOTES", "", "G", false),
            s("S-001", "STRUCTURAL SCHEDULES", "schedule", "S", false),
            // Only in the job's sheet index: shown as its PDF page.
            SheetOf { rect: None, ..s("A-000", "SYMBOLS AND ABBREVIATIONS", "", "A", false) },
        ];
        // An element of no trade (`general`) brings in no structural sheet.
        let trades: BTreeSet<String> = ["electrical", "architecture", "general"].into_iter().map(str::to_string).collect();
        let (got, over) = sheets(&all, &trades);
        assert!(got.iter().find(|d| d.number == "A-000").is_some_and(|d| d.rect.is_none()), "{got:?}");
        let got: Vec<(&str, &str)> = got.iter().map(|d| (d.number.as_str(), d.kind)).collect();
        assert_eq!(got, vec![("A-000", "legend"), ("A-601", "schedule"), ("E-001", "legend"), ("E-901", "schedule"), ("G-002", "general_notes")]);
        assert!(over.is_empty());
    }

    #[test]
    fn heights_read_in_feet_and_inches() {
        assert_eq!(feet_inches(7.0 + 10.0 / 12.0), "7'-10\"");
        assert_eq!(feet_inches(8.5), "8'-6\"");
        assert_eq!(feet_inches(9.0), "9'-0\"");
    }
}
