//! Drawing the sheets: each sheet is a drawing (model space in inches) with its title block,
//! plotted to PDF through the sheet's window and written as DXF.

use crate::design::width_in;
use crate::layout::{Kind, PLAN_MARGIN_IN, SITE_LAYER, UNDERGROUND_LAYER, frame, paper_to_model, to_model};
use crate::sitework::{EXISTING_GRADING_LAYER, FENCE_LAYER, FURNISH_LAYER, GRADING_LAYER, LANDSCAPE_LAYER, STRUCTURE_LAYER};
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
    pub(crate) sh: &'a Sheet,
    pub(crate) st: &'a Style,
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
        "CALLOUT" => (7, st.lw_text, "Continuous"),
        "A-SLAB-EDGE" => (8, st.lw_background, "DASHED"),
        "C-SITE" => (7, st.lw_run, "Continuous"),
        "C-UNDR" => (7, st.lw_run, "DASHED"),
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
pub(crate) fn readable(a: f64) -> f64 {
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
        Kind::WallTypes => {
            let fr = frame(st);
            crate::walls::draw_legend(&mut pen, t, &st.walls, fr.area);
            legend.push(item("wall_tag", "WALL TYPE TAG"));
            abbr.extend(
                [
                    "GYP. BD. = GYPSUM BOARD",
                    "GWB = GYPSUM WALLBOARD",
                    "MR = MOISTURE-RESISTANT",
                    "AR = ABUSE-RESISTANT",
                    "CMU = CONCRETE MASONRY UNIT",
                    "GA. = GAUGE",
                    "O.C. = ON CENTER",
                    "U/S = UNDERSIDE",
                    "STC = SOUND TRANSMISSION CLASS",
                ]
                .map(abbrev),
            );
        }
        Kind::CeilingDetails => {
            let fr = frame(st);
            crate::ceilings::draw_details(&mut pen, t, &st.walls, &st.ceilings, fr.area);
            legend.push(item("ceiling_type", "CEILING TYPE TAG"));
            legend.push(item("detail", "DETAIL NUMBER"));
            abbr.extend(
                [
                    "ACT = ACOUSTIC CEILING TILE",
                    "GYP. BD. = GYPSUM BOARD",
                    "GWB = GYPSUM WALLBOARD",
                    "MR = MOISTURE-RESISTANT",
                    "GA. = GAUGE",
                    "O.C. = ON CENTER",
                    "CLG = CEILING",
                ]
                .map(abbrev),
            );
        }
        Kind::FinishSchedule => {
            let fr = frame(st);
            crate::ceilings::draw_schedule(&mut pen, t, &st.ceilings, fr.area);
            abbr.extend(
                [
                    "CPT = CARPET",
                    "VCT = VINYL COMPOSITION TILE",
                    "CT = CERAMIC TILE",
                    "SC = SEALED CONCRETE",
                    "RB = RUBBER BASE",
                    "PT = PAINT",
                    "EPX = EPOXY PAINT",
                ]
                .map(abbrev),
            );
        }
        Kind::Floor(l) => {
            grid(&mut pen, t);
            crate::walls::draw_plan(&mut pen, t, &st.walls, l, sh.scale, false);
            crate::arch::draw_floor(&mut pen, t, &st.arch, l, sh.scale);
            casework_plan(&mut pen, t, st, l);
            fixture_plan(&mut pen, t, st, l);
            casework_schedule(&mut pen, t, l);
            // True device positions on the A-sheet (small crosses).
            draw_devices_arch(&mut pen, t, l);
            legend.push(item("base_cabinet", "BASE CABINET"));
            legend.push(item("upper_cabinet", "UPPER CABINET"));
            legend.push(item("counter", "COUNTERTOP"));
            legend.push(item("vanity", "VANITY"));
            legend.push(item("appliance", "APPLIANCE"));
            legend.push(item("wall_tag", "WALL TYPE TAG, SEE PARTITION TYPES"));
            legend.push(item("door", "DOOR"));
            if t.arch.is_some() {
                legend.push(item("door_tag", "DOOR MARK, SEE DOOR SCHEDULE"));
                legend.push(item("window_tag", "WINDOW TYPE, SEE WINDOW SCHEDULE"));
                legend.push(item("slab_edge", "SLAB EDGE"));
            }
            abbr.extend(["AFF = ABOVE FINISHED FLOOR", "TYP = TYPICAL", "U.N.O. = UNLESS NOTED OTHERWISE"].map(abbrev));
        }
        Kind::EnlargedFloor(l, area) => {
            let (lo, hi) = crate::layout::window(t, area);
            let corners = [pen.b(lo), pen.b([hi[0], lo[1]]), pen.b(hi), pen.b([lo[0], hi[1]])];
            pen.clip = Some(cadaddy_geom::Bounds2::from_points(corners));
            grid(&mut pen, t);
            crate::walls::draw_plan(&mut pen, t, &st.walls, l, sh.scale, true);
            pen.clip = None;
            legend.push(item("wall_tag", "WALL TYPE TAG, SEE PARTITION TYPES"));
            legend.push(item("door", "DOOR"));
        }
        Kind::Roof => {
            grid(&mut pen, t);
            crate::arch::draw_roof(&mut pen, t, &st.arch);
            legend.push(item("roof_drain", "ROOF DRAIN"));
            legend.push(item("curb", "EQUIPMENT CURB"));
            abbr.extend(["RD = ROOF DRAIN", "EF = EXHAUST FAN", "ELEV. = ELEVATOR"].map(abbrev));
        }
        Kind::RoofDrainage => {
            grid(&mut pen, t);
            crate::envelope::draw_drainage(&mut pen, t);
            legend.push(item("roof_drain", "ROOF DRAIN"));
            legend.push(item("overflow", "OVERFLOW SCUPPER / DRAIN"));
            legend.push(item("cricket", "TAPERED CRICKET / SADDLE"));
            legend.push(item("walkway", "WALKWAY PADS"));
            abbr.extend(
                ["RD = ROOF DRAIN", "OS = OVERFLOW SCUPPER", "OD = OVERFLOW DRAIN", "VTR = VENT THROUGH ROOF", "MIN. = MINIMUM", "GA. = GAUGE"]
                    .map(abbrev),
            );
        }
        Kind::Assemblies => {
            let fr = frame(st);
            crate::envelope::draw_assemblies(&mut pen, t, &st.envelope, fr.area);
            abbr.extend(
                [
                    "EW = EXTERIOR WALL",
                    "CI = CONTINUOUS INSULATION",
                    "EIFS = EXTERIOR INSULATION AND FINISH SYSTEM",
                    "TPO = THERMOPLASTIC POLYOLEFIN",
                    "EL. = ELEVATION",
                    "THK. = THICKNESS",
                ]
                .map(abbrev),
            );
        }
        Kind::DoorSchedule => {
            let fr = frame(st);
            crate::arch::draw_door_schedule(&mut pen, t, &st.arch, fr.area);
            abbr.extend(["HM = HOLLOW METAL", "WD = WOOD", "AL = ALUMINUM", "MIN = MINUTES", "INSUL. = INSULATED"].map(abbrev));
        }
        Kind::UnitPlans => {
            let fr = frame(st);
            crate::units::draw_unit_plans(&mut pen, t, fr.area);
            abbr.extend(["PTAC = PACKAGED TERMINAL AIR CONDITIONER", "ADA = ACCESSIBLE UNIT", "QTY = QUANTITY"].map(abbrev));
        }
        Kind::ArchFinishes => {
            let fr = frame(st);
            crate::arch::draw_finish_schedule(&mut pen, t, &st.arch, fr.area);
            abbr.extend(["CLG. = CEILING", "HT. = HEIGHT", "ACT = ACOUSTIC CEILING TILE", "GYP = GYPSUM BOARD"].map(abbrev));
        }
        Kind::Schedules(d) => schedules(&mut pen, t, d),
        Kind::Framing(l) => {
            grid(&mut pen, t);
            framing(&mut pen, t, l);
            legend.push(item("column", "STEEL COLUMN"));
            legend.push(item("beam", "STEEL BEAM / GIRDER, SIZE AT MIDSPAN"));
            legend.push(item("opening", "FLOOR OPENING / SHAFT"));
            abbr.extend(["T.O.S. = TOP OF STEEL", "U.N.O. = UNLESS NOTED OTHERWISE", "CONC = CONCRETE", "EL = ELEVATION"].map(abbrev));
        }
        Kind::Rcp(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            if sh.title.contains("FLOOR PLAN") {
                crate::units::draw_unit_tags(&mut pen, t, l);
            }
            rcp(&mut pen, t, st, l);
            crate::ceilings::draw_callouts(&mut pen, t, &st.ceilings, l);
            // The hotel's floor plans are drawn as ceiling plans with the casework, fixtures and
            // their schedule on them.
            if sh.number.starts_with("A-10") && sh.title.contains("FLOOR PLAN") && !sh.title.contains("CEILING") {
                casework_plan(&mut pen, t, st, l);
                fixture_plan(&mut pen, t, st, l);
                casework_schedule(&mut pen, t, l);
            }
            legend.push(item("act", "ACOUSTIC CEILING TILE (ACT) IN GRID"));
            legend.push(item("gyp", "GYPSUM BOARD CEILING (GYP)"));
            legend.push(item("ceiling_tag", &format!("CEILING TAG: {}", st.ceiling_text(9.0, "TYPE").join(" / "))));
            legend.push(item("soffit", "SOFFIT / BULKHEAD (DASHED)"));
            legend.extend(device_legend());
            legend.push(item("sprinkler_upright", "UPRIGHT SPRINKLER HEAD (OPEN TO STRUCTURE)"));
            if t.terminals.iter().any(|x| x.mount.as_deref() == Some("sidewall")) {
                legend.push(item("sprinkler_sidewall", "SIDEWALL SPRINKLER HEAD"));
            }
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
        Kind::Elec(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            draw_devices(&mut pen, t, l);
            legend.push(item("receptacle", "DUPLEX RECEPTACLE"));
            legend.push(item("switch", "SWITCH"));
            abbr.extend(["AFF = ABOVE FINISHED FLOOR", "R = RECEPTACLE", "S = SWITCH"].map(abbrev));
        }
        Kind::Fire(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            pipes(&mut pen, t, st, l, true, &format!("FP-10{l}"));
            legend.push(item("pipe_fp_main", "SPRINKLER MAIN"));
            legend.push(item("pipe_fp", "SPRINKLER BRANCH LINE"));
            legend.push(item("sprinkler_head", "PENDENT SPRINKLER HEAD (SH-1)"));
            legend.push(item("sprinkler_upright", &format!("UPRIGHT SPRINKLER HEAD ON {}\" SPRIG (SH-2)", t.fp_sprig_in.unwrap_or(4.0) as u32)));
            if t.terminals.iter().any(|x| x.mount.as_deref() == Some("sidewall")) {
                legend.push(item(
                    "sprinkler_sidewall",
                    &format!("SIDEWALL SPRINKLER HEAD, {}\" BELOW CEILING (SH-3)", t.fp_sidewall_below_in.unwrap_or(4.0) as u32),
                ));
            }
            legend.push(item("riser_up", "RISER UP"));
            legend.push(item("riser_down", "RISER DOWN"));
            legend.push(item("rise_drop", "RISE / DROP IN PIPE"));
            fp_schedules(&mut pen, t);
            abbr.push(abbrev(&format!("{} = {}", st.sys("FP"), "FIRE PROTECTION")));
            abbr.extend(["BOP = BOTTOM OF PIPE", "CL = CENTERLINE", "AFF = ABOVE FINISHED FLOOR"].map(abbrev));
        }
        Kind::Lighting(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            elec_devices(&mut pen, t, st, l, &format!("E-10{l}"), true);
            legend.push(item("light_2x4", "2X4 RECESSED TROFFER"));
            legend.push(item("light_2x2", "2X2 RECESSED TROFFER"));
            legend.push(item("light_down", "RECESSED DOWNLIGHT"));
            legend.push(item("elec_switch", "LIGHT SWITCH"));
            legend.push(item("exit_sign", "EXIT SIGN"));
            legend.push(item("emergency_light", "EMERGENCY LIGHT"));
            legend.push(item("junction_box", "JUNCTION BOX"));
            legend.push(item("homerun", "HOMERUN TO PANEL (CIRCUIT NO.)"));
            legend.push(item("conduit", "CONDUIT RUN (SIZE)"));
            legend.push(item("elec_panel", "PANELBOARD"));
            abbr.extend(
                ["CLG = CEILING", "AFF = ABOVE FINISHED FLOOR", "C = CONDUIT", "EMT = ELECTRICAL METALLIC TUBING", "MC = METAL CLAD CABLE"]
                    .map(abbrev),
            );
        }
        Kind::Power(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            elec_devices(&mut pen, t, st, l, &format!("E-20{l}"), false);
            trays(&mut pen, t, l);
            legend.push(item("cable_tray", "CABLE TRAY (WIDTH x DEPTH), LADDER"));
            legend.push(item("conduit_bank", "FEEDER CONDUIT BANK ((QTY) SIZE)"));
            legend.push(item("riser_up", "CONDUIT UP / DOWN"));
            legend.push(item("receptacle", "DUPLEX RECEPTACLE"));
            legend.push(item("data_outlet", "DATA/TELECOM OUTLET"));
            legend.push(item("floor_box", "FLOOR BOX"));
            legend.push(item("disconnect", "DISCONNECT SWITCH"));
            legend.push(item("homerun", "HOMERUN TO PANEL (CIRCUIT NO.)"));
            legend.push(item("conduit", "CONDUIT RUN (SIZE)"));
            legend.push(item("elec_panel", "PANELBOARD"));
            abbr.extend(
                [
                    "AFF = ABOVE FINISHED FLOOR",
                    "C = CONDUIT",
                    "EMT = ELECTRICAL METALLIC TUBING",
                    "R = RECEPTACLE",
                    "D = DATA/TELECOM",
                    "FB = FLOOR BOX",
                    "DS = DISCONNECT",
                ]
                .map(abbrev),
            );
        }
        Kind::FireAlarm(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            fa_devices(&mut pen, t, st, l, &format!("FA-10{l}"));
            legend.push(item("fa_smoke", "SMOKE DETECTOR (SD)"));
            legend.push(item("fa_heat", "HEAT DETECTOR (HD)"));
            legend.push(item("fa_pull", "PULL STATION (PS)"));
            legend.push(item("fa_horn_strobe", "HORN/STROBE (HS)"));
            legend.push(item("fa_strobe", "STROBE (ST)"));
            legend.push(item("fa_duct", "DUCT DETECTOR (DD)"));
            legend.push(item("fa_speaker", "SPEAKER/STROBE (SS)"));
            legend.push(item("fa_panel", "FIRE ALARM CONTROL PANEL (FACP)"));
            legend.push(item("fa_nac", "NAC POWER SUPPLY"));
            legend.push(item("fa_annunciator", "ANNUNCIATOR"));
            legend.push(item("fa_module", "MONITOR/RELAY MODULE (MM/RM)"));
            abbr.extend(
                [
                    "SD = SMOKE DETECTOR",
                    "HD = HEAT DETECTOR",
                    "PS = PULL STATION",
                    "HS = HORN/STROBE",
                    "DD = DUCT DETECTOR",
                    "FACP = FIRE ALARM CONTROL PANEL",
                    "NAC = NOTIFICATION APPLIANCE CIRCUIT",
                    "SLC = SIGNALLING LINE CIRCUIT",
                    "AFF = ABOVE FINISHED FLOOR",
                    "CLG = CEILING",
                ]
                .map(abbrev),
            );
        }
        Kind::Telecom(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            telecom_devices(&mut pen, t, st, l, &format!("T-10{l}"));
            legend.push(item("tc_data", "DATA OUTLET (D)"));
            legend.push(item("tc_tv", "TV OUTLET (TV)"));
            legend.push(item("tc_phone", "TELEPHONE OUTLET (TEL)"));
            legend.push(item("tc_wifi", "WIRELESS ACCESS POINT (WAP)"));
            legend.push(item("tc_camera", "SECURITY CAMERA (CAM)"));
            legend.push(item("tc_reader", "CARD READER (CR)"));
            legend.push(item("tc_speaker", "SPEAKER (SPK)"));
            legend.push(item("tc_idf", "IDF/MDF"));
            legend.push(item("tc_nurse", "NURSE CALL STATION (NC)"));
            legend.push(item("tc_ada_pull", "ADA PULL CORD"));
            abbr.extend(
                [
                    "D = DATA OUTLET",
                    "TV = TELEVISION OUTLET",
                    "TEL = TELEPHONE",
                    "WAP = WIRELESS ACCESS POINT",
                    "CAM = CAMERA",
                    "CR = CARD READER",
                    "IDF = INTERMEDIATE DISTRIBUTION FRAME",
                    "MDF = MAIN DISTRIBUTION FRAME",
                    "NC = NURSE CALL",
                    "ENT = ELECTRICAL NONMETALLIC TUBING",
                ]
                .map(abbrev),
            );
        }
        Kind::SleeveLayout(l) => {
            background(&mut pen, t, l, true);
            grid(&mut pen, t);
            crate::lv_reader::draw_sleeve_layout(&mut pen, t, st, l);
            legend.push(item("sleeve_fa", "FIRE ALARM SLEEVE (RED)"));
            legend.push(item("sleeve_data", "DATA/TELECOM SLEEVE (BLUE)"));
            legend.push(item("sleeve_security", "SECURITY SLEEVE (GREEN)"));
            legend.push(item("sleeve_floor", "FLOOR SLEEVE"));
            abbr.extend(
                [
                    "EMT = ELECTRICAL METALLIC TUBING",
                    "ENT = ELECTRICAL NONMETALLIC TUBING",
                    "FS = FIRESTOP",
                    "SL = SLEEVE",
                ]
                .map(abbrev),
            );
        }
        Kind::Site => {
            site_plan(&mut pen, t, st);
            legend.push(item("building", "BUILDING FOOTPRINT"));
            legend.push(item("parking", "PARKING SPACE"));
            legend.push(item("light_pole", "SITE LIGHT POLE"));
            legend.push(item("transformer", "TRANSFORMER PAD"));
            legend.push(item("fire_hydrant", "FIRE HYDRANT"));
            legend.push(item("dumpster", "DUMPSTER ENCLOSURE"));
            legend.push(item("underground", "UNDERGROUND UTILITY (DASHED)"));
            legend.push(item("tree", "DECIDUOUS TREE"));
            legend.push(item("shrub", "SHRUB"));
            legend.push(item("fence", "CHAIN LINK FENCE"));
            legend.push(item("bench", "SITE BENCH"));
            abbr.extend(
                [
                    "LP = LIGHT POLE",
                    "FH = FIRE HYDRANT",
                    "XFMR = TRANSFORMER",
                    "UG = UNDERGROUND",
                    "PVC = POLYVINYL CHLORIDE",
                    "EV = ELECTRIC VEHICLE",
                    "RW = RETAINING WALL",
                    "IRR = IRRIGATION",
                    "HC = HANDICAP",
                ]
                .map(abbrev),
            );
        }
        Kind::Grading => {
            grading_plan(&mut pen, t, st);
            legend.push(item("existing_contour", "EXISTING CONTOUR (DASHED)"));
            legend.push(item("proposed_contour", "PROPOSED CONTOUR (SOLID)"));
            legend.push(item("building_pad", "BUILDING PAD"));
            legend.push(item("retaining_wall", "RETAINING WALL"));
            abbr.extend(
                ["EX = EXISTING", "PROP = PROPOSED", "FFE = FINISHED FLOOR ELEVATION", "TW = TOP OF WALL", "BW = BOTTOM OF WALL", "CY = CUBIC YARD"]
                    .map(abbrev),
            );
        }
        Kind::SiteUtility => {
            site_utility_plan(&mut pen, t, st);
            legend.push(item("manhole", "MANHOLE"));
            legend.push(item("catch_basin", "CATCH BASIN"));
            legend.push(item("cleanout", "CLEANOUT"));
            legend.push(item("valve_box", "VALVE BOX"));
            legend.push(item("meter", "WATER METER"));
            legend.push(item("backflow", "BACKFLOW PREVENTER"));
            abbr.extend(
                [
                    "MH = MANHOLE",
                    "CB = CATCH BASIN",
                    "CO = CLEANOUT",
                    "VB = VALVE BOX",
                    "WM = WATER METER",
                    "BFP = BACKFLOW PREVENTER",
                    "JB = JUNCTION BOX",
                    "INV = INVERT",
                    "RIM = RIM ELEVATION",
                ]
                .map(abbrev),
            );
        }
        Kind::SiteSchedules => {
            site_schedules(&mut pen, t);
        }
    }
    if kind.plan() {
        plan_title(&mut pen, t, sh);
    }
    // Draw callout symbols on plan sheets.
    draw_callouts(&mut pen, t, sh);
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
        "E" => "ELECTRICAL",
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
        "wall_tag" => {
            let (c, r) = (pen.p([x, y]), pen.pl(0.09));
            crate::walls::tag_symbol(pen, "LEGEND", c, r, st.walls.symbol);
            pen.ptext("LEGEND", [x, y - 0.03], 0.06, "X", HAlign::Center);
        }
        "ceiling_type" => {
            let (c, r) = (pen.p([x, y]), pen.pl(0.09));
            crate::walls::tag_symbol(pen, "LEGEND", c, r, (st.walls.symbol + 1) % 4);
            pen.ptext("LEGEND", [x, y - 0.03], 0.06, "C", HAlign::Center);
        }
        "detail" => {
            let (c, r) = (pen.p([x, y]), pen.pl(0.09));
            pen.circle("LEGEND", c, r);
            pen.ptext("LEGEND", [x, y - 0.03], 0.06, "1", HAlign::Center);
        }
        "door" => {
            pen.pline("A-DOOR", [x - 0.15, y - 0.1], [x - 0.15, y + 0.15]);
            let (c, r) = (pen.p([x - 0.15, y - 0.1]), pen.pl(0.25));
            pen.arc("A-DOOR", c, r, 0.0, FRAC_PI_2);
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
        "sprinkler_head" | "sprinkler_upright" | "sprinkler_sidewall" => {
            let c = pen.p([x, y]);
            let r = pen.pl(0.05);
            let mount = kind.strip_prefix("sprinkler_").filter(|m| *m != "head").unwrap_or("pendent");
            head_symbol(pen, c, r, mount);
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
        "cable_tray" => {
            pen.pline("TRAY", [x - 0.3, y + 0.07], [x + 0.3, y + 0.07]);
            pen.pline("TRAY", [x - 0.3, y - 0.07], [x + 0.3, y - 0.07]);
            for k in [-0.2, 0.0, 0.2] {
                pen.pline("TRAY", [x + k, y - 0.07], [x + k, y + 0.07]);
            }
        }
        "conduit_bank" => {
            for k in [-0.05, 0.0, 0.05] {
                pen.pline("CONDUIT", [x - 0.3, y + k], [x + 0.3, y + k]);
            }
        }
        "column" => {
            let (a, b) = (pen.p([x - 0.06, y - 0.06]), pen.p([x + 0.06, y + 0.06]));
            pen.fill("STRUCT", [a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)]);
        }
        "beam" => pen.pline("HEAVY", [x - 0.3, y], [x + 0.3, y]),
        "opening" => pen.prect("DASHED", [x - 0.15, y - 0.1], [x + 0.15, y + 0.1]),
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
        // Electrical symbols (legend)
        "light_2x4" => pen.prect("DEVICE", [x - 0.2, y - 0.08], [x + 0.2, y + 0.08]),
        "light_2x2" => pen.prect("DEVICE", [x - s, y - s], [x + s, y + s]),
        "light_down" => {
            let cv = pen.p([x, y]);
            pen.circle("DEVICE", cv, pen.pl(0.06));
        }
        "elec_switch" => {
            let cv = pen.p([x, y]);
            pen.circle("DEVICE", cv, pen.pl(0.05));
            pen.ptext("DEVICE", [x, y - 0.025], 0.04, "S", HAlign::Center);
        }
        "exit_sign" => {
            let hs = 0.07;
            pen.prect("DEVICE", [x - hs, y - hs * 0.5], [x + hs, y + hs * 0.5]);
            pen.pline("DEVICE", [x - hs, y - hs * 0.5], [x + hs, y + hs * 0.5]);
            pen.pline("DEVICE", [x - hs, y + hs * 0.5], [x + hs, y - hs * 0.5]);
        }
        "emergency_light" => {
            let cv = pen.p([x, y]);
            pen.circle("DEVICE", cv, pen.pl(0.06));
            pen.ptext("DEVICE", [x, y - 0.025], 0.03, "EM", HAlign::Center);
        }
        "junction_box" => {
            let hs = 0.04;
            let (av, bv) = (pen.p([x - hs, y - hs]), pen.p([x + hs, y + hs]));
            pen.fill("DEVICE", [av, Vec2::new(bv.x, av.y), bv, Vec2::new(av.x, bv.y)]);
        }
        "receptacle" => {
            let cv = pen.p([x, y]);
            pen.circle("DEVICE", cv, pen.pl(0.05));
            let hs = pen.pl(0.03);
            pen.line("DEVICE", Vec2::new(cv.x - hs, cv.y + hs * 0.4), Vec2::new(cv.x + hs, cv.y + hs * 0.4));
            pen.line("DEVICE", Vec2::new(cv.x - hs, cv.y - hs * 0.4), Vec2::new(cv.x + hs, cv.y - hs * 0.4));
        }
        "data_outlet" => {
            let cv = pen.p([x, y]);
            let hs = pen.pl(0.05);
            pen.poly("DEVICE", &[cv + Vec2::new(0.0, hs), cv + Vec2::new(-hs, -hs), cv + Vec2::new(hs, -hs)], true);
        }
        "floor_box" => {
            let cv = pen.p([x, y]);
            pen.circle("DEVICE", cv, pen.pl(0.05));
            let hs = pen.pl(0.04);
            pen.line("DEVICE", Vec2::new(cv.x - hs, cv.y), Vec2::new(cv.x + hs, cv.y));
            pen.line("DEVICE", Vec2::new(cv.x, cv.y - hs), Vec2::new(cv.x, cv.y + hs));
        }
        "disconnect" => {
            pen.prect("DEVICE", [x - 0.1, y - 0.06], [x + 0.1, y + 0.06]);
            pen.ptext("DEVICE", [x, y - 0.025], 0.04, "DS", HAlign::Center);
        }
        "homerun" => {
            pen.pline("DEVICE", [x - 0.15, y - 0.08], [x + 0.15, y + 0.08]);
            pen.pline("DEVICE", [x + 0.15, y + 0.08], [x + 0.08, y + 0.08]);
            pen.pline("DEVICE", [x + 0.15, y + 0.08], [x + 0.15, y + 0.01]);
            pen.pline("DEVICE", [x - 0.03, y + 0.03], [x + 0.03, y - 0.03]);
        }
        "conduit" => pen.pline("DEVICE", [x - 0.3, y], [x + 0.3, y]),
        "elec_panel" => {
            let (av, bv) = (pen.p([x - 0.08, y - 0.1]), pen.p([x + 0.08, y + 0.1]));
            pen.fill("DEVICE", [av, Vec2::new(bv.x, av.y), bv, Vec2::new(av.x, bv.y)]);
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

/// Electrical devices (receptacles, switches) as small symbols on the E-sheet.
/// Symbols are drawn at the diagrammatic position (`diag_ft`), offset from the true
/// architectural position for readability.
fn draw_devices(pen: &mut Pen, t: &Truth, level: &str) {
    let th = pen.st.text_in;
    for dev in t.devices.iter().filter(|d| d.level == level) {
        // Use the diagrammatic position for E-sheet drawing.
        let pos = if dev.diag_ft != [0.0, 0.0] { dev.diag_ft } else { dev.at_ft };
        let c = pen.b(pos);
        let r = pen.bl(0.25);
        match dev.kind.as_str() {
            "receptacle" => {
                pen.circle("ELEC", c, r);
                pen.line("ELEC", Vec2::new(c.x - r * 0.5, c.y - r * 0.7), Vec2::new(c.x - r * 0.5, c.y + r * 0.7));
                pen.line("ELEC", Vec2::new(c.x + r * 0.5, c.y - r * 0.7), Vec2::new(c.x + r * 0.5, c.y + r * 0.7));
            }
            "switch" => {
                pen.circle("ELEC", c, r);
                pen.text("ELEC", c, th * 0.8, "S", 0.0, HAlign::Center, VAlign::Middle);
            }
            _ => {
                pen.circle("ELEC", c, r * 0.7);
            }
        }
        pen.text("ELEC-TEXT", Vec2::new(c.x + r * 1.5, c.y), th * 0.7, &dev.tag, 0.0, HAlign::Left, VAlign::Middle);
    }
}

/// Electrical devices drawn at their true architectural positions on A-sheets (small X marks
/// showing where the device actually goes on the wall).
fn draw_devices_arch(pen: &mut Pen, t: &Truth, level: &str) {
    let th = pen.st.text_in;
    for dev in t.devices.iter().filter(|d| d.level == level) {
        let c = pen.b(dev.at_ft);
        let r = pen.bl(0.12);
        // Small cross at the true position.
        pen.line("ELEC-ARCH", Vec2::new(c.x - r, c.y - r), Vec2::new(c.x + r, c.y + r));
        pen.line("ELEC-ARCH", Vec2::new(c.x - r, c.y + r), Vec2::new(c.x + r, c.y - r));
        pen.text("ELEC-ARCH", Vec2::new(c.x + r * 1.5, c.y), th * 0.5, &dev.tag, 0.0, HAlign::Left, VAlign::Middle);
    }
}

/// Draw casework items on an architectural floor plan: rectangles on `CASEWORK` layer with tags.
fn casework_plan(pen: &mut Pen, t: &Truth, st: &Style, level: &str) {
    let rot = readable(pen.rot());
    for cw in t.casework.iter().filter(|c| c.level == level) {
        // Draw the casework rectangle on the CASEWORK layer.
        pen.brect("CASEWORK", cw.min_ft, cw.max_ft);

        // A hatch pattern to distinguish the kind.
        match cw.kind.as_str() {
            "base_cabinet" | "vanity" => {
                // Diagonal hatch lines inside the rectangle.
                let n_lines = 4usize;
                let dx = cw.max_ft[0] - cw.min_ft[0];
                let dy = cw.max_ft[1] - cw.min_ft[1];
                for k in 1..=n_lines {
                    let frac = k as f64 / (n_lines + 1) as f64;
                    let a = pen.b([cw.min_ft[0] + frac * dx, cw.min_ft[1]]);
                    let b = pen.b([cw.min_ft[0], cw.min_ft[1] + frac * dy]);
                    pen.line("CASEWORK", a, b);
                }
            }
            "upper_cabinet" => {
                // Inner dashed rectangle.
                let inset = 0.15;
                let inner_min = [cw.min_ft[0] + inset, cw.min_ft[1] + inset];
                let inner_max = [cw.max_ft[0] - inset, cw.max_ft[1] - inset];
                if inner_max[0] > inner_min[0] && inner_max[1] > inner_min[1] {
                    pen.brect("CASEWORK", inner_min, inner_max);
                }
            }
            "appliance" => {
                // X mark inside.
                let a1 = pen.b(cw.min_ft);
                let b1 = pen.b(cw.max_ft);
                pen.line("CASEWORK", a1, b1);
                let a2 = pen.b([cw.min_ft[0], cw.max_ft[1]]);
                let b2 = pen.b([cw.max_ft[0], cw.min_ft[1]]);
                pen.line("CASEWORK", a2, b2);
            }
            _ => {} // counter, backsplash: just the rectangle
        }

        // Tag text centred on the casework item.
        let cx = (cw.min_ft[0] + cw.max_ft[0]) / 2.0;
        let cy = (cw.min_ft[1] + cw.max_ft[1]) / 2.0;
        let p = pen.b([cx, cy]);
        pen.text("CASEWORK", p, st.text_in * 0.8, &cw.tag, rot, HAlign::Center, VAlign::Middle);

        // Height note below the tag.
        let height_text = format!("{:.0}\" AFF", cw.height_in);
        let p2 = pen.b([cx, cy - 0.6]);
        pen.text("TEXT", p2, st.text_in * 0.6, &height_text, rot, HAlign::Center, VAlign::Middle);
    }
}

/// Plumbing fixtures and water heaters on an architectural floor plan: a symbol centred on each
/// (on [`crate::casework_reader::FIXTURE_LAYER`]) with its tag on it.
fn fixture_plan(pen: &mut Pen, t: &Truth, st: &Style, level: &str) {
    use crate::casework_reader::FIXTURE_LAYER as L;
    let rot = readable(pen.rot());
    let mut items: Vec<(&str, &str, [f64; 2], f64)> =
        t.fixtures.iter().filter(|f| f.level == level).map(|f| (f.kind.as_str(), f.tag.as_str(), [f.at_ft[0], f.at_ft[1]], 0.0)).collect();
    for e in t.equipment.iter().filter(|e| e.level == level && e.kind == "water_heater") {
        let c = [(e.min_ft[0] + e.max_ft[0]) / 2.0, (e.min_ft[1] + e.max_ft[1]) / 2.0];
        let r = ((e.max_ft[0] - e.min_ft[0]).min(e.max_ft[1] - e.min_ft[1]) / 2.0).clamp(0.5, 2.0);
        items.push((e.kind.as_str(), e.tag.as_str(), c, r));
    }
    for (kind, tag, at, r) in items {
        let c = pen.b(at);
        // Half sizes, inches (along x, along y of the building).
        let rect = |pen: &mut Pen, hx: f64, hy: f64| pen.brect(L, [at[0] - hx / 12.0, at[1] - hy / 12.0], [at[0] + hx / 12.0, at[1] + hy / 12.0]);
        match kind {
            "water_closet" => {
                rect(pen, 10.0, 14.0);
                pen.circle(L, c, 7.0);
            }
            "lavatory" => {
                rect(pen, 10.0, 9.0);
                pen.circle(L, c, 6.0);
            }
            "sink" => {
                rect(pen, 12.0, 11.0);
                pen.circle(L, c, 7.0);
            }
            "urinal" => {
                rect(pen, 9.0, 7.0);
                pen.circle(L, c, 4.0);
            }
            "shower" | "mop_sink" => {
                let h = if kind == "shower" { 18.0 } else { 12.0 };
                rect(pen, h, h);
                pen.line(L, pen.b([at[0] - h / 12.0, at[1] - h / 12.0]), pen.b([at[0] + h / 12.0, at[1] + h / 12.0]));
                pen.line(L, pen.b([at[0] - h / 12.0, at[1] + h / 12.0]), pen.b([at[0] + h / 12.0, at[1] - h / 12.0]));
            }
            "bathtub" => {
                rect(pen, 30.0, 15.0);
                rect(pen, 26.0, 11.0);
            }
            "water_heater" => pen.circle(L, c, pen.bl(r)),
            _ => pen.circle(L, c, 3.0),
        }
        pen.text(L, c, st.text_in * 0.6, tag, rot, HAlign::Center, VAlign::Middle);
    }
}

/// The rows of the casework and plumbing fixture schedule: one per tag (header first).
pub fn casework_schedule_rows(t: &Truth) -> Vec<Vec<String>> {
    use crate::casework_reader::description_for;
    let inch = |v: f64| if (v - v.round()).abs() < 0.05 { format!("{:.0}\"", v) } else { format!("{:.1}\"", v) };
    let mut rows = vec![["TAG", "DESCRIPTION", "SIZE (W x D)", "HEIGHT", "SPEC SECTION"].map(String::from).to_vec()];
    let mut seen: Vec<String> = Vec::new();
    for cw in &t.casework {
        if seen.contains(&cw.tag) {
            continue;
        }
        seen.push(cw.tag.clone());
        let widths: Vec<f64> =
            t.casework.iter().filter(|c| c.tag == cw.tag).map(|c| (c.max_ft[0] - c.min_ft[0]).max(c.max_ft[1] - c.min_ft[1]) * 12.0).collect();
        let w0 = widths.first().copied().unwrap_or(0.0);
        let width = if widths.iter().all(|w| (w - w0).abs() < 0.5) { inch(w0) } else { "VARIES".into() };
        let mut desc = description_for(&cw.kind).to_string();
        if cw.kind == "counter" && cw.spec_section.as_deref() == Some("22 40 00") {
            desc.push_str(" WITH SINK");
        }
        rows.push(vec![
            cw.tag.clone(),
            desc,
            format!("{width} x {}", inch(cw.depth_in)),
            format!("{} AFF", inch(cw.height_in)),
            cw.spec_section.clone().unwrap_or_else(|| "-".into()),
        ]);
    }
    let fixtures = t.fixtures.iter().map(|f| (f.tag.as_str(), f.kind.as_str()));
    let heaters = t.equipment.iter().filter(|e| e.kind == "water_heater").map(|e| (e.tag.as_str(), e.kind.as_str()));
    for (tag, kind) in fixtures.chain(heaters) {
        if tag.is_empty() || seen.iter().any(|s| s == tag) {
            continue;
        }
        seen.push(tag.to_string());
        let spec = if kind == "water_heater" { "22 33 00" } else { "22 40 00" };
        rows.push(vec![tag.to_string(), description_for(kind).to_string(), "-".into(), "-".into(), spec.into()]);
    }
    rows
}

/// The casework and plumbing fixture schedule on a floor plan, in a corner of the drawing area
/// the plan leaves free (on [`crate::casework_reader::SCHEDULE_LAYER`]).
fn casework_schedule(pen: &mut Pen, t: &Truth, level: &str) {
    use crate::casework_reader::{SCHEDULE_LAYER as L, SCHEDULE_TITLE};
    let rows = casework_schedule_rows(t);
    if rows.len() < 2 {
        return;
    }
    let (h, row_h) = (0.09, 0.24);
    let ncols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<f64> = (0..ncols).map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| text_width(s, h)).fold(0.6, f64::max) + 0.25).collect();
    let (w, ht) = (widths.iter().sum::<f64>(), row_h * rows.len() as f64 + 0.3);
    // The plan's extents on paper.
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for rm in t.building.rooms.iter().filter(|r| r.level == level) {
        for p in [rm.min_ft, rm.max_ft, [rm.min_ft[0], rm.max_ft[1]], [rm.max_ft[0], rm.min_ft[1]]] {
            let m = pen.b(p);
            let q = crate::layout::model_to_paper(pen.sh, [m.x, m.y]);
            lo = [lo[0].min(q[0]), lo[1].min(q[1])];
            hi = [hi[0].max(q[0]), hi[1].max(q[1])];
        }
    }
    let fr = frame(pen.st);
    let [[x0, y0], [x1, y1]] = fr.area;
    let free = |at: [f64; 2]| {
        let (a, b) = ([at[0], at[1] - ht], [at[0] + w, at[1] + 0.3]);
        a[0] >= x0 && b[0] <= x1 && a[1] >= y0 && b[1] <= y1 && (b[0] < lo[0] - 0.5 || a[0] > hi[0] + 0.5 || b[1] < lo[1] - 0.5 || a[1] > hi[1] + 0.5)
    };
    // Bottom right, then top right below the north arrow, then top left; kept out of the strips
    // along the right edge and the foot of the sheet a sheet reader takes for the title block
    // (its title would read as the sheet's).
    let [pw, ph] = fr.paper;
    let right = (x1 - w - 0.2).min(0.7 * pw);
    let cands = [[right, (y0 + ht + 0.2).max(0.25 * ph)], [right, y1 - 1.6], [x0 + 0.2, y1 - 0.5]];
    let Some(at) = cands.iter().copied().find(|c| free(*c)).or(cands.first().copied()) else { return };
    pen.ptext(L, [at[0], at[1] + 0.1], 0.13, SCHEDULE_TITLE, HAlign::Left);
    let mut y = at[1];
    for r in &rows {
        let mut x = at[0];
        for (c, cw) in widths.iter().enumerate() {
            pen.prect(L, [x, y - row_h], [x + cw, y]);
            if let Some(s) = r.get(c) {
                pen.ptext(L, [x + 0.1, y - row_h + 0.075], h, s, HAlign::Left);
            }
            x += cw;
        }
        y -= row_h;
    }
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
    for e in t.equipment.iter().filter(|e| e.level == level && ["ahu", "vav", "condensing_unit"].contains(&e.kind.as_str())) {
        pen.brect("EQUIP", [e.min_ft[0], e.min_ft[1]], [e.max_ft[0], e.max_ft[1]]);
        let th = st.text_in / pen.sh.scale;
        let rot = readable(pen.rot());
        if e.kind == "ahu" {
            // In the unit's south part, clear of the ducts that rise out of its top.
            let c = pen.b([e.min_ft[0] + 0.3 * (e.max_ft[0] - e.min_ft[0]), e.min_ft[1] + th * 1.2]);
            pen.block("TEXT", c, st.text_in * 1.3, std::slice::from_ref(&e.tag), rot);
        } else if e.kind == "condensing_unit" {
            // On its pad: the unit with an X (the fan), the tag beside it.
            let (a, b) = (pen.b([e.min_ft[0], e.min_ft[1]]), pen.b([e.max_ft[0], e.max_ft[1]]));
            pen.line("EQUIP", a, b);
            // Its tag under it (the louver's tag is east of it).
            let at = pen.b([(e.min_ft[0] + e.max_ft[0]) / 2.0, e.min_ft[1] - th * 1.2]);
            pen.text("TEXT", at, st.text_in, &e.tag, rot, HAlign::Center, VAlign::Middle);
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
                head_symbol(pen, c, r, tm.mount.as_deref().unwrap_or("pendent"));
            }
            _ => {}
        }
    }
}

/// A sprinkler head: pendent (circle with the set's mark), upright (circle in a circle),
/// sidewall (half circle off its wall, flat side to the north wall).
fn head_symbol(pen: &mut Pen, c: Vec2, r: f64, mount: &str) {
    match mount {
        "upright" => {
            pen.circle("DEVICE", c, r);
            pen.circle("DEVICE", c, r * 0.45);
        }
        "sidewall" => {
            let rot = pen.rot();
            let along = Vec2::new(rot.cos(), rot.sin());
            pen.line("DEVICE", c - along * r, c + along * r);
            pen.arc("DEVICE", c, r, PI + rot, 2.0 * PI + rot);
            let down = Vec2::new(rot.sin(), -rot.cos());
            pen.line("DEVICE", c, c + down * (r * 0.6));
        }
        _ => {
            pen.circle("DEVICE", c, r);
            head_mark(pen, c, r);
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
        for e in t.equipment.iter().filter(|e| e.level == level && ["water_heater", "gas_meter", "pump"].contains(&e.kind.as_str())) {
            let rot = readable(pen.rot());
            let c = [(e.min_ft[0] + e.max_ft[0]) / 2.0, (e.min_ft[1] + e.max_ft[1]) / 2.0];
            if e.kind == "water_heater" {
                let pc = pen.b(c);
                let r = pen.bl((e.max_ft[0] - e.min_ft[0]) / 2.0);
                pen.circle("EQUIP", pc, r);
                let at = pen.b([c[0] + 1.5 + th * 2.0, c[1] + 1.5]);
                pen.text("TEXT", at, st.text_in, &e.tag, rot, HAlign::Left, VAlign::Middle);
            } else if e.kind == "pump" {
                // In line on its pipe; the tag south of it on a leader, past the pipes beside it
                // (the pipe's own tags are north of it).
                pen.brect("EQUIP", [e.min_ft[0], e.min_ft[1]], [e.max_ft[0], e.max_ft[1]]);
                let at = [c[0], e.min_ft[1] - 3.2 - th];
                let (p, q) = (pen.b([c[0], e.min_ft[1]]), pen.b([c[0], at[1] + th * 0.6]));
                pen.line("TEXT", p, q);
                let a = pen.b(at);
                pen.text("TEXT", a, st.text_in, &e.tag, rot, HAlign::Center, VAlign::Middle);
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

/// Cable tray (ladder: both sides, rungs every 2', `CT` at the middle), feeder conduits (a line
/// each), the conduit riser bank in the mech room (circles and a note), and their tags.
fn trays(pen: &mut Pen, t: &Truth, level: &str) {
    let sheet = pen.sh.number.clone();
    let rot = readable(pen.rot());
    for s in t.segments.iter().filter(|s| s.sheet == sheet && !s.vertical && (s.kind == "tray" || s.kind == "conduit")) {
        let [a, b] = s.path_ft;
        if s.kind == "conduit" {
            let (p, q) = (pen.b([a[0], a[1]]), pen.b([b[0], b[1]]));
            pen.line("CONDUIT", p, q);
            continue;
        }
        let hw = s.size_in[0] / 24.0;
        let len = s.length_ft();
        let u = [(b[0] - a[0]) / len.max(1e-9), (b[1] - a[1]) / len.max(1e-9)];
        let n = [-u[1], u[0]];
        for sg in [1.0, -1.0] {
            let (p, q) = (pen.b([a[0] + n[0] * hw * sg, a[1] + n[1] * hw * sg]), pen.b([b[0] + n[0] * hw * sg, b[1] + n[1] * hw * sg]));
            pen.line("TRAY", p, q);
        }
        let mut d = 0.0;
        while d <= len + 1e-9 {
            let c = [a[0] + u[0] * d, a[1] + u[1] * d];
            let (p, q) = (pen.b([c[0] + n[0] * hw, c[1] + n[1] * hw]), pen.b([c[0] - n[0] * hw, c[1] - n[1] * hw]));
            pen.line("TRAY", p, q);
            d += 2.0;
        }
    }
    // The riser bank: (4) 2" between the levels.
    let marks: Vec<&Riser> = t.risers.iter().filter(|r| r.system == "EC" && r.shown.iter().any(|m| m.level == level)).collect();
    for r in &marks {
        let c = pen.b(r.at_ft);
        let rad = pen.pl(0.04);
        let sym = r.shown.iter().find(|m| m.level == level).map_or("up", |m| m.symbol.as_str());
        riser_mark(pen, c, rad, sym);
    }
    if let Some(first) = marks.first() {
        let th = pen.st.text_in / pen.sh.scale;
        let up = first.shown.iter().find(|m| m.level == level).is_some_and(|m| m.symbol == "up");
        let note = if up { format!("({}) 2\"C UP TO LEVEL 2 PANELS", marks.len()) } else { format!("({}) 2\"C FROM LEVEL 1 PANELS", marks.len()) };
        let at = pen.b([first.at_ft[0], first.at_ft[1] + th * 3.0]);
        pen.text("TEXT", at, pen.st.text_in, &note, rot, HAlign::Left, VAlign::Bottom);
    }
    tags(pen, t, &sheet);
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
            // The ceiling tag: in a box, a little off the room's centre (its kind alone when the
            // height is left out).
            let lines = if cl.height_hidden { vec![cl.kind.clone()] } else { st.ceiling_text(cl.height_ft, &cl.kind) };
            // Narrow rooms (corridors) get it on their centreline, well inside their walls.
            let at = [c[0] + 2.0, c[1] - if y1 - y0 > 12.0 { 2.5 } else { 0.0 }];
            let p = pen.b(at);
            pen.block("TEXT", p, st.text_in, &lines, rot);
            let w = lines.iter().map(|l| text_width(l, st.text_in)).fold(0.0, f64::max) / pen.sh.scale / 2.0 + 0.4;
            let h = lines.len() as f64 * st.text_in * 1.4 / pen.sh.scale / 2.0 + 0.3;
            pen.brect("TEXT", [at[0] - w, at[1] - h], [at[0] + w, at[1] + h]);
        }
    }
    terminals(pen, t, level, &["SA", "RA", "EA", "FP"], false);
    // Soffits: dashed rectangle with a tag.
    for sf in t.rcp_soffits.iter().filter(|s| s.level == level) {
        pen.brect("DASHED", sf.min_ft, sf.max_ft);
        let c = [(sf.min_ft[0] + sf.max_ft[0]) / 2.0, (sf.min_ft[1] + sf.max_ft[1]) / 2.0];
        let p = pen.b(c);
        // Small enough to fit along the soffit.
        let fit = sf.length_ft.max(sf.width_ft) * pen.sh.scale * 0.8 / (sf.tag.chars().count().max(1) as f64 * 1.1);
        pen.text("TEXT", p, (st.text_in * 0.9).min(fit), &sf.tag, rot, HAlign::Center, VAlign::Middle);
    }
}

// ---- Electrical ---------------------------------------------------------------------------------

/// Draw electrical devices, conduit runs, homerun arrows and panels for a level.
/// Inches as a fitter writes them, to the sixteenth: `3/8`, `1-1/2`, `4`.
pub fn frac_in(v: f64) -> String {
    let q = (v * 16.0).round() as i64;
    let (whole, mut num, mut den) = (q / 16, q % 16, 16);
    while num > 0 && num % 2 == 0 {
        num /= 2;
        den /= 2;
    }
    match (whole, num) {
        (w, 0) => w.to_string(),
        (0, n) => format!("{n}/{den}"),
        (w, n) => format!("{w}-{n}/{den}"),
    }
}

/// A light fixture type's row in the made-up light fixture schedule: description, mounting,
/// lamp/wattage (the answer key for what a fixture of that type carries).
pub fn light_fixture_row(type_tag: &str) -> (&'static str, &'static str, &'static str) {
    match type_tag {
        "A" => ("2X4 RECESSED TROFFER", "RECESSED", "LED 40W"),
        "B" => ("2X2 RECESSED TROFFER", "RECESSED", "LED 24W"),
        "C" => ("RECESSED DOWNLIGHT 6\"", "RECESSED", "LED 12W"),
        "D" => ("WALL SCONCE", "WALL MOUNT", "LED 9W"),
        "E" => ("PENDANT LINEAR 4'", "PENDANT", "LED 36W"),
        _ => ("LIGHT FIXTURE", "SURFACE", "LED"),
    }
}

/// The gear's schedules as printed: equipment (type, voltage, rating, size, mounting, entry
/// plates, gutter), lugs (one row per termination point) and feeders.
pub fn gear_tables(t: &Truth) -> Vec<(&'static str, Vec<Vec<String>>)> {
    let row = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
    let gear: Vec<&crate::truth::ElecPanel> = t.elec_panels.iter().filter(|p| !p.properties.is_empty()).collect();
    let cols = ["TYPE", "VOLTAGE", "RATING", "SIZE", "MOUNTING", "TOP ENTRY", "BOTTOM ENTRY", "GUTTER"];
    let mut eq = vec![std::iter::once("TAG").chain(cols).map(String::from).collect::<Vec<_>>()];
    for p in &gear {
        let mut r = vec![p.tag.clone()];
        r.extend(cols.iter().map(|c| p.properties.get(*c).cloned().unwrap_or_else(|| "-".into())));
        eq.push(r);
    }
    let mut lugs = vec![row(&["TAG", "POINT", "LUGS", "WIRES", "RANGE", "RATING", "TYPE", "SUPPLIED", "BOLT", "BENDING"])];
    for p in &gear {
        for l in &p.terminations {
            let bolt =
                if l.supplied { "-".to_string() } else { format!("{}\" {}-HOLE, {}\" STACK", frac_in(l.bolt_in), l.holes, frac_in(l.stack_in)) };
            let bend = if l.bending_in > 0.0 { format!("{}\" {}", l.bending_in, if l.opposite { "OPP" } else { "ADJ" }) } else { "-".into() };
            lugs.push(vec![
                p.tag.clone(),
                l.point.replace('_', " ").to_uppercase(),
                l.lugs.to_string(),
                l.wires_per_lug.to_string(),
                l.range.to_uppercase(),
                l.material.to_uppercase(),
                if l.lug_type == "compression" { "COMP".into() } else { "MECH".into() },
                if l.supplied { "YES".into() } else { "BY EC".into() },
                bolt,
                bend,
            ]);
        }
    }
    let mut feeders = vec![row(&["TAG", "FROM", "TO", "SETS", "PHASE", "NEUTRAL", "GROUND", "BONDING", "RACEWAY", "ENTRY"])];
    for f in &t.elec_feeders {
        let cond = |n: u32, size: &str| {
            if n == 0 || size.is_empty() { "-".to_string() } else { format!("{n} x {} {}", size.to_uppercase(), f.material.to_uppercase()) }
        };
        feeders.push(vec![
            f.tag.clone(),
            if f.from.is_empty() { "UTILITY".into() } else { f.from.clone() },
            f.to.clone(),
            f.sets.to_string(),
            format!("{} {}", f.size.to_uppercase(), f.material.to_uppercase()),
            cond(f.neutrals, if f.neutral_size.is_empty() { &f.size } else { &f.neutral_size }),
            cond(f.grounds, &f.ground_size),
            if f.extra_grounds.is_empty() { "-".into() } else { f.extra_grounds.join(", ").to_uppercase() },
            format!("{} x {}\"", f.conduits, frac_in(f.conduit_in)),
            f.entry.to_uppercase(),
        ]);
    }
    vec![("ELECTRICAL EQUIPMENT SCHEDULE", eq), ("LUG SCHEDULE", lugs), ("FEEDER SCHEDULE", feeders)]
}

fn elec_devices(pen: &mut Pen, t: &Truth, st: &Style, level: &str, sheet: &str, _lighting: bool) {
    let rot = readable(pen.rot());
    let th = st.text_in / pen.sh.scale;

    // Conduit runs on this sheet.
    for cd in t.elec_conduits.iter().filter(|c| c.level == level && c.sheet == sheet) {
        let a = pen.b(cd.from_ft);
        let b = pen.b(cd.to_ft);
        pen.line("DEVICE", a, b);
        if let Some(tag) = &cd.tag {
            let tp = pen.b(tag.at_ft);
            pen.text("TEXT", tp, st.text_in, &tag.text.join(" "), rot, HAlign::Left, VAlign::Bottom);
        }
    }

    // Panels on this sheet.
    for p in t.elec_panels.iter().filter(|p| p.level == level && p.sheet == sheet) {
        let a = pen.b(p.min_ft);
        let b = pen.b(p.max_ft);
        pen.rect("DEVICE", a, b);
        pen.fill("DEVICE", [a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)]);
        let tp = pen.b([p.at_ft[0], p.at_ft[1] + 0.5]);
        pen.text("TEXT", tp, st.text_in, &p.tag, rot, HAlign::Center, VAlign::Bottom);
    }

    // Devices on this sheet.
    let devs: Vec<&crate::truth::ElecDevice> = t.elec_devices.iter().filter(|d| d.level == level && d.sheet == sheet).collect();
    for d in &devs {
        let c = pen.b(d.drawn_ft);
        let s = th * 0.8; // symbol radius in model units
        let v = |dx: f64, dy: f64| Vec2::new(c.x + dx, c.y + dy);
        match d.kind.as_str() {
            "light_fixture" => {
                match d.type_tag.as_str() {
                    "A" => {
                        // 2x4 troffer: rectangle
                        let (hw, hh) = (s * 2.0, s);
                        pen.rect("DEVICE", v(-hw, -hh), v(hw, hh));
                    }
                    "B" => pen.rect("DEVICE", v(-s, -s), v(s, s)),
                    "C" => pen.circle("DEVICE", c, s * 0.7),
                    "D" => pen.arc("DEVICE", c, s * 0.6, 0.0, PI),
                    "E" => {
                        let w = s * 2.0;
                        pen.line("DEVICE", v(-w, 0.0), v(w, 0.0));
                        let t = s * 0.3;
                        pen.line("DEVICE", v(-w, -t), v(-w, t));
                        pen.line("DEVICE", v(w, -t), v(w, t));
                    }
                    _ => pen.circle("DEVICE", c, s * 0.7),
                }
                if !t.elec_spec.untagged.contains(&d.id) {
                    pen.text("TEXT", v(0.0, -s * 1.5), st.text_in, &d.tag, rot, HAlign::Center, VAlign::Top);
                }
            }
            "switch" => {
                pen.circle("DEVICE", c, s * 0.5);
                pen.text("DEVICE", c, st.text_in * 0.7, "S", rot, HAlign::Center, VAlign::Middle);
            }
            "exit_sign" => {
                let hs = s * 0.6;
                pen.rect("DEVICE", v(-hs, -hs * 0.5), v(hs, hs * 0.5));
                pen.line("DEVICE", v(-hs, -hs * 0.5), v(hs, hs * 0.5));
                pen.line("DEVICE", v(-hs, hs * 0.5), v(hs, -hs * 0.5));
            }
            "emergency_light" => {
                pen.circle("DEVICE", c, s * 0.6);
                pen.text("DEVICE", c, st.text_in * 0.5, "EM", rot, HAlign::Center, VAlign::Middle);
            }
            "junction_box" => {
                let hs = s * 0.4;
                pen.rect("DEVICE", v(-hs, -hs), v(hs, hs));
                pen.fill("DEVICE", [v(-hs, -hs), v(hs, -hs), v(hs, hs), v(-hs, hs)]);
            }
            "receptacle" => {
                pen.circle("DEVICE", c, s * 0.5);
                let hs = s * 0.3;
                pen.line("DEVICE", v(-hs, hs * 0.3), v(hs, hs * 0.3));
                pen.line("DEVICE", v(-hs, -hs * 0.3), v(hs, -hs * 0.3));
            }
            "data_outlet" => {
                let hs = s * 0.5;
                pen.poly("DEVICE", &[v(0.0, hs), v(-hs, -hs), v(hs, -hs)], true);
            }
            "floor_box" => {
                pen.circle("DEVICE", c, s * 0.5);
                let hs = s * 0.4;
                pen.line("DEVICE", v(-hs, 0.0), v(hs, 0.0));
                pen.line("DEVICE", v(0.0, -hs), v(0.0, hs));
            }
            "disconnect" => {
                let (hw, hh) = (s * 0.8, s * 0.5);
                pen.rect("DEVICE", v(-hw, -hh), v(hw, hh));
                pen.text("DEVICE", c, st.text_in * 0.7, "DS", rot, HAlign::Center, VAlign::Middle);
            }
            _ => pen.circle("DEVICE", c, s * 0.5),
        }

        // Mounting note beside the device (for non-ceiling mountings).
        if let Some(note) = &d.mounting_note
            && note != "CLG"
        {
            pen.text("TEXT", v(s * 2.0, 0.0), st.text_in * 0.8, note, rot, HAlign::Left, VAlign::Middle);
        }
        // Subscripts (GFCI, USB…) above the symbol and the circuit (P1-5) below it.
        if let Some(n) = t.elec_spec.note(&d.id) {
            if !n.subscripts.is_empty() {
                pen.text("TEXT", v(0.0, s * 1.2), st.text_in * 0.7, &n.subscripts.join(" "), rot, HAlign::Center, VAlign::Bottom);
            }
            if !n.circuit_ref.is_empty() {
                pen.text("TEXT", v(0.0, -s * 1.2), st.text_in * 0.7, &n.circuit_ref, rot, HAlign::Center, VAlign::Top);
            }
        }
    }

    // Homerun arrows.
    for ckt in t.elec_circuits.iter().filter(|ck| ck.level == level && ck.sheet == sheet) {
        let hr = pen.b(ckt.homerun_ft);
        let tip = Vec2::new(hr.x + th * 3.0, hr.y + th * 3.0);
        pen.line("DEVICE", hr, tip);
        let al = th * 0.8;
        pen.line("DEVICE", tip, Vec2::new(tip.x - al, tip.y));
        pen.line("DEVICE", tip, Vec2::new(tip.x, tip.y - al));
        let mid = Vec2::new((hr.x + tip.x) / 2.0, (hr.y + tip.y) / 2.0);
        pen.line("DEVICE", Vec2::new(mid.x - th * 0.3, mid.y + th * 0.3), Vec2::new(mid.x + th * 0.3, mid.y - th * 0.3));
        pen.text("TEXT", Vec2::new(tip.x + th * 0.5, tip.y), st.text_in, &ckt.label, rot, HAlign::Left, VAlign::Middle);
        if let Some((_, note)) = t.elec_spec.conductors.iter().find(|(c, _)| *c == ckt.id) {
            pen.text("TEXT", Vec2::new(tip.x + th * 0.5, tip.y - th * 1.4), st.text_in * 0.7, note, rot, HAlign::Left, VAlign::Middle);
        }
    }

    // "DO NOT SCALE" note.
    let note_at = pen.b([3.0, -2.0]);
    pen.text("TEXT", note_at, st.text_in, "DO NOT SCALE - LOCATIONS PER ARCHITECTURAL", rot, HAlign::Left, VAlign::Top);
}

// ---- Fire alarm devices on FA sheets ----------------------------------------------------------------

fn fa_devices(pen: &mut Pen, t: &Truth, st: &Style, level: &str, sheet: &str) {
    let rot = readable(pen.rot());
    let th = st.text_in / pen.sh.scale;
    let s = th * 0.8;
    const FA_LAYER: &str = "FA-DEVICE";
    const FA_TEXT: &str = "FA-TEXT";

    for fa in t.fire_alarm_devices.iter().filter(|d| d.level == level && d.sheet == sheet) {
        let c = pen.b([fa.at_ft[0], fa.at_ft[1]]);
        let v = |dx: f64, dy: f64| Vec2::new(c.x + dx, c.y + dy);
        match fa.kind.as_str() {
            "smoke_detector" | "co_detector" => {
                pen.circle(FA_LAYER, c, s * 0.5);
                pen.text(FA_LAYER, c, st.text_in * 0.6, "SD", rot, HAlign::Center, VAlign::Middle);
            }
            "heat_detector" => {
                pen.circle(FA_LAYER, c, s * 0.5);
                pen.text(FA_LAYER, c, st.text_in * 0.6, "HD", rot, HAlign::Center, VAlign::Middle);
            }
            "pull_station" => {
                let hs = s * 0.5;
                pen.rect(FA_LAYER, v(-hs, -hs), v(hs, hs));
                pen.text(FA_LAYER, c, st.text_in * 0.6, "PS", rot, HAlign::Center, VAlign::Middle);
            }
            "horn_strobe" | "speaker_strobe" => {
                pen.circle(FA_LAYER, c, s * 0.6);
                pen.line(FA_LAYER, v(-s * 0.4, 0.0), v(s * 0.4, 0.0));
                pen.line(FA_LAYER, v(0.0, -s * 0.4), v(0.0, s * 0.4));
                pen.text(FA_LAYER, c, st.text_in * 0.5, "HS", rot, HAlign::Center, VAlign::Middle);
            }
            "strobe" => {
                pen.circle(FA_LAYER, c, s * 0.5);
                pen.line(FA_LAYER, v(-s * 0.3, -s * 0.3), v(s * 0.3, s * 0.3));
                pen.line(FA_LAYER, v(-s * 0.3, s * 0.3), v(s * 0.3, -s * 0.3));
            }
            "speaker" => {
                pen.circle(FA_LAYER, c, s * 0.5);
                pen.text(FA_LAYER, c, st.text_in * 0.5, "SPK", rot, HAlign::Center, VAlign::Middle);
            }
            "duct_detector" => {
                let hw = s * 0.8;
                let hh = s * 0.4;
                pen.rect(FA_LAYER, v(-hw, -hh), v(hw, hh));
                pen.text(FA_LAYER, c, st.text_in * 0.5, "DD", rot, HAlign::Center, VAlign::Middle);
            }
            "facp" => {
                let hs = s * 1.2;
                pen.rect(FA_LAYER, v(-hs, -hs * 0.8), v(hs, hs * 0.8));
                pen.fill(FA_LAYER, [v(-hs, -hs * 0.8), v(hs, -hs * 0.8), v(hs, hs * 0.8), v(-hs, hs * 0.8)]);
                pen.text(FA_TEXT, c, st.text_in * 0.6, "FACP", rot, HAlign::Center, VAlign::Middle);
            }
            "nac_panel" => {
                let hs = s * 0.8;
                pen.rect(FA_LAYER, v(-hs, -hs * 0.6), v(hs, hs * 0.6));
                pen.text(FA_LAYER, c, st.text_in * 0.5, "NAC", rot, HAlign::Center, VAlign::Middle);
            }
            "annunciator" => {
                let hs = s * 0.7;
                pen.rect(FA_LAYER, v(-hs, -hs * 0.6), v(hs, hs * 0.6));
                pen.text(FA_LAYER, c, st.text_in * 0.5, "ANN", rot, HAlign::Center, VAlign::Middle);
            }
            "monitor_module" | "relay_module" => {
                let hs = s * 0.4;
                pen.rect(FA_LAYER, v(-hs, -hs), v(hs, hs));
                let label = if fa.kind == "monitor_module" { "MM" } else { "RM" };
                pen.text(FA_LAYER, c, st.text_in * 0.5, label, rot, HAlign::Center, VAlign::Middle);
            }
            "elevator_recall" | "door_holder" | "mag_door_holder" => {
                pen.circle(FA_LAYER, c, s * 0.4);
                pen.fill(FA_LAYER, [v(-s * 0.3, -s * 0.3), v(s * 0.3, -s * 0.3), v(s * 0.3, s * 0.3), v(-s * 0.3, s * 0.3)]);
            }
            _ => {
                pen.circle(FA_LAYER, c, s * 0.5);
            }
        }
        // Tag text beside the device.
        if !fa.tag.is_empty() {
            let tp = v(s * 1.2, 0.0);
            pen.text(FA_TEXT, tp, st.text_in * 0.8, &fa.tag, rot, HAlign::Left, VAlign::Middle);
        }
        // Candela note for notification appliances.
        if let Some(cd) = fa.candela {
            let tp = v(0.0, -s * 1.5);
            pen.text(FA_TEXT, tp, st.text_in * 0.6, &format!("{cd} cd"), rot, HAlign::Center, VAlign::Top);
        }
        // Mounting height note (non-ceiling).
        if fa.mounting != "CLG" && fa.mounting_in > 0.0 {
            let ht = crate::truth::fmt_in(fa.mounting_in);
            let tp = v(0.0, s * 1.2);
            pen.text(FA_TEXT, tp, st.text_in * 0.6, &format!("{ht} AFF"), rot, HAlign::Center, VAlign::Bottom);
        }
        // SLC/NAC circuit tags (small, below device).
        let mut note_y = -s * 2.2;
        if !fa.slc.is_empty() {
            let tp = v(0.0, note_y);
            pen.text(FA_TEXT, tp, st.text_in * 0.5, &fa.slc, rot, HAlign::Center, VAlign::Top);
            note_y -= s * 0.8;
        }
        if !fa.nac.is_empty() {
            let tp = v(0.0, note_y);
            pen.text(FA_TEXT, tp, st.text_in * 0.5, &fa.nac, rot, HAlign::Center, VAlign::Top);
        }
    }
}

// ---- Telecom/LV devices on T sheets ----------------------------------------------------------------

fn telecom_devices(pen: &mut Pen, t: &Truth, st: &Style, level: &str, sheet: &str) {
    let rot = readable(pen.rot());
    let th = st.text_in / pen.sh.scale;
    let s = th * 0.8;
    const TC_LAYER: &str = "TC-DEVICE";
    const TC_TEXT: &str = "TC-TEXT";

    for tc in t.telecom_devices.iter().filter(|d| d.level == level && d.sheet == sheet) {
        let c = pen.b([tc.at_ft[0], tc.at_ft[1]]);
        let v = |dx: f64, dy: f64| Vec2::new(c.x + dx, c.y + dy);
        match tc.kind.as_str() {
            "data_outlet" => {
                let hs = s * 0.5;
                pen.poly(TC_LAYER, &[v(0.0, hs), v(-hs, -hs), v(hs, -hs)], true);
                pen.text(TC_LAYER, c, st.text_in * 0.5, "D", rot, HAlign::Center, VAlign::Middle);
            }
            "tv_outlet" => {
                let hs = s * 0.5;
                pen.poly(TC_LAYER, &[v(0.0, hs), v(-hs, -hs), v(hs, -hs)], true);
                pen.text(TC_LAYER, c, st.text_in * 0.5, "TV", rot, HAlign::Center, VAlign::Middle);
            }
            "telephone" => {
                let hs = s * 0.5;
                pen.poly(TC_LAYER, &[v(0.0, hs), v(-hs, -hs), v(hs, -hs)], true);
                pen.text(TC_LAYER, c, st.text_in * 0.5, "T", rot, HAlign::Center, VAlign::Middle);
            }
            "wifi_ap" => {
                pen.circle(TC_LAYER, c, s * 0.5);
                pen.text(TC_LAYER, c, st.text_in * 0.4, "WAP", rot, HAlign::Center, VAlign::Middle);
            }
            "camera" => {
                pen.circle(TC_LAYER, c, s * 0.5);
                pen.text(TC_LAYER, c, st.text_in * 0.5, "CAM", rot, HAlign::Center, VAlign::Middle);
            }
            "card_reader" | "rex" => {
                let hs = s * 0.4;
                pen.rect(TC_LAYER, v(-hs, -hs * 1.2), v(hs, hs * 1.2));
                let label = if tc.kind == "card_reader" { "CR" } else { "REX" };
                pen.text(TC_LAYER, c, st.text_in * 0.5, label, rot, HAlign::Center, VAlign::Middle);
            }
            "door_contact" | "door_ajar" | "mag_lock" => {
                let hs = s * 0.3;
                pen.rect(TC_LAYER, v(-hs, -hs), v(hs, hs));
                pen.fill(TC_LAYER, [v(-hs, -hs), v(hs, -hs), v(hs, hs), v(-hs, hs)]);
            }
            "nurse_call_station" | "nurse_call_display" | "ada_pull_cord" => {
                pen.circle(TC_LAYER, c, s * 0.5);
                let label = if tc.kind == "ada_pull_cord" { "ADA" } else { "NC" };
                pen.text(TC_LAYER, c, st.text_in * 0.5, label, rot, HAlign::Center, VAlign::Middle);
            }
            "speaker" => {
                pen.circle(TC_LAYER, c, s * 0.5);
                pen.text(TC_LAYER, c, st.text_in * 0.5, "SPK", rot, HAlign::Center, VAlign::Middle);
            }
            "idf" | "mdf" | "patch_panel" | "backboard" | "rack" => {
                let hs = s * 1.0;
                pen.rect(TC_LAYER, v(-hs, -hs * 0.6), v(hs, hs * 0.6));
                pen.fill(TC_LAYER, [v(-hs, -hs * 0.6), v(hs, -hs * 0.6), v(hs, hs * 0.6), v(-hs, hs * 0.6)]);
                let label = tc.kind.to_uppercase();
                pen.text(TC_TEXT, c, st.text_in * 0.5, &label, rot, HAlign::Center, VAlign::Middle);
            }
            "pool_alarm" | "door_alarm" => {
                pen.circle(TC_LAYER, c, s * 0.4);
                pen.text(TC_LAYER, c, st.text_in * 0.4, "ALM", rot, HAlign::Center, VAlign::Middle);
            }
            _ => {
                pen.circle(TC_LAYER, c, s * 0.5);
            }
        }
        // Tag text beside the device.
        if !tc.tag.is_empty() {
            let tp = v(s * 1.2, 0.0);
            pen.text(TC_TEXT, tp, st.text_in * 0.8, &tc.tag, rot, HAlign::Left, VAlign::Middle);
        }
        // Cable spec note.
        if let Some(ref cable) = tc.cable_spec {
            let tp = v(0.0, -s * 1.5);
            pen.text(TC_TEXT, tp, st.text_in * 0.5, cable, rot, HAlign::Center, VAlign::Top);
        }
        // LV system label.
        if !tc.lv_system.is_empty() {
            let tp = v(0.0, s * 1.2);
            pen.text(TC_TEXT, tp, st.text_in * 0.5, &tc.lv_system.to_uppercase(), rot, HAlign::Center, VAlign::Bottom);
        }
    }

    // Cable pathways on this level.
    for cp in t.cable_pathways.iter().filter(|p| p.level == level) {
        let a = pen.b(cp.from_ft);
        let b = pen.b(cp.to_ft);
        let layer = match cp.kind.as_str() {
            "j_hook" => "TC-PATHWAY",
            "cable_tray" => "TC-TRAY",
            _ => "TC-PATHWAY",
        };
        pen.line(layer, a, b);
        // Size/spacing label at midpoint.
        let mid = Vec2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        let label = match cp.kind.as_str() {
            "j_hook" => {
                let sp = cp.spacing_in.unwrap_or(48.0);
                format!("J-HOOK @ {}\" O.C.", sp as u32)
            }
            "ent" => {
                let sz = cp.size_in.unwrap_or(2.0);
                format!("{}\" ENT", sz as u32)
            }
            "cable_tray" => {
                if let Some([w, d]) = cp.tray_size_in {
                    format!("{}\"x{}\" CT", w as u32, d as u32)
                } else {
                    "CT".into()
                }
            }
            _ => String::new(),
        };
        if !label.is_empty() {
            let rot_line = readable(pen.rot());
            pen.text(TC_TEXT, mid, st.text_in * 0.6, &label, rot_line, HAlign::Center, VAlign::Bottom);
        }
    }
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
    // Openings / shafts on this level: dashed rectangle with a label.
    for op in t.building.openings.iter().filter(|o| o.level == level) {
        pen.brect("DASHED", op.min_ft, op.max_ft);
        if let Some(lbl) = &op.label {
            let mid = [(op.min_ft[0] + op.max_ft[0]) / 2.0, (op.min_ft[1] + op.max_ft[1]) / 2.0];
            let p = pen.b(mid);
            pen.text("TEXT", p, st.text_in, lbl, rot, HAlign::Center, VAlign::Middle);
        }
    }
    // General notes under the plan's corner.
    let tos = t.building.beams.iter().find(|b| b.level == level).map_or(0.0, |b| b.tos_ft);
    let col = t.building.columns.first().map(|c| c.size.clone()).unwrap_or_default();
    let mut notes = vec!["GENERAL NOTES:".to_string(), format!("1. T.O.S. = EL {} U.N.O.", feet_inches(tos)), format!("2. COLUMNS: {col} U.N.O.")];
    if level == "R" {
        notes.push("3. 1-1/2\" METAL ROOF DECK.".into());
    } else {
        let slab = t.building.slabs.iter().find(|s| s.level == level);
        let conc = slab.map_or(t.building.slab_in, |s| s.concrete_in);
        let elev = t.level(level).map_or(0.0, |l| l.elevation_ft);
        notes.push(format!("3. {}\" CONC SLAB ON 1-1/2\" COMPOSITE DECK; TOP OF SLAB = EL {}.", fmt_in(conc), feet_inches(elev)));
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
    pen.ptext("TITLE", [(x0 + x1) / 2.0, y1 - 2.2], 0.2, "MEP AND ELECTRICAL - COORDINATION SET", HAlign::Center);
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
        "5. ELECTRICAL DEVICE LOCATIONS ON E-SHEETS ARE DIAGRAMMATIC.",
        "6. REFER TO ARCHITECTURAL PLANS FOR EXACT DEVICE LOCATIONS.",
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
        vec![
            ("AIR HANDLING UNIT SCHEDULE", vec!["ahu"]),
            ("VAV TERMINAL UNIT SCHEDULE", vec!["vav"]),
            ("EXHAUST FAN SCHEDULE", vec!["exhaust_fan"]),
            ("CONDENSING UNIT SCHEDULE", vec!["condensing_unit"]),
        ]
    } else {
        vec![("WATER HEATER SCHEDULE", vec!["water_heater"]), ("PUMP SCHEDULE", vec!["pump"]), ("GAS METER", vec!["gas_meter"])]
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
    } else if disc == "E" {
        // Light fixture schedule.
        let mut lt_rows = vec![vec!["TYPE".to_string(), "DESCRIPTION".to_string(), "MOUNTING".to_string(), "LAMP/WATTAGE".to_string()]];
        let mut seen_lt: Vec<String> = Vec::new();
        let mut devs: Vec<&crate::truth::ElecDevice> = t.elec_devices.iter().filter(|d| d.kind == "light_fixture").collect();
        devs.sort_by(|a, b| a.type_tag.cmp(&b.type_tag));
        for d in &devs {
            if seen_lt.contains(&d.type_tag) || t.elec_spec.unscheduled.contains(&d.type_tag) {
                continue;
            }
            seen_lt.push(d.type_tag.clone());
            let (desc, mount, watt) = light_fixture_row(&d.type_tag);
            lt_rows.push(vec![d.type_tag.clone(), desc.into(), mount.into(), watt.into()]);
        }
        let h1 = table(pen, [x, y], "LIGHT FIXTURE SCHEDULE", &lt_rows);
        y -= h1 + 0.6;

        // Panel schedules.
        for panel in &t.elec_panels {
            if panel.kind != "panel" || panel.schedule.is_empty() {
                continue;
            }
            let mut rows = vec![vec!["CKT".to_string(), "DESCRIPTION".to_string(), "BKR".to_string(), "POLES".to_string(), "TYPE".to_string()]];
            for row in &panel.schedule {
                let kind = t.elec_spec.breaker(&panel.id, &row.circuit).unwrap_or("-");
                rows.push(vec![row.circuit.clone(), row.description.clone(), row.breaker_amps.clone(), row.poles.clone(), kind.to_string()]);
            }
            let h2 = table(pen, [x, y], &format!("PANEL {} SCHEDULE", panel.tag), &rows);
            y -= h2 + 0.6;
            if y < fr.area[0][1] + 3.0 {
                y = y1 - 0.5;
                x += 13.0;
            }
        }
        // The gear: equipment, lug and feeder schedules (made-up data; see `gear_design`).
        for (title, rows) in gear_tables(t) {
            if rows.len() < 2 {
                continue;
            }
            if y < fr.area[0][1] + 3.0 + 0.25 * rows.len() as f64 {
                y = y1 - 0.5;
                x += 13.0;
            }
            let h = table(pen, [x, y], title, &rows);
            y -= h + 0.6;
        }
    } else {
        let rows = vec![
            vec!["TAG".to_string(), "FIXTURE".to_string(), "CW".to_string(), "HW".to_string(), "SAN".to_string(), "V".to_string()],
            vec!["WC-1".into(), "WATER CLOSET, WALL HUNG, FLUSH VALVE".into(), "1\"".into(), "-".into(), "3\"".into(), "2\"".into()],
            vec!["LAV-1".into(), "LAVATORY, WALL HUNG".into(), "1/2\"".into(), "1/2\"".into(), "1-1/2\"".into(), "1-1/2\"".into()],
        ];
        table(pen, [x, y], "PLUMBING FIXTURE SCHEDULE", &rows);
    }
}

/// The sprinkler head schedule and the pipe schedule (heads per branch size), bottom right of the
/// drawing area.
fn fp_schedules(pen: &mut Pen, t: &Truth) {
    let fr = frame(pen.st);
    let [[_, y0], [x1, _]] = fr.area;
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
    let mut heads = vec![
        s(&["TAG", "TYPE", "K", "TEMP", "FINISH", "WHERE"]),
        s(&["SH-1", "PENDENT", "5.6", "155F", "CHROME", "IN CEILINGS"]),
        s(&["SH-2", "UPRIGHT", "5.6", "155F", "BRASS", &format!("OPEN TO STRUCTURE, {}\" SPRIG", t.fp_sprig_in.unwrap_or(4.0) as u32)]),
    ];
    if t.terminals.iter().any(|x| x.mount.as_deref() == Some("sidewall")) {
        heads.push(s(&[
            "SH-3",
            "HORIZ SIDEWALL",
            "5.6",
            "155F",
            "WHITE",
            &format!("{}\" BELOW CEILING", t.fp_sidewall_below_in.unwrap_or(4.0) as u32),
        ]));
    }
    let mut pipes = vec![s(&["PIPE", "MAX HEADS"])];
    for (d, n) in crate::design::PIPE_SCHEDULE {
        pipes.push(vec![format!("{}\"", pipe_size(*d)), format!("{n}")]);
    }
    let top = y0 + 2.2;
    table(pen, [x1 - 2.6, top], "PIPE SCHEDULE", &pipes);
    table(pen, [x1 - 10.6, top], "SPRINKLER HEAD SCHEDULE", &heads);
    pen.ptext("TEXT", [x1 - 10.6, top - 1.4], 0.09, "ARMOVERS, SPRIGS AND DROPS 1\" U.N.O.", HAlign::Left);
}

/// A table with a title, top-left at paper `at`; returns its height, inches.
pub(crate) fn table(pen: &mut Pen, at: [f64; 2], title: &str, rows: &[Vec<String>]) -> f64 {
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

// ---- Callout symbols (Q306) -------------------------------------------------------------------

use crate::layout::CALLOUT_LAYER;

/// Draw all callout symbols that belong to this sheet.
fn draw_callouts(pen: &mut Pen, t: &Truth, sh: &Sheet) {
    for co in &t.callouts {
        if co.on_sheet != sh.number {
            continue;
        }
        match co.kind.as_str() {
            "section_mark" => draw_section_mark(pen, co),
            "elevation_tag" => draw_elevation_tag(pen, co),
            "detail_bubble" => draw_detail_bubble(pen, co),
            "enlarged_plan" => draw_enlarged_plan_callout(pen, co),
            "key_plan" => draw_key_plan(pen, t, sh),
            "match_line" => draw_match_line(pen, co),
            _ => {}
        }
    }
}

/// A section mark: a circle with the detail number on top and the sheet number on bottom,
/// separated by a horizontal line. A cut line (dashed) extends from the circle, with small
/// arrows showing the looking direction.
///
/// On the CALLOUT layer, text recognizable by the reader.
fn draw_section_mark(pen: &mut Pen, co: &CalloutEntry) {
    let center = pen.b(co.at_ft);
    let r = pen.pl(0.25);
    pen.circle(CALLOUT_LAYER, center, r);
    // Horizontal divider
    pen.line(CALLOUT_LAYER, center + Vec2::new(-r, 0.0), center + Vec2::new(r, 0.0));
    // Detail number (top half)
    pen.text(CALLOUT_LAYER, center + Vec2::new(0.0, r * 0.45), 0.1, &co.detail, 0.0, HAlign::Center, VAlign::Middle);
    // Target sheet (bottom half)
    pen.text(CALLOUT_LAYER, center - Vec2::new(0.0, r * 0.45), 0.1, &co.target_sheet, 0.0, HAlign::Center, VAlign::Middle);

    // Cut line: dashed line through the section cut
    if let (Some(from), Some(to)) = (co.cut_from_ft, co.cut_to_ft) {
        let a = pen.b(from);
        let b = pen.b(to);
        // Only draw the parts beyond the bubble radius
        let dir = (b - a).normalized();
        // Line from `a` end to the near edge of the bubble
        let near_a = center - dir * r;
        let near_b = center + dir * r;
        if (a - near_a).len() > r {
            pen.line("DASHED", a, near_a);
        }
        if (b - near_b).len() > r {
            pen.line("DASHED", near_b, b);
        }
        // Small arrows at the ends showing looking direction
        if let Some(looking) = &co.looking {
            let arrow_len = pen.pl(0.15);
            let perp = match looking.as_str() {
                "north" => Vec2::new(0.0, 1.0),
                "south" => Vec2::new(0.0, -1.0),
                "east" => Vec2::new(1.0, 0.0),
                "west" => Vec2::new(-1.0, 0.0),
                _ => Vec2::new(0.0, 1.0),
            };
            let perp_m = Vec2::new(perp.x * 12.0 * arrow_len / pen.pl(1.0), perp.y * 12.0 * arrow_len / pen.pl(1.0));
            // Arrow at end `a`
            let arr_a = a + perp_m;
            pen.line(CALLOUT_LAYER, a, arr_a);
            // Arrow at end `b`
            let arr_b = b + perp_m;
            pen.line(CALLOUT_LAYER, b, arr_b);
        }
    }
}

/// An elevation tag: a circle with the detail number on top and the sheet number on bottom,
/// with an arrow pointing in the looking direction.
fn draw_elevation_tag(pen: &mut Pen, co: &CalloutEntry) {
    let center = pen.b(co.at_ft);
    let r = pen.pl(0.25);
    pen.circle(CALLOUT_LAYER, center, r);
    // Horizontal divider
    pen.line(CALLOUT_LAYER, center + Vec2::new(-r, 0.0), center + Vec2::new(r, 0.0));
    // Detail number (top)
    pen.text(CALLOUT_LAYER, center + Vec2::new(0.0, r * 0.45), 0.1, &co.detail, 0.0, HAlign::Center, VAlign::Middle);
    // Target sheet (bottom)
    pen.text(CALLOUT_LAYER, center - Vec2::new(0.0, r * 0.45), 0.1, &co.target_sheet, 0.0, HAlign::Center, VAlign::Middle);
    // Arrow from the circle edge in the looking direction
    if let Some(looking) = &co.looking {
        let dir = match looking.as_str() {
            "north" => Vec2::new(0.0, 1.0),
            "south" => Vec2::new(0.0, -1.0),
            "east" => Vec2::new(1.0, 0.0),
            "west" => Vec2::new(-1.0, 0.0),
            _ => Vec2::new(0.0, 1.0),
        };
        let arrow_start = center + dir * r;
        let arrow_end = center + dir * (r + pen.pl(0.2));
        pen.line(CALLOUT_LAYER, arrow_start, arrow_end);
    }
}

/// A detail bubble: a circle with the detail number on top and the sheet number on bottom.
fn draw_detail_bubble(pen: &mut Pen, co: &CalloutEntry) {
    let center = pen.b(co.at_ft);
    let r = pen.pl(0.25);
    pen.circle(CALLOUT_LAYER, center, r);
    // Horizontal divider
    pen.line(CALLOUT_LAYER, center + Vec2::new(-r, 0.0), center + Vec2::new(r, 0.0));
    // Detail number (top)
    pen.text(CALLOUT_LAYER, center + Vec2::new(0.0, r * 0.45), 0.1, &co.detail, 0.0, HAlign::Center, VAlign::Middle);
    // Target sheet (bottom)
    pen.text(CALLOUT_LAYER, center - Vec2::new(0.0, r * 0.45), 0.1, &co.target_sheet, 0.0, HAlign::Center, VAlign::Middle);
}

/// An enlarged-plan boundary callout: a dashed rectangle around the area with a callout bubble
/// at the corner pointing to the enlarged plan sheet.
fn draw_enlarged_plan_callout(pen: &mut Pen, co: &CalloutEntry) {
    if let (Some(_from), Some(to)) = (co.cut_from_ft, co.cut_to_ft) {
        // The dashed rectangle is already drawn by the main plumbing draw code.
        // Add a callout bubble offset from the upper-right corner, with a leader back to it.
        let corner = pen.b([to[0], to[1]]);
        let r = pen.pl(0.25);
        let offset = r * 4.0;
        let bub = corner + Vec2::new(offset, offset);
        pen.circle(CALLOUT_LAYER, bub, r);
        pen.line(CALLOUT_LAYER, bub + Vec2::new(-r, 0.0), bub + Vec2::new(r, 0.0));
        pen.text(CALLOUT_LAYER, bub + Vec2::new(0.0, r * 0.45), 0.1, &co.detail, 0.0, HAlign::Center, VAlign::Middle);
        pen.text(CALLOUT_LAYER, bub - Vec2::new(0.0, r * 0.45), 0.1, &co.target_sheet, 0.0, HAlign::Center, VAlign::Middle);
        // Leader from the corner to the bubble edge
        let dir = (corner - bub).normalized();
        let edge = bub + dir * r;
        pen.line(CALLOUT_LAYER, corner, edge);
    }
}

/// A key plan: a small outline of the building grid with the current sheet's area highlighted,
/// drawn in the title block area.
fn draw_key_plan(pen: &mut Pen, t: &Truth, _sh: &Sheet) {
    let fr = frame(pen.st);
    // Place the key plan in the top-right of the title block.
    let kp_size = 1.2; // paper inches
    let margin = 0.2;
    let kp_x = fr.title[1][0] - kp_size - margin;
    let kp_y = fr.title[1][1] - kp_size - margin;

    let xs: Vec<f64> = t.building.grid.x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = t.building.grid.y.iter().map(|g| g.at_ft).collect();
    let x_min = xs.first().copied().unwrap_or(0.0);
    let y_min = ys.first().copied().unwrap_or(0.0);
    let x_max = xs.last().copied().unwrap_or(100.0);
    let y_max = ys.last().copied().unwrap_or(60.0);
    let bw = (x_max - x_min).max(1.0);
    let bh = (y_max - y_min).max(1.0);
    let s = kp_size / bw.max(bh);

    // Map building coords to key plan paper coords.
    let kp = |x: f64, y: f64| -> [f64; 2] { [kp_x + (x - x_min) * s, kp_y + (y - y_min) * s] };

    // Draw building outline
    let a = kp(x_min, y_min);
    let b = kp(x_max, y_max);
    pen.prect(CALLOUT_LAYER, a, b);
    // Label "KEY PLAN"
    pen.ptext(CALLOUT_LAYER, [kp_x, kp_y - 0.15], 0.08, "KEY PLAN", HAlign::Left);
}

/// A match line: a long dashed line across the plan with "SEE SHEET <target>" text,
/// indicating where the adjacent sheet continues.
fn draw_match_line(pen: &mut Pen, co: &CalloutEntry) {
    if let (Some(from), Some(to)) = (co.cut_from_ft, co.cut_to_ft) {
        let a = pen.b(from);
        let b = pen.b(to);
        pen.line("DASHED", a, b);
        // Label: "SEE SHEET <target>"
        let mid = pen.b(co.at_ft);
        let label = format!("SEE SHEET {}", co.target_sheet);
        pen.text(CALLOUT_LAYER, mid + Vec2::new(0.0, pen.pl(0.15)), 0.08, &label, 0.0, HAlign::Center, VAlign::Bottom);
    }
}

// ---- Site plan (C-101) --------------------------------------------------------------------------

/// Draw the site plan: building footprint, parking, light poles, transformer, underground
/// utilities and other site elements on appropriate layers.
fn site_plan(pen: &mut Pen, t: &Truth, _st: &Style) {
    // Building footprint (dashed outline).
    let xs: Vec<f64> = t.building.grid.x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = t.building.grid.y.iter().map(|g| g.at_ft).collect();
    let x_min = xs.first().copied().unwrap_or(0.0);
    let y_min = ys.first().copied().unwrap_or(0.0);
    let x_max = xs.last().copied().unwrap_or(100.0);
    let y_max = ys.last().copied().unwrap_or(60.0);
    pen.brect(SITE_LAYER, [x_min, y_min], [x_max, y_max]);
    // Label the building.
    let bc = pen.b([(x_min + x_max) / 2.0, (y_min + y_max) / 2.0]);
    pen.text(SITE_LAYER, bc, 0.12, "BUILDING", 0.0, HAlign::Center, VAlign::Middle);

    // Draw each site element by kind.
    for se in &t.site_elements {
        let layer = if se.kind.starts_with("underground_") { UNDERGROUND_LAYER } else { SITE_LAYER };
        match se.kind.as_str() {
            "parking_space" => draw_parking(pen, se, layer),
            "light_pole" => draw_light_pole(pen, se, layer),
            "transformer_pad" => draw_transformer_pad(pen, se, layer),
            "dumpster_enclosure" => draw_dumpster(pen, se, layer),
            "fire_hydrant" => draw_fire_hydrant(pen, se, layer),
            "bollard" => draw_bollard(pen, se, layer),
            "sign" => draw_sign(pen, se, layer),
            "driveway" => draw_driveway(pen, se, layer),
            "curb" | "sidewalk" => draw_area_element(pen, se, layer),
            "paving_asphalt" | "paving_concrete" | "building_pad" => draw_paving(pen, se, layer),
            "striping" => draw_striping(pen, se, layer),
            "retaining_wall" => draw_retaining_wall(pen, se, layer),
            "tree" => draw_tree(pen, se, LANDSCAPE_LAYER),
            "shrub" => draw_shrub(pen, se, LANDSCAPE_LAYER),
            "planter" => draw_planter(pen, se, LANDSCAPE_LAYER),
            "irrigation_head" => draw_irrigation(pen, se, LANDSCAPE_LAYER),
            "fence" => draw_fence(pen, se, FENCE_LAYER),
            "gate" => draw_gate(pen, se, FENCE_LAYER),
            "bench" => draw_bench(pen, se, FURNISH_LAYER),
            "bike_rack" => draw_bike_rack(pen, se, FURNISH_LAYER),
            "trash_can" => draw_trash_can(pen, se, FURNISH_LAYER),
            "pull_box" => draw_pull_box(pen, se, layer),
            "pole_base" => draw_pole_base(pen, se, layer),
            "ev_charger" => draw_ev_charger(pen, se, layer),
            "monument_sign" => draw_monument_sign(pen, se, layer),
            k if k.starts_with("underground_") => draw_underground(pen, se, layer),
            _ => draw_generic_site(pen, se, layer),
        }
    }
}

/// Parking space: a rectangle with the tag.
fn draw_parking(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
        let c = pen.b(se.at_ft);
        pen.text(layer, c, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Middle);
    }
}

/// Light pole: a filled circle with the tag above.
fn draw_light_pole(pen: &mut Pen, se: &SiteElement, layer: &str) {
    let c = pen.b(se.at_ft);
    let r = pen.pl(0.08);
    pen.circle(layer, c, r);
    // Cross inside the circle for visibility.
    pen.line(layer, c + Vec2::new(-r, 0.0), c + Vec2::new(r, 0.0));
    pen.line(layer, c + Vec2::new(0.0, -r), c + Vec2::new(0.0, r));
    let label_y = c + Vec2::new(0.0, r * 2.5);
    pen.text(layer, label_y, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Transformer pad: a rectangle with X and label.
fn draw_transformer_pad(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
        // Diagonal cross inside.
        let a = pen.b(mn);
        let b = pen.b(mx);
        let c1 = pen.b([mn[0], mx[1]]);
        let c2 = pen.b([mx[0], mn[1]]);
        pen.line(layer, a, b);
        pen.line(layer, c1, c2);
    }
    let c = pen.b(se.at_ft);
    let off = pen.pl(0.15);
    pen.text(layer, c + Vec2::new(0.0, off), 0.07, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Dumpster enclosure: a rectangle with tag.
fn draw_dumpster(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
    }
    let c = pen.b(se.at_ft);
    pen.text(layer, c, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Middle);
}

/// Fire hydrant: a circle with the tag.
fn draw_fire_hydrant(pen: &mut Pen, se: &SiteElement, layer: &str) {
    let c = pen.b(se.at_ft);
    let r = pen.pl(0.1);
    pen.circle(layer, c, r);
    // Small triangle pointing up inside to distinguish from poles.
    let top = c + Vec2::new(0.0, r * 0.7);
    let bl = c + Vec2::new(-r * 0.5, -r * 0.3);
    let br = c + Vec2::new(r * 0.5, -r * 0.3);
    pen.poly(layer, &[top, bl, br], true);
    let label_y = c + Vec2::new(0.0, r * 2.0);
    pen.text(layer, label_y, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Bollard: small filled circle.
fn draw_bollard(pen: &mut Pen, se: &SiteElement, layer: &str) {
    let c = pen.b(se.at_ft);
    let r = pen.pl(0.04);
    pen.circle(layer, c, r);
}

/// Sign: a small rectangle with tag.
fn draw_sign(pen: &mut Pen, se: &SiteElement, layer: &str) {
    let c = pen.b(se.at_ft);
    let s = pen.pl(0.1);
    pen.rect(layer, c - Vec2::new(s, s * 0.5), c + Vec2::new(s, s * 0.5));
    let label_y = c + Vec2::new(0.0, s);
    pen.text(layer, label_y, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Driveway: a rectangle with tag.
fn draw_driveway(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
    }
    let c = pen.b(se.at_ft);
    pen.text(layer, c, 0.07, &se.tag, 0.0, HAlign::Center, VAlign::Middle);
}

/// Generic area element (curb, sidewalk): a rectangle outline with tag.
fn draw_area_element(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
    }
    let c = pen.b(se.at_ft);
    pen.text(layer, c, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Middle);
}

/// Underground utility: a line from min to max (along its run) with depth tag and utility label.
fn draw_underground(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        // Draw the run as a line.
        let a = pen.b(mn);
        let b = pen.b(mx);
        pen.line(layer, a, b);
        // Short tick marks at each end perpendicular to the line.
        let dir = (b - a).normalized();
        let perp = Vec2::new(-dir.y, dir.x);
        let tick = pen.pl(0.08);
        pen.line(layer, a - perp * tick, a + perp * tick);
        pen.line(layer, b - perp * tick, b + perp * tick);
    }
    let c = pen.b(se.at_ft);
    // Tag: utility type.
    let label = &se.tag;
    let off = pen.pl(0.12);
    pen.text(layer, c + Vec2::new(off, 0.0), 0.06, label, 0.0, HAlign::Left, VAlign::Middle);
    // Depth note.
    if let Some(depth) = se.depth_ft {
        let depth_str = format!("{depth:.0}' DEEP");
        pen.text(layer, c + Vec2::new(off, -pen.pl(0.10)), 0.05, &depth_str, 0.0, HAlign::Left, VAlign::Middle);
    }
    // Pipe size and material (with conduit count for ductbanks).
    if let Some(size) = se.size_in {
        let pipe = if let Some(count) = se.conduit_count {
            format!("{count}-{size:.0}\" {}", se.material).trim_end().to_string()
        } else {
            format!("{size:.0}\" {}", se.material).trim_end().to_string()
        };
        pen.text(layer, c + Vec2::new(off, pen.pl(0.10)), 0.05, &pipe, 0.0, HAlign::Left, VAlign::Middle);
    }
}

/// Pull box / handhole: a small square with an X inside and a tag.
fn draw_pull_box(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
        // X inside.
        let a = pen.b(mn);
        let b = pen.b(mx);
        pen.line(layer, a, b);
        pen.line(layer, Vec2::new(a.x, b.y), Vec2::new(b.x, a.y));
    }
    let c = pen.b(se.at_ft);
    let label_y = c + Vec2::new(0.0, pen.pl(0.14));
    pen.text(layer, label_y, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Pole base: a filled square at the base of a light pole, with tag.
fn draw_pole_base(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
        // Diagonal hatching inside.
        let a = pen.b(mn);
        let b = pen.b(mx);
        pen.line(layer, a, b);
    }
    let c = pen.b(se.at_ft);
    let label_y = c + Vec2::new(0.0, pen.pl(0.14));
    pen.text(layer, label_y, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// EV charger: a small rectangle with a lightning bolt symbol (two lines) and tag.
fn draw_ev_charger(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
    }
    let c = pen.b(se.at_ft);
    // A simple lightning-bolt shape: two diagonal lines.
    let s = pen.pl(0.06);
    pen.line(layer, c + Vec2::new(-s * 0.3, s), c + Vec2::new(s * 0.3, 0.0));
    pen.line(layer, c + Vec2::new(s * 0.3, 0.0), c + Vec2::new(-s * 0.3, -s));
    let label_y = c + Vec2::new(0.0, pen.pl(0.14));
    pen.text(layer, label_y, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Monument sign: a larger rectangle with label inside.
fn draw_monument_sign(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
    }
    let c = pen.b(se.at_ft);
    pen.text(layer, c, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Middle);
}

/// Generic site element: a point marker with tag.
fn draw_generic_site(pen: &mut Pen, se: &SiteElement, layer: &str) {
    let c = pen.b(se.at_ft);
    let r = pen.pl(0.06);
    pen.circle(layer, c, r);
    let label_y = c + Vec2::new(0.0, r * 2.0);
    pen.text(layer, label_y, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

// ---- Sitework drawing functions (Q339) ----------------------------------------------------------

/// Paving/building pad area: hatched rectangle with label.
fn draw_paving(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
        // Cross-hatch interior for paving.
        let a = pen.b(mn);
        let b = pen.b(mx);
        let step = pen.pl(0.3);
        let mut x = a.x + step;
        while x < b.x {
            pen.line(layer, Vec2::new(x, a.y), Vec2::new(x, b.y));
            x += step;
        }
    }
    let c = pen.b(se.at_ft);
    pen.text(layer, c, 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Middle);
}

/// Striping: a thin rectangle (paint line).
fn draw_striping(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
    }
    let c = pen.b(se.at_ft);
    let off = pen.pl(0.10);
    if !se.tag.is_empty() {
        pen.text(layer, c + Vec2::new(off, 0.0), 0.04, &se.tag, 0.0, HAlign::Left, VAlign::Middle);
    }
}

/// Retaining wall: thick line with tag and height note.
fn draw_retaining_wall(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        let a = pen.b(mn);
        let b = pen.b([mx[0], mn[1]]);
        let c_mn = pen.b([mn[0], mx[1]]);
        let c_mx = pen.b(mx);
        // Draw as a double line (wall thickness).
        pen.line(layer, a, b);
        pen.line(layer, c_mn, c_mx);
        // Hatching between the lines.
        let dir = (b - a).normalized();
        let step = pen.pl(0.15);
        let len = (b - a).len();
        let mut d = step;
        while d < len {
            let p = a + dir * d;
            let q = c_mn + dir * d;
            pen.line(layer, p, q);
            d += step;
        }
    }
    let c = pen.b(se.at_ft);
    let off = pen.pl(0.15);
    pen.text(layer, c + Vec2::new(0.0, off), 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
    if let Some(h) = se.height_ft {
        pen.text(layer, c + Vec2::new(0.0, -off), 0.05, &format!("HT: {h:.0}'"), 0.0, HAlign::Center, VAlign::Top);
    }
}

/// Tree: circle with cross inside (crown).
fn draw_tree(pen: &mut Pen, se: &SiteElement, layer: &str) {
    let c = pen.b(se.at_ft);
    let r = pen.pl(0.12);
    pen.circle(layer, c, r);
    // Crown lines: 4 short radial lines.
    for ang in [0.0, FRAC_PI_2, PI, 3.0 * FRAC_PI_2] {
        let d = Vec2::new(ang.cos(), ang.sin());
        pen.line(layer, c + d * r * 0.4, c + d * r * 0.9);
    }
    let label_y = c + Vec2::new(0.0, r * 1.8);
    pen.text(layer, label_y, 0.05, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Shrub: small filled circle.
fn draw_shrub(pen: &mut Pen, se: &SiteElement, layer: &str) {
    let c = pen.b(se.at_ft);
    let r = pen.pl(0.05);
    pen.circle(layer, c, r);
    if !se.tag.is_empty() {
        let label_y = c + Vec2::new(0.0, r * 2.5);
        pen.text(layer, label_y, 0.04, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
    }
}

/// Planter: rectangle with diagonal lines and tag.
fn draw_planter(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
    }
    let c = pen.b(se.at_ft);
    pen.text(layer, c, 0.05, &se.tag, 0.0, HAlign::Center, VAlign::Middle);
}

/// Irrigation head: small triangle.
fn draw_irrigation(pen: &mut Pen, se: &SiteElement, layer: &str) {
    let c = pen.b(se.at_ft);
    let s = pen.pl(0.05);
    let top = c + Vec2::new(0.0, s);
    let bl = c + Vec2::new(-s * 0.7, -s * 0.5);
    let br = c + Vec2::new(s * 0.7, -s * 0.5);
    pen.poly(layer, &[top, bl, br], true);
    if !se.tag.is_empty() {
        pen.text(layer, c + Vec2::new(0.0, s * 2.0), 0.04, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
    }
}

/// Fence: line with X marks.
fn draw_fence(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        let a = pen.b(mn);
        let b = pen.b(mx);
        pen.line(layer, a, b);
        // X marks along the fence.
        let dir = (b - a).normalized();
        let perp = Vec2::new(-dir.y, dir.x);
        let tick = pen.pl(0.04);
        let step = pen.pl(0.3);
        let len = (b - a).len();
        let mut d = step;
        while d < len {
            let p = a + dir * d;
            pen.line(layer, p - perp * tick - dir * tick, p + perp * tick + dir * tick);
            pen.line(layer, p + perp * tick - dir * tick, p - perp * tick + dir * tick);
            d += step;
        }
    }
    let c = pen.b(se.at_ft);
    let off = pen.pl(0.10);
    pen.text(layer, c + Vec2::new(0.0, off), 0.05, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Gate: gap in fence with arc.
fn draw_gate(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        let a = pen.b(mn);
        let b = pen.b(mx);
        let mid = (a + b) * 0.5;
        let half = (b - a).len() * 0.5;
        // Draw an arc to indicate the gate swing.
        pen.circle(layer, mid, half * 0.3);
    }
    let c = pen.b(se.at_ft);
    let off = pen.pl(0.10);
    pen.text(layer, c + Vec2::new(0.0, off), 0.06, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Bench: small rectangle with tag.
fn draw_bench(pen: &mut Pen, se: &SiteElement, layer: &str) {
    let c = pen.b(se.at_ft);
    let s = pen.pl(0.08);
    pen.rect(layer, c - Vec2::new(s, s * 0.4), c + Vec2::new(s, s * 0.4));
    pen.text(layer, c + Vec2::new(0.0, s), 0.05, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Bike rack: small rectangle with tag.
fn draw_bike_rack(pen: &mut Pen, se: &SiteElement, layer: &str) {
    if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
        pen.brect(layer, mn, mx);
    } else {
        let c = pen.b(se.at_ft);
        let s = pen.pl(0.06);
        pen.rect(layer, c - Vec2::new(s, s), c + Vec2::new(s, s));
    }
    let c = pen.b(se.at_ft);
    let off = pen.pl(0.12);
    pen.text(layer, c + Vec2::new(0.0, off), 0.05, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

/// Trash can: circle with tag.
fn draw_trash_can(pen: &mut Pen, se: &SiteElement, layer: &str) {
    let c = pen.b(se.at_ft);
    let r = pen.pl(0.05);
    pen.circle(layer, c, r);
    pen.text(layer, c + Vec2::new(0.0, r * 2.0), 0.05, &se.tag, 0.0, HAlign::Center, VAlign::Bottom);
}

// ---- Grading plan (C-102) -----------------------------------------------------------------------

fn grading_plan(pen: &mut Pen, t: &Truth, _st: &Style) {
    // Building footprint (for reference).
    let xs: Vec<f64> = t.building.grid.x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = t.building.grid.y.iter().map(|g| g.at_ft).collect();
    let x_min = xs.first().copied().unwrap_or(0.0);
    let y_min = ys.first().copied().unwrap_or(0.0);
    let x_max = xs.last().copied().unwrap_or(100.0);
    let y_max = ys.last().copied().unwrap_or(60.0);
    pen.brect(SITE_LAYER, [x_min, y_min], [x_max, y_max]);
    let bc = pen.b([(x_min + x_max) / 2.0, (y_min + y_max) / 2.0]);
    pen.text(SITE_LAYER, bc, 0.10, "BUILDING", 0.0, HAlign::Center, VAlign::Middle);

    // Draw grading contours.
    for gc in &t.grading_contours {
        if gc.points.len() < 2 {
            continue;
        }
        let layer = if gc.surface == "existing" { EXISTING_GRADING_LAYER } else { GRADING_LAYER };
        let pts: Vec<Vec2> = gc.points.iter().map(|p| pen.b(*p)).collect();
        for w in pts.windows(2) {
            if let [a, b] = w {
                pen.line(layer, *a, *b);
            }
        }
        // Label at midpoint.
        if let (Some(first), Some(last)) = (pts.first(), pts.last()) {
            let mid = (*first + *last) * 0.5;
            // The elevation as built: half-foot contours keep their decimal.
            let label = if gc.elevation_ft.fract().abs() < 1e-9 { format!("{:.0}", gc.elevation_ft) } else { format!("{:.1}", gc.elevation_ft) };
            let size = if gc.weight == "major" { 0.06 } else { 0.04 };
            pen.text(layer, mid, size, &label, 0.0, HAlign::Center, VAlign::Middle);
        }
    }

    // Draw building pad.
    for se in &t.site_elements {
        if se.kind == "building_pad" {
            draw_paving(pen, se, GRADING_LAYER);
        }
    }

    // Draw retaining walls.
    for se in &t.site_elements {
        if se.kind == "retaining_wall" {
            draw_retaining_wall(pen, se, GRADING_LAYER);
        }
    }
}

// ---- Site utility plan (C-103) ------------------------------------------------------------------

fn site_utility_plan(pen: &mut Pen, t: &Truth, _st: &Style) {
    // Building footprint (for reference).
    let xs: Vec<f64> = t.building.grid.x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = t.building.grid.y.iter().map(|g| g.at_ft).collect();
    let x_min = xs.first().copied().unwrap_or(0.0);
    let y_min = ys.first().copied().unwrap_or(0.0);
    let x_max = xs.last().copied().unwrap_or(100.0);
    let y_max = ys.last().copied().unwrap_or(60.0);
    pen.brect(SITE_LAYER, [x_min, y_min], [x_max, y_max]);
    let bc = pen.b([(x_min + x_max) / 2.0, (y_min + y_max) / 2.0]);
    pen.text(SITE_LAYER, bc, 0.10, "BUILDING", 0.0, HAlign::Center, VAlign::Middle);

    // Draw underground utility runs.
    for se in &t.site_elements {
        if se.kind.starts_with("underground_") {
            draw_underground(pen, se, UNDERGROUND_LAYER);
        }
    }

    // Draw utility structures.
    for ss in &t.site_structures {
        draw_site_structure(pen, ss);
    }
}

/// Draw a site utility structure.
fn draw_site_structure(pen: &mut Pen, ss: &SiteStructure) {
    let c = pen.b(ss.at_ft);
    let r = pen.pl(0.08);
    match ss.kind.as_str() {
        "manhole" => {
            // Circle with cross.
            pen.circle(STRUCTURE_LAYER, c, r);
            pen.line(STRUCTURE_LAYER, c + Vec2::new(-r, 0.0), c + Vec2::new(r, 0.0));
            pen.line(STRUCTURE_LAYER, c + Vec2::new(0.0, -r), c + Vec2::new(0.0, r));
        }
        "catch_basin" => {
            // Square with X.
            pen.rect(STRUCTURE_LAYER, c - Vec2::new(r, r), c + Vec2::new(r, r));
            pen.line(STRUCTURE_LAYER, c - Vec2::new(r, r), c + Vec2::new(r, r));
            pen.line(STRUCTURE_LAYER, c + Vec2::new(-r, r), c + Vec2::new(r, -r));
        }
        "cleanout" => {
            // Small circle.
            pen.circle(STRUCTURE_LAYER, c, r * 0.5);
        }
        "valve_box" => {
            // Diamond.
            let d = r * 0.7;
            pen.poly(STRUCTURE_LAYER, &[c + Vec2::new(0.0, d), c + Vec2::new(d, 0.0), c + Vec2::new(0.0, -d), c + Vec2::new(-d, 0.0)], true);
        }
        "junction_box" => {
            // Square.
            pen.rect(STRUCTURE_LAYER, c - Vec2::new(r, r), c + Vec2::new(r, r));
        }
        "meter" | "backflow_preventer" => {
            // Triangle.
            let s = r * 0.8;
            pen.poly(STRUCTURE_LAYER, &[c + Vec2::new(0.0, s), c + Vec2::new(-s * 0.7, -s * 0.5), c + Vec2::new(s * 0.7, -s * 0.5)], true);
        }
        _ => {
            pen.circle(STRUCTURE_LAYER, c, r * 0.5);
        }
    }
    // Tag.
    let off = pen.pl(0.12);
    pen.text(STRUCTURE_LAYER, c + Vec2::new(off, 0.0), 0.05, &ss.tag, 0.0, HAlign::Left, VAlign::Middle);
    // Rim/invert notes.
    let mut y_off = -pen.pl(0.08);
    if let Some(rim) = ss.rim_ft {
        pen.text(STRUCTURE_LAYER, c + Vec2::new(off, y_off), 0.04, &format!("RIM {rim:.1}"), 0.0, HAlign::Left, VAlign::Middle);
        y_off -= pen.pl(0.07);
    }
    if let Some(inv) = ss.invert_ft {
        pen.text(STRUCTURE_LAYER, c + Vec2::new(off, y_off), 0.04, &format!("INV {inv:.1}"), 0.0, HAlign::Left, VAlign::Middle);
    }
}

// ---- Site schedules (C-601) ---------------------------------------------------------------------

fn site_schedules(pen: &mut Pen, t: &Truth) {
    let fr = frame(pen.st);
    let [[x0, y0], [x1, y1]] = fr.area;
    let x0 = x0 + 0.3;

    // Quantities table.
    let mut qty_rows: Vec<Vec<String>> = vec![vec!["CSI".into(), "DESCRIPTION".into(), "QTY".into(), "UNIT".into()]];
    for sq in &t.site_quantities {
        let qty = if sq.status == "insufficient data" { "-".to_string() } else { format!("{:.0}", sq.quantity) };
        qty_rows.push(vec![sq.csi.clone(), sq.description.clone(), qty, sq.unit.clone()]);
    }
    let h = table(pen, [x0, y1 - 0.5], "SITEWORK QUANTITIES", &qty_rows);

    // Utility structures table.
    if !t.site_structures.is_empty() {
        let mut ss_rows: Vec<Vec<String>> = vec![vec!["TAG".into(), "TYPE".into(), "SYSTEM".into(), "RIM".into(), "INV".into(), "SIZE".into()]];
        for ss in &t.site_structures {
            let rim = ss.rim_ft.map_or("-".into(), |v| format!("{v:.1}"));
            let inv = ss.invert_ft.map_or("-".into(), |v| format!("{v:.1}"));
            let size = ss.size_in.map_or("-".into(), |v| format!("{v:.0}\""));
            ss_rows.push(vec![ss.tag.clone(), ss.kind.replace('_', " ").to_uppercase(), ss.system.to_uppercase(), rim, inv, size]);
        }
        let y_start = y1 - 0.5 - h - 1.0;
        if y_start > y0 + 3.0 {
            table(pen, [x0, y_start], "UTILITY STRUCTURES SCHEDULE", &ss_rows);
        }
    }

    let _ = (x1, y0);
}
