//! A trade's plan of the site built as site work (Q6358).
//!
//! A site plan an electrical or combined MEP set issues (its sheet number's discipline or its
//! title names a trade), or any site sheet carrying a trade's content (a luminaire type its fixture
//! schedule lists, a panel circuit tag, a power or communications note), is that trade's plan of
//! the site, not a civil sheet: its pole lights, site equipment and buried runs are work the job
//! builds. Only a site sheet with no trade content stays `site/civil`.
//!
//! - [`trade_site_sheet`] and [`trade_content`]: which site sheets are a trade's.
//! - [`read`]: on one site sheet, the building outline it draws, the luminaires its fixture
//!   schedule types (each at its symbol), the panel circuit tags, the site equipment its labels
//!   name (each at its symbol), the conduit notes and the drawn lines (a dashed line's dashes
//!   joined).
//! - [`register`]: the sheet laid onto the slab level's plan by the building outline it draws
//!   (turned square to the model by its walls, centred on the footprint); when the outline isn't
//!   the footprint's size, by its centre, and said so.
//! - [`plan`]: what it builds. Each pole light: the luminaire at the pole's top, the pole and its
//!   concrete base. The site equipment (a transformer the gear step proposed by rule moves to where
//!   the site plan draws it). The runs drawn between them, or called by a note at its count and
//!   size. The circuits from their tags, a circuit's undrawn links and its home run to its panel
//!   (inferred). Every buried run in PVC at its cover, with its stub-ups.
//! - [`site_loads`]: the panel schedules' circuits whose load is on the site that no site plan
//!   carries: each is asked about.
//!
//! Every value comes from the documents or from `sitework.json` (rules data, each with its
//! source); every object made carries its provenance (drawn, derived, inferred) and its basis.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};

use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::levels::SheetRegistration;
use cadaddy_walk3d::tags::ElementTag;
use cadaddy_walk3d::{BuildingSpec, ConcreteElement, Made, MepEquipment, MepRun, PadDecision};
use serde::Deserialize;

use super::ugplan::Callout;

const RULES: &str = include_str!("sitework.json");

/// Why a trade's plan of the site is built on no storey: it is built as site work.
pub(crate) const SITE_WORK: &str = "site work";
/// The level code a trade's site plan reads on: the site datum, no storey's.
pub(crate) const SITE_LEVEL: &str = "SITE";
/// How the runs made here name their source (`site work E-001`): a rerun replaces them.
pub(crate) const SOURCE_PREFIX: &str = "site work ";
/// The `sources` key (equipment), the field (device tags) and the property (built elements)
/// marking what was made here.
pub(crate) const MARK: &str = "site_work";
/// The concrete made here: `SITE-PB-1`.
pub(crate) const CONCRETE_PREFIX: &str = "SITE-PB-";

/// Texts, shapes and segments of one sheet looked at, at most (hostile input).
const MAX_TEXTS: usize = 200_000;
const MAX_SHAPES: usize = 100_000;
const MAX_SEGS: usize = 200_000;
/// Luminaires, tags, equipment and notes kept per sheet, at most.
const MAX_ITEMS: usize = 2_000;
/// Segments of one drawn line followed, at most (a civil layer's linework is no run).
const MAX_COMPONENT: usize = 4_000;
/// Building outlines weighed per sheet, at most.
const MAX_OUTLINES: usize = 30;
/// Lines of one label read, at most.
const MAX_LABEL_LINES: usize = 4;
/// Conduits one note's run is built as, at most.
const MAX_CONDUITS: u32 = 12;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Register {
    pub area_ratio_min: f64,
    pub area_ratio_max: f64,
    pub scale_tolerance: f64,
    pub on_wall_ft: f64,
    pub source: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct EquipKind {
    pub kind: String,
    pub words: Vec<String>,
    pub height_in: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Pole {
    pub words: Vec<String>,
    pub default_height_ft: f64,
    pub height_source: String,
    pub size_in: f64,
    pub size_source: String,
    /// Each row: the tallest pole it holds (ft), the base's diameter, depth and height above
    /// grade (in).
    pub bases: Vec<[f64; 4]>,
    pub base_source: String,
}

/// The rules data (`sitework.json`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub trade_disciplines: Vec<String>,
    pub trade_title_words: Vec<String>,
    pub trade_source: String,
    pub content_note_words: Vec<String>,
    pub content_source: String,
    pub building_layer_words: Vec<String>,
    pub building_source: String,
    pub text_layers: Vec<String>,
    pub glyph_layer: String,
    pub glyph_max_h: f64,
    pub symbol_reach_h: f64,
    pub symbol_join_h: f64,
    pub symbol_max_ft: f64,
    pub equipment_reach_h: f64,
    pub equipment_min_ft: f64,
    pub equipment_max_ft: f64,
    pub label_max_chars: usize,
    pub line_gap_h: f64,
    pub line_turn_deg: f64,
    pub line_touch_h: f64,
    pub line_break_h: f64,
    pub line_break_deg: f64,
    pub line_end_reach_h: f64,
    pub line_source: String,
    pub register: Register,
    pub equipment: Vec<EquipKind>,
    pub equipment_source: String,
    pub utility_words: Vec<String>,
    pub action_words: Vec<String>,
    pub pole: Pole,
    pub ground_words: Vec<String>,
    pub box_marks: Vec<String>,
    pub site_light_words: Vec<String>,
    pub site_type_words: Vec<String>,
    pub site_type_source: String,
    pub trench_item: String,
    pub parallel_spacing_in: f64,
    pub parallel_source: String,
    pub cover_in: f64,
    pub cover_source: String,
    pub branch_conduit_in: f64,
    pub branch_conduit_source: String,
    pub stub_top_ft: f64,
    pub stub_source: String,
    pub primary_out_ft: f64,
    pub primary_source: String,
    pub site_load_words: Vec<String>,
    pub site_load_source: String,
    pub max_sheets: usize,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

fn has_any(up: &str, words: &[String]) -> bool {
    words.iter().any(|w| !w.is_empty() && up.contains(w.as_str()))
}

fn words_of(up: &str) -> Vec<&str> {
    up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect()
}

// ---------------- which site sheets are a trade's ----------------

/// Whether a site sheet is a trade's plan of the site by its discipline (`E`, `MEP`) or its
/// title (`SITE PLAN - ELECTRICAL`, `M/E/P SITE PLAN`).
pub(crate) fn trade_site_sheet(discipline: &str, title: &str) -> bool {
    let r = rules();
    let d = discipline.trim().to_ascii_uppercase();
    if !d.is_empty() && r.trade_disciplines.iter().any(|x| x.eq_ignore_ascii_case(&d)) {
        return true;
    }
    let up = title.to_ascii_uppercase();
    // `M/E/P` reads as `MEP`.
    let joined = up.replace('/', "");
    [up.as_str(), joined.as_str()].iter().any(|t| words_of(t).iter().any(|w| r.trade_title_words.iter().any(|x| x == w)))
}

/// A panel circuit tag as printed (`LP-7`, `A-1,3`, an arrow's glyph after it): the tag, its
/// panel and its circuits.
pub(crate) fn circuit_label(v: &str) -> Option<(String, String, String)> {
    let clean: String = v.trim().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == ',').collect();
    let clean = clean.trim_end_matches(',').to_string();
    if !cadaddy_mepfix::sheet_facts::is_circuit_label(&clean) {
        return None;
    }
    let (panel, circuits) = clean.split_once('-')?;
    Some((clean.clone(), panel.to_string(), circuits.to_string()))
}

/// What makes a site sheet a trade's by its content (none: a civil sheet): a luminaire type the
/// set's fixture schedule lists, a panel circuit tag, a power or communications note.
pub(crate) fn trade_content(texts: &[TextItem], lum_types: &BTreeSet<String>) -> Option<String> {
    let r = rules();
    for t in texts.iter().take(MAX_TEXTS) {
        let v = t.value.trim().to_ascii_uppercase();
        if v.is_empty() {
            continue;
        }
        if lum_types.contains(&v) {
            return Some(format!("luminaire type {v} (the fixture schedule's)"));
        }
        if let Some((tag, _, _)) = circuit_label(&v) {
            return Some(format!("circuit tag {tag}"));
        }
        if v.len() > 12 && has_any(&v, &r.content_note_words) {
            return Some(format!("a power note: {}", v.chars().take(60).collect::<String>()));
        }
    }
    None
}

// ---------------- reading one sheet ----------------

/// A closed shape on a sheet: a symbol, a pad, an outline.
#[derive(Clone, Debug)]
pub(crate) struct Shape {
    pub handle: Handle,
    pub layer: String,
    pub min: [f64; 2],
    pub max: [f64; 2],
    pub pts: Vec<[f64; 2]>,
}

impl Shape {
    fn size(&self) -> f64 {
        (self.max[0] - self.min[0]).max(self.max[1] - self.min[1])
    }
    fn bbox(&self) -> [f64; 4] {
        [self.min[0], self.min[1], self.max[0], self.max[1]]
    }
}

/// A straight piece of a drawn line, its layer by index.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seg {
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub layer: usize,
}

/// A luminaire its type tag marks: where its symbol is (sheet units).
#[derive(Clone, Debug)]
pub(crate) struct Light {
    /// Its type mark (empty: a symbol only its circuit tags).
    pub mark: String,
    pub at: [f64; 2],
    pub bbox: [f64; 4],
    pub symbol: Option<Handle>,
    /// The circuit tag printed by it in place of a type mark (`LP-7`).
    pub circuit: Option<String>,
}

/// A panel circuit tag (`LP-7`) on the sheet.
#[derive(Clone, Debug)]
pub(crate) struct CircuitTag {
    pub label: String,
    pub bbox: [f64; 4],
}

/// Site equipment a label names, at its symbol.
#[derive(Clone, Debug)]
pub(crate) struct Equip {
    pub kind: String,
    pub label: String,
    pub bbox: [f64; 4],
    pub symbol: Handle,
    pub utility: bool,
    pub height_in: f64,
    /// The circuit tags printed by it (`LP-11`).
    pub circuits: Vec<String>,
}

/// A conduit note and where its lines are.
#[derive(Clone, Debug)]
pub(crate) struct Note {
    pub callout: Callout,
    pub bbox: [f64; 4],
}

/// What one site sheet draws (sheet units).
#[derive(Clone, Debug, Default)]
pub(crate) struct SiteRead {
    pub sheet: String,
    /// Its usual text height: the yardstick of every reach.
    pub h: f64,
    /// Closed outlines that may be the building's, largest first, and whether their layer names
    /// the building.
    pub outlines: Vec<(Vec<[f64; 2]>, bool)>,
    pub lights: Vec<Light>,
    pub tags: Vec<CircuitTag>,
    pub equipment: Vec<Equip>,
    pub notes: Vec<Note>,
    pub segs: Vec<Seg>,
    pub layers: Vec<String>,
    /// Labels naming equipment with no symbol by them.
    pub unplaced_labels: Vec<String>,
    /// The view area read (`[x0, y0, x1, y1]`).
    pub area: [f64; 4],
}

fn bbox_of(pts: &[[f64; 2]]) -> [f64; 4] {
    pts.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
        [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
    })
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

fn centre(b: &[f64; 4]) -> [f64; 2] {
    [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// How far `p` is from the box (0 inside).
fn gap(b: &[f64; 4], p: [f64; 2]) -> f64 {
    let dx = (b[0] - p[0]).max(p[0] - b[2]).max(0.0);
    let dy = (b[1] - p[1]).max(p[1] - b[3]).max(0.0);
    dx.hypot(dy)
}

/// How far apart two boxes are (0 when they touch).
fn box_gap(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let dx = (a[0] - b[2]).max(b[0] - a[2]).max(0.0);
    let dy = (a[1] - b[3]).max(b[1] - a[3]).max(0.0);
    dx.hypot(dy)
}

/// A text's box, turned with it (its length by its characters).
fn text_box(t: &TextItem) -> [f64; 4] {
    let n = t.value.trim().chars().count().max(1) as f64;
    let (h, len) = (t.height.max(0.0), n * 0.8 * t.height.max(0.0));
    let (c, s) = (t.rotation.cos(), t.rotation.sin());
    let p = [t.at.x, t.at.y];
    bbox_of(&[p, [p[0] + c * len, p[1] + s * len], [p[0] - s * h, p[1] + c * h], [p[0] + c * len - s * h, p[1] + s * len + c * h]])
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    if l2 <= 1e-18 {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

/// The point on a closed polygon's edge nearest `p`.
fn nearest_on(poly: &[[f64; 2]], p: [f64; 2]) -> Option<[f64; 2]> {
    let n = poly.len();
    let mut best: Option<(f64, [f64; 2])> = None;
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n)) else { continue };
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = dx * dx + dy * dy;
        let t = if l2 <= 1e-18 { 0.0 } else { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) };
        let q = [a[0] + t * dx, a[1] + t * dy];
        let d = dist(p, q);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, q));
        }
    }
    best.map(|(_, q)| q)
}

fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

fn poly_area(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    let mut s = 0.0;
    for i in 0..n {
        if let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n)) {
            s += a[0] * b[1] - b[0] * a[1];
        }
    }
    (s / 2.0).abs()
}

/// The usual height of a sheet's texts (their median).
fn usual_height(texts: &[TextItem]) -> f64 {
    let mut hs: Vec<f64> = texts.iter().map(|t| t.height).filter(|h| h.is_finite() && *h > 0.0).collect();
    hs.sort_by(f64::total_cmp);
    hs.get(hs.len() / 2).copied().unwrap_or(0.0)
}

/// The lines of a label or note: `i`'s line and those next above and below it, left-aligned
/// with it and as tall, in reading order.
fn block(texts: &[&TextItem], i: usize) -> Vec<usize> {
    let Some(first) = texts.get(i) else { return Vec::new() };
    let h = first.height.max(1e-9);
    let mut out = vec![i];
    for up in [true, false] {
        let mut cur = first.at;
        for _ in 0..MAX_LABEL_LINES {
            let next = texts.iter().enumerate().find(|(j, o)| {
                let dy = if up { o.at.y - cur.y } else { cur.y - o.at.y };
                !out.contains(j)
                    && (o.at.x - cur.x).abs() < h
                    && dy > 0.3 * h
                    && dy < 1.9 * h
                    && (o.height - h).abs() < 0.25 * h
                    && o.rotation.abs() < 0.01
            });
            match next {
                Some((j, o)) => {
                    if up {
                        out.insert(0, j);
                    } else {
                        out.push(j);
                    }
                    cur = o.at;
                }
                None => break,
            }
        }
    }
    out.truncate(MAX_LABEL_LINES * 2 + 1);
    out
}

/// The symbol by `near`: the nearest unused shape no larger than `max` within `reach`, with
/// its other pieces on its layer (a pole's circle by its head's box) within `join`: its box and
/// its pieces.
fn cluster_near(
    shapes: &[Shape],
    used: &BTreeSet<Handle>,
    near: &[f64; 4],
    reach: f64,
    join: f64,
    max: f64,
    skip: &dyn Fn(&Shape) -> bool,
) -> Option<([f64; 4], Vec<Handle>)> {
    let s = shapes
        .iter()
        .filter(|s| s.size() <= max && !skip(s) && !used.contains(&s.handle))
        .map(|s| (s, box_gap(near, &s.bbox())))
        .filter(|(_, g)| *g <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(s, _)| s)?;
    let mut bb = s.bbox();
    let mut members = vec![s.handle];
    for _ in 0..12 {
        let more = shapes.iter().find(|o| {
            !members.contains(&o.handle) && o.layer == s.layer && o.size() <= max && !used.contains(&o.handle) && box_gap(&bb, &o.bbox()) <= join
        });
        match more {
            Some(o) => {
                bb = union(bb, o.bbox());
                members.push(o.handle);
            }
            None => break,
        }
    }
    Some((bb, members))
}

/// A label's lines: `i`'s, the lines over it that run into it (none ending a sentence or asking
/// for work), and those under it while it runs on (a line ending `.` ends it).
fn label_lines(texts: &[&TextItem], i: usize, action: &[String]) -> Vec<usize> {
    let all = block(texts, i);
    let at = all.iter().position(|j| *j == i).unwrap_or(0);
    let text = |j: usize| texts.get(j).map(|t| t.value.trim().to_ascii_uppercase()).unwrap_or_default();
    let mut start = at;
    while let Some(k) = start.checked_sub(1) {
        let up = all.get(k).map(|j| text(*j)).unwrap_or_default();
        if up.ends_with('.') || up.ends_with(':') || has_any(&up, action) {
            break;
        }
        start = k;
    }
    let mut end = at;
    while !all.get(end).map(|j| text(*j)).unwrap_or_default().ends_with('.') {
        let Some(next) = all.get(end + 1).map(|j| text(*j)) else { break };
        if has_any(&next, action) {
            break;
        }
        end += 1;
    }
    all.get(start..=end).map(<[usize]>::to_vec).unwrap_or_else(|| vec![i])
}

fn in_area(area: &[f64; 4], p: [f64; 2]) -> bool {
    p[0] >= area[0].min(area[2]) && p[0] <= area[0].max(area[2]) && p[1] >= area[1].min(area[3]) && p[1] <= area[1].max(area[3])
}

/// One site sheet read: `page` is its drawing (sheet units), `area` its view (off its border and
/// title block), `lum_types` the luminaire types the set's fixture schedule lists, `upf` drawing
/// units per foot.
pub(crate) fn read(sheet: &str, page: &Drawing, area: [f64; 4], lum_types: &BTreeSet<String>, upf: f64) -> SiteRead {
    let r = rules();
    let all = cadaddy_mepfix::sheet_facts::texts_with_tags(page);
    let texts: Vec<&TextItem> = all.iter().take(MAX_TEXTS).filter(|t| in_area(&area, [t.at.x, t.at.y]) && !t.value.trim().is_empty()).collect();
    let owned: Vec<TextItem> = texts.iter().map(|t| (*t).clone()).collect();
    let h = usual_height(&owned);
    let mut out = SiteRead { sheet: sheet.to_string(), h, area, ..SiteRead::default() };
    if !(h.is_finite() && h > 0.0) {
        return out;
    }
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let touch = r.line_touch_h * h;
    // The sheet's shapes and line pieces.
    let mut shapes: Vec<Shape> = Vec::new();
    let mut outline_pool: Vec<(Vec<[f64; 2]>, f64, bool)> = Vec::new();
    let mut layer_ix: HashMap<String, usize> = HashMap::new();
    let text_layer = |l: &str| r.text_layers.iter().any(|x| x.eq_ignore_ascii_case(l));
    for e in page.model.iter().take(MAX_SHAPES * 4) {
        if shapes.len() >= MAX_SHAPES || out.segs.len() >= MAX_SEGS {
            break;
        }
        let layer = e.common.layer.as_str();
        if text_layer(layer) {
            continue;
        }
        let b = cadaddy_doc::entity_bounds(page, e, 0);
        if b.is_empty() || !(b.min.x.is_finite() && b.min.y.is_finite() && b.max.x.is_finite() && b.max.y.is_finite()) {
            continue;
        }
        let bb = [b.min.x, b.min.y, b.max.x, b.max.y];
        if !in_area(&area, centre(&bb)) {
            continue;
        }
        let size = (bb[2] - bb[0]).max(bb[3] - bb[1]);
        // The outline OCR read as lettering keeps its strokes on its own layer: a glyph's are
        // text, a symbol it took for one (a pad's square) is still drawn.
        let glyph = layer.eq_ignore_ascii_case(&r.glyph_layer);
        if glyph && size < r.glyph_max_h * h {
            continue;
        }
        let mut shape = |pts: Vec<[f64; 2]>| {
            shapes.push(Shape { handle: e.handle, layer: layer.to_string(), min: [bb[0], bb[1]], max: [bb[2], bb[3]], pts });
        };
        match &e.kind {
            EntityKind::LwPolyline(p) => {
                let pts: Vec<[f64; 2]> = p.vertices.iter().take(10_000).map(|v| [v.p.x, v.p.y]).collect();
                let n = pts.len();
                let diag = (bb[2] - bb[0]).hypot(bb[3] - bb[1]);
                let ends_meet = match (pts.first(), pts.last()) {
                    (Some(a), Some(z)) => n >= 4 && dist(*a, *z) <= touch.max(0.1 * diag),
                    _ => false,
                };
                if (p.closed && n >= 3) || ends_meet {
                    let a = poly_area(&pts);
                    if n >= 4 && a >= (15.0 * h).powi(2) && outline_pool.len() < MAX_OUTLINES * 20 {
                        let up = layer.to_ascii_uppercase();
                        let named = words_of(&up).iter().any(|w| r.building_layer_words.iter().any(|x| x == w));
                        outline_pool.push((pts.clone(), a, named));
                    }
                    shape(pts);
                } else if !glyph {
                    let mut pieces = Vec::new();
                    super::gridtable::entity_segments(&e.kind, &mut pieces);
                    let li = layer_ix.len();
                    let li = *layer_ix.entry(layer.to_string()).or_insert(li);
                    out.segs.extend(pieces.into_iter().map(|(a, b)| Seg { a: [a.x, a.y], b: [b.x, b.y], layer: li }));
                }
            }
            EntityKind::Line(l) if !glyph => {
                let li = layer_ix.len();
                let li = *layer_ix.entry(layer.to_string()).or_insert(li);
                out.segs.push(Seg { a: [l.a.x, l.a.y], b: [l.b.x, l.b.y], layer: li });
            }
            EntityKind::Circle(_) | EntityKind::Insert(_) => shape(Vec::new()),
            _ => {}
        }
    }
    let mut layers: Vec<(String, usize)> = layer_ix.into_iter().collect();
    layers.sort_by_key(|(_, i)| *i);
    out.layers = layers.into_iter().map(|(l, _)| l).collect();
    // The building outlines: those on a layer named for the building first, then the largest.
    outline_pool.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.total_cmp(&a.1)));
    out.outlines = outline_pool.into_iter().take(MAX_OUTLINES).map(|(p, _, named)| (p, named)).collect();
    let big = |s: &Shape| out.outlines.iter().any(|(o, _)| o.len() == s.pts.len() && o.first() == s.pts.first());
    // The luminaires: each type tag the fixture schedule lists, at the symbol by it.
    let symbol_max = r.symbol_max_ft * upf;
    let (reach, join) = (r.symbol_reach_h * h, r.symbol_join_h * h);
    let mut used: BTreeSet<Handle> = BTreeSet::new();
    for t in &texts {
        if out.lights.len() >= MAX_ITEMS {
            break;
        }
        let mark = t.value.trim().to_ascii_uppercase();
        if !lum_types.contains(&mark) {
            continue;
        }
        let tb = text_box(t);
        let light = match cluster_near(&shapes, &used, &tb, reach, join, symbol_max, &big) {
            Some((bb, members)) => {
                let symbol = members.first().copied();
                used.extend(members);
                Light { mark, at: centre(&bb), bbox: bb, symbol, circuit: None }
            }
            None => Light { mark, at: centre(&tb), bbox: tb, symbol: None, circuit: None },
        };
        out.lights.push(light);
    }
    // The boxes in grade their marks name (`J`, `PB`), at the symbol by each.
    let pull_box = r.equipment.iter().find(|k| k.kind == "pull_box");
    for t in &texts {
        let mark = t.value.trim().to_ascii_uppercase();
        if out.equipment.len() >= MAX_ITEMS || !r.box_marks.iter().any(|m| *m == mark) {
            continue;
        }
        let tb = text_box(t);
        if let Some((bb, members)) = cluster_near(&shapes, &used, &tb, reach, join, symbol_max, &big)
            && let Some(symbol) = members.first().copied()
        {
            used.extend(members);
            out.equipment.push(Equip {
                kind: "pull_box".into(),
                label: mark,
                bbox: bb,
                symbol,
                utility: false,
                height_in: pull_box.map_or(24.0, |k| k.height_in),
                circuits: Vec::new(),
            });
        }
    }
    // The site equipment its labels name, at the symbol by each.
    let equip_words: BTreeSet<&str> = r.equipment.iter().flat_map(|k| k.words.iter().flat_map(|w| words_of(w))).collect();
    let mut labelled: BTreeSet<usize> = BTreeSet::new();
    for (i, t) in texts.iter().enumerate() {
        if out.equipment.len() >= MAX_ITEMS || labelled.contains(&i) {
            continue;
        }
        // A line with a word of an equipment's name (`MONUMENT` over `SIGN`): its label read.
        let up = t.value.to_ascii_uppercase();
        if !words_of(&up).iter().any(|w| equip_words.contains(*w)) {
            continue;
        }
        let lines = label_lines(&texts, i, &r.action_words);
        labelled.extend(lines.iter().copied());
        let label = lines.iter().filter_map(|j| texts.get(*j)).map(|x| x.value.trim()).collect::<Vec<_>>().join(" ");
        let label_up = label.to_ascii_uppercase();
        if label.chars().count() > r.label_max_chars || has_any(&label_up, &r.action_words) {
            continue;
        }
        let Some(kind) = r.equipment.iter().find(|k| has_any(&label_up, &k.words)) else { continue };
        let lb = lines.iter().filter_map(|j| texts.get(*j)).map(|x| text_box(x)).fold(text_box(t), union);
        let symbol = shapes
            .iter()
            .filter(|s| {
                let size = s.size();
                size >= r.equipment_min_ft * upf && size <= r.equipment_max_ft * upf && !big(s) && !used.contains(&s.handle)
            })
            .map(|s| (s, box_gap(&lb, &s.bbox())))
            .filter(|(_, g)| *g <= r.equipment_reach_h * h)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(s, _)| s);
        match symbol {
            Some(s) => {
                used.insert(s.handle);
                out.equipment.push(Equip {
                    kind: kind.kind.clone(),
                    label: label.chars().take(r.label_max_chars).collect(),
                    bbox: s.bbox(),
                    symbol: s.handle,
                    utility: has_any(&label_up, &r.utility_words),
                    height_in: kind.height_in,
                    circuits: Vec::new(),
                });
            }
            None => out.unplaced_labels.push(label),
        }
    }
    // The panel circuit tags; one by a symbol no type mark names marks that symbol (a site plan
    // tagging its pole lights by their circuit): what its schedule's load is says what it is.
    for t in &texts {
        if out.tags.len() >= MAX_ITEMS {
            break;
        }
        let Some((label, _, _)) = circuit_label(&t.value) else { continue };
        let tb = text_box(t);
        let typed = out.lights.iter().any(|l| l.circuit.is_none() && box_gap(&tb, &l.bbox) <= reach);
        // By a piece of site equipment its label names (a sign): that equipment's circuit.
        let by_equipment = out
            .equipment
            .iter_mut()
            .filter(|q| q.kind != "pull_box")
            .map(|q| {
                let g = box_gap(&tb, &q.bbox);
                (q, g)
            })
            .filter(|(_, g)| *g <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((q, _)) = by_equipment {
            if !q.circuits.contains(&label) && q.circuits.len() < 12 {
                q.circuits.push(label.clone());
            }
        } else if !typed
            && out.lights.len() < MAX_ITEMS
            && let Some((bb, members)) = cluster_near(&shapes, &used, &tb, reach, join, symbol_max, &big)
        {
            let symbol = members.first().copied();
            used.extend(members);
            out.lights.push(Light { mark: String::new(), at: centre(&bb), bbox: bb, symbol, circuit: Some(label.clone()) });
        }
        out.tags.push(CircuitTag { label, bbox: tb });
    }
    // The conduit notes (the underground readers' callouts) and where their lines are.
    for c in super::ugplan::callouts(sheet, &owned).into_iter().filter(|c| c.trade == "E").take(MAX_ITEMS) {
        let up = c.text.to_ascii_uppercase();
        let Some(first) = texts.iter().position(|t| {
            let v = t.value.trim().to_ascii_uppercase();
            v.chars().count() >= 6 && up.starts_with(&v)
        }) else {
            continue;
        };
        let bbox = block(&texts, first)
            .iter()
            .filter_map(|j| texts.get(*j))
            .filter(|x| up.contains(&x.value.trim().to_ascii_uppercase()))
            .map(|x| text_box(x))
            .fold(texts.get(first).map_or([0.0; 4], |t| text_box(t)), union);
        out.notes.push(Note { callout: c, bbox });
    }
    out
}

// ---------------- laying the sheet onto the model ----------------

/// How a site sheet was laid onto the slab level's plan.
#[derive(Clone, Debug)]
pub(crate) struct Fit {
    pub reg: SheetRegistration,
    /// `building outline` (fitted to the footprint), `outline centre` (by its centre: the sizes
    /// differ), `view centre` (no outline drawn), `none`.
    pub method: &'static str,
    pub why: String,
    /// The building outline on the model (empty: none read).
    pub outline: Vec<[f64; 2]>,
}

impl Fit {
    pub(crate) fn to_model(&self, p: [f64; 2]) -> [f64; 2] {
        self.reg.to_project(p)
    }
    /// Laid by the building it draws, at the footprint's size.
    pub(crate) fn fitted(&self) -> bool {
        self.method == "building outline"
    }
}

/// The outline's walls' direction, square-wise (radians, in [-π/4, π/4)): its edges' directions
/// four-fold, weighted by length.
fn wall_angle(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    let (mut c, mut s) = (0.0, 0.0);
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n)) else { continue };
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy);
        if l > 0.0 {
            let t = dy.atan2(dx) * 4.0;
            c += l * t.cos();
            s += l * t.sin();
        }
    }
    s.atan2(c) / 4.0
}

/// The site sheet laid onto the slab level's plan by the building outline it draws.
/// `sheet_real`: the sheet was imported at its printed scale (its feet are the model's).
pub(crate) fn register(read: &SiteRead, sp: &BuildingSpec, slab: &str, sheet_real: bool) -> Fit {
    let r = &rules().register;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let reference = sp
        .sheets
        .iter()
        .find(|l| {
            l.level.as_deref().is_some_and(|v| v.eq_ignore_ascii_case(slab)) && l.discipline.as_deref().is_some_and(|d| d.eq_ignore_ascii_case("A"))
        })
        .map_or_else(|| slab.to_string(), |l| l.number.clone());
    let none = |why: String| Fit {
        reg: SheetRegistration { reference: reference.clone(), method: "none".into(), a: 1.0, b: 0.0, tx: 0.0, ty: 0.0, matched: 0, residual: None },
        method: "none",
        why,
        outline: Vec::new(),
    };
    let Some(fp) = super::ugplan::slab_box(sp, slab) else { return none(format!("{slab} has no outline or region to lay the site plan on")) };
    let shells: Vec<[f64; 4]> = sp
        .shells
        .iter()
        .find(|(l, _)| l.eq_ignore_ascii_case(slab))
        .map(|(_, b)| b.iter().filter(|x| x.iter().all(|v| v.is_finite())).copied().collect())
        .unwrap_or_default();
    let fc = centre(&fp);
    let fp_area = ((fp[2] - fp[0]) * (fp[3] - fp[1])).abs();
    // The outline: one on a layer named for the building (the largest), else the one nearest the
    // footprint's size.
    let named = read.outlines.iter().find(|(_, n)| *n).map(|(p, _)| p);
    let sized = || {
        read.outlines
            .iter()
            .map(|(p, _)| p)
            .filter(|p| {
                sheet_real && fp_area > 0.0 && {
                    let ratio = poly_area(p) / fp_area;
                    ratio >= r.area_ratio_min * 0.5 && ratio <= r.area_ratio_max
                }
            })
            .min_by(|a, b| ((poly_area(a) / fp_area).ln().abs()).total_cmp(&(poly_area(b) / fp_area).ln().abs()))
    };
    let Some(poly) = named.or_else(sized).filter(|p| p.len() >= 3) else {
        // No building drawn: the view's middle on the footprint's.
        let ac = centre(&read.area);
        return Fit {
            reg: SheetRegistration {
                reference: reference.clone(),
                method: "view centre".into(),
                a: 1.0,
                b: 0.0,
                tx: fc[0] - ac[0],
                ty: fc[1] - ac[1],
                matched: 0,
                residual: None,
            },
            method: "view centre",
            why: "no building outline read on the site plan: its view's centre put on the building's (verify where the site work stands)".into(),
            outline: Vec::new(),
        };
    };
    let theta = wall_angle(poly);
    let rot = |p: [f64; 2], phi: f64, s: f64| [s * (phi.cos() * p[0] - phi.sin() * p[1]), s * (phi.sin() * p[0] + phi.cos() * p[1])];
    let tol = r.on_wall_ft.max(0.5) * upf;
    let mut best: Option<(f64, f64, f64, f64, [f64; 2])> = None; // (score, |phi|, phi, s, t)
    for k in 0..4 {
        let mut phi = -theta + f64::from(k) * std::f64::consts::FRAC_PI_2;
        while phi > std::f64::consts::PI {
            phi -= 2.0 * std::f64::consts::PI;
        }
        while phi <= -std::f64::consts::PI {
            phi += 2.0 * std::f64::consts::PI;
        }
        let turned: Vec<[f64; 2]> = poly.iter().map(|p| rot(*p, phi, 1.0)).collect();
        let tb = bbox_of(&turned);
        let t_area = ((tb[2] - tb[0]) * (tb[3] - tb[1])).abs();
        let s = if sheet_real || t_area <= 0.0 || fp_area <= 0.0 { 1.0 } else { (fp_area / t_area).sqrt() };
        let placed: Vec<[f64; 2]> = turned.iter().map(|p| [p[0] * s, p[1] * s]).collect();
        let pc = centre(&bbox_of(&placed));
        let t = [fc[0] - pc[0], fc[1] - pc[1]];
        let on = placed
            .iter()
            .map(|p| [p[0] + t[0], p[1] + t[1]])
            .filter(|p| {
                if shells.is_empty() {
                    let inner = [fp[0] + tol, fp[1] + tol, fp[2] - tol, fp[3] - tol];
                    gap(&fp, *p) <= tol && !(inner[0] < inner[2] && inner[1] < inner[3] && gap(&inner, *p) <= 0.0)
                } else {
                    shells.iter().any(|b| gap(b, *p) <= tol)
                }
            })
            .count();
        let score = on as f64 / placed.len().max(1) as f64;
        let better = best.is_none_or(|(bs, bphi, ..)| score > bs + 1e-9 || ((score - bs).abs() <= 1e-9 && phi.abs() < bphi));
        if better {
            best = Some((score, phi.abs(), phi, s, t));
        }
    }
    let Some((score, _, phi, s, t)) = best else { return none("the building outline gave no fit".into()) };
    let (a, b) = (s * phi.cos(), s * phi.sin());
    let reg = SheetRegistration { reference, method: String::new(), a, b, tx: t[0], ty: t[1], matched: poly.len(), residual: None };
    let outline: Vec<[f64; 2]> = poly.iter().map(|p| reg.to_project(*p)).collect();
    let ob = bbox_of(&outline);
    let ratio = if fp_area > 0.0 { ((ob[2] - ob[0]) * (ob[3] - ob[1])).abs() / fp_area } else { 0.0 };
    let fits = ratio >= r.area_ratio_min && ratio <= r.area_ratio_max && (!sheet_real || (s - 1.0).abs() <= r.scale_tolerance);
    let turn = phi.to_degrees();
    let (method, why) = if fits {
        (
            "building outline",
            format!(
                "laid on {slab} by the building outline it draws: turned {turn:.1} deg, its walls on the model's ({:.0}% of its corners), {:.0}% of the footprint's size",
                score * 100.0,
                ratio * 100.0
            ),
        )
    } else {
        (
            "outline centre",
            format!(
                "the building outline it draws is {:.0}% of {slab}'s footprint ({:.0} x {:.0} ft against {:.0} x {:.0} ft): placed by its centre, turned {turn:.1} deg (the model's plan may not be at its printed scale; verify where the site work stands)",
                ratio * 100.0,
                (ob[2] - ob[0]) / upf,
                (ob[3] - ob[1]) / upf,
                (fp[2] - fp[0]) / upf,
                (fp[3] - fp[1]) / upf
            ),
        )
    };
    Fit { reg: SheetRegistration { method: method.into(), ..reg }, method, why, outline }
}

// ---------------- the drawn lines ----------------

/// The drawn lines of a sheet: its pieces joined where they meet or a dashed line runs on.
struct Net {
    /// Each piece's line (a group of joined pieces).
    comp: Vec<usize>,
    /// The joins: piece end to piece end (`2i` = piece `i`'s start, `2i + 1` its end), and their
    /// lengths.
    joins: Vec<(usize, usize, f64)>,
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while let Some(&p) = parent.get(i) {
        if p == i {
            break;
        }
        let gp = parent.get(p).copied().unwrap_or(p);
        if let Some(x) = parent.get_mut(i) {
            *x = gp;
        }
        i = p;
    }
    i
}

fn net(segs: &[Seg], h: f64) -> Net {
    let r = rules();
    let gap_max = r.line_gap_h * h;
    let break_max = r.line_break_h.max(r.line_gap_h) * h;
    let touch = r.line_touch_h * h;
    let cos_turn = r.line_turn_deg.to_radians().cos();
    let cos_straight = r.line_break_deg.to_radians().cos();
    let n = segs.len();
    let mut parent: Vec<usize> = (0..n).collect();
    let mut joins = Vec::new();
    let cell = break_max.max(1e-9);
    let key = |p: [f64; 2]| ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64);
    let end = |i: usize, e: usize| segs.get(i).map(|s| if e == 0 { (s.a, s.b) } else { (s.b, s.a) });
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for i in 0..n {
        for e in 0..2 {
            if let Some((p, _)) = end(i, e) {
                grid.entry(key(p)).or_default().push(2 * i + e);
            }
        }
    }
    let unit = |a: [f64; 2], b: [f64; 2]| {
        let l = dist(a, b);
        if l > 1e-12 { Some([(b[0] - a[0]) / l, (b[1] - a[1]) / l]) } else { None }
    };
    for i in 0..n {
        for e in 0..2 {
            let Some((p, other)) = end(i, e) else { continue };
            let (kx, ky) = key(p);
            let layer = segs.get(i).map_or(usize::MAX, |s| s.layer);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let Some(list) = grid.get(&(kx + dx, ky + dy)) else { continue };
                    for &q in list {
                        let (j, f) = (q / 2, q % 2);
                        if j <= i || segs.get(j).is_none_or(|s| s.layer != layer) {
                            continue;
                        }
                        let Some((pq, other_q)) = end(j, f) else { continue };
                        let d = dist(p, pq);
                        if d > break_max {
                            continue;
                        }
                        // Pieces meeting join; across a gap, only where the line runs on (across a
                        // long one, where it runs on straight: a break for its label).
                        let limit = if d <= gap_max { cos_turn } else { cos_straight };
                        let runs_on = d <= touch
                            || match (unit(other, p), unit(p, pq), unit(pq, other_q)) {
                                (Some(out), Some(g), Some(into)) => {
                                    out[0] * g[0] + out[1] * g[1] >= limit && g[0] * into[0] + g[1] * into[1] >= limit
                                }
                                _ => false,
                            };
                        if runs_on {
                            joins.push((2 * i + e, 2 * j + f, d));
                            let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                            if ri != rj
                                && let Some(x) = parent.get_mut(ri)
                            {
                                *x = rj;
                            }
                        }
                    }
                }
            }
        }
    }
    let comp: Vec<usize> = (0..n).map(|i| find(&mut parent, i)).collect();
    Net { comp, joins }
}

/// A drawn line's longest path, as points: its pieces and joins walked end to end.
fn line_path(segs: &[Seg], members: &[usize], joins: &[(usize, usize, f64)]) -> Vec<[f64; 2]> {
    let set: BTreeSet<usize> = members.iter().copied().collect();
    let mut adj: BTreeMap<usize, Vec<(usize, f64)>> = BTreeMap::new();
    let point = |node: usize| segs.get(node / 2).map(|s| if node.is_multiple_of(2) { s.a } else { s.b });
    for &i in members {
        if let Some(s) = segs.get(i) {
            let l = dist(s.a, s.b);
            adj.entry(2 * i).or_default().push((2 * i + 1, l));
            adj.entry(2 * i + 1).or_default().push((2 * i, l));
        }
    }
    for &(a, b, d) in joins.iter().filter(|(a, b, _)| set.contains(&(a / 2)) && set.contains(&(b / 2))) {
        adj.entry(a).or_default().push((b, d));
        adj.entry(b).or_default().push((a, d));
    }
    let far = |from: usize| -> (usize, BTreeMap<usize, usize>) {
        let mut best: BTreeMap<usize, f64> = BTreeMap::new();
        let mut prev: BTreeMap<usize, usize> = BTreeMap::new();
        let mut heap = BinaryHeap::new();
        best.insert(from, 0.0);
        heap.push(Reverse((ordered(0.0), from)));
        while let Some(Reverse((d, u))) = heap.pop() {
            let d = d.0;
            if best.get(&u).is_some_and(|b| d > *b + 1e-12) {
                continue;
            }
            for &(v, w) in adj.get(&u).map(Vec::as_slice).unwrap_or_default() {
                let nd = d + w;
                if best.get(&v).is_none_or(|b| nd < *b - 1e-12) {
                    best.insert(v, nd);
                    prev.insert(v, u);
                    heap.push(Reverse((ordered(nd), v)));
                }
            }
        }
        let end = best.iter().max_by(|a, b| a.1.total_cmp(b.1)).map_or(from, |(k, _)| *k);
        (end, prev)
    };
    let Some(&start) = adj.keys().next() else { return Vec::new() };
    let (a, _) = far(start);
    let (b, prev) = far(a);
    let mut nodes = vec![b];
    let mut cur = b;
    while cur != a {
        match prev.get(&cur) {
            Some(&p) if nodes.len() <= 2 * MAX_COMPONENT + 2 => {
                nodes.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    let mut pts: Vec<[f64; 2]> = nodes.into_iter().filter_map(point).collect();
    pts.dedup_by(|x, y| dist(*x, *y) < 1e-9);
    pts
}

/// An `f64` the heap orders.
#[derive(Clone, Copy, Debug)]
struct Ord64(f64);
impl PartialEq for Ord64 {
    fn eq(&self, o: &Self) -> bool {
        self.0.total_cmp(&o.0).is_eq()
    }
}
impl Eq for Ord64 {}
impl PartialOrd for Ord64 {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Ord64 {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&o.0)
    }
}
fn ordered(v: f64) -> Ord64 {
    Ord64(v)
}

/// A path moved `d` to its left (each corner on both its sides' offsets).
fn offset_path(pts: &[[f64; 2]], d: f64) -> Vec<[f64; 2]> {
    if d.abs() < 1e-12 || pts.len() < 2 {
        return pts.to_vec();
    }
    let normal = |a: [f64; 2], b: [f64; 2]| {
        let l = dist(a, b);
        if l > 1e-12 { Some([-(b[1] - a[1]) / l, (b[0] - a[0]) / l]) } else { None }
    };
    let n = pts.len();
    (0..n)
        .map(|i| {
            let before = i.checked_sub(1).and_then(|j| pts.get(j).zip(pts.get(i))).and_then(|(a, b)| normal(*a, *b));
            let after = pts.get(i + 1).and_then(|b| normal(*pts.get(i)?, *b));
            let p = pts.get(i).copied().unwrap_or_default();
            let m = match (before, after) {
                (Some(a), Some(b)) => {
                    let s = [a[0] + b[0], a[1] + b[1]];
                    let dot = (1.0 + a[0] * b[0] + a[1] * b[1]) / 2.0;
                    // The corner's miter, kept short at a sharp turn.
                    let k = if dot > 0.1 { 1.0 / (2.0 * dot) } else { 5.0 };
                    [s[0] * k, s[1] * k]
                }
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => [0.0, 0.0],
            };
            [p[0] + m[0] * d, p[1] + m[1] * d]
        })
        .collect()
}

/// Whether two runs lie in one trench: as many points, each within `tol` of the other's.
fn same_trench(a: &[[f64; 2]], b: &[[f64; 2]], tol: f64) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(p, q)| dist(*p, *q) <= tol)
}

fn path_len(pts: &[[f64; 2]]) -> f64 {
    pts.windows(2)
        .map(|w| match w {
            [a, b] => dist(*a, *b),
            _ => 0.0,
        })
        .sum()
}

// ---------------- what is built ----------------

/// What a drawn line ends at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Obj {
    Light(usize),
    Equip(usize),
    Tag(usize),
    Building,
}

/// Counters across a job's site sheets: ids stay unique.
#[derive(Clone, Debug, Default)]
pub(crate) struct Seq {
    pub poles: usize,
    pub equipment: usize,
    /// Luminaires already built (model point, mark): a second site plan's same light is the same.
    pub lights: Vec<([f64; 2], String)>,
}

/// What one site sheet builds.
#[derive(Clone, Debug, Default)]
pub(crate) struct Built {
    pub tags: Vec<ElementTag>,
    pub equipment: Vec<MepEquipment>,
    /// Site gear already in the model (the gear step's, by rule) moved to where the site plan
    /// draws it: its tag, its new box, why.
    pub moved: Vec<(String, [f64; 2], [f64; 2], String)>,
    pub concrete: Vec<ConcreteElement>,
    pub pads: Vec<PadDecision>,
    pub runs: Vec<MepRun>,
    /// The circuits built (`LP-7`) and their luminaires.
    pub circuits: BTreeMap<String, usize>,
    /// What the job is told (each a finding line).
    pub findings: Vec<String>,
    pub lights_on_building: usize,
    pub equipment_on_building: usize,
    pub poles: usize,
    pub drawn_runs: usize,
    pub inferred_runs: usize,
    pub called_runs: usize,
}

fn ft(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if (r - r.round()).abs() < 1e-9 { format!("{r:.0}") } else { format!("{r:.1}") }
}

/// A pole height in feet a luminaire's words give (`MOUNTED ON 12' POLE`, `25 FT POLE`, `POLE
/// HEIGHT 20'-0"`).
pub(crate) fn pole_height_ft(up: &str) -> Option<f64> {
    let toks: Vec<&str> = up.split_whitespace().take(400).collect();
    let feet = |t: &str| -> Option<f64> {
        let t = t.trim_matches(|c: char| matches!(c, ',' | '.' | ';' | '(' | ')'));
        let (num, rest) = match t.find('\'') {
            Some(i) => (t.get(..i)?, t.get(i..)?),
            None => {
                let digits: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                (t.get(..digits.len())?, t.get(digits.len()..)?)
            }
        };
        let v: f64 = num.parse().ok()?;
        let unit_ok = rest.starts_with('\'') || rest.starts_with("FT") || rest.starts_with("FOOT") || rest.starts_with("FEET");
        (unit_ok && v.is_finite() && v > 3.0 && v <= 120.0).then_some(v)
    };
    for (i, t) in toks.iter().enumerate() {
        if !t.starts_with("POLE") {
            continue;
        }
        let back = toks.get(i.saturating_sub(4)..i).unwrap_or_default();
        for (j, b) in back.iter().enumerate().rev() {
            if let Some(v) = feet(b) {
                return Some(v);
            }
            // `25 FT POLE`: the number before its unit.
            if matches!(*b, "FT" | "FT." | "FOOT" | "FEET")
                && let Some(v) = j.checked_sub(1).and_then(|k| back.get(k)).and_then(|n| n.parse::<f64>().ok()).filter(|v| *v > 3.0 && *v <= 120.0)
            {
                return Some(v);
            }
        }
        for f in toks.get(i + 1..(i + 4).min(toks.len())).unwrap_or_default() {
            if let Some(v) = feet(f) {
                return Some(v);
            }
        }
    }
    None
}

fn base_for(pole_ft: f64) -> [f64; 4] {
    let p = &rules().pole;
    p.bases.iter().find(|b| pole_ft <= b[0]).or(p.bases.last()).copied().unwrap_or([pole_ft, 24.0, 72.0, 30.0])
}

/// A buried run (`E-UG`) of the site work: `pts` model points at the cover for its size.
fn buried(slab: &str, sheet: &str, pts: Vec<[f64; 2]>, size: f64, made: Made, basis: String, size_from: &str) -> MepRun {
    let r = rules();
    let bottom = -(r.cover_in + size) / 12.0;
    let mut run = super::printcond::run(super::ugplan::E_UG, slab, &format!("{SOURCE_PREFIX}{sheet}"), pts, size, bottom);
    run.height_source = match made {
        Made::Drawn => format!("drawn: {sheet} (site plan), at {} in cover ({})", ft(r.cover_in), r.cover_source),
        // ugsheets shows proposed work dashed by this word.
        _ => "proposed".into(),
    };
    run.size_source = size_from.to_string();
    run.made = made;
    run.basis = basis;
    run
}

/// A stub-up out of the ground at `at` to `top_ft` above grade.
fn stub(slab: &str, sheet: &str, at: [f64; 2], size: f64, top_ft: f64, made: Made, basis: &str) -> MepRun {
    let r = rules();
    let mut run = buried(slab, sheet, vec![at], size, made, format!("{basis}; {}", r.stub_source), "as its run");
    run.top_ft = Some(top_ft);
    run
}

/// Everything one site sheet builds on the slab level `slab`: `fit` lays it onto the model.
pub(crate) fn plan(sp: &BuildingSpec, read: &SiteRead, fit: &Fit, slab: &str, seq: &mut Seq) -> Built {
    let r = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let sheet = read.sheet.as_str();
    let h = read.h;
    let mut out = Built::default();
    if !(h.is_finite() && h > 0.0) {
        return out;
    }
    let to_model = |p: [f64; 2]| fit.to_model(p);
    let building: Vec<[f64; 2]> = if fit.outline.len() >= 3 {
        fit.outline.clone()
    } else {
        super::ugplan::slab_box(sp, slab).map(|b| vec![[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]).unwrap_or_default()
    };
    let placed_by = if fit.fitted() { format!("{sheet} (site plan, {})", fit.why) } else { format!("{sheet} (site plan; {})", fit.why) };
    // (1) The luminaires: each on its pole and base, unless on the building or a ground light. A
    // symbol only its circuit tags is what that circuit's load is (a sign, a charger: equipment).
    let mut light_ids: Vec<Option<usize>> = vec![None; read.lights.len()];
    let mut light_equip: Vec<Option<usize>> = vec![None; read.lights.len()];
    let mut light_top: Vec<f64> = vec![0.0; read.lights.len()];
    // The fixture schedule's pole or site types: an untyped site light is the one there is.
    let site_types: Vec<&cadaddy_walk3d::FixtureType> = sp
        .mep
        .fixture_schedule
        .iter()
        .filter(|x| has_any(&format!("{} {}", x.description, x.installation).to_ascii_uppercase(), &r.site_type_words))
        .collect();
    for (i, l) in read.lights.iter().enumerate() {
        let m = to_model(l.at);
        if building.len() >= 3 && (inside(&building, m) || nearest_on(&building, m).is_some_and(|q| dist(q, m) <= upf)) {
            out.lights_on_building += 1;
            continue;
        }
        let load = l.circuit.as_deref().and_then(|c| circuit_load(sp, c));
        let load_up = load.as_ref().map(|(d, _)| d.to_ascii_uppercase()).unwrap_or_default();
        let equipment_kind = (l.mark.is_empty() && !has_any(&load_up, &r.site_light_words))
            .then(|| r.equipment.iter().find(|k| has_any(&load_up, &k.words)).map(|k| (k.kind.clone(), k.height_in)))
            .flatten();
        if let (Some((kind, height_in)), Some((desc, src))) = (equipment_kind, load.as_ref()) {
            // Site equipment its circuit's load names, where its symbol is drawn.
            seq.equipment += 1;
            let half = ((l.bbox[2] - l.bbox[0]).max(l.bbox[3] - l.bbox[1]) / 2.0 * fit.reg.scale()).max(0.5 * upf);
            let mut props = BTreeMap::new();
            props.insert("NAME".into(), desc.clone());
            props.insert("CIRCUIT".into(), l.circuit.clone().unwrap_or_default());
            props.insert("PROVENANCE".into(), "drawn".into());
            props.insert("TRADE".into(), "electrical".into());
            props.insert("BASIS".into(), format!("drawn on {placed_by}: a symbol its circuit tags; what it is, the circuit's load on {src}"));
            let mut sources = BTreeMap::new();
            sources.insert(super::utilityxfmr::SITE_SOURCE.to_string(), "outside the building".to_string());
            sources.insert(MARK.to_string(), sheet.to_string());
            sources.insert("position".into(), format!("drawn: {placed_by}"));
            sources.insert("height".into(), format!("rule: {}", r.equipment_source));
            if let Some(x) = light_equip.get_mut(i) {
                *x = Some(out.equipment.len());
            }
            out.equipment.push(MepEquipment {
                tag: format!("SITE-{}-{}", kind.to_ascii_uppercase().replace('_', "-"), seq.equipment),
                kind,
                level: slab.to_string(),
                min: [m[0] - half, m[1] - half],
                max: [m[0] + half, m[1] + half],
                height_ft: height_in / 12.0,
                title: desc.clone(),
                properties: props,
                sources,
                confidence: 0.6,
                ..MepEquipment::default()
            });
            continue;
        }
        // An untyped site light: the schedule's one site type, else asked.
        let (mark, mark_from) = if !l.mark.is_empty() {
            (l.mark.clone(), "plan".to_string())
        } else if let [only] = site_types.as_slice() {
            (only.tag.clone(), format!("derived: the fixture schedule's one pole or site type ({}; {})", only.source, r.site_type_source))
        } else {
            out.findings.push(format!(
                "{sheet}: a site luminaire on {} marks no type and the fixture schedule has {} pole or site types: built untyped (which type is it?)",
                l.circuit.as_deref().unwrap_or("no circuit"),
                site_types.len()
            ));
            (String::new(), String::new())
        };
        if seq.lights.iter().any(|(p, k)| *k == mark && dist(*p, m) <= 3.0 * upf) {
            continue;
        }
        seq.lights.push((m, mark.clone()));
        let l = &Light { mark: mark.clone(), ..l.clone() };
        let row = {
            let ty = super::lightsched::type_of(&l.mark);
            sp.mep.fixture_schedule.iter().find(|x| !ty.is_empty() && x.tag == ty)
        };
        let words = row.map(|x| format!("{} {} {} {}", x.description, x.installation, x.notes, x.model).to_ascii_uppercase()).unwrap_or_default();
        let row_cite = row.map(|x| format!("{} type {}", x.source, x.tag)).unwrap_or_else(|| format!("type {}", l.mark));
        let at_ft = [m[0] / upf, m[1] / upf];
        let mut e = ElementTag {
            id: format!("device:{slab}:{:.1},{:.1}", at_ft[0], at_ft[1]),
            kind: "device".into(),
            level: slab.to_string(),
            at_ft,
            ..Default::default()
        };
        e.set("type", l.mark.clone(), if mark_from.starts_with("derived") { "schedule" } else { "plan" });
        if !mark_from.is_empty() && mark_from != "plan" {
            e.set("type_basis", mark_from.clone(), "schedule");
        }
        e.set(
            "device_type",
            if l.mark.is_empty() { "luminaire (site, type not marked)".to_string() } else { format!("luminaire {}", l.mark) },
            "schedule",
        );
        e.set("device_kind", "luminaire", "schedule");
        e.set("ifc_class", "IfcLightFixture", "schedule");
        e.set("sheet", sheet, "plan");
        e.set("symbol", l.symbol.map(|s| s.hex()).unwrap_or_default(), "plan");
        e.set(MARK, format!("drawn on {placed_by}"), "plan");
        if has_any(&words, &r.ground_words) {
            e.set("mount", "floor", "schedule");
            e.set("mount_height_in", "0", "schedule");
            e.set("mount_height_note", format!("at grade: {row_cite} ({})", words.trim().chars().take(80).collect::<String>()), "schedule");
            if let Some(x) = light_ids.get_mut(i) {
                *x = Some(out.tags.len());
            }
            out.tags.push(e);
            continue;
        }
        let (pole_ft, height_from) = match pole_height_ft(&words) {
            Some(v) => (v, format!("schedule: {row_cite}")),
            None if has_any(&words, &r.pole.words) => (r.pole.default_height_ft, format!("rule: {}", r.pole.height_source)),
            None => (
                r.pole.default_height_ft,
                format!(
                    "rule: a luminaire standing alone on the site plan is on a pole (usual; alternative: mounted on a structure the site plan doesn't show); {}",
                    r.pole.height_source
                ),
            ),
        };
        let base = base_for(pole_ft);
        let above_ft = base[3] / 12.0;
        let top_in = base[3] + pole_ft * 12.0;
        if let Some(t) = light_top.get_mut(i) {
            *t = above_ft;
        }
        e.set("mount", "pole", "schedule");
        e.set("mount_height_in", ft(top_in), if height_from.starts_with("schedule") { "schedule" } else { "rule" });
        e.set("mount_height_note", format!("on its {} ft pole ({height_from}) on its base {} in above grade", ft(pole_ft), ft(base[3])), "rule");
        seq.poles += 1;
        out.poles += 1;
        let n = seq.poles;
        let pole_tag = format!("POLE-{n}");
        let half = r.pole.size_in / 24.0 * upf;
        let mut props = BTreeMap::new();
        props.insert("POLE HEIGHT".into(), format!("{} ft", ft(pole_ft)));
        props.insert("FOR".into(), format!("luminaire {} at {:.1}, {:.1} ft", l.mark, at_ft[0], at_ft[1]));
        props.insert("PROVENANCE".into(), "derived".into());
        props.insert("TRADE".into(), "electrical".into());
        props.insert("BASIS".into(), format!("the luminaire's pole: {height_from}; {}", r.pole.size_source));
        let mut sources = BTreeMap::new();
        sources.insert(super::utilityxfmr::SITE_SOURCE.to_string(), "outside the building".to_string());
        sources.insert(MARK.to_string(), sheet.to_string());
        sources.insert("position".into(), format!("drawn: its luminaire on {placed_by}"));
        sources.insert("height".into(), height_from.clone());
        sources.insert("size".into(), format!("rule: {}", r.pole.size_source));
        out.equipment.push(MepEquipment {
            tag: pole_tag.clone(),
            kind: "pole".into(),
            level: slab.to_string(),
            min: [m[0] - half, m[1] - half],
            max: [m[0] + half, m[1] + half],
            bottom_ft: above_ft,
            height_ft: pole_ft,
            title: format!("light pole for {}", l.mark),
            properties: props,
            sources,
            confidence: 0.6,
            ..MepEquipment::default()
        });
        let rad = base[1] / 24.0 * upf;
        out.concrete.push(ConcreteElement {
            id: format!("{CONCRETE_PREFIX}{n}"),
            kind: "pole_base".into(),
            level: slab.to_string(),
            min: [m[0] - rad, m[1] - rad],
            max: [m[0] + rad, m[1] + rad],
            depth_in: base[2] + base[3],
            top_ft: above_ft,
            round: true,
            earth_formed: true,
            owner: "electrical".into(),
            source: format!(
                "inferred: {sheet} pole light {} ({} ft pole): {} in round, {} in deep, {} in above grade; {}",
                l.mark,
                ft(pole_ft),
                ft(base[1]),
                ft(base[2]),
                ft(base[3]),
                r.pole.base_source
            ),
            equipment: pole_tag,
            ..ConcreteElement::default()
        });
        if let Some(x) = light_ids.get_mut(i) {
            *x = Some(out.tags.len());
        }
        out.tags.push(e);
    }
    // (2) The site equipment: where the site plan draws it.
    let mut equip_at: Vec<Option<([f64; 2], [f64; 2], String)>> = vec![None; read.equipment.len()];
    for (i, q) in read.equipment.iter().enumerate() {
        let corners = [[q.bbox[0], q.bbox[1]], [q.bbox[2], q.bbox[1]], [q.bbox[2], q.bbox[3]], [q.bbox[0], q.bbox[3]]];
        let mb = bbox_of(&corners.iter().map(|p| to_model(*p)).collect::<Vec<_>>());
        let mc = centre(&mb);
        // In the building: its floor plan's (a box by the switchboard), not site work.
        if building.len() >= 3 && inside(&building, mc) {
            out.equipment_on_building += 1;
            continue;
        }
        // The gear step's transformer (proposed by rule outside the slab edge) is this one: moved
        // to where the site plan draws it, its size kept.
        let existing = sp.mep.equipment.iter().find(|e| {
            super::utilityxfmr::is_site(e)
                && !e.sources.contains_key(MARK)
                && e.kind.eq_ignore_ascii_case(&q.kind)
                && !out.moved.iter().any(|(t, ..)| t.eq_ignore_ascii_case(&e.tag))
        });
        if let Some(x) = existing {
            let (w, d) = ((x.max[0] - x.min[0]).abs() / 2.0, (x.max[1] - x.min[1]).abs() / 2.0);
            let why = format!("drawn on {placed_by} ({}); was placed by rule: {}", q.label, x.sources.get("position").cloned().unwrap_or_default());
            out.moved.push((x.tag.clone(), [mc[0] - w, mc[1] - d], [mc[0] + w, mc[1] + d], why));
            if let Some(v) = equip_at.get_mut(i) {
                *v = Some((mc, [mc[0] - w, mc[1] - d], x.tag.clone()));
            }
            continue;
        }
        seq.equipment += 1;
        let tag =
            if q.kind == "transformer" && q.utility && !sp.mep.equipment.iter().any(|e| e.tag.eq_ignore_ascii_case(&super::utilityxfmr::rules().tag))
            {
                super::utilityxfmr::rules().tag.clone()
            } else {
                format!("SITE-{}-{}", q.kind.to_ascii_uppercase().replace('_', "-"), seq.equipment)
            };
        let in_grade = q.kind == "pull_box";
        let mut props = BTreeMap::new();
        props.insert("NAME".into(), q.label.clone());
        props.insert("PROVENANCE".into(), "drawn".into());
        props.insert("TRADE".into(), "electrical".into());
        props.insert("BASIS".into(), format!("drawn on {placed_by}; its height the kind's usual: {}", r.equipment_source));
        props.insert("MOUNTING".into(), if in_grade { "in grade".into() } else { "on a concrete pad".into() });
        if q.utility {
            props.insert("FURNISHED BY".into(), super::utilityxfmr::rules().furnished_by.clone());
        }
        let mut sources = BTreeMap::new();
        sources.insert(super::utilityxfmr::SITE_SOURCE.to_string(), "outside the building".to_string());
        sources.insert(MARK.to_string(), sheet.to_string());
        sources.insert("position".into(), format!("drawn: {placed_by}"));
        sources.insert("size".into(), format!("drawn: its symbol on {sheet}"));
        sources.insert("symbol".into(), q.symbol.hex());
        sources.insert("height".into(), format!("rule: {}", r.equipment_source));
        let height_ft = q.height_in / 12.0;
        out.equipment.push(MepEquipment {
            tag: tag.clone(),
            kind: q.kind.clone(),
            level: slab.to_string(),
            min: [mb[0], mb[1]],
            max: [mb[2], mb[3]],
            bottom_ft: if in_grade { -height_ft } else { 0.0 },
            height_ft,
            title: q.label.clone(),
            properties: props,
            sources,
            confidence: 0.7,
            ..MepEquipment::default()
        });
        if !in_grade {
            let x = super::utilityxfmr::rules();
            out.pads.push(PadDecision {
                equipment: tag.clone(),
                required: Some(true),
                basis: format!("{} stands on the site: on a concrete pad ({})", q.label, x.pad_source),
                owner: if q.utility { x.pad_owner.clone() } else { "electrical".into() },
                margin_in: Some(x.pad_margin_in),
                height_in: Some(x.pad_height_in),
                ..PadDecision::default()
            });
        }
        if let Some(v) = equip_at.get_mut(i) {
            *v = Some((mc, [mb[0], mb[1]], tag));
        }
    }
    for l in &read.unplaced_labels {
        out.findings.push(format!(
            "{sheet}: \"{}\" names site equipment with no symbol by it: not built (locate it)",
            l.chars().take(80).collect::<String>()
        ));
    }
    // (3) The drawn lines and what each ends at.
    let reach = r.line_end_reach_h * h;
    let at_end = |p: [f64; 2]| -> Option<Obj> {
        let mut best: Option<(f64, Obj)> = None;
        let mut consider = |d: f64, o: Obj| {
            if d <= reach && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, o));
            }
        };
        for (i, l) in read.lights.iter().enumerate() {
            consider(gap(&l.bbox, p), Obj::Light(i));
        }
        for (i, q) in read.equipment.iter().enumerate() {
            consider(gap(&q.bbox, p), Obj::Equip(i));
        }
        for (i, t) in read.tags.iter().enumerate() {
            consider(gap(&t.bbox, p) + 0.5 * reach, Obj::Tag(i));
        }
        let sheet_outline = read.outlines.iter().find(|(o, _)| fit.outline.len() == o.len()).map(|(o, _)| o.as_slice());
        if let Some(q) = sheet_outline.filter(|_| fit.method != "view centre").and_then(|o| nearest_on(o, p)) {
            consider(dist(q, p) + 0.25 * reach, Obj::Building);
        }
        best.map(|(_, o)| o)
    };
    let nt = net(&read.segs, h);
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, c) in nt.comp.iter().enumerate() {
        let g = groups.entry(*c).or_default();
        if g.len() <= MAX_COMPONENT {
            g.push(i);
        }
    }
    struct Line {
        pts: Vec<[f64; 2]>,
        ends: (Option<Obj>, Option<Obj>),
    }
    let mut lines: Vec<Line> = Vec::new();
    for members in groups.values().filter(|m| m.len() <= MAX_COMPONENT) {
        // Only a line ending at one of the site's objects can be a run.
        let near_any = members.iter().filter_map(|i| read.segs.get(*i)).any(|s| at_end(s.a).is_some() || at_end(s.b).is_some());
        if !near_any {
            continue;
        }
        let pts = line_path(&read.segs, members, &nt.joins);
        let (Some(a), Some(z)) = (pts.first().copied(), pts.last().copied()) else { continue };
        lines.push(Line { pts, ends: (at_end(a), at_end(z)) });
    }
    // A note's leader: the line from the note to the run it names.
    let called: Vec<Option<usize>> = read
        .notes
        .iter()
        .map(|n| {
            let near_note = |p: [f64; 2]| gap(&n.bbox, p) <= 1.5 * h;
            let leader_end = groups
                .values()
                .filter(|m| m.len() <= 64)
                .filter_map(|m| {
                    let ends: Vec<[f64; 2]> = m.iter().filter_map(|i| read.segs.get(*i)).flat_map(|s| [s.a, s.b]).collect();
                    ends.iter().any(|p| near_note(*p)).then(|| ends.iter().copied().max_by(|a, b| gap(&n.bbox, *a).total_cmp(&gap(&n.bbox, *b))))?
                })
                .find(|p| gap(&n.bbox, *p) > 1.5 * h);
            let target = leader_end.unwrap_or_else(|| centre(&n.bbox));
            lines
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let d = l.pts.windows(2).filter_map(|w| match w {
                        [a, b] => Some(seg_dist(target, *a, *b)),
                        _ => None,
                    });
                    (i, d.fold(f64::INFINITY, f64::min))
                })
                .filter(|(_, d)| *d <= if leader_end.is_some() { reach } else { 2.0 * h })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        })
        .collect();
    let note_of = |line: usize| read.notes.iter().zip(&called).find(|(_, c)| **c == Some(line)).map(|(n, _)| n);
    // (4) The circuits: those tagged by a symbol; each other tag's luminaire, and those its drawn
    // lines reach.
    let built_light = |i: usize| light_ids.get(i).copied().flatten().is_some() || light_equip.get(i).copied().flatten().is_some();
    let mut light_circuit: Vec<Option<String>> = read.lights.iter().enumerate().map(|(i, l)| l.circuit.clone().filter(|_| built_light(i))).collect();
    let mut links: Vec<(usize, usize)> = Vec::new();
    for l in &lines {
        if let (Some(Obj::Light(a)), Some(Obj::Light(b))) = l.ends
            && a != b
        {
            links.push((a, b));
        }
    }
    let spread = |light_circuit: &mut Vec<Option<String>>, home: usize, label: &str| {
        let mut stack = vec![home];
        let mut steps = 0usize;
        while let Some(i) = stack.pop() {
            steps += 1;
            if steps > 10 * MAX_ITEMS || light_circuit.get(i).is_some_and(|c| c.as_deref().is_some_and(|x| x != label)) {
                continue;
            }
            if light_circuit.get(i).is_some_and(Option::is_some) && i != home {
                continue;
            }
            if let Some(c) = light_circuit.get_mut(i) {
                *c = Some(label.to_string());
            }
            for (a, b) in &links {
                let other = if *a == i {
                    *b
                } else if *b == i {
                    *a
                } else {
                    continue;
                };
                if light_circuit.get(other).is_some_and(Option::is_none) && built_light(other) {
                    stack.push(other);
                }
            }
        }
    };
    for (ti, t) in read.tags.iter().enumerate() {
        let drawn = lines.iter().find_map(|l| match l.ends {
            (Some(Obj::Tag(x)), Some(Obj::Light(y))) | (Some(Obj::Light(y)), Some(Obj::Tag(x))) if x == ti => Some(y),
            _ => None,
        });
        let home = drawn.or_else(|| {
            read.lights
                .iter()
                .enumerate()
                .filter(|(i, _)| built_light(*i))
                .map(|(i, l)| (i, box_gap(&t.bbox, &l.bbox)))
                .filter(|(_, g)| *g <= r.symbol_reach_h * h)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        });
        if let Some(home) = home {
            spread(&mut light_circuit, home, &t.label);
        }
    }
    // Those a circuit's drawn lines reach from its tagged symbols.
    for i in 0..read.lights.len() {
        if let Some(label) = read.lights.get(i).and_then(|l| l.circuit.clone()).filter(|_| built_light(i)) {
            spread(&mut light_circuit, i, &label);
        }
    }
    // A sheet of one circuit: its luminaires no line reaches are on it too (inferred).
    let mut inferred_links: Vec<(usize, usize)> = Vec::new();
    let labels: BTreeSet<&str> = read.tags.iter().map(|t| t.label.as_str()).collect();
    if let (1, Some(only)) = (labels.len(), labels.first()) {
        loop {
            let on: Vec<usize> = (0..read.lights.len()).filter(|i| light_circuit.get(*i).is_some_and(Option::is_some)).collect();
            let off = (0..read.lights.len())
                .filter(|i| light_circuit.get(*i).is_some_and(Option::is_none) && built_light(*i))
                .flat_map(|i| on.iter().map(move |j| (i, *j)))
                .filter_map(|(i, j)| Some((i, j, dist(read.lights.get(i)?.at, read.lights.get(j)?.at))))
                .min_by(|a, b| a.2.total_cmp(&b.2));
            let Some((i, j, _)) = off else { break };
            if let Some(c) = light_circuit.get_mut(i) {
                *c = Some((*only).to_string());
            }
            inferred_links.push((j, i));
        }
    }
    for (i, c) in light_circuit.iter().enumerate() {
        let Some(label) = c else { continue };
        if let Some(Some(ix)) = light_ids.get(i)
            && let Some(e) = out.tags.get_mut(*ix)
        {
            e.set("circuit", label.clone(), "plan");
        }
        if let Some(Some(ix)) = light_equip.get(i)
            && let Some(e) = out.equipment.get_mut(*ix)
        {
            e.properties.insert("CIRCUIT".into(), label.clone());
        }
        *out.circuits.entry(label.clone()).or_default() += 1;
    }
    let unassigned = (0..read.lights.len())
        .filter(|i| light_ids.get(*i).copied().flatten().is_some() && light_circuit.get(*i).is_some_and(Option::is_none))
        .count();
    if unassigned > 0 {
        out.findings.push(format!(
            "{sheet}: {unassigned} site luminaire(s) on no circuit the plan tags: built without their circuit (which circuit feeds them?)"
        ));
    }
    // The site equipment its labels name (a monument sign) on the panel schedules' circuits for
    // that load (their descriptions' site-load words): fed from them.
    let mut equip_circuits: Vec<(usize, Vec<String>)> = Vec::new();
    for (qi, q) in read.equipment.iter().enumerate() {
        if equip_at.get(qi).is_none_or(Option::is_none) {
            continue;
        }
        let up = q.label.to_ascii_uppercase();
        let words: Vec<&String> = r.site_load_words.iter().filter(|w| up.contains(w.as_str())).collect();
        // The circuits printed by it, else those its name's load is on.
        let mut found: Vec<String> = q.circuits.clone();
        let by_name = found.is_empty() && !words.is_empty();
        for ps in sp.mep.panel_schedules.iter().take(2_000).filter(|_| by_name) {
            for c in ps.circuits.iter().take(500) {
                let d = c.description.to_ascii_uppercase();
                if words.iter().any(|w| d.contains(w.as_str())) && found.len() < 12 {
                    found.push(format!("{}-{}", ps.panel.trim(), c.number.trim()));
                }
            }
        }
        if found.is_empty() {
            continue;
        }
        for f in &found {
            *out.circuits.entry(f.clone()).or_default() += 1;
        }
        if let Some(Some((_, _, tag))) = equip_at.get(qi)
            && let Some(e) = out.equipment.iter_mut().find(|e| e.tag.eq_ignore_ascii_case(tag))
        {
            e.properties.insert("CIRCUIT".into(), found.join(", "));
        }
        equip_circuits.push((qi, found));
    }
    // (5) The runs.
    let model_end = |o: Obj, fallback: [f64; 2]| -> [f64; 2] {
        match o {
            Obj::Light(i) => read.lights.get(i).map_or(to_model(fallback), |l| to_model(l.at)),
            Obj::Equip(i) => equip_at.get(i).cloned().flatten().map_or(to_model(fallback), |(c, ..)| c),
            Obj::Tag(_) => to_model(fallback),
            Obj::Building => nearest_on(&building, to_model(fallback)).unwrap_or(to_model(fallback)),
        }
    };
    let top_at = |o: Obj| -> f64 {
        match o {
            Obj::Light(i) => light_top.get(i).copied().unwrap_or(0.0) + r.stub_top_ft,
            _ => r.stub_top_ft,
        }
    };
    let feeder_from = |o: Obj| match o {
        Obj::Equip(i) => {
            equip_at.get(i).cloned().flatten().and_then(|(_, _, tag)| sp.mep.feeders.iter().find(|f| f.from.eq_ignore_ascii_case(&tag)).cloned())
        }
        _ => None,
    };
    let add_run = |out: &mut Built, pts: Vec<[f64; 2]>, ends: [Option<Obj>; 2], made: Made, note: Option<&Note>, why: &str, branch: bool| {
        let (count, size, size_from) = match note {
            Some(n) => (
                n.callout.count.clamp(1, MAX_CONDUITS),
                n.callout.size_in.unwrap_or(if branch { r.branch_conduit_in } else { super::keynotes::BURIED_MIN_IN }),
                if n.callout.size_in.is_some() {
                    format!("note: {}", n.callout.cite())
                } else {
                    format!("rule: the note gives no size ({})", r.branch_conduit_source)
                },
            ),
            None if branch => (1, r.branch_conduit_in, format!("rule: {}", r.branch_conduit_source)),
            None => (1, super::keynotes::BURIED_MIN_IN, "rule: smallest trade size (no note or schedule gives it)".to_string()),
        };
        let mut basis = match note {
            Some(n) => format!("{why}; called by {}: {}", n.callout.cite(), n.callout.text.chars().take(160).collect::<String>()),
            None => why.to_string(),
        };
        if count > 1 {
            basis = format!("{basis}; {} conduits, {}", count, r.parallel_source);
        }
        // Parallel conduits side by side in their trench, each its own run.
        let pitch = (size + r.parallel_spacing_in.max(0.0)) / 12.0 * upf;
        for k in 0..count {
            let pts = offset_path(&pts, (f64::from(k) - f64::from(count - 1) / 2.0) * pitch);
            if pts.len() >= 2 {
                out.runs.push(buried(slab, sheet, pts.clone(), size, made, basis.clone(), &size_from));
            }
            for (o, p) in ends.iter().zip([pts.first(), pts.last()]) {
                if let (Some(o), Some(p)) = (o, p)
                    && !matches!(o, Obj::Tag(_))
                {
                    out.runs.push(stub(slab, sheet, *p, size, top_at(*o), made, &basis));
                }
            }
        }
        match made {
            _ if note.is_some() => out.called_runs += 1,
            Made::Drawn => out.drawn_runs += 1,
            _ => out.inferred_runs += 1,
        }
    };
    let mut home_drawn: BTreeSet<usize> = BTreeSet::new();
    for (li, l) in lines.iter().enumerate() {
        let note = note_of(li);
        let (a, z) = l.ends;
        // A line from a tag (a home run's arrow) to its luminaire is the tag's, not a run.
        if matches!((a, z), (Some(Obj::Tag(_)), _) | (_, Some(Obj::Tag(_)))) {
            continue;
        }
        let both = a.is_some() && z.is_some() && a != z;
        if !both && note.is_none() {
            continue;
        }
        let (Some(first), Some(last)) = (l.pts.first().copied(), l.pts.last().copied()) else { continue };
        let straight = dist(first, last);
        if both && path_len(&l.pts) > 3.0 * straight + 2.0 * reach {
            continue;
        }
        if let Some(f) = a.and_then(feeder_from).or_else(|| z.and_then(feeder_from)) {
            out.findings.push(format!(
                "{sheet}: the line drawn from site gear is its feeder {} ({}->{}): built by the feeder routes from where the site plan draws the gear",
                f.tag, f.from, f.to
            ));
            continue;
        }
        let mut pts: Vec<[f64; 2]> = l.pts.iter().map(|p| to_model(*p)).collect();
        if let (Some(o), Some(p)) = (a, pts.first_mut()) {
            *p = model_end(o, first);
        }
        if let (Some(o), Some(p)) = (z, pts.last_mut()) {
            *p = model_end(o, last);
        }
        let branch = matches!(a, Some(Obj::Light(_))) || matches!(z, Some(Obj::Light(_)));
        if matches!((a, z), (Some(Obj::Light(_)), Some(Obj::Building)) | (Some(Obj::Building), Some(Obj::Light(_))))
            && let Some(Obj::Light(i)) = [a, z].into_iter().flatten().find(|o| matches!(o, Obj::Light(_)))
        {
            home_drawn.insert(i);
        }
        let why = format!("drawn on {placed_by}; {}", r.line_source);
        add_run(&mut out, pts, [a, z], Made::Drawn, note, &why, branch);
    }
    // A circuit's luminaires no line joins: the nearest of its others, straight (inferred).
    for (a, b) in &inferred_links {
        let (Some(la), Some(lb)) = (read.lights.get(*a), read.lights.get(*b)) else { continue };
        let why =
            format!("inferred: {sheet} shows no line between these luminaires of one circuit; trenched straight from the nearest (trade practice)");
        add_run(&mut out, vec![to_model(la.at), to_model(lb.at)], [Some(Obj::Light(*a)), Some(Obj::Light(*b))], Made::Inferred, None, &why, true);
    }
    // Each circuit's home run: from the circuit's object nearest the building to the building,
    // then under the slab to its panel when the model has it (inferred).
    let mut homes: BTreeMap<String, Vec<(Obj, [f64; 2])>> = BTreeMap::new();
    for (i, c) in light_circuit.iter().enumerate() {
        if let (Some(label), Some(l)) = (c, read.lights.get(i))
            && !home_drawn.contains(&i)
        {
            homes.entry(label.clone()).or_default().push((Obj::Light(i), to_model(l.at)));
        }
    }
    let drawn_home: BTreeSet<String> =
        light_circuit.iter().enumerate().filter(|(i, _)| home_drawn.contains(i)).filter_map(|(_, c)| c.clone()).collect();
    for (qi, found) in &equip_circuits {
        if let Some(Some((c, ..))) = equip_at.get(*qi) {
            homes.entry(found.join(",")).or_default().push((Obj::Equip(*qi), *c));
        }
    }
    for (label, members) in homes.iter().filter(|(k, _)| !drawn_home.contains(*k)).take(MAX_ITEMS) {
        let panel_name = label.split_once('-').map_or(label.as_str(), |(p, _)| p).trim();
        let panel = sp.mep.equipment.iter().find(|e| e.tag.eq_ignore_ascii_case(panel_name) && e.level.eq_ignore_ascii_case(slab));
        let pc = panel.map(|p| [(p.min[0] + p.max[0]) / 2.0, (p.min[1] + p.max[1]) / 2.0]);
        let start = members.iter().min_by(|a, b| {
            let d = |p: [f64; 2]| nearest_on(&building, p).map_or(f64::INFINITY, |q| dist(p, q));
            d(a.1).total_cmp(&d(b.1))
        });
        let Some((obj, from)) = start.copied() else { continue };
        let entry = match pc {
            Some(p) => nearest_on(&building, p),
            None => nearest_on(&building, from),
        }
        .unwrap_or(from);
        let size = r.branch_conduit_in;
        let out_bottom = -(r.cover_in + size) / 12.0;
        let under = -super::ugplan::rules().depths.conduit_under_slab_ft;
        let mut pts = vec![from, entry];
        let mut bottoms = vec![out_bottom];
        if let Some(p) = pc {
            pts.push([p[0], entry[1]]);
            pts.push(p);
            bottoms.push(under);
            bottoms.push(under);
            pts.dedup_by(|a, b| dist(*a, *b) < 1e-6);
            bottoms.truncate(pts.len().saturating_sub(1).max(1));
        }
        let why = match panel {
            Some(p) => format!(
                "inferred: home run of {label} to panel {} (its site plan's circuit; routed by trade practice: straight to the nearest wall, under the slab to the panel)",
                p.tag
            ),
            None => {
                out.findings.push(format!(
                    "{sheet}: circuit {label}'s panel {panel_name} is not in the model: its home run ends at the building wall (continue it to the panel)"
                ));
                format!("inferred: home run of {label} to the building wall nearest it; panel {panel_name} is not in the model")
            }
        };
        let mut run = buried(slab, sheet, pts.clone(), size, Made::Inferred, why.clone(), &format!("rule: {}", r.branch_conduit_source));
        if bottoms.len() == run.points.len().saturating_sub(1) {
            run.bottoms_ft = bottoms;
        }
        out.runs.push(run);
        out.runs.push(stub(slab, sheet, from, size, top_at(obj), Made::Inferred, &why));
        if let Some(last) = pts.last() {
            out.runs.push(stub(slab, sheet, *last, size, r.stub_top_ft, Made::Inferred, &why));
        }
        out.inferred_runs += 1;
    }
    // (6) The conduit notes no line carries: a run between what they name (inferred).
    for (n, c) in read.notes.iter().zip(&called) {
        if c.is_some() {
            continue;
        }
        let up = n.callout.text.to_ascii_uppercase();
        let named: Vec<usize> = read
            .equipment
            .iter()
            .enumerate()
            .filter(|(_, q)| has_any(&up, &r.equipment.iter().filter(|k| k.kind == q.kind).flat_map(|k| k.words.clone()).collect::<Vec<_>>()))
            .map(|(i, _)| i)
            .collect();
        let xfmr = read.equipment.iter().position(|q| q.kind == "transformer");
        let target = if n.callout.what == "service_primary" { xfmr.or(named.first().copied()) } else { named.first().copied() };
        let Some(qi) = target else {
            out.findings.push(format!(
                "{sheet}: the note \"{}\" calls conduits but no line or named object on the site plan locates them (left to the underground plan)",
                n.callout.text.chars().take(80).collect::<String>()
            ));
            continue;
        };
        let Some((qc, ..)) = equip_at.get(qi).cloned().flatten() else { continue };
        let bc = if building.len() >= 3 { centre(&bbox_of(&building)) } else { qc };
        let pts = if n.callout.what == "service_primary" {
            let (dx, dy) = (qc[0] - bc[0], qc[1] - bc[1]);
            let l = dx.hypot(dy).max(1e-9);
            vec![[qc[0] + dx / l * r.primary_out_ft * upf, qc[1] + dy / l * r.primary_out_ft * upf], qc]
        } else {
            vec![qc, nearest_on(&building, qc).unwrap_or(bc)]
        };
        let why = if n.callout.what == "service_primary" {
            format!("inferred: {}", r.primary_source)
        } else {
            "inferred: the note names it and draws no line: trenched straight to the nearest building wall".to_string()
        };
        add_run(&mut out, pts, [None, Some(Obj::Equip(qi))], Made::Inferred, Some(n), &why, false);
    }
    out
}

/// The trench site work's buried runs need, feet by level: one trench for the runs on one path
/// (a note's parallel conduits share it), its stub-ups none.
pub(crate) fn trench_ft(sp: &BuildingSpec) -> BTreeMap<String, (f64, Vec<String>)> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let tol = 2.0 * upf;
    let mut seen: Vec<(&str, &[[f64; 2]])> = Vec::new();
    let mut out: BTreeMap<String, (f64, Vec<String>)> = BTreeMap::new();
    for (i, r) in sp.mep.runs.iter().enumerate().take(20_000) {
        if !r.source_sheet.starts_with(SOURCE_PREFIX) || r.points.len() < 2 || r.top_ft.is_some() {
            continue;
        }
        if seen.iter().any(|(l, p)| *l == r.level.as_str() && same_trench(p, &r.points, tol)) {
            continue;
        }
        seen.push((r.level.as_str(), &r.points));
        let e = out.entry(r.level.clone()).or_default();
        e.0 += path_len(&r.points) / upf;
        if e.1.len() < 200 {
            e.1.push(format!("run:{i}"));
        }
    }
    out
}

/// What a circuit tag's circuit feeds as its panel schedule prints it (`LP-7` → `SITE LIGHTING`)
/// and the schedule's sheet; none when no schedule lists it.
fn circuit_load(sp: &BuildingSpec, label: &str) -> Option<(String, String)> {
    let (panel, rest) = label.split_once('-')?;
    let number = rest.split(',').next()?.trim();
    sp.mep
        .panel_schedules
        .iter()
        .filter(|p| p.panel.trim().eq_ignore_ascii_case(panel.trim()))
        .find_map(|p| p.circuits.iter().find(|c| c.number.trim() == number).map(|c| (c.description.trim().to_string(), c.source.clone())))
}

/// A note this step built from (its runs' basis quotes it): the underground plan leaves it.
pub(crate) fn built_callout(sp: &BuildingSpec, c: &Callout) -> bool {
    let head: String = c.text.chars().take(160).collect();
    !head.is_empty() && sp.mep.runs.iter().any(|r| r.source_sheet.starts_with(SOURCE_PREFIX) && r.basis.contains(&head))
}

/// Everything an earlier pass of this step put in the spec taken out (a rerun builds it again).
/// The site gear it moved stays (the gear step places it again). How many went.
pub(crate) fn forget(sp: &mut BuildingSpec) -> usize {
    let before = sp.tags.len() + sp.mep.equipment.len() + sp.concrete.elements.len() + sp.concrete.pads.len() + sp.mep.runs.len();
    let mine: BTreeSet<String> = sp.mep.equipment.iter().filter(|e| e.sources.contains_key(MARK)).map(|e| e.tag.to_ascii_uppercase()).collect();
    sp.tags.retain(|t| !t.fields.contains_key(MARK));
    sp.mep.equipment.retain(|e| !e.sources.contains_key(MARK));
    sp.concrete.elements.retain(|e| !e.id.starts_with(CONCRETE_PREFIX));
    sp.concrete.pads.retain(|p| !mine.contains(&p.equipment.trim().to_ascii_uppercase()));
    sp.mep.runs.retain(|r| !r.source_sheet.starts_with(SOURCE_PREFIX));
    before - (sp.tags.len() + sp.mep.equipment.len() + sp.concrete.elements.len() + sp.concrete.pads.len() + sp.mep.runs.len())
}

/// Whether a panel schedule circuit's load is on the site (its description's words).
pub(crate) fn is_site_load(description: &str) -> bool {
    has_any(&description.to_ascii_uppercase(), &rules().site_load_words)
}

/// A panel schedule circuit whose load is on the site.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SiteLoad {
    pub panel: String,
    pub number: String,
    pub description: String,
    pub source: String,
}

/// The panel schedules' circuits whose load is on the site (their descriptions' words) and that
/// no site work carries (`built`: the circuit tags the site plans built, `LP-7`).
pub(crate) fn site_loads(sp: &BuildingSpec, built: &BTreeSet<String>) -> Vec<SiteLoad> {
    let mut out = Vec::new();
    for ps in sp.mep.panel_schedules.iter().take(2_000) {
        for c in ps.circuits.iter().take(500) {
            if !is_site_load(&c.description) {
                continue;
            }
            let label = format!("{}-{}", ps.panel.trim(), c.number.trim()).to_ascii_uppercase();
            let carried = built.iter().any(|b| {
                let (bp, bc) = b.split_once('-').unwrap_or((b.as_str(), ""));
                bp.eq_ignore_ascii_case(ps.panel.trim()) && bc.split(',').any(|n| n.trim() == c.number.trim())
            });
            if carried || built.contains(&label) {
                continue;
            }
            out.push(SiteLoad { panel: ps.panel.clone(), number: c.number.clone(), description: c.description.clone(), source: c.source.clone() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, HAlign, Line, LwPolyline, Space, Text, VAlign};
    use cadaddy_geom::{PolyVertex, Vec2, Vec3};

    fn add(d: &mut Drawing, layer: &str, kind: EntityKind) -> Handle {
        d.add(&Space::Model, Common { layer: layer.into(), ..Common::default() }, kind).unwrap_or(Handle(0))
    }
    fn text(d: &mut Drawing, s: &str, x: f64, y: f64, h: f64) {
        let t = Text {
            insert: Vec3::new(x, y, 0.0),
            align_pt: None,
            height: h,
            value: s.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        };
        add(d, "E-TEXT", EntityKind::Text(t));
    }
    fn pline(d: &mut Drawing, layer: &str, pts: &[[f64; 2]], closed: bool) -> Handle {
        let vertices = pts.iter().map(|p| PolyVertex::new(Vec2::new(p[0], p[1]))).collect();
        add(d, layer, EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: false }))
    }
    fn line(d: &mut Drawing, layer: &str, a: [f64; 2], b: [f64; 2]) {
        add(d, layer, EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }));
    }
    /// A dashed line from `a` to `b`: dashes `dash` long, `gap` apart.
    fn dashed(d: &mut Drawing, layer: &str, a: [f64; 2], b: [f64; 2], dash: f64, gap: f64) {
        let l = dist(a, b);
        let u = [(b[0] - a[0]) / l, (b[1] - a[1]) / l];
        let mut s = 0.0;
        while s < l {
            let e = (s + dash).min(l);
            line(d, layer, [a[0] + u[0] * s, a[1] + u[1] * s], [a[0] + u[0] * e, a[1] + u[1] * e]);
            s = e + gap;
        }
    }
    fn square(c: [f64; 2], half: f64) -> Vec<[f64; 2]> {
        vec![[c[0] - half, c[1] - half], [c[0] + half, c[1] - half], [c[0] + half, c[1] + half], [c[0] - half, c[1] + half]]
    }

    #[test]
    fn a_site_plan_is_a_trades_by_its_discipline_or_title() {
        assert!(trade_site_sheet("E", "SITE PLAN"));
        assert!(trade_site_sheet("MEP", "SITE PLAN - MEP"));
        assert!(trade_site_sheet("C", "SITE ELECTRICAL PLAN"));
        assert!(trade_site_sheet("A", "M/E/P SITE PLAN"));
        assert!(!trade_site_sheet("C", "SITE PLAN"));
        assert!(!trade_site_sheet("L", "PLANTING PLAN"));
        assert!(!trade_site_sheet("A", "ARCHITECTURAL SITE PLAN"));
    }

    #[test]
    fn a_site_sheets_trade_content_is_its_tags_and_power_notes() {
        let t = |v: &str| TextItem { value: v.into(), at: Vec2::ZERO, height: 1.0, rotation: 0.0 };
        let types: BTreeSet<String> = ["SL1".to_string()].into_iter().collect();
        assert!(trade_content(&[t("PARKING"), t("SL1")], &types).is_some());
        assert!(trade_content(&[t("LP-3\u{2260}")], &types).is_some_and(|w| w.contains("LP-3")));
        assert!(trade_content(&[t("PROVIDE (2) 4\" CONDUITS TO PAD")], &types).is_some());
        assert!(trade_content(&[t("6\" CURB"), t("ASPHALT PAVING"), t("EX. 8\" WATER")], &types).is_none());
    }

    #[test]
    fn a_pole_height_is_read_from_the_fixture_words() {
        assert_eq!(pole_height_ft("LED AREA LIGHT MOUNTED ON 14' POLE WITH TYPE 3"), Some(14.0));
        assert_eq!(pole_height_ft("AREA LIGHT, 25 FT POLE, BLACK"), Some(25.0));
        assert_eq!(pole_height_ft("SQUARE POLE HEIGHT 30'-0\""), Some(30.0));
        assert_eq!(pole_height_ft("WALL PACK"), None);
        assert_eq!(base_for(12.0), [15.0, 18.0, 48.0, 24.0]);
        assert_eq!(base_for(60.0)[1], 30.0);
    }

    #[test]
    fn circuit_tags_lose_the_home_run_arrow() {
        assert_eq!(circuit_label("SP-5\u{2260}"), Some(("SP-5".into(), "SP".into(), "5".into())));
        assert_eq!(circuit_label("L-1,3"), Some(("L-1,3".into(), "L".into(), "1,3".into())));
        assert!(circuit_label("C-POLE").is_none());
    }

    #[test]
    fn dashes_of_one_line_join_and_a_crossing_line_does_not() {
        let mut d = Drawing::default();
        dashed(&mut d, "E-CIRC", [0.0, 0.0], [100.0, 0.0], 6.0, 3.0);
        // A dashed line crossing it square: its dashes are its own.
        dashed(&mut d, "E-CIRC", [50.0, -40.0], [50.0, -4.0], 6.0, 3.0);
        let mut segs = Vec::new();
        for e in d.model.iter() {
            if let EntityKind::Line(l) = &e.kind {
                segs.push(Seg { a: [l.a.x, l.a.y], b: [l.b.x, l.b.y], layer: 0 });
            }
        }
        let n = net(&segs, 2.0);
        let first = n.comp.first().copied();
        let along = segs.iter().zip(&n.comp).filter(|(s, _)| s.a[1].abs() < 1e-9 && s.b[1].abs() < 1e-9).all(|(_, c)| Some(*c) == first);
        assert!(along, "{:?}", n.comp);
        let across: BTreeSet<usize> = segs.iter().zip(&n.comp).filter(|(s, _)| (s.a[0] - 50.0).abs() < 1e-9).map(|(_, c)| *c).collect();
        assert_eq!(across.len(), 1);
        assert!(!across.contains(&first.unwrap_or(usize::MAX)));
        let members: Vec<usize> = n.comp.iter().enumerate().filter(|(_, c)| Some(**c) == first).map(|(i, _)| i).collect();
        let pts = line_path(&segs, &members, &n.joins);
        let len = path_len(&pts);
        assert!((len - 100.0).abs() < 1e-6, "{len} {pts:?}");

        // A line broken for its label runs on straight across the break; one that turns there
        // is another line.
        let seg = |a: [f64; 2], b: [f64; 2]| Seg { a, b, layer: 0 };
        let broken = [seg([0.0, 0.0], [20.0, 0.0]), seg([35.0, 0.0], [60.0, 0.0]), seg([75.0, 5.5], [95.0, 13.0])];
        let n = net(&broken, 2.0);
        assert_eq!(n.comp.first(), n.comp.get(1), "joined across the label's break");
        assert_ne!(n.comp.get(1), n.comp.get(2), "a turn across a long gap is no run-on");
    }

    /// A trade's site plan at 1" = 20' (drawing units inches): a building outline turned 20
    /// degrees, two pole lights a dashed circuit joins, the circuit's tag by one, a utility pad
    /// transformer with a dashed secondary to the building and a primary its note calls.
    fn site_sheet() -> (Drawing, [f64; 4]) {
        let mut d = Drawing::default();
        let h = 18.0;
        let (c, s) = (20f64.to_radians().cos(), 20f64.to_radians().sin());
        let turn = |p: [f64; 2]| [10_000.0 + c * p[0] - s * p[1], 2_000.0 + s * p[0] + c * p[1]];
        let outline: Vec<[f64; 2]> = [[0.0, 0.0], [960.0, 0.0], [960.0, 600.0], [0.0, 600.0]].iter().map(|p| turn(*p)).collect();
        pline(&mut d, "-BASEC-BUILD", &outline, true);
        // Parking lot linework (a civil layer): long curbs.
        line(&mut d, "-BASEC-CURB", [9_000.0, 3_000.0], [13_000.0, 3_000.0]);
        // Two pole lights north of the building, their dashed circuit, the home run's tag.
        let l1 = [10_200.0, 3_400.0];
        let l2 = [11_100.0, 3_400.0];
        for (p, mark_at) in [(l1, [l1[0] + 40.0, l1[1] - 10.0]), (l2, [l2[0] + 40.0, l2[1] - 10.0])] {
            pline(&mut d, "E-LGHT", &square(p, 25.0), true);
            text(&mut d, "SL1", mark_at[0], mark_at[1], h);
        }
        dashed(&mut d, "E-LGHT-CIRC", [l1[0] + 30.0, l1[1]], [l2[0] - 30.0, l2[1]], 30.0, 15.0);
        text(&mut d, "LP-7\u{2260}", l1[0] - 20.0, l1[1] + 40.0, h);
        // The utility's transformer east of the building, its label beside it.
        let x = [11_400.0, 2_200.0];
        pline(&mut d, "E-POWR", &square(x, 50.0), true);
        text(&mut d, "UTILITY COMPANY PAD", x[0] + 80.0, x[1] + 10.0, h);
        text(&mut d, "MOUNTED TRANSFORMER", x[0] + 80.0, x[1] - 20.0, h);
        // The secondary: dashed from the transformer to the building's east wall.
        let east = turn([960.0, 300.0]);
        dashed(&mut d, "E-CNDT", [x[0] - 50.0, x[1]], east, 30.0, 15.0);
        // The primary: dashed north from the transformer, its note's leader on it.
        dashed(&mut d, "E-CNDT", [x[0], x[1] + 50.0], [x[0], x[1] + 600.0], 30.0, 15.0);
        text(&mut d, "PRIMARY FEEDER. PROVIDE (2)4\" CONDUITS WITH", x[0] + 200.0, x[1] + 300.0, h);
        text(&mut d, "PULL-STRINGS. COORDINATE WITH UTILITY COMPANY.", x[0] + 200.0, x[1] + 270.0, h);
        pline(&mut d, "E-TEXT", &[[x[0] + 195.0, x[1] + 305.0], [x[0] + 5.0, x[1] + 305.0]], false);
        (d, [9_000.0, 1_000.0, 14_000.0, 5_000.0])
    }

    fn spec_with_slab() -> BuildingSpec {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        let level: cadaddy_walk3d::Level = serde_json::from_value(
            serde_json::json!({"name": "Level 1", "elevation_ft": 0.0, "height_ft": 12.0, "region": [0.0, 0.0, 2_000.0, 1_500.0]}),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        sp.levels.push(level);
        // The building's walls: 80 x 50 ft, square to the model.
        sp.shells.insert(
            "Level 1".into(),
            vec![[100.0, 100.0, 1_060.0, 106.0], [100.0, 694.0, 1_060.0, 700.0], [100.0, 100.0, 106.0, 700.0], [1_054.0, 100.0, 1_060.0, 700.0]],
        );
        sp.mep.fixture_schedule.push(cadaddy_walk3d::FixtureType {
            tag: "SL1".into(),
            description: "LED AREA LIGHT MOUNTED ON 12' POLE".into(),
            source: "E-501 LIGHT FIXTURE SCHEDULE".into(),
            ..cadaddy_walk3d::FixtureType::default()
        });
        sp
    }

    #[test]
    fn a_trade_site_plan_is_read_laid_on_the_building_and_built() {
        let (d, area) = site_sheet();
        let types: BTreeSet<String> = ["SL1".to_string()].into_iter().collect();
        let read = read("E-001", &d, area, &types, 12.0);
        assert_eq!(read.lights.len(), 2, "{:?}", read.lights);
        assert_eq!(read.tags.len(), 1);
        assert_eq!(read.equipment.len(), 1, "{:?} {:?}", read.equipment, read.unplaced_labels);
        assert!(read.equipment.first().is_some_and(|q| q.kind == "transformer" && q.utility));
        assert_eq!(read.notes.len(), 1, "{:?}", read.notes);
        assert!(read.notes.first().is_some_and(|n| n.callout.count == 2 && n.callout.size_in == Some(4.0)), "{:?}", read.notes);
        assert!(read.outlines.first().is_some_and(|(_, named)| *named));

        let sp = spec_with_slab();
        let fit = register(&read, &sp, "Level 1", true);
        assert_eq!(fit.method, "building outline", "{}", fit.why);
        // Turned back 20 degrees, the outline lies on the walls.
        let turned = fit.reg.b.atan2(fit.reg.a).to_degrees();
        assert!((turned + 20.0).abs() < 0.5, "{turned}");
        let corner = fit.to_model([10_000.0, 2_000.0]);
        assert!(dist(corner, [100.0, 100.0]) < 6.0, "{corner:?}");

        let mut seq = Seq::default();
        let built = plan(&sp, &read, &fit, "Level 1", &mut seq);
        // Two luminaires on 12 ft poles on their bases.
        let lights: Vec<&ElementTag> = built.tags.iter().filter(|t| t.fields.get("device_kind").is_some_and(|f| f.value == "luminaire")).collect();
        assert_eq!(lights.len(), 2);
        assert!(lights.iter().all(|t| t.fields.get("circuit").is_some_and(|f| f.value == "LP-7")), "{lights:?}");
        assert!(lights.iter().all(|t| t.fields.get("mount_height_in").is_some_and(|f| f.value == "168")), "{lights:?}");
        assert_eq!(built.equipment.iter().filter(|e| e.kind == "pole").count(), 2);
        assert!(
            built.equipment.iter().filter(|e| e.kind == "pole").all(|e| (e.height_ft - 12.0).abs() < 1e-9 && super::super::utilityxfmr::is_site(e))
        );
        assert_eq!(built.concrete.iter().filter(|c| c.kind == "pole_base").count(), 2);
        // The transformer, where the site plan draws it, on its pad.
        let x = built.equipment.iter().find(|e| e.kind == "transformer");
        assert!(x.is_some_and(|e| e.properties.get("PROVENANCE").is_some_and(|p| p == "drawn")), "{x:?}");
        assert!(built.pads.iter().any(|p| x.is_some_and(|e| e.tag == p.equipment)));
        // The runs: the circuit between the poles (drawn), its home run (inferred), the secondary
        // (drawn) and the primary its note calls: two 4" conduits.
        let ug: Vec<&MepRun> = built.runs.iter().filter(|r| r.points.len() >= 2).collect();
        assert!(ug.iter().all(|r| r.system == super::super::ugplan::E_UG && r.bottoms_ft.iter().all(|b| *b < 0.0)));
        let between = ug.iter().filter(|r| r.made == Made::Drawn && (r.size_in[0] - 1.0).abs() < 1e-9).count();
        assert!(between >= 1, "{ug:?}");
        assert!(ug.iter().any(|r| r.made == Made::Inferred && r.basis.contains("home run of LP-7")), "{ug:?}");
        let primary: Vec<&&MepRun> = ug.iter().filter(|r| (r.size_in[0] - 4.0).abs() < 1e-9).collect();
        assert_eq!(primary.len(), 2, "{ug:?}");
        assert!(primary.iter().all(|r| r.made == Made::Drawn && r.basis.contains("PROVIDE (2)4")), "{primary:?}");
        assert!(built.runs.iter().any(|r| r.top_ft.is_some()), "stub-ups");
        // Everything says where it came from, and forgetting takes it all out again.
        let mut sp2 = sp.clone();
        sp2.tags.extend(built.tags.clone());
        sp2.mep.equipment.extend(built.equipment.clone());
        sp2.concrete.elements.extend(built.concrete.clone());
        sp2.concrete.pads.extend(built.pads.clone());
        sp2.mep.runs.extend(built.runs.clone());
        // One trench for the note's two conduits.
        let conduit_ft: f64 = built.runs.iter().filter(|r| r.top_ft.is_none()).map(|r| path_len(&r.points) / 12.0).sum();
        let primary_ft = primary.first().map_or(0.0, |r| path_len(&r.points) / 12.0);
        let trench: f64 = trench_ft(&sp2).values().map(|(f, _)| *f).sum();
        assert!(primary_ft > 0.0 && (trench - (conduit_ft - primary_ft)).abs() < 0.1, "{trench} {conduit_ft} {primary_ft}");
        // Side by side, 6 in apart centre to centre (4 in conduits, 2 in between).
        let gap_in = match (primary.first(), primary.get(1)) {
            (Some(a), Some(b)) => a.points.first().zip(b.points.first()).map(|(p, q)| dist(*p, *q)),
            _ => None,
        };
        assert!(gap_in.is_some_and(|g| (g - 6.0).abs() < 1e-6), "{gap_in:?}");
        assert!(forget(&mut sp2) > 0);
        assert_eq!(sp2, sp);
    }

    #[test]
    fn a_site_plan_whose_building_is_another_size_is_placed_by_its_centre_and_said_so() {
        let (d, area) = site_sheet();
        let types: BTreeSet<String> = ["SL1".to_string()].into_iter().collect();
        let read = read("E-001", &d, area, &types, 12.0);
        let mut sp = spec_with_slab();
        sp.shells.insert("Level 1".into(), vec![[100.0, 100.0, 110.0, 106.0], [100.0, 100.0, 106.0, 110.0]]);
        let fit = register(&read, &sp, "Level 1", true);
        assert_eq!(fit.method, "outline centre", "{}", fit.why);
        assert!(fit.why.contains("printed scale"));
    }

    #[test]
    fn the_job_builds_the_trade_site_plan_and_skips_the_civil_one() {
        use super::super::guided::{GuidedJob, JobGoal, SheetRegion, site_work_step};
        let mut s = crate::Session::new();
        let (site, area) = site_sheet();
        // A civil site plan beside it: curbs, paving and a water line, no trade content.
        let mut civil = Drawing::default();
        line(&mut civil, "C-CURB", [20_000.0, 2_000.0], [24_000.0, 2_000.0]);
        text(&mut civil, "ASPHALT PAVING", 21_000.0, 2_500.0, 18.0);
        text(&mut civil, "EX. 8\" WATER", 22_000.0, 3_000.0, 18.0);
        let mut sp = spec_with_slab();
        sp.mep.panel_schedules.push(cadaddy_walk3d::PanelSchedule {
            panel: "LP".into(),
            source: "E-601".into(),
            circuits: vec![
                cadaddy_walk3d::PanelCircuit {
                    number: "7".into(),
                    description: "SITE LIGHTING".into(),
                    source: "E-601".into(),
                    ..Default::default()
                },
                cadaddy_walk3d::PanelCircuit {
                    number: "9".into(),
                    description: "MONUMENT SIGN".into(),
                    source: "E-601".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        if let Ok(d) = s.doc_mut() {
            for e in site.model.iter().chain(civil.model.iter()) {
                let _ = d.add(&Space::Model, e.common.clone(), e.kind.clone());
            }
            d.building_spec = Some(sp.to_json()).into();
        }
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        let region = |number: &str, discipline: &str, rect: [f64; 4], why: &str| SheetRegion {
            number: number.into(),
            level: SITE_LEVEL.into(),
            discipline: discipline.into(),
            kind: "plan".into(),
            rect,
            area: Some(rect),
            title_block: None,
            not_built: why.into(),
            levels: Vec::new(),
            view: None,
        };
        // As a checkpoint made before site work marks them: the electrical site plan still
        // `site/civil`, its discipline making it a trade's.
        job.sheet_regions =
            vec![region("E-001", "E", area, "site/civil"), region("C-101", "C", [19_000.0, 1_000.0, 25_000.0, 5_000.0], "site/civil")];
        let mut counts = Vec::new();
        for _ in 0..2 {
            let mut result = serde_json::json!({});
            site_work_step(&mut s, &mut job, &mut result).unwrap_or_else(|e| panic!("{e}"));
            let w = &result["site_work"];
            assert_eq!(w["luminaires"], serde_json::json!(2), "{w:#}");
            assert_eq!(w["poles"], serde_json::json!(2), "{w:#}");
            assert_eq!(w["equipment"], serde_json::json!(1), "{w:#}");
            assert_eq!(w["circuits"]["LP-7"], serde_json::json!(2), "{w:#}");
            // The sign the schedule names has no place on any site plan: asked about.
            assert!(
                w["site_loads_asked"].as_array().is_some_and(|a| a.len() == 1 && a.iter().any(|x| x.as_str().is_some_and(|t| t.contains("LP-9")))),
                "{w:#}"
            );
            let (sp, _) = super::super::bspec::current(&s, "test").unwrap_or_else(|e| panic!("{e}"));
            let d = s.doc().unwrap_or_else(|e| panic!("{e}"));
            let marked = d.model.iter().filter(|e| cadaddy_doc::props::read(&e.common.xdata).get(MARK).is_some()).count();
            counts.push((
                sp.tags.iter().filter(|t| t.fields.contains_key(MARK)).count(),
                sp.mep.equipment.iter().filter(|e| e.sources.contains_key(MARK)).count(),
                sp.concrete.elements.iter().filter(|e| e.id.starts_with(CONCRETE_PREFIX)).count(),
                sp.mep.runs.iter().filter(|r| r.source_sheet.starts_with(SOURCE_PREFIX)).count(),
                sp.rfis.len(),
                marked,
            ));
            assert!(marked > 0, "the built site work is marked");
            // The civil sheet stays out.
            assert_eq!(job.sheet_regions.get(1).map(|r| r.not_built.as_str()), Some("site/civil"));
            assert_eq!(job.sheet_regions.first().map(|r| r.not_built.as_str()), Some(SITE_WORK));
        }
        // A rerun builds the same again, never twice.
        assert_eq!(counts.first(), counts.get(1), "{counts:?}");
        // No review item of its own: the breakers' check asks about the sign's row.
        assert!(counts.first().is_some_and(|c| c.0 == 2 && c.1 == 3 && c.2 == 2 && c.3 > 4 && c.4 == 0), "{counts:?}");
    }

    /// A site plan tagging its pole lights by their circuit only (no type mark), a monument sign
    /// its label names, a box in grade its mark names; a transformer label a note runs on from.
    #[test]
    fn symbols_tagged_by_their_circuit_are_what_the_circuit_feeds() {
        let mut d = Drawing::default();
        let h = 18.0;
        pline(&mut d, "-BASEC-BUILD", &[[10_000.0, 2_000.0], [10_960.0, 2_000.0], [10_960.0, 2_600.0], [10_000.0, 2_600.0]], true);
        for (x, c) in [(10_200.0, "SP-5"), (10_700.0, "SP-5"), (11_200.0, "SP-9")] {
            pline(&mut d, "E-SITE", &square([x, 3_400.0], 25.0), true);
            text(&mut d, c, x + 35.0, 3_390.0, h);
        }
        pline(&mut d, "E-SITE", &square([11_600.0, 3_000.0], 40.0), true);
        text(&mut d, "MONUMENT", 11_660.0, 3_010.0, h);
        text(&mut d, "SIGN", 11_660.0, 2_980.0, h);
        // The sign's circuit printed by it: the sign's, no device of its own.
        text(&mut d, "SP-11", 11_520.0, 3_060.0, h);
        pline(&mut d, "E-SITE", &square([10_500.0, 3_200.0], 12.0), true);
        text(&mut d, "J", 10_520.0, 3_195.0, h);
        pline(&mut d, "E-POWR", &square([11_400.0, 2_200.0], 50.0), true);
        text(&mut d, "UTILITY CO. PAD MOUNTED", 11_480.0, 2_210.0, h);
        text(&mut d, "TRANSFORMER.", 11_480.0, 2_180.0, h);
        text(&mut d, "COORDINATE WITH UTILITY", 11_480.0, 2_150.0, h);
        text(&mut d, "FOR ALL REQUIREMENTS.", 11_480.0, 2_120.0, h);
        let read = read("E-001", &d, [9_000.0, 1_000.0, 14_000.0, 5_000.0], &BTreeSet::new(), 12.0);
        assert_eq!(read.lights.iter().filter(|l| l.circuit.is_some()).count(), 3, "{:?}", read.lights);
        assert!(read.equipment.iter().any(|q| q.kind == "sign" && q.circuits == ["SP-11"]), "{:?}", read.equipment);
        let kinds: Vec<&str> = read.equipment.iter().map(|q| q.kind.as_str()).collect();
        assert!(kinds.contains(&"sign") && kinds.contains(&"pull_box") && kinds.contains(&"transformer"), "{kinds:?} {:?}", read.unplaced_labels);

        let mut sp = spec_with_slab();
        let circuit =
            |n: &str, d: &str| cadaddy_walk3d::PanelCircuit { number: n.into(), description: d.into(), source: "E-601".into(), ..Default::default() };
        sp.mep.panel_schedules.push(cadaddy_walk3d::PanelSchedule {
            panel: "SP".into(),
            circuits: vec![circuit("5", "SITE LIGHTING"), circuit("9", "EV CHARGER"), circuit("11", "MONUMENT SIGN")],
            ..Default::default()
        });
        let fit = register(&read, &sp, "Level 1", true);
        let built = plan(&sp, &read, &fit, "Level 1", &mut Seq::default());
        // SP-5's two symbols: the schedule's one pole type on poles, on their circuit.
        let lights: Vec<&ElementTag> = built.tags.iter().filter(|t| t.fields.get("device_kind").is_some_and(|f| f.value == "luminaire")).collect();
        assert_eq!(lights.len(), 2, "{lights:?}");
        assert!(
            lights
                .iter()
                .all(|t| t.fields.get("type").is_some_and(|f| f.value == "SL1") && t.fields.get("circuit").is_some_and(|f| f.value == "SP-5"))
        );
        assert!(lights.iter().all(|t| t.fields.get("type_basis").is_some_and(|f| f.value.starts_with("derived"))));
        assert_eq!(built.equipment.iter().filter(|e| e.kind == "pole").count(), 2);
        // SP-9 feeds a charger: equipment, not a light; the sign takes the sign's circuit.
        assert!(
            built.equipment.iter().any(|e| e.kind == "ev_charger" && e.properties.get("CIRCUIT").is_some_and(|c| c == "SP-9")),
            "{:?}",
            built.equipment
        );
        assert!(
            built.equipment.iter().any(|e| e.kind == "sign" && e.properties.get("CIRCUIT").is_some_and(|c| c == "SP-11")),
            "{:?}",
            built.equipment
        );
        assert!(built.equipment.iter().any(|e| e.kind == "pull_box" && e.bottom_ft < 0.0));
        assert!(built.equipment.iter().any(|e| e.kind == "transformer"));
        // Each circuit's home run, inferred; none of the three asked about.
        for c in ["SP-5", "SP-9", "SP-11"] {
            assert!(built.runs.iter().any(|r| r.made == Made::Inferred && r.basis.contains(&format!("home run of {c}"))), "{c}: {:?}", built.runs);
        }
        let carried: BTreeSet<String> = built.circuits.keys().cloned().collect();
        assert!(site_loads(&sp, &carried).is_empty(), "{carried:?}");
    }

    #[test]
    fn a_site_load_no_plan_carries_is_asked_where_it_is() {
        let row = |d: &str| super::super::breakers::Mismatch {
            kind: "row_no_plan".into(),
            panel: "LP".into(),
            circuit: "LP-9".into(),
            description: d.into(),
            sheet: "E-601".into(),
            ..Default::default()
        };
        let sign = row("MONUMENT SIGN").title();
        assert!(sign.contains("on the site") && sign.contains("where is it?") && sign.contains("underground"), "{sign}");
        assert!(!row("RECEPTACLES").title().contains("on the site"));
    }

    #[test]
    fn site_loads_no_site_plan_carries_are_listed() {
        let mut sp = BuildingSpec::default();
        let circuit =
            |n: &str, d: &str| cadaddy_walk3d::PanelCircuit { number: n.into(), description: d.into(), source: "E-601".into(), ..Default::default() };
        sp.mep.panel_schedules.push(cadaddy_walk3d::PanelSchedule {
            panel: "LP".into(),
            circuits: vec![
                circuit("7", "SITE LIGHTING"),
                circuit("9", "MONUMENT SIGN"),
                circuit("11", "EXTERIOR LIGHTING"),
                circuit("3", "RECEPTACLES"),
            ],
            ..Default::default()
        });
        let built: BTreeSet<String> = ["LP-7".to_string()].into_iter().collect();
        let left = site_loads(&sp, &built);
        assert_eq!(left.iter().map(|l| l.number.as_str()).collect::<Vec<_>>(), ["9"]);
    }
}
