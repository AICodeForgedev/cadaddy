//! Where a symbol block's drawn shape is when the block holds its lettering too (Q6604).
//!
//! A PDF import folds a symbol drawn again and again into a block, from its first copy's strokes
//! around that copy's middle. Lettering printed by the symbol that is not read as text (a circuit
//! number, an exit sign's or a door holder's letters, drawn as strokes), and a leader to it, can
//! go into the block with the symbol, so the block's insert point sits between the symbol and its
//! lettering, often on the letters. A drawing's own blocks can hold lettering too (text, an
//! attribute's definition) with the insert point by the text.
//!
//! The device such a block shows stands where its shape is drawn, not where its lettering is:
//! the block's outline (its biggest closed loop, a circle or a box, with the closed loops over it
//! and the short pieces drawn on it: a receptacle's lines, a tail), when lettering is drawn apart
//! from that shape inside the block and the insert point is off the shape. A block with no
//! closed outline, or no lettering apart from it, keeps its insert point.
//!
//! The rule is the drafting convention (a symbol's mark and circuit are set beside it, a leader
//! where they are far), not one set's: no block name, mark or job value is in it.

use std::collections::HashMap;

use cadaddy_doc::{Block, Drawing, EntityKind, Insert};
use cadaddy_geom::Vec2;

/// Pieces of one block looked at, at most (a bigger block is a drawing, not a symbol).
const MAX_BLOCK_PIECES: usize = 256;
/// A loop's ends within this share of its size close it (a circle the import drew as a polyline).
const CLOSE_TOL: f64 = 0.02;
/// Lettering is smaller than its symbol: a piece apart from the shape at most this share of the
/// shape's size is a letter (or a part of one).
const LETTER_MAX: f64 = 0.75;
/// A piece drawn on the shape at most this share of its size is the shape's own (a receptacle's
/// lines, a tail); a longer one is a leader to the lettering.
const ATTACHED_MAX: f64 = 1.0;
/// Closed loops at least this share of the biggest one's size, over it, are the shape's too (a
/// circle in its box).
const OUTLINE_SHARE: f64 = 0.4;
/// The insert point is off the shape when it is more than this share of the shape's size from
/// its middle; nearer, the block's point is the symbol's.
const OFF_SHAPE: f64 = 0.15;
/// A letter's width as a share of its height (text drawn in a block, to find its extent).
const CHAR_W: f64 = 0.8;

/// What a device on its symbol's drawn shape says (its basis): the block's lettering kept apart.
pub(crate) const SHAPE_SPOT_BASIS: &str = "on its symbol's drawn shape, not the block's insert point: the block holds lettering beside the symbol";

/// A box: min x, min y, max x, max y.
type Box2 = [f64; 4];

fn size(b: &Box2) -> f64 {
    (b[2] - b[0]).max(b[3] - b[1])
}

fn middle(b: &Box2) -> Vec2 {
    Vec2::new((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0)
}

fn overlaps(a: &Box2, b: &Box2) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn union(a: &Box2, b: &Box2) -> Box2 {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

fn box_of(pts: impl IntoIterator<Item = Vec2>) -> Option<Box2> {
    let mut b: Option<Box2> = None;
    for p in pts {
        if !(p.x.is_finite() && p.y.is_finite()) {
            return None;
        }
        b = Some(b.map_or([p.x, p.y, p.x, p.y], |b| [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)]));
    }
    b
}

/// One piece of a block, as this rule sees it.
enum Piece {
    /// A closed loop (circle, closed polyline, one whose ends meet).
    Outline(Box2),
    /// An open stroke (line, arc, open polyline).
    Stroke(Box2),
    /// Lettering written as text.
    Text(Box2),
}

/// The text's extent (rotation ignored: lettering by a symbol reads left to right).
fn text_box(at: Vec2, height: f64, chars: usize) -> Option<Box2> {
    let ok = at.x.is_finite() && at.y.is_finite() && height.is_finite() && height > 0.0 && chars > 0;
    ok.then(|| [at.x, at.y, at.x + height * CHAR_W * chars.min(200) as f64, at.y + height])
}

fn piece(kind: &EntityKind) -> Option<Piece> {
    match kind {
        EntityKind::Line(l) => box_of([l.a.xy(), l.b.xy()]).map(Piece::Stroke),
        EntityKind::Circle(c) if c.radius.is_finite() && c.radius > 0.0 => {
            let (p, r) = (c.center.xy(), c.radius);
            box_of([Vec2::new(p.x - r, p.y - r), Vec2::new(p.x + r, p.y + r)]).map(Piece::Outline)
        }
        EntityKind::Arc(a) if a.radius.is_finite() && a.radius > 0.0 && a.start.is_finite() && a.end.is_finite() => {
            let mut sweep = a.end - a.start;
            while sweep < 0.0 {
                sweep += std::f64::consts::TAU;
            }
            let sweep = sweep.min(std::f64::consts::TAU);
            let at = |k: usize| {
                let t = a.start + sweep * k as f64 / 16.0;
                Vec2::new(a.center.x + a.radius * t.cos(), a.center.y + a.radius * t.sin())
            };
            box_of((0..=16).map(at)).map(Piece::Stroke)
        }
        EntityKind::LwPolyline(p) => {
            let b = box_of(p.vertices.iter().map(|v| v.p))?;
            let ends_meet = match (p.vertices.first(), p.vertices.last()) {
                (Some(a), Some(z)) => p.vertices.len() >= 4 && a.p.dist(z.p) <= CLOSE_TOL * size(&b).max(1e-12),
                _ => false,
            };
            Some(if (p.closed && p.vertices.len() >= 3) || ends_meet { Piece::Outline(b) } else { Piece::Stroke(b) })
        }
        EntityKind::Text(t) => text_box(t.insert.xy(), t.height, t.value.trim().chars().count()).map(Piece::Text),
        EntityKind::MText(t) => {
            let longest = t.contents.lines().map(|l| l.trim().chars().count()).max().unwrap_or(0);
            text_box(t.insert.xy(), t.height, longest).map(Piece::Text)
        }
        EntityKind::AttDef(a) if !a.invisible => {
            text_box(a.text.insert.xy(), a.text.height, a.text.value.trim().chars().count().max(1)).map(Piece::Text)
        }
        _ => None,
    }
}

/// Where `b`'s drawn shape is, in the block's own coordinates, when the block holds lettering
/// apart from its shape and its base point is off the shape; `None` keeps the base point.
pub(crate) fn lettered_shape(b: &Block) -> Option<Vec2> {
    let pieces: Vec<Piece> = b.entities.iter().take(MAX_BLOCK_PIECES).filter_map(|e| piece(&e.kind)).collect();
    // The biggest closed loop, with the big closed loops over it.
    let outlines: Vec<&Box2> = pieces.iter().filter_map(|p| if let Piece::Outline(o) = p { Some(o) } else { None }).collect();
    let main = outlines.iter().copied().max_by(|a, b| size(a).total_cmp(&size(b)))?;
    let main_size = size(main);
    if !(main_size.is_finite() && main_size > 0.0) {
        return None;
    }
    let mut outline = *main;
    for o in &outlines {
        if size(o) >= OUTLINE_SHARE * main_size && overlaps(o, main) {
            outline = union(&outline, o);
        }
    }
    let shape_size = size(&outline);
    // The short pieces drawn on it are its own; lettering is apart from it, and smaller.
    let mut shape = outline;
    let mut letters = 0usize;
    let mut texts = 0usize;
    for p in &pieces {
        match p {
            Piece::Outline(q) | Piece::Stroke(q) if overlaps(q, &outline) => {
                if size(q) <= ATTACHED_MAX * shape_size {
                    shape = union(&shape, q);
                }
            }
            Piece::Outline(q) | Piece::Stroke(q) => {
                if size(q) <= LETTER_MAX * shape_size {
                    letters += 1;
                }
            }
            Piece::Text(q) => {
                if !overlaps(q, &outline) {
                    texts += 1;
                }
            }
        }
    }
    // One stray piece is a part of the symbol; letters come two or more strokes at a time.
    if texts == 0 && letters < 2 {
        return None;
    }
    let at = middle(&shape);
    let base = b.base.xy();
    (at.is_finite() && at.dist(base) > OFF_SHAPE * size(&shape)).then_some(at)
}

/// The drawing's symbol blocks' shapes ([`lettered_shape`]), each block looked at once.
pub(crate) struct BlockShapes<'a> {
    d: &'a Drawing,
    local: HashMap<String, Option<Vec2>>,
}

impl<'a> BlockShapes<'a> {
    pub(crate) fn new(d: &'a Drawing) -> BlockShapes<'a> {
        BlockShapes { d, local: HashMap::new() }
    }

    /// Where the symbol `ins` shows is drawn, in the drawing: on its block's shape when the block
    /// holds lettering beside it; `None` when its insert point is the symbol's.
    pub(crate) fn spot(&mut self, ins: &Insert) -> Option<Vec2> {
        let d = self.d;
        let local = (*self.local.entry(ins.block.clone()).or_insert_with(|| d.block(&ins.block).and_then(|b| lettered_shape(b))))?;
        let base = d.block(&ins.block).map_or(Vec2::ZERO, |b| b.base.xy());
        let p = ins.transform(base).apply(local);
        p.is_finite().then_some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, Entity, Handle, Line, LwPolyline, Text};
    use cadaddy_geom::{PolyVertex, Vec3};

    fn line(a: [f64; 2], b: [f64; 2]) -> EntityKind {
        EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) })
    }

    fn loop_of(pts: &[[f64; 2]], closed: bool) -> EntityKind {
        EntityKind::LwPolyline(LwPolyline {
            vertices: pts.iter().map(|p| PolyVertex { p: Vec2::new(p[0], p[1]), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect(),
            closed,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        })
    }

    /// A circle drawn as a 24-sided polyline whose ends meet (as a PDF import draws it).
    fn ring(c: [f64; 2], r: f64) -> EntityKind {
        let pts: Vec<[f64; 2]> = (0..=24)
            .map(|k| {
                let t = std::f64::consts::TAU * k as f64 / 24.0;
                [c[0] + r * t.cos(), c[1] + r * t.sin()]
            })
            .collect();
        loop_of(&pts, false)
    }

    fn block(kinds: Vec<EntityKind>) -> Block {
        let mut b = Block::new("SYM-T");
        for (i, k) in kinds.into_iter().enumerate() {
            b.entities.push(Entity { handle: Handle(i as u64 + 1), common: Common::default(), kind: k });
        }
        b
    }

    /// Stroke lettering: `n` letters, each two strokes, of height `h`, from `x0` along `y0`.
    fn letters(n: usize, x0: f64, y0: f64, h: f64) -> Vec<EntityKind> {
        (0..n)
            .flat_map(|i| {
                let x = x0 + i as f64 * h * 0.9;
                [line([x, y0], [x + 0.3 * h, y0 + h]), line([x + 0.3 * h, y0 + h], [x + 0.6 * h, y0])]
            })
            .collect()
    }

    #[test]
    fn a_block_holding_lettering_beside_its_circle_is_on_the_circle() {
        // A downlight's circle (radius 4) at (-6, 0) with two-digit stroke lettering to its right;
        // the import's base point is the middle of both.
        let mut k = vec![ring([-6.0, 0.0], 4.0)];
        k.extend(letters(2, 1.0, -1.5, 3.0));
        let at = lettered_shape(&block(k)).expect("the lettering is apart from the shape");
        assert!(at.dist(Vec2::new(-6.0, 0.0)) < 1e-6, "{at:?}");
    }

    #[test]
    fn a_leader_to_the_lettering_is_not_the_shape() {
        // A door holder's box at (10, 10), a leader from its corner out to its letters at the
        // block's base point.
        let mut k = vec![loop_of(&[[7.0, 7.0], [13.0, 7.0], [13.0, 13.0], [7.0, 13.0]], true), line([7.0, 7.0], [-1.0, 1.5])];
        k.extend(letters(2, -2.5, -1.5, 3.0));
        let at = lettered_shape(&block(k)).expect("on the box");
        assert!(at.dist(Vec2::new(10.0, 10.0)) < 1e-6, "{at:?}");
    }

    #[test]
    fn text_in_the_block_is_lettering() {
        // A drawing's own block: an exit sign's box, the word written as text beside it, the base
        // point at the word.
        let k = vec![
            loop_of(&[[10.0, -3.0], [22.0, -3.0], [22.0, 3.0], [10.0, 3.0]], true),
            EntityKind::Text(Text {
                insert: Vec3::new(0.0, 0.0, 0.0),
                align_pt: None,
                height: 2.0,
                value: "EX".into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: Default::default(),
                valign: Default::default(),
            }),
        ];
        let at = lettered_shape(&block(k)).expect("on the box");
        assert!(at.dist(Vec2::new(16.0, 0.0)) < 1e-6, "{at:?}");
    }

    #[test]
    fn a_symbol_with_no_lettering_or_no_outline_keeps_its_point() {
        // A receptacle: its circle with two lines across it and a tail; no lettering apart.
        let k = vec![ring([0.0, 0.0], 4.0), line([-6.0, 1.0], [6.0, 1.0]), line([-6.0, -1.0], [6.0, -1.0]), line([0.0, 4.0], [0.0, 7.0])];
        assert_eq!(lettered_shape(&block(k)), None);
        // Lettering inside the outline (a J in its circle) is the symbol's own.
        let mut k = vec![ring([0.0, 0.0], 5.0)];
        k.extend(letters(1, -1.0, -1.5, 3.0));
        assert_eq!(lettered_shape(&block(k)), None);
        // Strokes only (a switch's S): no shape to tell its letters from.
        assert_eq!(lettered_shape(&block(letters(3, 0.0, 0.0, 3.0))), None);
        // Lettering beside a shape already at the base point: the point is the symbol's.
        let mut k = vec![ring([0.0, 0.0], 4.0)];
        k.extend(letters(2, 5.0, -1.5, 1.0));
        assert_eq!(lettered_shape(&block(k)), None);
        // One stray piece apart is a part of the symbol, not lettering.
        let k = vec![ring([-6.0, 0.0], 4.0), line([3.0, -1.0], [4.0, 1.0])];
        assert_eq!(lettered_shape(&block(k)), None);
        // Nothing, hostile values: no panic, no spot.
        assert_eq!(lettered_shape(&block(Vec::new())), None);
        let k = vec![ring([f64::NAN, 0.0], 4.0), line([f64::INFINITY, 0.0], [1.0, 1.0]), line([0.0, 0.0], [1.0, f64::NAN])];
        assert_eq!(lettered_shape(&block(k)), None);
    }

    #[test]
    fn an_insert_turned_and_scaled_carries_its_shape_with_it() {
        let mut d = Drawing::default();
        let mut k = vec![ring([-6.0, 0.0], 4.0)];
        k.extend(letters(2, 1.0, -1.5, 3.0));
        d.blocks.insert("SYM-T".into(), std::sync::Arc::new(block(k)));
        let ins = Insert {
            block: "SYM-T".into(),
            insert: Vec3::new(100.0, 50.0, 0.0),
            scale: Vec3::new(2.0, 2.0, 1.0),
            rotation: std::f64::consts::FRAC_PI_2,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        let mut shapes = BlockShapes::new(&d);
        let at = shapes.spot(&ins).expect("on the shape");
        // (-6, 0) scaled 2 and turned a quarter: (0, -12) from the insert.
        assert!(at.dist(Vec2::new(100.0, 38.0)) < 1e-6, "{at:?}");
        // A block not in the drawing: its insert point stands.
        assert_eq!(shapes.spot(&Insert { block: "NONE".into(), ..ins }), None);
    }
}
