//! A wall's exterior finish layers (its cladding) as pieces, over each face's net area with the
//! openings taken out: stucco by the panel between its control joints, with the lath and the
//! weather barrier under it, the screed at its base, casing bead round the openings and corner
//! aid at the corners; siding by the course; panels and EIFS boards by the sheet; masonry veneer
//! by the course with its ties; any other finish layer by area.
//!
//! Which system a layer is, what it needs under it and its stock come from the trade rules
//! (`cladding` in tradeatoms.json, each with its code or trade practice source). A lath or a
//! barrier the assembly lists itself is laid out as itself; one it leaves out is laid in by the
//! system's rule and marked inferred with that rule as its basis.

use super::*;

/// The categories the other layouts take; a face layer of any other is a finish layer here.
pub(super) const LAID_ELSEWHERE: &[&str] = &["framing", "masonry", "insulation", "sheathing", "furring", "board"];
/// Most pieces one layer of one face gets.
const MAX_PER_FACE: usize = 5_000;

/// How a system's finish is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// Panels between control joints (plaster).
    Panels,
    /// Sheets or boards by their stock size.
    Sheets,
    /// Courses at the exposure, cut from boards.
    Courses,
    /// Masonry units by the course band.
    Units,
}

/// One exterior finish system of the rules.
#[derive(Clone, Debug)]
struct System {
    name: String,
    trade: &'static str,
    kind: &'static str,
    layout: Layout,
    /// Stock (feet): across, along its length.
    stock: (f64, f64),
    horizontal: bool,
    exposure_in: f64,
    coats: f64,
    lath: bool,
    plies: f64,
    plies_over_wood: f64,
    screed: bool,
    casing: bool,
    corner_aid: bool,
    joint_max_ft: f64,
    panel_max_sf: f64,
    units_per_sf: f64,
    tie_sf: f64,
    basis: String,
}

impl System {
    fn of(v: &Value) -> Option<System> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let n = |k: &str, d: f64| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).unwrap_or(d);
        let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let name = s("system");
        if name.is_empty() {
            return None;
        }
        let stock = match v.get("stock_ft") {
            Some(Value::Array(a)) => (
                a.first().and_then(Value::as_f64).filter(|x| *x > 0.1).unwrap_or(4.0),
                a.get(1).and_then(Value::as_f64).filter(|x| *x > 0.1).unwrap_or(8.0),
            ),
            Some(x) => (0.0, x.as_f64().filter(|x| *x > 0.1).unwrap_or(12.0)),
            None => (4.0, 8.0),
        };
        Some(System {
            trade: if s("trade") == "masonry" { "masonry" } else { "cladding" },
            kind: match s("kind").as_str() {
                "plaster" => "plaster",
                "panel" => "panel",
                "siding" => "siding",
                "brick" => "brick",
                _ => "cladding",
            },
            layout: match s("layout").as_str() {
                "panels" => Layout::Panels,
                "courses" => Layout::Courses,
                "units" => Layout::Units,
                _ => Layout::Sheets,
            },
            stock,
            horizontal: s("hung") == "horizontal",
            exposure_in: n("exposure_in", 7.0).clamp(1.0, 48.0),
            coats: n("coats", 1.0).clamp(1.0, 6.0).round(),
            lath: b("lath"),
            plies: n("barrier_plies", 0.0).clamp(0.0, 4.0).round(),
            plies_over_wood: n("barrier_plies_over_wood", n("barrier_plies", 0.0)).clamp(0.0, 4.0).round(),
            screed: b("screed"),
            casing: b("casing_bead"),
            corner_aid: b("corner_aid"),
            joint_max_ft: n("control_joint_max_ft", 0.0),
            panel_max_sf: n("control_panel_max_sf", 0.0),
            units_per_sf: n("units_per_sf", 0.0),
            tie_sf: n("tie_sf", 0.0),
            basis: s("source"),
            name,
        })
    }
}

/// What a face layer is.
#[derive(Clone, Debug)]
enum What {
    System(Box<System>),
    Lath,
    Barrier,
    Other,
}

/// Text as whole words: upper case, anything but letters and digits a space, a space each side.
fn spaced(s: &str) -> String {
    let w: Vec<String> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect();
    format!(" {} ", w.join(" "))
}

fn has_any(text: &str, words: &[String]) -> bool {
    words.iter().map(|w| spaced(w)).any(|w| !w.trim().is_empty() && text.contains(w.as_str()))
}

fn what(r: &Rules, l: &AssemblyLayer) -> What {
    let text = spaced(&l.material);
    for sy in r.list("cladding.systems").iter().take(32) {
        let words: Vec<String> =
            sy.get("words").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        if has_any(&text, &words)
            && let Some(s) = System::of(sy)
        {
            return What::System(Box::new(s));
        }
    }
    if has_any(&text, &r.words("cladding.lath_words")) {
        return What::Lath;
    }
    let cat = l.category.trim().to_ascii_uppercase();
    if has_any(&text, &r.words("cladding.barrier_words")) || r.words("cladding.barrier_categories").contains(&cat) {
        return What::Barrier;
    }
    What::Other
}

/// How many layers a text names (`(2) layers`, `two layers of`, `2-ply`, `double`).
pub(super) fn layers_named(text: &str) -> Option<f64> {
    let t = spaced(text);
    for (w, n) in [("ONE", 1.0), ("TWO", 2.0), ("THREE", 3.0), ("DOUBLE", 2.0), ("SINGLE", 1.0)] {
        if t.contains(&format!(" {w} LAYER")) || t.contains(&format!(" {w} PLY")) || (w == "DOUBLE" && t.contains(" DOUBLE ")) {
            return Some(n);
        }
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    words.windows(2).find_map(|p| {
        let [a, b] = p else { return None };
        let n = a.parse::<f64>().ok().filter(|n| (1.0..=4.0).contains(n))?;
        (b.starts_with("LAYER") || b.starts_with("PLY")).then_some(n)
    })
}

/// What lays a face's pieces out: the wall, its rules, its scale, and where the pieces go.
pub(super) struct Face<'a> {
    pub w: &'a Wall,
    pub r: &'a Rules,
    pub k: f64,
    pub upf: f64,
    pub base: &'a dyn Fn(Piece) -> Piece,
    pub src: &'a Source,
    pub want: &'a dyn Fn(&str) -> bool,
    pub jn: [EndJoin; 2],
    pub corner_mine: [bool; 2],
    /// The wall stands on the building's lowest floor (its screed is at the foundation plate).
    pub ground: bool,
    /// The straight run of walls it is part of.
    pub run: Run,
    /// The room each face faces (side 1, side 2; empty: none).
    pub rooms: [String; 2],
}

/// Where a wall is in the straight run of walls it is part of (a facade drawn as several walls):
/// the run's position of its `s0` end and the run's length (sheet units), and whether it runs
/// the other way along it. The default: a run of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Run {
    pub start: f64,
    pub len: f64,
    pub reversed: bool,
}

/// Most walls of one level put in runs (hostile models; the rest are runs of their own).
const MAX_RUN_WALLS: usize = 4_000;

/// The straight runs of the walls marked `want`, level by level: walls on one line whose ends
/// meet (or overlap) are one run.
pub(super) fn runs(walls: &[Wall], want: &[bool]) -> Vec<Run> {
    let mut out = vec![Run::default(); walls.len()];
    let mut by_level: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, w) in walls.iter().enumerate() {
        if want.get(i).copied().unwrap_or(false) {
            by_level.entry(w.level.to_ascii_uppercase()).or_default().push(i);
        }
    }
    for idx in by_level.values() {
        let idx: Vec<usize> = idx.iter().copied().take(MAX_RUN_WALLS).collect();
        let mut parent: Vec<usize> = (0..idx.len()).collect();
        fn root(p: &mut [usize], mut i: usize) -> usize {
            let mut n = 0;
            while p.get(i).is_some_and(|x| *x != i) && n < 10_000 {
                i = p.get(i).copied().unwrap_or(i);
                n += 1;
            }
            i
        }
        for a in 0..idx.len() {
            for b in a + 1..idx.len() {
                let (Some(wa), Some(wb)) = (idx.get(a).and_then(|i| walls.get(*i)), idx.get(b).and_then(|i| walls.get(*i))) else { continue };
                let cross = (wa.u.x * wb.u.y - wa.u.y * wb.u.x).abs();
                if cross > 0.02 || (wb.o - wa.o).dot(wa.n).abs() > wa.t.max(wb.t) {
                    continue;
                }
                let [b0, b1] = wb.ends();
                let (p0, p1) = ((b0 - wa.o).dot(wa.u), (b1 - wa.o).dot(wa.u));
                let gap = wa.t + wb.t;
                if p0.max(p1) < wa.s0 - gap || p0.min(p1) > wa.s1 + gap {
                    continue;
                }
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                if let Some(x) = parent.get_mut(ra.max(rb)) {
                    *x = ra.min(rb);
                }
            }
        }
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for a in 0..idx.len() {
            let r = root(&mut parent, a);
            groups.entry(r).or_default().push(a);
        }
        for members in groups.values().filter(|m| m.len() > 1) {
            let Some(first) = members.first().and_then(|a| idx.get(*a)).and_then(|i| walls.get(*i)) else { continue };
            let (o, d) = (first.o, first.u);
            let pos = |w: &Wall, s: f64| (w.at(s, 0.0) - o).dot(d);
            let lo = members
                .iter()
                .filter_map(|a| idx.get(*a).and_then(|i| walls.get(*i)))
                .map(|w| pos(w, w.s0).min(pos(w, w.s1)))
                .fold(f64::INFINITY, f64::min);
            let hi = members
                .iter()
                .filter_map(|a| idx.get(*a).and_then(|i| walls.get(*i)))
                .map(|w| pos(w, w.s0).max(pos(w, w.s1)))
                .fold(f64::NEG_INFINITY, f64::max);
            if !(lo.is_finite() && hi > lo) {
                continue;
            }
            for a in members {
                let Some(i) = idx.get(*a).copied() else { continue };
                let Some(w) = walls.get(i) else { continue };
                let (p0, p1) = (pos(w, w.s0), pos(w, w.s1));
                if let Some(slot) = out.get_mut(i) {
                    *slot = Run { start: p0 - lo, len: hi - lo, reversed: p1 < p0 };
                }
            }
        }
    }
    out
}

/// One rectangle of a layout: along the wall `a`–`b`, up it `c`–`e`, and its stock's size.
#[derive(Clone, Copy, Debug)]
struct Rect {
    a: f64,
    b: f64,
    c: f64,
    e: f64,
    full: bool,
}

impl Face<'_> {
    /// Lay out every finish layer of both faces.
    pub(super) fn lay(&self, placed: &[Placed], batch: &mut Batch) {
        for side in [LayerSide::Side1, LayerSide::Side2] {
            self.lay_face(placed, side, batch);
        }
    }

    fn lay_face(&self, placed: &[Placed], side: LayerSide, batch: &mut Batch) {
        let mut layers: Vec<&Placed> = placed
            .iter()
            .filter(|p| p.layer.side == side && !LAID_ELSEWHERE.contains(&p.layer.category.trim().to_ascii_lowercase().as_str()))
            .collect();
        if layers.is_empty() {
            return;
        }
        if side == LayerSide::Side2 {
            layers.reverse();
        }
        let whats: Vec<What> = layers.iter().map(|p| what(self.r, p.layer)).collect();
        let mut lath_done = whats.iter().any(|x| matches!(x, What::Lath));
        let mut barrier_done = whats.iter().any(|x| matches!(x, What::Barrier));
        let wood_words = self.r.words("cladding.wood_sheathing_words");
        let over_wood =
            placed.iter().any(|p| p.layer.side == side && p.layer.category == "sheathing" && has_any(&spaced(&p.layer.material), &wood_words));
        let sign = if side == LayerSide::Side1 { 1.0 } else { -1.0 };
        let face = if side == LayerSide::Side1 { "side 1" } else { "side 2" };
        let clad = (self.want)("cladding");
        let first = batch.pieces.len();
        for (pl, wh) in layers.iter().zip(whats) {
            match wh {
                What::System(sy) => {
                    if (self.want)(sy.trade) {
                        self.finish(pl, &sy, face, batch);
                    }
                    if !clad {
                        continue;
                    }
                    // Its inner face: what it needs under it goes there.
                    let inner = pl.off - sign * pl.depth / 2.0;
                    if sy.lath && !lath_done {
                        let t = (0.25 * self.k).min(pl.depth * 0.5).max(1e-6);
                        let why = format!("{} ({} on its lath)", self.r.source_text("cladding.lath_item"), sy.name);
                        self.lath(inner + sign * t / 2.0, t, &pl.layer.material, face, Some(why), batch);
                        lath_done = true;
                    }
                    let named = has_any(&spaced(&pl.layer.material), &self.r.words("cladding.barrier_words"))
                        .then(|| layers_named(&pl.layer.material))
                        .flatten();
                    let plies = named.unwrap_or(if over_wood { sy.plies_over_wood } else { sy.plies });
                    if plies > 0.0 && !barrier_done {
                        let why = format!(
                            "{} ({} layer{} under {} over {})",
                            self.r.source_text("cladding.barrier_item"),
                            plies,
                            if plies > 1.0 { "s" } else { "" },
                            sy.name,
                            if over_wood { "wood-based sheathing" } else { "its backing" }
                        );
                        self.barrier(inner, sign, plies as usize, face, Some(why), batch);
                        barrier_done = true;
                    }
                    if sy.screed {
                        self.screed(pl, face, batch);
                    }
                    if sy.casing {
                        self.casing(pl, face, batch);
                    }
                    if sy.corner_aid {
                        self.corner_aids(pl, face, batch);
                    }
                }
                What::Lath if clad => {
                    self.lath(pl.off, pl.depth.max(0.05 * self.k), &pl.layer.material, face, None, batch);
                }
                What::Barrier if clad => {
                    let plies = layers_named(&pl.layer.material).unwrap_or(1.0) as usize;
                    self.barrier(pl.off + sign * pl.depth / 2.0, sign, plies, face, None, batch);
                }
                What::Other if clad => {
                    let st = self.r.nums("cladding.other_stock_ft");
                    let sy = System {
                        name: "finish".into(),
                        trade: "cladding",
                        kind: "cladding",
                        layout: Layout::Sheets,
                        stock: (st.first().copied().unwrap_or(4.0), st.get(1).copied().unwrap_or(8.0)),
                        horizontal: false,
                        exposure_in: 0.0,
                        coats: 1.0,
                        lath: false,
                        plies: 0.0,
                        plies_over_wood: 0.0,
                        screed: false,
                        casing: false,
                        corner_aid: false,
                        joint_max_ft: 0.0,
                        panel_max_sf: 0.0,
                        units_per_sf: 0.0,
                        tie_sf: 0.0,
                        basis: self.r.source_text("cladding.other_stock_ft"),
                    };
                    self.finish(pl, &sy, face, batch);
                    let note = format!(
                        "finish layer `{}`: no cladding system names it, laid out by area",
                        pl.layer.material.chars().take(60).collect::<String>()
                    );
                    if !batch.notes.contains(&note) && batch.notes.len() < 200 {
                        batch.notes.push(note);
                    }
                }
                _ => {}
            }
        }
        // The pieces face the face's room (none outside).
        let room = self.rooms.get(usize::from(side == LayerSide::Side2)).cloned().unwrap_or_default();
        for p in batch.pieces.iter_mut().skip(first).filter(|p| p.room.is_empty()) {
            p.room = room.clone();
        }
    }

    /// Rows `[z, z + pitch_z]` up the wall and pieces `[s, s + pitch_s]` along each (sheet units);
    /// `full`: the rectangle is its whole pitch (not cut at the wall's edge).
    fn grid(&self, pitch_s: f64, pitch_z: f64) -> Vec<Rect> {
        let w = self.w;
        let mut out = Vec::new();
        if !(pitch_s.is_finite() && pitch_z.is_finite() && pitch_s > 1e-6 && pitch_z > 1e-6) {
            return out;
        }
        let mut z = w.zf;
        while z < w.zt - 1e-6 && out.len() < MAX_PER_FACE {
            let e = (z + pitch_z).min(w.zt);
            let mut s = w.s0;
            while s < w.s1 - 1e-6 && out.len() < MAX_PER_FACE {
                let b = (s + pitch_s).min(w.s1);
                out.push(Rect { a: s, b, c: z, e, full: b - s >= pitch_s - 1e-6 && e - z >= pitch_z - 1e-6 });
                s = b;
            }
            z = e;
        }
        out
    }

    /// The net area (sheet units²) of a rectangle and the area its openings take out of it;
    /// `None` when the openings take (nearly) all of it.
    fn net(&self, r: &Rect) -> Option<(f64, f64)> {
        let gross = (r.b - r.a) * (r.e - r.c);
        let open: f64 = self.w.holes.iter().map(|h| h.overlap(r.a, r.b, r.c, r.e)).sum();
        (gross > 1e-9 && open < gross * 0.98).then_some((gross - open, open))
    }

    fn piece(&self, trade: &'static str, kind: &'static str, item: String, source: Source, inferred: Option<&str>) -> Piece {
        let mut p = (self.base)(Piece::new(trade, kind, item, &self.w.level, source));
        p.inferred = inferred.map(str::to_string);
        p
    }

    /// The pieces of a rectangle's body: the rectangle less the openings, `off` and `depth` across
    /// the wall.
    fn body(&self, p: &mut Piece, r: &Rect, off: f64, depth: f64) {
        for (ra, rb, rc, re) in minus_holes(r.a, r.b, r.c, r.e, &self.w.holes) {
            p.blocks.push(self.w.block(ra, rb, off, depth, rc, re));
        }
    }

    /// A system's own finish pieces.
    fn finish(&self, pl: &Placed, sy: &System, face: &str, batch: &mut Batch) {
        let (w, k, upf) = (self.w, self.k, self.upf);
        let sf = upf * upf;
        let th_in = layer_depth(pl.layer);
        let item = pl.layer.material.clone();
        let source = self.src.clone();
        let system_src = super::super::super::mepatoms::rule_source(&sy.basis);
        match sy.layout {
            Layout::Panels => {
                // Panels no wider or higher than the joint spacing, and no bigger than the most
                // area a panel may have.
                let (len, h) = (w.len(), w.zt - w.zf);
                let max_ft = if sy.joint_max_ft > 0.0 { sy.joint_max_ft } else { f64::INFINITY };
                let rows = ((h / upf) / max_ft).ceil().clamp(1.0, 64.0);
                let row_h = h / rows;
                let by_area = if sy.panel_max_sf > 0.0 { sy.panel_max_sf / (row_h / upf) } else { f64::INFINITY };
                let wide = max_ft.min(by_area).max(1.0);
                // The vertical joints: the straight run of walls the wall is part of is divided
                // into equal panels (a wall alone is its own run), so they fall where the
                // facade's spacing puts them, not where the wall pieces end.
                let run_len = if self.run.len > len { self.run.len } else { len };
                let (start, dir) = if self.run.len > len { (self.run.start, if self.run.reversed { -1.0 } else { 1.0 }) } else { (0.0, 1.0) };
                let cols = ((run_len / upf) / wide).ceil().clamp(1.0, MAX_PER_FACE as f64);
                let col_w = run_len / cols;
                let tol = 0.25 * upf;
                let mut cuts: Vec<f64> =
                    (1..cols as usize).map(|m| w.s0 + (col_w * m as f64 - start) * dir).filter(|s| *s > w.s0 + tol && *s < w.s1 - tol).collect();
                cuts.sort_by(f64::total_cmp);
                let edges: Vec<f64> = std::iter::once(w.s0).chain(cuts.iter().copied()).chain(std::iter::once(w.s1)).collect();
                for ri in 0..rows as usize {
                    for pair in edges.windows(2) {
                        let [a, b] = pair else { continue };
                        let r = Rect { a: *a, b: *b, c: w.zf + row_h * ri as f64, e: w.zf + row_h * (ri + 1) as f64, full: true };
                        let Some((net, open)) = self.net(&r) else { continue };
                        let mut p = self.piece(sy.trade, sy.kind, item.clone(), source.clone(), None);
                        p.ty = format!("{} coat{}", num_text(sy.coats), if sy.coats > 1.0 { "s" } else { "" });
                        p.size = inches_text(th_in);
                        p.area_sf = net / sf;
                        p.volume_cf = net / sf * th_in / 12.0;
                        p = p
                            .with("coats", num_text(sy.coats), system_src.clone())
                            .with("system", sy.name.clone(), system_src.clone())
                            .computed("face", face)
                            .computed("panel_ft", format!("{}x{}", num_text((r.b - r.a) / upf), num_text((r.e - r.c) / upf)));
                        if open > 0.0 {
                            p = p.computed("opening_cut_sf", num_text(open / sf));
                        }
                        self.body(&mut p, &r, pl.off, pl.depth);
                        batch.push(p);
                    }
                }
                // The control joints between the panels, less what falls in the openings.
                let why = self.r.source_text("cladding.control_joint_item");
                let jw = 1.0 * k;
                for s in cuts {
                    let open: f64 =
                        w.holes.iter().filter(|x| s > x.s0 && s < x.s1).map(|x| x.z1.min(w.zt) - x.z0.max(w.zf)).filter(|v| *v > 0.0).sum();
                    let run = (h - open).max(0.0);
                    if run <= 1e-6 {
                        continue;
                    }
                    let mut p = self.piece(
                        "cladding",
                        "control_joint",
                        self.r.text("cladding.control_joint_item"),
                        self.r.source("cladding.control_joint_item"),
                        Some(&why),
                    );
                    p.length_ft = run / upf;
                    p = p.computed("face", face).computed("at", "vertical joint between panels");
                    let r = Rect { a: s - jw / 2.0, b: s + jw / 2.0, c: w.zf, e: w.zt, full: true };
                    self.body(&mut p, &r, pl.off, pl.depth);
                    batch.push(p);
                }
                for ri in 1..rows as usize {
                    let z = w.zf + row_h * ri as f64;
                    let open: f64 =
                        w.holes.iter().filter(|x| z > x.z0 && z < x.z1).map(|x| x.s1.min(w.s1) - x.s0.max(w.s0)).filter(|v| *v > 0.0).sum();
                    let run = (len - open).max(0.0);
                    if run <= 1e-6 {
                        continue;
                    }
                    let mut p = self.piece(
                        "cladding",
                        "control_joint",
                        self.r.text("cladding.control_joint_item"),
                        self.r.source("cladding.control_joint_item"),
                        Some(&why),
                    );
                    p.length_ft = run / upf;
                    p = p.computed("face", face).computed("at", "horizontal joint between panels");
                    let r = Rect { a: w.s0, b: w.s1, c: z - jw / 2.0, e: z + jw / 2.0, full: true };
                    self.body(&mut p, &r, pl.off, pl.depth);
                    batch.push(p);
                }
            }
            Layout::Sheets => {
                let (across, along) = (sy.stock.0 * upf, sy.stock.1 * upf);
                let (ps, pz) = if sy.horizontal { (along, across) } else { (across, along) };
                for r in self.grid(ps, pz) {
                    let Some((net, open)) = self.net(&r) else { continue };
                    let mut p =
                        self.piece(sy.trade, sy.kind, format!("{item}, {}x{}", num_text(sy.stock.0), num_text(sy.stock.1)), source.clone(), None);
                    p.size = inches_text(th_in);
                    p.area_sf = net / sf;
                    p = p
                        .with("sheet_ft", format!("{}x{}", num_text(sy.stock.0), num_text(sy.stock.1)), system_src.clone())
                        .with("system", sy.name.clone(), system_src.clone())
                        .computed("cut", if !r.full || open > 0.0 { "true" } else { "false" })
                        .computed("face", face);
                    self.body(&mut p, &r, pl.off, pl.depth);
                    batch.push(p);
                }
            }
            Layout::Courses => {
                let board_ft = if sy.stock.1 > 0.1 { sy.stock.1 } else { 12.0 };
                for r in self.grid(w.len(), sy.exposure_in * k) {
                    // The runs of the course between the openings it meets.
                    let cuts = geo::merge(w.holes.iter().filter(|h| h.z0 < r.e - 1e-6 && h.z1 > r.c + 1e-6).map(|h| (h.s0, h.s1)).collect());
                    let mut runs: Vec<(f64, f64)> = Vec::new();
                    let mut at = w.s0;
                    for (a, b) in cuts {
                        if a > at {
                            runs.push((at, a));
                        }
                        at = at.max(b);
                    }
                    if w.s1 > at {
                        runs.push((at, w.s1));
                    }
                    let run_ft: f64 = runs.iter().map(|(a, b)| (b - a) / upf).sum();
                    if run_ft <= 1e-6 {
                        continue;
                    }
                    let boards: f64 = runs.iter().map(|(a, b)| ((b - a) / upf / board_ft).ceil()).sum();
                    let mut p = self.piece(sy.trade, sy.kind, item.clone(), source.clone(), None);
                    p.size = format!("{} exposure", inches_text(sy.exposure_in));
                    p.length_ft = run_ft;
                    p.area_sf = run_ft * (r.e - r.c) / upf;
                    p.qty = boards.max(1.0);
                    p = p
                        .with("board_ft", num_text(board_ft), system_src.clone())
                        .with("exposure_in", num_text(sy.exposure_in), system_src.clone())
                        .with("system", sy.name.clone(), system_src.clone())
                        .computed("face", face);
                    for (a, b) in runs.iter().take(32) {
                        p.blocks.push(w.block(*a, *b, pl.off, pl.depth, r.c, r.e));
                    }
                    batch.push(p);
                }
            }
            Layout::Units => {
                let band = self.r.num("masonry.brick_course_in", 8.0).clamp(1.0, 48.0) * k;
                let mut total = 0.0;
                for r in self.grid(w.len(), band) {
                    let Some((net, _)) = self.net(&r) else { continue };
                    let net_sf = net / sf;
                    total += net_sf;
                    let mut p = self.piece(sy.trade, sy.kind, item.clone(), source.clone(), None);
                    p.size = inches_text(th_in);
                    p.area_sf = net_sf;
                    p.length_ft = net / (r.e - r.c).max(1e-9) / upf;
                    p.qty = (net_sf * sy.units_per_sf).ceil().max(1.0);
                    p = p
                        .with("units_per_sf", num_text(sy.units_per_sf), system_src.clone())
                        .with("system", sy.name.clone(), system_src.clone())
                        .computed("face", face);
                    self.body(&mut p, &r, pl.off, pl.depth);
                    batch.push(p);
                }
                if sy.tie_sf > 0.0 && total > 0.0 {
                    let mut p = self.piece(sy.trade, "tie", self.r.text("cladding.tie_item"), system_src.clone(), Some(&sy.basis));
                    p.qty = (total / sy.tie_sf).ceil();
                    p = p.with("tie_sf", num_text(sy.tie_sf), system_src.clone()).computed("face", face);
                    let sign = if face == "side 1" { 1.0 } else { -1.0 };
                    let inner = pl.off - sign * pl.depth / 2.0;
                    let r = Rect { a: w.s0, b: w.s0 + 2.0 * k, c: w.zf, e: w.zt, full: true };
                    self.body(&mut p, &r, inner, 0.25 * k);
                    batch.push(p);
                }
            }
        }
    }

    /// Lath sheets over the face, the long side along the wall, lapped at their sides and ends;
    /// `inferred`: why it is laid in (none: the assembly lists it).
    fn lath(&self, off: f64, depth: f64, material: &str, face: &str, inferred: Option<String>, batch: &mut Batch) {
        let (k, upf) = (self.k, self.upf);
        let size = self.r.nums("cladding.lath_sheet_in");
        let lap = self.r.list("cladding.lath_lap_in");
        let (sw, sl) = (size.first().copied().unwrap_or(27.0), size.get(1).copied().unwrap_or(96.0));
        let lap_n = |i: usize| lap.get(i).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0);
        let (side_lap, end_lap) = (lap_n(0).min(sw * 0.5), lap_n(1).min(sl * 0.5));
        let item = if inferred.is_some() { self.r.text("cladding.lath_item") } else { material.to_string() };
        let src = if inferred.is_some() { self.r.source("cladding.lath_item") } else { self.src.clone() };
        for r in self.grid((sl - end_lap) * k, (sw - side_lap) * k) {
            let Some((net, open)) = self.net(&r) else { continue };
            let mut p = self.piece("cladding", "lath", format!("{item}, {}x{} in", num_text(sw), num_text(sl)), src.clone(), inferred.as_deref());
            p.area_sf = net / (upf * upf);
            p = p
                .with("sheet_in", format!("{}x{}", num_text(sw), num_text(sl)), self.r.source("cladding.lath_sheet_in"))
                .with("lap_in", format!("{} side, {} end", num_text(side_lap), num_text(end_lap)), self.r.source("cladding.lath_lap_in"))
                .computed("cut", if !r.full || open > 0.0 { "true" } else { "false" })
                .computed("face", face);
            self.body(&mut p, &r, off, depth);
            batch.push(p);
        }
    }

    /// The barrier's courses, `plies` layers of them, from `inner` inwards (sheet units, `sign`
    /// towards the face): each course one piece along the wall, cut to the roll and lapped.
    fn barrier(&self, inner: f64, sign: f64, plies: usize, face: &str, inferred: Option<String>, batch: &mut Batch) {
        let (k, upf) = (self.k, self.upf);
        let roll = self.r.nums("cladding.barrier_roll_ft");
        let lap = self.r.nums("cladding.barrier_lap_in");
        let (rw, rl) = (roll.first().copied().unwrap_or(9.0), roll.get(1).copied().unwrap_or(150.0));
        let (hl, vl) = (lap.first().copied().unwrap_or(2.0).min(rw * 6.0), lap.get(1).copied().unwrap_or(6.0).min(rl * 6.0));
        let pitch_z = (rw * 12.0 - hl) * k;
        let pitch_s = (rl * 12.0 - vl) * k;
        let item = self.r.text("cladding.barrier_item");
        let src = self.r.source("cladding.barrier_item");
        let t = 0.04 * k;
        for ply in 1..=plies.clamp(1, 4) {
            let off = inner - sign * t * (ply as f64 - 0.5);
            for r in self.grid(pitch_s, pitch_z) {
                let Some((net, _)) = self.net(&r) else { continue };
                let mut p = self.piece("cladding", "weather_barrier", item.clone(), src.clone(), inferred.as_deref());
                p.ty = format!("{plies} layer{}", if plies > 1 { "s" } else { "" });
                p.length_ft = (r.b - r.a) / upf;
                p.area_sf = net / (upf * upf);
                p = p
                    .with("roll_ft", format!("{}x{}", num_text(rw), num_text(rl)), self.r.source("cladding.barrier_roll_ft"))
                    .with("lap_in", format!("{} horizontal, {} vertical", num_text(hl), num_text(vl)), self.r.source("cladding.barrier_lap_in"))
                    .computed("ply", ply.to_string())
                    .computed("face", face);
                self.body(&mut p, &r, off, t);
                batch.push(p);
            }
        }
    }

    /// The screed along the finish's base, stopping at the doors.
    fn screed(&self, pl: &Placed, face: &str, batch: &mut Batch) {
        let (w, k, upf) = (self.w, self.k, self.upf);
        let doors = geo::merge(w.holes.iter().filter(|h| h.z0 <= w.zf + 1e-6).map(|h| (h.s0, h.s1)).collect());
        let mut runs: Vec<(f64, f64)> = Vec::new();
        let mut at = w.s0;
        for (a, b) in doors {
            if a > at {
                runs.push((at, a));
            }
            at = at.max(b);
        }
        if w.s1 > at {
            runs.push((at, w.s1));
        }
        let why = self.r.source_text("cladding.screed_item");
        let at_text = if self.ground { "the foundation plate line" } else { "the floor line" };
        for (a, b) in runs.into_iter().take(64) {
            let mut p = self.piece("cladding", "screed", self.r.text("cladding.screed_item"), self.r.source("cladding.screed_item"), Some(&why));
            p.length_ft = (b - a) / upf;
            p = p.computed("at", at_text).computed("face", face);
            p.blocks.push(w.block(a, b, pl.off, pl.depth, w.zf, w.zf + 1.5 * k));
            batch.push(p);
        }
    }

    /// Casing bead round each opening (not across a door's sill or a full-height opening's head)
    /// and at the finish's free ends.
    fn casing(&self, pl: &Placed, face: &str, batch: &mut Batch) {
        let (w, k, upf) = (self.w, self.k, self.upf);
        let why = self.r.source_text("cladding.casing_bead_item");
        let bw = 1.0 * k;
        for h in w.holes.iter().take(MAX_PER_FACE) {
            let (a, b) = (h.s0.max(w.s0), h.s1.min(w.s1));
            let (c, e) = (h.z0.max(w.zf), h.z1.min(w.zt));
            if b - a <= 1e-6 || e - c <= 1e-6 {
                continue;
            }
            let head = e < w.zt - 1e-6;
            let sill = c > w.zf + 1e-6;
            let len = 2.0 * (e - c) + if head { b - a } else { 0.0 } + if sill { b - a } else { 0.0 };
            let mut p = self.piece(
                "cladding",
                "casing_bead",
                self.r.text("cladding.casing_bead_item"),
                self.r.source("cladding.casing_bead_item"),
                Some(&why),
            );
            p.length_ft = len / upf;
            p = p
                .computed("at", "opening")
                .computed("face", face)
                .computed("opening_ft", format!("{}x{}", num_text((b - a) / upf), num_text((e - c) / upf)));
            p.blocks.push(w.block(a - bw, a, pl.off, pl.depth, c, e));
            p.blocks.push(w.block(b, b + bw, pl.off, pl.depth, c, e));
            if head {
                p.blocks.push(w.block(a - bw, b + bw, pl.off, pl.depth, e, e + bw));
            }
            if sill {
                p.blocks.push(w.block(a - bw, b + bw, pl.off, pl.depth, c - bw, c));
            }
            batch.push(p);
        }
        for (end, j) in self.jn.iter().enumerate() {
            if j.corner || j.tee {
                continue;
            }
            let s = if end == 0 { w.s0 } else { w.s1 };
            let mut p = self.piece(
                "cladding",
                "casing_bead",
                self.r.text("cladding.casing_bead_item"),
                self.r.source("cladding.casing_bead_item"),
                Some(&why),
            );
            p.length_ft = (w.zt - w.zf) / upf;
            p = p.computed("at", "free end").computed("face", face);
            p.blocks.push(w.block(s - bw / 2.0, s + bw / 2.0, pl.off, pl.depth, w.zf, w.zt));
            batch.push(p);
        }
    }

    /// Corner aid at the corners this wall owns (the first of the corner's two walls).
    fn corner_aids(&self, pl: &Placed, face: &str, batch: &mut Batch) {
        let (w, k, upf) = (self.w, self.k, self.upf);
        let why = self.r.source_text("cladding.corner_aid_item");
        for (end, j) in self.jn.iter().enumerate() {
            if !(j.corner && j.bent && self.corner_mine.get(end).copied().unwrap_or(false)) {
                continue;
            }
            let s = if end == 0 { w.s0 } else { w.s1 };
            let mut p =
                self.piece("cladding", "corner_aid", self.r.text("cladding.corner_aid_item"), self.r.source("cladding.corner_aid_item"), Some(&why));
            p.length_ft = (w.zt - w.zf) / upf;
            p = p.computed("at", "outside corner").computed("face", face);
            let bk = 1.5 * k;
            p.blocks.push(w.block(s - bk / 2.0, s + bk / 2.0, pl.off, pl.depth, w.zf, w.zt));
            batch.push(p);
        }
    }
}
