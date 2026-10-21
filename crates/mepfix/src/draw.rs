//! Drawing the sheets: each sheet is a drawing (model space in inches) with its title block,
//! plotted to PDF through the sheet's window and written as DXF.

use crate::design::width_in;
use crate::layout::{Kind, PLAN_MARGIN_IN, frame, paper_to_model, to_model};
use crate::style::Style;
use crate::truth::*;
use cadaddy_color::Color;
use cadaddy_doc::{Arc, Circle, Common, Drawing, EntityKind, HAlign, Layer, Line, Lineweight, LwPolyline, Solid, Space, Text, VAlign};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use std::collections::BTreeMap;
use std::f64::consts::{FRAC_PI_2, PI};

/// A sheet being drawn.
pub struct Pen<'a> {
    pub d: Drawing,
    sh: &'a Sheet,
    st: &'a Style,
    /// Model inches per paper inch.
    k: f64,
    /// While set (enlarged plans): only what lies in this model rectangle is drawn, lines cut at
    /// its edge.
    clip: Option<cadaddy_geom::Bounds2>,
}

/// Layers: name, colour index, pen (hundredths of mm), linetype.
fn layer_def(name: &str, st: &Style) -> (u8, u16, &'static str) {
    match name {
        "BORDER" => (7, st.lw_frame, "Continuous"),
        "TITLE" | "TEXT" | "LEGEND" => (7, st.lw_text, "Continuous"),
        "GRID" => (8, st.lw_background, "CENTER"),
        "GRID-BUBBLE" => (7, st.lw_text, "Continuous"),
        "ARCH" | "ARCH-TEXT" => (8, st.lw_background, "Continuous"),
        "CENTRE" => (8, st.lw_background, "CENTER"),
        "HIDDEN" => (7, st.lw_run, "HIDDEN"),
        "DASHED" => (7, st.lw_run, "DASHED"),
        "HEAVY" => (7, st.lw_run + 20, "Continuous"),
        "LIGHT" => (7, st.lw_background, "Continuous"),
        _ => (7, st.lw_run, "Continuous"),
    }
}

impl<'a> Pen<'a> {
    pub fn new(sh: &'a Sheet, st: &'a Style) -> Pen<'a> {
        let mut d = Drawing::new_imperial();
        for lt in cadaddy_doc::library::standard_linetypes() {
            if ["CENTER", "HIDDEN", "DASHED"].contains(&lt.name.as_str()) && !d.linetypes.iter().any(|l| l.name.eq_ignore_ascii_case(&lt.name)) {
                d.linetypes.push(lt);
            }
        }
        Pen { d, sh, st, k: 1.0 / sh.paper_from_model.scale.max(1e-9), clip: None }
    }

    fn ensure(&mut self, name: &str) {
        if self.d.layers.iter().any(|l| l.name == name) {
            return;
        }
        let (c, lw, lt) = layer_def(name, self.st);
        let mut l = Layer::new(name);
        l.color = Color::Index(c);
        l.lineweight = Lineweight::Mm100(lw);
        l.linetype = lt.into();
        self.d.layers.push(l);
    }

    fn add(&mut self, layer: &str, kind: EntityKind) {
        if let Some(w) = self.clip {
            let inside = |p: Vec3| p.x >= w.min.x && p.x <= w.max.x && p.y >= w.min.y && p.y <= w.max.y;
            let keep = match &kind {
                EntityKind::Line(l) => inside(l.a) && inside(l.b),
                EntityKind::Text(t) => inside(t.insert),
                EntityKind::Circle(c) => inside(c.center),
                EntityKind::Arc(a) => inside(a.center),
                EntityKind::Solid(s) => s.corners.iter().all(|c| inside(*c)),
                EntityKind::LwPolyline(p) => p.vertices.iter().all(|v| inside(v3(v.p))),
                _ => true,
            };
            if !keep {
                // Lines and polylines are cut at the window; anything else outside is left out.
                match kind {
                    EntityKind::Line(l) => {
                        if let Some((a, b)) = clip_line(l.a.xy(), l.b.xy(), w) {
                            self.add_unclipped(layer, EntityKind::Line(Line { a: v3(a), b: v3(b) }));
                        }
                    }
                    EntityKind::LwPolyline(p) => {
                        let n = p.vertices.len();
                        let m = if p.closed { n } else { n.saturating_sub(1) };
                        for i in 0..m {
                            if let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get((i + 1) % n.max(1)))
                                && let Some((a, b)) = clip_line(a.p, b.p, w)
                            {
                                self.add_unclipped(layer, EntityKind::Line(Line { a: v3(a), b: v3(b) }));
                            }
                        }
                    }
                    _ => {}
                }
                return;
            }
        }
        self.add_unclipped(layer, kind);
    }

    fn add_unclipped(&mut self, layer: &str, kind: EntityKind) {
        self.ensure(layer);
        // Linetype dashes about 3/8" long on paper.
        let common = Common { layer: layer.into(), ltscale: self.k * 0.3, ..Common::default() };
        let _ = self.d.add(&Space::Model, common, kind);
    }

    /// Building feet → model.
    pub fn b(&self, p: [f64; 2]) -> Vec2 {
        let m = to_model(self.sh, p);
        Vec2::new(m[0], m[1])
    }

    /// Paper inches → model.
    pub fn p(&self, p: [f64; 2]) -> Vec2 {
        let m = paper_to_model(self.sh, p);
        Vec2::new(m[0], m[1])
    }

    /// A paper length → model.
    pub fn pl(&self, inches: f64) -> f64 {
        inches * self.k
    }

    /// A building length (feet) → model.
    pub fn bl(&self, feet: f64) -> f64 {
        feet * 12.0
    }

    /// The plan's rotation on the sheet, radians.
    pub fn rot(&self) -> f64 {
        self.sh.model_from_building.rotation_deg.to_radians()
    }

    pub fn line(&mut self, layer: &str, a: Vec2, b: Vec2) {
        if (a - b).len() > 1e-9 {
            self.add(layer, EntityKind::Line(Line { a: v3(a), b: v3(b) }));
        }
    }

    pub fn poly(&mut self, layer: &str, pts: &[Vec2], closed: bool) {
        let vertices = pts.iter().map(|p| PolyVertex { p: *p, bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
        self.add(layer, EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: false }));
    }

    pub fn rect(&mut self, layer: &str, a: Vec2, b: Vec2) {
        self.poly(layer, &[a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)], true);
    }

    /// A rectangle given in building feet (it turns with the plan).
    pub fn brect(&mut self, layer: &str, min: [f64; 2], max: [f64; 2]) {
        let pts = [self.b(min), self.b([max[0], min[1]]), self.b(max), self.b([min[0], max[1]])];
        self.poly(layer, &pts, true);
    }

    pub fn circle(&mut self, layer: &str, c: Vec2, r: f64) {
        self.add(layer, EntityKind::Circle(Circle { center: v3(c), radius: r }));
    }

    pub fn arc(&mut self, layer: &str, c: Vec2, r: f64, start: f64, end: f64) {
        self.add(layer, EntityKind::Arc(Arc { center: v3(c), radius: r, start, end }));
    }

    pub fn fill(&mut self, layer: &str, pts: [Vec2; 4]) {
        // SOLID's corner order: 1, 2, 4, 3.
        self.add(layer, EntityKind::Solid(Solid { corners: [v3(pts[0]), v3(pts[1]), v3(pts[3]), v3(pts[2])] }));
    }

    /// Text at a model point; height in paper inches.
    pub fn text(&mut self, layer: &str, at: Vec2, h_in: f64, s: &str, rot: f64, h: HAlign, v: VAlign) {
        if s.trim().is_empty() {
            return;
        }
        let left = h == HAlign::Left && v == VAlign::Baseline;
        self.add(
            layer,
            EntityKind::Text(Text {
                insert: v3(at),
                align_pt: if left { None } else { Some(v3(at)) },
                height: self.pl(h_in),
                value: s.into(),
                rotation: rot,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: h,
                valign: v,
            }),
        );
    }

    /// Lines of text centred on a model point, upright on the paper unless turned by `rot`.
    pub fn block(&mut self, layer: &str, at: Vec2, h_in: f64, lines: &[String], rot: f64) {
        let n = lines.len() as f64;
        let step = self.pl(h_in * 1.4);
        let up = Vec2::new(-rot.sin(), rot.cos());
        for (i, l) in lines.iter().enumerate() {
            let off = ((n - 1.0) / 2.0 - i as f64) * step;
            self.text(layer, at + up * off, h_in, l, rot, HAlign::Center, VAlign::Middle);
        }
    }

    /// Paper-space text (title block, legend): at paper inches.
    pub fn ptext(&mut self, layer: &str, at: [f64; 2], h_in: f64, s: &str, h: HAlign) {
        let p = self.p(at);
        self.text(layer, p, h_in, s, 0.0, h, if h == HAlign::Left { VAlign::Baseline } else { VAlign::Bottom });
    }

    pub fn pline(&mut self, layer: &str, a: [f64; 2], b: [f64; 2]) {
        let (a, b) = (self.p(a), self.p(b));
        self.line(layer, a, b);
    }

    pub fn prect(&mut self, layer: &str, a: [f64; 2], b: [f64; 2]) {
        let (a, b) = (self.p(a), self.p(b));
        self.rect(layer, a, b);
    }
}

/// The part of segment a–b inside `w` (Liang–Barsky).
fn clip_line(a: Vec2, b: Vec2, w: cadaddy_geom::Bounds2) -> Option<(Vec2, Vec2)> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-d.x, a.x - w.min.x), (d.x, w.max.x - a.x), (-d.y, a.y - w.min.y), (d.y, w.max.y - a.y)] {
        if p.abs() < 1e-12 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    (t0 < t1 - 1e-9).then(|| (a + d * t0, a + d * t1))
}

fn v3(p: Vec2) -> Vec3 {
    Vec3::new(p.x, p.y, 0.0)
}

/// Text that reads upright: an angle turned into (-90°, 90°].
fn readable(a: f64) -> f64 {
    let mut a = a.rem_euclid(2.0 * PI);
    if a > FRAC_PI_2 + 1e-9 && a <= 3.0 * FRAC_PI_2 + 1e-9 {
        a -= PI;
    }
    if a > PI {
        a -= 2.0 * PI;
    }
    a
}

/// A text height at most `h` that fits `s` in `width` (paper inches).
fn fit(s: &str, width: f64, h: f64) -> f64 {
    h.min(width / (s.chars().count().max(1) as f64 * 0.9))
}

fn text_width(s: &str, h: f64) -> f64 {
    s.chars().count() as f64 * h * 0.85
}

/// Draw one sheet; returns the drawing with what its legend and abbreviation list say.
pub fn draw(t: &Truth, st: &Style, sh: &Sheet, kind: &Kind) -> (Drawing, Vec<LegendItem>, Vec<LegendItem>) {
    let mut pen = Pen::new(sh, st);
    let mut legend = Vec::new();
    let mut abbr = Vec::new();
    let level = sh.level.clone().unwrap_or_default();
    match kind {
        Kind::Cover => cover(&mut pen, t, st),
        Kind::Schedules(d) => schedules(&mut pen, t, d),
        Kind::Framing(l) => {
            grid(&mut pen, t);
            framing(&mut pen, t, l);
            legend.push(item("column", "STEEL COLUMN"));
            legend.push(item("beam", "STEEL BEAM / GIRDER, SIZE AT MIDSPAN"));
            abbr.extend(["T.O.S. = TOP OF STEEL", "U.N.O. = UNLESS NOTED OTHERWISE", "CONC = CONCRETE"].map(abbrev));
        }
        Kind::Rcp(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            rcp(&mut pen, t, st, l);
            legend.push(item("act", "ACOUSTIC CEILING TILE (ACT) IN GRID"));
            legend.push(item("gyp", "GYPSUM BOARD CEILING (GYP)"));
            legend.push(item("ceiling_tag", &format!("CEILING TAG: {}", st.ceiling_text(9.0, "TYPE").join(" / "))));
            legend.extend(device_legend());
            abbr.extend(["ACT = ACOUSTIC CEILING TILE", "GYP = GYPSUM BOARD", "AFF = ABOVE FINISHED FLOOR", "CLG = CEILING"].map(abbrev));
        }
        Kind::Mech(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            mech(&mut pen, t, st, l);
            for (code, name) in [("SA", "SUPPLY AIR DUCT"), ("RA", "RETURN AIR DUCT"), ("EA", "EXHAUST AIR DUCT"), ("OA", "OUTSIDE AIR DUCT")] {
                legend.push(item(&format!("duct_{}", code.to_lowercase()), &format!("{name} ({})", st.sys(code))));
            }
            legend.extend(device_legend().into_iter().filter(|i| i.symbol != "sprinkler_head"));
            legend.push(item("vav", "VAV TERMINAL UNIT"));
            legend.push(item("duct_riser", "DUCT RISE / DROP"));
            legend.push(item("size_tag", &format!("DUCT SIZE: {} (WIDTH x DEPTH), ROUND {}", st.rect_size(24.0, 12.0), st.round_size(10.0))));
            legend.push(item("height_note", &format!("HEIGHT: {} ABOVE FINISHED FLOOR", st.height_note(10.5, 11.5, 11.0))));
            for code in ["SA", "RA", "EA", "OA"] {
                abbr.push(abbrev(&format!("{} = {}", st.sys(code), t.system(code).map_or("", |s| s.name.as_str()))));
            }
            abbr.extend(
                [
                    "BOD = BOTTOM OF DUCT",
                    "TOD = TOP OF DUCT",
                    "CL = CENTERLINE",
                    "AFF = ABOVE FINISHED FLOOR",
                    "VAV = VARIABLE AIR VOLUME",
                    "AHU = AIR HANDLING UNIT",
                ]
                .map(abbrev),
            );
        }
        Kind::Plumb(l) | Kind::Enlarged(l, _) => {
            if let Kind::Enlarged(_, area) = kind {
                let (lo, hi) = crate::layout::window(t, area);
                let corners = [pen.b(lo), pen.b([hi[0], lo[1]]), pen.b(hi), pen.b([lo[0], hi[1]])];
                pen.clip = Some(cadaddy_geom::Bounds2::from_points(corners));
            }
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            pipes(&mut pen, t, st, l, false, &format!("P-10{l}"));
            if let Kind::Enlarged(..) = kind {
                pen.clip = None;
            } else {
                // Boxes round the enlarged areas, pointing to their sheets.
                let rot = readable(pen.rot());
                for (n, _, _, k) in crate::layout::sheet_list() {
                    let Kind::Enlarged(el, area) = k else { continue };
                    if el != *l {
                        continue;
                    }
                    let (a, b) = crate::layout::window(t, &area);
                    pen.brect("DASHED", a, b);
                    let p = pen.b([a[0] + 0.5, b[1] + 0.8]);
                    pen.text("TEXT", p, st.text_in, &format!("SEE ENLARGED PLAN {n}"), rot, HAlign::Left, VAlign::Bottom);
                }
            }
            for code in ["CW", "HW", "HWR", "SAN", "V", "G"] {
                legend.push(item(
                    &format!("pipe_{}", code.to_lowercase()),
                    &format!("{} ({})", t.system(code).map_or("", |s| s.name.as_str()), st.sys(code)),
                ));
            }
            legend.push(item("pipe_below", "PIPING BELOW FLOOR"));
            legend.push(item("riser_up", "PIPE UP / RISER"));
            legend.push(item("riser_down", "PIPE DOWN"));
            legend.push(item("rise_drop", "RISE / DROP IN PIPE"));
            for code in ["CW", "HW", "HWR", "SAN", "V", "G"] {
                abbr.push(abbrev(&format!("{} = {}", st.sys(code), t.system(code).map_or("", |s| s.name.as_str()))));
            }
            abbr.extend(
                [
                    "BOP = BOTTOM OF PIPE",
                    "CL = CENTERLINE",
                    "AFF = ABOVE FINISHED FLOOR",
                    "VTR = VENT THROUGH ROOF",
                    "WH = WATER HEATER",
                    "GM = GAS METER",
                ]
                .map(abbrev),
            );
        }
        Kind::Fire(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            pipes(&mut pen, t, st, l, true, &format!("FP-10{l}"));
            legend.push(item("pipe_fp_main", "SPRINKLER MAIN"));
            legend.push(item("pipe_fp", "SPRINKLER BRANCH LINE"));
            legend.push(item("sprinkler_head", "PENDENT SPRINKLER HEAD"));
            legend.push(item("riser_up", "RISER UP"));
            legend.push(item("riser_down", "RISER DOWN"));
            legend.push(item("rise_drop", "RISE / DROP IN PIPE"));
            abbr.push(abbrev(&format!("{} = {}", st.sys("FP"), "FIRE PROTECTION")));
            abbr.extend(["BOP = BOTTOM OF PIPE", "CL = CENTERLINE", "AFF = ABOVE FINISHED FLOOR"].map(abbrev));
        }
    }
    if kind.plan() {
        plan_title(&mut pen, t, sh);
    }
    title_block(&mut pen, t, st, sh, &level, &legend, &abbr);
    (pen.d, legend, abbr)
}

fn item(symbol: &str, text: &str) -> LegendItem {
    LegendItem { symbol: symbol.into(), text: text.into() }
}

fn abbrev(s: &str) -> LegendItem {
    let (a, b) = s.split_once(" = ").unwrap_or((s, ""));
    LegendItem { symbol: a.into(), text: b.into() }
}

fn device_legend() -> Vec<LegendItem> {
    vec![
        item("supply_diffuser", "CEILING SUPPLY DIFFUSER (TYPE / CFM)"),
        item("return_grille", "RETURN GRILLE"),
        item("exhaust_grille", "EXHAUST GRILLE"),
        item("sprinkler_head", "SPRINKLER HEAD"),
    ]
}

// ---- Sheet furniture ---------------------------------------------------------------------------

fn title_block(pen: &mut Pen, t: &Truth, st: &Style, sh: &Sheet, level: &str, legend: &[LegendItem], abbr: &[LegendItem]) {
    let fr = frame(st);
    pen.prect("BORDER", fr.border[0], fr.border[1]);
    pen.prect("BORDER", fr.title[0], fr.title[1]);
    let [[x0, y0], [x1, y1]] = fr.title;
    let disc = match sh.discipline.as_str() {
        "M" => "MECHANICAL",
        "P" => "PLUMBING",
        "FP" => "FIRE PROTECTION",
        "S" => "STRUCTURAL",
        "A" => "ARCHITECTURAL",
        _ => "GENERAL",
    };
    let lvl = t.level(level).map(|l| l.name.clone()).unwrap_or_default();
    if st.title_right {
        // Project at the top, legend and abbreviations in the middle, the sheet at the bottom.
        let cx = (x0 + x1) / 2.0;
        pen.ptext("TITLE", [cx, y1 - 0.6], fit(&t.project, x1 - x0 - 0.3, 0.2), &t.project, HAlign::Center);
        pen.ptext("TITLE", [cx, y1 - 0.9], 0.1, "MADE-UP BUILDING - TEST FIXTURE", HAlign::Center);
        pen.ptext("TITLE", [cx, y1 - 1.1], 0.08, &format!("CADADDY MEP FIXTURE SET {}", t.seed), HAlign::Center);
        pen.pline("BORDER", [x0, y1 - 1.3], [x1, y1 - 1.3]);
        let mut y = y1 - 1.65;
        legend_items(pen, st, x0 + 0.2, &mut y, legend, abbr, y0 + 5.2);
        pen.pline("BORDER", [x0, y0 + 5.0], [x1, y0 + 5.0]);
        pen.ptext("TITLE", [x0 + 0.2, y0 + 4.6], 0.1, disc, HAlign::Left);
        if !lvl.is_empty() {
            pen.ptext("TITLE", [x0 + 0.2, y0 + 4.35], 0.1, &lvl, HAlign::Left);
        }
        pen.ptext("TITLE", [x0 + 0.2, y0 + 3.9], fit(&sh.title, x1 - x0 - 0.4, 0.16), &sh.title, HAlign::Left);
        if let Some(s) = &sh.scale_text {
            pen.ptext("TITLE", [x0 + 0.2, y0 + 3.5], 0.1, &format!("SCALE: {s}"), HAlign::Left);
        }
        pen.ptext("TITLE", [x0 + 0.2, y0 + 3.2], 0.08, "ISSUED FOR COORDINATION TESTING", HAlign::Left);
        pen.pline("BORDER", [x0, y0 + 2.9], [x1, y0 + 2.9]);
        pen.ptext("TITLE", [x0 + 0.2, y0 + 2.6], 0.08, "SHEET NUMBER", HAlign::Left);
        pen.ptext("TITLE", [cx, y0 + 1.0], 0.6, &sh.number, HAlign::Center);
    } else {
        // Legend on the left, project in the middle, the sheet box on the right.
        let mid = x0 + (x1 - x0) * 0.62;
        let right = x1 - 4.0;
        pen.pline("BORDER", [mid, y0], [mid, y1]);
        pen.pline("BORDER", [right, y0], [right, y1]);
        let mut y = y1 - 0.3;
        legend_items(pen, st, x0 + 0.2, &mut y, legend, abbr, y0 + 0.15);
        let cx = (mid + right) / 2.0;
        pen.ptext("TITLE", [cx, y1 - 0.5], fit(&t.project, right - mid - 0.3, 0.18), &t.project, HAlign::Center);
        pen.ptext("TITLE", [cx, y1 - 0.8], 0.09, "MADE-UP BUILDING - TEST FIXTURE", HAlign::Center);
        pen.ptext("TITLE", [cx, y1 - 1.25], fit(&sh.title, right - mid - 0.3, 0.14), &sh.title, HAlign::Center);
        pen.ptext("TITLE", [cx, y1 - 1.55], 0.09, &format!("{disc}  {lvl}"), HAlign::Center);
        if let Some(s) = &sh.scale_text {
            pen.ptext("TITLE", [cx, y1 - 1.85], 0.09, &format!("SCALE: {s}"), HAlign::Center);
        }
        pen.ptext("TITLE", [cx, y0 + 0.3], 0.07, &format!("CADADDY MEP FIXTURE SET {}  -  ISSUED FOR COORDINATION TESTING", t.seed), HAlign::Center);
        pen.ptext("TITLE", [right + 0.2, y1 - 0.35], 0.08, "SHEET", HAlign::Left);
        pen.ptext("TITLE", [(right + x1) / 2.0, y0 + 0.9], 0.6, &sh.number, HAlign::Center);
    }
}

/// The legend and abbreviations, top-down from `y`, wrapping to a second column when the bottom
/// is reached (bottom title strip).
fn legend_items(pen: &mut Pen, st: &Style, x: f64, y: &mut f64, legend: &[LegendItem], abbr: &[LegendItem], bottom: f64) {
    let top = *y;
    let mut x = x;
    let col_w = if st.title_right { 4.2 } else { 5.6 };
    let next = |y: &mut f64, x: &mut f64, step: f64| {
        *y -= step;
        if *y < bottom {
            *y = top - step;
            *x += col_w;
        }
    };
    if !legend.is_empty() {
        pen.ptext("LEGEND", [x, *y], 0.11, "LEGEND", HAlign::Left);
        next(y, &mut x, 0.3);
        for it in legend {
            symbol(pen, st, &it.symbol, [x + 0.35, *y + 0.04]);
            pen.ptext("LEGEND", [x + 0.85, *y], 0.07, &it.text, HAlign::Left);
            next(y, &mut x, 0.25);
        }
    }
    if !abbr.is_empty() {
        next(y, &mut x, 0.1);
        pen.ptext("LEGEND", [x, *y], 0.11, "ABBREVIATIONS", HAlign::Left);
        next(y, &mut x, 0.28);
        for it in abbr {
            pen.ptext("LEGEND", [x, *y], 0.07, &format!("{} = {}", it.symbol, it.text), HAlign::Left);
            next(y, &mut x, 0.18);
        }
    }
}

/// A legend symbol centred at paper `c`.
fn symbol(pen: &mut Pen, st: &Style, kind: &str, c: [f64; 2]) {
    let (x, y) = (c[0], c[1]);
    let s = 0.1;
    match kind {
        k if k.starts_with("duct_") && k != "duct_riser" => {
            let layer = "DUCT";
            pen.pline(layer, [x - 0.3, y + 0.05], [x + 0.3, y + 0.05]);
            pen.pline(layer, [x - 0.3, y - 0.05], [x + 0.3, y - 0.05]);
            let label = st.sys(&k.trim_start_matches("duct_").to_uppercase());
            pen.ptext("LEGEND", [x, y - 0.03], 0.05, &label, HAlign::Center);
        }
        "duct_riser" => {
            pen.prect("DUCT", [x - s, y - s], [x + s, y + s]);
            pen.pline("DUCT", [x - s, y - s], [x + s, y + s]);
            pen.pline("DUCT", [x - s, y + s], [x + s, y - s]);
        }
        "supply_diffuser" => {
            pen.prect("DEVICE", [x - s, y - s], [x + s, y + s]);
            pen.pline("DEVICE", [x - s, y - s], [x + s, y + s]);
            pen.pline("DEVICE", [x - s, y + s], [x + s, y - s]);
        }
        "return_grille" => {
            pen.prect("DEVICE", [x - s, y - s], [x + s, y + s]);
            for k in [-0.5, 0.0, 0.5] {
                pen.pline("DEVICE", [x - s, y + k * s], [x + s, y + k * s]);
            }
        }
        "exhaust_grille" => {
            pen.prect("DEVICE", [x - s, y - s], [x + s, y + s]);
            pen.pline("DEVICE", [x - s, y - s], [x + s, y + s]);
        }
        "sprinkler_head" => {
            let c = pen.p([x, y]);
            let r = pen.pl(0.05);
            pen.circle("DEVICE", c, r);
            head_mark(pen, c, r);
        }
        "vav" => pen.prect("EQUIP", [x - 0.2, y - 0.08], [x + 0.2, y + 0.08]),
        "riser_up" | "riser_down" => {
            let c = pen.p([x, y]);
            let r = pen.pl(0.06);
            riser_mark(pen, c, r, if kind == "riser_up" { "up" } else { "down" });
        }
        "rise_drop" => {
            let c = pen.p([x, y]);
            let r = pen.pl(0.04);
            pen.circle("PIPE", c, r);
        }
        "column" => {
            let (a, b) = (pen.p([x - 0.06, y - 0.06]), pen.p([x + 0.06, y + 0.06]));
            pen.fill("STRUCT", [a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)]);
        }
        "beam" => pen.pline("HEAVY", [x - 0.3, y], [x + 0.3, y]),
        "act" => {
            pen.prect("LIGHT", [x - 0.3, y - 0.1], [x + 0.3, y + 0.1]);
            for k in [-0.15, 0.0, 0.15] {
                pen.pline("LIGHT", [x + k, y - 0.1], [x + k, y + 0.1]);
            }
        }
        "gyp" => pen.prect("LIGHT", [x - 0.3, y - 0.1], [x + 0.3, y + 0.1]),
        "ceiling_tag" => pen.prect("TEXT", [x - 0.25, y - 0.08], [x + 0.25, y + 0.08]),
        k if k.starts_with("pipe_") => {
            let code = k.trim_start_matches("pipe_").to_uppercase();
            let layer = match code.as_str() {
                "BELOW" => "HIDDEN",
                "V" => "DASHED",
                "FP_MAIN" => "HEAVY",
                _ => "PIPE",
            };
            let label = match code.as_str() {
                "BELOW" | "FP_MAIN" | "FP" => String::new(),
                c => st.sys(c),
            };
            if label.is_empty() {
                pen.pline(layer, [x - 0.3, y], [x + 0.3, y]);
            } else {
                pen.pline(layer, [x - 0.3, y], [x - 0.12, y]);
                pen.pline(layer, [x + 0.12, y], [x + 0.3, y]);
                pen.ptext("LEGEND", [x, y - 0.025], 0.05, &label, HAlign::Center);
            }
        }
        "size_tag" | "height_note" => {
            pen.pline("TEXT", [x - 0.25, y - 0.06], [x + 0.25, y - 0.06]);
        }
        _ => {}
    }
}

fn plan_title(pen: &mut Pen, t: &Truth, sh: &Sheet) {
    let fr = frame(pen.st);
    let x = fr.area[0][0] + 0.3;
    let y = fr.area[0][1] + 0.55;
    // Title bubble, title, scale.
    let c = pen.p([x + 0.25, y]);
    let r = pen.pl(0.25);
    pen.circle("TEXT", c, r);
    pen.ptext("TEXT", [x + 0.25, y - 0.05], 0.12, "1", HAlign::Center);
    pen.ptext("TEXT", [x + 0.7, y + 0.02], 0.18, &sh.title, HAlign::Left);
    pen.pline("TEXT", [x + 0.7, y - 0.06], [x + 0.7 + text_width(&sh.title, 0.18), y - 0.06]);
    if let Some(s) = &sh.scale_text {
        pen.ptext("TEXT", [x + 0.7, y - 0.3], 0.1, &format!("SCALE: {s}"), HAlign::Left);
        // Scale bar: 0, 4, 8, 16 feet.
        let x0 = x + 0.7 + text_width(&format!("SCALE: {s}"), 0.1) + 0.4;
        let yb = y - 0.3;
        let ft = sh.scale;
        let mut prev = 0.0;
        for (i, d) in [4.0, 8.0, 16.0].iter().enumerate() {
            let (a, b) = (x0 + prev * ft, x0 + d * ft);
            let (pa, pb) = (pen.p([a, yb]), pen.p([b, yb + 0.06]));
            if i % 2 == 0 {
                pen.fill("TEXT", [pa, Vec2::new(pb.x, pa.y), pb, Vec2::new(pa.x, pb.y)]);
            } else {
                pen.rect("TEXT", pa, pb);
            }
            prev = *d;
        }
        for d in [0.0, 4.0, 8.0, 16.0] {
            pen.ptext("TEXT", [x0 + d * ft, yb + 0.12], 0.06, &format!("{d:.0}'"), HAlign::Center);
        }
    }
    // North arrow at the drawing area's upper right, turned with the plan.
    let na = [fr.area[1][0] - 0.6, fr.area[1][1] - 0.6];
    let c = pen.p(na);
    let r = pen.pl(0.3);
    pen.circle("TEXT", c, r);
    let rot = pen.rot();
    let up = Vec2::new(-rot.sin(), rot.cos());
    let side = Vec2::new(up.y, -up.x);
    let tip = c + up * r;
    let base = c - up * (r * 0.6);
    pen.poly("TEXT", &[tip, base + side * (r * 0.4), base - side * (r * 0.4)], true);
    pen.text("TEXT", c + up * (r * 1.4), 0.1, "N", rot, HAlign::Center, VAlign::Middle);
    let _ = t;
}

/// Walls (rooms inset 3", so neighbours make 6" walls), room names and numbers.
fn background(pen: &mut Pen, t: &Truth, level: &str, names: bool) {
    let w = 0.25;
    for rm in t.building.rooms.iter().filter(|r| r.level == level) {
        pen.brect("ARCH", [rm.min_ft[0] + w, rm.min_ft[1] + w], [rm.max_ft[0] - w, rm.max_ft[1] - w]);
        if names {
            let c = [(rm.min_ft[0] + rm.max_ft[0]) / 2.0, rm.min_ft[1] + 0.82 * (rm.max_ft[1] - rm.min_ft[1])];
            let c =
                if rm.name == "CORRIDOR" { [rm.min_ft[0] + 0.12 * (rm.max_ft[0] - rm.min_ft[0]), (rm.min_ft[1] + rm.max_ft[1]) / 2.0] } else { c };
            let p = pen.b(c);
            let rot = readable(pen.rot());
            pen.block("ARCH-TEXT", p, 0.08, &[rm.name.clone(), rm.number.clone()], rot);
        }
    }
    // The building's outline (exterior walls) on top.
    let xs: Vec<f64> = t.building.grid.x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = t.building.grid.y.iter().map(|g| g.at_ft).collect();
    if let (Some(x0), Some(x1), Some(y0), Some(y1)) = (xs.first(), xs.last(), ys.first(), ys.last()) {
        pen.brect("ARCH", [x0 - w, y0 - w], [x1 + w, y1 + w]);
    }
}

/// Grid lines past the building with labelled bubbles at both ends.
fn grid(pen: &mut Pen, t: &Truth) {
    let xs = &t.building.grid.x;
    let ys = &t.building.grid.y;
    let (Some(fx), Some(lx), Some(fy), Some(ly)) = (xs.first(), xs.last(), ys.first(), ys.last()) else { return };
    let scale = pen.sh.scale;
    let ext = 0.9 / scale; // feet past the building: 0.9" on paper
    let r_ft = 0.2 / scale;
    let rot = readable(pen.rot());
    for g in xs {
        let (a, b) = ([g.at_ft, fy.at_ft - ext], [g.at_ft, ly.at_ft + ext]);
        let (pa, pb) = (pen.b(a), pen.b(b));
        pen.line("GRID", pa, pb);
        for (end, d) in [(a, -1.0), (b, 1.0)] {
            let c = pen.b([end[0], end[1] + d * r_ft]);
            let r = pen.bl(r_ft);
            pen.circle("GRID-BUBBLE", c, r);
            pen.text("GRID-BUBBLE", c, 0.14, &g.label, rot, HAlign::Center, VAlign::Middle);
        }
    }
    for g in ys {
        let (a, b) = ([fx.at_ft - ext, g.at_ft], [lx.at_ft + ext, g.at_ft]);
        let (pa, pb) = (pen.b(a), pen.b(b));
        pen.line("GRID", pa, pb);
        for (end, d) in [(a, -1.0), (b, 1.0)] {
            let c = pen.b([end[0] + d * r_ft, end[1]]);
            let r = pen.bl(r_ft);
            pen.circle("GRID-BUBBLE", c, r);
            pen.text("GRID-BUBBLE", c, 0.14, &g.label, rot, HAlign::Center, VAlign::Middle);
        }
    }
    let _ = PLAN_MARGIN_IN;
}

// ---- Mechanical ----------------------------------------------------------------------------------

/// A horizontal run's direction away from a node, on the plan.
fn away(s: &Segment, node: [f64; 3]) -> [f64; 2] {
    let [a, b] = s.path_ft;
    let (from, to) = if (a[0] - node[0]).abs() < 1e-6 && (a[1] - node[1]).abs() < 1e-6 { (a, b) } else { (b, a) };
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let l = (dx * dx + dy * dy).sqrt().max(1e-12);
    [dx / l, dy / l]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn mech(pen: &mut Pen, t: &Truth, st: &Style, level: &str) {
    let sheet = pen.sh.number.clone();
    let segs: Vec<&Segment> = t.segments.iter().filter(|s| s.kind == "duct" && s.sheet == sheet).collect();
    let nodes: BTreeMap<&str, &Node> = t.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let by_id: BTreeMap<&str, &Segment> = t.segments.iter().map(|s| (s.id.as_str(), s)).collect();
    let radius_elbows = st.radius_elbows;
    let mut drawn_tapers = Vec::new();
    for s in segs.iter().filter(|s| !s.vertical) {
        let hw = width_in(&s.shape, s.size_in) / 24.0;
        let mut ends = Vec::new(); // per end: (node point, away dir, trims [left, right])
        for nid in [&s.from, &s.to] {
            let Some(n) = nodes.get(nid.as_str()) else { continue };
            let e = away(s, n.at_ft);
            let nl = [-e[1], e[0]];
            let others: Vec<&Segment> = n
                .segments
                .iter()
                .filter(|id| **id != s.id)
                .filter_map(|id| by_id.get(id.as_str()).copied())
                .filter(|o| !o.vertical && o.kind == "duct")
                .collect();
            let cont = others.iter().find(|o| dot(away(o, n.at_ft), e) < -0.99);
            let side = |sg: f64| others.iter().find(|o| dot(away(o, n.at_ft), nl) * sg > 0.99).copied();
            let mut trims = [0.0, 0.0];
            let mut cap = cont.is_none() && side(1.0).is_none() && side(-1.0).is_none();
            for (k, sg) in [(0, 1.0), (1, -1.0)] {
                if let Some(p) = side(sg) {
                    trims[k] = width_in(&p.shape, p.size_in) / 24.0;
                } else if let Some(c) = cont {
                    let cw = width_in(&c.shape, c.size_in) / 24.0;
                    if (cw - hw).abs() > 1e-6 {
                        trims[k] = 0.5;
                    }
                } else if let Some(p) = side(-sg) {
                    trims[k] = -(width_in(&p.shape, p.size_in) / 24.0);
                }
            }
            // A radius elbow: both runs stop short of the corner and arcs join them.
            let elbow = others.len() == 1 && cont.is_none();
            if elbow
                && radius_elbows
                && let Some(p) = others.first()
            {
                let pw = width_in(&p.shape, p.size_in) / 24.0;
                let r = hw.max(pw) * 1.5;
                if s.length_ft() > r + 0.2 && p.length_ft() > r + 0.2 && (pw - hw).abs() < 1e-6 {
                    trims = [r, r];
                    if s.id < p.id {
                        let v = away(p, n.at_ft);
                        let c = [n.at_ft[0] + (e[0] + v[0]) * r, n.at_ft[1] + (e[1] + v[1]) * r];
                        let a1 = (-v[1]).atan2(-v[0]) + pen.rot();
                        let a2 = (-e[1]).atan2(-e[0]) + pen.rot();
                        let cross = (-v[0]) * (-e[1]) - (-v[1]) * (-e[0]);
                        let (sa, ea) = if cross > 0.0 { (a1, a2) } else { (a2, a1) };
                        let cc = pen.b(c);
                        for rr in [r - hw, r + hw] {
                            let rad = pen.bl(rr);
                            pen.arc("DUCT", cc, rad, sa, ea);
                        }
                    }
                }
            }
            // A transition: tapered sides to the next size, drawn once.
            if let Some(c) = cont {
                let cw = width_in(&c.shape, c.size_in) / 24.0;
                if (cw - hw).abs() > 1e-6 && !drawn_tapers.contains(&n.id) {
                    drawn_tapers.push(n.id.clone());
                    for sg in [1.0, -1.0] {
                        let a = [n.at_ft[0] + e[0] * 0.5 + nl[0] * hw * sg, n.at_ft[1] + e[1] * 0.5 + nl[1] * hw * sg];
                        let b = [n.at_ft[0] - e[0] * 0.5 + nl[0] * cw * sg, n.at_ft[1] - e[1] * 0.5 + nl[1] * cw * sg];
                        let (pa, pb) = (pen.b(a), pen.b(b));
                        pen.line("DUCT", pa, pb);
                    }
                }
            }
            // Ends: a cap line unless equipment or a turn up/down is there.
            let vertical_here = n.segments.iter().filter_map(|id| by_id.get(id.as_str())).any(|o| o.vertical);
            if n.kind == "equipment" || vertical_here {
                cap = false;
            }
            if cap {
                let a = [n.at_ft[0] + nl[0] * hw, n.at_ft[1] + nl[1] * hw];
                let b = [n.at_ft[0] - nl[0] * hw, n.at_ft[1] - nl[1] * hw];
                let (pa, pb) = (pen.b(a), pen.b(b));
                pen.line("DUCT", pa, pb);
            }
            ends.push((n.at_ft, e, trims));
        }
        let [Some(a), Some(b)] = [ends.first(), ends.get(1)] else { continue };
        let nl = [-a.1[1], a.1[0]];
        for (k, sg) in [(0usize, 1.0), (1, -1.0)] {
            // The far end's left is this end's right.
            let ta = a.2[k];
            let tb = b.2[1 - k];
            let pa = [a.0[0] + a.1[0] * ta + nl[0] * hw * sg, a.0[1] + a.1[1] * ta + nl[1] * hw * sg];
            let pb = [b.0[0] + b.1[0] * tb + nl[0] * hw * sg, b.0[1] + b.1[1] * tb + nl[1] * hw * sg];
            let (ma, mb) = (pen.b(pa), pen.b(pb));
            pen.line("DUCT", ma, mb);
        }
        if st.centrelines {
            let (ma, mb) = (pen.b([a.0[0], a.0[1]]), pen.b([b.0[0], b.0[1]]));
            pen.line("CENTRE", ma, mb);
        }
    }
    // Turns up or down (not the drops to diffusers and grilles): a box or circle with a cross.
    let mut marked: Vec<[f64; 2]> = Vec::new();
    for s in segs.iter().filter(|s| s.vertical) {
        let ends_at_device = [&s.from, &s.to].iter().any(|id| nodes.get(id.as_str()).is_some_and(|n| n.kind == "terminal"));
        let at = [s.path_ft[0][0], s.path_ft[0][1]];
        if ends_at_device || marked.iter().any(|m| (m[0] - at[0]).abs() < 1e-6 && (m[1] - at[1]).abs() < 1e-6) {
            continue;
        }
        marked.push(at);
        duct_turn(pen, s, at);
    }
    for r in t.risers.iter().filter(|r| t.system(&r.system).is_some_and(|s| s.kind == "duct")) {
        let Some(m) = r.shown.iter().find(|m| m.level == level) else { continue };
        if !marked.iter().any(|p| (p[0] - r.at_ft[0]).abs() < 1e-6 && (p[1] - r.at_ft[1]).abs() < 1e-6)
            && let Some(s) = r.segments.first().and_then(|id| t.segment(id))
        {
            duct_turn(pen, s, r.at_ft);
        }
        let th = pen.st.text_in / pen.sh.scale;
        let p = pen.b([r.at_ft[0] - 1.2 - th, r.at_ft[1] + 1.2 + th]);
        let q = pen.b([r.at_ft[0] - 0.4, r.at_ft[1] + 0.4]);
        pen.line("TEXT", p, q);
        let rot = readable(pen.rot());
        pen.text("TEXT", p, pen.st.text_in, &m.note, rot, HAlign::Right, VAlign::Bottom);
    }
    terminals(pen, t, level, &["SA", "RA", "EA", "OA"], true);
    for e in t.equipment.iter().filter(|e| e.level == level && ["ahu", "vav"].contains(&e.kind.as_str())) {
        pen.brect("EQUIP", [e.min_ft[0], e.min_ft[1]], [e.max_ft[0], e.max_ft[1]]);
        let th = st.text_in / pen.sh.scale;
        let rot = readable(pen.rot());
        if e.kind == "ahu" {
            // In the unit's south part, clear of the ducts that rise out of its top.
            let c = pen.b([e.min_ft[0] + 0.3 * (e.max_ft[0] - e.min_ft[0]), e.min_ft[1] + th * 1.2]);
            pen.block("TEXT", c, st.text_in * 1.3, std::slice::from_ref(&e.tag), rot);
        } else {
            // VAV tag out to the side on a leader.
            let at = [e.max_ft[0] + th * 6.0, (e.min_ft[1] + e.max_ft[1]) / 2.0];
            let (p, q) = (pen.b([e.max_ft[0], (e.min_ft[1] + e.max_ft[1]) / 2.0]), pen.b([at[0] - th * 0.3, at[1]]));
            pen.line("TEXT", p, q);
            let a = pen.b(at);
            pen.text("TEXT", a, st.text_in, &e.tag, rot, HAlign::Left, VAlign::Middle);
        }
    }
    tags(pen, t, &sheet);
}

fn duct_turn(pen: &mut Pen, s: &Segment, at: [f64; 2]) {
    let (w, d) = if s.shape == "rect" { (s.size_in[0] / 24.0, s.size_in[1] / 24.0) } else { (s.size_in[0] / 24.0, s.size_in[0] / 24.0) };
    if s.shape == "rect" {
        pen.brect("DUCT", [at[0] - w, at[1] - d], [at[0] + w, at[1] + d]);
        for (a, b) in [([at[0] - w, at[1] - d], [at[0] + w, at[1] + d]), ([at[0] - w, at[1] + d], [at[0] + w, at[1] - d])] {
            let (pa, pb) = (pen.b(a), pen.b(b));
            pen.line("DUCT", pa, pb);
        }
    } else {
        let c = pen.b(at);
        let r = pen.bl(w);
        pen.circle("DUCT", c, r);
        for (a, b) in [
            ([at[0] - w * 0.7, at[1] - w * 0.7], [at[0] + w * 0.7, at[1] + w * 0.7]),
            ([at[0] - w * 0.7, at[1] + w * 0.7], [at[0] + w * 0.7, at[1] - w * 0.7]),
        ] {
            let (pa, pb) = (pen.b(a), pen.b(b));
            pen.line("DUCT", pa, pb);
        }
    }
}

/// Air devices and heads of the given systems, with their tags when `tagged`.
fn terminals(pen: &mut Pen, t: &Truth, level: &str, systems: &[&str], tagged: bool) {
    let st = pen.st;
    let scale = pen.sh.scale;
    let th = st.text_in / scale;
    let rot = readable(pen.rot());
    for tm in t.terminals.iter().filter(|x| x.level == level && systems.contains(&x.system.as_str())) {
        let [x, y, _] = tm.at_ft;
        match tm.kind.as_str() {
            "supply_diffuser" | "return_grille" | "exhaust_grille" => {
                let h = tm.face_in[0] / 24.0;
                pen.brect("DEVICE", [x - h, y - h], [x + h, y + h]);
                let lines: Vec<([f64; 2], [f64; 2])> = match tm.kind.as_str() {
                    "supply_diffuser" => vec![([x - h, y - h], [x + h, y + h]), ([x - h, y + h], [x + h, y - h])],
                    "return_grille" => [-0.5, 0.0, 0.5].iter().map(|k| ([x - h, y + k * h], [x + h, y + k * h])).collect(),
                    _ => vec![([x - h, y - h], [x + h, y + h]), ([x - h, y], [x, y + h]), ([x, y - h], [x + h, y])],
                };
                for (a, b) in lines {
                    let (pa, pb) = (pen.b(a), pen.b(b));
                    pen.line("DEVICE", pa, pb);
                }
                if tm.kind == "supply_diffuser" {
                    let i = h * 0.45;
                    pen.brect("DEVICE", [x - i, y - i], [x + i, y + i]);
                }
                if tagged {
                    let mut lines = vec![tm.type_tag.clone()];
                    if let Some(c) = tm.cfm {
                        lines.push(format!("{c:.0}"));
                    }
                    // East of the device (run tags go west of north-south runs).
                    let w = lines.iter().map(|l| text_width(l, th * 0.9)).fold(0.0, f64::max);
                    // Ducted grilles at a main's end: below, clear of their neighbour.
                    let at = if tm.kind == "return_grille" && tm.node.is_some() {
                        pen.b([x, y - h - th * 2.0])
                    } else {
                        pen.b([x + h + th * 0.6 + w / 2.0, y])
                    };
                    pen.block("TEXT", at, st.text_in * 0.9, &lines, rot);
                }
            }
            "louver" => {
                let h = tm.face_in[0] / 24.0;
                pen.brect("DEVICE", [x - h, y - 0.25], [x + h, y + 0.25]);
                for k in [-0.5, 0.0, 0.5] {
                    let (pa, pb) = (pen.b([x + k * h - 0.2, y - 0.25]), pen.b([x + k * h + 0.2, y + 0.25]));
                    pen.line("DEVICE", pa, pb);
                }
                if tagged {
                    let at = pen.b([x, y - 0.25 - th * 2.5]);
                    pen.block("TEXT", at, st.text_in, &[tm.type_tag.clone(), format!("{:.0} CFM", tm.cfm.unwrap_or(0.0))], rot);
                }
            }
            "sprinkler_head" => {
                let c = pen.b([x, y]);
                let r = pen.pl(0.045);
                pen.circle("DEVICE", c, r);
                head_mark(pen, c, r);
            }
            _ => {}
        }
    }
}

fn head_mark(pen: &mut Pen, c: Vec2, r: f64) {
    if pen.st.head_symbol == 0 {
        pen.line("DEVICE", c - Vec2::new(r, 0.0), c + Vec2::new(r, 0.0));
        pen.line("DEVICE", c - Vec2::new(0.0, r), c + Vec2::new(0.0, r));
    } else {
        let q = r * 0.5;
        pen.fill("DEVICE", [c + Vec2::new(-q, -q), c + Vec2::new(q, -q), c + Vec2::new(q, q), c + Vec2::new(-q, q)]);
    }
}

/// Tags of the runs drawn on a sheet: text, and a leader to the run when set.
fn tags(pen: &mut Pen, t: &Truth, sheet: &str) {
    let st = pen.st;
    for s in t.segments.iter().filter(|s| s.sheet == sheet) {
        let Some(tag) = &s.tag else { continue };
        let [a, b] = s.path_ft;
        let ang = (b[1] - a[1]).atan2(b[0] - a[0]) + pen.rot();
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let inside = (tag.at_ft[0] - mid[0]).abs() < 1e-6 && (tag.at_ft[1] - mid[1]).abs() < 1e-6;
        let rot = if st.tags_along || inside { readable(ang) } else { readable(pen.rot()) };
        let at = pen.b(tag.at_ft);
        pen.block("TEXT", at, st.text_in, &tag.text, rot);
        if tag.leader {
            // From the run, square to it under the tag.
            let len = s.length_ft().max(1e-9);
            let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
            let k = ((tag.at_ft[0] - a[0]) * u[0] + (tag.at_ft[1] - a[1]) * u[1]).clamp(0.0, len);
            let foot = [a[0] + u[0] * k, a[1] + u[1] * k];
            let d = [tag.at_ft[0] - foot[0], tag.at_ft[1] - foot[1]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-9);
            let half = width_in(&s.shape, s.size_in) / 24.0;
            let th = st.text_in / pen.sh.scale;
            let start = [foot[0] + d[0] / l * half, foot[1] + d[1] / l * half];
            let end = [tag.at_ft[0] - d[0] / l * (tag.text.len() as f64 * th * 0.8), tag.at_ft[1] - d[1] / l * (tag.text.len() as f64 * th * 0.8)];
            let (p, q) = (pen.b(start), pen.b(end));
            pen.line("TEXT", p, q);
            // Arrowhead at the run.
            let dir = (q - p).normalized();
            let side = Vec2::new(-dir.y, dir.x);
            let ah = pen.pl(0.06);
            pen.poly("TEXT", &[p, p + dir * ah + side * (ah * 0.3), p + dir * ah - side * (ah * 0.3)], true);
        }
    }
}

// ---- Piping -------------------------------------------------------------------------------------

/// Piping of `source` (the level's plan sheet the runs belong to).
fn pipes(pen: &mut Pen, t: &Truth, st: &Style, level: &str, fire: bool, source: &str) {
    let sheet = source.to_string();
    let segs: Vec<&Segment> = t.segments.iter().filter(|s| s.kind == "pipe" && s.sheet == sheet && (s.system == "FP") == fire).collect();
    let nodes: BTreeMap<&str, &Node> = t.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let th_in = st.text_in * 0.85;
    let th = th_in / pen.sh.scale;
    for s in segs.iter().filter(|s| !s.vertical) {
        let layer = if s.below_floor {
            "HIDDEN"
        } else if s.system == "V" {
            "DASHED"
        } else if (fire && s.size_in[0] >= 3.0) || s.system == "SAN" {
            "HEAVY"
        } else {
            "PIPE"
        };
        let [a, b] = s.path_ft;
        let len = s.length_ft();
        let label = if fire { String::new() } else { st.sys(&s.system) };
        let gap = text_width(&label, th) + th;
        let (pa, pb) = (pen.b([a[0], a[1]]), pen.b([b[0], b[1]]));
        if !label.is_empty() && len > gap * 4.0 && s.tag.is_none() {
            // The system's letters in a break at the middle.
            let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
            let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let (g1, g2) = (pen.b([m[0] - u[0] * gap / 2.0, m[1] - u[1] * gap / 2.0]), pen.b([m[0] + u[0] * gap / 2.0, m[1] + u[1] * gap / 2.0]));
            pen.line(layer, pa, g1);
            pen.line(layer, g2, pb);
            let rot = readable(u[1].atan2(u[0]) + pen.rot());
            let c = pen.b(m);
            pen.text("TEXT", c, th_in, &label, rot, HAlign::Center, VAlign::Middle);
        } else {
            pen.line(layer, pa, pb);
        }
    }
    // Rises and drops inside the floor (not to fixtures, heads or equipment).
    let mut marked: Vec<[f64; 2]> = Vec::new();
    for s in segs.iter().filter(|s| s.vertical) {
        let to_device = [&s.from, &s.to].iter().any(|id| {
            nodes.get(id.as_str()).is_some_and(|n| ["terminal", "fixture", "equipment", "service", "cap", "vtr"].contains(&n.kind.as_str()))
        });
        let riser = t.risers.iter().any(|r| r.segments.contains(&s.id));
        let at = [s.path_ft[0][0], s.path_ft[0][1]];
        if to_device || riser || marked.iter().any(|m| (m[0] - at[0]).abs() < 1e-6 && (m[1] - at[1]).abs() < 1e-6) {
            continue;
        }
        marked.push(at);
        let c = pen.b(at);
        let r = pen.pl(0.035);
        pen.circle("PIPE", c, r);
    }
    for rs in t.risers.iter().filter(|r| t.system(&r.system).is_some_and(|s| s.kind == "pipe") && (r.system == "FP") == fire) {
        let Some(m) = rs.shown.iter().find(|m| m.level == level) else { continue };
        let c = pen.b(rs.at_ft);
        let r = pen.pl(0.07);
        riser_mark(pen, c, r, &m.symbol);
        let size = st.pipe_size(rs.size_in[0]);
        let k =
            t.risers.iter().filter(|o| o.system != "EA" && (o.at_ft[1] - rs.at_ft[1]).abs() < 1e-6 && o.at_ft[0] > rs.at_ft[0] + 1e-6).count() as f64;
        // Notes fan out up and to the left so neighbouring risers' notes don't overlap.
        let p = pen.b([rs.at_ft[0] - 0.3, rs.at_ft[1] - 0.3]);
        let q = pen.b([rs.at_ft[0] - 2.0 - th * 2.0, rs.at_ft[1] - 2.0 - th * (2.0 + k * 2.6)]);
        pen.line("TEXT", p, q);
        let rot = readable(pen.rot());
        pen.text("TEXT", q, st.text_in * 0.85, &format!("{size} {}", m.note), rot, HAlign::Right, VAlign::Middle);
    }
    if fire {
        terminals(pen, t, level, &["FP"], false);
    } else {
        for f in t.fixtures.iter().filter(|f| f.level == level) {
            let [x, y, _] = f.at_ft;
            let rot = readable(pen.rot());
            if f.kind == "water_closet" {
                pen.brect("EQUIP", [x - 0.9, y + 0.2], [x + 0.9, y + 0.75]);
                let c = pen.b([x, y - 0.5]);
                let r = pen.bl(0.65);
                pen.circle("EQUIP", c, r);
            } else {
                pen.brect("EQUIP", [x - 0.8, y - 0.6], [x + 0.8, y + 0.75]);
                let c = pen.b([x, y]);
                let r = pen.bl(0.45);
                pen.circle("EQUIP", c, r);
            }
            let at = pen.b([x, y - 1.6 - th]);
            pen.text("TEXT", at, st.text_in * 0.85, &f.tag, rot, HAlign::Center, VAlign::Middle);
        }
        for e in t.equipment.iter().filter(|e| e.level == level && ["water_heater", "gas_meter"].contains(&e.kind.as_str())) {
            let rot = readable(pen.rot());
            let c = [(e.min_ft[0] + e.max_ft[0]) / 2.0, (e.min_ft[1] + e.max_ft[1]) / 2.0];
            if e.kind == "water_heater" {
                let pc = pen.b(c);
                let r = pen.bl((e.max_ft[0] - e.min_ft[0]) / 2.0);
                pen.circle("EQUIP", pc, r);
                let at = pen.b([c[0] + 1.5 + th * 2.0, c[1] + 1.5]);
                pen.text("TEXT", at, st.text_in, &e.tag, rot, HAlign::Left, VAlign::Middle);
            } else {
                pen.brect("EQUIP", [e.min_ft[0], e.min_ft[1]], [e.max_ft[0], e.max_ft[1]]);
                let at = pen.b([e.max_ft[0] + th, c[1] - th * 1.5]);
                pen.text("TEXT", at, st.text_in, &e.tag, rot, HAlign::Left, VAlign::Middle);
            }
        }
        // Where the services come in and go out.
        for n in t.nodes.iter().filter(|n| n.kind == "service" && n.level == level && n.system != "FP") {
            let rot = readable(pen.rot());
            let at = pen.b([n.at_ft[0] + th, n.at_ft[1] + if n.at_ft[1] > 1.0 { th * 2.0 } else { -th * 3.0 }]);
            let what = match n.system.as_str() {
                "SAN" => "TO SITE SEWER",
                "CW" => "FROM SITE WATER",
                _ => "SERVICE",
            };
            pen.text("TEXT", at, st.text_in * 0.85, &format!("{} {what}", st.sys(&n.system)), rot, HAlign::Left, VAlign::Middle);
        }
    }
    if fire {
        for n in t.nodes.iter().filter(|n| n.kind == "service" && n.level == level && n.system == "FP") {
            let rot = readable(pen.rot());
            let at = pen.b([n.at_ft[0] + th * 1.5, n.at_ft[1] - th * 4.0]);
            pen.text("TEXT", at, st.text_in * 0.85, "FIRE SERVICE ENTRY BELOW", rot, HAlign::Left, VAlign::Middle);
        }
    }
    tags(pen, t, &sheet);
}

/// A riser symbol: `up` (circle with a dot or a cross), `down` (circle with a half fill or a
/// line), `through` (both).
fn riser_mark(pen: &mut Pen, c: Vec2, r: f64, kind: &str) {
    pen.circle("PIPE", c, r);
    let up = |pen: &mut Pen| {
        if pen.st.riser_symbols == 0 {
            let q = r * 0.3;
            pen.fill("PIPE", [c + Vec2::new(-q, -q), c + Vec2::new(q, -q), c + Vec2::new(q, q), c + Vec2::new(-q, q)]);
        } else {
            let q = r * 0.7;
            pen.line("PIPE", c + Vec2::new(-q, -q), c + Vec2::new(q, q));
            pen.line("PIPE", c + Vec2::new(-q, q), c + Vec2::new(q, -q));
        }
    };
    let down = |pen: &mut Pen| {
        if pen.st.riser_symbols == 0 {
            pen.fill(
                "PIPE",
                [c + Vec2::new(-r * 0.7, 0.0), c + Vec2::new(r * 0.7, 0.0), c + Vec2::new(r * 0.5, -r * 0.6), c + Vec2::new(-r * 0.5, -r * 0.6)],
            );
        } else {
            pen.line("PIPE", c + Vec2::new(-r, 0.0), c + Vec2::new(r, 0.0));
        }
    };
    match kind {
        "up" => up(pen),
        "down" => down(pen),
        _ => {
            up(pen);
            down(pen);
        }
    }
}

// ---- Ceilings -----------------------------------------------------------------------------------

fn rcp(pen: &mut Pen, t: &Truth, st: &Style, level: &str) {
    let rot = readable(pen.rot());
    let two_by_two = t.seed.is_multiple_of(3);
    for rm in t.building.rooms.iter().filter(|r| r.level == level) {
        let (x0, y0, x1, y1) = (rm.min_ft[0] + 0.25, rm.min_ft[1] + 0.25, rm.max_ft[0] - 0.25, rm.max_ft[1] - 0.25);
        let c = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
        match &rm.ceiling {
            Some(cl) if cl.kind == "ACT" => {
                // A 2x4 (or 2x2) grid centred on the room.
                let (sx, sy) = if two_by_two { (2.0, 2.0) } else { (2.0, 4.0) };
                let mut x = c[0] - ((c[0] - x0) / sx).floor() * sx;
                while x < x1 - 0.05 {
                    if x > x0 + 0.05 {
                        let (a, b) = (pen.b([x, y0]), pen.b([x, y1]));
                        pen.line("LIGHT", a, b);
                    }
                    x += sx;
                }
                let mut y = c[1] - ((c[1] - y0) / sy).floor() * sy;
                while y < y1 - 0.05 {
                    if y > y0 + 0.05 {
                        let (a, b) = (pen.b([x0, y]), pen.b([x1, y]));
                        pen.line("LIGHT", a, b);
                    }
                    y += sy;
                }
            }
            Some(_) => {}
            None => {
                let p = pen.b([c[0], c[1] - 3.0]);
                pen.text("TEXT", p, st.text_in, "OPEN TO STRUCTURE", rot, HAlign::Center, VAlign::Middle);
            }
        }
        if let Some(cl) = &rm.ceiling {
            // The ceiling tag: in a box, a little off the room's centre.
            let lines = st.ceiling_text(cl.height_ft, &cl.kind);
            let at = [c[0] + 2.0, c[1] - 2.5];
            let p = pen.b(at);
            pen.block("TEXT", p, st.text_in, &lines, rot);
            let w = lines.iter().map(|l| text_width(l, st.text_in)).fold(0.0, f64::max) / pen.sh.scale / 2.0 + 0.4;
            let h = lines.len() as f64 * st.text_in * 1.4 / pen.sh.scale / 2.0 + 0.3;
            pen.brect("TEXT", [at[0] - w, at[1] - h], [at[0] + w, at[1] + h]);
        }
    }
    terminals(pen, t, level, &["SA", "RA", "EA", "FP"], false);
}

// ---- Structure ----------------------------------------------------------------------------------

fn framing(pen: &mut Pen, t: &Truth, level: &str) {
    let st = pen.st;
    let rot = readable(pen.rot());
    let th = st.text_in / pen.sh.scale;
    for b in t.building.beams.iter().filter(|b| b.level == level) {
        let (a, c) = (pen.b(b.from_ft), pen.b(b.to_ft));
        pen.line("HEAVY", a, c);
        let mid = [(b.from_ft[0] + b.to_ft[0]) / 2.0, (b.from_ft[1] + b.to_ft[1]) / 2.0];
        let along_x = (b.from_ft[0] - b.to_ft[0]).abs() > 1e-6;
        let ang = if along_x { 0.0 } else { FRAC_PI_2 };
        let off = if along_x { [0.0, th * 0.9] } else { [-th * 0.9, 0.0] };
        let p = pen.b([mid[0] + off[0], mid[1] + off[1]]);
        pen.text("TEXT", p, st.text_in, &b.size, readable(ang + pen.rot()), HAlign::Center, VAlign::Middle);
    }
    for c in &t.building.columns {
        let s = (c.footprint_in[0].max(c.footprint_in[1]) / 24.0).max(0.07 / pen.sh.scale);
        let pts = [
            pen.b([c.at_ft[0] - s, c.at_ft[1] - s]),
            pen.b([c.at_ft[0] + s, c.at_ft[1] - s]),
            pen.b([c.at_ft[0] + s, c.at_ft[1] + s]),
            pen.b([c.at_ft[0] - s, c.at_ft[1] + s]),
        ];
        pen.fill("STRUCT", pts);
    }
    // General notes under the plan's corner.
    let tos = t.building.beams.iter().find(|b| b.level == level).map_or(0.0, |b| b.tos_ft);
    let col = t.building.columns.first().map(|c| c.size.clone()).unwrap_or_default();
    let mut notes = vec!["GENERAL NOTES:".to_string(), format!("1. T.O.S. = EL {} U.N.O.", feet_inches(tos)), format!("2. COLUMNS: {col} U.N.O.")];
    if level == "R" {
        notes.push("3. 1-1/2\" METAL ROOF DECK.".into());
    } else {
        notes.push(format!(
            "3. {}\" CONC SLAB ON 1-1/2\" COMPOSITE DECK; TOP OF SLAB = EL {}.",
            fmt_in(t.building.slab_in),
            feet_inches(t.level(level).map_or(0.0, |l| l.elevation_ft))
        ));
    }
    let fr = frame(st);
    let x = fr.area[1][0] - 5.2;
    let mut y = fr.area[0][1] + 1.0;
    for n in notes.iter().rev() {
        pen.ptext("TEXT", [x, y], 0.09, n, HAlign::Left);
        y += 0.16;
    }
    let _ = rot;
}

// ---- Cover and schedules ------------------------------------------------------------------------

fn cover(pen: &mut Pen, t: &Truth, st: &Style) {
    let fr = frame(st);
    let [[x0, _], [x1, y1]] = fr.area;
    pen.ptext("TITLE", [(x0 + x1) / 2.0, y1 - 1.5], 0.5, &t.project, HAlign::Center);
    pen.ptext("TITLE", [(x0 + x1) / 2.0, y1 - 2.2], 0.2, "MECHANICAL, PLUMBING AND FIRE PROTECTION - COORDINATION SET", HAlign::Center);
    pen.ptext("TITLE", [(x0 + x1) / 2.0, y1 - 2.6], 0.14, "A MADE-UP BUILDING FOR TESTING PRINT READERS. NOT A REAL PROJECT.", HAlign::Center);
    let mut rows = vec![vec!["SHEET".to_string(), "TITLE".to_string()]];
    for (n, title, _, _) in crate::layout::sheet_list() {
        rows.push(vec![n, title]);
    }
    table(pen, [x0 + 0.5, y1 - 3.5], "SHEET INDEX", &rows);
    let notes = [
        "GENERAL NOTES:",
        "1. ELEVATIONS ARE ABOVE THE FINISHED FLOOR OF THE LEVEL SHOWN U.N.O.",
        "2. DUCT SIZES ARE WIDTH x DEPTH, INCHES. ROUND DUCT SIZES ARE DIAMETERS.",
        "3. COORDINATE ALL TRADES IN THE CEILING SPACE BEFORE INSTALLATION.",
        "4. DUCT HEIGHTS NOT NOTED: HANG TIGHT TO STRUCTURE, CENTERLINES ALIGNED.",
    ];
    let mut y = y1 - 3.5;
    for n in notes {
        pen.ptext("TEXT", [(x0 + x1) / 2.0 + 0.5, y], 0.12, n, HAlign::Left);
        y -= 0.25;
    }
    let levels: Vec<String> = t.building.levels.iter().map(|l| format!("{} = EL {}", l.name, feet_inches(l.elevation_ft))).collect();
    y -= 0.3;
    pen.ptext("TEXT", [(x0 + x1) / 2.0 + 0.5, y], 0.12, "LEVELS:", HAlign::Left);
    for l in levels {
        y -= 0.25;
        pen.ptext("TEXT", [(x0 + x1) / 2.0 + 0.5, y], 0.12, &l, HAlign::Left);
    }
}

fn schedules(pen: &mut Pen, t: &Truth, disc: &str) {
    let fr = frame(pen.st);
    let [[x0, _], [_, y1]] = fr.area;
    let mut y = y1 - 0.5;
    let mut x = x0 + 0.3;
    let groups: Vec<(&str, Vec<&str>)> = if disc == "M" {
        vec![("AIR HANDLING UNIT SCHEDULE", vec!["ahu"]), ("VAV TERMINAL UNIT SCHEDULE", vec!["vav"]), ("EXHAUST FAN SCHEDULE", vec!["exhaust_fan"])]
    } else {
        vec![("WATER HEATER SCHEDULE", vec!["water_heater"]), ("GAS METER", vec!["gas_meter"])]
    };
    for (title, kinds) in groups {
        let eq: Vec<&Equipment> = t.equipment.iter().filter(|e| kinds.contains(&e.kind.as_str())).collect();
        let Some(first) = eq.first() else { continue };
        let mut cols: Vec<String> = vec!["TAG".into()];
        cols.extend(first.schedule.keys().filter(|k| k.as_str() != "TAG").cloned());
        let mut rows = vec![cols.clone()];
        let mut seen = Vec::new();
        for e in eq {
            if seen.contains(&e.tag) {
                continue;
            }
            seen.push(e.tag.clone());
            rows.push(cols.iter().map(|c| e.schedule.get(c).cloned().unwrap_or_default()).collect());
        }
        let h = table(pen, [x, y], title, &rows);
        y -= h + 0.6;
        if y < fr.area[0][1] + 3.0 {
            y = y1 - 0.5;
            x += 13.0;
        }
    }
    if disc == "M" {
        let mut rows = vec![vec!["TAG".to_string(), "TYPE".to_string(), "FACE (IN)".to_string(), "NECK (IN)".to_string(), "MOUNTING".to_string()]];
        let mut seen: Vec<String> = Vec::new();
        let mut terms: Vec<&Terminal> = t.terminals.iter().filter(|x| x.system != "FP").collect();
        terms.sort_by(|a, b| a.type_tag.cmp(&b.type_tag));
        for tm in terms {
            if seen.contains(&tm.type_tag) {
                continue;
            }
            seen.push(tm.type_tag.clone());
            let ty = match tm.kind.as_str() {
                "supply_diffuser" => "LOUVERED FACE SUPPLY DIFFUSER",
                "return_grille" if tm.node.is_some() => "RETURN GRILLE, DUCTED",
                "return_grille" => "RETURN GRILLE, PLENUM",
                "exhaust_grille" => "EXHAUST GRILLE",
                _ => "INTAKE LOUVER",
            };
            let face = format!("{}x{}", fmt_in(tm.face_in[0]), fmt_in(tm.face_in[1]));
            let neck = if tm.neck_in > 0.0 { fmt_in(tm.neck_in) } else { "-".into() };
            let mount = if tm.kind == "louver" { "WALL" } else { "LAY-IN / CEILING" };
            rows.push(vec![tm.type_tag.clone(), ty.into(), face, neck, mount.into()]);
        }
        table(pen, [x, y], "AIR DEVICE SCHEDULE", &rows);
    } else {
        let rows = vec![
            vec!["TAG".to_string(), "FIXTURE".to_string(), "CW".to_string(), "HW".to_string(), "SAN".to_string(), "V".to_string()],
            vec!["WC-1".into(), "WATER CLOSET, WALL HUNG, FLUSH VALVE".into(), "1\"".into(), "-".into(), "3\"".into(), "2\"".into()],
            vec!["LAV-1".into(), "LAVATORY, WALL HUNG".into(), "1/2\"".into(), "1/2\"".into(), "1-1/2\"".into(), "1-1/2\"".into()],
        ];
        table(pen, [x, y], "PLUMBING FIXTURE SCHEDULE", &rows);
    }
}

/// A table with a title, top-left at paper `at`; returns its height, inches.
fn table(pen: &mut Pen, at: [f64; 2], title: &str, rows: &[Vec<String>]) -> f64 {
    let h = 0.09;
    let row_h = 0.24;
    let ncols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<f64> = (0..ncols).map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| text_width(s, h)).fold(0.6, f64::max) + 0.25).collect();
    let total: f64 = widths.iter().sum();
    pen.ptext("TEXT", [at[0], at[1] + 0.1], 0.13, title, HAlign::Left);
    let mut y = at[1];
    for (i, r) in rows.iter().enumerate() {
        let mut x = at[0];
        for (c, w) in widths.iter().enumerate() {
            pen.prect(if i == 0 { "BORDER" } else { "TEXT" }, [x, y - row_h], [x + w, y]);
            if let Some(s) = r.get(c) {
                pen.ptext("TEXT", [x + 0.1, y - row_h + 0.075], h, s, HAlign::Left);
            }
            x += w;
        }
        y -= row_h;
    }
    let _ = total;
    at[1] - y + 0.2
}
