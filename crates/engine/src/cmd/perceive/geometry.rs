//! The geometric perceptions: wall face pairs, door swings and opening blocks, and room outlines.

use cadaddy_doc::{Drawing, Entity, EntityKind};
use cadaddy_geom::{Polyline, Segment, Vec2};
use serde_json::{Value, json};

use super::{objects, page, pt, round, texts, units_per_inch};
use crate::cmd::{bool_or, str_param};
use crate::{Result, Session};

/// Straight segments looked at, at most (hostile drawings).
const MAX_SEGS: usize = 200_000;
/// Pairs reported at most.
const MAX_PAIRS: usize = 50_000;
/// Rooms traced at most.
const MAX_ROOMS: usize = 1_000;
/// A room's least width, inches (a schedule cell, a tag box and a sliver between two lines are
/// narrower).
const MIN_ROOM_WIDTH_IN: f64 = 36.0;
/// Rooms traced at most on one sheet region, when the labels are read sheet by sheet.
const MAX_ROOMS_PER_REGION: usize = 250;
/// Rooms traced at most over all the sheet regions (a whole print set).
const MAX_ROOMS_ALL_REGIONS: usize = 8_000;

/// A straight segment of a line or polyline.
#[derive(Clone, Debug)]
pub(crate) struct Seg {
    pub a: Vec2,
    pub b: Vec2,
    pub layer: String,
}

/// The straight segments of the space's lines and polylines.
pub(crate) fn segments(ents: &[&std::sync::Arc<Entity>]) -> Vec<Seg> {
    let mut out = Vec::new();
    for e in ents {
        if out.len() >= MAX_SEGS {
            break;
        }
        match &e.kind {
            EntityKind::Line(l) => out.push(Seg { a: l.a.xy(), b: l.b.xy(), layer: e.common.layer.to_string() }),
            EntityKind::LwPolyline(p) => {
                for sg in (Polyline { vertices: p.vertices.clone(), closed: p.closed }).segments() {
                    if let Segment::Line(l) = sg {
                        out.push(Seg { a: l.a, b: l.b, layer: e.common.layer.to_string() });
                    }
                }
            }
            _ => {}
        }
    }
    out.retain(|s| s.a.is_finite() && s.b.is_finite() && s.a.dist(s.b) > 1e-9);
    out
}

/// Wall size limits in drawing units.
pub(crate) struct Limits {
    pub min_thick: f64,
    pub max_thick: f64,
    pub min_len: f64,
}

impl Limits {
    pub fn default_for(d: &Drawing) -> Limits {
        let k = units_per_inch(d);
        Limits { min_thick: 2.5 * k, max_thick: 20.0 * k, min_len: 12.0 * k }
    }
}

/// A wall candidate: two parallel faces a wall's thickness apart.
pub(crate) struct Pair {
    pub layer: String,
    pub c0: Vec2,
    pub c1: Vec2,
    pub thickness: f64,
    pub length: f64,
    pub confidence: f64,
    /// The two faces (indices into the segments).
    pub faces: [usize; 2],
}

/// Direction buckets: half a degree each over 180°.
const BUCKETS: i64 = 360;

/// Parallel face pairs: segments in the same direction (within half a degree), their lines
/// `min_thick..max_thick` apart, overlapping along the direction by at least `min_len`.
pub(crate) fn pairs(_d: &Drawing, segs: &[Seg], lim: &Limits) -> Vec<Pair> {
    use std::f64::consts::PI;
    // Group by direction (half-degree buckets over [0°, 180°): a face drawn right to left is the
    // same direction). Faces within half a degree are in one bucket or two neighbouring ones.
    let mut groups: std::collections::HashMap<i64, Vec<usize>> = std::collections::HashMap::new();
    let mut angs = Vec::with_capacity(segs.len());
    for (i, s) in segs.iter().enumerate() {
        let d = s.b - s.a;
        let ang = d.y.atan2(d.x).rem_euclid(PI);
        angs.push(ang);
        let key = ((ang.to_degrees() * 2.0).floor() as i64).rem_euclid(BUCKETS);
        groups.entry(key).or_default().push(i);
    }
    let half_degree = 0.5f64.to_radians();
    let mut keys: Vec<i64> = groups.keys().copied().collect();
    keys.sort_unstable();
    let mut out = Vec::new();
    for key in keys {
        // This bucket with the next one (across 180° → 0° too), sorted by offset along one normal
        // for all of them: the shared edge's. Pairs inside the next bucket are its own turn.
        let next = (key + 1).rem_euclid(BUCKETS);
        let edge = ((key + 1) as f64 / 2.0).to_radians();
        let n = Vec2::new(-edge.sin(), edge.cos());
        let mut g: Vec<(f64, usize, bool)> = Vec::new();
        for (k, home) in [(key, true), (next, false)] {
            for &i in groups.get(&k).map(Vec::as_slice).unwrap_or_default() {
                let s = &segs[i];
                g.push((n.dot(s.a.mid(s.b)), i, home));
            }
        }
        g.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Midpoint offsets along the shared normal are off by up to the faces' length times the
        // sine of their half-degree turn from it; the window allows for that.
        let longest = g.iter().map(|&(_, i, _)| segs[i].a.dist(segs[i].b)).fold(0.0, f64::max);
        let window = lim.max_thick + longest * (2.0 * half_degree).sin();
        for x in 0..g.len() {
            for y in x + 1..g.len() {
                if g[y].0 - g[x].0 > window {
                    break;
                }
                if !g[x].2 && !g[y].2 {
                    continue;
                }
                let (i, j) = (g[x].1, g[y].1);
                let turn = (angs[i] - angs[j]).abs();
                if turn.min(PI - turn) > half_degree {
                    continue;
                }
                let Some(p) = face_pair(&segs[i], &segs[j], lim) else { continue };
                let (s1, s2) = (&segs[i], &segs[j]);
                let shorter = s1.a.dist(s1.b).min(s2.a.dist(s2.b)).max(1e-9);
                let wall_layer = s1.layer.to_ascii_uppercase().contains("WALL");
                let confidence = (0.35
                    + 0.25 * f64::from(u8::from(wall_layer))
                    + 0.25 * (p.length / shorter).min(1.0)
                    + 0.15 * f64::from(u8::from(s1.layer == s2.layer)))
                .min(1.0);
                out.push(Pair { layer: s1.layer.clone(), c0: p.c0, c1: p.c1, thickness: p.gap, length: p.length, confidence, faces: [i, j] });
                if out.len() >= MAX_PAIRS {
                    return out;
                }
            }
        }
    }
    out.sort_by(|a, b| b.confidence.total_cmp(&a.confidence).then(b.length.total_cmp(&a.length)));
    out
}

/// Two faces measured as a wall: the gap between their lines, their overlap and the centreline.
struct FacePair {
    gap: f64,
    length: f64,
    c0: Vec2,
    c1: Vec2,
}

/// Two nearly parallel faces as a wall when their lines are `min_thick..max_thick` apart and
/// they overlap by at least `min_len` (and 1½ times the gap), measured along their mean direction.
fn face_pair(s1: &Seg, s2: &Seg, lim: &Limits) -> Option<FacePair> {
    let d1 = (s1.b - s1.a).normalized();
    let mut d2 = (s2.b - s2.a).normalized();
    if d1.dot(d2) < 0.0 {
        d2 = d2 * -1.0;
    }
    let u = (d1 + d2).normalized();
    if !u.is_finite() || u.len() < 0.5 {
        return None;
    }
    let n = Vec2::new(-u.y, u.x);
    let (a0, a1) = minmax(u.dot(s1.a), u.dot(s1.b));
    let (b0, b1) = minmax(u.dot(s2.a), u.dot(s2.b));
    let (lo, hi) = (a0.max(b0), a1.min(b1));
    // Each face's line across `n` at a point `t` along `u`.
    let across = |s: &Seg, d: Vec2, t: f64| {
        let along = u.dot(d);
        n.dot(s.a) + (t - u.dot(s.a)) * n.dot(d) / if along.abs() > 1e-12 { along } else { 1.0 }
    };
    let tm = (lo + hi) / 2.0;
    let gap = (across(s2, d2, tm) - across(s1, d1, tm)).abs();
    let length = hi - lo;
    if !(lim.min_thick..=lim.max_thick).contains(&gap) || length < lim.min_len.max(gap * 1.5) {
        return None;
    }
    let mid = |t: f64| (across(s1, d1, t) + across(s2, d2, t)) / 2.0;
    Some(FacePair { gap, length, c0: u * lo + n * mid(lo), c1: u * hi + n * mid(hi) })
}

fn minmax(a: f64, b: f64) -> (f64, f64) {
    if a <= b { (a, b) } else { (b, a) }
}

pub(super) fn run_walls(s: &mut Session, p: &Value) -> Result<Value> {
    let (d, ents) = objects(s)?;
    let mut lim = Limits::default_for(d);
    let num = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0);
    if let Some(v) = num("minThickness") {
        lim.min_thick = v;
    }
    if let Some(v) = num("maxThickness") {
        lim.max_thick = v;
    }
    if let Some(v) = num("minLength") {
        lim.min_len = v;
    }
    let mut segs = segments(&ents);
    if let Some(pat) = str_param(p, "layers").filter(|x| !x.trim().is_empty()) {
        segs.retain(|g| pat.split(',').any(|q| crate::cmd::qselect::wildcard(q.trim(), &g.layer)));
    }
    let items: Vec<Value> = pairs(d, &segs, &lim)
        .into_iter()
        .map(|w| json!({"layer": w.layer, "centerline": [pt(w.c0), pt(w.c1)], "thickness": round(w.thickness), "length": round(w.length), "confidence": round(w.confidence)}))
        .collect();
    Ok(page(items, p))
}

// ---------------- plan.openings ----------------

/// A door or window found in the plan.
pub(crate) struct Opening {
    pub kind: &'static str,
    /// A door swing's hinge (the arc's centre) or a block's insertion point.
    pub at: Vec2,
    pub width: f64,
    pub layer: String,
    pub handle: cadaddy_doc::Handle,
    /// A door swing's degrees and the arc's two ends.
    pub swing: Option<(f64, [Vec2; 2])>,
    pub block: Option<String>,
    pub confidence: f64,
    /// A swing whose leaf is drawn too (a line from the hinge to an end of the arc).
    pub leaf: bool,
}

/// Door swings (arcs of 60–100° with a door's radius, 16"–54", ideally with a leaf line from the
/// hinge to an end) and door and window blocks (by name).
pub(crate) fn openings(d: &Drawing, ents: &[&std::sync::Arc<Entity>], segs: &[Seg], k: f64) -> Vec<Opening> {
    let (rmin, rmax) = (16.0 * k, 54.0 * k);
    let mut items = Vec::new();
    for e in ents {
        match &e.kind {
            EntityKind::Arc(a) => {
                let mut sweep = (a.end - a.start).rem_euclid(std::f64::consts::TAU);
                if sweep < 1e-9 {
                    sweep = std::f64::consts::TAU;
                }
                let deg = sweep.to_degrees();
                if !(60.0..=100.0).contains(&deg) || !(rmin..=rmax).contains(&a.radius) {
                    continue;
                }
                let c = a.center.xy();
                let ends = [c + Vec2::from_angle(a.start) * a.radius, c + Vec2::from_angle(a.end) * a.radius];
                let tol = a.radius * 0.05;
                let leaf = segs.iter().any(|g| {
                    let (p, q) = (g.a, g.b);
                    ends.iter().any(|e| (p.dist(c) < tol && q.dist(*e) < tol) || (q.dist(c) < tol && p.dist(*e) < tol))
                });
                let on_door_layer = e.common.layer.to_ascii_uppercase().contains("DOOR");
                let confidence = (0.5 + 0.3 * f64::from(u8::from(leaf)) + 0.2 * f64::from(u8::from(on_door_layer))).min(1.0);
                items.push(Opening {
                    kind: "door",
                    at: c,
                    width: a.radius,
                    layer: e.common.layer.to_string(),
                    handle: e.handle,
                    swing: Some((deg, ends)),
                    block: None,
                    confidence,
                    leaf,
                });
            }
            // A swing flattened to a polyline (how PDF prints arrive): a quarter-ish circle run.
            EntityKind::LwPolyline(p) if !p.closed && (3..=200).contains(&p.vertices.len()) => {
                let pts = Polyline { vertices: p.vertices.clone(), closed: false }.tessellate(1e-3);
                let Some((c, r, deg, ends)) = flat_swing(&pts) else { continue };
                if !(rmin..=rmax).contains(&r) {
                    continue;
                }
                let tol = r * 0.05;
                let leaf = segs.iter().any(|g| {
                    let (p, q) = (g.a, g.b);
                    ends.iter().any(|e| (p.dist(c) < tol && q.dist(*e) < tol) || (q.dist(c) < tol && p.dist(*e) < tol))
                });
                let on_door_layer = e.common.layer.to_ascii_uppercase().contains("DOOR");
                let confidence = (0.45 + 0.3 * f64::from(u8::from(leaf)) + 0.2 * f64::from(u8::from(on_door_layer))).min(1.0);
                items.push(Opening {
                    kind: "door",
                    at: c,
                    width: r,
                    layer: e.common.layer.to_string(),
                    handle: e.handle,
                    swing: Some((deg, ends)),
                    block: None,
                    confidence,
                    leaf,
                });
            }
            EntityKind::Insert(i) => {
                let name = i.block.to_ascii_uppercase();
                let kind = if name.contains("DOOR") || name.starts_with("DR") {
                    "door"
                } else if name.contains("WIN") || name.contains("WDW") || name.contains("GLAZ") {
                    "window"
                } else {
                    continue;
                };
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                let width = if b.is_empty() { 0.0 } else { b.width().max(b.height()) };
                items.push(Opening {
                    kind,
                    at: i.insert.xy(),
                    width,
                    layer: e.common.layer.to_string(),
                    handle: e.handle,
                    swing: None,
                    block: Some(i.block.clone()),
                    confidence: 0.75,
                    leaf: false,
                });
            }
            _ => {}
        }
    }
    items
}

/// A run of points that is a door swing: on one circle (within 5% of its radius) through 60–100°.
/// Its centre (the hinge), radius, sweep in degrees and two ends.
pub(crate) fn flat_swing(pts: &[Vec2]) -> Option<(Vec2, f64, f64, [Vec2; 2])> {
    let (first, last) = (*pts.first()?, *pts.last()?);
    let mid = *pts.get(pts.len() / 2)?;
    let d = 2.0 * (first.x * (mid.y - last.y) + mid.x * (last.y - first.y) + last.x * (first.y - mid.y));
    if !d.is_finite() || d.abs() < 1e-12 {
        return None;
    }
    let sq = |p: Vec2| p.x * p.x + p.y * p.y;
    let (a2, b2, c2) = (sq(first), sq(mid), sq(last));
    let c = Vec2::new(
        (a2 * (mid.y - last.y) + b2 * (last.y - first.y) + c2 * (first.y - mid.y)) / d,
        (a2 * (last.x - mid.x) + b2 * (first.x - last.x) + c2 * (mid.x - first.x)) / d,
    );
    let r = first.dist(c);
    if !c.is_finite() || !r.is_finite() || r <= 0.0 || pts.iter().any(|p| (p.dist(c) - r).abs() > 0.05 * r) {
        return None;
    }
    // The sweep: the turn from the first to the last point, through the middle one.
    let ang = |p: Vec2| (p.y - c.y).atan2(p.x - c.x);
    let sweep = |from: f64, to: f64| (to - from).rem_euclid(std::f64::consts::TAU);
    let (s_mid, s_end) = (sweep(ang(first), ang(mid)), sweep(ang(first), ang(last)));
    let deg = if s_mid <= s_end { s_end } else { std::f64::consts::TAU - s_end }.to_degrees();
    (60.0..=100.0).contains(&deg).then_some((c, r, deg, [first, last]))
}

pub(super) fn run_openings(s: &mut Session, p: &Value) -> Result<Value> {
    let (d, ents) = objects(s)?;
    let segs = segments(&ents);
    let items = openings(d, &ents, &segs, units_per_inch(d))
        .into_iter()
        .map(|o| {
            let mut v = json!({"kind": o.kind, "at": pt(o.at), "width": round(o.width), "layer": o.layer, "handle": o.handle.hex(), "confidence": round(o.confidence)});
            if let Some(obj) = v.as_object_mut() {
                if let Some((deg, _)) = o.swing {
                    obj.insert("swing".into(), json!(round(deg)));
                }
                if let Some(b) = o.block {
                    obj.insert("block".into(), json!(b));
                }
            }
            v
        })
        .collect();
    Ok(page(items, p))
}

// ---------------- plan.rooms ----------------

/// A room number: optional letter, 2–4 digits, optional letter (`101`, `B12`, `204A`).
pub(crate) fn room_number(t: &str) -> bool {
    let t = t.trim();
    let digits = t.chars().filter(char::is_ascii_digit).count();
    let letters = t.chars().filter(char::is_ascii_alphabetic).count();
    (2..=4).contains(&digits) && letters <= 2 && t.len() <= 6 && t.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Words of a room's tag: unit types and the rooms a plan names.
const ROOM_WORDS: &[&str] = &[
    "SINGLE",
    "DOUBLE",
    "QUEEN",
    "KING",
    "ADA",
    "SUITE",
    "UNIT",
    "STUDIO",
    "GUEST",
    "BEDROOM",
    "BED",
    "LIVING",
    "CORRIDOR",
    "CORR",
    "HALL",
    "HALLWAY",
    "STAIR",
    "STAIRS",
    "LOBBY",
    "ELEV",
    "ELEVATOR",
    "MECH",
    "MECHANICAL",
    "ELEC",
    "ELECTRICAL",
    "STORAGE",
    "STOR",
    "LAUNDRY",
    "LINEN",
    "OFFICE",
    "TOILET",
    "RESTROOM",
    "WOMEN",
    "MEN",
    "BATH",
    "BATHROOM",
    "KITCHEN",
    "BREAK",
    "VESTIBULE",
    "VEST",
    "CLOSET",
    "CLO",
    "JANITOR",
    "JAN",
    "FITNESS",
    "POOL",
    "LOUNGE",
    "DINING",
    "MEETING",
    "CONFERENCE",
    "CONF",
    "ROOM",
    "UTILITY",
    "IDF",
    "MDF",
    "TRASH",
    "VENDING",
    "ICE",
    "PANTRY",
    "RECEPTION",
    "MAINTENANCE",
    "ENTRY",
    "WORK",
    "SHAFT",
    "CHASE",
    "SPRINKLER",
    "RISER",
    "MARKET",
    "DESK",
    "LIBRARY",
    "CLASSROOM",
];

/// Words of equipment and device tags, never a room's name (`PNLBD 'D'`, `XFMR T1`, `DISC`).
const EQUIPMENT_WORDS: &[&str] = &[
    "PNLBD",
    "PNL",
    "PANEL",
    "PANELBOARD",
    "XFMR",
    "TRANSFORMER",
    "DISC",
    "DISCONNECT",
    "MCC",
    "SWBD",
    "SWITCHBOARD",
    "RTU",
    "AHU",
    "FCU",
    "VFD",
    "ATS",
    "GEN",
    "GENERATOR",
    "JBOX",
    "J-BOX",
    "CKT",
    "CIRCUIT",
    "HOMERUN",
    "CONDUIT",
    "TYP",
    "NIC",
    "SEE",
    "NOTE",
    "NOTES",
    "DETAIL",
    "SHEET",
    "MOUNT",
    "MTD",
    "AFF",
    "EXISTING",
    "RELOCATE",
    "PTAC",
    "EWC",
    "WH",
    "EF",
];

fn tag_words(t: &str) -> Vec<String> {
    t.to_ascii_uppercase().split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// Whether a text is an equipment or device tag, a note or a dimension rather than a room's
/// name: a panelboard or transformer label, a quoted panel letter (`'D'`), a device mark (`OS`,
/// `DH`, `J`), a circuit (`A-12`), a sheet reference or a length.
pub(crate) fn equipment_tag(t: &str) -> bool {
    let t = t.trim();
    let words = tag_words(t);
    let quoted = t.contains('\'') && t.chars().filter(|c| *c == '\'').count() >= 2;
    let length = t.chars().any(|c| c.is_ascii_digit()) && (t.contains('"') || t.contains("'-") || t.contains("' -"));
    let device_mark = words.len() == 1 && cadaddy_mepfix::sheet_facts::is_type_tag(t) && !ROOM_WORDS.contains(&t.to_ascii_uppercase().as_str());
    let circuit =
        words.len() == 1 && t.rsplit_once('-').is_some_and(|(h, n)| !h.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    quoted || length || device_mark || circuit || words.iter().any(|w| EQUIPMENT_WORDS.contains(&w.as_str()))
}

/// Whether a text is a note rather than a room's name: a finish code (`CPT-1`), a ceiling, level
/// or scale note (`CLG 9'-0"`, `LEVEL 2`, `EL. 100'`, `SCALE: 1/4" = 1'-0"`, `NTS`) or a sheet
/// reference (`SEE A-201`, `A-5.1`).
pub(crate) fn note_like(t: &str) -> bool {
    let up = t.trim().to_ascii_uppercase();
    let words = tag_words(&up);
    let has = |w: &str| words.iter().any(|x| x == w);
    let ceiling = has("CLG") || has("CEILING") || has("AFF") || has("ACT");
    let level = up.starts_with("LEVEL") || has("EL") || up.contains("EL.") || up.contains("ELEV.") || up.contains("T.O.") || has("TOS");
    let scale = has("SCALE") || has("NTS") || up.contains("NOT TO SCALE");
    // A sheet reference: a sheet number with a dash or a dot (`A-201`, `E1.1`), or after SEE/REFER.
    let sheet_ref = up.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '/')).enumerate().any(|(i, w)| {
        super::sheet_number(w)
            && (w.contains('-') || w.contains('.') || (i > 0 && (has("SEE") || has("REFER") || has("REF") || has("SHEET") || has("DWG"))))
    });
    !super::finish_codes(&up).is_empty() || ceiling || level || scale || sheet_ref
}

/// How well a text names a room: 3 with a room or unit-type word (`DOUBLE Q`, `CORRIDOR`), 1 for
/// another name, 0 for an equipment or device tag, a note or a number.
pub(crate) fn room_name_score(t: &str) -> u8 {
    if equipment_tag(t) || note_like(t) || room_number(t) || t.chars().filter(|c| c.is_alphabetic()).count() < 2 {
        0
    } else if tag_words(t).iter().any(|w| ROOM_WORDS.contains(&w.as_str())) {
        3
    } else {
        1
    }
}

fn polygon(l: &cadaddy_doc::HatchLoop) -> Vec<Vec2> {
    Polyline { vertices: l.vertices.clone(), closed: true }.tessellate(1e-3)
}

/// A room found from its name label: the closed outline around the label (the hatch boundary
/// search), its number (a number text inside), area and perimeter.
#[derive(Clone, Debug)]
pub(crate) struct FoundRoom {
    pub name: String,
    pub number: Option<String>,
    pub at: Vec2,
    pub label: cadaddy_doc::Handle,
    pub text_height: f64,
    pub outline: Vec<Vec2>,
    pub area: f64,
    pub perimeter: f64,
    pub confidence: f64,
}

/// A room tag block's attribute: `Some(true)` for the room's name (`NAME`, `ROOM_NAME`, `RM NAME`),
/// `Some(false)` for its number (`NUMBER`, `ROOM_NO`, `RM#`), else `None`.
fn tag_attribute(tag: &str) -> Option<bool> {
    let t: String = tag.to_ascii_uppercase().chars().filter(char::is_ascii_alphanumeric).collect();
    let t = ["ROOM", "SPACE", "RM"].iter().find_map(|p| t.strip_prefix(p)).unwrap_or(&t);
    match t {
        "NAME" | "NAM" => Some(true),
        "NUMBER" | "NUM" | "NO" | "NBR" | "" => Some(false),
        _ => None,
    }
}

/// A name or number candidate: (handle, text, where, height, from a room tag block's attribute).
type Label = (cadaddy_doc::Handle, String, Vec2, f64, bool);

/// The rooms of the current space, from its name labels: texts, and the name and number
/// attributes of room tag blocks. With a building spec, the labels on its room tag layers (else
/// the texts naming its rooms), traced on what bounds rooms ([`crate::cmd::bspec::room_plan`]).
/// Each outline takes its best name whatever order the labels were drawn in.
pub(crate) fn rooms_found(s: &mut Session) -> Result<Vec<FoundRoom>> {
    rooms_found_in(s, None)
}

/// The labels to trace, best room names first. With `regions` (sheet rectangles, `[x0, y0, x1,
/// y1]`): only the labels inside one, each region's best [`MAX_ROOMS_PER_REGION`] in turn, so a
/// print set's later sheets are read as fully as its first ones; else the drawing's best
/// [`MAX_ROOMS`].
fn labels_to_trace<T>(mut names: Vec<T>, at: impl Fn(&T) -> Vec2, score: impl Fn(&T) -> u8, regions: Option<&[[f64; 4]]>) -> Vec<T> {
    // A room or unit-type word claims its outline before another label in the same room.
    names.sort_by_key(|n| std::cmp::Reverse(score(n)));
    let Some(regions) = regions else {
        names.truncate(MAX_ROOMS);
        return names;
    };
    let inside = |r: &[f64; 4], p: Vec2| p.x >= r[0].min(r[2]) && p.x <= r[0].max(r[2]) && p.y >= r[1].min(r[3]) && p.y <= r[1].max(r[3]);
    let mut per: Vec<Vec<T>> = regions.iter().map(|_| Vec::new()).collect();
    for n in names {
        let p = at(&n);
        if let Some(i) = regions.iter().position(|r| inside(r, p))
            && let Some(list) = per.get_mut(i)
            && list.len() < MAX_ROOMS_PER_REGION
        {
            list.push(n);
        }
    }
    per.into_iter().flatten().take(MAX_ROOMS_ALL_REGIONS).collect()
}

/// [`rooms_found`], the labels read only inside `regions` (sheet rectangles), sheet by sheet.
pub(crate) fn rooms_found_in(s: &mut Session, regions: Option<&[[f64; 4]]>) -> Result<Vec<FoundRoom>> {
    let (d, ents) = objects(s)?;
    let k = units_per_inch(d);
    let min_area = 16.0 * 144.0 * k * k;
    let ext = d.extents(&s.space());
    let sheet = (ext.width() * ext.height()).max(1e-9);
    let spec = d.building_spec.as_deref().and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok());
    let tagged = spec.as_ref().is_some_and(|sp| sp.layers.iter().any(|r| r.role == cadaddy_walk3d::Role::RoomTag));
    let on_tags: Vec<_> = ents
        .iter()
        .copied()
        .filter(|e| !tagged || spec.as_ref().and_then(|sp| sp.role(&e.common.layer)).is_some_and(|r| r.role == cadaddy_walk3d::Role::RoomTag))
        .collect();
    let mut all: Vec<Label> = texts(&on_tags).into_iter().map(|(h, t, at, ht)| (h, t, at, ht, false)).collect();
    // Room tag blocks: their name and number attributes. The blocks themselves (a box or a bubble
    // around the tag) bound no room.
    let mut tag_blocks = std::collections::HashSet::new();
    let mut tag_numbers: Vec<Label> = Vec::new();
    for e in &on_tags {
        let EntityKind::Insert(i) = &e.kind else { continue };
        // A title block (its sheet number, title, project, date) is not a room tag.
        let title_block = i.attribs.iter().any(|a| {
            let t = a.tag.to_ascii_uppercase();
            ["SHEET", "DWG", "TITLE", "PROJECT", "DATE", "SCALE", "DRAWN"].iter().any(|w| t.contains(w))
        });
        if title_block {
            continue;
        }
        for a in &i.attribs {
            let v = a.text.value.trim();
            if v.is_empty() || v.chars().count() > 40 {
                continue;
            }
            match tag_attribute(&a.tag) {
                Some(true) => all.push((e.handle, v.to_string(), a.text.justify_point(), a.text.height, true)),
                Some(false) => tag_numbers.push((e.handle, v.to_string(), a.text.justify_point(), a.text.height, true)),
                None => continue,
            }
            tag_blocks.insert(e.handle);
        }
    }
    // Untagged, a spec with rooms names them.
    let spec_rooms = spec.as_ref().filter(|sp| !tagged && !sp.rooms.is_empty()).map(|sp| sp.rooms.as_slice());
    let score = |(_, t, _, _, tag): &Label| if *tag { 4 } else { room_name_score(t) };
    let mut names: Vec<Label> = all
        .iter()
        .filter(|(_, t, _, _, tag)| t.chars().count() <= 40 && (*tag || !note_like(t)))
        .filter(|l| match spec_rooms {
            Some(rs) => {
                let t = &l.1;
                l.4 || (t.chars().filter(|c| c.is_alphabetic()).count() >= 2
                    && !room_number(t)
                    && rs.iter().any(|r| crate::cmd::bspec::names_room(r, t)))
            }
            // Equipment and device tags (a panelboard label beside a room) never name it.
            None => score(l) > 0,
        })
        .cloned()
        .collect();
    // The best name claims its outline first, whatever the drawing order: a tag block's name, then
    // a room or unit-type word, then the tallest text, then by text and place.
    names.sort_by(|a, b| {
        score(b).cmp(&score(a)).then(b.3.total_cmp(&a.3)).then_with(|| a.1.cmp(&b.1)).then(b.2.y.total_cmp(&a.2.y)).then(a.2.x.total_cmp(&b.2.x))
    });
    // The drawing's best names, or each sheet's best in turn (the stable sort keeps the order above).
    let names = labels_to_trace(names, |n| n.2, score, regions);
    let mut numbers: Vec<Label> = all.iter().filter(|(_, t, _, _, tag)| !tag && room_number(t)).cloned().collect();
    numbers.extend(tag_numbers);
    let plan = crate::cmd::bspec::room_plan(s)?;
    // Without the tag blocks: a tag's box is not its room.
    let plan = if tag_blocks.is_empty() || s.space().is_paper() {
        plan
    } else {
        let mut wd = plan.as_ref().unwrap_or(s).doc()?.clone();
        for h in &tag_blocks {
            wd.model.remove(*h);
        }
        let mut t = Session::empty();
        t.open_drawing(wd, "Room boundaries", None);
        Some(t)
    };
    let plan = plan.as_ref().unwrap_or(s);
    let mut out: Vec<FoundRoom> = Vec::new();
    let mut outline_cache = None;
    for (h, name, at, height, _) in names {
        let Ok(loops) = crate::cmd::hatch::loops_at_lazy(plan, at, &mut outline_cache) else { continue };
        let Some(outer) = loops.iter().find(|l| l.outer).or(loops.first()) else { continue };
        let poly = polygon(outer);
        if poly.len() < 3 {
            continue;
        }
        let area = cadaddy_geom::shoelace(&poly).abs();
        if area < min_area {
            continue;
        }
        // One room per outline: a second label in the same room is its number or a note.
        if out.iter().any(|q| q.outline.len() == poly.len() && q.outline.iter().zip(&poly).all(|(a, b)| a.dist(*b) < 1e-6)) {
            continue;
        }
        // Read sheet by sheet, a schedule's or legend's cells trace too: no room is narrower.
        let b = cadaddy_geom::Bounds2::from_points(poly.iter().copied());
        if regions.is_some() && b.width().min(b.height()) < MIN_ROOM_WIDTH_IN * k {
            continue;
        }
        let perimeter: f64 = poly.iter().zip(poly.iter().cycle().skip(1)).map(|(a, b)| a.dist(*b)).sum();
        // The room number: a number inside the outline, the one in the name's tag block, else the
        // nearest to the name.
        let number = numbers
            .iter()
            .filter(|(_, _, q, _, _)| cadaddy_geom::point_in_polygon(&poly, *q))
            .min_by(|a, b| u8::from(a.0 != h).cmp(&u8::from(b.0 != h)).then(a.2.dist(at).total_cmp(&b.2.dist(at))).then_with(|| a.1.cmp(&b.1)))
            .map(|(_, t, _, _, _)| t.clone());
        let share = area / sheet;
        let confidence = (0.5 + 0.2 * f64::from(u8::from(number.is_some())) + if share < 0.5 { 0.3 } else { 0.0 }
            - if poly.len() > 200 { 0.1 } else { 0.0 })
        .clamp(0.0, 1.0);
        out.push(FoundRoom { name, number, at, label: h, text_height: height, outline: poly, area, perimeter, confidence });
    }
    Ok(out)
}

pub(super) fn run_rooms(s: &mut Session, p: &Value) -> Result<Value> {
    let with_outline = bool_or(p, "outline", true);
    let items: Vec<Value> = rooms_found(s)?
        .into_iter()
        .map(|r| {
            let mut v = json!({
                "name": r.name, "number": r.number, "at": pt(r.at), "area": round(r.area), "perimeter": round(r.perimeter),
                "label": r.label.hex(), "textHeight": round(r.text_height), "confidence": round(r.confidence),
            });
            if with_outline {
                let step = (r.outline.len() / 64).max(1);
                v["outline"] = json!(r.outline.iter().step_by(step).map(|q| pt(*q)).collect::<Vec<_>>());
            }
            v
        })
        .collect();
    Ok(page(items, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_print_sets_last_sheets_are_read_as_fully_as_its_first() {
        // Four sheets side by side, each with more labels than one reading traces in all; the
        // labels in drawing order (sheet 1's first), all as good names.
        let per_sheet = MAX_ROOMS / 2 + 10;
        let regions: Vec<[f64; 4]> = (0..4).map(|i| [f64::from(i) * 100.0, 0.0, f64::from(i) * 100.0 + 90.0, 90.0]).collect();
        let names: Vec<(usize, Vec2)> =
            (0..4).flat_map(|i| (0..per_sheet).map(move |j| (i, Vec2::new(f64::from(i as u32) * 100.0 + 5.0, 5.0 + (j % 80) as f64)))).collect();
        let by_sheet = |picked: &[(usize, Vec2)]| (0..4).map(|i| picked.iter().filter(|(s, _)| *s == i).count()).collect::<Vec<_>>();
        // Read over the whole drawing: the last sheets get nothing.
        let whole = labels_to_trace(names.clone(), |n| n.1, |_| 3, None);
        assert_eq!(by_sheet(&whole), [per_sheet, MAX_ROOMS - per_sheet, 0, 0]);
        // Sheet by sheet: each its own share, labels outside every sheet left out.
        let mut with_stray = names;
        with_stray.push((9, Vec2::new(-50.0, 5.0)));
        let picked = labels_to_trace(with_stray, |n| n.1, |_| 3, Some(&regions));
        let each = per_sheet.min(MAX_ROOMS_PER_REGION);
        assert_eq!(by_sheet(&picked), [each, each, each, each]);
        assert!(picked.iter().all(|(s, _)| *s != 9));
        // The best names first within a sheet.
        let scored = labels_to_trace(vec![(0, Vec2::new(5.0, 5.0)), (1, Vec2::new(6.0, 5.0))], |n| n.1, |n| u8::from(n.0 == 1), Some(&regions));
        assert_eq!(scored.first().map(|n| n.0), Some(1));
    }
}
