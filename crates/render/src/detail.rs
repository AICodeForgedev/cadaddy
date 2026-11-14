//! Screen detail levels for 2D views (Q6417), like a map's zoom levels: a list built for the
//! screen at a zoom draws what would be under a pixel or unreadable there as less.
//!
//! - An object whose box is under [`PlanRules::dot_px`] pixels is one dot.
//! - Text under [`PlanRules::text_bar_px`] pixels high is a grey bar where its words are.
//! - A hatch whose pattern lines are closer than [`PlanRules::hatch_tint_px`] pixels is a light
//!   tint of its area (closer still it already reads solid, see `hatch::reads_solid`).
//! - A symbol (block reference) is its box until it is big enough on screen for its tier
//!   ([`PlanRules::tier_px`], by [`Tier`] as the 3D view's fidelity layers: building, floor,
//!   room, detail); its contents then come in. Which tier a symbol is comes from rules data
//!   ([`SymbolRule`]: words of its block's and layer's names), not from the code.
//!
//! Only lists built for the screen carry a [`Detail`] (`Options::detail`); plots, exports,
//! picks, counts and takeoffs build without one and get every line. The snaps ask [`Detail::look`]
//! too, so they never offer a point of something drawn as less.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use cadaddy_doc::{Drawing, Entity, EntityKind, Insert};
use cadaddy_geom::{Bounds2, Mat3, Vec2};

pub use crate::model3::Tier;

/// A rule saying which tier symbols are: any of its words in the block's name, or any of its
/// layer words in the reference's layer name (a rule with both needs one of each). The first
/// rule that holds wins.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SymbolRule {
    pub words: Vec<String>,
    pub layers: Vec<String>,
    pub tier: Tier,
}

impl SymbolRule {
    fn matches(&self, block: &str, layer: &str) -> bool {
        if self.words.is_empty() && self.layers.is_empty() {
            return false;
        }
        (self.words.is_empty() || self.words.iter().any(|w| has_word(block, w)))
            && (self.layers.is_empty() || self.layers.iter().any(|w| has_word(layer, w)))
    }
}

/// The thresholds (screen pixels) and the symbol rules.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanRules {
    /// An object whose box is smaller than this is one dot.
    pub dot_px: f64,
    /// Text whose height is under this is a bar.
    pub text_bar_px: f64,
    /// A hatch whose pattern lines are closer than this is a tint of its area.
    pub hatch_tint_px: f64,
    /// A symbol smaller than this is its box whatever its tier.
    pub box_px: f64,
    /// Per tier (building, floor, room, detail): how big a symbol of that tier must be on screen
    /// before its contents are drawn.
    pub tier_px: [f64; 4],
    /// The tier of a symbol no rule names.
    pub default_tier: Tier,
    pub symbols: Vec<SymbolRule>,
    /// Transparency (percent) of the text bars and of the hatch tints.
    pub bar_transparency: u8,
    pub tint_transparency: u8,
    /// The long side (pixels) of the picture of a sheet the screen shows zoomed out.
    pub image_px: u32,
}

/// Most symbol rules (rules data from a project).
pub const MAX_SYMBOL_RULES: usize = 256;
/// Most text bars one multiline text draws (one per line).
const MAX_BARS: usize = 64;
/// Most block extents a [`Detail`] keeps (a hostile drawing's block count).
const MAX_BLOCKS: usize = 100_000;

impl Default for PlanRules {
    /// What the screen uses when a drawing's rules data can't be read: thresholds only, every
    /// symbol in the floor tier.
    fn default() -> Self {
        PlanRules {
            dot_px: 1.0,
            text_bar_px: 3.0,
            hatch_tint_px: 4.0,
            box_px: 4.0,
            tier_px: [0.0, 8.0, 16.0, 32.0],
            default_tier: Tier::Floor,
            symbols: Vec::new(),
            bar_transparency: 55,
            tint_transparency: 75,
            image_px: 2048,
        }
    }
}

impl PlanRules {
    /// The rules made safe to use: thresholds finite and within reason, at most
    /// [`MAX_SYMBOL_RULES`] symbol rules.
    pub fn sane(mut self) -> PlanRules {
        let d = PlanRules::default();
        let px = |v: f64, d: f64| if v.is_finite() && v >= 0.0 { v.min(1000.0) } else { d };
        self.dot_px = px(self.dot_px, d.dot_px);
        self.text_bar_px = px(self.text_bar_px, d.text_bar_px);
        self.hatch_tint_px = px(self.hatch_tint_px, d.hatch_tint_px);
        self.box_px = px(self.box_px, d.box_px);
        for (v, dv) in self.tier_px.iter_mut().zip(d.tier_px) {
            *v = px(*v, dv);
        }
        self.symbols.truncate(MAX_SYMBOL_RULES);
        // Words compared as the names are: trimmed, lower case (once, not per symbol drawn).
        let norm = |v: &mut Vec<String>| {
            for w in v.iter_mut() {
                *w = w.trim().to_ascii_lowercase();
            }
            v.retain(|w| !w.is_empty());
        };
        for r in &mut self.symbols {
            norm(&mut r.words);
            norm(&mut r.layers);
        }
        self.bar_transparency = self.bar_transparency.min(90);
        self.tint_transparency = self.tint_transparency.min(90);
        self.image_px = self.image_px.clamp(256, 4096);
        self
    }

    /// The tier of a reference to `block` on `layer` (rules made safe by [`PlanRules::sane`]).
    pub fn tier_of(&self, block: &str, layer: &str) -> Tier {
        let (block, layer) = (block.to_ascii_lowercase(), layer.to_ascii_lowercase());
        self.symbols.iter().find(|r| r.matches(&block, &layer)).map_or(self.default_tier, |r| r.tier)
    }

    /// How big (pixels) a symbol of `tier` must be for its contents to be drawn.
    pub fn full_px(&self, tier: Tier) -> f64 {
        self.tier_px.get(tier.index()).copied().unwrap_or(0.0).max(self.box_px)
    }

    /// How big (pixels) a symbol of any tier must be for its contents to be drawn: past it no
    /// symbol's tier needs looking up.
    fn any_full_px(&self) -> f64 {
        self.tier_px.iter().copied().fold(self.box_px, f64::max)
    }
}

/// Whether `w` is in `text` (both trimmed and lower case, see [`PlanRules::sane`]): a phrase
/// (with a space or dash) as text, a short word as a whole word, a word of five letters or more
/// as a word's start (plurals).
fn has_word(text: &str, w: &str) -> bool {
    if w.is_empty() {
        return false;
    }
    if w.contains(' ') || w.contains('-') {
        return text.contains(w);
    }
    text.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t == w || (w.len() >= 5 && t.starts_with(w)))
}

/// How an object is drawn at a [`Detail`].
#[derive(Clone, Debug, PartialEq)]
pub enum Look {
    /// As it is.
    Full,
    /// One dot at this point (the object's own coordinates).
    Dot(Vec2),
    /// A symbol as its box (corners in the reference's own coordinates).
    Box([Vec2; 4]),
    /// Text as bars (filled quads in its own coordinates).
    Bars(Vec<[Vec2; 4]>),
}

/// The detail a screen list is built at: a pixel in world units at the finest zoom the list
/// serves, and the rules.
#[derive(Clone)]
pub struct Detail {
    pub px: f64,
    pub rules: Arc<PlanRules>,
    /// Block contents' extents (block coordinates) by name, measured once per detail (on first
    /// use, by one thread; then read without a lock by every thread of a parallel build).
    blocks: Arc<OnceLock<HashMap<String, Bounds2>>>,
}

impl std::fmt::Debug for Detail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Detail").field("px", &self.px).field("rules", &self.rules).finish()
    }
}

impl PartialEq for Detail {
    fn eq(&self, o: &Detail) -> bool {
        self.px.to_bits() == o.px.to_bits() && (Arc::ptr_eq(&self.rules, &o.rules) || self.rules == o.rules)
    }
}

impl Detail {
    /// The detail for a pixel of `px` world units; `None` when `px` isn't a usable size.
    pub fn new(px: f64, rules: Arc<PlanRules>) -> Option<Detail> {
        (px.is_finite() && px > 0.0).then(|| Detail { px, rules, blocks: Arc::default() })
    }

    /// The same rules at another pixel size, for the same drawing (block extents carried over:
    /// they don't depend on it).
    pub fn at(&self, px: f64) -> Option<Detail> {
        (px.is_finite() && px > 0.0).then(|| Detail { px, rules: self.rules.clone(), blocks: self.blocks.clone() })
    }

    /// What tells two details apart in a cache key.
    pub fn key(&self) -> String {
        format!("{:x}/{}", self.px.to_bits(), self.rules_key())
    }

    /// [`Detail::key`] without the pixel size: the rules alone (the chunk cache keeps a part
    /// built at another pixel size out of view, `cache::ViewDetail`).
    pub fn rules_key(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{:?}", self.rules).hash(&mut h);
        format!("{:x}", h.finish())
    }

    /// The extents of `blk`'s contents in block coordinates (empty: draws nothing).
    fn block_box(&self, d: &Drawing, blk: &cadaddy_doc::Block) -> Bounds2 {
        let all = self.blocks.get_or_init(|| {
            if d.blocks.len() > MAX_BLOCKS {
                return HashMap::new();
            }
            d.blocks.iter().map(|(name, b)| (name.clone(), contents_box(d, b))).collect()
        });
        match all.get(&blk.name) {
            Some(b) => *b,
            None => contents_box(d, blk),
        }
    }

    /// Pixels a length `l` in coordinates mapped by `xf` covers on screen (along the most
    /// stretched axis, so nothing that shows is taken for less).
    fn pixels(&self, xf: &Mat3, l: f64) -> f64 {
        let k = crate::max_scale(xf);
        let p = l * k / self.px;
        if p.is_finite() { p } else { f64::INFINITY }
    }

    /// How `e` (drawn through `xf`, its coordinates to the list's) looks at this detail.
    pub fn look(&self, d: &Drawing, e: &Entity, xf: &Mat3) -> Look {
        let r = &self.rules;
        match &e.kind {
            EntityKind::Text(t) => self.text_look(xf, t, t.value.chars().count()),
            EntityKind::AttDef(a) => self.text_look(xf, &a.text, a.tag.chars().count()),
            EntityKind::MText(t) => self.mtext_look(xf, t),
            EntityKind::Insert(ins) => self.insert_look(d, e, ins, xf),
            _ => match linework_box(d, e) {
                Some(b) if !b.is_empty() && self.pixels(xf, b.width().max(b.height())) < r.dot_px => Look::Dot(b.center()),
                _ => Look::Full,
            },
        }
    }

    /// A single-line text of `len` characters: a bar when under [`PlanRules::text_bar_px`].
    pub fn text_look(&self, xf: &Mat3, t: &cadaddy_doc::Text, len: usize) -> Look {
        if !(t.height.is_finite() && t.height > 0.0) || self.pixels(xf, t.height) >= self.rules.text_bar_px {
            return Look::Full;
        }
        let [p0, p1, p2, p3] = cadaddy_doc::text_quad(t, len);
        if !(p0.is_finite() && p1.is_finite() && p2.is_finite() && p3.is_finite()) {
            return Look::Full;
        }
        if self.pixels(xf, p0.dist(p1).max(p0.dist(p3))) < self.rules.dot_px {
            return Look::Dot((p0 + p2) * 0.5);
        }
        // The middle of the line's height, where the letters' ink is.
        let up = p3 - p0;
        Look::Bars(vec![[p0 + up * 0.2, p1 + up * 0.2, p1 + up * 0.8, p0 + up * 0.8]])
    }

    fn mtext_look(&self, xf: &Mat3, t: &cadaddy_doc::MText) -> Look {
        if !(t.height.is_finite() && t.height > 0.0) || self.pixels(xf, t.height) >= self.rules.text_bar_px {
            return Look::Full;
        }
        // Top left, top right, bottom right, bottom left.
        let [tl, tr, br, bl] = cadaddy_doc::mtext_quad(t);
        if !(tl.is_finite() && tr.is_finite() && br.is_finite() && bl.is_finite()) {
            return Look::Full;
        }
        if self.pixels(xf, tl.dist(tr).max(tl.dist(bl))) < self.rules.dot_px {
            return Look::Dot((tl + br) * 0.5);
        }
        let plain = cadaddy_fonts::plain_text(&cadaddy_fonts::plain_mtext(&t.contents));
        let lines: Vec<usize> = plain.lines().map(|l| l.trim_end().chars().count()).take(MAX_BARS).collect();
        let longest = lines.iter().copied().max().unwrap_or(0).max(1) as f64;
        let n = lines.len().max(1) as f64;
        let (across, down) = (tr - tl, (bl - tl) * (1.0 / n));
        // Where a shorter line starts along the width: left, centre or right by the attachment.
        let lead = match t.attach {
            2 | 5 | 8 => 0.5,
            3 | 6 | 9 => 1.0,
            _ => 0.0,
        };
        let bars: Vec<[Vec2; 4]> = lines
            .iter()
            .enumerate()
            .filter(|(_, c)| **c > 0)
            .map(|(i, c)| {
                let share = (*c as f64 / longest).clamp(0.0, 1.0);
                let a = tl + down * (i as f64) + across * ((1.0 - share) * lead);
                let w = across * share;
                [a + down * 0.25, a + w + down * 0.25, a + w + down * 0.75, a + down * 0.75]
            })
            .collect();
        if bars.is_empty() { Look::Full } else { Look::Bars(bars) }
    }

    fn insert_look(&self, d: &Drawing, e: &Entity, ins: &Insert, xf: &Mat3) -> Look {
        // An array of copies or a clipped reference is drawn as it is.
        if ins.cols > 1 || ins.rows > 1 || ins.clip.as_ref().is_some_and(|c| c.active()) {
            return Look::Full;
        }
        let Some(blk) = d.block(&ins.block) else { return Look::Full };
        let local = self.block_box(d, blk);
        if local.is_empty() {
            return Look::Full;
        }
        let m = ins.transform(blk.base.xy());
        let corners = local.corners().map(|c| m.apply(c));
        if !corners.iter().all(|c| c.is_finite()) {
            return Look::Full;
        }
        let size = self.pixels(&xf.then_before(m), local.width().max(local.height()));
        let r = &self.rules;
        if size >= r.any_full_px() {
            return Look::Full;
        }
        if size < r.dot_px {
            let [a, _, c, _] = corners;
            return Look::Dot((a + c) * 0.5);
        }
        if size < r.full_px(r.tier_of(&ins.block, &e.common.layer)) {
            return Look::Box(corners);
        }
        Look::Full
    }
}

/// The extents of a block's contents in block coordinates (what a reference to it draws, less its
/// attributes).
fn contents_box(d: &Drawing, blk: &cadaddy_doc::Block) -> Bounds2 {
    let mut b = Bounds2::EMPTY;
    for be in blk.entities.iter().filter(|be| !matches!(be.kind, EntityKind::AttDef(_)) && be.common.visible) {
        b = b.union(&cadaddy_doc::entity_bounds(d, be, 1));
    }
    b
}

/// The box of a piece of linework in its own coordinates, when it is quick to find (`None`: not
/// linework, or an object that is never drawn as less, such as a wipeout, image or viewport).
/// A hatch is measured where it is drawn, from the boundary it flattens anyway.
fn linework_box(d: &Drawing, e: &Entity) -> Option<Bounds2> {
    let b = match &e.kind {
        EntityKind::Line(l) => Bounds2::from_points([l.a.xy(), l.b.xy()]),
        EntityKind::Circle(c) => Bounds2::new(c.center.xy(), c.center.xy()).expand(c.radius.abs()),
        EntityKind::Arc(a) => Bounds2::new(a.center.xy(), a.center.xy()).expand(a.radius.abs()),
        EntityKind::LwPolyline(p) => {
            let b = Bounds2::from_points(p.vertices.iter().map(|v| v.p));
            // A bulge bows a segment out by up to half its chord.
            if p.vertices.iter().any(|v| v.bulge != 0.0) { b.expand(b.width().max(b.height()) * 0.5) } else { b }
        }
        EntityKind::Ellipse(el) => Bounds2::new(el.center.xy(), el.center.xy()).expand(el.major.len()),
        EntityKind::Spline(_) | EntityKind::Polyline3d(_) | EntityKind::Solid(_) => cadaddy_doc::entity_bounds(d, e, 1),
        _ => return None,
    };
    let finite = b.min.is_finite() && b.max.is_finite();
    finite.then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_match_whole_words_and_long_word_starts() {
        assert!(has_word("duplex receptacle", "receptacle"));
        assert!(has_word("receptacles-2", "recep"));
        assert!(has_word("e-sym-tv", "tv"));
        assert!(!has_word("tvx", "tv"));
        assert!(has_word("fire alarm horn", "alarm horn"));
    }

    #[test]
    fn first_rule_wins_and_the_default_takes_the_rest() {
        let rules = PlanRules {
            symbols: vec![
                SymbolRule { words: vec![" Outlet".into()], layers: vec![], tier: Tier::Room },
                SymbolRule { words: vec![], layers: vec!["equip".into()], tier: Tier::Building },
            ],
            ..PlanRules::default()
        }
        .sane();
        assert_eq!(rules.tier_of("SYM-OUTLET", "E-EQUIP"), Tier::Room);
        assert_eq!(rules.tier_of("SYM-PANEL", "E-EQUIP"), Tier::Building);
        assert_eq!(rules.tier_of("SYM-PANEL", "A-WALL"), Tier::Floor);
        assert!(rules.full_px(Tier::Room) > rules.full_px(Tier::Floor));
    }

    #[test]
    fn insane_thresholds_are_replaced() {
        let r = PlanRules { dot_px: f64::NAN, text_bar_px: -1.0, tier_px: [f64::INFINITY, 1.0, 2.0, 3.0], ..PlanRules::default() }.sane();
        assert_eq!(r.dot_px, 1.0);
        assert_eq!(r.text_bar_px, 3.0);
        assert_eq!(r.tier_px[0], 0.0);
        assert!(Detail::new(0.0, Arc::new(r.clone())).is_none());
        assert!(Detail::new(f64::NAN, Arc::new(r)).is_none());
    }

    use crate::{Kind, Options, build, build_plot};
    use cadaddy_doc::{Common, Hatch, HatchLoop, Space, Text};
    use cadaddy_geom::{PolyVertex, Vec3};

    fn v3(p: Vec2) -> Vec3 {
        Vec3::new(p.x, p.y, 0.0)
    }

    fn text(at: Vec2, h: f64, s: &str) -> EntityKind {
        EntityKind::Text(Text {
            insert: v3(at),
            align_pt: None,
            height: h,
            value: s.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        })
    }

    fn insert(block: &str, at: Vec2) -> EntityKind {
        EntityKind::Insert(Insert {
            block: block.into(),
            insert: v3(at),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        })
    }

    fn line(a: Vec2, b: Vec2) -> EntityKind {
        EntityKind::Line(cadaddy_doc::Line { a: v3(a), b: v3(b) })
    }

    /// A made-up sheet: a long wall line, a short tick, a label, a hatched area and two symbols
    /// 10 units across (one named for a device, one not).
    fn sheet() -> Drawing {
        let mut d = Drawing::new_imperial();
        for name in ["SYM-OUTLET", "SYM-BOX"] {
            let mut blk = cadaddy_doc::Block::new(name);
            for k in [
                EntityKind::Circle(cadaddy_doc::Circle { center: Vec3::ZERO, radius: 5.0 }),
                line(Vec2::new(-5.0, 0.0), Vec2::new(5.0, 0.0)),
                line(Vec2::new(0.0, -5.0), Vec2::new(0.0, 5.0)),
            ] {
                blk.entities.push(Entity::new(d.new_handle(), k));
            }
            d.blocks.insert(blk.name.clone(), Arc::new(blk));
        }
        let m = Space::Model;
        let c = Common::default;
        let _ = d.add(&m, c(), line(Vec2::new(0.0, 0.0), Vec2::new(1000.0, 0.0)));
        let _ = d.add(&m, c(), line(Vec2::new(10.0, 10.0), Vec2::new(10.5, 10.0)));
        let _ = d.add(&m, c(), text(Vec2::new(100.0, 100.0), 6.0, "ROOM"));
        let rect = [Vec2::new(200.0, 200.0), Vec2::new(400.0, 200.0), Vec2::new(400.0, 400.0), Vec2::new(200.0, 400.0)];
        let _ = d.add(
            &m,
            c(),
            EntityKind::Hatch(Box::new(Hatch {
                pattern: "ANSI31".into(),
                solid: false,
                loops: vec![HatchLoop {
                    curves: Vec::new(),
                    vertices: rect.iter().map(|p| PolyVertex::new(*p)).collect(),
                    outer: true,
                    sources: Vec::new(),
                }],
                scale: 12.0,
                angle: 0.0,
                associative: false,
                style: 0,
                elevation: 0.0,
                gradient: None,
                origin: Vec2::ZERO,
                background: None,
                pattern_lines: Vec::new(),
            })),
        );
        let _ = d.add(&m, c(), insert("SYM-OUTLET", Vec2::new(600.0, 600.0)));
        let _ = d.add(&m, c(), insert("SYM-BOX", Vec2::new(700.0, 600.0)));
        d
    }

    fn rules() -> Arc<PlanRules> {
        Arc::new(
            PlanRules { symbols: vec![SymbolRule { words: vec!["outlet".into()], layers: vec![], tier: Tier::Room }], ..PlanRules::default() }.sane(),
        )
    }

    fn at(px: f64) -> Options {
        Options { tolerance: px * 0.5, min_dash: px * 2.0, detail: Detail::new(px, rules()), ..Options::default() }
    }

    fn handles(d: &Drawing) -> Vec<cadaddy_doc::Handle> {
        d.model.iter().map(|e| e.handle).collect()
    }

    /// The primitives drawn for object `i`.
    fn of(list: &crate::DisplayList, h: cadaddy_doc::Handle) -> Vec<crate::DPrim> {
        list.prims.iter().filter(|p| p.handle == h).copied().collect()
    }

    #[test]
    fn far_out_small_things_draw_as_less_and_close_up_in_full() {
        let d = sheet();
        let hs = handles(&d);
        let (wall, tick, label, hatch, outlet, boxsym) = (hs[0], hs[1], hs[2], hs[3], hs[4], hs[5]);
        // A pixel is 1.2 units: the tick (0.5) is under a pixel, the label 5 px high, the
        // hatch's lines 2.6 px apart (dense enough for a tint, not for solid), the symbols 8 px.
        let far = build(&d, &Space::Model, &at(1.2));
        assert!(of(&far, wall).iter().all(|p| p.kind == Kind::Polyline), "the wall is a line");
        assert_eq!(of(&far, tick).iter().map(|p| p.kind).collect::<Vec<_>>(), vec![Kind::Point], "the tick is a dot");
        assert!(of(&far, hatch).iter().all(|p| p.kind == Kind::Tris && p.transparency > 0), "the hatch is a tint");
        let outlet_far = of(&far, outlet);
        assert_eq!(outlet_far.len(), 1, "the room-tier symbol is its box");
        assert_eq!(outlet_far.first().map(|p| (p.kind, p.len)), Some((Kind::Polyline, 5)));
        assert!(of(&far, boxsym).len() > 1, "a floor-tier symbol of the same size is drawn in full");
        // Text: at 2.5 units a pixel the label is 2.4 px high, a bar.
        let farther = build(&d, &Space::Model, &at(2.5));
        let bar = of(&farther, label);
        assert!(!bar.is_empty() && bar.iter().all(|p| p.kind == Kind::Tris && p.transparency > 0), "the label is a bar: {bar:?}");
        // Close up everything is in full, as without a detail level.
        let near = build(&d, &Space::Model, &at(0.05));
        let full = build(&d, &Space::Model, &Options { detail: None, ..at(0.05) });
        assert_eq!(near.prims, full.prims);
        assert_eq!(near.verts, full.verts);
        assert!(of(&near, outlet).len() > 1, "zoomed in, the outlet's contents come in");
        // And far out the list is much smaller.
        let far_full = build(&d, &Space::Model, &Options { detail: None, ..at(1.2) });
        assert!(
            far.prims.len() + far.verts.len() < (far_full.prims.len() + far_full.verts.len()) / 2,
            "{} / {}",
            far.verts.len(),
            far_full.verts.len()
        );
    }

    #[test]
    fn a_plot_is_always_full_detail() {
        let d = sheet();
        let with = build_plot(&d, &Space::Model, &at(5.0));
        let without = build_plot(&d, &Space::Model, &Options { detail: None, ..at(5.0) });
        assert_eq!(with.prims, without.prims);
        assert_eq!(with.verts, without.verts);
        assert_eq!(with.tris, without.tris);
    }

    #[test]
    fn the_snaps_see_what_is_drawn() {
        let d = sheet();
        let Some(e) = d.model.iter().nth(4) else { panic!("outlet") };
        let det = Detail::new(1.2, rules()).unwrap_or_else(|| panic!("detail"));
        assert!(matches!(det.look(&d, e, &Mat3::IDENTITY), Look::Box(_)));
        let Some(near) = det.at(0.1) else { panic!("detail") };
        assert_eq!(near.look(&d, e, &Mat3::IDENTITY), Look::Full);
        // Scaled up 4x by its context it is big enough again.
        assert_eq!(det.look(&d, e, &Mat3::scale(4.0, 4.0)), Look::Full);
    }
}
