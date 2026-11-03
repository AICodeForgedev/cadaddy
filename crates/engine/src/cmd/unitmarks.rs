//! The marks a typical room's plans draw that the device reading left (Q6159): every symbol on a
//! unit type's enlarged plans goes into every room of the type, not only the ones the import made
//! symbol blocks of.
//!
//! A PDF import blocks a symbol only where it finds it drawn again and again on a page; a symbol
//! drawn once per enlarged view (a special purpose receptacle, a data outlet), drawn with its
//! leader or wall line touching it, or whose strokes the lettering reader took for letters stays
//! loose linework, and the device reading (which reads symbol blocks and the designations printed
//! by switches) never sees it. In each typical-room view this reads:
//!
//! - the legend's symbols drawn as loose linework: each legend row's drawing ([`Template`]) looked
//!   for in the view's linework, turned any quarter, a little larger or smaller, other linework
//!   touching it or not ([`find_symbols`]); the most detailed row wins where two rows' drawings
//!   are found at one place (a quad receptacle holds a duplex one). Its kind is the row's;
//! - the circuit labels no read device took: each a device the label names, a luminaire of the
//!   fixture mark printed over it (a mark the fixture schedule or the submittals list), else a
//!   luminaire when the legend says a lower-case letter after a circuit is a fixture's switch
//!   leg, else a receptacle (a symbol only its circuit labels is a receptacle, as the device
//!   reading takes it);
//! - the switch designations no read switch took, one switch for each designation a label
//!   names (`3,D,a`: a 3-way and a dimmer on leg a).
//!
//! The marks found are new read devices (their sheet point, the view's sheet and level), so the
//! stamping puts them into every room of the type and Validate expects them there. This module
//! holds the reading; the job gives it the views, their lettering and linework, the devices
//! already read and the legend's rows.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::EntityKind;
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;

use super::readers::ReadDevice;

/// A symbol's drawn piece: a straight piece (its two ends), or a circle or arc (its centre and
/// radius).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Piece {
    Seg(Vec2, Vec2),
    Round(Vec2, f64),
}

/// Corners of one polyline looked at, at most.
const MAX_CORNERS: usize = 256;

/// The pieces an object draws into `out`: a line, a circle, an arc (as its circle), a polyline's
/// sides. Anything else draws none.
pub(crate) fn pieces(kind: &EntityKind, out: &mut Vec<Piece>) {
    match kind {
        EntityKind::Line(l) => out.push(Piece::Seg(Vec2::new(l.a.x, l.a.y), Vec2::new(l.b.x, l.b.y))),
        EntityKind::Circle(c) => out.push(Piece::Round(Vec2::new(c.center.x, c.center.y), c.radius)),
        EntityKind::Arc(a) => out.push(Piece::Round(Vec2::new(a.center.x, a.center.y), a.radius)),
        EntityKind::LwPolyline(pl) => {
            let pts: Vec<Vec2> = pl.vertices.iter().take(MAX_CORNERS).map(|v| Vec2::new(v.p.x, v.p.y)).collect();
            for w in pts.windows(2) {
                if let [a, b] = w {
                    out.push(Piece::Seg(*a, *b));
                }
            }
            if pl.closed
                && pts.len() > 2
                && let (Some(a), Some(b)) = (pts.last(), pts.first())
            {
                out.push(Piece::Seg(*a, *b));
            }
        }
        _ => {}
    }
}

/// A piece's box: `[x0, y0, x1, y1]`.
fn piece_box(p: &Piece) -> [f64; 4] {
    match *p {
        Piece::Seg(a, b) => [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)],
        Piece::Round(c, r) => [c.x - r, c.y - r, c.x + r, c.y + r],
    }
}

/// Pieces that draw nothing to look for: a straight piece of no length, a circle of no size, a
/// point far off.
fn usable(p: &Piece) -> bool {
    match *p {
        Piece::Seg(a, b) => a.is_finite() && b.is_finite() && a.dist(b) > 1e-9,
        Piece::Round(c, r) => c.is_finite() && r.is_finite() && r > 1e-9,
    }
}

/// A legend row's symbol drawing, to look for on the plans: the row's words, its sheet, the kind
/// and IFC class the row names, its pieces about the middle of their box, and the height of the
/// row's lettering (a legend and a plan letter their notes the same size on paper, so that
/// height gives the size a symbol is drawn at on a plan whatever units each page was read in).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Template {
    pub row: String,
    pub sheet: String,
    pub kind: String,
    pub ifc: String,
    pieces: Vec<Piece>,
    /// Its box's larger side.
    size: f64,
    text_h: f64,
    /// Whether its row is a low voltage device's (on no power circuit: no circuit label is its).
    pub low_voltage: bool,
}

/// A legend symbol of fewer pieces than this is no drawing to look for (a lone circle is every
/// circle); one of more is lettering or a table's lines, not a symbol.
const MIN_TEMPLATE_PIECES: usize = 3;
/// A piece this near its box's edge (a share of the box) lies on it.
const BOX_EDGE_SHARE: f64 = 0.03;
const MAX_TEMPLATE_PIECES: usize = 64;

impl Template {
    /// The row's symbol from what its cell draws and its lettering's height (in the legend sheet's
    /// units). None for too few or too many pieces, or a drawing or lettering of no size.
    pub(crate) fn new(row: &str, sheet: &str, kind: &str, ifc: &str, drawn: &[Piece], text_h: f64) -> Option<Template> {
        if !(text_h.is_finite() && text_h > 0.0) {
            return None;
        }
        let drawn: Vec<Piece> = drawn.iter().filter(|p| usable(p)).copied().collect();
        if !(MIN_TEMPLATE_PIECES..=MAX_TEMPLATE_PIECES).contains(&drawn.len()) {
            return None;
        }
        let b = drawn.iter().map(piece_box).fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |a, q| {
            [a[0].min(q[0]), a[1].min(q[1]), a[2].max(q[2]), a[3].max(q[3])]
        });
        let size = (b[2] - b[0]).max(b[3] - b[1]);
        if !(size.is_finite() && size > 0.0) {
            return None;
        }
        // A plain box (every piece on the edges of its box) tells nothing apart: a keynote's, a
        // tag's and a cabinet's box are drawn the same, and the row's letters in it are lettering,
        // not linework.
        let eps = BOX_EDGE_SHARE * size;
        let on_edge = |p: Vec2| (p.x - b[0]).abs() <= eps || (p.x - b[2]).abs() <= eps || (p.y - b[1]).abs() <= eps || (p.y - b[3]).abs() <= eps;
        let plain = drawn.iter().all(|p| match *p {
            Piece::Seg(a, c) => {
                let m = (a + c) * 0.5;
                on_edge(a) && on_edge(c) && on_edge(m) && ((a.x - c.x).abs() <= eps || (a.y - c.y).abs() <= eps)
            }
            Piece::Round(..) => false,
        });
        if plain {
            return None;
        }
        let mid = Vec2::new((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        let pieces = drawn
            .iter()
            .map(|p| match *p {
                Piece::Seg(a, b) => Piece::Seg(a - mid, b - mid),
                Piece::Round(c, r) => Piece::Round(c - mid, r),
            })
            .collect();
        Some(Template {
            row: row.trim().to_string(),
            sheet: sheet.to_string(),
            kind: kind.to_string(),
            ifc: ifc.to_string(),
            pieces,
            size,
            text_h,
            low_voltage: false,
        })
    }
}

/// A view's linework, by cells, to look the legend's symbols up in.
pub(crate) struct Scene {
    pieces: Vec<Piece>,
    cell: f64,
    grid: HashMap<(i64, i64), Vec<usize>>,
}

/// Pieces of one view looked at, at most; and the cells one piece is filed under, at most (a
/// long wall line is found from its ends' cells and those between).
const MAX_SCENE_PIECES: usize = 200_000;
const MAX_PIECE_CELLS: i64 = 400;

impl Scene {
    /// The pieces, filed by cells of `cell` (a symbol's size or so).
    pub(crate) fn new(pieces: Vec<Piece>, cell: f64) -> Scene {
        let cell = if cell.is_finite() && cell > 0.0 { cell } else { 1.0 };
        let pieces: Vec<Piece> = pieces.into_iter().filter(usable).take(MAX_SCENE_PIECES).collect();
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, p) in pieces.iter().enumerate() {
            let b = piece_box(p);
            let (x0, y0, x1, y1) =
                ((b[0] / cell).floor() as i64, (b[1] / cell).floor() as i64, (b[2] / cell).floor() as i64, (b[3] / cell).floor() as i64);
            if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > MAX_PIECE_CELLS {
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    grid.entry((x, y)).or_default().push(i);
                }
            }
        }
        Scene { pieces, cell, grid }
    }

    /// The pieces filed within `r` of `p` (each once).
    fn near(&self, p: Vec2, r: f64) -> Vec<&Piece> {
        let (x0, y0, x1, y1) = (
            ((p.x - r) / self.cell).floor() as i64,
            ((p.y - r) / self.cell).floor() as i64,
            ((p.x + r) / self.cell).floor() as i64,
            ((p.y + r) / self.cell).floor() as i64,
        );
        if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > MAX_PIECE_CELLS {
            return Vec::new();
        }
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.grid.get(&(x, y)) {
                    seen.extend(v.iter().copied());
                }
            }
        }
        seen.into_iter().filter_map(|i| self.pieces.get(i)).collect()
    }
}

/// A legend symbol found drawn in a view: where its middle is, which template, its size there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Found {
    pub at: Vec2,
    pub template: usize,
    pub size: f64,
}

/// How much larger or smaller than its legend drawing a symbol may be drawn.
const MIN_SCALE: f64 = 0.7;
const MAX_SCALE: f64 = 1.45;
/// How far off its place a piece of a found symbol may be, as a share of the symbol's size.
const PIECE_TOL: f64 = 0.08;
/// Two straight pieces this far from parallel (radians) aren't the same.
const PARALLEL_TOL: f64 = 0.12;
/// Candidate places looked at per template in one view, at most.
const MAX_ANCHORS: usize = 20_000;

/// `v` turned `k` quarters counter-clockwise.
fn quarter(v: Vec2, k: u8) -> Vec2 {
    match k % 4 {
        1 => Vec2::new(-v.y, v.x),
        2 => Vec2::new(-v.x, -v.y),
        3 => Vec2::new(v.y, -v.x),
        _ => v,
    }
}

/// The distance from `p` to the straight piece `a`–`b`.
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.len2();
    if l2 <= 0.0 {
        return p.dist(a);
    }
    let u = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(a + ab * u)
}

/// Whether two directions are parallel either way.
fn parallel(u: Vec2, v: Vec2) -> bool {
    let (lu, lv) = (u.len(), v.len());
    lu > 0.0 && lv > 0.0 && (u.cross(v) / (lu * lv)).abs() <= PARALLEL_TOL.sin()
}

/// Whether the template, turned `k`, scaled `s` and its middle put at `t`, is all drawn in the
/// scene: each circle by a circle of its size there, each straight piece by a parallel straight
/// piece through both its ends and its middle.
fn drawn_at(tp: &Template, scene: &Scene, k: u8, s: f64, t: Vec2) -> bool {
    let tol = PIECE_TOL * tp.size * s;
    tp.pieces.iter().all(|p| match *p {
        Piece::Round(c, r) => {
            let c = t + quarter(c, k) * s;
            scene.near(c, tol).into_iter().any(|q| matches!(*q, Piece::Round(qc, qr) if qc.dist(c) <= tol && (qr - r * s).abs() <= tol))
        }
        Piece::Seg(a, b) => {
            let (a, b) = (t + quarter(a, k) * s, t + quarter(b, k) * s);
            let m = (a + b) * 0.5;
            let on = |p: Vec2| {
                scene.near(p, tol).into_iter().any(|q| matches!(*q, Piece::Seg(qa, qb) if parallel(qb - qa, b - a) && seg_dist(p, qa, qb) <= tol))
            };
            on(a) && on(b) && on(m)
        }
    })
}

/// Every place in the scene's `area` (`[x0, y0, x1, y1]`, lettered `text_h` high) where one of
/// the templates is drawn: the template's largest circle (else its longest straight piece) laid
/// on each circle (straight piece) of the scene that size or so, each quarter turn, then the rest
/// checked. Where two templates are found at one place the one of more pieces stays (the more
/// detailed symbol).
pub(crate) fn find_symbols(templates: &[Template], scene: &Scene, area: [f64; 4], text_h: f64) -> Vec<Found> {
    let inside = |p: Vec2| p.x >= area[0] && p.x <= area[2] && p.y >= area[1] && p.y <= area[3];
    let mut found: Vec<(Found, usize, f64)> = Vec::new();
    for (ti, tp) in templates.iter().enumerate() {
        // The scale its lettering gives: the symbol a little larger or smaller than that.
        let unit = text_h / tp.text_h;
        if !(unit.is_finite() && unit > 0.0) {
            continue;
        }
        let scales = (MIN_SCALE * unit)..=(MAX_SCALE * unit);
        let round = tp.pieces.iter().filter_map(|p| if let Piece::Round(c, r) = *p { Some((c, r)) } else { None }).max_by(|a, b| a.1.total_cmp(&b.1));
        let long = tp
            .pieces
            .iter()
            .filter_map(|p| if let Piece::Seg(a, b) = *p { Some((a, b)) } else { None })
            .max_by(|x, y| x.0.dist(x.1).total_cmp(&y.0.dist(y.1)));
        // Each candidate (turn, scale, middle).
        let mut tries: Vec<(u8, f64, Vec2)> = Vec::new();
        for q in scene.pieces.iter() {
            if tries.len() >= MAX_ANCHORS {
                break;
            }
            match (round, long, *q) {
                (Some((c, r)), _, Piece::Round(qc, qr)) if inside(qc) => {
                    let s = qr / r;
                    if scales.contains(&s) {
                        for k in 0..4u8 {
                            tries.push((k, s, qc - quarter(c, k) * s));
                        }
                    }
                }
                (None, Some((a, b)), Piece::Seg(qa, qb)) if inside(qa) || inside(qb) => {
                    let s = qa.dist(qb) / a.dist(b);
                    if !scales.contains(&s) {
                        continue;
                    }
                    for k in 0..4u8 {
                        let d = quarter(b - a, k);
                        if !parallel(d, qb - qa) {
                            continue;
                        }
                        // Its ends either way round.
                        let tol = PIECE_TOL * tp.size * s;
                        for (p, q2) in [(qa, qb), (qb, qa)] {
                            let t = p - quarter(a, k) * s;
                            if (t + quarter(b, k) * s).dist(q2) <= tol {
                                tries.push((k, s, t));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for (k, s, t) in tries {
            if !inside(t) || found.iter().any(|(f, _, _)| f.template == ti && f.at.dist(t) <= 0.5 * tp.size * s) {
                continue;
            }
            if drawn_at(tp, scene, k, s, t) {
                found.push((Found { at: t, template: ti, size: tp.size * s }, tp.pieces.len(), drawn_length(tp) * s));
            }
        }
    }
    // The most detailed first (the most pieces, then the most linework explained: a wall duplex's
    // bars run through its circle, a ceiling one's stop inside it); another found within half a
    // symbol of a kept one is the same.
    found.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| b.2.total_cmp(&a.2)).then_with(|| a.0.at.x.total_cmp(&b.0.at.x)).then_with(|| a.0.at.y.total_cmp(&b.0.at.y))
    });
    let mut kept: Vec<Found> = Vec::new();
    for (f, _, _) in found {
        if !kept.iter().any(|k| k.at.dist(f.at) <= 0.5 * k.size.max(f.size)) {
            kept.push(f);
        }
    }
    kept
}

/// How much linework a template draws: its straight pieces' lengths and its circles' rounds.
fn drawn_length(tp: &Template) -> f64 {
    tp.pieces
        .iter()
        .map(|p| match *p {
            Piece::Seg(a, b) => a.dist(b),
            Piece::Round(_, r) => std::f64::consts::TAU * r,
        })
        .sum()
}

/// One typical-room view as the reading takes it: its sheet and level, its box, the lettering in
/// it, the devices already read in it.
pub(crate) struct View<'a> {
    pub sheet: &'a str,
    pub level: &'a str,
    pub rect: [f64; 4],
    pub texts: Vec<&'a TextItem>,
    pub read: Vec<&'a ReadDevice>,
}

/// What the documents say the view's marks are: the fixture marks of the fixture schedule and
/// submittals (ceiling, then wall), the legend row that says a lower-case letter after a circuit
/// is a fixture's switch leg (its sheet and words), and the set's sheet numbers (never a circuit).
#[derive(Default)]
pub(crate) struct Marks {
    pub luminaires: BTreeSet<String>,
    pub wall_luminaires: BTreeSet<String>,
    pub leg_rule: Option<(String, String)>,
    pub sheet_numbers: BTreeSet<String>,
}

/// How far from a symbol its circuit label is printed, and a read device takes the label it
/// printed, in label heights.
const LABEL_REACH_H: f64 = 6.0;
/// A fixture mark stacked over its circuit: this far to either side and up, in label heights.
const STACK_DX_H: f64 = 1.5;
const STACK_DY_H: f64 = 2.5;
/// A switch read off its designation sits on the designation's lettering: within this many
/// heights of it.
const SWITCH_AT_H: f64 = 0.75;
/// A mark printed by a symbol found by its drawing: within this many of the view's lettering
/// heights of it, and from this share to this many times as high as the view's lettering
/// (smaller or larger is a misread of strokes).
const MARK_REACH_H: f64 = 3.0;
const MIN_MARK_H: f64 = 0.6;
const MAX_MARK_H: f64 = 2.0;
/// A label on a leader names a symbol up to this many label heights off.
const LABEL_FAR_H: f64 = 12.0;
/// Marks of one view read at most.
const MAX_VIEW_MARKS: usize = 2_000;

/// Whether a label is a circuit: `G-1`, `LP-3,5`, `G-1,a` (its leg after it), never a sheet
/// number.
fn circuit_label(t: &str, sheet_numbers: &BTreeSet<String>) -> Option<(String, Option<char>)> {
    let t = t.trim();
    // The leg letter after it: `G-1,a`.
    let (head, leg) = match t.rsplit_once(',') {
        Some((h, l)) if l.trim().len() == 1 && l.trim().chars().all(|c| c.is_ascii_lowercase()) => (h.trim(), l.trim().chars().next()),
        _ => (t, None),
    };
    (super::guided::circuit_text(head, sheet_numbers) || super::printcond::parse_circuit(head).is_some()).then(|| (head.to_string(), leg))
}

/// The switches a designation printed by switch symbols names, as (tag, leg): `3,D,a` a 3-way
/// (`S3`) and a dimmer (`SD`) on leg `a`; one designation as [`super::readers::switch_designation`].
pub(crate) fn switch_designations(t: &str) -> Vec<(String, String)> {
    if let Some(one) = super::readers::switch_designation(t) {
        return vec![one];
    }
    let t = t.trim();
    let Some((codes, leg)) = t.rsplit_once(',') else { return Vec::new() };
    let codes: Vec<&str> = codes.split(',').map(str::trim).collect();
    if codes.len() < 2 || codes.len() > 4 {
        return Vec::new();
    }
    let each: Option<Vec<(String, String)>> = codes.iter().map(|c| super::readers::switch_designation(&format!("{c},{}", leg.trim()))).collect();
    let each = each.unwrap_or_default();
    // A designation names each switch once (`3,3,a` is no reading).
    let tags: BTreeSet<&str> = each.iter().map(|(tag, _)| tag.as_str()).collect();
    if tags.len() == each.len() { each } else { Vec::new() }
}

/// A read device's kind: the one read, else what its tag says.
pub(crate) fn kind_of(dv: &ReadDevice) -> String {
    if dv.kind.is_empty() { super::readers::device_kind(&dv.tag).to_string() } else { dv.kind.clone() }
}

/// A symbol drawn in a view: where its middle is, its size, the kind and IFC class it is and
/// where that comes from, and its mark when that is known (a symbol block the job read elsewhere
/// as a device with that mark).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Symbol {
    pub at: Vec2,
    pub size: f64,
    pub kind: String,
    pub ifc: String,
    pub source: String,
    pub mark: Option<String>,
    /// Whether it is on a power circuit (takes a circuit label).
    pub powered: bool,
}

/// The legend's drawings `found` in a view, as symbols of their rows' kinds.
pub(crate) fn legend_symbols(templates: &[Template], found: &[Found]) -> Vec<Symbol> {
    found
        .iter()
        .filter_map(|f| {
            let tp = templates.get(f.template)?;
            Some(Symbol {
                at: f.at,
                size: f.size,
                kind: tp.kind.clone(),
                ifc: tp.ifc.clone(),
                source: format!("{} legend: {}", tp.sheet, tp.row),
                mark: None,
                powered: !tp.low_voltage,
            })
        })
        .collect()
}

/// The marks of a typical-room view that no device read takes: the symbols drawn in it that no
/// read device stands on (the legend's drawn as loose linework, [`legend_symbols`]; symbol blocks
/// the job read elsewhere), the circuit labels no read device took, the switch designations no
/// read switch took. Each a new device at its sheet point.
pub(crate) fn read_view(view: &View, symbols: &[Symbol], marks: &Marks) -> Vec<ReadDevice> {
    let inside = |p: Vec2| p.x >= view.rect[0] && p.x <= view.rect[2] && p.y >= view.rect[1] && p.y <= view.rect[3];
    let sized = |t: &&TextItem| t.height.is_finite() && t.height > 0.0;
    let texts: Vec<&TextItem> = view.texts.iter().copied().filter(sized).filter(|t| inside(t.at)).take(MAX_VIEW_MARKS).collect();
    // The view's lettering height (its middle one): lettering much larger is a misread of
    // strokes, no mark.
    let mut heights: Vec<f64> = texts.iter().map(|t| t.height).collect();
    heights.sort_by(f64::total_cmp);
    let text_h = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    let circuits: Vec<(&TextItem, String, Option<char>)> =
        texts.iter().filter_map(|t| circuit_label(&t.value, &marks.sheet_numbers).map(|(c, leg)| (*t, c, leg))).collect();
    let at_of = |dv: &ReadDevice| Vec2::new(dv.at[0], dv.at[1]);
    let mut out: Vec<ReadDevice> = Vec::new();
    let new = |tag: &str, at: Vec2, kind: &str, why: &str| ReadDevice {
        tag: tag.to_string(),
        at: [at.x, at.y],
        sheet: view.sheet.to_string(),
        level: view.level.to_string(),
        kind: kind.to_string(),
        kind_source: why.to_string(),
        ..ReadDevice::default()
    };
    // Each circuit label's taker, one label each.
    let mut taken = vec![false; circuits.len()];
    let full = |c: &str, leg: Option<char>| match leg {
        Some(l) => format!("{c},{l}"),
        None => c.to_string(),
    };
    // A fixture mark printed over a circuit label: the fixture's (its kind by the mark's list).
    let mark_over = |t: &TextItem| -> Option<(&TextItem, &'static str)> {
        texts
            .iter()
            .copied()
            .filter(|m| {
                (m.at.x - t.at.x).abs() <= STACK_DX_H * t.height && m.at.y - t.at.y > 0.3 * t.height && m.at.y - t.at.y <= STACK_DY_H * t.height
            })
            .find_map(|m| {
                let v = m.value.trim().to_ascii_uppercase();
                if marks.luminaires.contains(&v) {
                    Some((m, "luminaire"))
                } else if marks.wall_luminaires.contains(&v) {
                    Some((m, "wall luminaire"))
                } else {
                    None
                }
            })
    };
    // The read devices that name a circuit take its labels, the nearest pairs first (one label
    // each): the labels printed as they name it, then (for a device still without one) a label
    // printed by it that adds a leg letter to it (the reading kept the circuit, not its leg) and
    // no fixture's mark is printed over.
    let mut took: BTreeSet<usize> = BTreeSet::new();
    for exact in [true, false] {
        let mut named: Vec<(f64, usize, usize)> = Vec::new();
        for (j, dv) in view.read.iter().enumerate().filter(|(j, dv)| !dv.circuit.is_empty() && !took.contains(j)) {
            for (i, (t, c, leg)) in circuits.iter().enumerate().filter(|(i, _)| !taken.get(*i).copied().unwrap_or(true)) {
                let d = t.at.dist(at_of(dv));
                // (A fixture's label, its mark over it, is never a receptacle's.)
                let names = if exact {
                    dv.circuit.trim().eq_ignore_ascii_case(&full(c, *leg))
                } else {
                    leg.is_some() && dv.circuit.trim().eq_ignore_ascii_case(c) && mark_over(t).is_none()
                };
                let reach = if exact { LABEL_FAR_H } else { LABEL_REACH_H };
                if names && d <= reach * t.height {
                    named.push((d, i, j));
                }
            }
        }
        named.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, i, j) in named {
            if !taken.get(i).copied().unwrap_or(true)
                && !took.contains(&j)
                && let Some(slot) = taken.get_mut(i)
            {
                *slot = true;
                took.insert(j);
            }
        }
    }
    // The legend's symbols drawn loose: those no read device stands on. Each takes the label by it,
    // the nearest pairs first.
    let fresh: Vec<&Symbol> = symbols.iter().filter(|f| inside(f.at) && !view.read.iter().any(|dv| at_of(dv).dist(f.at) <= f.size)).collect();
    let mut by_label: Vec<(f64, usize, usize)> = Vec::new();
    for (j, f) in fresh.iter().enumerate().filter(|(_, f)| f.powered) {
        for (i, (t, _, _)) in circuits.iter().enumerate().filter(|(i, _)| !taken.get(*i).copied().unwrap_or(true)) {
            let d = t.at.dist(f.at);
            if d <= LABEL_REACH_H * t.height {
                by_label.push((d, i, j));
            }
        }
    }
    by_label.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut label_of: BTreeMap<usize, usize> = BTreeMap::new();
    for (_, i, j) in by_label {
        if !taken.get(i).copied().unwrap_or(true)
            && !label_of.contains_key(&j)
            && let Some(slot) = taken.get_mut(i)
        {
            *slot = true;
            label_of.insert(j, i);
        }
    }
    // (The low voltage ones, on no power circuit, by their place in `out`.)
    let mut unpowered: BTreeSet<usize> = BTreeSet::new();
    for (j, f) in fresh.iter().enumerate() {
        if out.iter().any(|o| at_of(o).dist(f.at) <= f.size) {
            continue;
        }
        let circuit = label_of.get(&j).and_then(|i| circuits.get(*i)).map(|(t, c, leg)| (t, c.clone(), *leg));
        // Its mark: the one known, else the one printed by it (a short word no other reading
        // explains).
        let printed = texts
            .iter()
            .filter(|t| {
                t.height >= MIN_MARK_H * text_h && t.height <= MAX_MARK_H * text_h && t.at.dist(f.at) <= MARK_REACH_H * text_h.max(f.size / 3.0)
            })
            .map(|t| t.value.trim())
            .filter(|v| {
                (1..=4).contains(&v.len())
                    && v.chars().all(|c| c.is_ascii_alphanumeric())
                    && v.chars().any(|c| c.is_ascii_alphabetic())
                    && circuit_label(v, &marks.sheet_numbers).is_none()
                    && switch_designations(v).is_empty()
            })
            .min_by_key(|v| v.len())
            .map(str::to_ascii_uppercase);
        let mark = f.mark.clone().or(printed);
        let receptacle = f.kind.contains("receptacle");
        let tag = match (&mark, receptacle) {
            (Some(m), _) => m.clone(),
            (None, true) => {
                let near: Vec<&str> = circuit
                    .as_ref()
                    .map(|(t, _, _)| texts.iter().filter(|o| o.at.dist(t.at) <= 4.0 * t.height).map(|o| o.value.as_str()).collect())
                    .unwrap_or_default();
                receptacle_mark(&near)
            }
            (None, false) => f.kind.to_ascii_uppercase(),
        };
        let mut dv = new(&tag, f.at, &f.kind, &f.source);
        dv.ifc_class.clone_from(&f.ifc);
        if let Some((_, c, leg)) = circuit {
            dv.circuit = full(&c, leg);
        }
        if !f.powered {
            unpowered.insert(out.len());
        }
        out.push(dv);
    }
    // A label printed farther off (on a leader) names a symbol left without one: the nearest
    // such symbol takes it, the nearest pairs first (a switch's designation is no circuit's).
    let mut pairs: Vec<(f64, usize, Option<usize>)> = Vec::new();
    for (i, (t, _, _)) in circuits.iter().enumerate().filter(|(i, _)| !taken.get(*i).copied().unwrap_or(true)) {
        let near = |p: Vec2| p.dist(t.at) <= LABEL_FAR_H * t.height;
        for (j, o) in out.iter().enumerate().filter(|(j, o)| !unpowered.contains(j) && o.circuit.is_empty() && o.leg.is_empty() && near(at_of(o))) {
            pairs.push((at_of(o).dist(t.at), i, Some(j)));
        }
        for dv in view.read.iter().filter(|dv| dv.circuit.is_empty() && dv.leg.is_empty() && kind_of(dv) != "switch" && near(at_of(dv))) {
            pairs.push((at_of(dv).dist(t.at), i, None));
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut used: BTreeSet<usize> = BTreeSet::new();
    for (_, i, j) in pairs {
        if taken.get(i).copied().unwrap_or(true) || j.is_some_and(|j| used.contains(&j)) {
            continue;
        }
        if let Some(slot) = taken.get_mut(i) {
            *slot = true;
        }
        if let (Some(j), Some((t, c, leg))) = (j, circuits.get(i)) {
            used.insert(j);
            if let Some(o) = out.get_mut(j) {
                o.circuit = full(c, *leg);
                // A receptacle's designations are printed by its label.
                if o.tag == "REC" {
                    let near: Vec<&str> = texts.iter().filter(|x| x.at.dist(t.at) <= 4.0 * t.height).map(|x| x.value.as_str()).collect();
                    o.tag = receptacle_mark(&near);
                }
            }
        }
    }
    // The circuit labels left: each names a device.
    for (i, (t, c, leg)) in circuits.iter().enumerate() {
        if taken.get(i).copied().unwrap_or(true) || out.len() >= MAX_VIEW_MARKS {
            continue;
        }
        let circuit = full(c, *leg);
        let mut dv = match (mark_over(t), leg, &marks.leg_rule) {
            (Some((m, kind)), _, _) => {
                // The fixture is drawn where its mark and circuit are printed: by them.
                new(
                    &m.value.trim().to_ascii_uppercase(),
                    t.at,
                    kind,
                    &format!("a fixture mark the fixture schedule or submittals list ({}) over its circuit", m.value.trim()),
                )
            }
            (None, Some(_), Some((sheet, row))) => new("LUMINAIRE", t.at, "luminaire", &format!("{sheet} legend: {row}")),
            _ => {
                let near: Vec<&str> = texts.iter().filter(|o| o.at.dist(t.at) <= 4.0 * t.height).map(|o| o.value.as_str()).collect();
                new(&receptacle_mark(&near), t.at, "receptacle", "a symbol only its circuit labels: a receptacle")
            }
        };
        dv.circuit = circuit;
        out.push(dv);
    }
    // The switch designations no read switch took.
    for t in &texts {
        let named = switch_designations(&t.value);
        if named.len() < 2 && view.read.iter().any(|dv| !dv.leg.is_empty() && at_of(dv).dist(t.at) <= SWITCH_AT_H * t.height) {
            continue;
        }
        for (n, (tag, leg)) in named.iter().enumerate() {
            let have = view.read.iter().copied().chain(out.iter()).any(|dv| {
                dv.leg == *leg && at_of(dv).dist(t.at) <= SWITCH_AT_H * t.height && (dv.tag == *tag || (tag.is_empty() && kind_of(dv) == "switch"))
            });
            if have || out.len() >= MAX_VIEW_MARKS {
                continue;
            }
            // Side by side along the lettering: one switch symbol each.
            let at = t.at + Vec2::from_angle(t.rotation) * (0.6 * t.height * n as f64);
            let tag = if tag.is_empty() { "S".to_string() } else { tag.clone() };
            let mut dv = new(&tag, at, "", &format!("{} switch designation {}", view.sheet, t.value.trim()));
            dv.leg.clone_from(leg);
            if super::readers::device_kind(&tag) == "device" {
                dv.kind = "switch".into();
            }
            out.push(dv);
        }
    }
    out
}

/// A receptacle's mark from what is printed by it: its designations (`GFCI`), else `REC`.
fn receptacle_mark(near: &[&str]) -> String {
    let words: BTreeSet<String> =
        near.iter().flat_map(|t| t.split(|c: char| !c.is_ascii_alphanumeric())).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect();
    if words.contains("GFCI") || words.contains("GFI") { "GFCI".to_string() } else { "REC".to_string() }
}

/// The devices stamped into one room from different trades' sheets that are one thing: the same
/// kind (each read as [`kind_of`]) within `reach` of each other, from sheets of different
/// disciplines (an electrical plan's thermostat and the mechanical plan's): the first stays, the
/// other goes. A kind nothing names (`device`) is never taken for another's. The ones that went.
pub(crate) fn merge_across_trades(devices: &mut Vec<ReadDevice>, reach: f64) -> Vec<ReadDevice> {
    let discipline = |s: &str| cadaddy_mepfix::sheet_facts::discipline_of(s).to_ascii_uppercase();
    let mut keep = vec![true; devices.len()];
    let mut by_room: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (i, dv) in devices.iter().enumerate().filter(|(_, dv)| !dv.room.is_empty()) {
        by_room.entry((dv.level.as_str(), dv.room.as_str())).or_default().push(i);
    }
    for ids in by_room.values() {
        for (n, &i) in ids.iter().enumerate() {
            if !keep.get(i).copied().unwrap_or(false) {
                continue;
            }
            let Some(a) = devices.get(i) else { continue };
            let ka = kind_of(a);
            if ka == "device" || ka.is_empty() {
                continue;
            }
            for &j in ids.iter().skip(n + 1) {
                let Some(b) = devices.get(j) else { continue };
                if keep.get(j).copied().unwrap_or(false)
                    && discipline(&a.sheet) != discipline(&b.sheet)
                    && kind_of(b) == ka
                    && (a.at[0] - b.at[0]).hypot(a.at[1] - b.at[1]) <= reach
                    && let Some(slot) = keep.get_mut(j)
                {
                    *slot = false;
                }
            }
        }
    }
    let gone: Vec<ReadDevice> = devices.iter().zip(keep.iter()).filter(|(_, k)| !**k).map(|(dv, _)| dv.clone()).collect();
    let mut it = keep.iter();
    devices.retain(|_| it.next().copied().unwrap_or(true));
    gone
}

/// One unit type's line of the per-type table: its rooms, and by kind what its views show and
/// what each room holds (fewest, most, the rooms holding fewer than the views show).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub(crate) struct TypeLine {
    pub unit_type: String,
    pub rooms: usize,
    pub kinds: BTreeMap<String, KindLine>,
}

/// One kind's counts in a [`TypeLine`].
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub(crate) struct KindLine {
    pub sheets: usize,
    pub fewest: usize,
    pub most: usize,
    pub rooms_short: usize,
}

/// The per-type table: for each type, what its views show by kind (`shown`) and what each of its
/// rooms holds by kind (`held`, one map per room).
pub(crate) fn type_table(types: &BTreeMap<String, (BTreeMap<String, usize>, Vec<BTreeMap<String, usize>>)>) -> Vec<TypeLine> {
    types
        .iter()
        .map(|(name, (shown, held))| {
            let all: BTreeSet<&String> = shown.keys().chain(held.iter().flat_map(|h| h.keys())).collect();
            let kinds = all
                .into_iter()
                .map(|k| {
                    let sheets = shown.get(k).copied().unwrap_or(0);
                    let n: Vec<usize> = held.iter().map(|h| h.get(k).copied().unwrap_or(0)).collect();
                    let line = KindLine {
                        sheets,
                        fewest: n.iter().copied().min().unwrap_or(0),
                        most: n.iter().copied().max().unwrap_or(0),
                        rooms_short: n.iter().filter(|c| **c < sheets).count(),
                    };
                    (k.clone(), line)
                })
                .collect();
            TypeLine { unit_type: name.clone(), rooms: held.len(), kinds }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(a: [f64; 2], b: [f64; 2]) -> Piece {
        Piece::Seg(Vec2::new(a[0], a[1]), Vec2::new(b[0], b[1]))
    }
    fn round(c: [f64; 2], r: f64) -> Piece {
        Piece::Round(Vec2::new(c[0], c[1]), r)
    }
    /// A receptacle-like drawing: a circle with two short bars through it, a stub to its wall.
    fn duplex(o: [f64; 2], s: f64, k: u8) -> Vec<Piece> {
        let p = |x: f64, y: f64| {
            let v = quarter(Vec2::new(x * s, y * s), k);
            [o[0] + v.x, o[1] + v.y]
        };
        vec![round(p(0.0, 0.0), 2.0 * s), seg(p(-1.0, -2.5), p(-1.0, 2.5)), seg(p(1.0, -2.5), p(1.0, 2.5)), seg(p(2.0, 0.0), p(4.0, 0.0))]
    }
    /// A triangle (an outlet), side `a`.
    fn triangle(o: [f64; 2], a: f64) -> Vec<Piece> {
        let (p, q, r) = ([o[0], o[1]], [o[0] + a, o[1]], [o[0] + a / 2.0, o[1] + a * 0.866]);
        vec![seg(p, q), seg(q, r), seg(r, p)]
    }
    fn templates() -> Vec<Template> {
        vec![
            Template::new("DUPLEX RECEPTACLE", "E-0", "receptacle", "IfcOutlet", &duplex([500.0, 500.0], 1.0, 0), 4.0).unwrap(),
            Template::new("DATA OUTLET", "E-0", "data_outlet", "IfcOutlet", &triangle([600.0, 500.0], 6.0), 4.0).unwrap(),
        ]
    }

    #[test]
    fn a_legend_symbol_drawn_loose_is_found_turned_and_touching_other_linework() {
        let tps = templates();
        let mut scene = Vec::new();
        // A duplex turned a quarter, a little larger, its stub running on into a wall line.
        scene.extend(duplex([100.0, 100.0], 1.2, 1));
        scene.push(seg([100.0, 104.8], [100.0, 160.0]));
        scene.push(seg([60.0, 160.0], [160.0, 160.0]));
        // A triangle with its leader touching a corner, half round.
        scene.extend(triangle([140.0, 120.0], 6.0).into_iter().map(|p| match p {
            Piece::Seg(a, b) => Piece::Seg(Vec2::new(280.0, 240.0) - a, Vec2::new(280.0, 240.0) - b),
            r => r,
        }));
        scene.push(seg([140.0, 120.0], [120.0, 130.0]));
        // Lone circles and crossing lines that aren't symbols.
        scene.push(round([60.0, 60.0], 2.0));
        scene.push(seg([50.0, 50.0], [70.0, 70.0]));
        let sc = Scene::new(scene, 10.0);
        let found = find_symbols(&tps, &sc, [0.0, 0.0, 200.0, 200.0], 4.0);
        let mut got: Vec<(usize, [i64; 2])> = found.iter().map(|f| (f.template, [f.at.x.round() as i64, f.at.y.round() as i64])).collect();
        got.sort();
        assert_eq!(got.len(), 2, "{found:?}");
        assert_eq!(got.first().map(|g| g.0), Some(0));
        assert_eq!(got.get(1).map(|g| g.0), Some(1));
        // The duplex's middle (its box's: the stub makes it lopsided) near where it was drawn.
        assert!(found.iter().any(|f| f.template == 0 && f.at.dist(Vec2::new(100.0, 101.2)) < 1.0), "{found:?}");
        // Outside the area: nothing.
        assert!(find_symbols(&tps, &sc, [0.0, 0.0, 50.0, 50.0], 4.0).is_empty());
    }

    #[test]
    fn the_more_detailed_symbol_wins_where_two_are_found_at_one_place() {
        // A quad (two duplexes' bars, side by side) holds a duplex.
        let mut quad = duplex([0.0, 0.0], 1.0, 0);
        quad.push(seg([-1.0, 3.0], [1.0, 3.0]));
        quad.push(seg([-1.0, -3.0], [1.0, -3.0]));
        let tps = vec![
            Template::new("DUPLEX RECEPTACLE", "E-0", "receptacle", "IfcOutlet", &duplex([0.0, 0.0], 1.0, 0), 4.0).unwrap(),
            Template::new("QUAD RECEPTACLE", "E-0", "quad_receptacle", "IfcOutlet", &quad.clone(), 4.0).unwrap(),
        ];
        let scene: Vec<Piece> = quad
            .iter()
            .map(|p| match *p {
                Piece::Seg(a, b) => Piece::Seg(a + Vec2::new(50.0, 50.0), b + Vec2::new(50.0, 50.0)),
                Piece::Round(c, r) => Piece::Round(c + Vec2::new(50.0, 50.0), r),
            })
            .collect();
        let found = find_symbols(&tps, &Scene::new(scene, 10.0), [0.0, 0.0, 100.0, 100.0], 4.0);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found.first().map(|f| f.template), Some(1));
    }

    #[test]
    fn a_symbol_holding_another_rows_drawing_is_the_row_whose_drawing_explains_more() {
        // A ceiling duplex: two short bars inside its circle; a wall duplex: bars through it.
        let ceiling = [round([0.0, 0.0], 3.0), seg([-1.0, -2.5], [-1.0, 2.5]), seg([1.0, -2.5], [1.0, 2.5])];
        let wall = [round([0.0, 0.0], 3.0), seg([-1.0, -5.0], [-1.0, 5.0]), seg([1.0, -5.0], [1.0, 5.0])];
        let tps = vec![
            Template::new("CEILING MOUNTED DUPLEX RECEPTACLE", "E-0", "ceiling_receptacle", "IfcOutlet", &ceiling, 4.0).unwrap(),
            Template::new("DUPLEX RECEPTACLE", "E-0", "receptacle", "IfcOutlet", &wall, 4.0).unwrap(),
        ];
        let shift = |ps: &[Piece], d: Vec2| -> Vec<Piece> {
            ps.iter()
                .map(|p| match *p {
                    Piece::Seg(a, b) => Piece::Seg(a + d, b + d),
                    Piece::Round(c, r) => Piece::Round(c + d, r),
                })
                .collect()
        };
        let mut scene = shift(&wall, Vec2::new(50.0, 50.0));
        scene.extend(shift(&ceiling, Vec2::new(150.0, 50.0)));
        let found = find_symbols(&tps, &Scene::new(scene, 10.0), [0.0, 0.0, 200.0, 100.0], 4.0);
        let mut got: Vec<(i64, usize)> = found.iter().map(|f| (f.at.x.round() as i64, f.template)).collect();
        got.sort_unstable();
        assert_eq!(got, [(50, 1), (150, 0)], "{found:?}");
    }

    #[test]
    fn a_template_needs_a_drawing_not_a_lone_circle() {
        assert!(Template::new("X", "E-0", "receptacle", "IfcOutlet", &[round([0.0, 0.0], 2.0)], 4.0).is_none());
        assert!(Template::new("X", "E-0", "receptacle", "IfcOutlet", &triangle([0.0, 0.0], 4.0), 4.0).is_some());
        // A plain box (its letters are lettering) is every keynote's box too; a box with a bar
        // across it is a drawing.
        let square = [seg([0.0, 0.0], [6.0, 0.0]), seg([6.0, 0.0], [6.0, 6.0]), seg([6.0, 6.0], [0.0, 6.0]), seg([0.0, 6.0], [0.0, 0.0])];
        assert!(Template::new("X", "E-0", "low_voltage_device", "IfcCommunicationsAppliance", &square, 4.0).is_none());
        let mut barred = square.to_vec();
        barred.push(seg([0.0, 3.0], [6.0, 3.0]));
        assert!(Template::new("X", "E-0", "low_voltage_device", "IfcCommunicationsAppliance", &barred, 4.0).is_some());
    }

    #[test]
    fn several_designations_by_one_label_are_one_switch_each() {
        let tags = |t: &str| switch_designations(t).into_iter().map(|(t, l)| format!("{t}:{l}")).collect::<Vec<_>>();
        assert_eq!(tags("3,D,a"), ["S3:a", "SD:a"]);
        assert_eq!(tags("D,b"), ["SD:b"]);
        assert_eq!(tags("4, M ,c"), ["S4:c", "SM:c"]);
        assert_eq!(tags("c"), [":c"]);
        assert!(tags("3,3,a").is_empty(), "the same switch twice is no reading");
        assert!(tags("G-1,b").is_empty() && tags("1,2,3,4,5,a").is_empty() && tags("3,D").is_empty());
    }

    fn text(v: &str, at: [f64; 2]) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(at[0], at[1]), height: 4.0, rotation: 0.0 }
    }
    fn read(tag: &str, at: [f64; 2], circuit: &str, leg: &str) -> ReadDevice {
        ReadDevice { tag: tag.into(), at, circuit: circuit.into(), leg: leg.into(), sheet: "E-9".into(), level: "1".into(), ..ReadDevice::default() }
    }

    #[test]
    fn a_views_unread_marks_become_devices_of_the_kind_the_documents_give() {
        let tps = templates();
        let texts = vec![
            // Read already: a receptacle on Q-1 (its symbol a block).
            text("Q-1", [20.0, 20.0]),
            // A two-pole receptacle's circuit, its symbol drawn loose (a legend duplex).
            text("Q-13,15", [60.0, 62.0]),
            // A fixture mark over its circuit (the fixture schedule lists K2).
            text("K2", [100.0, 108.0]),
            text("Q-1,a", [100.0, 100.0]),
            // A circuit with a leg letter, no mark: the legend's rule makes it a fixture.
            text("Q-1,d", [140.0, 100.0]),
            // A circuit alone with GFCI by it: a receptacle.
            text("Q-4", [20.0, 140.0]),
            text("GFCI", [20.0, 133.0]),
            // Two switches' designation, one of them read.
            text("3,D,a", [150.0, 20.0]),
            // A data outlet drawn loose with its mark by it.
            text("2X", [178.0, 150.0]),
            // A sheet number in a note: never a circuit.
            text("Q-101", [10.0, 190.0]),
            // A receptacle's label on a leader, farther off than a label printed by its symbol.
            text("Q-20", [120.0, 160.0]),
            text("GFCI", [120.0, 153.0]),
            text("Q-3,c", [66.0, 114.0]),
        ];
        // Strokes read as giant lettering by the first receptacle: no mark.
        let mut texts = texts;
        texts.push(TextItem { value: "ZZ".into(), at: Vec2::new(64.0, 72.0), height: 60.0, rotation: 0.0 });
        texts.push(TextItem { value: "I".into(), at: Vec2::new(58.0, 68.0), height: 1.0, rotation: 0.0 });
        let mut scene = duplex([60.0, 70.0], 1.0, 2);
        scene.extend(duplex([152.0, 160.0], 1.0, 1));
        scene.extend(triangle([180.0, 140.0], 6.0));
        let sc = Scene::new(scene, 10.0);
        // Read already: a receptacle on Q-1 by the fixture's Q-1,a (a circuit without the leg is
        // another circuit's label).
        // And one read with its circuit but not the leg letter its label adds (Q-3,c).
        let have = [
            read("REC", [22.0, 26.0], "Q-1", ""),
            read("S3", [150.0, 20.0], "", "a"),
            read("REC", [104.0, 96.0], "Q-1", ""),
            read("REC", [60.0, 120.0], "Q-3", ""),
        ];
        let view = View { sheet: "E-9", level: "1", rect: [0.0, 0.0, 200.0, 200.0], texts: texts.iter().collect(), read: have.iter().collect() };
        let marks = Marks {
            luminaires: ["K2".to_string()].into(),
            wall_luminaires: BTreeSet::new(),
            leg_rule: Some(("E-0".into(), "LOWER CASE LETTERING INDICATES SWITCH LEG".into())),
            sheet_numbers: ["Q-101".to_string()].into(),
        };
        let found = find_symbols(&tps, &sc, view.rect, 4.0);
        let got = read_view(&view, &legend_symbols(&tps, &found), &marks);
        let line = |dv: &ReadDevice| {
            format!(
                "{} {} {} {}",
                dv.tag,
                if dv.kind.is_empty() { "-" } else { &dv.kind },
                if dv.circuit.is_empty() { "-" } else { &dv.circuit },
                if dv.leg.is_empty() { "-" } else { &dv.leg }
            )
        };
        let mut lines: Vec<String> = got.iter().map(line).collect();
        lines.sort();
        assert_eq!(
            lines,
            [
                "2X data_outlet - -",
                "GFCI receptacle Q-20 -",
                "GFCI receptacle Q-4 -",
                "K2 luminaire Q-1,a -",
                "LUMINAIRE luminaire Q-1,d -",
                "REC receptacle Q-13,15 -",
                "SD - - a"
            ],
            "{got:?}"
        );
        // Where each was drawn, from its sheet and level, with its source.
        let rec = got.iter().find(|d| d.circuit == "Q-13,15");
        assert!(
            rec.is_some_and(|d| (d.at[0] - 60.0).abs() < 3.0 && (d.at[1] - 70.0).abs() < 3.0 && d.kind_source.contains("E-0 legend: DUPLEX")),
            "{rec:?}"
        );
        // The leader's receptacle is where its symbol is drawn, not at its label.
        let far = got.iter().find(|d| d.circuit == "Q-20");
        assert!(far.is_some_and(|d| (d.at[0] - 152.0).abs() < 3.0 && (d.at[1] - 161.0).abs() < 3.0), "{far:?}");
        assert!(got.iter().all(|d| d.sheet == "E-9" && d.level == "1" && !d.kind_source.is_empty()));
        // Read again with what it read: nothing new (the job's second run adds none).
        let mut all = have.to_vec();
        all.extend(got);
        let again = View { read: all.iter().collect(), ..view };
        assert!(read_view(&again, &legend_symbols(&tps, &found), &marks).is_empty());
        // Without the legend's rule a circuit with a leg letter is a receptacle's.
        let plain = Marks { leg_rule: None, ..marks };
        let view = View { read: have.iter().collect(), ..again };
        assert!(read_view(&view, &legend_symbols(&tps, &found), &plain).iter().any(|d| d.circuit == "Q-1,d" && d.kind == "receptacle"));
    }

    #[test]
    fn two_trades_one_thing_merge_and_different_things_stay() {
        let dv = |tag: &str, kind: &str, sheet: &str, room: &str, at: [f64; 2]| ReadDevice {
            tag: tag.into(),
            kind: kind.into(),
            sheet: sheet.into(),
            room: room.into(),
            level: "Level 2".into(),
            at,
            ..ReadDevice::default()
        };
        let mut v = vec![
            dv("T", "thermostat", "E-9", "201", [10.0, 10.0]),
            dv("T", "thermostat", "M-9", "201", [14.0, 12.0]),
            // Another room's: stays.
            dv("T", "thermostat", "M-9", "202", [14.0, 12.0]),
            // Far apart: two.
            dv("REC", "receptacle", "E-9", "201", [100.0, 10.0]),
            dv("REC", "receptacle", "A-9", "201", [200.0, 10.0]),
            // The same sheet's two: two (a sheet shows each device once).
            dv("REC", "receptacle", "E-9", "201", [102.0, 10.0]),
            // An electrical connection and the fan it feeds: two things.
            dv("EF", "equipment connection", "E-9", "201", [50.0, 50.0]),
            dv("EF", "exhaust fan", "M-9", "201", [50.0, 50.0]),
            // Marks nothing names: never one another's.
            dv("Z", "device", "E-9", "201", [70.0, 70.0]),
            dv("Z", "device", "M-9", "201", [70.0, 70.0]),
        ];
        let gone = merge_across_trades(&mut v, 12.0);
        assert_eq!(
            gone.iter().map(|d| (d.room.as_str(), d.sheet.as_str(), kind_of(d))).collect::<Vec<_>>(),
            [("201", "M-9", "thermostat".to_string())]
        );
        assert_eq!(v.iter().filter(|d| d.tag == "T" && d.room == "201").count(), 1);
        assert_eq!(v.len(), 9);
    }

    #[test]
    fn the_type_table_counts_each_kind_against_the_views() {
        let m = |pairs: &[(&str, usize)]| pairs.iter().map(|(k, n)| (k.to_string(), *n)).collect::<BTreeMap<String, usize>>();
        let mut types = BTreeMap::new();
        types.insert(
            "T1".to_string(),
            (m(&[("receptacle", 5), ("switch", 2)]), vec![m(&[("receptacle", 5), ("switch", 2)]), m(&[("receptacle", 4), ("thermostat", 1)])]),
        );
        let t = type_table(&types);
        let line = t.first().unwrap();
        assert_eq!((line.unit_type.as_str(), line.rooms), ("T1", 2));
        assert_eq!(line.kinds.get("receptacle"), Some(&KindLine { sheets: 5, fewest: 4, most: 5, rooms_short: 1 }));
        assert_eq!(line.kinds.get("switch"), Some(&KindLine { sheets: 2, fewest: 0, most: 2, rooms_short: 1 }));
        assert_eq!(line.kinds.get("thermostat"), Some(&KindLine { sheets: 0, fewest: 0, most: 1, rooms_short: 0 }));
    }
}
