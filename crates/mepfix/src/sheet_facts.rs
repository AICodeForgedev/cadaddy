//! Sheet facts read from a plotted sheet's own text: which sheet it is (the title block's sheet
//! number, title and discipline), the views drawn on it (each view title's number, title and
//! scale, and the region of the sheet the view occupies) and a tag census of a region (how many
//! of each device/fixture type tag and circuit label it holds).
//!
//! What it does comes from reading a real plotted construction set:
//! - A PDF named for one discipline can hold others: an "electrical" set carried architectural
//!   enlarged plans, low-voltage and mechanical sheets. The sheet says what it is in its title
//!   block ("DRAWING TITLE:" over the title lines, the sheet number in the largest lettering
//!   below), so the discipline comes from the sheet number, never from the file name.
//! - A sheet holds several views (an enlarged plan, unit plans, details), each labelled at its
//!   bottom left: a number bubble, the view title and a "SCALE:" line under it. The view's
//!   drawing is the block of linework the title sits in or just below. Blocks are found as
//!   connected areas of an occupancy grid over the sheet (borders and title-block rules, which
//!   span the sheet, are left out).
//! - Devices on electrical plans carry a type tag (`A`, `E`, `OS`, `XA`) and a circuit label
//!   (`A-1,f`, `B-14`) as real text, so counting the type tags in a view counts its devices by
//!   type; circuit labels without a type tag beside them are mostly receptacles (they also catch
//!   disconnects and equipment feeds, so that number is a cross-check, not a count).
//!
//! Sheets whose lettering was plotted as strokes (SHX) have no text to read here.

use cadaddy_doc::{Drawing, EntityKind, entity_bounds};
use cadaddy_geom::Vec2;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

/// Text objects read per sheet (a guard against hostile files).
const MAX_TEXTS: usize = 200_000;
/// Objects measured for the view regions.
const MAX_OBJECTS: usize = 400_000;
/// Occupancy grid cells across the sheet's width (about 0.2 in on a 42 in sheet).
const GRID_CELLS: f64 = 210.0;
/// Most grid cells in all (caps a hostile aspect ratio).
const MAX_GRID: usize = 200_000;
/// Objects wider or taller than this share of the sheet are borders and title-block rules.
const SPAN_LIMIT: f64 = 0.35;
/// Smallest block of linework (in grid cells) that can be a view.
const MIN_REGION_CELLS: usize = 20;

/// One line of text on the sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct TextItem {
    pub value: String,
    pub at: Vec2,
    pub height: f64,
    /// Rotation in radians (view titles and title blocks read horizontally).
    pub rotation: f64,
}

/// The sheet as its title block names it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SheetIdent {
    /// `E-6`, `A122`, `M-2`.
    pub number: String,
    /// The title lines joined (`ELECTRICAL ENLARGED PLANS`); empty when only the number was found.
    pub title: String,
    /// The sheet number's letters (`E`, `A`, `FP`).
    pub discipline: String,
    /// `title_block` (under a "DRAWING TITLE" label) or `largest_number` (the largest
    /// sheet-number lettering on the sheet).
    pub source: String,
}

/// A view drawn on the sheet, from its title.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ViewTitle {
    /// The number in the view's bubble (`1`), when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    pub title: String,
    /// The scale as printed (`1/4" = 1'-0"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_text: Option<String>,
    pub not_to_scale: bool,
    /// Where the title is.
    pub at: [f64; 2],
    /// The block of linework the view occupies `[x0, y0, x1, y1]`, when one was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<[f64; 4]>,
}

/// Device/fixture type tags and circuit labels in a region.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TagCensus {
    /// Count of each short type tag (`A`: 37, `OS`: 9).
    pub by_type: BTreeMap<String, usize>,
    /// Circuit labels (`A-1,f`, `B-14`, `C-23,25`).
    pub circuit_labels: usize,
    /// Circuit labels by panel letter.
    pub by_panel: BTreeMap<String, usize>,
    /// Circuit labels with no type tag beside them: mostly receptacles, plus disconnects and
    /// equipment feeds. An upper-bound cross-check, not a count.
    pub untyped_circuit_labels: usize,
}

/// Everything read from one sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SheetFacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheet: Option<SheetIdent>,
    pub views: Vec<ViewTitle>,
}

/// Words that are short and capitalised but are notes, not type tags.
const NOT_TAGS: &[&str] = &[
    "TYP", "EQ", "AFF", "UP", "DN", "NIC", "RR", "GFI", "GFCI", "AND", "OR", "TO", "AT", "ON", "IN", "OF", "THE", "FOR", "NO", "W", "X", "BY",
];

/// The model-space text of a drawing, one item per line of text.
pub fn texts(d: &Drawing) -> Vec<TextItem> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if out.len() >= MAX_TEXTS {
            break;
        }
        let (value, at, height, rotation) = match &e.kind {
            EntityKind::Text(t) => (t.value.clone(), t.insert.xy(), t.height, t.rotation),
            EntityKind::MText(t) => (t.contents.clone(), t.insert.xy(), t.height, t.rotation),
            _ => continue,
        };
        let value = value.trim().to_string();
        if value.is_empty() || !at.is_finite() || !height.is_finite() || height <= 0.0 {
            continue;
        }
        out.push(TextItem { value, at, height, rotation: if rotation.is_finite() { rotation } else { 0.0 } });
    }
    out
}

fn horizontal(t: &TextItem) -> bool {
    t.rotation.sin().abs() < 0.05 && t.rotation.cos() > 0.0
}

/// A sheet number: one to three letters, maybe a dash, then digits with maybe a dotted part
/// (`E-6`, `A122`, `FP-1.1`, `M2.01`).
pub fn is_sheet_number(t: &str) -> bool {
    let t = t.trim();
    let letters = t.chars().take_while(char::is_ascii_uppercase).count();
    let rest = t.get(letters..).unwrap_or("").trim_start_matches('-');
    (1..=3).contains(&letters)
        && !rest.is_empty()
        && rest.len() <= 6
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The discipline letters of a sheet number (`E-6` → `E`, `FP101` → `FP`).
pub fn discipline_of(number: &str) -> String {
    number.trim().chars().take_while(char::is_ascii_uppercase).collect()
}

/// The sheet's number, title and discipline from its title block.
pub fn identify_sheet(texts: &[TextItem]) -> Option<SheetIdent> {
    let label = texts.iter().filter(|t| horizontal(t)).find(|t| {
        let u = t.value.to_ascii_uppercase();
        u.starts_with("DRAWING TITLE") || u.starts_with("SHEET TITLE")
    });
    if let Some(l) = label {
        let h = l.height;
        // The sheet number: the largest sheet-number lettering below the label, near it.
        let number = texts
            .iter()
            .filter(|t| is_sheet_number(&t.value) && t.at.y < l.at.y && t.at.x >= l.at.x - 5.0 * h && t.at.dist(l.at) <= 60.0 * h)
            .max_by(|a, b| a.height.total_cmp(&b.height));
        // The title: the lines right under the label, top down, until a gap or the number.
        let mut lines: Vec<&TextItem> = texts
            .iter()
            .filter(|t| {
                horizontal(t)
                    && (t.at.x - l.at.x).abs() <= 3.0 * h
                    && t.at.y < l.at.y
                    && t.at.y > l.at.y - 14.0 * h
                    && t.height >= 0.9 * h
                    && !is_sheet_number(&t.value)
            })
            .collect();
        lines.sort_by(|a, b| b.at.y.total_cmp(&a.at.y));
        let mut title: Vec<&str> = Vec::new();
        let mut last_y = l.at.y;
        for t in lines {
            if last_y - t.at.y > 3.0 * t.height.max(h) {
                break;
            }
            title.push(&t.value);
            last_y = t.at.y;
        }
        if let Some(n) = number {
            return Some(SheetIdent {
                number: n.value.trim().to_string(),
                title: title.join(" "),
                discipline: discipline_of(&n.value),
                source: "title_block".into(),
            });
        }
    }
    // No labelled title block: the largest sheet-number lettering, if it stands out.
    let mut heights: Vec<f64> = texts.iter().map(|t| t.height).collect();
    heights.sort_by(f64::total_cmp);
    let median = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    texts
        .iter()
        .filter(|t| is_sheet_number(&t.value) && t.height >= 2.0 * median)
        .max_by(|a, b| a.height.total_cmp(&b.height))
        .map(|n| SheetIdent {
            number: n.value.trim().to_string(),
            title: String::new(),
            discipline: discipline_of(&n.value),
            source: "largest_number".into(),
        })
}

fn is_scale_line(t: &TextItem) -> bool {
    t.value.to_ascii_uppercase().starts_with("SCALE")
}

/// The printed scale after "SCALE:" (`1/4" = 1'-0"`).
fn scale_text(t: &str) -> String {
    let u = t.trim();
    let after = u.find(':').map_or(u.get(5..).unwrap_or(""), |i| u.get(i + 1..).unwrap_or(""));
    after.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The views on the sheet: a title is the larger line right above a "SCALE:" line.
pub fn view_titles(d: &Drawing, texts: &[TextItem]) -> Vec<ViewTitle> {
    let regions = view_regions(d);
    let mut out: Vec<ViewTitle> = Vec::new();
    for s in texts.iter().filter(|t| horizontal(t) && is_scale_line(t)) {
        let h = s.height;
        let Some(title) = texts
            .iter()
            .filter(|t| {
                horizontal(t)
                    && !is_scale_line(t)
                    && (t.at.x - s.at.x).abs() <= 2.0 * h
                    && t.at.y > s.at.y
                    && t.at.y - s.at.y <= 4.0 * h
                    && t.height > 1.2 * h
            })
            .min_by(|a, b| (a.at.y - s.at.y).total_cmp(&(b.at.y - s.at.y)))
        else {
            continue;
        };
        let th = title.height;
        let number = texts
            .iter()
            .filter(|t| {
                t.value.len() <= 3
                    && t.value.chars().all(|c| c.is_ascii_digit())
                    && t.at.x < title.at.x
                    && title.at.x - t.at.x <= 8.0 * th
                    && (t.at.y - title.at.y).abs() <= 2.5 * th
            })
            .min_by(|a, b| (title.at.x - a.at.x).total_cmp(&(title.at.x - b.at.x)))
            .map(|t| t.value.clone());
        let st = scale_text(&s.value);
        let nts = st.to_ascii_uppercase().contains("NOT TO SCALE") || st.to_ascii_uppercase() == "NTS";
        if out.iter().any(|v| v.title == title.value && v.at == [title.at.x, title.at.y]) {
            continue;
        }
        out.push(ViewTitle {
            number,
            title: title.value.clone(),
            scale_text: (!nts && !st.is_empty()).then_some(st),
            not_to_scale: nts,
            at: [title.at.x, title.at.y],
            region: region_for(&regions, title.at, th),
        });
    }
    out.sort_by(|a, b| a.number.as_deref().unwrap_or("~").cmp(b.number.as_deref().unwrap_or("~")).then(a.title.cmp(&b.title)));
    out
}

/// The block a title labels: the largest block holding the title, else the nearest block right
/// above it that spans the title's start.
fn region_for(regions: &[([f64; 4], usize)], at: Vec2, h: f64) -> Option<[f64; 4]> {
    let inside = |r: &[f64; 4]| r[0] <= at.x && at.x <= r[2] && r[1] <= at.y && at.y <= r[3];
    if let Some((r, _)) = regions.iter().filter(|(r, _)| inside(r)).max_by_key(|(_, n)| *n) {
        return Some(*r);
    }
    regions
        .iter()
        .filter(|(r, _)| r[1] >= at.y && r[1] - at.y <= 12.0 * h && r[0] <= at.x + 4.0 * h && at.x <= r[2])
        .min_by(|a, b| (a.0[1] - at.y).total_cmp(&(b.0[1] - at.y)))
        .map(|(r, _)| *r)
}

/// Blocks of linework on the sheet: connected areas of an occupancy grid, with their cell
/// counts. Borders and title-block rules (objects spanning much of the sheet) are left out.
pub fn view_regions(d: &Drawing) -> Vec<([f64; 4], usize)> {
    let mut boxes = Vec::new();
    for e in d.model.iter().take(MAX_OBJECTS) {
        if !matches!(e.kind, EntityKind::Line(_) | EntityKind::LwPolyline(_) | EntityKind::Circle(_) | EntityKind::Arc(_) | EntityKind::Text(_)) {
            continue;
        }
        let b = entity_bounds(d, e, 0);
        if !b.is_empty() && b.min.is_finite() && b.max.is_finite() {
            boxes.push(b);
        }
    }
    let Some(all) = boxes.iter().copied().reduce(|a, b| a.union(&b)) else { return Vec::new() };
    let (w, h) = (all.max.x - all.min.x, all.max.y - all.min.y);
    if !(w > 0.0 && h > 0.0) {
        return Vec::new();
    }
    let cell = w / GRID_CELLS;
    let nx = (w / cell).ceil() as usize + 1;
    let ny = (h / cell).ceil() as usize + 1;
    if nx.saturating_mul(ny) > MAX_GRID {
        return Vec::new();
    }
    let idx = |v: f64, lo: f64, n: usize| (((v - lo) / cell).floor().max(0.0) as usize).min(n - 1);
    let mut occ: HashSet<(usize, usize)> = HashSet::new();
    for b in &boxes {
        if b.max.x - b.min.x > SPAN_LIMIT * w || b.max.y - b.min.y > SPAN_LIMIT * h {
            continue;
        }
        for i in idx(b.min.x, all.min.x, nx)..=idx(b.max.x, all.min.x, nx) {
            for j in idx(b.min.y, all.min.y, ny)..=idx(b.max.y, all.min.y, ny) {
                occ.insert((i, j));
            }
        }
    }
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    let mut out = Vec::new();
    let mut cells: Vec<(usize, usize)> = occ.iter().copied().collect();
    cells.sort_unstable();
    for c in cells {
        if !seen.insert(c) {
            continue;
        }
        let (mut stack, mut n) = (vec![c], 0usize);
        let (mut i0, mut j0, mut i1, mut j1) = (c.0, c.1, c.0, c.1);
        while let Some((i, j)) = stack.pop() {
            n += 1;
            (i0, j0, i1, j1) = (i0.min(i), j0.min(j), i1.max(i), j1.max(j));
            let next = [(i + 1, j), (i.wrapping_sub(1), j), (i, j + 1), (i, j.wrapping_sub(1))];
            for q in next {
                if occ.contains(&q) && seen.insert(q) {
                    stack.push(q);
                }
            }
        }
        if n >= MIN_REGION_CELLS {
            let at = |i: usize, lo: f64| lo + i as f64 * cell;
            out.push(([at(i0, all.min.x), at(j0, all.min.y), at(i1 + 1, all.min.x), at(j1 + 1, all.min.y)], n));
        }
    }
    out
}

/// A circuit label: panel letters, a dash, the circuit number(s), maybe a switch leg
/// (`A-1`, `B-14`, `C-23,25`, `A-1,f`, `RP-3`).
pub fn is_circuit_label(t: &str) -> bool {
    let t = t.trim();
    let letters = t.chars().take_while(char::is_ascii_uppercase).count();
    let Some(rest) = t.get(letters..).and_then(|r| r.strip_prefix('-')) else { return false };
    let mut parts = rest.split(',');
    let first_ok = parts.next().is_some_and(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()));
    (1..=2).contains(&letters)
        && first_ok
        && parts.all(|p| (!p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit())) || (p.len() == 1 && p.chars().all(|c| c.is_ascii_lowercase())))
}

/// A device/fixture type tag: one to three capital letters and up to two digits (`A`, `A1`,
/// `XA`, `OS`, `EF`, `L4`), not a common note word.
pub fn is_type_tag(t: &str) -> bool {
    let t = t.trim();
    let letters = t.chars().take_while(char::is_ascii_uppercase).count();
    let digits = t.len().saturating_sub(letters);
    (1..=3).contains(&letters) && digits <= 2 && t.chars().skip(letters).all(|c| c.is_ascii_digit()) && !NOT_TAGS.contains(&t)
}

/// Type tags and circuit labels inside a region `[x0, y0, x1, y1]`.
pub fn tag_census(texts: &[TextItem], region: [f64; 4]) -> TagCensus {
    let inside = |p: Vec2| region[0] <= p.x && p.x <= region[2] && region[1] <= p.y && p.y <= region[3];
    let here: Vec<&TextItem> = texts.iter().filter(|t| inside(t.at)).collect();
    let tags: Vec<&&TextItem> = here.iter().filter(|t| is_type_tag(&t.value)).collect();
    let mut c = TagCensus::default();
    for t in &tags {
        *c.by_type.entry(t.value.trim().to_string()).or_default() += 1;
    }
    for t in here.iter().filter(|t| is_circuit_label(&t.value)) {
        c.circuit_labels += 1;
        let panel: String = t.value.trim().chars().take_while(char::is_ascii_uppercase).collect();
        *c.by_panel.entry(panel).or_default() += 1;
        let h = t.height;
        // A fixture's circuit is printed under its tag, or beside it on the same line.
        let typed = tags.iter().any(|k| {
            ((k.at.x - t.at.x).abs() <= 3.5 * h && k.at.y >= t.at.y && k.at.y - t.at.y <= 2.6 * h)
                || ((k.at.y - t.at.y).abs() <= 1.2 * h && t.at.x > k.at.x && t.at.x - k.at.x <= 6.0 * h)
        });
        if !typed {
            c.untyped_circuit_labels += 1;
        }
    }
    c
}

/// The sheet and its views.
pub fn read_sheet_facts(d: &Drawing) -> SheetFacts {
    let t = texts(d);
    SheetFacts { sheet: identify_sheet(&t), views: view_titles(d, &t) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, HAlign, Line, Space, Text, VAlign};
    use cadaddy_geom::Vec3;

    fn add(d: &mut Drawing, kind: EntityKind) {
        let _ = d.add(&Space::Model, Common { layer: "0".into(), ..Common::default() }, kind);
    }
    fn text(d: &mut Drawing, s: &str, x: f64, y: f64, h: f64) {
        let p = Vec3::new(x, y, 0.0);
        add(
            d,
            EntityKind::Text(Text {
                insert: p,
                align_pt: None,
                height: h,
                value: s.into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: HAlign::Left,
                valign: VAlign::Baseline,
            }),
        );
    }
    fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
        add(d, EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) }));
    }
    /// A plan's worth of linework: a box with rooms (walls both ways every 40 units) inside.
    fn plan(d: &mut Drawing, x0: f64, y0: f64, w: f64, h: f64) {
        line(d, (x0, y0), (x0 + w, y0));
        line(d, (x0 + w, y0), (x0 + w, y0 + h));
        line(d, (x0 + w, y0 + h), (x0, y0 + h));
        line(d, (x0, y0 + h), (x0, y0));
        let mut x = x0 + 40.0;
        while x < x0 + w {
            line(d, (x, y0), (x, y0 + h));
            x += 40.0;
        }
        let mut y = y0 + 40.0;
        while y < y0 + h {
            line(d, (x0, y), (x0 + w, y));
            y += 40.0;
        }
    }

    /// A made-up 42x30 sheet: border, title block, one enlarged plan with tagged devices, one
    /// detail; lettering at the sizes a plotted sheet uses.
    fn sheet() -> Drawing {
        let mut d = Drawing::default();
        // border and title-block rule (span the sheet: not a view)
        line(&mut d, (0.0, 0.0), (2016.0, 0.0));
        line(&mut d, (2016.0, 0.0), (2016.0, 1440.0));
        line(&mut d, (2016.0, 1440.0), (0.0, 1440.0));
        line(&mut d, (0.0, 1440.0), (0.0, 0.0));
        line(&mut d, (1860.0, 0.0), (1860.0, 1440.0));
        text(&mut d, "DRAWING TITLE:", 1879.0, 131.0, 4.6);
        text(&mut d, "POWER ENLARGED", 1879.0, 116.0, 6.0);
        text(&mut d, "PLANS", 1879.0, 106.0, 6.0);
        text(&mut d, "Q-7", 1900.0, 40.0, 18.0);
        // view 1: enlarged plan
        plan(&mut d, 100.0, 90.0, 600.0, 420.0);
        text(&mut d, "1", 93.0, 63.0, 7.4);
        text(&mut d, "POWER - LEVEL 1 - LOBBY", 113.0, 71.0, 7.4);
        text(&mut d, "SCALE:  1/4\" = 1'-0\"", 113.0, 62.0, 4.6);
        // devices: 3 type-A fixtures (tag over circuit), 2 OS, 1 EF beside its circuit, 4 receptacles
        for (i, x) in [200.0, 300.0, 400.0].iter().enumerate() {
            text(&mut d, "A", *x, 500.0, 4.6);
            text(&mut d, &format!("A-1,{}", ["a", "b", "c"].get(i).copied().unwrap_or("a")), *x, 491.0, 4.6);
        }
        text(&mut d, "OS", 250.0, 400.0, 4.6);
        text(&mut d, "OS", 350.0, 400.0, 4.6);
        text(&mut d, "EF", 450.0, 300.0, 4.6);
        text(&mut d, "A-3", 465.0, 300.0, 4.6);
        for (k, x) in [150.0, 250.0, 350.0, 550.0].iter().enumerate() {
            text(&mut d, &format!("B-{}", k + 1), *x, 200.0, 4.6);
        }
        text(&mut d, "TYP", 600.0, 200.0, 4.6);
        // view 2: a detail, not to scale
        plan(&mut d, 900.0, 600.0, 200.0, 200.0);
        text(&mut d, "2", 893.0, 573.0, 7.4);
        text(&mut d, "DAMPER DETAIL", 913.0, 581.0, 7.4);
        text(&mut d, "SCALE: NOT TO SCALE", 913.0, 572.0, 4.6);
        d
    }

    #[test]
    fn title_block_names_the_sheet_and_its_discipline() {
        let d = sheet();
        let s = identify_sheet(&texts(&d)).expect("title block");
        assert_eq!((s.number.as_str(), s.title.as_str(), s.discipline.as_str(), s.source.as_str()), ("Q-7", "POWER ENLARGED PLANS", "Q", "title_block"));
    }

    #[test]
    fn without_a_label_the_largest_sheet_number_wins() {
        let mut d = Drawing::default();
        for i in 0..20 {
            text(&mut d, "NOTE", 10.0, f64::from(i) * 10.0, 4.0);
        }
        text(&mut d, "B-12", 500.0, 10.0, 4.0); // small: a circuit-like label, not the sheet
        text(&mut d, "M2.01", 900.0, 10.0, 16.0);
        let s = identify_sheet(&texts(&d)).expect("number");
        assert_eq!((s.number.as_str(), s.discipline.as_str(), s.source.as_str()), ("M2.01", "M", "largest_number"));
    }

    #[test]
    fn view_titles_carry_number_scale_and_region() {
        let d = sheet();
        let v = view_titles(&d, &texts(&d));
        assert_eq!(v.len(), 2, "{v:?}");
        let lobby = v.iter().find(|x| x.number.as_deref() == Some("1")).expect("view 1");
        assert_eq!(lobby.title, "POWER - LEVEL 1 - LOBBY");
        assert_eq!(lobby.scale_text.as_deref(), Some("1/4\" = 1'-0\""));
        assert!(!lobby.not_to_scale);
        let r = lobby.region.expect("region");
        assert!(r[0] <= 100.0 && r[2] >= 700.0 && r[1] <= 90.0 && r[3] >= 510.0 && r[2] < 900.0, "{r:?}");
        let det = v.iter().find(|x| x.number.as_deref() == Some("2")).expect("view 2");
        assert!(det.not_to_scale && det.scale_text.is_none());
        let r2 = det.region.expect("detail region");
        assert!(r2[0] >= 850.0 && r2[1] >= 590.0, "{r2:?}");
    }

    #[test]
    fn census_counts_type_tags_and_untyped_circuits() {
        let d = sheet();
        let t = texts(&d);
        let v = view_titles(&d, &t);
        let r = v.iter().find(|x| x.number.as_deref() == Some("1")).and_then(|x| x.region).expect("region");
        let c = tag_census(&t, r);
        assert_eq!(c.by_type.get("A"), Some(&3));
        assert_eq!(c.by_type.get("OS"), Some(&2));
        assert_eq!(c.by_type.get("EF"), Some(&1));
        assert!(!c.by_type.contains_key("TYP"), "{:?}", c.by_type);
        assert_eq!(c.circuit_labels, 8);
        assert_eq!(c.by_panel.get("B"), Some(&4));
        assert_eq!(c.untyped_circuit_labels, 4, "the receptacles");
    }

    #[test]
    fn label_shapes() {
        for s in ["E-6", "A122", "FP-1.1", "M2.01", "L-1"] {
            assert!(is_sheet_number(s), "{s}");
        }
        for s in ["E", "6", "ELEV.", "A-", "ABCD-1"] {
            assert!(!is_sheet_number(s), "{s}");
        }
        for s in ["A-1", "B-14", "C-23,25", "A-1,f", "RP-3", "A-37,39,41"] {
            assert!(is_circuit_label(s), "{s}");
        }
        for s in ["A-", "A-1,F", "AB-", "-1", "A1-2x", "ABC-1"] {
            assert!(!is_circuit_label(s), "{s}");
        }
        for s in ["A", "A1", "XA", "OS", "EF", "L4", "FCU"] {
            assert!(is_type_tag(s), "{s}");
        }
        for s in ["TYP", "EQ", "A123", "ABCD", "a", "GFCI"] {
            assert!(!is_type_tag(s), "{s}");
        }
    }

    #[test]
    fn hostile_text_never_panics() {
        let mut d = Drawing::default();
        text(&mut d, "DRAWING TITLE:", f64::NAN, 1.0, 4.0);
        text(&mut d, "SCALE:", 1.0, 1.0, f64::INFINITY);
        text(&mut d, "SCALE", 0.0, 0.0, 1e-300);
        text(&mut d, "\u{0}\u{FFFF}-", 1e300, -1e300, 4.0);
        line(&mut d, (0.0, 0.0), (1e300, 1e-300));
        let f = read_sheet_facts(&d);
        let _ = tag_census(&texts(&d), [f64::NEG_INFINITY, f64::NAN, 0.0, 1.0]);
        assert!(f.views.len() <= 2);
    }
}
