//! The architect locates, the engineer circuits (Q6026): devices go where the architectural
//! sheets put them.
//!
//! Electrical plans are diagrammatic — they say what a device is, its circuit and panel, and
//! roughly where (the E general note: "refer to architectural elevations for all locations and
//! mounting heights"). The architectural sheets fix the spot: interior and millwork elevations
//! give the position along a wall and the mounting height, reflected ceiling and enlarged plans
//! give the plan position. This module reads that evidence and moves each E device of the model
//! onto its architectural counterpart, keeping its circuit:
//!
//! 1. **Elevations.** A view title naming an elevation (`INTERIOR ELEVATION`, `ROOM 101 - NORTH`)
//!    sits under its view; the view's floor line is the long line above the title with wall-end
//!    lines rising from both ends, its top the ceiling. Devices are the tagged symbols, device
//!    marks and device words inside that box; `48" AFF` callouts give heights. The title names
//!    the room (its number, else a name the electrical plan prints once) and, when it does, the
//!    wall (`NORTH` = the north wall, seen from inside the room).
//! 2. **The wall in the model.** The room's tag on the electrical plan, and the wall faces the
//!    plan draws around it (a wall face has its parallel other face a wall thickness behind it),
//!    give the room's walls; an elevation's width maps onto the wall's length, so neither sheet's
//!    scale is needed. With no direction in the title, the wall is the one whose length matches
//!    the elevation's proportions and whose devices line up best.
//! 3. **Matching.** E devices on that wall and elevation devices are paired in order along the
//!    wall (an order-keeping alignment), only between compatible kinds (the same mark, or light ↔
//!    light / junction box, receptacle ↔ receptacle, switch ↔ switch, data ↔ data), within a
//!    tolerance. Architectural plans (RCP, enlarged plans) are registered on the electrical plan
//!    by the room numbers both print, then paired nearest-first inside the same room.
//! 4. **Merging.** Location along the wall and mounting height from the architectural sheet,
//!    circuit and panel stay electrical; every value records its source. Unpaired devices keep
//!    the electrical position (source `electrical diagrammatic`) and become findings, as do
//!    architectural devices with no circuit and walls whose counts differ.
//!
//! Precedence is the project rule `custom / device_location_precedence` (`architect`, the
//! default, or `electrical`: then nothing moves and the architectural spot is only recorded).
//! A device a person placed (`reviewed`, or its location's source `user`) never moves.
//!
//! No project-, sheet- or room-specific constants: every rule here is general.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{Drawing, Entity, EntityKind};
use serde_json::{Value, json};

use super::guided::GuidedJob;

/// Most architectural pages read for evidence in one job.
const MAX_PAGES: usize = 64;
/// Most objects looked at per page or sheet.
const MAX_OBJECTS: usize = 400_000;
/// Most devices on one wall (the alignment is quadratic).
const MAX_ON_WALL: usize = 64;
/// Most elevation views read per page.
const MAX_VIEWS: usize = 200;
/// Most findings listed in a report (the rest are counted).
const MAX_FINDINGS: usize = 200;
/// Most job findings raised (the report lists them all).
const MAX_JOB_FINDINGS: usize = 12;

/// What kind of device, for pairing an electrical symbol with an architectural one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Class {
    Light,
    Receptacle,
    Switch,
    Data,
    /// A mark that says nothing about its kind (`device type A`).
    Other,
}

impl Class {
    fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Receptacle => "receptacle",
            Self::Switch => "switch",
            Self::Data => "data",
            Self::Other => "device",
        }
    }
}

/// The class a device's kind (`switch`, `junction_box`, `luminaire`) or mark (`GFI`, `S3`,
/// `LM-12`) says, if either says one.
pub(crate) fn classify(mark: &str, kind: &str) -> Option<Class> {
    let k = kind.to_ascii_lowercase();
    if k.contains("switch") || k.contains("occupancy") || k.contains("dimmer") {
        return Some(Class::Switch);
    }
    if k.contains("low_voltage") || k.contains("low voltage") || k.contains("data") || k.contains("tele") {
        return Some(Class::Data);
    }
    if k.contains("receptacle") || k.contains("outlet") {
        return Some(Class::Receptacle);
    }
    if k.contains("junction") || k.contains("luminaire") || k.contains("light") || k.contains("fixture") || k.contains("sconce") {
        return Some(Class::Light);
    }
    let m = mark.trim().trim_end_matches('.').to_ascii_uppercase();
    let m = m.as_str();
    match m {
        "GFI" | "GFCI" | "WP" | "USB" | "DUP" | "DUPLEX" | "QUAD" | "RECEPT" | "RECEPTACLE" | "OUTLET" | "CONV" | "CONVENIENCE" => {
            Some(Class::Receptacle)
        }
        "S" | "S2" | "S3" | "S4" | "SD" | "SW" | "SWITCH" | "DIM" | "DIMMER" | "OS" | "VS" | "SK" => Some(Class::Switch),
        "DATA" | "D" | "TEL" | "TV" | "CATV" | "WAP" | "CAT6" | "VOICE" | "COAX" => Some(Class::Data),
        "J" | "JB" | "J-BOX" | "JBOX" | "JUNCTION" | "SCONCE" | "LIGHT" | "FIXTURE" | "VANITY" | "LTG" | "LUMINAIRE" | "PENDANT" | "DOWNLIGHT" => {
            Some(Class::Light)
        }
        _ if m.starts_with("RECEPT") || m.starts_with("OUTLET") => Some(Class::Receptacle),
        _ if m.starts_with("SWITCH") => Some(Class::Switch),
        _ if m.starts_with("SCONCE") || m.starts_with("LIGHT") || m.starts_with("FIXTURE") => Some(Class::Light),
        _ => None,
    }
}

/// A fixture type mark as fixture schedules write them: one to three letters, then optionally
/// a dash and a number (`A`, `F2`, `LM-12`, `XA`).
fn fixture_mark(t: &str) -> bool {
    let t = t.trim();
    let letters = t.chars().take_while(char::is_ascii_alphabetic).count();
    let rest = t.get(letters..).unwrap_or("");
    let rest = rest.strip_prefix('-').unwrap_or(rest);
    (1..=3).contains(&letters) && rest.len() <= 3 && rest.chars().all(|c| c.is_ascii_digit())
}

/// How well an electrical device (`e`) and an architectural one (`a`) can be the same device:
/// a cost in feet added to their distance, or `None` when they can't.
fn compat(e: (Class, &str), a: (Class, &str)) -> Option<f64> {
    if !e.1.is_empty() && e.1.eq_ignore_ascii_case(a.1) {
        return Some(0.0);
    }
    match (e.0, a.0) {
        (x, y) if x == y && x != Class::Other => Some(0.5),
        (Class::Other, Class::Light | Class::Other) | (Class::Light, Class::Other) => Some(1.5),
        (_, Class::Other) | (Class::Other, _) => Some(3.0),
        _ => None,
    }
}

// ─── geometry read off a sheet ──────────────────────────────────────────────────────────────

/// An axis-aligned stretch of linework: `at` is its y (horizontal) or x (vertical), `lo..hi`
/// its extent along the other axis.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Run {
    at: f64,
    lo: f64,
    hi: f64,
}

impl Run {
    fn len(&self) -> f64 {
        self.hi - self.lo
    }
}

/// A lettering item: upper-cased text, where it is and its height.
#[derive(Clone, Debug)]
struct Label {
    text: String,
    at: [f64; 2],
    height: f64,
}

/// A symbol: a block insert (its TAG value when it has one) or a small circle.
#[derive(Clone, Debug)]
struct Symbol {
    tag: String,
    at: [f64; 2],
}

/// What a sheet (or a part of one) draws: its straight linework split into horizontal and
/// vertical runs (sorted by `at`), its lettering and its symbols.
#[derive(Default)]
struct Sheet {
    h: Vec<Run>,
    v: Vec<Run>,
    labels: Vec<Label>,
    symbols: Vec<Symbol>,
    /// Every straight piece as drawn (leaders run at any angle), capped.
    segs: Vec<[[f64; 2]; 2]>,
}

/// Most straight pieces kept per sheet for leaders.
const MAX_SEGS: usize = 200_000;

fn inside(r: Option<[f64; 4]>, p: [f64; 2]) -> bool {
    r.is_none_or(|r| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3])
}

impl Sheet {
    /// Read `ents` (only what lies in `rect`, when given). `tol` is how far off level a line may
    /// be and still count as horizontal or vertical, as a slope.
    fn read<'a>(ents: impl Iterator<Item = &'a Entity>, rect: Option<[f64; 4]>) -> Self {
        let mut s = Sheet::default();
        let seg = |a: [f64; 2], b: [f64; 2], s: &mut Sheet| {
            if !inside(rect, a) || !inside(rect, b) {
                return;
            }
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            if !(dx.is_finite() && dy.is_finite()) {
                return;
            }
            if s.segs.len() < MAX_SEGS && (dx != 0.0 || dy != 0.0) {
                s.segs.push([a, b]);
            }
            if dy.abs() <= dx.abs() * 0.01 && dx.abs() > 0.0 {
                s.h.push(Run { at: (a[1] + b[1]) / 2.0, lo: a[0].min(b[0]), hi: a[0].max(b[0]) });
            } else if dx.abs() <= dy.abs() * 0.01 && dy.abs() > 0.0 {
                s.v.push(Run { at: (a[0] + b[0]) / 2.0, lo: a[1].min(b[1]), hi: a[1].max(b[1]) });
            }
        };
        // Capped by what is kept (a sheet's share of a whole set's drawing), not by what is seen.
        for e in ents {
            if s.h.len() + s.v.len() + s.labels.len() + s.symbols.len() >= MAX_OBJECTS {
                break;
            }
            match &e.kind {
                EntityKind::Line(l) => seg([l.a.x, l.a.y], [l.b.x, l.b.y], &mut s),
                EntityKind::LwPolyline(p) => {
                    let n = p.vertices.len();
                    let edges = if p.closed { n } else { n.saturating_sub(1) };
                    for i in 0..edges {
                        let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get((i + 1) % n.max(1))) else { continue };
                        if a.bulge.abs() < 1e-9 {
                            seg([a.p.x, a.p.y], [b.p.x, b.p.y], &mut s);
                        }
                    }
                }
                EntityKind::Text(t) => {
                    let at = t.justify_point();
                    if inside(rect, [at.x, at.y]) && !t.value.trim().is_empty() {
                        s.labels.push(Label { text: clean(&t.value), at: [at.x, at.y], height: t.height.abs().max(1e-9) });
                    }
                }
                EntityKind::MText(t) => {
                    let at = [t.insert.x, t.insert.y];
                    let v = cadaddy_fonts::plain_mtext(&t.contents);
                    if inside(rect, at) && !v.trim().is_empty() {
                        s.labels.push(Label { text: clean(&v), at, height: t.height.abs().max(1e-9) });
                    }
                }
                EntityKind::Insert(ins) => {
                    let at = [ins.insert.x, ins.insert.y];
                    if inside(rect, at) {
                        let tag = ins.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case("TAG")).map(|a| a.text.value.trim().to_ascii_uppercase());
                        s.symbols.push(Symbol { tag: tag.unwrap_or_default(), at });
                    }
                }
                EntityKind::Circle(c) => {
                    let at = [c.center.x, c.center.y];
                    if inside(rect, at) {
                        s.symbols.push(Symbol { tag: String::new(), at });
                    }
                }
                _ => {}
            }
        }
        s.h.retain(|r| r.len() > 0.0 && r.at.is_finite());
        s.v.retain(|r| r.len() > 0.0 && r.at.is_finite());
        s.h.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.lo.total_cmp(&b.lo)));
        s.v.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.lo.total_cmp(&b.lo)));
        s
    }

    /// Only the lettering of `ents`, sorted into `rects` (each label into the first that holds it),
    /// in one pass: the plans that are looked up but not measured.
    fn labels_in<'a>(ents: impl Iterator<Item = &'a Entity>, rects: &[Option<[f64; 4]>]) -> Vec<Vec<Label>> {
        let mut out: Vec<Vec<Label>> = vec![Vec::new(); rects.len()];
        let mut kept = 0usize;
        for e in ents {
            if kept >= MAX_OBJECTS {
                break;
            }
            let (text, at, height) = match &e.kind {
                EntityKind::Text(t) => {
                    let at = t.justify_point();
                    (t.value.clone(), [at.x, at.y], t.height)
                }
                EntityKind::MText(t) => (cadaddy_fonts::plain_mtext(&t.contents), [t.insert.x, t.insert.y], t.height),
                _ => continue,
            };
            if text.trim().is_empty() || !(at[0].is_finite() && at[1].is_finite()) {
                continue;
            }
            if let Some(v) = rects.iter().position(|r| inside(*r, at)).and_then(|i| out.get_mut(i)) {
                v.push(Label { text: clean(&text), at, height: height.abs().max(1e-9) });
                kept += 1;
            }
        }
        out
    }

    /// Join runs on one line that touch or nearly do (a PDF draws a line in pieces).
    fn merge(&mut self, tol: f64, gap: f64) {
        for runs in [&mut self.h, &mut self.v] {
            let mut out: Vec<Run> = Vec::with_capacity(runs.len());
            // Group by line (`at` within tol), then join along it.
            let mut i = 0;
            while i < runs.len() {
                let Some(first) = runs.get(i).copied() else { break };
                let mut j = i;
                while runs.get(j).is_some_and(|r| r.at - first.at <= tol) {
                    j += 1;
                }
                let mut group: Vec<Run> = runs.get(i..j).map(<[Run]>::to_vec).unwrap_or_default();
                group.sort_by(|a, b| a.lo.total_cmp(&b.lo));
                let mut cur: Option<Run> = None;
                for r in group {
                    cur = match cur {
                        Some(c) if r.lo <= c.hi + gap => Some(Run { at: c.at, lo: c.lo, hi: c.hi.max(r.hi) }),
                        Some(c) => {
                            out.push(c);
                            Some(r)
                        }
                        None => Some(r),
                    };
                }
                out.extend(cur);
                i = j.max(i + 1);
            }
            out.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.lo.total_cmp(&b.lo)));
            *runs = out;
        }
    }
}

/// Upper case, single spaces.
fn clean(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase()
}

/// A room number as plans print them (`101`, `101A`, `B102`).
fn room_number(t: &str) -> bool {
    super::perceive::geometry::room_number(t)
}

// ─── the room's walls on a plan ─────────────────────────────────────────────────────────────

/// Which wall of a room, by the way it faces from inside (north = plan up).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Side {
    North,
    South,
    East,
    West,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
        }
    }

    fn from_word(w: &str) -> Option<Self> {
        match w.trim_matches(|c: char| !c.is_ascii_alphabetic()) {
            "NORTH" | "NORTHERN" => Some(Self::North),
            "SOUTH" | "SOUTHERN" => Some(Self::South),
            "EAST" | "EASTERN" => Some(Self::East),
            "WEST" | "WESTERN" => Some(Self::West),
            _ => None,
        }
    }
}

/// One wall face of a room as an elevation draws it: seen from inside, `a` is its left end (the
/// elevation's left edge) and `b` its right; `inward` points into the room.
#[derive(Clone, Copy, Debug)]
struct Wall {
    side: Side,
    a: [f64; 2],
    b: [f64; 2],
    inward: [f64; 2],
}

impl Wall {
    fn len(&self) -> f64 {
        ((self.b[0] - self.a[0]).powi(2) + (self.b[1] - self.a[1]).powi(2)).sqrt()
    }

    /// Where `p` projects along the wall (0 at `a`, 1 at `b`) and how far it is from it.
    fn project(&self, p: [f64; 2]) -> (f64, f64) {
        let l = self.len().max(1e-9);
        let (ux, uy) = ((self.b[0] - self.a[0]) / l, (self.b[1] - self.a[1]) / l);
        let (px, py) = (p[0] - self.a[0], p[1] - self.a[1]);
        ((px * ux + py * uy) / l, (px * uy - py * ux).abs())
    }

    /// The point `t` of the way along the wall, `off` in front of its face.
    fn point(&self, t: f64, off: f64) -> [f64; 2] {
        [self.a[0] + (self.b[0] - self.a[0]) * t + self.inward[0] * off, self.a[1] + (self.b[1] - self.a[1]) * t + self.inward[1] * off]
    }
}

/// The four walls of a room box `[x0, y0, x1, y1]`.
fn walls_of(b: [f64; 4]) -> [Wall; 4] {
    let [x0, y0, x1, y1] = b;
    [
        Wall { side: Side::North, a: [x0, y1], b: [x1, y1], inward: [0.0, -1.0] },
        Wall { side: Side::South, a: [x1, y0], b: [x0, y0], inward: [0.0, 1.0] },
        Wall { side: Side::East, a: [x1, y1], b: [x1, y0], inward: [-1.0, 0.0] },
        Wall { side: Side::West, a: [x0, y0], b: [x0, y1], inward: [1.0, 0.0] },
    ]
}

/// The first wall face from `p` along one axis: `runs` are the lines across the ray (sorted by
/// `at`), `along` the ray's position across them, `up` its direction. A wall face has its
/// other face a wall thickness (`thick`) further on; failing any, the first line at least
/// `min_bare` long. `None` past `max`.
fn ray(runs: &[Run], p_at: f64, along: f64, up: bool, min_len: f64, thick: (f64, f64), min_bare: f64, max: f64) -> Option<f64> {
    let crosses = |r: &Run| r.lo <= along && r.hi >= along && r.len() >= min_len;
    let start = runs.partition_point(|r| r.at <= p_at);
    let candidates: Box<dyn Iterator<Item = &Run>> = if up {
        Box::new(runs.get(start..).unwrap_or(&[]).iter().take_while(|r| r.at - p_at <= max))
    } else {
        Box::new(runs.get(..start).unwrap_or(&[]).iter().rev().take_while(|r| p_at - r.at <= max))
    };
    let mut bare: Option<f64> = None;
    for r in candidates.filter(|r| crosses(r)) {
        // The face's partner: a line a wall thickness beyond, running along with it.
        let (lo, hi) = if up { (r.at + thick.0, r.at + thick.1) } else { (r.at - thick.1, r.at - thick.0) };
        let i = runs.partition_point(|q| q.at < lo);
        let partner = runs.get(i..).unwrap_or(&[]).iter().take_while(|q| q.at <= hi).any(|q| q.lo <= along + min_len && q.hi >= along - min_len);
        if partner {
            return Some(r.at);
        }
        if bare.is_none() && r.len() >= min_bare {
            bare = Some(r.at);
        }
    }
    bare
}

/// The room box `[x0, y0, x1, y1]` around a room tag at `p` on a plan drawn `upf` units to the
/// foot: the nearest wall face each way.
fn room_box(sheet: &Sheet, p: [f64; 2], upf: f64) -> Option<[f64; 4]> {
    let ft = upf;
    let (min_len, thick, bare, max) = (1.0 * ft, (0.15 * ft, 1.4 * ft), 4.0 * ft, 80.0 * ft);
    let n = ray(&sheet.h, p[1], p[0], true, min_len, thick, bare, max)?;
    let s = ray(&sheet.h, p[1], p[0], false, min_len, thick, bare, max)?;
    let e = ray(&sheet.v, p[0], p[1], true, min_len, thick, bare, max)?;
    let w = ray(&sheet.v, p[0], p[1], false, min_len, thick, bare, max)?;
    let (wd, ht) = ((e - w) / ft, (n - s) / ft);
    ((3.0..=200.0).contains(&wd) && (3.0..=200.0).contains(&ht)).then_some([w, s, e, n])
}

// ─── architectural evidence ─────────────────────────────────────────────────────────────────

/// A device an architectural sheet draws.
#[derive(Clone, Debug)]
pub(crate) struct ArchDevice {
    pub class: Class,
    /// Its mark (`LM-12`, `GFI`), upper case; empty for a device word or untagged symbol.
    pub mark: String,
    /// Where on the sheet.
    pub at: [f64; 2],
    /// A mounting-height callout next to it (`48" AFF`), feet.
    pub aff_ft: Option<f64>,
}

/// An interior / millwork elevation view.
#[derive(Clone, Debug)]
pub(crate) struct ElevView {
    pub title: String,
    /// Its view number (the `4` of `4/A-733`), when its bubble prints one.
    pub number: Option<String>,
    pub room_number: Option<String>,
    /// Words of the title that may be the room's name (`LOBBY`).
    pub room_words: Vec<String>,
    pub side: Option<Side>,
    /// The floor line's ends and the ceiling's height above it, sheet units.
    pub x0: f64,
    pub x1: f64,
    pub floor: f64,
    pub height: f64,
    pub devices: Vec<ArchDevice>,
}

impl ElevView {
    fn width(&self) -> f64 {
        self.x1 - self.x0
    }
}

/// A mounting height written as a callout: `48" AFF`, `+18"`, `7'-6" A.F.F.`, `AFF 44"`.
pub(crate) fn aff_height(t: &str) -> Option<f64> {
    let u = t.to_ascii_uppercase();
    let squashed: String = u.chars().filter(|c| *c != '.').collect();
    let aff = squashed.contains("AFF") || squashed.contains("ABOVE FIN") || squashed.contains("AFG");
    let plus = u.trim_start().starts_with('+');
    if !aff && !plus {
        return None;
    }
    // The first number with a foot or inch mark.
    let b = u.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b.get(i).is_some_and(u8::is_ascii_digit) {
            let start = i;
            while b.get(i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                i += 1;
            }
            let num: f64 = u.get(start..i)?.parse().ok()?;
            match b.get(i) {
                Some(b'\'') => {
                    // feet, then maybe -inches"
                    let rest = u.get(i + 1..).unwrap_or("");
                    let rest = rest.trim_start_matches(['-', ' ']);
                    let inches: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                    let inch = if rest.get(inches.len()..).is_some_and(|r| r.starts_with('"')) { inches.parse::<f64>().unwrap_or(0.0) } else { 0.0 };
                    let ft = num + inch / 12.0;
                    return (ft > 0.0 && ft < 40.0).then_some(ft);
                }
                Some(b'"') => {
                    let ft = num / 12.0;
                    return (ft > 0.0 && ft < 40.0).then_some(ft);
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// A dimension or other number-only lettering (`3'-6"`, `12"`, `1/2" = 1'-0"`).
fn is_measure(t: &str) -> bool {
    t.chars().any(|c| c.is_ascii_digit()) && (t.contains('\'') || t.contains('"') || t.contains('/'))
}

/// Words that make a title an exterior or building view, not a room's.
const NOT_ROOM_VIEW: &[&str] = &["EXTERIOR", "BUILDING ELEV", "BLDG ELEV", "SITE"];

/// Words of a view title (one found by its scale line) that make it a view other than a wall's
/// face: a plan, a cut, a schedule, an opening's detail.
const NOT_WALL_VIEW: &[&str] = &[
    "PLAN",
    "PLANS",
    "RCP",
    "CEILING",
    "SECTION",
    "SECTIONS",
    "SCHEDULE",
    "LEGEND",
    "ROOF",
    "FOUNDATION",
    "FOOTING",
    "DIAGRAM",
    "RISER",
    "HEAD",
    "JAMB",
    "SILL",
    "THRESHOLD",
];

/// The devices drawn in `sheet` within `bx` (`[x0, y0, x1, y1]`), and the height callouts. `h`:
/// a lettering height, the unit for "next to". `e_marks`: the marks the electrical plans use.
fn devices_in(sheet: &Sheet, bx: [f64; 4], h: f64, e_marks: &BTreeSet<String>, skip: &BTreeSet<usize>) -> Vec<ArchDevice> {
    let within = |p: [f64; 2]| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    let mut out: Vec<ArchDevice> = Vec::new();
    // Tagged symbols.
    for sy in sheet.symbols.iter().filter(|s| within(s.at) && !s.tag.is_empty()) {
        let class = classify(&sy.tag, super::readers::device_kind(&sy.tag))
            .or_else(|| (e_marks.contains(&sy.tag) || fixture_mark(&sy.tag)).then_some(Class::Other));
        if let Some(class) = class {
            let class = if class == Class::Other && fixture_mark(&sy.tag) { Class::Light } else { class };
            out.push(ArchDevice { class, mark: sy.tag.clone(), at: sy.at, aff_ft: None });
        }
    }
    // The untagged symbols lettering can point at.
    let anchors: Vec<[f64; 2]> = sheet.symbols.iter().filter(|s| within(s.at) && s.tag.is_empty()).map(|s| s.at).collect();
    let anchor = |p: [f64; 2], r: f64| -> Option<[f64; 2]> {
        anchors.iter().map(|a| (dist(*a, p), *a)).filter(|(d, _)| *d <= r).min_by(|x, y| x.0.total_cmp(&y.0)).map(|(_, a)| a)
    };
    let mut callouts: Vec<([f64; 2], f64)> = Vec::new();
    // Notes beside the view reach into it with leaders.
    let reach = (0.5 * (bx[2] - bx[0])).max(20.0 * h);
    let beside = |p: [f64; 2]| p[0] >= bx[0] - reach && p[0] <= bx[2] + reach && p[1] >= bx[1] - reach && p[1] <= bx[3] + reach;
    for (i, l) in sheet.labels.iter().enumerate() {
        if skip.contains(&i) || !beside(l.at) {
            continue;
        }
        if !within(l.at) {
            // Lettering outside the view names a device only through a leader into it.
            if let Some(class) = note_device(&l.text)
                && let Some(end) = leader_end(sheet, l)
            {
                let at = anchor(end, 3.0 * h).unwrap_or(end);
                if within(at) && !out.iter().any(|d| dist(d.at, at) <= 1.5 * h && compat((d.class, &d.mark), (class, "")).is_some()) {
                    out.push(ArchDevice { class, mark: String::new(), at, aff_ft: aff_height(&l.text) });
                }
            }
            continue;
        }
        if let Some(ft) = aff_height(&l.text) {
            // A note naming a device and its height, with a leader to it (`SCONCE @ 6'-0" AFF`).
            if let Some(class) = note_device(&l.text)
                && let Some(end) = leader_end(sheet, l)
            {
                let at = anchor(end, 3.0 * h).unwrap_or(end);
                if within(at) {
                    out.push(ArchDevice { class, mark: String::new(), at, aff_ft: Some(ft) });
                    continue;
                }
            }
            callouts.push((l.at, ft));
            continue;
        }
        if is_measure(&l.text) {
            continue;
        }
        let words: Vec<&str> = l.text.split(' ').collect();
        // A note ("PROVIDE BLOCKING FOR TV") is not a device: device words come alone or in two,
        // or as a note naming the device with a leader to it ("WALL SCONCE, SEE ELEC.").
        if words.len() > 3 {
            if let Some(class) = note_device(&l.text)
                && let Some(end) = leader_end(sheet, l)
            {
                let at = anchor(end, 3.0 * h).unwrap_or(end);
                if within(at) && !out.iter().any(|d| dist(d.at, at) <= 1.5 * h && compat((d.class, &d.mark), (class, "")).is_some()) {
                    out.push(ArchDevice { class, mark: String::new(), at, aff_ft: aff_height(&l.text) });
                }
            }
            continue;
        }
        let whole = l.text.as_str();
        let (class, mark) = if e_marks.contains(whole) && (whole.len() >= 2 || anchor(l.at, 2.5 * h).is_some()) {
            (
                classify(whole, super::readers::device_kind(whole)).unwrap_or(if fixture_mark(whole) { Class::Light } else { Class::Other }),
                whole.to_string(),
            )
        } else if let Some(c) = classify(whole, "").or_else(|| words.iter().find_map(|w| classify(w, ""))) {
            // A one-letter word (`S`, `J`, `D`) is a device only beside a symbol.
            if whole.len() < 2 && anchor(l.at, 2.5 * h).is_none() {
                continue;
            }
            (c, if whole.len() <= 6 { whole.to_string() } else { String::new() })
        } else {
            continue;
        };
        // Beside its symbol, else at the end of its leader, else where it is lettered.
        let at = anchor(l.at, 4.0 * h).or_else(|| leader_end(sheet, l).map(|end| anchor(end, 3.0 * h).unwrap_or(end))).unwrap_or(l.at);
        // The same device twice (a symbol and its tag): keep one.
        if out.iter().any(|d| dist(d.at, at) <= 1.5 * h && compat((d.class, &d.mark), (class, &mark)).is_some()) {
            continue;
        }
        out.push(ArchDevice { class, mark, at, aff_ft: None });
    }
    // Each callout to the nearest device it is next to.
    let mut used = vec![false; out.len()];
    for (at, ft) in callouts {
        let best = out
            .iter()
            .enumerate()
            .filter(|(i, _)| !used.get(*i).copied().unwrap_or(true))
            .map(|(i, d)| (i, dist(d.at, at)))
            .filter(|(_, d)| *d <= 10.0 * h)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = best {
            if let Some(d) = out.get_mut(i) {
                d.aff_ft = Some(ft);
            }
            if let Some(u) = used.get_mut(i) {
                *u = true;
            }
        }
    }
    out
}

/// Words that make a note about a device's support, not the device (`BLOCKING FOR SCONCE`).
const SUPPORT_WORDS: &[&str] = &["BLOCKING", "BACKING", "REINFORCING", "REINFORCEMENT", "STUD", "STUDS"];

/// The device a note names (`WALL SCONCE, SEE ELEC.`, `J-BOX @ 6'-0" AFF`): the first word of
/// three letters or more that names a device kind. Notes about the device's support, and long
/// notes, name none.
fn note_device(t: &str) -> Option<Class> {
    let words: Vec<&str> = t.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '(' | ')')).filter(|w| !w.is_empty()).collect();
    if words.len() > 10 || words.iter().any(|w| SUPPORT_WORDS.contains(&w.trim_matches('.'))) {
        return None;
    }
    words.iter().map(|w| w.trim_matches('.')).filter(|w| w.len() >= 3).find_map(|w| classify(w, ""))
}

/// Where the leader leaving lettering `l` ends: a straight piece starting at the lettering's
/// left or right end, followed through its bends (pieces meeting end to end, at most four).
/// `None` when no piece leaves the lettering.
fn leader_end(sheet: &Sheet, l: &Label) -> Option<[f64; 2]> {
    let h = l.height.max(1e-9);
    let wide = l.text.chars().count() as f64 * 0.8 * h;
    let ends = [[l.at[0] - 0.5 * h, l.at[1] + 0.4 * h], [l.at[0] + wide + 0.5 * h, l.at[1] + 0.4 * h]];
    let near_text =
        |p: [f64; 2]| p[0] >= l.at[0] - 1.5 * h && p[0] <= l.at[0] + wide + 1.5 * h && p[1] >= l.at[1] - 1.5 * h && p[1] <= l.at[1] + 2.0 * h;
    // The first piece: one end by the lettering's start or end, the other well away from it.
    let mut best: Option<(f64, usize, [f64; 2])> = None;
    for (i, sg) in sheet.segs.iter().enumerate() {
        for (near, far) in [(sg[0], sg[1]), (sg[1], sg[0])] {
            if near_text(far) || dist(near, far) < 2.0 * h {
                continue;
            }
            let d = ends.iter().map(|e| dist(*e, near)).fold(f64::INFINITY, f64::min);
            if d <= 1.2 * h && best.is_none_or(|b| d < b.0) {
                best = Some((d, i, far));
            }
        }
    }
    let (_, mut last, mut end) = best?;
    // Its bends: a piece of some length starting where this one ends, heading on.
    for _ in 0..4 {
        let next = sheet.segs.iter().enumerate().filter(|(i, _)| *i != last).find_map(|(i, sg)| {
            [(sg[0], sg[1]), (sg[1], sg[0])]
                .into_iter()
                .find(|(a, b)| dist(*a, end) <= 0.15 * h && dist(*a, *b) >= 2.0 * h && !near_text(*b))
                .map(|(_, b)| (i, b))
        });
        let Some((i, b)) = next else { break };
        last = i;
        end = b;
    }
    Some(end)
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// The elevation views a sheet draws: each titled view's floor line, wall ends and ceiling,
/// the room and wall its title names, and the devices inside it.
/// `elev_sheet`: the sheet's title says it holds elevations, so a view title there may name
/// only the room and wall (`ROOM 101 - NORTH`).
pub(crate) fn read_elevations(d: &Drawing, e_marks: &BTreeSet<String>, elev_sheet: bool) -> Vec<ElevView> {
    let mut sheet = Sheet::read(d.model.iter().map(|e| e.as_ref()), None);
    let names_view = |t: &str| {
        t.contains("ELEVATION")
            || t.contains("ELEV.")
            || (elev_sheet && t.split([' ', '-', ',', ':']).any(|w| Side::from_word(w).is_some() || room_number(w)))
    };
    let room_view = |t: &str| !NOT_ROOM_VIEW.iter().any(|w| t.contains(w));
    // Title → (it names an elevation, its view number when its bubble prints one).
    let mut titles: BTreeMap<usize, (bool, Option<String>)> = sheet
        .labels
        .iter()
        .enumerate()
        .filter(|(_, l)| names_view(&l.text) && room_view(&l.text))
        .map(|(i, _)| (i, (true, None)))
        .take(MAX_VIEWS)
        .collect();
    // Any view title with its scale under it: a wall detail need not say ELEVATION (then it must
    // show devices to count).
    let texts = cadaddy_mepfix::sheet_facts::texts(d);
    for vt in cadaddy_mepfix::sheet_facts::view_titles(d, &texts) {
        let t = clean(&cadaddy_fonts::plain_mtext(&vt.title));
        if !room_view(&t) || t.split(|c: char| !c.is_ascii_alphabetic()).any(|w| NOT_WALL_VIEW.contains(&w)) || titles.len() >= MAX_VIEWS {
            continue;
        }
        let found = sheet
            .labels
            .iter()
            .enumerate()
            .filter(|(_, l)| l.text == t || dist(l.at, vt.at) <= 0.5 * l.height)
            .min_by(|a, b| dist(a.1.at, vt.at).total_cmp(&dist(b.1.at, vt.at)))
            .filter(|(_, l)| dist(l.at, vt.at) <= 4.0 * l.height);
        if let Some((i, _)) = found {
            let e = titles.entry(i).or_insert((false, None));
            e.1 = vt.number.clone();
        }
    }
    if titles.is_empty() {
        return Vec::new();
    }
    // Lettering height: the titles' (the unit of every "next to" below).
    let mut hs: Vec<f64> = titles.keys().filter_map(|i| sheet.labels.get(*i)).map(|l| l.height).filter(|h| *h > 0.0).collect();
    hs.sort_by(f64::total_cmp);
    let h = hs.get(hs.len() / 2).copied().unwrap_or(1.0);
    sheet.merge(0.05 * h, 0.3 * h);
    let mut views: Vec<ElevView> = Vec::new();
    for (ti, (names_elevation, number)) in titles {
        let Some(title) = sheet.labels.get(ti).cloned() else { continue };
        let th = title.height.max(1e-9);
        // The view's number: its bubble's, else a number of up to three digits just left of it.
        let number = number.or_else(|| {
            sheet
                .labels
                .iter()
                .filter(|l| {
                    l.text.len() <= 3
                        && l.text.chars().all(|c| c.is_ascii_digit())
                        && l.at[0] < title.at[0]
                        && title.at[0] - l.at[0] <= 8.0 * th
                        && (l.at[1] - title.at[1]).abs() <= 2.5 * th
                })
                .min_by(|a, b| (title.at[0] - a.at[0]).total_cmp(&(title.at[0] - b.at[0])))
                .map(|l| l.text.clone())
        });
        let tw = (title.text.len() as f64 * 0.8 * th).max(4.0 * th);
        // The title's own lettering: the lines just above and below it, along it.
        let near: Vec<usize> = sheet
            .labels
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                (l.at[1] - title.at[1]).abs() <= 2.6 * th && l.at[0] >= title.at[0] - 12.0 * th && l.at[0] <= title.at[0] + tw + 4.0 * th
            })
            .map(|(i, _)| i)
            .collect();
        let words: Vec<String> = near
            .iter()
            .filter_map(|i| sheet.labels.get(*i))
            .flat_map(|l| l.text.split([' ', '-', ',', ':', '(', ')']).map(str::to_string).collect::<Vec<_>>())
            .collect();
        // The floor line: a long line just above the title, with walls rising from both ends.
        let span = (title.at[0] - 2.0 * th, title.at[0] + tw);
        let mut best: Option<(f64, Run, f64)> = None;
        for r in sheet
            .h
            .iter()
            .filter(|r| r.at > title.at[1] + 0.3 * th && r.at <= title.at[1] + 20.0 * th && r.len() >= 10.0 * th && r.lo <= span.1 && r.hi >= span.0)
        {
            let top = |x: f64| -> f64 {
                let tol = (0.01 * r.len()).max(0.3 * th);
                let mut ends: Vec<&Run> = sheet.v.iter().filter(|v| (v.at - x).abs() <= tol && v.hi > r.at + tol).collect();
                ends.sort_by(|a, b| a.lo.total_cmp(&b.lo));
                let mut reach = r.at;
                for v in ends {
                    if v.lo <= reach + 0.5 * th {
                        reach = reach.max(v.hi);
                    }
                }
                reach - r.at
            };
            let height = top(r.lo).min(top(r.hi));
            if height >= 0.2 * r.len() && height <= 3.0 * r.len() {
                // The lowest such line is the floor (a dimension line under it has no walls).
                if best.as_ref().is_none_or(|b| r.at < b.1.at) {
                    best = Some((r.at, *r, height));
                }
            }
        }
        let Some((floor, fl, height)) = best else { continue };
        let side = words.iter().find_map(|w| Side::from_word(w));
        let room_number = words.iter().find(|w| room_number(w)).cloned();
        let room_words: Vec<String> = words
            .iter()
            .filter(|w| w.len() >= 3 && w.chars().all(|c| c.is_ascii_alphabetic()) && Side::from_word(w).is_none())
            .filter(|w| {
                !matches!(
                    w.as_str(),
                    "ELEVATION"
                        | "ELEVATIONS"
                        | "INTERIOR"
                        | "ELEV"
                        | "SCALE"
                        | "WALL"
                        | "TYPICAL"
                        | "TYP"
                        | "VIEW"
                        | "ROOM"
                        | "MILLWORK"
                        | "CASEWORK"
                )
            })
            .cloned()
            .collect();
        let bx = [fl.lo - 0.02 * fl.len(), floor - 0.1 * th, fl.hi + 0.02 * fl.len(), floor + height + 0.1 * th];
        let skip: BTreeSet<usize> = near.iter().copied().collect();
        // A view's second title line finds the same view: one view, both lines' words.
        if let Some(seen) = views.iter_mut().find(|v| (v.floor - floor).abs() <= 0.05 * th && (v.x0 - fl.lo).abs() <= 0.05 * th) {
            seen.side = seen.side.or(side);
            if seen.room_number.is_none() {
                seen.room_number = room_number;
            }
            if seen.number.is_none() {
                seen.number = number;
            }
            for w in room_words {
                if !seen.room_words.contains(&w) {
                    seen.room_words.push(w);
                }
            }
            continue;
        }
        let devices = devices_in(&sheet, bx, th, e_marks, &skip);
        if !names_elevation && devices.is_empty() {
            continue;
        }
        views.push(ElevView { title: title.text.clone(), number, room_number, room_words, side, x0: fl.lo, x1: fl.hi, floor, height, devices });
    }
    views
}

/// The devices an architectural plan (RCP, enlarged plan) draws, and its room numbers.
fn read_plan_evidence(d: &Drawing, e_marks: &BTreeSet<String>) -> (Vec<ArchDevice>, Vec<Label>) {
    let sheet = Sheet::read(d.model.iter().map(|e| e.as_ref()), None);
    let mut hs: Vec<f64> = sheet.labels.iter().map(|l| l.height).filter(|h| *h > 0.0).collect();
    hs.sort_by(f64::total_cmp);
    let h = hs.get(hs.len() / 2).copied().unwrap_or(1.0);
    let all = [f64::MIN, f64::MIN, f64::MAX, f64::MAX];
    // On a plan only marks the electrical plans use and tagged symbols count: device words
    // there are notes.
    let devices: Vec<ArchDevice> = devices_in(&sheet, all, h, e_marks, &BTreeSet::new())
        .into_iter()
        .filter(|d| !d.mark.is_empty() && (e_marks.contains(&d.mark) || d.mark.len() >= 2))
        .collect();
    let rooms: Vec<Label> = sheet.labels.into_iter().filter(|l| room_number(&l.text)).collect();
    (devices, rooms)
}

// ─── electrical side ────────────────────────────────────────────────────────────────────────

/// An electrical device of the model.
#[derive(Clone, Debug)]
pub(crate) struct EDevice {
    /// Its index in the spec's tags.
    pub tag: usize,
    pub class: Class,
    pub mark: String,
    /// Where the E sheet draws it, drawing units.
    pub at: [f64; 2],
    pub sheet: String,
}

/// A plan the model's devices sit on: its number, level, where it is in the drawing and what it
/// draws.
struct EPlan {
    number: String,
    level: String,
    rect: Option<[f64; 4]>,
    sheet: Sheet,
}

/// A plan's references to views: (view number, sheet number as [`norm_sheet`] spells it) → where
/// the plan prints them. A reference is `4/A-733` (also inside a note, `SEE 4/A733`) or a bubble
/// printing the view number over the sheet number.
///
/// [`norm_sheet`]: cadaddy_mepfix::printlinks::norm_sheet
type RefIndex = BTreeMap<(String, String), Vec<[f64; 2]>>;

fn ref_index(labels: &[Label]) -> RefIndex {
    use cadaddy_mepfix::printlinks::{norm_sheet, text_refs};
    use cadaddy_mepfix::sheet_facts::is_sheet_number;
    let mut out: RefIndex = BTreeMap::new();
    let mut add = |k: (String, String), at: [f64; 2]| {
        let v = out.entry(k).or_default();
        if v.len() < MAX_ROOM_TAGS {
            v.push(at);
        }
    };
    let alnum = |w: &str| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.').trim_end_matches('.').to_string();
    for l in labels {
        let mut keys: BTreeSet<(String, String)> = BTreeSet::new();
        for (a, b) in l.text.split_whitespace().filter_map(|w| w.split_once('/')) {
            let (a, b) = (alnum(a), alnum(b));
            if (1..=3).contains(&a.len()) && is_sheet_number(&b) {
                keys.insert((a, norm_sheet(&b)));
            }
        }
        for (_, d, t) in text_refs(&l.text) {
            if !d.is_empty() {
                keys.insert((d, norm_sheet(&t)));
            }
        }
        for k in keys {
            add(k, l.at);
        }
    }
    // Bubbles: a short number with a sheet number right under it.
    let sheets: Vec<&Label> = labels.iter().filter(|m| is_sheet_number(&m.text)).take(20_000).collect();
    for l in labels.iter().filter(|l| (1..=3).contains(&l.text.len()) && l.text.chars().all(|c| c.is_ascii_alphanumeric())) {
        if let Some(under) = sheets.iter().find(|m| {
            let dy = l.at[1] - m.at[1];
            dy > 0.0 && dy <= 3.5 * l.height && (m.at[0] - l.at[0]).abs() <= 3.0 * l.height
        }) {
            add((l.text.clone(), norm_sheet(&under.text)), [(l.at[0] + under.at[0]) / 2.0, (l.at[1] + under.at[1]) / 2.0]);
        }
    }
    out
}

/// Most places a room's number or name is looked up at.
const MAX_ROOM_TAGS: usize = 16;

/// The room an elevation shows among the rooms around `tags` (where its number or name is
/// printed): one room, or the one with a wall (`side`'s, when the view says) closest to the
/// view's estimated width `est_len` (feet).
fn best_room(sheet: &Sheet, tags: &[[f64; 2]], side: Option<Side>, est_len: f64, upf: f64) -> Option<[f64; 4]> {
    let mut boxes: Vec<[f64; 4]> = Vec::new();
    for at in tags.iter().take(MAX_ROOM_TAGS) {
        if let Some(bx) = room_box(sheet, *at, upf)
            && !boxes.iter().any(|b| b.iter().zip(bx.iter()).all(|(x, y)| (x - y).abs() <= 0.25 * upf))
        {
            boxes.push(bx);
        }
    }
    if boxes.len() <= 1 {
        return boxes.first().copied();
    }
    let fit = |bx: &[f64; 4]| -> f64 {
        walls_of(*bx)
            .iter()
            .filter(|w| side.is_none_or(|s| w.side == s))
            .map(|w| (w.len() / upf / est_len.max(1e-9)).max(1e-9).ln().abs())
            .fold(f64::INFINITY, f64::min)
    };
    let mut scored: Vec<(f64, [f64; 4])> = boxes.iter().map(|b| (fit(b), *b)).filter(|(f, _)| f.is_finite()).collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    match scored.as_slice() {
        // Two rooms that fit alike: not told.
        [a, b, ..] if (b.0 - a.0).abs() < 0.05 => None,
        [a, ..] => Some(a.1),
        [] => None,
    }
}

/// Unique room numbers of `labels`: number → where.
fn unique_numbers(labels: &[Label]) -> BTreeMap<String, [f64; 2]> {
    let mut seen: BTreeMap<String, Option<[f64; 2]>> = BTreeMap::new();
    for l in labels.iter().filter(|l| room_number(&l.text)) {
        seen.entry(l.text.clone()).and_modify(|v| *v = None).or_insert(Some(l.at));
    }
    seen.into_iter().filter_map(|(k, v)| v.map(|p| (k, p))).collect()
}

/// Scale and offset taking `from` points onto `to` points (`to = s·from + t`), from pairs of
/// room numbers both print: the model with the most pairs within `tol`, fitted on them. Two
/// pairs always fit one, so they are enough only when there are no more to check them against:
/// `None` unless three agree (or the only two do).
fn register(pairs: &[([f64; 2], [f64; 2])], tol: f64) -> Option<(f64, [f64; 2], usize)> {
    let fit = |ps: &[&([f64; 2], [f64; 2])]| -> Option<(f64, [f64; 2])> {
        let n = ps.len() as f64;
        if ps.len() < 2 {
            return None;
        }
        let ca = [ps.iter().map(|p| p.0[0]).sum::<f64>() / n, ps.iter().map(|p| p.0[1]).sum::<f64>() / n];
        let cb = [ps.iter().map(|p| p.1[0]).sum::<f64>() / n, ps.iter().map(|p| p.1[1]).sum::<f64>() / n];
        let num: f64 = ps.iter().map(|p| (p.0[0] - ca[0]) * (p.1[0] - cb[0]) + (p.0[1] - ca[1]) * (p.1[1] - cb[1])).sum();
        let den: f64 = ps.iter().map(|p| (p.0[0] - ca[0]).powi(2) + (p.0[1] - ca[1]).powi(2)).sum();
        if den <= 0.0 || !num.is_finite() {
            return None;
        }
        let s = num / den;
        (s > 0.0 && s.is_finite()).then(|| (s, [cb[0] - s * ca[0], cb[1] - s * ca[1]]))
    };
    let apply = |m: (f64, [f64; 2]), p: [f64; 2]| [m.0 * p[0] + m.1[0], m.0 * p[1] + m.1[1]];
    let pairs = pairs.get(..pairs.len().min(60)).unwrap_or(&[]);
    let mut best: Option<(f64, [f64; 2], usize)> = None;
    for i in 0..pairs.len() {
        for j in i + 1..pairs.len() {
            let (Some(a), Some(b)) = (pairs.get(i), pairs.get(j)) else { continue };
            let Some(m) = fit(&[a, b]) else { continue };
            let inl: Vec<&([f64; 2], [f64; 2])> = pairs.iter().filter(|p| dist(apply(m, p.0), p.1) <= tol).collect();
            if inl.len() >= 2
                && best.is_none_or(|b| inl.len() > b.2)
                && let Some(m2) = fit(&inl)
            {
                best = Some((m2.0, m2.1, inl.len()));
            }
        }
    }
    best.filter(|b| b.2 >= 3 || pairs.len() == 2)
}

// ─── pairing ────────────────────────────────────────────────────────────────────────────────

/// Pair two lists in order along a wall: `e` and `a` are `(position_ft, (class, mark))`,
/// each sorted by position. Pairs are compatible and at most `tol` apart; skipping a device
/// costs `tol`. Returns the pairs `(i_e, i_a)`.
fn align(e: &[(f64, (Class, &str))], a: &[(f64, (Class, &str))], tol: f64) -> Vec<(usize, usize)> {
    let (n, m) = (e.len(), a.len());
    let w = m + 1;
    let idx = |i: usize, j: usize| i * w + j;
    let mut cost = vec![f64::INFINITY; (n + 1) * w];
    let mut from = vec![0u8; (n + 1) * w];
    if let Some(c) = cost.get_mut(0) {
        *c = 0.0;
    }
    for i in 0..=n {
        for j in 0..=m {
            let here = cost.get(idx(i, j)).copied().unwrap_or(f64::INFINITY);
            if !here.is_finite() {
                continue;
            }
            let relax = |k: usize, c: f64, how: u8, cost: &mut Vec<f64>, from: &mut Vec<u8>| {
                if let Some(slot) = cost.get_mut(k)
                    && c < *slot
                {
                    *slot = c;
                    if let Some(f) = from.get_mut(k) {
                        *f = how;
                    }
                }
            };
            if i < n {
                relax(idx(i + 1, j), here + tol, 1, &mut cost, &mut from);
            }
            if j < m {
                relax(idx(i, j + 1), here + tol, 2, &mut cost, &mut from);
            }
            if let (Some(ei), Some(aj)) = (e.get(i), a.get(j))
                && let Some(pen) = compat(ei.1, aj.1)
            {
                let d = (ei.0 - aj.0).abs();
                if d <= tol {
                    relax(idx(i + 1, j + 1), here + d + pen, 3, &mut cost, &mut from);
                }
            }
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        match from.get(idx(i, j)).copied().unwrap_or(0) {
            3 => {
                out.push((i - 1, j - 1));
                i -= 1;
                j -= 1;
            }
            1 => i -= 1,
            2 => j -= 1,
            _ => break,
        }
    }
    out.reverse();
    out
}

// ─── the run ────────────────────────────────────────────────────────────────────────────────

/// Settings: how far apart an E device and its architectural counterpart may be along a wall
/// (feet, at least; long walls allow a share of their length) or on a plan.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Opts {
    pub wall_tol_ft: f64,
    pub plan_tol_ft: f64,
    /// `false`: the electrical position wins (the project rule says so).
    pub architect_locates: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self { wall_tol_ft: 4.0, plan_tol_ft: 5.0, architect_locates: true }
    }
}

/// An architectural page read for evidence: its sheet number and title, and the drawing.
pub(crate) struct ArchPage {
    pub number: String,
    pub title: String,
    pub drawing: Drawing,
}

/// What a run found, for the report.
#[derive(Default)]
struct Run2 {
    moved: usize,
    recorded: usize,
    kept_electrical: usize,
    walls_read: usize,
    views_read: usize,
    views_placed: usize,
    plans_registered: usize,
    findings: Vec<Value>,
    finding_count: usize,
    per_view: Vec<Value>,
}

impl Run2 {
    fn finding(&mut self, kind: &str, v: Value) {
        self.finding_count += 1;
        if self.findings.len() < MAX_FINDINGS {
            let mut v = v;
            v["kind"] = json!(kind);
            self.findings.push(v);
        }
    }
}

/// Move the devices of `sp` onto their architectural spots. `d` is the drawing holding the
/// electrical plans; `e_sheets` each E plan's number, level and rectangle on it (none: the whole
/// drawing is one plan). `pages` are the architectural sheets read.
pub(crate) fn locate(
    sp: &mut cadaddy_walk3d::BuildingSpec,
    d: &Drawing,
    e_sheets: &[(String, String, Option<[f64; 4]>)],
    pages: &[ArchPage],
    opts: Opts,
) -> Value {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut run = Run2::default();
    // The model's electrical devices (their E-sheet spot: kept from an earlier run, else now).
    let mut devs: Vec<EDevice> = Vec::new();
    for (i, t) in sp.tags.iter().enumerate() {
        if !t.id.starts_with("device:") {
            continue;
        }
        let f = |k: &str| t.fields.get(k).map(|f| f.value.clone()).unwrap_or_default();
        let mark = f("type").trim().to_ascii_uppercase();
        let at_ft = parse_xy(&f("e_at_ft")).unwrap_or(t.at_ft);
        let class = classify(&mark, &t.kind).unwrap_or(Class::Other);
        devs.push(EDevice { tag: i, class, mark, at: [at_ft[0] * upf, at_ft[1] * upf], sheet: f("sheet") });
    }
    let e_marks: BTreeSet<String> = devs.iter().map(|d| d.mark.clone()).filter(|m| !m.is_empty()).collect();
    // A device is on the plan it sits on (an enlarged plan's devices may have been moved onto
    // their overall plan); a plan with no place in the drawing goes by the device's sheet.
    let holds = |number: &str, rect: Option<[f64; 4]>, d: &EDevice| match rect {
        Some(r) => inside(Some(r), d.at),
        None => number.is_empty() || d.sheet == number,
    };
    // The plans devices sit on, read once; every plan's lettering (for references to a view).
    let plans: Vec<EPlan> = if e_sheets.is_empty() {
        vec![EPlan { number: String::new(), level: String::new(), rect: None, sheet: Sheet::read(d.model.iter().map(|e| e.as_ref()), None) }]
    } else {
        e_sheets
            .iter()
            .filter(|(n, _, r)| devs.iter().any(|d| holds(n, *r, d)))
            .map(|(n, l, r)| EPlan { number: n.clone(), level: l.clone(), rect: *r, sheet: Sheet::read(d.model.iter().map(|e| e.as_ref()), *r) })
            .collect()
    };
    // Every plan: number, level, its references to views, its room numbers.
    let others: Vec<(String, String, RefIndex, BTreeMap<String, [f64; 2]>)> = if e_sheets.is_empty() {
        plans.iter().map(|p| (p.number.clone(), p.level.clone(), ref_index(&p.sheet.labels), unique_numbers(&p.sheet.labels))).collect()
    } else {
        let rects: Vec<Option<[f64; 4]>> = e_sheets.iter().map(|(_, _, r)| *r).collect();
        e_sheets
            .iter()
            .zip(Sheet::labels_in(d.model.iter().map(|e| e.as_ref()), &rects))
            .map(|((n, l, _), labels)| (n.clone(), l.clone(), ref_index(&labels), unique_numbers(&labels)))
            .collect()
    };
    let on_plan = |p: &EPlan, d: &EDevice| holds(&p.number, p.rect, d);
    // Plan `q`'s lettering mapped onto device plan `p` (scale, offset), by the room numbers
    // both print; computed once per pair.
    let mut fits: BTreeMap<(usize, usize), Option<(f64, [f64; 2])>> = BTreeMap::new();
    let ceiling_of = |level: &str| sp.levels.iter().find(|l| l.name == level).map_or(9.0, |l| l.ceiling_ft).clamp(6.0, 30.0);
    // Where each device goes: tag → (at, fields).
    let mut placed: BTreeMap<usize, Placement> = BTreeMap::new();
    // Devices on a wall some elevation shows: (plan, room) → walls shown.
    let mut shown: BTreeMap<(String, [i64; 4]), BTreeSet<Side>> = BTreeMap::new();
    let mut rooms_seen: BTreeMap<(String, [i64; 4]), ([f64; 4], String)> = BTreeMap::new();

    for page in pages {
        let elev_sheet = ["ELEVATION", "INTERIOR", "MILLWORK", "CASEWORK"].iter().any(|w| page.title.to_ascii_uppercase().contains(w));
        let views = read_elevations(&page.drawing, &e_marks, elev_sheet);
        run.views_read += views.len();
        for v in &views {
            let mut placed_view = false;
            for (pi, plan) in plans.iter().enumerate() {
                // Where the plans refer to this view (`4/A-733`), on this plan's sheet: on this
                // plan, and on the other plans of its level registered onto it by room numbers.
                let mut refs: Vec<[f64; 2]> = Vec::new();
                if let Some(num) = &v.number {
                    let key = (num.clone(), cadaddy_mepfix::printlinks::norm_sheet(&page.number));
                    for (qi, (qn, ql, qrefs, qnums)) in others.iter().enumerate() {
                        if !(ql == &plan.level || ql.is_empty() || plan.level.is_empty()) {
                            continue;
                        }
                        let Some(pts) = qrefs.get(&key) else { continue };
                        if qn == &plan.number {
                            refs.extend(pts.iter().copied());
                            continue;
                        }
                        let fit = *fits.entry((qi, pi)).or_insert_with(|| {
                            let mine = unique_numbers(&plan.sheet.labels);
                            let pairs: Vec<([f64; 2], [f64; 2])> = qnums.iter().filter_map(|(k, p)| mine.get(k).map(|e| (*p, *e))).collect();
                            register(&pairs, 2.0 * upf).map(|(s, t, _)| (s, t))
                        });
                        if let Some((s, t)) = fit {
                            refs.extend(pts.iter().map(|p| [s * p[0] + t[0], s * p[1] + t[1]]));
                        }
                    }
                    refs.truncate(MAX_ROOM_TAGS);
                }
                // The room on this plan: its number, else a name the plan prints once, else
                // where the plans refer to the view.
                let tag_at = match &v.room_number {
                    Some(n) => plan.sheet.labels.iter().filter(|l| &l.text == n).map(|l| l.at).collect::<Vec<_>>(),
                    None => {
                        let named: Vec<[f64; 2]> = plan
                            .sheet
                            .labels
                            .iter()
                            .filter(|l| !v.room_words.is_empty() && l.text.split(' ').any(|w| v.room_words.iter().any(|r| r == w)))
                            .map(|l| l.at)
                            .collect();
                        named
                    }
                };
                let tag_at = if tag_at.is_empty() { refs.clone() } else { tag_at };
                if tag_at.is_empty() {
                    continue;
                }
                let est_len = v.width() / v.height.max(1e-9) * ceiling_of(&plan.level);
                // A number printed more than once (a door numbered as its room, a keyed note):
                // the rooms its prints sit in, the one whose wall fits the view best.
                let Some(bx) = best_room(&plan.sheet, &tag_at, v.side, est_len, upf) else {
                    run.finding("room_walls_not_found", json!({"view": v.title, "sheet": page.number, "e_sheet": plan.number}));
                    continue;
                };
                let key = (plan.number.clone(), bx.map(|x| (x * 100.0).round() as i64));
                rooms_seen.insert(key.clone(), (bx, v.room_number.clone().unwrap_or_else(|| v.room_words.join(" "))));
                let in_room: Vec<&EDevice> = devs
                    .iter()
                    .filter(|d| on_plan(plan, d) && !placed.contains_key(&d.tag))
                    .filter(|d| {
                        d.at[0] >= bx[0] - 1.5 * upf && d.at[0] <= bx[2] + 1.5 * upf && d.at[1] >= bx[1] - 1.5 * upf && d.at[1] <= bx[3] + 1.5 * upf
                    })
                    .collect();
                let walls = walls_of(bx);
                // Each room device to its nearest wall, when it is near one.
                let near_wall = |d: &EDevice| -> Option<Side> {
                    walls
                        .iter()
                        .map(|w| (w.side, w.project(d.at).1, w))
                        .filter(|(_, off, w)| *off <= (4.0 * upf).min(0.35 * perp(&bx, w.side)))
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(s, _, _)| s)
                };
                // A reference printed by one wall (a callout on the wall it details) names it.
                let by_ref = refs
                    .iter()
                    .filter(|p| p[0] >= bx[0] - 2.0 * upf && p[0] <= bx[2] + 2.0 * upf && p[1] >= bx[1] - 2.0 * upf && p[1] <= bx[3] + 2.0 * upf)
                    .filter_map(|p| {
                        let mut ds: Vec<(f64, Side)> = walls.iter().map(|w| (w.project(*p).1, w.side)).collect();
                        ds.sort_by(|a, b| a.0.total_cmp(&b.0));
                        match ds.as_slice() {
                            [a, b, ..] if a.0 <= (3.0 * upf).min(0.25 * perp(&bx, a.1)) && b.0 >= 2.0 * a.0.max(0.5 * upf) => Some(a.1),
                            _ => None,
                        }
                    })
                    .next();
                let candidates: Vec<&Wall> = match v.side.or(by_ref) {
                    Some(s) => walls.iter().filter(|w| w.side == s).collect(),
                    None => {
                        let fit: Vec<&Wall> = walls.iter().filter(|w| (0.7..=1.43).contains(&(w.len() / upf / est_len.max(1e-9)))).collect();
                        // No wall has the view's proportions (a detail showing part of the height):
                        // every wall, its devices decide.
                        if fit.is_empty() { walls.iter().collect() } else { fit }
                    }
                };
                // Score each candidate wall by how its devices line up.
                let mut scored: Vec<(f64, &Wall, Vec<(usize, usize)>, Vec<&EDevice>, Vec<usize>)> = Vec::new();
                for w in candidates {
                    let l_ft = w.len() / upf;
                    let mut on: Vec<&EDevice> = in_room.iter().copied().filter(|d| near_wall(d) == Some(w.side)).take(MAX_ON_WALL).collect();
                    on.sort_by(|a, b| w.project(a.at).0.total_cmp(&w.project(b.at).0));
                    let mut ai: Vec<usize> = (0..v.devices.len().min(MAX_ON_WALL)).collect();
                    ai.sort_by(|a, b| {
                        let x = v.devices.get(*a).map_or(0.0, |d| d.at[0]);
                        let y = v.devices.get(*b).map_or(0.0, |d| d.at[0]);
                        x.total_cmp(&y)
                    });
                    let es: Vec<(f64, (Class, &str))> = on.iter().map(|d| (w.project(d.at).0 * l_ft, (d.class, d.mark.as_str()))).collect();
                    let as_: Vec<(f64, (Class, &str))> = ai
                        .iter()
                        .filter_map(|i| v.devices.get(*i))
                        .map(|d| ((d.at[0] - v.x0) / v.width().max(1e-9) * l_ft, (d.class, d.mark.as_str())))
                        .collect();
                    let tol = opts.wall_tol_ft.max(0.5 * l_ft).min(20.0);
                    let pairs = align(&es, &as_, tol);
                    let paired_cost: f64 = pairs.iter().filter_map(|(i, j)| Some((es.get(*i)?.0 - as_.get(*j)?.0).abs())).sum();
                    let unpaired = (es.len() + as_.len()).saturating_sub(2 * pairs.len()) as f64;
                    let cost = paired_cost + unpaired * tol;
                    scored.push((cost, w, pairs, on, ai));
                }
                scored.sort_by(|a, b| a.0.total_cmp(&b.0));
                let pick = match scored.as_slice() {
                    [] => None,
                    [one] => Some(one),
                    [first, second, ..] => {
                        // Two walls that fit as well (a room's two equal walls): the evidence
                        // can't tell which wall this is.
                        if v.side.or(by_ref).is_none() && !first.2.is_empty() && second.0 - first.0 < 0.3 * first.0.max(1.0) {
                            run.finding("wall_ambiguous", json!({"view": v.title, "sheet": page.number, "e_sheet": plan.number, "walls": [first.1.side.name(), second.1.side.name()]}));
                            None
                        } else {
                            Some(first)
                        }
                    }
                };
                let Some((_, wall, pairs, on, ai)) = pick else { continue };
                if v.side.or(by_ref).is_none() && pairs.is_empty() {
                    continue;
                }
                placed_view = true;
                run.walls_read += 1;
                shown.entry(key.clone()).or_default().insert(wall.side);
                let l_ft = wall.len() / upf;
                let source = format!("{} {}", page.number, v.title).trim().to_string();
                for (i, j) in pairs {
                    let (Some(e), Some(a)) = (on.get(*i), ai.get(*j).and_then(|j| v.devices.get(*j))) else { continue };
                    let t = ((a.at[0] - v.x0) / v.width().max(1e-9)).clamp(0.0, 1.0);
                    let height = a.aff_ft.unwrap_or((a.at[1] - v.floor) / v.width().max(1e-9) * l_ft);
                    let at = wall.point(t, 0.2 * upf);
                    placed.insert(
                        e.tag,
                        Placement {
                            at,
                            from: e.at,
                            source: source.clone(),
                            room: v.room_number.clone(),
                            wall: Some((wall.side, t * l_ft)),
                            height: Some(height).filter(|h| h.is_finite() && *h >= 0.0 && *h <= 40.0),
                            height_from: if a.aff_ft.is_some() { "callout" } else { "drawn" },
                            arch_mark: a.mark.clone(),
                        },
                    );
                }
                // What one side shows and the other doesn't.
                let paired_e: BTreeSet<usize> = pairs.iter().map(|p| p.0).collect();
                let paired_a: BTreeSet<usize> = pairs.iter().map(|p| p.1).collect();
                for (_, e) in on.iter().enumerate().filter(|(i, _)| !paired_e.contains(i)) {
                    run.finding(
                        "no_architectural_counterpart",
                        json!({"device": sp.tags.get(e.tag).map(|t| t.id.clone()), "mark": e.mark, "wall": wall.side.name(), "view": source, "e_sheet": plan.number}),
                    );
                }
                for j in (0..ai.len()).filter(|j| !paired_a.contains(j)) {
                    if let Some(a) = ai.get(j).and_then(|j| v.devices.get(*j)) {
                        let t = ((a.at[0] - v.x0) / v.width().max(1e-9)).clamp(0.0, 1.0);
                        run.finding(
                            "no_circuit_on_e_sheets",
                            json!({"mark": a.mark, "class": a.class.name(), "view": source, "wall": wall.side.name(), "along_wall_ft": r2(t * l_ft), "e_sheet": plan.number}),
                        );
                    }
                }
                if on.len() != ai.len() {
                    run.finding(
                        "count_differs",
                        json!({"view": source, "wall": wall.side.name(), "electrical": on.len(), "architectural": ai.len(), "e_sheet": plan.number}),
                    );
                }
                run.per_view.push(json!({"view": source, "e_sheet": plan.number, "wall": wall.side.name(), "wall_ft": r2(l_ft), "paired": pairs.len(), "electrical": on.len(), "architectural": ai.len()}));
            }
            if placed_view {
                run.views_placed += 1;
            }
        }
        // Plans: registered on each E plan by the room numbers both print.
        let (adevs, arooms) = if views.is_empty() { read_plan_evidence(&page.drawing, &e_marks) } else { (Vec::new(), Vec::new()) };
        if adevs.is_empty() {
            continue;
        }
        let anums = unique_numbers(&arooms);
        for plan in &plans {
            let enums = unique_numbers(&plan.sheet.labels);
            let pairs: Vec<([f64; 2], [f64; 2])> = anums.iter().filter_map(|(k, a)| enums.get(k).map(|e| (*a, *e))).collect();
            let Some((s, t, n)) = register(&pairs, 2.0 * upf) else { continue };
            run.plans_registered += 1;
            let map = |p: [f64; 2]| [s * p[0] + t[0], s * p[1] + t[1]];
            // The room each point is in: the room box of its nearest room number.
            let numbers: Vec<[f64; 2]> = enums.values().copied().collect();
            let box_of = |p: [f64; 2]| -> Option<[f64; 4]> {
                let mut near: Vec<&[f64; 2]> = numbers.iter().collect();
                near.sort_by(|a, b| dist(**a, p).total_cmp(&dist(**b, p)));
                near.into_iter()
                    .take(3)
                    .filter_map(|r| room_box(&plan.sheet, *r, upf))
                    .find(|b| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3])
            };
            let mut cand: Vec<(f64, usize, usize)> = Vec::new();
            let mapped: Vec<([f64; 2], Option<[f64; 4]>)> = adevs
                .iter()
                .map(|a| {
                    let m = map(a.at);
                    (m, box_of(m))
                })
                .collect();
            for (j, a) in adevs.iter().enumerate() {
                let Some((m, abox)) = mapped.get(j) else { continue };
                for (i, e) in devs.iter().enumerate().filter(|(_, e)| on_plan(plan, e) && !placed.contains_key(&e.tag)) {
                    let Some(pen) = compat((e.class, &e.mark), (a.class, &a.mark)) else { continue };
                    let dd = dist(e.at, *m) / upf;
                    if dd > opts.plan_tol_ft {
                        continue;
                    }
                    if let Some(b) = abox
                        && !(e.at[0] >= b[0] && e.at[0] <= b[2] && e.at[1] >= b[1] && e.at[1] <= b[3])
                    {
                        continue;
                    }
                    cand.push((dd + pen, i, j));
                }
            }
            cand.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (mut used_e, mut used_a) = (BTreeSet::new(), BTreeSet::new());
            for (_, i, j) in cand {
                if used_e.contains(&i) || used_a.contains(&j) {
                    continue;
                }
                let (Some(e), Some(a), Some((m, _))) = (devs.get(i), adevs.get(j), mapped.get(j)) else { continue };
                used_e.insert(i);
                used_a.insert(j);
                placed.insert(
                    e.tag,
                    Placement {
                        at: *m,
                        from: e.at,
                        source: format!("{} {}", page.number, page.title).trim().to_string(),
                        room: None,
                        wall: None,
                        height: a.aff_ft,
                        height_from: "callout",
                        arch_mark: a.mark.clone(),
                    },
                );
            }
            run.per_view.push(json!({"plan": page.number, "e_sheet": plan.number, "rooms_matched": n, "scale": s, "paired": used_a.len(), "architectural": adevs.len()}));
        }
    }

    // Walls of a room some elevation shows, whose devices no elevation does.
    for (key, sides) in &shown {
        let Some((bx, room)) = rooms_seen.get(key) else { continue };
        for w in walls_of(*bx).iter().filter(|w| !sides.contains(&w.side)) {
            let on: Vec<&EDevice> = devs
                .iter()
                .filter(|d| plans.iter().any(|p| p.number == key.0 && on_plan(p, d)) && !placed.contains_key(&d.tag))
                .filter(|d| d.at[0] >= bx[0] - upf && d.at[0] <= bx[2] + upf && d.at[1] >= bx[1] - upf && d.at[1] <= bx[3] + upf)
                .filter(|d| w.project(d.at).1 <= (2.0 * upf).min(0.25 * perp(bx, w.side)))
                .collect();
            if !on.is_empty() {
                run.finding("wall_not_shown", json!({"room": room, "wall": w.side.name(), "devices": on.len(), "e_sheet": key.0}));
            }
        }
    }

    // Merge: location and height architectural, circuit electrical; a person's placement stays.
    let mut kept: BTreeSet<usize> = BTreeSet::new();
    for (tag, p) in &placed {
        let Some(t) = sp.tags.get_mut(*tag) else { continue };
        let by_person = t.reviewed || t.fields.get("location_source").is_some_and(|f| f.source == "user");
        let arch = "architectural";
        let at_ft = [r2(p.at[0] / upf), r2(p.at[1] / upf)];
        t.set("arch_at_ft", format!("{},{}", at_ft[0], at_ft[1]), arch);
        t.set("located_from", p.source.clone(), arch);
        if let Some((side, along)) = p.wall {
            t.set("wall", side.name(), arch);
            t.set("along_wall_ft", format!("{}", r2(along)), arch);
        }
        if let Some(h) = p.height {
            t.set("mount_height_ft", format!("{}", r2(h)), if p.height_from == "callout" { "architectural callout" } else { arch });
        }
        if let Some(r) = &p.room
            && !t.fields.contains_key("room")
        {
            t.set("room", r.clone(), arch);
        }
        if !p.arch_mark.is_empty() && fixture_mark(&p.arch_mark) && classify(&p.arch_mark, "").is_none() {
            t.set("fixture_type", p.arch_mark.clone(), arch);
        }
        if by_person || !opts.architect_locates {
            run.recorded += 1;
            continue;
        }
        if !t.fields.contains_key("e_at_ft") {
            t.set("e_at_ft", format!("{},{}", r2(p.from[0] / upf), r2(p.from[1] / upf)), "electrical diagrammatic");
        }
        t.at_ft = at_ft;
        t.set("location_source", arch, arch);
        kept.insert(*tag);
        run.moved += 1;
    }
    // The rest keep the E spot, saying so.
    for d in &devs {
        if kept.contains(&d.tag) {
            continue;
        }
        if let Some(t) = sp.tags.get_mut(d.tag)
            && t.fields.get("location_source").is_none_or(|f| f.source != "user")
        {
            t.set("location_source", "electrical diagrammatic", "electrical diagrammatic");
            run.kept_electrical += 1;
        }
    }
    json!({
        "devices": devs.len(),
        "moved": run.moved,
        "recorded_not_moved": run.recorded,
        "kept_electrical": run.kept_electrical,
        "precedence": if opts.architect_locates { "architect locates, engineer circuits" } else { "electrical" },
        "pages_read": pages.len(),
        "elevation_views": run.views_read,
        "elevation_views_placed": run.views_placed,
        "walls_read": run.walls_read,
        "plans_registered": run.plans_registered,
        "views": run.per_view,
        "findings": run.findings,
        "finding_count": run.finding_count,
    })
}

/// Where one device goes and why.
struct Placement {
    at: [f64; 2],
    from: [f64; 2],
    source: String,
    room: Option<String>,
    wall: Option<(Side, f64)>,
    height: Option<f64>,
    height_from: &'static str,
    arch_mark: String,
}

/// The room's depth across a wall (its other dimension).
fn perp(bx: &[f64; 4], side: Side) -> f64 {
    match side {
        Side::North | Side::South => bx[3] - bx[1],
        Side::East | Side::West => bx[2] - bx[0],
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn parse_xy(s: &str) -> Option<[f64; 2]> {
    let (x, y) = s.split_once(',')?;
    let (x, y) = (x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?);
    (x.is_finite() && y.is_finite()).then_some([x, y])
}

/// The project rule: does the architect locate (default) or the engineer?
pub(crate) fn architect_locates(d: &Drawing) -> bool {
    d.project_rules
        .resolve(cadaddy_doc::projrules::RuleDomain::Custom, "device_location_precedence")
        .is_none_or(|r| !r.value.trim().to_ascii_lowercase().starts_with("elec"))
}

/// Whether a sheet's views can place devices, and how soon to read it (lower first): interior,
/// millwork and casework elevations; reflected ceiling and enlarged plans; detail and section
/// sheets a plan refers to (`referenced`, sheet numbers as `norm_sheet` spells them); other
/// detail and section sheets.
fn evidence_rank(sh: &Value, referenced: &BTreeSet<String>) -> Option<u8> {
    let s = |k: &str| sh.get(k).and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
    let (number, title, kind, disc) = (s("number"), s("title"), s("type"), s("discipline"));
    let arch = disc == "A" || disc == "I" || (disc.is_empty() && (number.starts_with('A') || number.starts_with('I')));
    if !arch || title.contains("EXTERIOR") || title.contains("BUILDING ELEV") {
        return None;
    }
    let has = |ws: &[&str]| ws.iter().any(|w| title.contains(w));
    if kind == "elevation" || has(&["INTERIOR", "ELEVATION", "CASEWORK", "MILLWORK"]) {
        Some(0)
    } else if matches!(kind.as_str(), "rcp" | "enlarged_plan") || has(&["REFLECTED", "RCP", "ENLARGED"]) {
        Some(1)
    } else if matches!(kind.as_str(), "detail" | "section") || has(&["DETAIL", "SECTION"]) {
        Some(if referenced.contains(&cadaddy_mepfix::printlinks::norm_sheet(&number)) { 2 } else { 3 })
    } else {
        None
    }
}

/// The sheet numbers lettered on the job's plans (`4/A-733`, `SEE A733`, a bubble's sheet), as
/// `norm_sheet` spells them.
fn referenced_sheets(d: &Drawing) -> BTreeSet<String> {
    use cadaddy_mepfix::sheet_facts::is_sheet_number;
    let mut out = BTreeSet::new();
    for e in d.model.iter().take(4 * MAX_OBJECTS) {
        let t = match &e.kind {
            EntityKind::Text(t) => t.value.clone(),
            EntityKind::MText(t) => cadaddy_fonts::plain_mtext(&t.contents),
            _ => continue,
        };
        for w in t.split(|c: char| c.is_whitespace() || matches!(c, '/' | ',' | ';' | '(' | ')')) {
            let w = w.trim_end_matches(['.', ':']);
            if is_sheet_number(w) && out.len() < 10_000 {
                out.insert(cadaddy_mepfix::printlinks::norm_sheet(w));
            }
        }
    }
    out
}

/// The job's architectural evidence pages, read from the prints (each page its own drawing,
/// so nothing is added to the job's drawing).
fn read_pages(job: &GuidedJob, referenced: &BTreeSet<String>) -> (Vec<ArchPage>, Vec<String>) {
    let Some(hooks) = super::file::io() else { return (Vec::new(), vec!["file formats are not available in this build".into()]) };
    let Some(read_pdf) = hooks.read_pdf else { return (Vec::new(), vec!["PDF import is not available in this build".into()]) };
    let sheets: Vec<Value> = job.sheets.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array).cloned().unwrap_or_default();
    let current = |sh: &Value| sh.get("superseded_by").and_then(Value::as_str).is_none_or(str::is_empty);
    let mut ranked: Vec<(u8, &Value)> =
        sheets.iter().filter(|sh| current(sh)).filter_map(|sh| evidence_rank(sh, referenced).map(|r| (r, sh))).collect();
    ranked.sort_by_key(|(r, _)| *r);
    let mut want: Vec<(String, String, String, u64)> = Vec::new();
    let mut seen = BTreeSet::new();
    for sh in ranked.into_iter().map(|(_, sh)| sh) {
        let file = sh.get("file").and_then(Value::as_str).unwrap_or_default();
        if file.is_empty() {
            continue;
        }
        let page = sh.get("page").and_then(Value::as_u64).filter(|p| *p >= 1).unwrap_or(1);
        if !seen.insert((file.to_string(), page)) {
            continue;
        }
        let path = if std::path::Path::new(file).is_absolute() { file.to_string() } else { format!("{}/{}", job.pdfs_path, file) };
        let s = |k: &str| sh.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        want.push((path, s("number"), s("title"), page));
        if want.len() >= MAX_PAGES {
            break;
        }
    }
    let mut files: BTreeMap<String, Option<std::sync::Arc<Vec<u8>>>> = BTreeMap::new();
    for (path, ..) in &want {
        files.entry(path.clone()).or_insert_with(|| crate::readfile::read(path).ok().map(std::sync::Arc::new));
    }
    let convert = |(path, number, title, page): &(String, String, String, u64)| -> Result<ArchPage, String> {
        let bytes = files.get(path).cloned().flatten().ok_or_else(|| format!("{path}: not read"))?;
        let (drawing, _) = read_pdf(&bytes, &json!({"page": page, "images": false, "fills": "merge"}))?;
        Ok(ArchPage { number: number.clone(), title: title.clone(), drawing })
    };
    #[cfg(not(target_arch = "wasm32"))]
    let results: Vec<Result<ArchPage, String>> = {
        use rayon::prelude::*;
        want.par_iter().map(convert).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let results: Vec<Result<ArchPage, String>> = want.iter().map(convert).collect();
    let mut pages = Vec::new();
    let mut errors = Vec::new();
    for r in results {
        match r {
            Ok(p) => pages.push(p),
            Err(e) => errors.push(e),
        }
    }
    (pages, errors)
}

/// The job's run: read the architectural evidence and move the model's devices onto it.
pub(crate) fn run_for_job(s: &mut crate::Session, job: &mut GuidedJob) -> Value {
    let referenced = s.doc().map(referenced_sheets).unwrap_or_default();
    let (pages, errors) = read_pages(job, &referenced);
    if pages.is_empty() {
        return json!({"pages_read": 0, "moved": 0, "why_not": "no architectural elevation, RCP, enlarged plan, detail or section sheets in the set", "errors": errors});
    }
    // Every plan of the job (electrical first): a device is on the one it sits on, and any plan
    // may print the references that place a view.
    let mut regions: Vec<&super::guided::SheetRegion> = job
        .sheet_regions
        .iter()
        .filter(|r| matches!(r.kind.as_str(), "plan" | "enlarged_plan" | "rcp" | "") && r.rect.iter().all(|v| v.is_finite()))
        .collect();
    regions.sort_by_key(|r| !r.discipline.eq_ignore_ascii_case("E"));
    let e_sheets: Vec<(String, String, Option<[f64; 4]>)> =
        regions.iter().map(|r| (r.number.clone(), r.level.clone(), Some(r.area.unwrap_or(r.rect)))).collect();
    let Ok(d) = s.doc() else { return json!({"moved": 0, "why_not": "no drawing"}) };
    let Some(text) = d.building_spec.as_deref().filter(|t| !t.trim().is_empty()) else { return json!({"moved": 0, "why_not": "no building model"}) };
    let Ok(mut sp) = cadaddy_walk3d::BuildingSpec::from_json(text) else { return json!({"moved": 0, "why_not": "the building model did not read"}) };
    let opts = Opts { architect_locates: architect_locates(d), ..Opts::default() };
    let mut report = locate(&mut sp, d, &e_sheets, &pages, opts);
    if let Ok(d) = s.doc_mut() {
        d.building_spec = Some(sp.to_json());
    }
    if !errors.is_empty() {
        report["errors"] = json!(errors);
    }
    report
}

/// Job findings for a run's report: one per kind, with how many and the first examples.
pub(crate) fn job_findings(report: &Value) -> Vec<(String, String)> {
    let mut by_kind: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for f in report.get("findings").and_then(Value::as_array).into_iter().flatten() {
        let k = f.get("kind").and_then(Value::as_str).unwrap_or("finding").to_string();
        by_kind.entry(k).or_default().push(f);
    }
    by_kind
        .into_iter()
        .take(MAX_JOB_FINDINGS)
        .map(|(k, fs)| {
            let what = match k.as_str() {
                "no_architectural_counterpart" => {
                    "electrical devices on a wall an elevation shows have no counterpart there (kept at the electrical position)"
                }
                "no_circuit_on_e_sheets" => "devices drawn on architectural elevations have no circuit on the E sheets",
                "count_differs" => "walls where the electrical and architectural device counts differ",
                "wall_not_shown" => "walls with devices that no elevation shows",
                "wall_ambiguous" => "elevations whose wall could not be told (two walls fit alike)",
                "room_walls_not_found" => "elevations whose room walls were not found on the electrical plan",
                _ => "device location findings",
            };
            let examples: Vec<String> = fs.iter().take(3).map(|f| f.to_string()).collect();
            (format!("Device locations: {} {what}", fs.len()), examples.join("; "))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Attrib, AttribModes, AttribProps, Common, HAlign, Insert, Line, Space, Text, VAlign};
    use cadaddy_geom::Vec3;

    fn text(v: &str, x: f64, y: f64, h: f64) -> Text {
        Text {
            insert: Vec3::new(x, y, 0.0),
            align_pt: None,
            height: h,
            value: v.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::default(),
            valign: VAlign::default(),
        }
    }

    fn add_line(d: &mut Drawing, a: [f64; 2], b: [f64; 2]) {
        d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) })).ok();
    }

    fn add_text(d: &mut Drawing, v: &str, at: [f64; 2], h: f64) {
        d.add(&Space::Model, Common::default(), EntityKind::Text(text(v, at[0], at[1], h))).ok();
    }

    fn add_symbol(d: &mut Drawing, tag: &str, at: [f64; 2]) {
        let attribs = if tag.is_empty() {
            Vec::new()
        } else {
            vec![Attrib {
                tag: "TAG".into(),
                text: text(tag, at[0], at[1], 2.0),
                invisible: false,
                constant: false,
                prompt: String::new(),
                props: AttribProps::default(),
                multiline: None,
                modes: AttribModes::default(),
            }]
        };
        let ins = Insert {
            block: "PDF-SYM-1".into(),
            insert: Vec3::new(at[0], at[1], 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs,
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        d.add(&Space::Model, Common::default(), EntityKind::Insert(ins)).ok();
    }

    /// A double-line wall rectangle: inner face `[x0, y0, x1, y1]`, 6" thick (units: inches).
    fn room(d: &mut Drawing, b: [f64; 4]) {
        let [x0, y0, x1, y1] = b;
        for o in [0.0, 6.0] {
            let (a, c) = ([x0 - o, y0 - o], [x1 + o, y1 + o]);
            add_line(d, a, [c[0], a[1]]);
            add_line(d, [c[0], a[1]], c);
            add_line(d, c, [a[0], c[1]]);
            add_line(d, [a[0], c[1]], a);
        }
    }

    /// A made-up E plan (inches): room 101 is 14' × 12' (168" × 144"), its tag in the middle;
    /// three junction boxes drawn unevenly along the north wall, two receptacles on the south.
    fn e_plan() -> Drawing {
        let mut d = Drawing::default();
        room(&mut d, [0.0, 0.0, 168.0, 144.0]);
        // A bed: furniture lines inside the room are not walls.
        add_line(&mut d, [40.0, 30.0], [120.0, 30.0]);
        add_line(&mut d, [40.0, 100.0], [120.0, 100.0]);
        add_text(&mut d, "101", [84.0, 72.0], 6.0);
        add_text(&mut d, "GUEST SUITE", [70.0, 62.0], 6.0);
        d
    }

    /// The spec with the E plan's devices: J at 20", 40", 150" along the north wall (they belong
    /// at 42", 84", 126"), receptacles on the south wall.
    fn spec() -> cadaddy_walk3d::BuildingSpec {
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        let mut add = |kind: &str, mark: &str, at: [f64; 2], circuit: &str| {
            let mut t = cadaddy_walk3d::tags::ElementTag {
                id: format!("device:L1:{:.1},{:.1}", at[0] / 12.0, at[1] / 12.0),
                kind: kind.into(),
                level: "Level 1".into(),
                at_ft: [at[0] / 12.0, at[1] / 12.0],
                ..Default::default()
            };
            t.set("type", mark, "plan");
            t.set("circuit", circuit, "plan");
            t.set("sheet", "E-1", "plan");
            t.set("symbol", "1A", "plan");
            sp.tags.push(t);
        };
        add("junction_box", "J", [20.0, 136.0], "LA-1");
        add("junction_box", "J", [40.0, 136.0], "LA-1");
        add("junction_box", "J", [150.0, 136.0], "LA-1");
        add("receptacle", "GFI", [30.0, 8.0], "LA-3");
        add("receptacle", "GFI", [140.0, 8.0], "LA-3");
        sp
    }

    /// A made-up interior elevation sheet (sheet units, any scale): the north wall of room 101
    /// drawn 168 wide and 108 high (9'), three sconces evenly at 42/84/126 at 6'-6", tagged.
    fn elevation(title: &str, devices: &[(&str, f64, f64)], extra: &[(&str, [f64; 2])]) -> Drawing {
        let mut d = Drawing::default();
        let (x0, y0) = (1000.0, 500.0);
        add_line(&mut d, [x0, y0], [x0 + 168.0, y0]);
        add_line(&mut d, [x0, y0], [x0, y0 + 108.0]);
        add_line(&mut d, [x0 + 168.0, y0], [x0 + 168.0, y0 + 108.0]);
        add_line(&mut d, [x0, y0 + 108.0], [x0 + 168.0, y0 + 108.0]);
        // A counter: a partial line, not the floor.
        add_line(&mut d, [x0 + 20.0, y0 + 34.0], [x0 + 80.0, y0 + 34.0]);
        // A dimension line under the floor: long, but no walls rise from it.
        add_line(&mut d, [x0, y0 - 10.0], [x0 + 168.0, y0 - 10.0]);
        add_text(&mut d, title, [x0, y0 - 24.0], 4.0);
        add_text(&mut d, "SCALE: 1/2\" = 1'-0\"", [x0, y0 - 31.0], 3.0);
        for (tag, x, y) in devices {
            add_symbol(&mut d, tag, [x0 + x, y0 + y]);
        }
        for (t, at) in extra {
            add_text(&mut d, t, [x0 + at[0], y0 + at[1]], 3.0);
        }
        d
    }

    fn page(d: Drawing) -> ArchPage {
        ArchPage { number: "A-501".into(), title: "INTERIOR ELEVATIONS".into(), drawing: d }
    }

    fn field(sp: &cadaddy_walk3d::BuildingSpec, i: usize, k: &str) -> String {
        sp.tags.get(i).and_then(|t| t.fields.get(k)).map(|f| f.value.clone()).unwrap_or_default()
    }

    fn e_sheets() -> Vec<(String, String, Option<[f64; 4]>)> {
        vec![("E-1".into(), "Level 1".into(), None)]
    }

    #[test]
    fn three_unevenly_drawn_boxes_go_to_the_architectural_spacing_and_height_on_their_circuit() {
        let plan = e_plan();
        let mut sp = spec();
        let elev = elevation("ROOM 101 - NORTH INTERIOR ELEVATION", &[("SCONCE", 42.0, 78.0), ("SCONCE", 84.0, 78.0), ("SCONCE", 126.0, 78.0)], &[]);
        let r = locate(&mut sp, &plan, &e_sheets(), &[page(elev)], Opts::default());
        assert_eq!(r["moved"], 3, "{r}");
        let xs: Vec<f64> = (0..3).map(|i| sp.tags[i].at_ft[0]).collect();
        assert_eq!(xs, vec![3.5, 7.0, 10.5], "{r}");
        for i in 0..3 {
            assert!((sp.tags[i].at_ft[1] - (144.0 - 2.4) / 12.0).abs() < 0.05, "on the north wall face: {:?}", sp.tags[i].at_ft);
            assert_eq!(field(&sp, i, "mount_height_ft"), "6.5");
            assert_eq!(field(&sp, i, "circuit"), "LA-1", "the circuit stays electrical");
            assert_eq!(field(&sp, i, "location_source"), "architectural");
            assert_eq!(sp.tags[i].fields["location_source"].source, "architectural");
            assert_eq!(field(&sp, i, "wall"), "north");
            assert_eq!(field(&sp, i, "room"), "101");
            assert!(field(&sp, i, "located_from").starts_with("A-501 ROOM 101"));
        }
        assert_eq!(field(&sp, 0, "e_at_ft"), "1.67,11.33", "the E spot is kept");
        // The receptacles' wall has no elevation: they keep their E spot and circuit, and say so.
        for i in 3..5 {
            assert_eq!(field(&sp, i, "location_source"), "electrical diagrammatic");
            assert_eq!(field(&sp, i, "circuit"), "LA-3");
        }
        assert_eq!(sp.tags[3].at_ft, [2.5, 8.0 / 12.0]);
        let kinds: Vec<&str> = r["findings"].as_array().unwrap().iter().filter_map(|f| f["kind"].as_str()).collect();
        assert!(kinds.contains(&"wall_not_shown"), "{r}");
    }

    #[test]
    fn a_room_number_printed_twice_finds_the_room_whose_wall_fits_the_view() {
        // 101 printed again in an 8' x 6' closet next door (a door mark, a keyed note).
        let mut plan = e_plan();
        room(&mut plan, [300.0, 0.0, 396.0, 72.0]);
        add_text(&mut plan, "101", [348.0, 36.0], 6.0);
        let mut sp = spec();
        let elev = elevation("ROOM 101 - NORTH INTERIOR ELEVATION", &[("SCONCE", 42.0, 78.0), ("SCONCE", 84.0, 78.0), ("SCONCE", 126.0, 78.0)], &[]);
        let r = locate(&mut sp, &plan, &e_sheets(), &[page(elev)], Opts::default());
        assert_eq!(r["moved"], 3, "{r}");
        let xs: Vec<f64> = (0..3).map(|i| sp.tags[i].at_ft[0]).collect();
        assert_eq!(xs, vec![3.5, 7.0, 10.5], "{r}");
        // Two rooms alike: not told.
        let sheet = Sheet::read(plan.model.iter().map(|e| e.as_ref()), None);
        assert_eq!(best_room(&sheet, &[[84.0, 72.0], [348.0, 36.0]], Some(Side::North), 14.0, 12.0), Some([0.0, 0.0, 168.0, 144.0]));
        assert_eq!(best_room(&sheet, &[[84.0, 72.0], [348.0, 36.0]], Some(Side::North), 8.0, 12.0), Some([300.0, 0.0, 396.0, 72.0]));
        let mut twin = e_plan();
        room(&mut twin, [300.0, 0.0, 468.0, 144.0]);
        let sheet = Sheet::read(twin.model.iter().map(|e| e.as_ref()), None);
        assert_eq!(best_room(&sheet, &[[84.0, 72.0], [384.0, 72.0]], None, 14.0, 12.0), None);
        assert_eq!(best_room(&sheet, &[], None, 14.0, 12.0), None);
    }

    #[test]
    fn receptacles_on_an_elevation_take_its_spots_and_heights_and_keep_their_circuits() {
        let plan = e_plan();
        let mut sp = spec();
        // The south wall seen from inside: its left end is the room's east end, so the
        // receptacle 24" from the elevation's left edge sits 24" from the east wall.
        let elev = elevation(
            "101 SOUTH ELEVATION",
            &[("GFI", 24.0, 18.0), ("GFI", 144.0, 44.0)],
            &[("18\" AFF", [28.0, 22.0]), ("44\" AFF", [148.0, 48.0])],
        );
        let r = locate(&mut sp, &plan, &e_sheets(), &[page(elev)], Opts::default());
        assert_eq!(r["moved"], 2, "{r}");
        // E receptacles at x=30" and 140" (west to east); seen from inside the west one is the
        // elevation's right one.
        assert_eq!(sp.tags[3].at_ft[0], 2.0, "west receptacle 24\" from the west end");
        assert_eq!(sp.tags[4].at_ft[0], 12.0, "east receptacle 24\" from the east end");
        assert_eq!(field(&sp, 3, "mount_height_ft"), "3.67");
        assert_eq!(field(&sp, 4, "mount_height_ft"), "1.5");
        assert_eq!(sp.tags[4].fields["mount_height_ft"].source, "architectural callout");
        assert_eq!(field(&sp, 3, "circuit"), "LA-3");
        assert_eq!(field(&sp, 3, "wall"), "south");
        // The junction boxes on the north wall stay where the E plan draws them.
        assert_eq!(field(&sp, 0, "location_source"), "electrical diagrammatic");
    }

    #[test]
    fn a_title_without_a_direction_finds_its_wall_by_length_and_devices() {
        let mut plan = e_plan();
        // Make the room 20' × 12' so the walls differ in length.
        plan = {
            let mut d = Drawing::default();
            room(&mut d, [0.0, 0.0, 240.0, 144.0]);
            add_text(&mut d, "101", [120.0, 72.0], 6.0);
            let _ = plan;
            d
        };
        let mut sp = spec();
        // Elevation 144 wide × 108 high: 1.33 × 9' = 12' — the east or west wall; the E plan
        // has two receptacles near the east wall (x = 236").
        sp.tags[3].at_ft = [236.0 / 12.0, 40.0 / 12.0];
        sp.tags[4].at_ft = [236.0 / 12.0, 100.0 / 12.0];
        let mut elev = Drawing::default();
        let (x0, y0) = (0.0, 0.0);
        add_line(&mut elev, [x0, y0], [x0 + 144.0, y0]);
        add_line(&mut elev, [x0, y0], [x0, y0 + 108.0]);
        add_line(&mut elev, [x0 + 144.0, y0], [x0 + 144.0, y0 + 108.0]);
        add_line(&mut elev, [x0, y0 + 108.0], [x0 + 144.0, y0 + 108.0]);
        add_text(&mut elev, "ROOM 101 INTERIOR ELEVATION", [x0, y0 - 20.0], 4.0);
        add_symbol(&mut elev, "GFI", [x0 + 30.0, y0 + 18.0]);
        add_symbol(&mut elev, "GFI", [x0 + 110.0, y0 + 18.0]);
        let r = locate(&mut sp, &plan, &e_sheets(), &[page(elev)], Opts::default());
        assert_eq!(r["moved"], 2, "{r}");
        assert_eq!(field(&sp, 3, "wall"), "east");
        // East wall seen from inside: left end is north (y=144"), so 30" from the left → y=114".
        assert_eq!(sp.tags[4].at_ft[1], 9.5, "{:?}", sp.tags[4].at_ft);
        assert_eq!(sp.tags[3].at_ft[1], 2.83, "{:?}", sp.tags[3].at_ft);
    }

    #[test]
    fn the_engineer_locates_when_the_project_says_so_and_a_persons_placement_stays() {
        let plan = e_plan();
        let mut sp = spec();
        sp.tags[1].reviewed = true;
        let elev = elevation("ROOM 101 - NORTH", &[("SCONCE", 42.0, 78.0), ("SCONCE", 84.0, 78.0), ("SCONCE", 126.0, 78.0)], &[]);
        let r = locate(&mut sp, &plan, &e_sheets(), &[page(elev)], Opts::default());
        assert_eq!(r["moved"], 2, "{r}");
        assert_eq!(sp.tags[1].at_ft, [40.0 / 12.0, 136.0 / 12.0], "a reviewed device keeps its spot");
        assert_eq!(field(&sp, 1, "arch_at_ft"), "7,11.8", "but the architectural spot is recorded");
        let mut sp = spec();
        let elev = elevation("ROOM 101 - NORTH", &[("SCONCE", 42.0, 78.0), ("SCONCE", 84.0, 78.0), ("SCONCE", 126.0, 78.0)], &[]);
        let r = locate(&mut sp, &plan, &e_sheets(), &[page(elev)], Opts { architect_locates: false, ..Opts::default() });
        assert_eq!(r["moved"], 0);
        assert_eq!(r["recorded_not_moved"], 3);
        assert_eq!(sp.tags[0].at_ft, [20.0 / 12.0, 136.0 / 12.0]);
        assert_eq!(field(&sp, 0, "mount_height_ft"), "6.5");
    }

    #[test]
    fn counts_that_differ_are_findings_and_the_extra_devices_say_where_they_are() {
        let plan = e_plan();
        let mut sp = spec();
        // Four sconces on the elevation, three boxes on the E plan.
        let elev = elevation("101 NORTH", &[("SCONCE", 20.0, 78.0), ("SCONCE", 60.0, 78.0), ("SCONCE", 100.0, 78.0), ("SCONCE", 150.0, 78.0)], &[]);
        let r = locate(&mut sp, &plan, &e_sheets(), &[page(elev)], Opts::default());
        assert_eq!(r["moved"], 3, "{r}");
        let f = r["findings"].as_array().unwrap();
        assert!(f.iter().any(|f| f["kind"] == "count_differs" && f["electrical"] == 3 && f["architectural"] == 4), "{r}");
        assert!(f.iter().any(|f| f["kind"] == "no_circuit_on_e_sheets"), "{r}");
        let jf = job_findings(&r);
        assert!(jf.iter().any(|(d, _)| d.contains("no circuit on the E sheets")), "{jf:?}");
    }

    #[test]
    fn incompatible_kinds_never_pair() {
        let plan = e_plan();
        let mut sp = spec();
        // Switches on the north wall: the junction boxes (lights) there must not take them.
        let elev = elevation("101 NORTH", &[("S", 42.0, 48.0), ("S3", 84.0, 48.0)], &[]);
        let r = locate(&mut sp, &plan, &e_sheets(), &[page(elev)], Opts::default());
        assert_eq!(r["moved"], 0, "{r}");
    }

    #[test]
    fn an_exterior_elevation_or_a_note_is_not_device_evidence() {
        let elev = elevation("NORTH EXTERIOR ELEVATION", &[("SCONCE", 42.0, 78.0)], &[]);
        assert!(read_elevations(&elev, &BTreeSet::new(), true).is_empty());
        let elev = elevation("101 NORTH", &[], &[("PROVIDE BLOCKING FOR TV BRACKET", [50.0, 60.0]), ("TV", [90.0, 60.0])]);
        let v = read_elevations(&elev, &BTreeSet::new(), true);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].devices.len(), 1, "{:?}", v[0].devices);
        assert_eq!(v[0].devices[0].class, Class::Data);
        assert_eq!(v[0].room_number.as_deref(), Some("101"));
        assert_eq!(v[0].side, Some(Side::North));
        assert!((v[0].floor - 500.0).abs() < 1e-9, "the dimension line under the floor is not the floor");
    }

    #[test]
    fn mounting_height_callouts_read() {
        assert_eq!(aff_height("48\" AFF"), Some(4.0));
        assert_eq!(aff_height("+18\""), Some(1.5));
        assert_eq!(aff_height("7'-6\" A.F.F."), Some(7.5));
        assert_eq!(aff_height("MTD @ 44\" AFF TO CENTER"), Some(44.0 / 12.0));
        assert_eq!(aff_height("3'-6\""), None, "a dimension is not a callout");
        assert_eq!(aff_height("AFF"), None);
    }

    #[test]
    fn an_rcp_registered_by_room_numbers_places_lights() {
        // E plan: rooms 101 and 102 side by side; lights drawn off-centre.
        let mut plan = Drawing::default();
        room(&mut plan, [0.0, 0.0, 168.0, 144.0]);
        room(&mut plan, [180.0, 0.0, 348.0, 144.0]);
        add_text(&mut plan, "101", [84.0, 72.0], 6.0);
        add_text(&mut plan, "102", [264.0, 72.0], 6.0);
        let mut sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        for (x, y) in [(60.0, 50.0), (250.0, 100.0)] {
            let mut t = cadaddy_walk3d::tags::ElementTag {
                id: format!("device:{x}"),
                kind: "device".into(),
                level: "Level 1".into(),
                at_ft: [x / 12.0, y / 12.0],
                ..Default::default()
            };
            t.set("type", "LM-2", "plan");
            t.set("circuit", "LA-5", "plan");
            t.set("sheet", "E-1", "plan");
            sp.tags.push(t);
        }
        // RCP at half scale, moved: room numbers at (2084, 1036), (2174, 1036).
        let mut rcp = Drawing::default();
        add_text(&mut rcp, "101", [2042.0 + 42.0, 1000.0 + 36.0], 3.0);
        add_text(&mut rcp, "102", [2042.0 + 132.0, 1000.0 + 36.0], 3.0);
        add_symbol(&mut rcp, "LM-2", [2042.0 + 42.0, 1000.0 + 36.0 + 10.0]);
        add_symbol(&mut rcp, "LM-2", [2042.0 + 132.0, 1000.0 + 36.0 + 10.0]);
        let r = locate(
            &mut sp,
            &plan,
            &e_sheets(),
            &[ArchPage { number: "A-201".into(), title: "REFLECTED CEILING PLAN".into(), drawing: rcp }],
            Opts::default(),
        );
        assert_eq!(r["moved"], 2, "{r}");
        assert_eq!(sp.tags[0].at_ft, [7.0, 7.67]);
        assert_eq!(sp.tags[1].at_ft, [22.0, 7.67]);
        assert_eq!(field(&sp, 0, "circuit"), "LA-5");
    }

    #[test]
    fn order_keeping_alignment_skips_what_does_not_fit() {
        let e = [(1.0, (Class::Light, "J")), (2.0, (Class::Light, "J")), (12.0, (Class::Light, "J"))];
        let a = [(3.5, (Class::Light, "")), (7.0, (Class::Light, "")), (10.5, (Class::Light, ""))];
        assert_eq!(align(&e, &a, 6.0), vec![(0, 0), (1, 1), (2, 2)]);
        // Too far apart: the near ones pair, the rest are skipped.
        assert_eq!(align(&e, &a, 4.0), vec![(1, 0), (2, 2)]);
        let a = [(1.2, (Class::Receptacle, "GFI"))];
        assert!(align(&e, &a, 4.0).is_empty());
    }

    #[test]
    fn wall_faces_beat_furniture() {
        let plan = e_plan();
        let sheet = Sheet::read(plan.model.iter().map(|e| e.as_ref()), None);
        assert_eq!(room_box(&sheet, [84.0, 72.0], 12.0), Some([0.0, 0.0, 168.0, 144.0]));
    }

    fn add_circle(d: &mut Drawing, at: [f64; 2], r: f64) {
        d.add(&Space::Model, Common::default(), EntityKind::Circle(cadaddy_doc::Circle { center: Vec3::new(at[0], at[1], 0.0), radius: r })).ok();
    }

    /// A made-up detail sheet: view 4, a wall drawn like `elevation`'s but titled with no room,
    /// wall or "ELEVATION"; three sconces drawn as circles at 42/84/126 (6'-6"), named only by
    /// notes beside the view with leaders to them, and a blocking note that is not a device.
    fn wall_detail() -> Drawing {
        let mut d = elevation("RECEPTION FEATURE WALL", &[], &[]);
        let (x0, y0) = (1000.0, 500.0);
        add_text(&mut d, "4", [x0 - 12.0, y0 - 24.0], 4.0);
        for (k, x) in [42.0, 84.0, 126.0].into_iter().enumerate() {
            add_circle(&mut d, [x0 + x, y0 + 78.0], 3.0);
            let ny = y0 + 95.0 - 8.0 * k as f64;
            add_text(&mut d, "WALL SCONCE, SEE ELECTRICAL", [x0 + 200.0, ny], 3.0);
            // The leader: from the note's start, bent once, ending on the sconce's edge.
            add_line(&mut d, [x0 + 199.0, ny + 1.0], [x0 + 180.0, ny + 1.0]);
            add_line(&mut d, [x0 + 180.0, ny + 1.0], [x0 + x + 2.0, y0 + 80.0]);
        }
        add_text(&mut d, "PROVIDE BLOCKING FOR SCONCES", [x0 + 200.0, y0 + 40.0], 3.0);
        add_line(&mut d, [x0 + 199.0, y0 + 41.0], [x0 + 100.0, y0 + 60.0]);
        d
    }

    fn details_page(d: Drawing) -> ArchPage {
        ArchPage { number: "A-733".into(), title: "INTERIOR DETAILS AND SECTIONS".into(), drawing: d }
    }

    #[test]
    fn a_wall_detail_titled_without_room_or_wall_is_placed_where_the_plan_refers_to_it() {
        // The detail view and its sconces are read: leader notes are devices, the blocking note
        // is not, and the view number comes from the title's bubble.
        let views = read_elevations(&wall_detail(), &BTreeSet::new(), false);
        assert_eq!(views.len(), 1, "{views:?}");
        assert_eq!(views[0].number.as_deref(), Some("4"));
        let mut xs: Vec<f64> = views[0].devices.iter().map(|d| d.at[0] - 1000.0).collect();
        xs.sort_by(f64::total_cmp);
        assert_eq!(xs, vec![42.0, 84.0, 126.0], "{views:?}");
        assert!(views[0].devices.iter().all(|d| d.class == Class::Light));

        // The plan prints `4/A-733` by the room's north wall: the view is that wall.
        let mut plan = e_plan();
        add_text(&mut plan, "4/A-733", [84.0, 132.0], 3.0);
        let mut sp = spec();
        let r = locate(&mut sp, &plan, &e_sheets(), &[details_page(wall_detail())], Opts::default());
        assert_eq!(r["moved"], 3, "{r}");
        let xs: Vec<f64> = (0..3).map(|i| sp.tags[i].at_ft[0]).collect();
        assert_eq!(xs, vec![3.5, 7.0, 10.5], "{r}");
        for i in 0..3 {
            assert_eq!(field(&sp, i, "wall"), "north");
            assert_eq!(field(&sp, i, "mount_height_ft"), "6.5");
            assert_eq!(field(&sp, i, "circuit"), "LA-1");
            assert!(field(&sp, i, "located_from").starts_with("A-733 RECEPTION FEATURE WALL"), "{}", field(&sp, i, "located_from"));
        }

        // No reference anywhere: the view names no room, so nothing moves.
        let mut sp = spec();
        let r = locate(&mut sp, &e_plan(), &e_sheets(), &[details_page(wall_detail())], Opts::default());
        assert_eq!(r["moved"], 0, "{r}");
    }

    #[test]
    fn a_reference_on_the_architectural_plan_places_devices_moved_onto_their_overall_plan() {
        // The drawing holds the E plan and, beside it, the architectural plan of the same level;
        // both print rooms 101 and 102. Only the architectural plan has the view's bubble
        // (4 over A-733). The devices came from an enlarged sheet (E-9) and were moved onto the
        // E plan: they are found by where they sit, not by the sheet they were read on.
        let mut d = e_plan();
        room(&mut d, [200.0, 0.0, 320.0, 144.0]);
        add_text(&mut d, "102", [260.0, 72.0], 6.0);
        for (x, y, n) in [(84.0, 72.0, "101"), (260.0, 72.0, "102")] {
            add_text(&mut d, n, [1000.0 + x, y], 6.0);
        }
        room(&mut d, [1000.0, 0.0, 1168.0, 144.0]);
        room(&mut d, [1200.0, 0.0, 1320.0, 144.0]);
        add_text(&mut d, "4", [1084.0, 134.0], 3.0);
        add_text(&mut d, "A-733", [1082.0, 128.0], 3.0);
        let mut sp = spec();
        for t in &mut sp.tags {
            t.set("sheet", "E-9", "plan");
        }
        let sheets = vec![
            ("E-1".to_string(), "Level 1".to_string(), Some([-20.0, -20.0, 400.0, 200.0])),
            ("A-1".to_string(), "Level 1".to_string(), Some([980.0, -20.0, 1400.0, 200.0])),
        ];
        let r = locate(&mut sp, &d, &sheets, &[details_page(wall_detail())], Opts::default());
        assert_eq!(r["moved"], 3, "{r}");
        let xs: Vec<f64> = (0..3).map(|i| sp.tags[i].at_ft[0]).collect();
        assert_eq!(xs, vec![3.5, 7.0, 10.5], "{r}");
        assert_eq!(field(&sp, 0, "wall"), "north");
        // A plan of another level is not looked at.
        let mut sp = spec();
        let other = vec![sheets[0].clone(), ("A-2".to_string(), "Level 2".to_string(), sheets[1].2)];
        let r = locate(&mut sp, &d, &other, &[details_page(wall_detail())], Opts::default());
        assert_eq!(r["moved"], 0, "{r}");
    }

    #[test]
    fn plan_references_to_views_read() {
        let l = |t: &str, x: f64, y: f64| Label { text: t.into(), at: [x, y], height: 3.0 };
        let idx = ref_index(&[
            l("SEE 3/A501 FOR LOCATIONS", 0.0, 0.0),
            l("7/A-612", 50.0, 0.0),
            l("2", 100.0, 10.0),
            l("A-410", 99.0, 4.0),
            l("2", 200.0, 10.0),
        ]);
        assert_eq!(idx.get(&("3".into(), "A501".into())), Some(&vec![[0.0, 0.0]]));
        assert_eq!(idx.get(&("7".into(), "A612".into())), Some(&vec![[50.0, 0.0]]));
        assert_eq!(idx.get(&("2".into(), "A410".into())), Some(&vec![[99.5, 7.0]]), "a bubble: number over sheet");
        assert_eq!(idx.len(), 3, "{idx:?}");
    }

    #[test]
    fn notes_name_devices_and_detail_sheets_are_read_after_elevations() {
        assert_eq!(note_device("WALL SCONCE, SEE ELECTRICAL"), Some(Class::Light));
        assert_eq!(note_device("J-BOX @ 6'-0\" AFF"), Some(Class::Light));
        assert_eq!(note_device("DUPLEX RECEPTACLE ABOVE COUNTER"), Some(Class::Receptacle));
        assert_eq!(note_device("PROVIDE BLOCKING FOR SCONCES"), None);
        assert_eq!(note_device("ALIGN WITH MIRROR"), None);
        let sh = |n: &str, t: &str, k: &str| json!({"number": n, "title": t, "type": k, "discipline": "A"});
        let refd: BTreeSet<String> = ["A733".to_string()].into();
        assert_eq!(evidence_rank(&sh("A-401", "INTERIOR ELEVATIONS", "elevation"), &refd), Some(0));
        assert_eq!(evidence_rank(&sh("A-201", "REFLECTED CEILING PLAN", "rcp"), &refd), Some(1));
        assert_eq!(evidence_rank(&sh("A-733", "DETAILS", "detail"), &refd), Some(2));
        assert_eq!(evidence_rank(&sh("A-530", "DETAILS", "detail"), &refd), Some(3));
        assert_eq!(evidence_rank(&sh("A-301", "EXTERIOR ELEVATIONS", "elevation"), &refd), None);
        assert_eq!(evidence_rank(&sh("A-101", "FLOOR PLAN", "plan"), &refd), None);
        // A titled view with no devices and no "ELEVATION" is no wall view.
        let bare = elevation("TYPICAL WALL TYPES", &[], &[]);
        assert!(read_elevations(&bare, &BTreeSet::new(), false).is_empty());
    }
}
