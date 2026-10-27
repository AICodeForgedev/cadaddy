//! The views a sheet carries, drawn from the model's elements: plans per discipline (with their
//! tags, dimensions, circuits and sizes), reflected ceiling and roof plans, sections and
//! elevations at cut lines, the 3D view, and the annotation every view has (title, scale bar,
//! north arrow, keynotes).

use std::collections::{BTreeMap, HashSet};

use cadaddy_doc::{Drawing, Handle, Space};
use cadaddy_geom::Vec3;
use cadaddy_render::hlr::{Scene3, View, hidden_lines};

use super::model::{Disc, Item, bounds, ft_in};
use super::pen::{Align, Pen, TagShape, ok};

/// Paper scales tried, largest first: paper inches per real foot.
pub(super) const SCALES: [(f64, &str); 9] = [
    (0.5, "1/2\" = 1'-0\""),
    (0.25, "1/4\" = 1'-0\""),
    (0.1875, "3/16\" = 1'-0\""),
    (0.125, "1/8\" = 1'-0\""),
    (3.0 / 32.0, "3/32\" = 1'-0\""),
    (1.0 / 16.0, "1/16\" = 1'-0\""),
    (1.0 / 32.0, "1/32\" = 1'-0\""),
    (1.0 / 64.0, "1/64\" = 1'-0\""),
    (1.0 / 128.0, "1/128\" = 1'-0\""),
];
/// Most dimensions in one string.
const MAX_STRING: usize = 40;
/// Most devices one circuit's wiring is drawn through.
const MAX_CIRCUIT: usize = 200;
/// Most model meshes a hidden-line view is made of.
pub(super) const MAX_HLR: usize = 4_000;

/// Text sizes on paper (inches).
pub(super) const TXT: f64 = 0.09;
const TAG: f64 = 0.1;

/// The scale label for paper inches per foot.
pub(super) fn scale_label(per_ft: f64) -> String {
    SCALES
        .iter()
        .find(|(k, _)| (k - per_ft).abs() < 1e-9)
        .map(|(_, l)| (*l).to_string())
        .unwrap_or_else(|| if per_ft > 0.0 { format!("1:{}", (12.0 / per_ft).round()) } else { "NOT TO SCALE".into() })
}

/// The largest scale at which a `w` x `h` feet view fits a `pw` x `ph` inch box.
pub(super) fn fit_scale(w: f64, h: f64, pw: f64, ph: f64, at_most: f64) -> f64 {
    SCALES
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| *k <= at_most + 1e-12)
        .find(|k| w * k <= pw && h * k <= ph)
        .unwrap_or_else(|| (pw / w.max(1.0)).min(ph / h.max(1.0)))
}

/// Model (or view) point → paper point.
#[derive(Clone, Copy, Debug)]
pub(super) struct Frame {
    pub from: [f64; 2],
    pub to: [f64; 2],
    /// Paper inches per drawing unit.
    pub k: f64,
}

impl Frame {
    /// The view centred in `paper` ([x0, y0, x1, y1]) at `k` paper inches per drawing unit.
    pub fn centred(lo: [f64; 2], hi: [f64; 2], paper: [f64; 4], k: f64) -> Frame {
        let from = if ok(lo) && ok(hi) { [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0] } else { [0.0, 0.0] };
        Frame { from, to: [(paper[0] + paper[2]) / 2.0, (paper[1] + paper[3]) / 2.0], k }
    }
    pub fn p(&self, m: [f64; 2]) -> [f64; 2] {
        [self.to[0] + (m[0] - self.from[0]) * self.k, self.to[1] + (m[1] - self.from[1]) * self.k]
    }
    pub fn len(&self, m: f64) -> f64 {
        m * self.k
    }
}

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn mul(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}

/// What a plan shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlanKind {
    Floor,
    Enlarged,
    Ceiling,
    Roof,
    Power,
    Lighting,
    Systems,
    Mechanical,
    Plumbing,
    Fire,
}

impl PlanKind {
    pub fn disc(self) -> Disc {
        match self {
            PlanKind::Floor | PlanKind::Enlarged | PlanKind::Ceiling | PlanKind::Roof => Disc::Arch,
            PlanKind::Power | PlanKind::Lighting | PlanKind::Systems => Disc::Elec,
            PlanKind::Mechanical => Disc::Mech,
            PlanKind::Plumbing => Disc::Plumb,
            PlanKind::Fire => Disc::Fire,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            PlanKind::Floor => "FLOOR PLAN",
            PlanKind::Enlarged => "ENLARGED PLAN",
            PlanKind::Ceiling => "REFLECTED CEILING PLAN",
            PlanKind::Roof => "ROOF PLAN",
            PlanKind::Power => "POWER PLAN",
            PlanKind::Lighting => "LIGHTING PLAN",
            PlanKind::Systems => "SYSTEMS PLAN",
            PlanKind::Mechanical => "MECHANICAL PLAN",
            PlanKind::Plumbing => "PLUMBING PLAN",
            PlanKind::Fire => "FIRE PROTECTION PLAN",
        }
    }
    /// Whether an element is this plan's own (not background).
    pub fn shows(self, it: &Item) -> bool {
        let k = it.kind.as_str();
        match self {
            PlanKind::Floor | PlanKind::Enlarged => it.disc == Disc::Arch && !matches!(k, "ceiling" | "roof" | "floor" | "slab"),
            PlanKind::Ceiling => matches!(k, "wall" | "ceiling" | "light_fixture") || it.terminal,
            PlanKind::Roof => k == "roof" || (k == "equipment" && it.disc != Disc::Elec),
            PlanKind::Power => (it.disc == Disc::Elec && k != "light_fixture" && !it.systems) || (k == "equipment" && it.circuit.is_some()),
            PlanKind::Lighting => k == "light_fixture",
            PlanKind::Systems => it.systems,
            PlanKind::Mechanical => it.disc == Disc::Mech,
            PlanKind::Plumbing => it.disc == Disc::Plumb,
            PlanKind::Fire => it.disc == Disc::Fire,
        }
    }
}

/// A plan's grid lines (drawing units).
#[derive(Default)]
pub(super) struct GridLines {
    pub x: Vec<(String, f64)>,
    pub y: Vec<(String, f64)>,
}

/// The things a plan needs besides its elements.
pub(super) struct PlanCtx<'a> {
    pub upf: f64,
    pub f: Frame,
    pub grids: &'a GridLines,
    /// Every element by tag (upper case), for circuits' panels.
    pub panels: &'a BTreeMap<String, [f64; 2]>,
    /// Floor elevation of the level (drawing units), for ceiling heights.
    pub floor_z: f64,
    /// The plan's middle: tags go to the inside of the walls, clear of the dimension strings.
    pub mid: [f64; 2],
}

/// `n` or its opposite, whichever points from `c` toward `mid`.
fn inward(n: [f64; 2], c: [f64; 2], mid: [f64; 2]) -> [f64; 2] {
    if (mid[0] - c[0]) * n[0] + (mid[1] - c[1]) * n[1] >= 0.0 { n } else { mul(n, -1.0) }
}

fn mapped(f: &Frame, pts: &[[f64; 2]]) -> Vec<[f64; 2]> {
    pts.iter().map(|p| f.p(*p)).collect()
}

/// An outline at least `min` paper inches across (a tiny device still shows).
fn outline(pen: &mut Pen, f: &Frame, it: &Item, min: f64) {
    let (lo, hi) = it.bounds();
    if f.len((hi[0] - lo[0]).max(hi[1] - lo[1])) < min {
        let c = f.p(it.center());
        let h = min / 2.0;
        pen.rect([c[0] - h, c[1] - h], [c[0] + h, c[1] + h]);
    } else {
        pen.pline(&mapped(f, &it.hull), true);
    }
}

/// Upright text angle along `u` (degrees, never upside down).
fn along(u: [f64; 2]) -> f64 {
    let mut a = u[1].atan2(u[0]).to_degrees();
    if a > 90.0 + 1e-6 {
        a -= 180.0;
    } else if a <= -90.0 + 1e-6 {
        a += 180.0;
    }
    a.to_radians()
}

fn size_text(it: &Item, upf: f64) -> String {
    if let Some(s) = it.first(&["size", "diameter", "nominal_size"]) {
        return s.to_string();
    }
    if let (Some(w), Some(h)) = (it.p("width"), it.p("height")) {
        return format!("{w}x{h}");
    }
    let (_, _, short) = it.axis();
    let inches = (short / upf * 12.0).round();
    if inches > 0.0 { format!("{inches}\"") } else { String::new() }
}

fn system_abbrev(it: &Item) -> String {
    it.first(&["system", "service"]).map(str::to_string).unwrap_or_default()
}

/// The walls of a plan, as background (screened, no tags) or the plan's own.
fn walls(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item], own: bool) {
    pen.layer(if own { "A-WALL" } else { "A-WALL-BKGD" });
    for it in items.iter().filter(|it| it.kind == "wall") {
        pen.pline(&mapped(&ctx.f, &it.hull), true);
        if own {
            pen.count("walls");
        }
    }
    if !own {
        pen.layer("A-DOOR-BKGD");
        for it in items.iter().filter(|it| matches!(it.kind.as_str(), "door" | "window")) {
            pen.pline(&mapped(&ctx.f, &it.hull), true);
        }
        pen.layer("A-AREA-IDEN-BKGD");
        for it in items.iter().filter(|it| it.kind == "room") {
            let label = [it.p("name"), it.p("number")].into_iter().flatten().collect::<Vec<_>>().join(" ");
            pen.ctext(ctx.f.p(it.center()), &label, TXT * 0.8);
        }
    }
}

fn wall_tags(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item]) {
    pen.layer("A-WALL-IDEN");
    for it in items.iter().filter(|it| it.kind == "wall") {
        if let Some(t) = it.first(&["type", "type_mark", "wall_type"]) {
            pen.tag(ctx.f.p(it.center()), t, TXT, TagShape::Diamond);
            pen.count("wall_tags");
        }
    }
}

fn doors(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item]) {
    let f = &ctx.f;
    for it in items.iter().filter(|it| it.kind == "door") {
        pen.layer("A-DOOR");
        pen.pline(&mapped(f, &it.hull), true);
        pen.count("doors");
        let (u, long, short) = it.axis();
        let n = [-u[1], u[0]];
        let c = it.center();
        // Hinged at one jamb on the wall's face, swung 90° open.
        let hinge = add(add(c, mul(u, -long / 2.0)), mul(n, short / 2.0));
        let leaf = add(hinge, mul(n, long));
        pen.line(f.p(hinge), f.p(leaf));
        let a0 = u[1].atan2(u[0]);
        pen.arc(f.p(hinge), f.len(long), a0, a0 + std::f64::consts::FRAC_PI_2);
        if let Some(num) = it.first(&["number", "mark", "tag"]) {
            pen.layer("A-DOOR-IDEN");
            let at = f.p(add(c, mul(inward(n, c, ctx.mid), short / 2.0 + ctx.upf * 1.5)));
            pen.tag(at, num, TXT, TagShape::Circle);
            pen.count("door_tags");
        }
    }
}

fn windows(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item]) {
    let f = &ctx.f;
    for it in items.iter().filter(|it| it.kind == "window") {
        pen.layer("A-GLAZ");
        pen.pline(&mapped(f, &it.hull), true);
        pen.count("windows");
        let (u, long, short) = it.axis();
        let c = it.center();
        pen.line(f.p(add(c, mul(u, -long / 2.0))), f.p(add(c, mul(u, long / 2.0))));
        if let Some(t) = it.first(&["type_mark", "type", "number"]) {
            pen.layer("A-GLAZ-IDEN");
            let n = inward([-u[1], u[0]], c, ctx.mid);
            pen.tag(f.p(add(c, mul(n, short / 2.0 + ctx.upf * 1.5))), t, TXT, TagShape::Hexagon);
            pen.count("window_tags");
        }
    }
}

fn hull_area(h: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for (i, p) in h.iter().enumerate() {
        let q = h.get((i + 1) % h.len()).copied().unwrap_or(*p);
        a += p[0] * q[1] - q[0] * p[1];
    }
    (a / 2.0).abs()
}

fn rooms(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item]) {
    pen.layer("A-AREA-IDEN");
    for it in items.iter().filter(|it| it.kind == "room") {
        let c = ctx.f.p(it.center());
        let name = it.p("name").unwrap_or("ROOM");
        pen.ctext([c[0], c[1] + TAG * 0.6], name, TAG * 1.2);
        if let Some(n) = it.p("number") {
            pen.tag([c[0], c[1] - TAG * 1.1], n, TAG, TagShape::Box);
        }
        let area = it.p("area").map(str::to_string).unwrap_or_else(|| format!("{:.0}", hull_area(&it.hull) / (ctx.upf * ctx.upf)));
        let area = if area.chars().all(|c| c.is_ascii_digit() || c == '.') { format!("{area} SF") } else { area };
        pen.ctext([c[0], c[1] - TAG * 3.0], &area, TXT * 0.8);
        pen.count("room_tags");
    }
}

/// Columns, stairs, casework and the rest: their outlines on their own layers.
fn others(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item]) {
    for it in items.iter().filter(|it| !matches!(it.kind.as_str(), "wall" | "door" | "window" | "room" | "ceiling" | "roof" | "floor" | "slab")) {
        let layer = match it.kind.as_str() {
            "column" => "S-COLS",
            "stair" | "stairs" => "A-FLOR-STRS",
            "casework" => "A-FLOR-CASE",
            _ => "A-FLOR-OTHR",
        };
        pen.layer(layer);
        outline(pen, &ctx.f, it, 0.05);
        pen.count("other_elements");
    }
}

/// Dimension strings: wall to wall, opening locations, overall, grid to grid.
fn dimensions(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item]) {
    let f = &ctx.f;
    let walls: Vec<&&Item> = items.iter().filter(|it| it.kind == "wall").collect();
    if walls.is_empty() {
        return;
    }
    let all: Vec<[f64; 2]> = walls.iter().flat_map(|w| w.hull.iter().copied()).collect();
    let (lo, hi) = bounds(&all);
    if !ok(lo) || !ok(hi) {
        return;
    }
    let (plo, phi) = (f.p(lo), f.p(hi));
    pen.layer("A-ANNO-DIMS");
    let ft = |d: f64| ft_in(d / ctx.upf);
    let merge = |mut v: Vec<f64>| {
        v.retain(|x| x.is_finite());
        v.sort_by(f64::total_cmp);
        v.dedup_by(|a, b| (*a - *b).abs() < ctx.upf * 0.25);
        v
    };
    // Wall centrelines, x of the walls running north-south, y of those running east-west.
    let xs = merge(walls.iter().filter(|w| w.axis().0[1].abs() > 0.9).map(|w| w.center()[0]).collect());
    let ys = merge(walls.iter().filter(|w| w.axis().0[0].abs() > 0.9).map(|w| w.center()[1]).collect());
    let string = |pen: &mut Pen, v: &[f64], vertical: bool, at: f64| {
        for pair in v.windows(2).take(MAX_STRING) {
            let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else { continue };
            if vertical {
                let (pa, pb) = (f.p([lo[0], *a]), f.p([lo[0], *b]));
                pen.dim([at, pa[1]], [at, pb[1]], [at, (pa[1] + pb[1]) / 2.0], &ft(b - a), true);
            } else {
                let (pa, pb) = (f.p([*a, lo[1]]), f.p([*b, lo[1]]));
                pen.dim([pa[0], at], [pb[0], at], [(pa[0] + pb[0]) / 2.0, at], &ft(b - a), false);
            }
        }
    };
    string(pen, &xs, false, phi[1] + 0.5);
    string(pen, &ys, true, phi[0] + 0.5);
    // Openings: lo → each door and window centre → hi, along the walls they're in.
    let openings: Vec<&&Item> = items.iter().filter(|it| matches!(it.kind.as_str(), "door" | "window")).collect();
    let mut ox = vec![lo[0], hi[0]];
    ox.extend(openings.iter().filter(|o| o.axis().0[0].abs() > 0.9).map(|o| o.center()[0]));
    let mut oy = vec![lo[1], hi[1]];
    oy.extend(openings.iter().filter(|o| o.axis().0[1].abs() > 0.9).map(|o| o.center()[1]));
    let (ox, oy) = (merge(ox), merge(oy));
    if ox.len() > 2 {
        string(pen, &ox, false, plo[1] - 0.5);
        pen.count("opening_strings");
    }
    if oy.len() > 2 {
        string(pen, &oy, true, plo[0] - 0.5);
        pen.count("opening_strings");
    }
    // Overall.
    pen.dim([plo[0], plo[1] - 1.0], [phi[0], plo[1] - 1.0], [(plo[0] + phi[0]) / 2.0, plo[1] - 1.0], &ft(hi[0] - lo[0]), false);
    pen.dim([plo[0] - 1.0, plo[1]], [plo[0] - 1.0, phi[1]], [plo[0] - 1.0, (plo[1] + phi[1]) / 2.0], &ft(hi[1] - lo[1]), true);
    // Grid to grid.
    let gx: Vec<f64> = ctx.grids.x.iter().map(|g| g.1).collect();
    let gy: Vec<f64> = ctx.grids.y.iter().map(|g| g.1).collect();
    if gx.len() > 1 {
        string(pen, &gx, false, phi[1] + 1.0);
        pen.count("grid_strings");
    }
    if gy.len() > 1 {
        string(pen, &gy, true, phi[0] + 1.0);
        pen.count("grid_strings");
    }
}

fn grids(pen: &mut Pen, ctx: &PlanCtx, lo: [f64; 2], hi: [f64; 2]) {
    let f = &ctx.f;
    let (plo, phi) = (f.p(lo), f.p(hi));
    for (label, x) in &ctx.grids.x {
        let px = f.p([*x, lo[1]])[0];
        pen.layer("S-GRID");
        pen.line([px, plo[1] - 0.3], [px, phi[1] + 1.3]);
        pen.layer("S-GRID-IDEN");
        pen.tag([px, phi[1] + 1.55], label, TAG, TagShape::Circle);
        pen.count("grids");
    }
    for (label, y) in &ctx.grids.y {
        let py = f.p([lo[0], *y])[1];
        pen.layer("S-GRID");
        pen.line([plo[0] - 0.3, py], [phi[0] + 1.3, py]);
        pen.layer("S-GRID-IDEN");
        pen.tag([phi[0] + 1.55, py], label, TAG, TagShape::Circle);
        pen.count("grids");
    }
}

/// Devices on their circuits: a circuit tag at each, the wiring through them, and the home run
/// to the panel.
fn circuits(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item], layer: &str) {
    let f = &ctx.f;
    let mut by: BTreeMap<String, (String, Vec<[f64; 2]>)> = BTreeMap::new();
    for it in items {
        if let Some((key, panel)) = &it.circuit {
            let e = by.entry(key.clone()).or_insert_with(|| (panel.clone(), Vec::new()));
            if e.1.len() < MAX_CIRCUIT {
                e.1.push(it.center());
            }
        }
    }
    for (key, (panel, mut pts)) in by {
        let panel_at = ctx.panels.get(&panel.to_ascii_uppercase()).copied();
        // From the device nearest the panel, each next the nearest left.
        let d2 = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2);
        let start = panel_at.unwrap_or_else(|| pts.first().copied().unwrap_or([0.0, 0.0]));
        let mut chain = Vec::new();
        let mut at = start;
        while !pts.is_empty() {
            let Some((i, _)) = pts.iter().enumerate().min_by(|(_, a), (_, b)| d2(**a, at).total_cmp(&d2(**b, at))) else { break };
            let p = pts.swap_remove(i);
            chain.push(p);
            at = p;
        }
        let Some(first) = chain.first().copied() else { continue };
        pen.layer(&format!("{layer}-WIRE"));
        if chain.len() > 1 {
            let v: Vec<([f64; 2], f64)> = chain.iter().map(|p| (f.p(*p), 0.25)).collect();
            pen.bulged(&v);
        }
        // The home run: an arrow from the first device toward the panel.
        let p0 = f.p(first);
        let dir = match panel_at.map(|q| f.p(q)) {
            Some(q) if (q[0] - p0[0]).hypot(q[1] - p0[1]) > 1e-6 => {
                let l = (q[0] - p0[0]).hypot(q[1] - p0[1]);
                [(q[0] - p0[0]) / l, (q[1] - p0[1]) / l]
            }
            _ => [std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2],
        };
        let tip = add(p0, mul(dir, 0.45));
        pen.line(p0, tip);
        let back = mul(dir, -0.08);
        let side = [-dir[1] * 0.035, dir[0] * 0.035];
        pen.pline(&[tip, add(add(tip, back), side), add(add(tip, back), mul(side, -1.0))], true);
        pen.text(add(tip, [0.04, 0.02]), &key, TXT * 0.9);
        pen.count("home_runs");
        pen.layer(&format!("{layer}-IDEN"));
        for p in &chain {
            let q = f.p(*p);
            pen.text([q[0] + 0.07, q[1] - 0.14], &key, TXT * 0.7);
            pen.count("circuit_tags");
        }
    }
}

fn symbol(pen: &mut Pen, ctx: &PlanCtx, it: &Item, label: Option<&str>) {
    let c = ctx.f.p(it.center());
    pen.circle(c, 0.06);
    if let Some(t) = label {
        pen.text([c[0] + 0.08, c[1] + 0.02], t, TXT * 0.8);
    }
}

fn panels(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item]) {
    pen.layer("E-POWR-EQPM");
    for it in items.iter().filter(|it| it.kind == "equipment") {
        outline(pen, &ctx.f, it, 0.12);
        if let Some(t) = it.tag.as_deref().or(it.p("type_mark")) {
            let (_, hi) = it.bounds();
            let c = ctx.f.p([it.center()[0], hi[1]]);
            pen.ctext([c[0], c[1] + 0.06], t, TAG);
            pen.count("equipment_tags");
        }
        if it.panel {
            pen.count("panels");
        }
    }
}

fn equipment(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item], layer: &str) {
    pen.layer(layer);
    for it in items.iter().filter(|it| it.kind == "equipment") {
        outline(pen, &ctx.f, it, 0.12);
        pen.count("equipment");
        if let Some(t) = it.tag.as_deref().or(it.p("type_mark")) {
            pen.ctext(ctx.f.p(it.center()), t, TAG);
            pen.count("equipment_tags");
        }
    }
}

/// A plan of one level: background, the discipline's elements with their tags, dimensions on
/// the architectural plans, circuits on the electrical ones; the keynotes it found.
pub(super) fn plan(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item], kind: PlanKind, lo: [f64; 2], hi: [f64; 2]) -> Vec<String> {
    let mine: Vec<&Item> = items.iter().copied().filter(|it| kind.shows(it)).collect();
    let f = &ctx.f;
    match kind {
        PlanKind::Floor | PlanKind::Enlarged => {
            grids(pen, ctx, lo, hi);
            walls(pen, ctx, &mine, true);
            doors(pen, ctx, &mine);
            windows(pen, ctx, &mine);
            rooms(pen, ctx, &mine);
            others(pen, ctx, &mine);
            wall_tags(pen, ctx, &mine);
            if kind == PlanKind::Floor {
                dimensions(pen, ctx, &mine);
            }
        }
        PlanKind::Ceiling => {
            walls(pen, ctx, &mine, true);
            pen.layer("A-CLNG");
            for it in mine.iter().filter(|it| it.kind == "ceiling") {
                pen.pline(&mapped(f, &it.hull), true);
                pen.count("ceilings");
                let height = it
                    .first(&["ceiling_height", "height", "elevation"])
                    .map(str::to_string)
                    .unwrap_or_else(|| ft_in((it.z[0] - ctx.floor_z) / ctx.upf));
                let what = it.first(&["type", "type_mark", "finish", "ceiling_finish"]).unwrap_or("CLG");
                pen.layer("A-CLNG-IDEN");
                pen.tag(f.p(it.center()), &format!("{what} @ {height}"), TXT * 0.8, TagShape::Box);
                pen.count("ceiling_tags");
                pen.layer("A-CLNG");
            }
            lights(pen, ctx, &mine, false);
            terminals(pen, ctx, &mine);
        }
        PlanKind::Roof => {
            walls(pen, ctx, items, false);
            pen.layer("A-ROOF");
            for it in mine.iter().filter(|it| it.kind == "roof") {
                pen.pline(&mapped(f, &it.hull), true);
                pen.count("roofs");
                if let Some(s) = it.first(&["slope", "pitch"]) {
                    pen.layer("A-ROOF-IDEN");
                    pen.ctext(f.p(it.center()), &format!("SLOPE {s}"), TXT);
                    pen.layer("A-ROOF");
                }
            }
            equipment(pen, ctx, &mine, "M-EQPM");
        }
        PlanKind::Power => {
            walls(pen, ctx, items, false);
            pen.layer("E-POWR");
            for it in mine.iter().filter(|it| it.kind == "device") {
                symbol(pen, ctx, it, it.first(&["type_mark", "symbol"]));
                pen.count("devices");
            }
            for it in mine.iter().filter(|it| it.kind == "conduit") {
                pipe(pen, ctx, it, "E-POWR-COND");
            }
            panels(pen, ctx, &mine);
            circuits(pen, ctx, &mine, "E-POWR");
        }
        PlanKind::Lighting => {
            walls(pen, ctx, items, false);
            lights(pen, ctx, &mine, true);
            circuits(pen, ctx, &mine, "E-LITE");
        }
        PlanKind::Systems => {
            walls(pen, ctx, items, false);
            pen.layer("E-SYST");
            for it in &mine {
                let c = f.p(it.center());
                pen.pline(&[[c[0], c[1] + 0.07], [c[0] + 0.07, c[1] - 0.05], [c[0] - 0.07, c[1] - 0.05]], true);
                pen.layer("E-SYST-IDEN");
                pen.text([c[0] + 0.09, c[1]], it.first(&["type_mark", "type"]).unwrap_or(""), TXT * 0.8);
                pen.layer("E-SYST");
                pen.count("devices");
            }
        }
        PlanKind::Mechanical => {
            walls(pen, ctx, items, false);
            for it in mine.iter().filter(|it| it.kind == "duct") {
                pen.layer("M-DUCT");
                pen.pline(&mapped(f, &it.hull), true);
                pen.count("ducts");
                let t = size_text(it, ctx.upf);
                if !t.is_empty() {
                    pen.layer("M-DUCT-IDEN");
                    let (u, _, _) = it.axis();
                    let c = f.p(it.center());
                    pen.text_as([c[0], c[1] + 0.03], &t, TXT * 0.8, Align::Center, along(u));
                    pen.count("duct_tags");
                }
            }
            for it in mine.iter().filter(|it| it.kind == "pipe") {
                pipe(pen, ctx, it, "M-PIPE");
            }
            terminals(pen, ctx, &mine);
            equipment(pen, ctx, &mine, "M-EQPM");
        }
        PlanKind::Plumbing => {
            walls(pen, ctx, items, false);
            for it in mine.iter().filter(|it| it.kind == "pipe") {
                pipe(pen, ctx, it, "P-PIPE");
            }
            pen.layer("P-FIXT");
            for it in mine.iter().filter(|it| it.kind == "fixture" || it.kind == "device") {
                outline(pen, f, it, 0.12);
                pen.count("fixtures");
                if let Some(t) = it.first(&["type_mark", "tag", "type"]) {
                    pen.layer("P-FIXT-IDEN");
                    let c = f.p(it.center());
                    pen.text([c[0] + 0.1, c[1] + 0.05], t, TXT);
                    pen.count("fixture_tags");
                    pen.layer("P-FIXT");
                }
            }
            equipment(pen, ctx, &mine, "P-EQPM");
        }
        PlanKind::Fire => {
            walls(pen, ctx, items, false);
            for it in mine.iter().filter(|it| it.kind == "pipe") {
                pipe(pen, ctx, it, "F-PIPE");
            }
            pen.layer("F-SPRN");
            for it in mine.iter().filter(|it| it.sprinkler || !matches!(it.kind.as_str(), "pipe" | "equipment")) {
                pen.circle(f.p(it.center()), 0.05);
                pen.count("heads");
            }
        }
    }
    keynotes(pen, ctx, &mine, kind)
}

fn pipe(pen: &mut Pen, ctx: &PlanCtx, it: &Item, layer: &str) {
    let f = &ctx.f;
    let (u, long, _) = it.axis();
    let c = it.center();
    pen.layer(layer);
    pen.line(f.p(add(c, mul(u, -long / 2.0))), f.p(add(c, mul(u, long / 2.0))));
    pen.count("pipes");
    let t = [size_text(it, ctx.upf), system_abbrev(it)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    if !t.is_empty() {
        pen.layer(&format!("{layer}-IDEN"));
        let p = f.p(c);
        pen.text_as([p[0], p[1] + 0.03], &t, TXT * 0.8, Align::Center, along(u));
        pen.count("pipe_tags");
    }
}

fn lights(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item], tags: bool) {
    let f = &ctx.f;
    for it in items.iter().filter(|it| it.kind == "light_fixture") {
        pen.layer("E-LITE");
        outline(pen, f, it, 0.14);
        pen.count("light_fixtures");
        if tags && let Some(t) = it.first(&["type_mark", "type"]) {
            pen.layer("E-LITE-IDEN");
            let c = f.p(it.center());
            pen.text([c[0] - 0.05, c[1] + 0.1], t, TXT * 0.8);
            pen.count("fixture_tags");
        }
    }
}

fn terminals(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item]) {
    let f = &ctx.f;
    for it in items.iter().filter(|it| it.terminal) {
        pen.layer("M-HVAC-CDFF");
        let (lo, hi) = it.bounds();
        let (a, b) = (f.p(lo), f.p(hi));
        let (a, b) = if (b[0] - a[0]).max(b[1] - a[1]) < 0.12 {
            let c = f.p(it.center());
            ([c[0] - 0.06, c[1] - 0.06], [c[0] + 0.06, c[1] + 0.06])
        } else {
            (a, b)
        };
        pen.rect(a, b);
        pen.line(a, b);
        pen.line([a[0], b[1]], [b[0], a[1]]);
        pen.count("terminals");
        let label = [it.p("type_mark"), it.p("cfm").map(|c| c.trim_end_matches(" cfm"))].into_iter().flatten().collect::<Vec<_>>().join(" ");
        if !label.is_empty() {
            pen.layer("M-HVAC-IDEN");
            let label = if it.p("cfm").is_some() { format!("{label} CFM") } else { label };
            pen.text([b[0] + 0.04, b[1]], &label, TXT * 0.8);
            pen.count("terminal_tags");
        }
    }
}

/// Keynotes from the elements' properties: fire ratings, GFCI, dampers and explicit notes; a
/// numbered square at each element and the list for the legend.
fn keynotes(pen: &mut Pen, ctx: &PlanCtx, items: &[&Item], kind: PlanKind) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    let layer = format!("{}-ANNO-KEYN", kind.disc().prefix());
    for it in items {
        let note = if let Some(n) = it.first(&["keynote", "note"]) {
            Some(n.to_ascii_uppercase())
        } else if matches!(it.kind.as_str(), "wall" | "door")
            && matches!(kind, PlanKind::Floor | PlanKind::Enlarged)
            && let Some(r) = it.p("fire_rating").filter(|r| !matches!(r.to_ascii_lowercase().as_str(), "none" | "0" | "n/a" | "-"))
        {
            Some(format!("{} FIRE-RATED {} ASSEMBLY", r.to_ascii_uppercase(), it.kind.to_ascii_uppercase()))
        } else if kind == PlanKind::Power && it.words.contains("gfci") {
            Some("GFCI PROTECTED RECEPTACLE".into())
        } else if kind == PlanKind::Mechanical && it.kind == "duct" {
            it.first(&["damper", "fire_damper"]).map(|d| format!("{} DAMPER", d.to_ascii_uppercase()))
        } else {
            None
        };
        let Some(note) = note else { continue };
        let i = match notes.iter().position(|n| *n == note) {
            Some(i) => i,
            None => {
                if notes.len() >= 30 {
                    continue;
                }
                notes.push(note);
                notes.len() - 1
            }
        };
        pen.layer(&layer);
        let c = ctx.f.p(it.center());
        pen.tag([c[0] + 0.18, c[1] + 0.18], &(i + 1).to_string(), TXT * 0.8, TagShape::Box);
        pen.count("keynote_tags");
    }
    notes
}

/// The view's title under it: a numbered bubble, the title, its scale and a scale bar.
pub(super) fn view_title(pen: &mut Pen, at: [f64; 2], num: usize, title: &str, per_ft: Option<f64>) {
    let [x, y] = at;
    pen.layer("G-ANNO-TEXT");
    pen.tag([x + 0.25, y + 0.25], &num.to_string(), 0.12, TagShape::Circle);
    let len = (title.chars().count() as f64 * 0.18 * 0.85).max(2.5);
    pen.line([x + 0.55, y + 0.22], [x + 0.65 + len, y + 0.22]);
    pen.text([x + 0.6, y + 0.3], title, 0.18);
    match per_ft {
        Some(k) if k > 0.0 => {
            pen.text([x + 0.6, y + 0.04], &format!("SCALE: {}", scale_label(k)), 0.1);
            scale_bar(pen, [x + 0.65 + len + 0.3, y + 0.1], k);
        }
        _ => pen.text([x + 0.6, y + 0.04], "NOT TO SCALE", 0.1),
    }
}

/// A bar of four steps, each a round number of feet, about 2" long in all.
fn scale_bar(pen: &mut Pen, at: [f64; 2], per_ft: f64) {
    let step = [1.0, 2.0, 4.0, 5.0, 8.0, 10.0, 16.0, 20.0, 25.0, 32.0, 50.0, 64.0, 100.0, 200.0]
        .into_iter()
        .find(|s| s * per_ft * 4.0 >= 1.5)
        .unwrap_or(200.0);
    let w = step * per_ft;
    pen.layer("G-ANNO-SYMB");
    for i in 0..4 {
        let x0 = at[0] + f64::from(i) * w;
        pen.rect([x0, at[1]], [x0 + w, at[1] + 0.06]);
        if i % 2 == 0 {
            pen.line([x0, at[1] + 0.03], [x0 + w, at[1] + 0.03]);
        }
    }
    for i in [0, 1, 2, 4] {
        let x0 = at[0] + f64::from(i) * w;
        pen.ctext([x0, at[1] + 0.1], &format!("{}'", step * f64::from(i)), 0.07);
    }
    pen.count("scale_bars");
}

pub(super) fn north_arrow(pen: &mut Pen, c: [f64; 2]) {
    pen.layer("G-ANNO-SYMB");
    pen.circle(c, 0.35);
    pen.pline(&[[c[0], c[1] + 0.32], [c[0] + 0.14, c[1] - 0.22], [c[0], c[1] - 0.1], [c[0] - 0.14, c[1] - 0.22]], true);
    pen.ctext([c[0], c[1] + 0.42], "N", 0.16);
    pen.count("north_arrows");
}

/// A numbered list down from `top` at `x`, wrapped to `width` characters; the y below it.
pub(super) fn list(pen: &mut Pen, x: f64, top: f64, heading: &str, rows: &[String], width: usize, bottom: f64) -> f64 {
    if rows.is_empty() {
        return top;
    }
    pen.text([x, top], heading, 0.12);
    let mut y = top - 0.25;
    for (i, r) in rows.iter().enumerate() {
        let mut first = true;
        for line in wrap(r, width) {
            if y < bottom {
                return y;
            }
            let lead = if first { format!("{}.", i + 1) } else { String::new() };
            pen.text([x, y], &lead, TXT);
            pen.text([x + 0.3, y], &line, TXT);
            first = false;
            y -= 0.16;
        }
    }
    y - 0.2
}

pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > width {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ─── Sections, elevations, the 3D view ──────────────────────────────────────

/// A vertical view along a line: points `(s, z)` in drawing units, `s` along `u` from `a`.
pub(super) struct Upright {
    pub a: [f64; 2],
    pub u: [f64; 2],
    /// Looking along `n` (to the left of `u`).
    pub n: [f64; 2],
    pub len: f64,
}

impl Upright {
    pub fn new(a: [f64; 2], b: [f64; 2]) -> Option<Upright> {
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if !len.is_finite() || len < 1e-6 {
            return None;
        }
        let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        Some(Upright { a, u, n: [-u[1], u[0]], len })
    }
    fn view(&self) -> View {
        View { dir: Vec3::new(-self.n[0], -self.n[1], 0.0), right: Vec3::new(self.u[0], self.u[1], 0.0), up: Vec3::new(0.0, 0.0, 1.0) }
    }
    /// How far in front of the line `p` is (along `n`).
    pub fn depth(&self, p: [f64; 2]) -> f64 {
        (p[0] - self.a[0]) * self.n[0] + (p[1] - self.a[1]) * self.n[1]
    }
    fn s(&self, p: [f64; 2]) -> f64 {
        (p[0] - self.a[0]) * self.u[0] + (p[1] - self.a[1]) * self.u[1]
    }
    /// Where the line crosses the outline: the `s` range.
    pub fn cut(&self, hull: &[[f64; 2]]) -> Option<(f64, f64)> {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for (i, p) in hull.iter().enumerate() {
            let q = hull.get((i + 1) % hull.len()).copied().unwrap_or(*p);
            let (dp, dq) = (self.depth(*p), self.depth(q));
            if dp.abs() < 1e-9 {
                lo = lo.min(self.s(*p));
                hi = hi.max(self.s(*p));
            }
            if (dp < 0.0) != (dq < 0.0) && (dp - dq).abs() > 1e-12 {
                let t = dp / (dp - dq);
                let x = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
                lo = lo.min(self.s(x));
                hi = hi.max(self.s(x));
            }
        }
        let (lo, hi) = (lo.max(0.0), hi.min(self.len));
        (lo.is_finite() && hi.is_finite() && hi > lo).then_some((lo, hi))
    }
}

/// The hidden-line drawing of the elements `keep` from `view`: visible lines in the view's 2D
/// coordinates.
fn hlr(d: &Drawing, keep: &HashSet<Handle>, view: &View) -> Vec<[[f64; 2]; 2]> {
    let sc = Scene3::of_space(d, &Space::Model, view, |e| keep.contains(&e.handle));
    let (vis, _) = hidden_lines(&sc, view);
    vis.into_iter().map(|[a, b]| [[a.x, a.y], [b.x, b.y]]).filter(|[a, b]| ok(*a) && ok(*b)).collect()
}

/// What an upright view is drawn from, before it is put on paper (drawing units, `(s, z)`).
#[derive(Default)]
pub(super) struct UprightDrawing {
    /// Cut outlines, by layer.
    pub cut: Vec<(&'static str, [[f64; 2]; 2])>,
    pub beyond: Vec<[[f64; 2]; 2]>,
    pub labels: Vec<([f64; 2], String)>,
}

impl UprightDrawing {
    pub fn bounds(&self) -> ([f64; 2], [f64; 2]) {
        let mut pts: Vec<[f64; 2]> = self.cut.iter().flat_map(|(_, r)| [r[0], r[1]]).collect();
        pts.extend(self.beyond.iter().flat_map(|l| [l[0], l[1]]));
        bounds(&pts)
    }
}

/// A building section along `line`: the elements it cuts (outlined), the model beyond it
/// (hidden lines removed), the rooms it passes through named.
pub(super) fn section(d: &Drawing, items: &[Item], line: &Upright, depth: f64) -> UprightDrawing {
    let mut out = UprightDrawing::default();
    let mut beyond = HashSet::new();
    for it in items {
        if matches!(it.kind.as_str(), "device" | "light_fixture" | "fixture" | "conduit") {
            continue;
        }
        let Some((s0, s1)) = line.cut(&it.hull) else {
            let dd = line.depth(it.center());
            if dd > 0.0 && dd < depth && it.kind != "room" && beyond.len() < MAX_HLR {
                beyond.insert(it.handle);
            }
            continue;
        };
        if it.kind == "room" {
            if let Some(name) = it.p("name") {
                let label = [Some(name), it.p("number")].into_iter().flatten().collect::<Vec<_>>().join(" ");
                out.labels.push(([(s0 + s1) / 2.0, it.z[0] + (it.z[1] - it.z[0]) * 0.4], label));
            }
            continue;
        }
        let layer = match it.kind.as_str() {
            "duct" | "equipment" => "M-SECT",
            "pipe" => "P-SECT",
            "wall" | "column" => "A-SECT-WALL",
            "door" | "window" => "A-SECT-OPNG",
            _ => "A-SECT",
        };
        out.cut.push((layer, [[s0, it.z[0]], [s1, it.z[1]]]));
    }
    let view = line.view();
    let off = line.a[0] * line.u[0] + line.a[1] * line.u[1];
    out.beyond = hlr(d, &beyond, &view).into_iter().map(|[a, b]| [[a[0] - off, a[1]], [b[0] - off, b[1]]]).collect();
    out
}

/// An exterior elevation looking along `line.n`: every building element's visible lines.
pub(super) fn elevation(d: &Drawing, items: &[Item], line: &Upright) -> UprightDrawing {
    let keep: HashSet<Handle> = items
        .iter()
        .filter(|it| matches!(it.kind.as_str(), "wall" | "door" | "window" | "roof" | "floor" | "slab" | "column" | "stair" | "equipment"))
        .take(MAX_HLR)
        .map(|it| it.handle)
        .collect();
    let off = line.a[0] * line.u[0] + line.a[1] * line.u[1];
    let beyond = hlr(d, &keep, &line.view()).into_iter().map(|[a, b]| [[a[0] - off, a[1]], [b[0] - off, b[1]]]).collect();
    UprightDrawing { beyond, ..UprightDrawing::default() }
}

/// The 3D view from the south-east, hidden lines removed.
pub(super) fn axon(d: &Drawing, items: &[Item]) -> Vec<[[f64; 2]; 2]> {
    let keep: HashSet<Handle> = items
        .iter()
        .filter(|it| !matches!(it.kind.as_str(), "room" | "ceiling" | "device" | "light_fixture" | "conduit"))
        .take(MAX_HLR)
        .map(|it| it.handle)
        .collect();
    match View::new(Vec3::new(1.0, -1.0, 1.0)) {
        Some(v) => hlr(d, &keep, &v),
        None => Vec::new(),
    }
}

/// An upright view on paper through `f` (`(s, z)` → paper), with the levels marked at their
/// elevations (drawing units) across it.
pub(super) fn put_upright(pen: &mut Pen, f: &Frame, v: &UprightDrawing, levels: &[(String, f64)], upf: f64, beyond_layer: &str) {
    let (lo, hi) = v.bounds();
    if !ok(lo) || !ok(hi) {
        return;
    }
    for (layer, [a, b]) in &v.cut {
        pen.layer(layer);
        pen.rect(f.p(*a), f.p(*b));
        pen.count("cut");
    }
    pen.layer(beyond_layer);
    for [a, b] in &v.beyond {
        pen.line(f.p(*a), f.p(*b));
    }
    pen.layer("A-SECT-IDEN");
    for (at, t) in &v.labels {
        pen.ctext(f.p(*at), t, TXT);
        pen.count("room_tags");
    }
    // The levels: a line across with a target at the end and the name and elevation.
    let (x0, x1) = (f.p(lo)[0] - 0.4, f.p(hi)[0] + 0.4);
    for (name, z) in levels {
        let y = f.p([lo[0], *z])[1];
        pen.layer("A-ANNO-LEVL");
        pen.line([x0, y], [x1, y]);
        pen.circle([x1 + 0.12, y], 0.1);
        pen.line([x1 + 0.02, y], [x1 + 0.22, y]);
        pen.text([x1 + 0.3, y + 0.05], name, 0.1);
        pen.text([x1 + 0.3, y - 0.12], &format!("EL. {}", ft_in(*z / upf)), 0.08);
        pen.count("level_markers");
    }
}
