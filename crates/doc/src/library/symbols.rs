//! CADaddy's built-in symbol library: plan-view symbols placed as blocks (SYMBOL command, the
//! Symbols palette). Every symbol is drawn here in code, from common drafting conventions; no
//! outside block library is copied. Each one carries smart attributes: a visible tag or size the
//! user fills in, and constant hidden `TRADE` and `TYPE` values that counts and schedules group by.
//!
//! Sizes are inches. A *fixture* is drawn at its real size, its insertion point at the middle of
//! its back edge (the wall), extending in −Y. An *annotation* symbol (valves, cleanouts, arrows)
//! is drawn at its plotted size, centred on its insertion point with its pipe along X, and placed
//! at the drawing's scale (see [`Symbol::annotation`]).

use cadaddy_geom::{PolyVertex, Vec2, Vec3};

use crate::{Attrib, Circle, Ellipse, Entity, EntityKind, HAlign, Handle, Line, LwPolyline, Text, VAlign};

mod electrical;
mod plumbing;

/// One library symbol.
#[derive(Clone, Copy, Debug)]
pub struct Symbol {
    /// The block name it is placed as (`PL_WATER_CLOSET`).
    pub name: &'static str,
    /// What the palette shows ("Water closet (tank)").
    pub label: &'static str,
    /// Its group in the palette ("Plumbing fixtures").
    pub category: &'static str,
    /// Trade it belongs to (the hidden `TRADE` attribute): "PLUMBING".
    pub trade: &'static str,
    /// Drawn at plotted size and placed at the drawing's scale (annotation symbols); otherwise
    /// drawn at real size and placed 1:1.
    pub annotation: bool,
    /// Words that find it in the palette's search besides its label.
    pub keywords: &'static str,
    draw: fn(&mut Sketch),
}

impl Symbol {
    /// The symbol's objects in block coordinates (insertion point at the origin), with handle 0:
    /// the drawing gives them handles when the block is defined.
    pub fn entities(&self) -> Vec<Entity> {
        let mut k = Sketch::default();
        (self.draw)(&mut k);
        // The type is the name without its trade prefix (PL_, EL_).
        let ty = self.name.split_once('_').map_or(self.name, |(_, rest)| rest);
        let hidden = if self.annotation { 0.05 } else { 2.0 };
        k.attdef_constant("TRADE", self.trade, Vec2::new(0.0, 0.0), hidden);
        k.attdef_constant("TYPE", ty, Vec2::new(0.0, -hidden * 1.5), hidden);
        k.out
    }
    /// Whether `query` (words, any case) finds this symbol: every word in its name, label,
    /// category or keywords.
    pub fn matches(&self, query: &str) -> bool {
        let hay = format!("{} {} {} {}", self.name, self.label, self.category, self.keywords).to_ascii_lowercase();
        query.split_whitespace().all(|w| hay.contains(&w.to_ascii_lowercase()))
    }
}

/// Every library symbol, palette order: electrical, then plumbing.
pub fn symbols() -> &'static [Symbol] {
    static ALL: std::sync::LazyLock<Vec<Symbol>> =
        std::sync::LazyLock::new(|| electrical::SYMBOLS.iter().chain(plumbing::SYMBOLS).copied().collect());
    &ALL
}

/// The library symbol named `name` (its block name, any case).
pub fn symbol(name: &str) -> Option<&'static Symbol> {
    let n = name.trim();
    symbols().iter().find(|s| s.name.eq_ignore_ascii_case(n))
}

/// Builds a symbol's objects.
#[derive(Default)]
struct Sketch {
    out: Vec<Entity>,
}

fn p3(p: Vec2) -> Vec3 {
    Vec3::new(p.x, p.y, 0.0)
}

impl Sketch {
    fn add(&mut self, kind: EntityKind) {
        self.out.push(Entity::new(Handle(0), kind));
    }
    fn line(&mut self, a: (f64, f64), b: (f64, f64)) {
        self.add(EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) }));
    }
    fn circle(&mut self, c: (f64, f64), r: f64) {
        self.add(EntityKind::Circle(Circle { center: Vec3::new(c.0, c.1, 0.0), radius: r }));
    }
    /// A full ellipse with half-widths `rx` (along X) and `ry`.
    fn ellipse(&mut self, c: (f64, f64), rx: f64, ry: f64) {
        let (major, ratio) = if rx >= ry { (Vec3::new(rx, 0.0, 0.0), ry / rx) } else { (Vec3::new(0.0, ry, 0.0), rx / ry) };
        self.add(EntityKind::Ellipse(Ellipse { center: Vec3::new(c.0, c.1, 0.0), major, ratio, start: 0.0, end: std::f64::consts::TAU }));
    }
    /// A polyline through `pts` ((x, y, bulge to the next point)).
    fn pline(&mut self, pts: &[(f64, f64, f64)], closed: bool, width: f64) {
        let vertices = pts.iter().map(|&(x, y, b)| PolyVertex { p: Vec2::new(x, y), bulge: b, start_width: width, end_width: width }).collect();
        self.add(EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: width, elevation: 0.0, plinegen: false }));
    }
    fn rect(&mut self, a: (f64, f64), b: (f64, f64)) {
        self.pline(&[(a.0, a.1, 0.0), (b.0, a.1, 0.0), (b.0, b.1, 0.0), (a.0, b.1, 0.0)], true, 0.0);
    }
    /// A filled dot of radius `r` (a donut with no hole).
    fn dot(&mut self, c: (f64, f64), r: f64) {
        let h = r / 2.0;
        self.pline(&[(c.0 - h, c.1, 1.0), (c.0 + h, c.1, 1.0)], true, r);
    }
    fn text(&mut self, at: (f64, f64), height: f64, value: &str) {
        let p = Vec3::new(at.0, at.1, 0.0);
        self.add(EntityKind::Text(Text {
            insert: p,
            align_pt: Some(p),
            height,
            value: value.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::Center,
            valign: VAlign::Middle,
        }));
    }
    fn attrib(tag: &str, prompt: &str, default: &str, at: Vec2, height: f64) -> Attrib {
        Attrib {
            tag: tag.into(),
            text: Text {
                insert: p3(at),
                align_pt: None,
                height,
                value: default.into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: HAlign::Left,
                valign: VAlign::Baseline,
            },
            invisible: false,
            constant: false,
            prompt: prompt.into(),
            props: Default::default(),
            multiline: None,
            modes: Default::default(),
        }
    }
    /// A visible attribute the user fills in when placing the symbol.
    fn attdef(&mut self, tag: &str, prompt: &str, default: &str, at: (f64, f64), height: f64) {
        let a = Self::attrib(tag, prompt, default, Vec2::new(at.0, at.1), height);
        self.add(EntityKind::AttDef(a));
    }
    /// An attribute the user fills in that doesn't show on the plan (PANEL, MOUNT_HT): for
    /// schedules, counts and extraction.
    fn attdef_hidden(&mut self, tag: &str, prompt: &str, default: &str, at: (f64, f64), height: f64) {
        let mut a = Self::attrib(tag, prompt, default, Vec2::new(at.0, at.1), height);
        a.invisible = true;
        self.add(EntityKind::AttDef(a));
    }
    /// A constant, hidden attribute (not asked for): what counts and schedules group by.
    fn attdef_constant(&mut self, tag: &str, value: &str, at: Vec2, height: f64) {
        let mut a = Self::attrib(tag, "", value, at, height);
        a.invisible = true;
        a.constant = true;
        self.add(EntityKind::AttDef(a));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_symbol_draws_tags_and_has_a_unique_name() {
        let mut names = std::collections::HashSet::new();
        assert!(symbols().len() >= 60);
        for s in symbols() {
            assert!(names.insert(s.name), "duplicate {}", s.name);
            assert!((s.name.starts_with("PL_") || s.name.starts_with("EL_")) && s.name == s.name.to_ascii_uppercase(), "{}", s.name);
            let es = s.entities();
            let drawn = es.iter().filter(|e| !matches!(e.kind, EntityKind::AttDef(_))).count();
            assert!(drawn >= 1, "{} draws nothing", s.name);
            let tags: Vec<&str> = es.iter().filter_map(|e| if let EntityKind::AttDef(a) = &e.kind { Some(a.tag.as_str()) } else { None }).collect();
            assert!(tags.contains(&"TRADE") && tags.contains(&"TYPE"), "{} tags {tags:?}", s.name);
            assert!(tags.iter().any(|t| ["TAG", "SIZE", "CIRCUIT", "MOUNT_HT"].contains(t)), "{} has nothing to fill in: {tags:?}", s.name);
            // Every coordinate is a real number of sensible size.
            let d = crate::Drawing::new_imperial();
            let b = es.iter().fold(cadaddy_geom::Bounds2::EMPTY, |b, e| b.union(&crate::extents::entity_bounds(&d, e, 1)));
            assert!(b.min.is_finite() && b.max.is_finite() && b.width() < 200.0 && b.height() < 200.0, "{} bounds {b:?}", s.name);
        }
        assert!(symbol("pl_water_closet").is_some());
        assert!(symbol("nope").is_none());
    }

    #[test]
    fn search_finds_by_label_and_keywords() {
        let found: Vec<&str> = symbols().iter().filter(|s| s.matches("toilet")).map(|s| s.name).collect();
        assert!(found.contains(&"PL_WATER_CLOSET"), "{found:?}");
        assert!(symbols().iter().any(|s| s.matches("valve ball")));
        assert!(!symbols().iter().any(|s| s.matches("zzzz")));
    }
}
