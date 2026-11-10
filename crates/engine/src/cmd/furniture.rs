//! Casework and furniture as model elements (Q6422).
//!
//! `model.build` (and so the guided job) builds both, level by level:
//! - **Casework** (cabinets, counters, vanities, wardrobes, backsplashes, appliances): read from the
//!   plan and its casework schedule by the casework reader (`casework.read`'s,
//!   [`cadaddy_mepfix::casework_reader`]), each an element at the plan's footprint with its tag,
//!   kind, size, height, source and room.
//! - **Furniture** (beds, nightstands, desks, chairs, sofas, tables, dressers…): from the furniture
//!   the plans draw: a block whose name names a kind, an outline on a furniture layer (its kind by
//!   the label printed in it, else by its size), and an outline anywhere labelled with a kind's name
//!   that is that kind's size. A room of a type (its name) the plans leave unfurnished takes the
//!   furniture of a furnished room of the same type whose outline matches (turned or mirrored as
//!   the outlines say): derived, its basis the room it repeats.
//!
//! Every element's footprint is the documents' (the drawn shape, the schedule's size); its height
//! is the documents' where they give one, else the generic size of its kind (rules data,
//! `furniture.json`), marked generic with its source. Nothing is built where nothing is drawn. The
//! elements are their own trades, cabinets and furniture (the BIM view's switches, the takeoff's
//! lines, the AR export's groups), never the electrical or mechanical work.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, RawTag, Space};
use cadaddy_geom::{Bounds2, Mat3, Vec2, point_in_polygon};
use cadaddy_mepfix::casework_reader::{self, DetectedCasework};
use cadaddy_walk3d::{BuildingSpec, Made, Role, level_at};
use serde::Deserialize;
use serde_json::{Value, json};

const RULES: &str = include_str!("furniture.json");

/// The build parts and their layers.
pub(crate) const CASEWORK_PART: &str = "casework";
pub(crate) const FURNITURE_PART: &str = "furniture";
const CASEWORK_LAYER: &str = "3D-Casework";
const FURNITURE_LAYER: &str = "3D-Furniture";
/// The build tag that marks a size taken from the rules data, not the documents.
pub(crate) const GENERIC_TAG: &str = "generic";

/// Most plan objects looked at, sources kept, elements built, rooms weighed (hostile drawings).
const MAX_SCAN: usize = 4_000_000;
const MAX_SOURCES: usize = 50_000;
const MAX_ELEMENTS: usize = 100_000;
const MAX_ROOMS: usize = 20_000;
/// Most vertices of an outline looked at (a longer one is no piece of furniture).
const MAX_VERTICES: usize = 200;
/// Most label texts a piece of furniture is looked for under.
const MAX_LABELS: usize = 200_000;

// ─── Rules data ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub source: String,
    pub furniture_layers: Vec<String>,
    pub skip_layers: Vec<String>,
    pub label_reach_in: f64,
    pub label_max_words: usize,
    pub size_tolerance: f64,
    pub min_side_in: f64,
    pub max_side_in: f64,
    pub same_place_in: f64,
    pub casework: CaseworkRules,
    pub furniture: FurnitureRules,
    pub repeat: Repeat,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CaseworkRules {
    pub source: String,
    pub kinds: Vec<CaseworkKind>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CaseworkKind {
    pub kind: String,
    /// `floor` (stands on the floor: its height is its top) or `wall` (hung: its height is its
    /// bottom).
    pub mount: String,
    pub top_in: f64,
    pub bottom_in: f64,
    pub tall_in: f64,
    /// A countertop's thickness: a counter over a cabinet is this slab.
    pub slab_in: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FurnitureRules {
    pub source: String,
    pub kinds: Vec<FurnitureKind>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FurnitureKind {
    pub kind: String,
    pub words: Vec<String>,
    /// The long and short sides a drawn one has, inches [min, max].
    pub long_in: [f64; 2],
    pub short_in: [f64; 2],
    /// Its usual footprints (long, short), inches: an unlabelled outline of this size is one.
    pub sizes_in: Vec<[f64; 2]>,
    pub height_in: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Repeat {
    pub on: bool,
    pub match_tolerance_in: f64,
    pub basis: String,
}

pub(crate) fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

impl FurnitureKind {
    fn fits(&self, long: f64, short: f64) -> bool {
        let ok = |v: f64, r: [f64; 2]| r[1] <= 0.0 || (v >= r[0] && v <= r[1]);
        ok(long, self.long_in) && ok(short, self.short_in)
    }
}

/// Words, upper case, letters split from digits (`CHAIR01` → CHAIR, 01), spaced each side.
fn spaced(s: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for w in s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()) {
        let mut cur = String::new();
        let mut digit: Option<bool> = None;
        for c in w.chars() {
            let d = c.is_ascii_digit();
            if digit.is_some_and(|x| x != d) && !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            digit = Some(d);
            cur.push(c.to_ascii_uppercase());
        }
        if !cur.is_empty() {
            words.push(cur);
        }
    }
    format!(" {} ", words.join(" "))
}

/// The furniture kind a name or label says (the first kind, in the rules' order, one of whose
/// words it holds as whole words).
fn kind_named<'a>(r: &'a Rules, text: &str) -> Option<&'a FurnitureKind> {
    let t = spaced(text);
    r.furniture.kinds.iter().find(|k| k.words.iter().any(|w| !w.trim().is_empty() && t.contains(&spaced(w))))
}

fn layer_is(pats: &[String], layer: &str) -> bool {
    pats.iter().any(|p| super::qselect::wildcard(p.trim(), layer))
}

fn title(s: &str) -> String {
    s.split(['_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ─── What the build is given ─────────────────────────────────────────────────

/// A room of the built model: its level, outline (sheet units), label and type (its name).
#[derive(Clone, Debug, Default)]
pub(crate) struct RoomRef {
    pub level: String,
    pub outline: Vec<Vec2>,
    pub label: String,
    pub type_key: String,
}

/// The type a room's name gives it: its words, upper case (`Guest Room` and `GUEST  ROOM` are
/// one type).
pub(crate) fn room_type(name: &str) -> String {
    name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect::<Vec<_>>().join(" ")
}

// ─── An element to build ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Piece {
    part: &'static str,
    kind: String,
    tag: String,
    level: String,
    /// Footprint centre (sheet units), the direction of its long side (radians), long and short
    /// sides (sheet units).
    at: Vec2,
    angle: f64,
    long: f64,
    short: f64,
    /// Bottom and top above the floor, inches.
    bottom_in: f64,
    top_in: f64,
    /// The height came from the rules data (no document gives one).
    generic_height: bool,
    /// Where the height came from (a schedule, a note, the rules), for its property.
    height_source: Source,
    height_note: Option<String>,
    source: Handle,
    /// The document the item was read from (`print`, `schedule`).
    read_from: Source,
    made: Made,
    basis: String,
    description: String,
    spec_section: Option<String>,
    /// How it was found: `schedule`, `note`, `pattern` (casework); `block`, `label`, `layer`,
    /// `size` (furniture); `repeated`.
    found: String,
    room: Option<usize>,
}

// ─── Casework ────────────────────────────────────────────────────────────────

/// The casework reader's reading of the drawing, its coordinates in sheet units. The reader works
/// in inches (a building drawn full size); a plan drawn at another scale is read from a copy of
/// what it looks at scaled to inches.
fn read_casework(d: &Drawing, upf: f64) -> (Vec<DetectedCasework>, f64) {
    let inch = upf / 12.0;
    if (inch - 1.0).abs() < 1e-6 {
        return (casework_reader::read(d).casework, 1.0);
    }
    if !(inch.is_finite() && inch > 0.0) {
        return (Vec::new(), 1.0);
    }
    let mut c = Drawing::new_imperial();
    let m = Mat3::scale(1.0 / inch, 1.0 / inch);
    for e in d.model.iter().take(MAX_SCAN) {
        let l = &e.common.layer;
        let wanted = l.eq_ignore_ascii_case("CASEWORK")
            || l.eq_ignore_ascii_case(casework_reader::SCHEDULE_LAYER)
            || matches!(e.kind, EntityKind::Text(_) | EntityKind::MText(_));
        if !wanted || super::wallgen::made_as(e).is_some() {
            continue;
        }
        let mut k = e.kind.clone();
        k.transform(&m);
        let _ = c.add(&Space::Model, Common { layer: e.common.layer.clone(), ..Common::default() }, k);
    }
    (casework_reader::read(&c).casework, inch)
}

fn casework_kind<'a>(r: &'a Rules, kind: &str) -> Option<&'a CaseworkKind> {
    r.casework.kinds.iter().find(|k| k.kind.eq_ignore_ascii_case(kind)).or_else(|| r.casework.kinds.iter().find(|k| k.kind == "casework"))
}

/// The casework rectangles the plan draws (closed four-sided outlines on a casework layer): their
/// bounds and handles, to name each item's source.
fn casework_outlines(d: &Drawing) -> Vec<(Bounds2, Handle)> {
    d.model
        .iter()
        .take(MAX_SCAN)
        .filter(|e| e.common.layer.eq_ignore_ascii_case("CASEWORK") && super::wallgen::made_as(e).is_none())
        .filter(|e| matches!(&e.kind, EntityKind::LwPolyline(p) if p.closed))
        .map(|e| (cadaddy_doc::entity_bounds(d, e, 0), e.handle))
        .filter(|(b, _)| !b.is_empty())
        .take(MAX_SOURCES)
        .collect()
}

fn casework_pieces(d: &Drawing, sp: &BuildingSpec, upf: f64, notes: &mut Vec<String>) -> Vec<Piece> {
    let r = rules();
    let (items, scale) = read_casework(d, upf);
    let outlines = casework_outlines(d);
    let inch = upf / 12.0;
    let mut out: Vec<Piece> = Vec::new();
    let mut no_level = 0usize;
    for (i, c) in items.iter().enumerate().take(MAX_ELEMENTS) {
        let (lo, hi) = (Vec2::new(c.min_in[0] * scale, c.min_in[1] * scale), Vec2::new(c.max_in[0] * scale, c.max_in[1] * scale));
        let (w, h) = (hi.x - lo.x, hi.y - lo.y);
        if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
            continue;
        }
        let at = (lo + hi) * 0.5;
        let Some(level) = level_at(&sp.levels, at.x, at.y) else {
            no_level += 1;
            continue;
        };
        let Some(rule) = casework_kind(r, &c.kind) else { continue };
        let documented = c.height_in.is_finite() && c.height_in > 0.0;
        let wall = rule.mount.eq_ignore_ascii_case("wall");
        // A height on the documents is a floor item's top and a hung one's bottom.
        let (bottom_in, top_in) = match (wall, documented) {
            (true, true) => (c.height_in, c.height_in + rule.tall_in.max(1.0)),
            (true, false) => (rule.bottom_in, rule.bottom_in + rule.tall_in.max(1.0)),
            (false, true) => (0.0, c.height_in),
            (false, false) => (0.0, rule.top_in.max(1.0)),
        };
        let (height_source, height_note) = if documented {
            let src = if c.status == "schedule" {
                Source::from_doc(SourceKind::Schedule, casework_reader::SCHEDULE_TITLE)
            } else {
                Source::from_doc(SourceKind::Note, "the plan's AFF note by its tag")
            };
            let other = if wall { "its top" } else { "its bottom (a hung item)" };
            (src, Some(format!("the AFF height read as {}; {other} is the other reading", if wall { "its bottom" } else { "its top" })))
        } else {
            (
                Source::from_doc(SourceKind::Rule, "furniture.json casework"),
                Some(format!("generic: no document gives its height; {}", r.casework.source)),
            )
        };
        let source = outlines
            .iter()
            .find(|(b, _)| {
                (b.min.x - lo.x).abs() <= inch && (b.min.y - lo.y).abs() <= inch && (b.max.x - hi.x).abs() <= inch && (b.max.y - hi.y).abs() <= inch
            })
            .map_or(Handle(0), |(_, h)| *h);
        let read_from = if c.status == "schedule" {
            Source::from_doc(SourceKind::Schedule, casework_reader::SCHEDULE_TITLE)
        } else {
            Source::print(level.sheet.as_deref().unwrap_or_default(), Some([at.x, at.y]))
        };
        let (long, short, angle) = if w >= h { (w, h, 0.0) } else { (h, w, std::f64::consts::FRAC_PI_2) };
        out.push(Piece {
            part: CASEWORK_PART,
            kind: c.kind.clone(),
            tag: c.tag.clone(),
            level: level.name.clone(),
            at,
            angle,
            long,
            short,
            bottom_in,
            top_in,
            generic_height: !documented,
            height_source,
            height_note,
            source,
            read_from,
            made: Made::Drawn,
            basis: String::new(),
            description: c.description.clone(),
            spec_section: c.spec_section.clone(),
            found: c.status.clone(),
            room: None,
        });
        let _ = i;
    }
    // A counter over a cabinet is its top: the slab, not a second cabinet.
    let cabinets: Vec<(String, Vec2, f64, f64)> =
        out.iter().filter(|p| p.kind != "counter" && p.bottom_in <= 0.0).map(|p| (p.level.clone(), p.at, p.long, p.short)).collect();
    for p in out.iter_mut().filter(|p| p.kind == "counter") {
        let over = cabinets
            .iter()
            .any(|(l, at, long, _)| l == &p.level && at.dist(p.at) <= (p.long + long) / 2.0 && at.dist(p.at) < p.long.max(*long) / 2.0);
        let slab = casework_kind(r, "counter").map_or(1.5, |k| k.slab_in.max(0.0));
        if over && slab > 0.0 {
            p.bottom_in = (p.top_in - slab).max(0.0);
        }
    }
    if no_level > 0 {
        notes.push(format!("{no_level} casework item(s) read outside every level's plan: not built."));
    }
    out
}

// ─── Furniture ───────────────────────────────────────────────────────────────

/// A thing on the plan that may be furniture: its centre, long side's direction, sides (sheet
/// units), outline (a closed shape's), handle and layer.
struct Shape {
    handle: Handle,
    at: Vec2,
    angle: f64,
    long: f64,
    short: f64,
    outline: Vec<Vec2>,
    layer: String,
    /// The level it stands on, and the registered sheet it was drawn on when that is not the
    /// level's plan (an enlarged plan put onto it by the door and room numbers they share).
    level: String,
    sheet: Option<String>,
}

/// Where a point of the sheets is on a level's plan: on a level's plan, as it is; on a sheet the
/// spec registers onto a level (an enlarged plan, by the door and room numbers it shares), by that
/// registration. The level, the point, the turn and the scale (sheet units to the plan's).
fn place(sp: &BuildingSpec, p: Vec2) -> Option<(&cadaddy_walk3d::Level, Vec2, f64, f64, Option<String>)> {
    if let Some(l) = level_at(&sp.levels, p.x, p.y).filter(|l| l.region.is_some() || sp.sheets.iter().all(|s| s.registration.is_none())) {
        return Some((l, p, 0.0, 1.0, None));
    }
    for link in sp.sheets.iter().take(10_000) {
        let (Some(rect), Some(reg), Some(name)) = (link.rect, link.registration.as_ref(), link.level.as_deref()) else { continue };
        let inside = p.x >= rect[0].min(rect[2]) && p.x <= rect[0].max(rect[2]) && p.y >= rect[1].min(rect[3]) && p.y <= rect[1].max(rect[3]);
        let k = reg.scale();
        if !inside || !k.is_finite() || k <= 1e-9 {
            continue;
        }
        let q = reg.to_project([p.x, p.y]);
        let q = Vec2::new(q[0], q[1]);
        let Some(level) = sp.level(name) else { continue };
        // Onto its level's plan, never another's.
        if level_at(&sp.levels, q.x, q.y).is_some_and(|l| l.name.eq_ignore_ascii_case(&level.name)) {
            return Some((level, q, reg.b.atan2(reg.a), k, Some(link.number.clone())));
        }
    }
    level_at(&sp.levels, p.x, p.y).map(|l| (l, p, 0.0, 1.0, None))
}

/// A shape on its level's plan (by [`place`]), none off every level.
fn placed(sp: &BuildingSpec, mut s: Shape) -> Option<Shape> {
    let (level, at, turn, k, sheet) = place(sp, s.at)?;
    if sheet.is_some() {
        let from = s.at;
        s.outline = s.outline.iter().map(|p| at + (*p - from).rotate(turn) * k).collect();
        s.at = at;
        s.angle += turn;
        s.long *= k;
        s.short *= k;
    }
    s.level = level.name.clone();
    s.sheet = sheet;
    Some(s)
}

/// The smallest rectangle round `pts`: centre, angle of its long side, (long, short).
fn min_rect(pts: &[Vec2]) -> Option<(Vec2, f64, f64, f64)> {
    if pts.len() < 3 || pts.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
        return None;
    }
    let mut best: Option<(f64, Vec2, f64, f64, f64)> = None;
    let n = pts.len();
    for i in 0..n.min(MAX_VERTICES) {
        let (Some(a), Some(b)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
        let dir = *b - *a;
        if dir.len() < 1e-12 {
            continue;
        }
        let ang = dir.angle();
        let (u, v) = (Vec2::from_angle(ang), Vec2::from_angle(ang).perp());
        let (mut lo, mut hi) = (Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
        for p in pts {
            let q = Vec2::new(p.dot(u), p.dot(v));
            lo = Vec2::new(lo.x.min(q.x), lo.y.min(q.y));
            hi = Vec2::new(hi.x.max(q.x), hi.y.max(q.y));
        }
        let (w, h) = (hi.x - lo.x, hi.y - lo.y);
        if best.as_ref().is_none_or(|x| w * h < x.0) {
            let mid = (lo + hi) * 0.5;
            let c = u * mid.x + v * mid.y;
            best = Some(if w >= h { (w * h, c, ang, w, h) } else { (w * h, c, ang + std::f64::consts::FRAC_PI_2, h, w) });
        }
    }
    best.map(|(_, c, a, l, s)| (c, a, l, s))
}

/// The closed outlines and block references of the plan that may be furniture.
fn shapes(d: &Drawing, sp: &BuildingSpec, r: &Rules) -> (Vec<Shape>, Vec<(Shape, String)>, usize) {
    let mut outlines = Vec::new();
    let mut blocks = Vec::new();
    let mut off_levels = 0usize;
    for e in d.model.iter().take(MAX_SCAN) {
        if outlines.len() + blocks.len() >= MAX_SOURCES * 4 {
            break;
        }
        if super::wallgen::made_as(e).is_some() || layer_is(&r.skip_layers, &e.common.layer) {
            continue;
        }
        // A layer the spec builds as something else (walls, doors, devices…) is not furniture.
        if sp.role(&e.common.layer).is_some_and(|x| !matches!(x.role, Role::Ignore)) {
            continue;
        }
        match &e.kind {
            EntityKind::LwPolyline(p) if p.closed && p.vertices.len() >= 3 && p.vertices.len() <= MAX_VERTICES => {
                let pts: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
                if let Some((at, angle, long, short)) = min_rect(&pts) {
                    let sh = Shape {
                        handle: e.handle,
                        at,
                        angle,
                        long,
                        short,
                        outline: pts,
                        layer: e.common.layer.to_string(),
                        level: String::new(),
                        sheet: None,
                    };
                    match placed(sp, sh) {
                        Some(sh) => outlines.push(sh),
                        None => off_levels += 1,
                    }
                }
            }
            EntityKind::Insert(i) => {
                if cadaddy_doc::library::models3d::model(&i.block).is_some() {
                    continue;
                }
                let Some(b) = d.block(&i.block) else { continue };
                let mut bb = Bounds2::EMPTY;
                for be in b.entities.iter().take(10_000) {
                    let x = cadaddy_doc::entity_bounds(d, be, 1);
                    if !x.is_empty() {
                        bb = bb.union(&x);
                    }
                }
                if bb.is_empty() {
                    continue;
                }
                let (sx, sy) = (i.scale.x.abs(), i.scale.y.abs());
                let off = bb.center() - b.base.xy();
                let at = i.insert.xy() + Vec2::new(off.x * i.scale.x, off.y * i.scale.y).rotate(i.rotation);
                let (w, h) = (bb.width() * sx, bb.height() * sy);
                if !(at.x.is_finite() && at.y.is_finite() && w.is_finite() && h.is_finite()) {
                    continue;
                }
                let (long, short, angle) = if w >= h { (w, h, i.rotation) } else { (h, w, i.rotation + std::f64::consts::FRAC_PI_2) };
                let outline = rect(at, angle, long, short);
                let sh =
                    Shape { handle: e.handle, at, angle, long, short, outline, layer: e.common.layer.to_string(), level: String::new(), sheet: None };
                match placed(sp, sh) {
                    Some(sh) => blocks.push((sh, i.block.clone())),
                    None => off_levels += 1,
                }
            }
            _ => {}
        }
    }
    (outlines, blocks, off_levels)
}

/// A rectangle's corners: centre, long side's direction, sides.
fn rect(at: Vec2, angle: f64, long: f64, short: f64) -> Vec<Vec2> {
    let (u, v) = (Vec2::from_angle(angle) * (long / 2.0), Vec2::from_angle(angle).perp() * (short / 2.0));
    vec![at - u - v, at + u - v, at + u + v, at - u + v]
}

/// The short texts of the plan that name a furniture kind: where, and the kind.
fn labels<'a>(d: &Drawing, sp: &BuildingSpec, r: &'a Rules) -> Vec<(Vec2, &'a FurnitureKind)> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_SCAN) {
        if out.len() >= MAX_LABELS {
            break;
        }
        let text = match &e.kind {
            EntityKind::Text(t) => t.value.as_str(),
            EntityKind::MText(t) => t.contents.as_str(),
            _ => continue,
        };
        if text.len() > 80 || layer_is(&r.skip_layers, &e.common.layer) {
            continue;
        }
        let plain = plain(text);
        let words = plain.split_whitespace().count();
        if words == 0 || words > r.label_max_words.max(1) {
            continue;
        }
        let Some(k) = kind_named(r, &plain) else { continue };
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if !b.is_empty()
            && let Some((_, at, _, _, _)) = place(sp, b.center())
        {
            out.push((at, k));
        }
    }
    out
}

/// A text without its MTEXT codes (a paragraph break a space; font, height, colour codes and braces dropped).
fn plain(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut it = t.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '{' | '}' => {}
            '\\' => match it.next() {
                Some('P' | 'p' | 'X' | '~') => out.push(' '),
                Some('f' | 'F' | 'H' | 'h' | 'C' | 'c' | 'T' | 't' | 'Q' | 'q' | 'W' | 'w' | 'A' | 'a') => {
                    for x in it.by_ref() {
                        if x == ';' {
                            break;
                        }
                    }
                }
                Some(o) => out.push(o),
                None => {}
            },
            _ => out.push(c),
        }
    }
    out
}

/// The kind a drawn size is (the closest usual footprint within the tolerance).
fn kind_by_size(r: &Rules, long_in: f64, short_in: f64) -> Option<&FurnitureKind> {
    let tol = r.size_tolerance.clamp(0.0, 1.0);
    r.furniture
        .kinds
        .iter()
        .flat_map(|k| k.sizes_in.iter().map(move |s| (k, s)))
        .filter_map(|(k, s)| {
            let (a, b) = (s[0].max(s[1]), s[0].min(s[1]));
            if a <= 0.0 || b <= 0.0 {
                return None;
            }
            let e = ((long_in - a) / a).abs().max(((short_in - b) / b).abs());
            (e <= tol).then_some((e, k))
        })
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .map(|(_, k)| k)
}

fn generic_kind(r: &Rules) -> Option<&FurnitureKind> {
    r.furniture.kinds.iter().find(|k| k.kind == "furniture")
}

fn furniture_pieces(d: &Drawing, sp: &BuildingSpec, upf: f64, casework: &[Piece], notes: &mut Vec<String>) -> Vec<Piece> {
    let r = rules();
    let inch = upf / 12.0;
    let (outlines, blocks, off_levels) = shapes(d, sp, r);
    let labels = labels(d, sp, r);
    let side_ok = |s: &Shape| {
        let (l, w) = (s.long / inch, s.short / inch);
        l <= r.max_side_in && w >= r.min_side_in
    };
    // (shape, kind, found)
    let mut found: Vec<(&Shape, &FurnitureKind, &'static str)> = Vec::new();
    for (s, name) in &blocks {
        if !side_ok(s) {
            continue;
        }
        let (l, w) = (s.long / inch, s.short / inch);
        let named = kind_named(r, name).filter(|k| k.fits(l, w));
        let k = match named {
            Some(k) => Some((k, "block")),
            None if layer_is(&r.furniture_layers, &s.layer) => {
                kind_by_size(r, l, w).map(|k| (k, "size")).or_else(|| generic_kind(r).map(|k| (k, "layer")))
            }
            None => None,
        };
        if let Some((k, how)) = k {
            found.push((s, k, how));
        }
    }
    // Each label: the largest outline round it that is its kind's size.
    let reach = r.label_reach_in.max(0.0) * inch;
    let mut labelled: BTreeMap<usize, &FurnitureKind> = BTreeMap::new();
    for (p, k) in &labels {
        let best = outlines
            .iter()
            .enumerate()
            .filter(|(_, s)| side_ok(s) && k.fits(s.long / inch, s.short / inch))
            .filter(|(_, s)| s.at.dist(*p) <= (s.long / 2.0) + reach && (point_in_polygon(&s.outline, *p) || near_outline(&s.outline, *p, reach)))
            .max_by(|a, b| (a.1.long * a.1.short).total_cmp(&(b.1.long * b.1.short)));
        if let Some((i, _)) = best {
            labelled.entry(i).or_insert(*k);
        }
    }
    for (i, s) in outlines.iter().enumerate() {
        if !side_ok(s) {
            continue;
        }
        if let Some(k) = labelled.get(&i) {
            found.push((s, *k, "label"));
        } else if layer_is(&r.furniture_layers, &s.layer) {
            let (l, w) = (s.long / inch, s.short / inch);
            if let Some(k) = kind_by_size(r, l, w) {
                found.push((s, k, "size"));
            } else if let Some(k) = generic_kind(r).filter(|k| k.fits(l, w)) {
                found.push((s, k, "layer"));
            }
        }
    }
    // A shape inside another found shape (a pillow on its bed, a cushion on its sofa) is a part of
    // it; two at one place (a block and its outline) are one.
    let same = r.same_place_in.max(0.0) * inch;
    let mut keep: Vec<(&Shape, &FurnitureKind, &'static str)> = Vec::new();
    let mut by_area: Vec<_> = found;
    by_area.sort_by(|a, b| (b.0.long * b.0.short).total_cmp(&(a.0.long * a.0.short)).then(a.0.handle.0.cmp(&b.0.handle.0)));
    for f in by_area {
        let inside = keep.iter().any(|k| {
            (k.0.at.dist(f.0.at) <= same && (k.0.long - f.0.long).abs() <= same * 2.0)
                || (point_in_polygon(&k.0.outline, f.0.at) && f.0.long * f.0.short < k.0.long * k.0.short)
        });
        // A cabinet the casework reader built there is casework, not furniture.
        let is_casework = casework.iter().any(|c| c.at.dist(f.0.at) <= same.max(c.short / 2.0) && (c.long - f.0.long).abs() <= same * 2.0);
        if !inside && !is_casework && keep.len() < MAX_ELEMENTS {
            keep.push(f);
        }
    }
    let mut out = Vec::new();
    for (s, k, how) in keep {
        let Some(level) = sp.level(&s.level) else { continue };
        if super::wallgen::roof_level(&sp.levels, &level.name) {
            continue;
        }
        let sheet = s.sheet.as_deref().or(level.sheet.as_deref()).unwrap_or_default();
        let read_from = Source::print(sheet, Some([s.at.x, s.at.y]));
        out.push(Piece {
            part: FURNITURE_PART,
            kind: k.kind.clone(),
            tag: String::new(),
            level: level.name.clone(),
            at: s.at,
            angle: s.angle,
            long: s.long,
            short: s.short,
            bottom_in: 0.0,
            top_in: k.height_in.max(1.0),
            generic_height: true,
            height_source: Source::from_doc(SourceKind::Rule, "furniture.json furniture"),
            height_note: Some(format!("generic: no document gives its height; {}", r.furniture.source)),
            source: s.handle,
            read_from,
            made: Made::Drawn,
            basis: String::new(),
            description: String::new(),
            spec_section: None,
            found: if s.sheet.is_some() { format!("{how} on {sheet}") } else { how.to_string() },
            room: None,
        });
    }
    if off_levels > 0 {
        notes.push(format!(
            "{off_levels} closed outline(s) or block(s) lie outside every level's plan and every sheet registered onto one (details, schedules, an enlarged plan not registered by its room and door numbers): no furniture read there."
        ));
    }
    out
}

fn near_outline(poly: &[Vec2], p: Vec2, reach: f64) -> bool {
    reach > 0.0 && seg_dist(poly, p) <= reach
}

/// The distance from `p` to the nearest edge of the closed outline `poly`.
fn seg_dist(poly: &[Vec2], p: Vec2) -> f64 {
    let n = poly.len();
    let mut best = f64::INFINITY;
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n)) else { continue };
        let ab = *b - *a;
        let l2 = ab.dot(ab);
        let t = if l2 > 1e-18 { ((p - *a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
        best = best.min((*a + ab * t).dist(p));
    }
    best
}

// ─── Rooms and repetition ────────────────────────────────────────────────────

fn room_of(rooms: &[RoomRef], level: &str, p: Vec2) -> Option<usize> {
    rooms.iter().position(|r| r.level.eq_ignore_ascii_case(level) && r.outline.len() >= 3 && point_in_polygon(&r.outline, p))
}

/// One of the eight ways an outline can be laid on another its size (turned by quarter turns,
/// mirrored or not), about their centres.
#[derive(Clone, Copy, Debug)]
struct Lay {
    quarter: u8,
    mirror: bool,
}

impl Lay {
    fn apply(self, v: Vec2) -> Vec2 {
        let v = if self.mirror { Vec2::new(-v.x, v.y) } else { v };
        v.rotate(f64::from(self.quarter) * std::f64::consts::FRAC_PI_2)
    }
    fn angle(self, a: f64) -> f64 {
        let a = if self.mirror { std::f64::consts::PI - a } else { a };
        a + f64::from(self.quarter) * std::f64::consts::FRAC_PI_2
    }
    fn words(self) -> String {
        let turn = match self.quarter {
            0 => "as drawn",
            1 => "turned a quarter left",
            2 => "turned half round",
            _ => "turned a quarter right",
        };
        if self.mirror { format!("mirrored, {turn}") } else { turn.to_string() }
    }
}

fn centre_of(pts: &[Vec2]) -> Vec2 {
    Bounds2::from_points(pts.iter().copied()).center()
}

/// How `from` lies on `to` (the best of the eight ways, its mean distance), when their outlines
/// match within `tol`. Several ways equally good (a plain rectangle): the first, and whether
/// another fits as well.
fn lay_on(from: &[Vec2], to: &[Vec2], tol: f64) -> Option<(Lay, f64, bool)> {
    let (cf, ct) = (centre_of(from), centre_of(to));
    let mut tried: Vec<(Lay, f64)> = Vec::new();
    for mirror in [false, true] {
        for quarter in 0..4u8 {
            let lay = Lay { quarter, mirror };
            let moved: Vec<Vec2> = from.iter().map(|p| ct + lay.apply(*p - cf)).collect();
            let a = moved.iter().map(|p| seg_dist(to, *p)).sum::<f64>() / moved.len().max(1) as f64;
            let b = to.iter().map(|p| seg_dist(&moved, *p)).sum::<f64>() / to.len().max(1) as f64;
            let score = a.max(b);
            if score.is_finite() {
                tried.push((lay, score));
            }
        }
    }
    let best = tried.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1))?;
    if best.1 > tol {
        return None;
    }
    let first = tried.iter().copied().find(|t| t.1 <= best.1 + tol * 0.1)?;
    let ties = tried.iter().filter(|t| t.1 <= best.1 + tol * 0.1).count() > 1;
    Some((first.0, first.1, ties))
}

/// The furniture of a furnished room of each type, laid into the rooms of that type the plans
/// leave empty. Returns the copies and how many rooms took them, and notes the rooms that could
/// not (their outline matches no furnished room of their type).
fn repeat(pieces: &[Piece], rooms: &[RoomRef], upf: f64, notes: &mut Vec<String>) -> (Vec<Piece>, usize) {
    let r = rules();
    if !r.repeat.on {
        return (Vec::new(), 0);
    }
    let tol = r.repeat.match_tolerance_in.max(0.0) * upf / 12.0;
    let mut by_room: BTreeMap<usize, Vec<&Piece>> = BTreeMap::new();
    for p in pieces.iter().filter(|p| p.part == FURNITURE_PART && p.made.is_drawn()) {
        if let Some(i) = p.room {
            by_room.entry(i).or_default().push(p);
        }
    }
    let mut out = Vec::new();
    let mut filled = 0usize;
    let mut unmatched: BTreeMap<String, usize> = BTreeMap::new();
    for (ti, target) in rooms.iter().enumerate().take(MAX_ROOMS) {
        if target.type_key.is_empty() || by_room.contains_key(&ti) || target.outline.len() < 3 {
            continue;
        }
        // The furnished rooms of its type, the most furnished first.
        let mut sources: Vec<(&usize, &Vec<&Piece>)> =
            by_room.iter().filter(|(i, _)| rooms.get(**i).is_some_and(|s| s.type_key == target.type_key)).collect();
        if sources.is_empty() {
            continue;
        }
        sources.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
        let fit = sources.iter().find_map(|(si, ps)| {
            let s = rooms.get(**si)?;
            lay_on(&s.outline, &target.outline, tol).map(|l| (s, ps, l))
        });
        let Some((src, ps, (lay, _, ties))) = fit else {
            *unmatched.entry(target.type_key.clone()).or_default() += 1;
            continue;
        };
        let (cf, ct) = (centre_of(&src.outline), centre_of(&target.outline));
        let choice =
            if ties { "; its outline fits other ways as well: laid as the typical room is drawn, the mirror image is the other choice" } else { "" };
        let basis = format!("{} (room {}, {}{choice})", r.repeat.basis, src.label, lay.words());
        for p in ps.iter().take(MAX_ELEMENTS.saturating_sub(pieces.len() + out.len())) {
            let mut c = (*p).clone();
            c.at = ct + lay.apply(p.at - cf);
            c.angle = lay.angle(p.angle);
            c.level = target.level.clone();
            c.made = Made::Derived;
            c.basis = basis.clone();
            c.found = "repeated".into();
            c.room = Some(ti);
            out.push(c);
        }
        filled += 1;
    }
    for (t, n) in unmatched {
        notes.push(format!("{n} room(s) of type {t} draw no furniture and match no furnished room of their type: left unfurnished (the plans furnish none like them)."));
    }
    (out, filled)
}

// ─── Building ────────────────────────────────────────────────────────────────

fn ensure_layer(d: &mut Drawing, name: &str, color: u8) {
    if d.layer(name).is_none() {
        d.layers.push(cadaddy_doc::Layer { name: name.into(), color: cadaddy_doc::color::Color::Index(color), ..cadaddy_doc::Layer::default() });
    }
}

fn add_piece(d: &mut Drawing, sp: &BuildingSpec, upf: f64, rooms: &[RoomRef], p: &Piece) -> bool {
    let Some(level) = sp.level(&p.level) else { return false };
    let inch = upf / 12.0;
    let floor = level.elevation_ft * upf;
    let (z0, z1) = (floor + p.bottom_in * inch, floor + p.top_in * inch);
    let u = Vec2::from_angle(p.angle) * (p.long / 2.0);
    let mut m: Mesh = super::wallgen::empty_mesh();
    super::wallgen::add_box(&mut m, p.at - u, p.at + u, p.short, z0, z1);
    if m.faces.is_empty() {
        return false;
    }
    let ty = title(&p.kind);
    let name = if p.tag.is_empty() { ty.clone() } else { p.tag.clone() };
    let (w_in, d_in) = (round2(p.long / inch), round2(p.short / inch));
    let t = |v: String| RawTag { code: 1000, value: v.chars().take(250).collect() };
    let mut x = vec![
        RawTag { code: 1001, value: super::wallgen::APP.into() },
        t(p.part.into()),
        t(p.level.clone()),
        t(p.source.hex()),
        t(name.replace(':', " ")),
        t(format!("kind:{}", p.kind)),
        t(format!("type:{ty}")),
        t(format!("size:{w_in}x{d_in}")),
        t(format!("found:{}", p.found)),
    ];
    if !p.tag.is_empty() {
        x.push(t(format!("tag:{}", p.tag)));
    }
    let room = p.room.and_then(|i| rooms.get(i));
    if let Some(rm) = room.filter(|r| !r.label.is_empty()) {
        x.push(t(format!("room:{}", rm.label)));
    }
    if p.generic_height {
        x.push(t(format!("{GENERIC_TAG}:height")));
    }
    x.extend(super::provenance::tags(p.made, &p.basis));
    // Its properties, each with its source.
    let mut store = Props::default();
    let drawn = p.read_from.clone();
    store.set("kind", Prop::new(p.kind.clone(), drawn.clone()));
    store.set("type", Prop::new(ty, drawn.clone()));
    if !p.tag.is_empty() {
        store.set("type_mark", Prop::new(p.tag.clone(), drawn.clone()));
    }
    store.set("width", Prop::new(w_in, drawn.clone()).with_unit("in"));
    store.set("depth", Prop::new(d_in, drawn.clone()).with_unit("in"));
    let mut h = Prop::new(round2(p.top_in - p.bottom_in), p.height_source.clone()).with_unit("in");
    h.note.clone_from(&p.height_note);
    store.set("height", h);
    let mut top = Prop::new(round2(p.top_in), p.height_source.clone()).with_unit("in");
    top.note = Some("its top above the floor".into());
    store.set("top_height", top);
    if p.bottom_in > 0.0 {
        let mut b = Prop::new(round2(p.bottom_in), p.height_source.clone()).with_unit("in");
        b.note = Some("its bottom above the floor".into());
        store.set("mount_height", b);
    }
    let mut g = Prop::new(p.generic_height, Source::computed());
    g.note = Some("true: its height is a generic size from the rules data (furniture.json), not the documents'".into());
    store.set("generic", g);
    if !p.description.is_empty() {
        store.set("description", Prop::new(p.description.clone(), Source::from_doc(SourceKind::Schedule, casework_reader::SCHEDULE_TITLE)));
    }
    if let Some(sec) = &p.spec_section {
        store.set("spec_section", Prop::new(sec.clone(), Source::from_doc(SourceKind::Schedule, casework_reader::SCHEDULE_TITLE)));
    }
    if let Some(rm) = room.filter(|r| !r.label.is_empty()) {
        store.set("room", Prop::new(rm.label.clone(), Source::computed()));
    }
    store.set("level", Prop::new(p.level.clone(), Source::computed()));
    let xdata = props::write(&x, &store);
    let layer = if p.part == CASEWORK_PART { CASEWORK_LAYER } else { FURNITURE_LAYER };
    d.add(&Space::Model, Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok()
}

/// Builds the casework and furniture of the plans into model space (sheet units): the counts read
/// and built, how many have a generic height, and what was repeated.
pub(crate) fn build(d: &mut Drawing, sp: &BuildingSpec, rooms: &[RoomRef]) -> (Value, Value) {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut notes: Vec<String> = Vec::new();
    let mut cw = casework_pieces(d, sp, upf, &mut notes);
    let mut fu = furniture_pieces(d, sp, upf, &cw, &mut notes);
    for p in cw.iter_mut().chain(fu.iter_mut()) {
        p.room = room_of(rooms, &p.level, p.at);
    }
    let read_furniture = fu.len();
    let (copies, rooms_filled) = repeat(&fu, rooms, upf, &mut notes);
    fu.extend(copies);
    if !cw.is_empty() {
        ensure_layer(d, CASEWORK_LAYER, 32);
    }
    if !fu.is_empty() {
        ensure_layer(d, FURNITURE_LAYER, 42);
    }
    let count = |ps: &[Piece], d: &mut Drawing| {
        let mut built = 0usize;
        let mut generic = 0usize;
        let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_made: BTreeMap<&'static str, usize> = BTreeMap::new();
        for p in ps.iter().take(MAX_ELEMENTS) {
            if add_piece(d, sp, upf, rooms, p) {
                built += 1;
                generic += usize::from(p.generic_height);
                *by_kind.entry(p.kind.clone()).or_default() += 1;
                *by_made.entry(p.made.as_str()).or_default() += 1;
            }
        }
        (built, generic, by_kind, by_made)
    };
    let (cb, cg, ck, _) = count(&cw, d);
    let mut by_found: BTreeMap<String, usize> = BTreeMap::new();
    for p in &cw {
        *by_found.entry(p.found.clone()).or_default() += 1;
    }
    let casework = json!({
        "read": cw.len(), "built": cb, "generic": cg, "by_kind": ck, "by_status": by_found,
        "in_rooms": cw.iter().filter(|p| p.room.is_some()).count(),
    });
    let (fb, fg, fk, fm) = count(&fu, d);
    let mut how: BTreeMap<String, usize> = BTreeMap::new();
    for p in &fu {
        *how.entry(p.found.clone()).or_default() += 1;
    }
    let furniture = json!({
        "read": read_furniture, "built": fb, "generic": fg, "by_kind": fk, "by_made": fm, "by_found": how,
        "repeated": fu.len().saturating_sub(read_furniture), "rooms_furnished_by_type": rooms_filled,
        "in_rooms": fu.iter().filter(|p| p.room.is_some()).count(),
        "notes": notes,
    });
    (casework, furniture)
}

/// Whether a built element is casework or furniture: its trade key (`cabinets`, `furniture`).
pub(crate) fn trade_of(e: &Entity) -> Option<&'static str> {
    match super::wallgen::made_as(e)?.as_str() {
        CASEWORK_PART => Some("cabinets"),
        FURNITURE_PART => Some("furniture"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_data_reads() {
        let r = rules();
        assert!(r.furniture.kinds.len() >= 8, "{r:?}");
        assert!(r.casework.kinds.iter().any(|k| k.kind == "upper_cabinet" && k.mount == "wall"));
        assert!(r.furniture.kinds.iter().all(|k| k.height_in > 0.0 && !k.words.is_empty()));
        assert!(!r.furniture.source.is_empty() && !r.casework.source.is_empty());
    }

    #[test]
    fn kinds_by_their_whole_words() {
        let r = rules();
        assert_eq!(kind_named(r, "QUEEN BED").map(|k| k.kind.as_str()), Some("bed"));
        assert_eq!(kind_named(r, "FURN_CHAIR01").map(|k| k.kind.as_str()), Some("chair"));
        assert_eq!(kind_named(r, "COFFEE TABLE").map(|k| k.kind.as_str()), Some("coffee_table"));
        assert!(kind_named(r, "BEDROOM").is_none());
        assert!(kind_named(r, "TABLEAU").is_none());
    }

    #[test]
    fn a_rectangle_lays_on_its_mirror_and_a_notched_room_finds_its_mirror() {
        let sq = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 5.0), Vec2::new(0.0, 5.0)];
        let moved: Vec<Vec2> = sq.iter().map(|p| *p + Vec2::new(100.0, 0.0)).collect();
        let (lay, _, ties) = lay_on(&sq, &moved, 0.5).unwrap();
        assert!(lay.quarter == 0 && !lay.mirror && ties);
        // An L: its mirror image is only the mirror.
        let l = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 4.0), Vec2::new(4.0, 4.0), Vec2::new(4.0, 8.0), Vec2::new(0.0, 8.0)];
        let mirrored: Vec<Vec2> = l.iter().map(|p| Vec2::new(200.0 - p.x, p.y)).collect();
        let (lay, score, ties) = lay_on(&l, &mirrored, 0.5).unwrap();
        assert!(lay.mirror && lay.quarter == 0 && score < 1e-6 && !ties, "{lay:?} {score}");
        assert!(lay_on(&l, &sq, 0.5).is_none());
    }
}
