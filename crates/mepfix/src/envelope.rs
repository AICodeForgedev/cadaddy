//! Roofing and the building envelope (Q337): the roof assembly (deck, vapor retarder,
//! insulation, tapered insulation, cover board, membrane), the roof edge (coping on a parapet or
//! edge metal), drains with their overflows, crickets behind the curbs and saddles between the
//! drains, pipe penetrations, walkway pads, and the exterior wall assemblies (cladding, air space
//! or furring, continuous insulation, air/water barrier, sheathing) by zone.
//!
//! Two sheets show it: `A-104 ROOF DRAINAGE AND FLASHING PLAN` (on top of the Q307 roof plan's
//! edge, drains and curbs) and `A-504 ROOF AND EXTERIOR WALL ASSEMBLIES` (assembly tables, a
//! typical wall section with its datums, and the detail index). A Division 07 spec text goes with
//! the set (`specs.txt`).
//!
//! The takeoff (SQ, SF, LF, EA, BF, with waste) and the coordination flags come from a
//! [`Model`]: the truth builds one from what it designed, the reader from what it read, and both
//! go through the same [`takeoff`] and [`flags`]. Like the walls and the rest of the
//! architecture, everything here comes from a random stream of its own.

use crate::draw::{Pen, table};
use crate::rng::Rng;
use crate::truth::*;
use cadaddy_doc::{HAlign, VAlign};
use cadaddy_geom::Vec2;

/// The roof and exterior wall assemblies sheet (A-501 is the wall types sheet, A-502 the ceiling
/// details, A-503 the wall sections).
pub const ASSEMBLIES_SHEET: &str = "A-504";

/// How the envelope sheets and the spec word and draw it.
#[derive(Clone, Debug, Default)]
pub struct EnvStyle {
    /// `tpo`, `pvc`, `epdm`, `sbs`.
    pub membrane: &'static str,
    pub mil: Option<f64>,
    pub attachment: &'static str,
    /// Cover board: thickness, inches, and words.
    pub cover: (f64, &'static str),
    /// Flat insulation boards: how many and how thick, inches.
    pub boards: (u32, f64),
    /// Least tapered insulation (at the drains), inches.
    pub tapered_min_in: f64,
    pub vapor_retarder: bool,
    pub r_per_in: f64,
    pub min_r: f64,
    pub metal_ga: u32,
    /// Claddings: 0 one cladding all the way up, 1 brick on level 1 and another above.
    pub zones: u8,
    /// The cladding (above level 1 for zones 1): `brick`, `siding`, `eifs`, `panel`.
    pub cladding: &'static str,
    /// Continuous insulation, inches.
    pub ci_in: f64,
    /// Overflows through the parapet as scuppers (otherwise overflow drains).
    pub scuppers: bool,
    pub walkways: bool,
    /// Assembly table headers: 0 `COMPONENT / DESCRIPTION / THICKNESS`, 1 `LAYER / MATERIAL / THK.`.
    pub headers: u8,
    /// Wall section datums: 0 name over `EL. x`, 1 `NAME = x`.
    pub datum_words: u8,
    /// Spec wording variant.
    pub spec_words: u8,
    /// Planted gaps.
    pub plant_curb: Option<&'static str>,
    pub plant_overflow: bool,
    pub plant_mil: bool,
}

impl EnvStyle {
    pub fn new(r: &mut Rng) -> EnvStyle {
        let membrane = r.pick(&["tpo", "tpo", "pvc", "epdm", "sbs"]);
        let (mil, attachment) = match membrane {
            "tpo" | "pvc" => (Some(r.pick::<f64>(&[60.0, 80.0])), r.pick(&["fully adhered", "mechanically fastened", "induction welded"])),
            "epdm" => (Some(r.pick::<f64>(&[60.0, 90.0])), r.pick(&["fully adhered", "mechanically fastened"])),
            _ => (None, "torch applied"),
        };
        EnvStyle {
            membrane,
            mil,
            attachment,
            cover: r.pick(&[(0.5, "GYPSUM COVER BOARD"), (0.5, "HD POLYISO COVER BOARD"), (0.25, "GYPSUM COVER BOARD")]),
            boards: (r.pick(&[1, 2, 2]), r.pick::<f64>(&[1.5, 2.0, 2.5, 3.0])),
            tapered_min_in: r.pick::<f64>(&[0.5, 0.5, 1.0]),
            vapor_retarder: r.chance(0.5),
            r_per_in: r.pick::<f64>(&[5.7, 5.6]),
            min_r: r.pick::<f64>(&[25.0, 30.0, 30.0, 40.0]),
            metal_ga: r.pick(&[24, 24, 22]),
            zones: r.pick(&[0, 1, 1]),
            cladding: r.pick(&["brick", "siding", "eifs", "panel"]),
            ci_in: r.pick::<f64>(&[1.5, 2.0, 3.0]),
            scuppers: r.chance(0.5),
            walkways: r.chance(0.8),
            headers: r.pick(&[0, 1]),
            datum_words: r.pick(&[0, 1]),
            spec_words: r.pick(&[0, 1]),
            plant_curb: if r.chance(0.35) { Some(r.pick(&["no_curb_detail", "missing_detail"])) } else { None },
            plant_overflow: r.chance(0.3),
            plant_mil: r.chance(0.3),
        }
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn r6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

fn half(v: f64) -> f64 {
    (v * 2.0).round() / 2.0
}

/// Inches as the assembly tables print them, to the sixteenth: `5/16"`, `1 1/2"`, `3"`.
pub fn frac16(v: f64) -> String {
    let n = (v * 16.0).round() as i64;
    let (whole, mut num, mut den) = (n / 16, n % 16, 16);
    while num > 0 && num % 2 == 0 {
        num /= 2;
        den /= 2;
    }
    match (whole, num) {
        (0, 0) => "0\"".into(),
        (0, _) => format!("{num}/{den}\""),
        (w, 0) => format!("{w}\""),
        (w, _) => format!("{w} {num}/{den}\""),
    }
}

fn layer(component: &str, description: String, thickness_in: f64) -> EnvLayer {
    EnvLayer { component: component.into(), description, thickness_in, count: 1 }
}

/// A cladding's layers, outside to inside, ending with the backup wall's sheathing.
fn cladding_layers(kind: &str, ci_in: f64, sheathing_in: f64) -> Vec<EnvLayer> {
    let f = frac16;
    let mut v = match kind {
        "brick" => vec![
            layer("cladding", "BRICK VENEER W/ ADJUSTABLE TIES @ 16\" O.C.".into(), 3.625),
            layer("air_space", "AIR SPACE".into(), 2.0),
            layer("ci", format!("{} MINERAL WOOL BOARD INSULATION", f(ci_in)), ci_in),
        ],
        "siding" => vec![
            layer("cladding", "FIBER CEMENT LAP SIDING".into(), 0.3125),
            layer("furring", "3/4\" PT WOOD FURRING @ 16\" O.C. (RAINSCREEN)".into(), 0.75),
            layer("ci", format!("{} MINERAL WOOL BOARD INSULATION", f(ci_in)), ci_in),
        ],
        "eifs" => vec![
            layer("cladding", "EIFS FINISH AND BASE COAT W/ MESH".into(), 0.25),
            layer("ci", format!("{} EPS INSULATION BOARD", f(ci_in)), ci_in),
        ],
        _ => vec![
            layer("cladding", "ALUMINUM COMPOSITE METAL PANEL, RAINSCREEN".into(), 0.1875),
            layer("furring", "1 1/2\" HAT CHANNEL GIRTS".into(), 1.5),
            layer("ci", format!("{} MINERAL WOOL BOARD INSULATION", f(ci_in)), ci_in),
        ],
    };
    v.push(layer("barrier", "FLUID-APPLIED AIR AND WATER BARRIER".into(), 0.0));
    v.push(layer("sheathing", format!("{} GLASS-MAT GYPSUM SHEATHING", f(sheathing_in)), sheathing_in));
    v
}

fn membrane_words(kind: &str, mil: Option<f64>, attachment: &str) -> String {
    let m = match kind {
        "tpo" => "TPO MEMBRANE",
        "pvc" => "PVC MEMBRANE",
        "epdm" => "EPDM MEMBRANE",
        _ => "2-PLY SBS MODIFIED BITUMEN, GRANULE CAP SHEET",
    };
    match mil {
        Some(t) => format!("{t:.0} MIL {m}, {}", attachment.to_ascii_uppercase()),
        None => format!("{m}, {}", attachment.to_ascii_uppercase()),
    }
}

/// Design the envelope from the truth's roof, walls, levels and plumbing vent.
pub fn design_envelope(t: &mut Truth, st: &EnvStyle) {
    let Some(arch) = t.arch.as_ref() else { return };
    let rf = arch.roof.clone();
    let elev = |c: &str| t.building.levels.iter().find(|l| l.code == c).map(|l| l.elevation_ft);
    let deck_in = t.building.slabs.iter().find(|s| s.level == "R").map_or(1.5, |s| s.deck_in);
    let sheathing_in =
        t.wall_types.iter().find(|w| w.external).and_then(|w| w.layers.iter().find(|l| l.material == "sheathing")).map_or(0.625, |l| l.thickness_in);
    let mut e = Envelope { edge: if rf.parapet_in.is_some() { "coping".into() } else { "edge_metal".into() }, ..Envelope::default() };

    // ---- The roof assembly, top down ----
    let mut layers = vec![
        layer("membrane", membrane_words(st.membrane, st.mil, st.attachment), 0.0),
        layer("cover_board", format!("{} {}", crate::walls::frac(st.cover.0, false), st.cover.1), st.cover.0),
        layer("tapered", format!("TAPERED POLYISO, {}/FT", crate::walls::frac(rf.slope_in_ft, false)), st.tapered_min_in),
        EnvLayer {
            component: "insulation".into(),
            description: format!("{} POLYISO BOARD INSULATION", crate::walls::frac(st.boards.1, false)),
            thickness_in: st.boards.1,
            count: st.boards.0,
        },
    ];
    if st.vapor_retarder {
        layers.push(layer("vapor_retarder", "SELF-ADHERED VAPOR RETARDER".into(), 0.0));
    }
    layers.push(layer("deck", format!("{} TYPE B STEEL ROOF DECK", crate::walls::frac(deck_in, false)), deck_in));
    e.roof = RoofAssembly { tag: "R-1".into(), membrane: st.membrane.into(), mil: st.mil, attachment: st.attachment.into(), layers };

    // ---- Wall section datums and claddings ----
    for c in ["1", "2", "R"] {
        if let Some(v) = elev(c) {
            e.datums.push(Datum { code: c.into(), elevation_ft: r6(v) });
        }
    }
    let roof_el = elev("R").unwrap_or(27.0);
    e.datums.push(Datum { code: "P".into(), elevation_ft: r6(roof_el + rf.parapet_in.map_or(1.0, |p| p / 12.0)) });
    let floors: Vec<String> = t.building.levels.iter().filter(|l| l.code != "R").map(|l| l.code.clone()).collect();
    if st.zones == 1 && st.cladding != "brick" && floors.len() > 1 {
        e.claddings.push(Cladding {
            tag: "EW-1".into(),
            kind: "brick".into(),
            levels: floors.iter().take(1).cloned().collect(),
            to_top: false,
            layers: cladding_layers("brick", st.ci_in, sheathing_in),
        });
        e.claddings.push(Cladding {
            tag: "EW-2".into(),
            kind: st.cladding.into(),
            levels: floors.iter().skip(1).cloned().collect(),
            to_top: true,
            layers: cladding_layers(st.cladding, st.ci_in, sheathing_in),
        });
    } else {
        e.claddings.push(Cladding {
            tag: "EW-1".into(),
            kind: st.cladding.into(),
            levels: floors.clone(),
            to_top: true,
            layers: cladding_layers(st.cladding, st.ci_in, sheathing_in),
        });
    }

    // ---- Drainage: overflows, saddles, crickets ----
    let y_mid = rf.drains.first().map_or((rf.min_ft[1] + rf.max_ft[1]) / 2.0, |d| d[1]);
    let skip = if st.plant_overflow && rf.parapet_in.is_some() && !rf.drains.is_empty() { Some(rf.drains.len() - 1) } else { None };
    if skip.is_some() {
        e.planted.push("no_overflow".into());
    }
    if rf.parapet_in.is_some() {
        for (i, d) in rf.drains.iter().enumerate() {
            if Some(i) == skip {
                continue;
            }
            if st.scuppers {
                // Through the nearer long parapet.
                let y = if d[1] - rf.min_ft[1] < rf.max_ft[1] - d[1] { rf.min_ft[1] } else { rf.max_ft[1] };
                e.overflows.push(Overflow { kind: "scupper".into(), at_ft: [r6(d[0]), r6(y)] });
            } else {
                e.overflows.push(Overflow { kind: "drain".into(), at_ft: [r6(d[0] + 2.0), r6(d[1])] });
            }
        }
    }
    let mut drains = rf.drains.clone();
    drains.sort_by(|a, b| a[0].total_cmp(&b[0]));
    for w in drains.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        let gap = b[0] - a[0];
        let (hx, hy) = (gap / 4.0, (gap / 8.0).min(4.0));
        let c = [(a[0] + b[0]) / 2.0, y_mid];
        e.ridges.push(RoofRidge {
            kind: "saddle".into(),
            at: String::new(),
            outline: vec![[r6(c[0] - hx), r6(c[1])], [r6(c[0]), r6(c[1] - hy)], [r6(c[0] + hx), r6(c[1])], [r6(c[0]), r6(c[1] + hy)]],
            height_in: half(hx * rf.slope_in_ft).max(0.5),
        });
    }
    for c in &rf.curbs {
        let wx = c.max_ft[0] - c.min_ft[0];
        if wx <= 2.0 {
            continue;
        }
        let up = if (c.min_ft[1] + c.max_ft[1]) / 2.0 > y_mid { 1.0 } else { -1.0 };
        let edge_y = if up > 0.0 { c.max_ft[1] } else { c.min_ft[1] };
        let room = if up > 0.0 { rf.max_ft[1] - edge_y } else { edge_y - rf.min_ft[1] };
        let len = (wx / 2.0).min(room - 0.5);
        if len < 0.5 {
            continue;
        }
        let mid = (c.min_ft[0] + c.max_ft[0]) / 2.0;
        e.ridges.push(RoofRidge {
            kind: "cricket".into(),
            at: c.tag.clone(),
            outline: vec![[r6(c.min_ft[0]), r6(edge_y)], [r6(c.max_ft[0]), r6(edge_y)], [r6(mid), r6(edge_y + up * len)]],
            height_in: half(len * rf.slope_in_ft * 2.0).max(0.5),
        });
    }

    // ---- Penetrations ----
    for n in t.nodes.iter().filter(|n| n.kind == "vtr") {
        let p = [n.at_ft[0], n.at_ft[1]];
        if p[0] > rf.min_ft[0] && p[0] < rf.max_ft[0] && p[1] > rf.min_ft[1] && p[1] < rf.max_ft[1] {
            let size = t.segments.iter().find(|s| s.to == n.id || s.from == n.id).map_or(3.0, |s| s.size_in[0]);
            e.pipes.push(RoofPipe { tag: "VTR".into(), at_ft: [r6(p[0]), r6(p[1])], size_in: size });
        }
    }

    // ---- Walkway pads: roof hatch to each equipment curb ----
    if st.walkways
        && let Some(h) = rf.curbs.iter().find(|c| c.kind == "hatch")
    {
        let hc = [(h.min_ft[0] + h.max_ft[0]) / 2.0, (h.min_ft[1] + h.max_ft[1]) / 2.0];
        for q in rf.curbs.iter().filter(|c| c.kind == "equipment") {
            let qc = [(q.min_ft[0] + q.max_ft[0]) / 2.0, (q.min_ft[1] + q.max_ft[1]) / 2.0];
            let w = 1.25;
            // Along x from the hatch's side to over the unit, then along y to its curb.
            let (x0, x1) = if qc[0] > hc[0] { (h.max_ft[0], qc[0] + w) } else { (qc[0] - w, h.min_ft[0]) };
            if x1 - x0 >= 1.0 {
                e.walkways.push(Walkway { min_ft: [r6(x0), r6(hc[1] - w)], max_ft: [r6(x1), r6(hc[1] + w)] });
            }
            let (y0, y1) = if qc[1] > hc[1] { (hc[1] + w, q.min_ft[1]) } else { (q.max_ft[1], hc[1] - w) };
            if y1 - y0 >= 1.0 {
                e.walkways.push(Walkway { min_ft: [r6(qc[0] - w), r6(y0)], max_ft: [r6(qc[0] + w), r6(y1)] });
            }
        }
    }

    // ---- Details and their callouts ----
    let mut n = 0;
    let mut detail = |title: String, subject: &str, details: &mut Vec<Detail>| {
        n += 1;
        details.push(Detail { number: n, title, subject: subject.into() });
        n
    };
    let mut details = Vec::new();
    let edge_title = if rf.parapet_in.is_some() { "PARAPET AND COPING" } else { "ROOF EDGE AND FASCIA" };
    let d_edge = detail(edge_title.into(), "edge", &mut details);
    let d_drain = detail(if e.overflows.is_empty() { "ROOF DRAIN".into() } else { "ROOF DRAIN AND OVERFLOW".into() }, "drain", &mut details);
    let mut curb_details = Vec::new();
    for c in &rf.curbs {
        let title = match c.kind.as_str() {
            "equipment" => format!("EQUIPMENT CURB - {}", c.tag),
            "hatch" => "ROOF HATCH CURB".into(),
            _ => "ELEVATOR OVERRUN BASE FLASHING".into(),
        };
        curb_details.push((detail(title, &c.tag, &mut details), c.clone()));
    }
    let d_pipe = (!e.pipes.is_empty()).then(|| detail("PIPE PENETRATION".into(), "pipe", &mut details));
    if e.ridges.iter().any(|r| r.kind == "cricket") {
        detail("TAPERED CRICKET".into(), "cricket", &mut details);
    }
    detail("WINDOW HEAD, JAMB AND SILL".into(), "window", &mut details);
    if arch.window_types.iter().any(|w| w.storefront) {
        detail("STOREFRONT SILL".into(), "storefront", &mut details);
    }
    let sheet = ASSEMBLIES_SHEET.to_string();
    let call =
        |number: u32, at: [f64; 2], subject: &str| Callout { number, sheet: sheet.clone(), at_ft: [r6(at[0]), r6(at[1])], subject: subject.into() };
    e.callouts.push(call(d_edge, [rf.min_ft[0] + 8.0, rf.max_ft[1] + 3.0], "edge"));
    if let Some(d) = rf.drains.first() {
        e.callouts.push(call(d_drain, [d[0] - 3.0, d[1] - 3.0], "drain"));
    }
    if let (Some(n), Some(p)) = (d_pipe, e.pipes.first()) {
        e.callouts.push(call(n, [p.at_ft[0] + 3.0, p.at_ft[1] - 3.5], "pipe"));
    }
    let planted_tag = st.plant_curb.and_then(|_| rf.curbs.iter().find(|c| c.kind == "equipment").map(|c| c.tag.clone()));
    for (num, c) in &curb_details {
        let at = [c.max_ft[0] + 2.2, c.max_ft[1] + 2.2];
        if planted_tag.as_deref() == Some(c.tag.as_str()) {
            if st.plant_curb == Some("missing_detail") {
                e.callouts.push(call(*num, at, &c.tag));
            }
            details.retain(|d| d.number != *num);
            continue;
        }
        e.callouts.push(call(*num, at, &c.tag));
    }
    if let (Some(p), Some(_)) = (st.plant_curb, &planted_tag) {
        e.planted.push(p.into());
    }
    e.details = details;

    // ---- The spec ----
    let spec_mil = match st.mil {
        Some(m) if st.plant_mil => {
            e.planted.push("spec_mil".into());
            Some(if m >= 80.0 {
                60.0
            } else if st.membrane == "epdm" {
                90.0
            } else {
                80.0
            })
        }
        m => m,
    };
    e.spec = EnvSpec { membrane_mil: spec_mil, min_r: st.min_r, r_per_in: st.r_per_in, metal_ga: st.metal_ga };

    t.envelope = Some(e);
    let m = model_from_truth(t);
    if let Some(e) = t.envelope.as_mut() {
        e.takeoff = takeoff(&m);
        e.flags = flags(&m);
    }
}

// ---- The model the takeoff and flags work from -------------------------------------------------

/// An opening in the exterior walls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnvOpening {
    /// `window`, `storefront`, `door`.
    pub kind: String,
    pub level: String,
    pub width_in: f64,
    pub height_in: f64,
}

/// What the takeoff and the flags need, from the truth or from the prints.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub roof_min: [f64; 2],
    pub roof_max: [f64; 2],
    pub parapet_in: Option<f64>,
    pub slope_in_ft: f64,
    pub drains: Vec<[f64; 2]>,
    pub curbs: Vec<Curb>,
    pub overflows: Vec<Overflow>,
    pub pipes: Vec<RoofPipe>,
    pub ridges: Vec<RoofRidge>,
    pub walkways: Vec<Walkway>,
    pub edge: String,
    pub details: Vec<Detail>,
    pub callouts: Vec<Callout>,
    pub roof: RoofAssembly,
    pub claddings: Vec<Cladding>,
    pub datums: Vec<Datum>,
    /// The exterior walls' outer face, building feet.
    pub outline: Option<([f64; 2], [f64; 2])>,
    pub openings: Vec<EnvOpening>,
    /// Roof beams: ends and grid line.
    pub beams: Vec<([f64; 2], [f64; 2], String)>,
    pub spec: Option<EnvSpec>,
}

/// The model of what the truth designed.
pub fn model_from_truth(t: &Truth) -> Model {
    let (Some(arch), Some(e)) = (t.arch.as_ref(), t.envelope.as_ref()) else { return Model::default() };
    let rf = &arch.roof;
    // The outer face: each level 1 wall out by half its type's thickness across it (the
    // exterior walls are the outermost; their ends already reach the corners).
    let mut lo = [f64::MAX; 2];
    let mut hi = [f64::MIN; 2];
    for w in t.plan_walls.iter().filter(|w| w.level == "1") {
        let h = t.wall_types.iter().find(|x| x.tag == w.wall_type).map_or(0.0, |x| x.thickness_in) / 24.0;
        let along_x = (w.a_ft[1] - w.b_ft[1]).abs() < (w.a_ft[0] - w.b_ft[0]).abs();
        let (hx, hy) = if along_x { (0.0, h) } else { (h, 0.0) };
        for p in [w.a_ft, w.b_ft] {
            lo = [lo[0].min(p[0] - hx), lo[1].min(p[1] - hy)];
            hi = [hi[0].max(p[0] + hx), hi[1].max(p[1] + hy)];
        }
    }
    let outline = (lo[0] < hi[0] && lo[1] < hi[1]).then_some((lo, hi));
    let mut openings = Vec::new();
    for w in &arch.windows {
        if let Some(wt) = arch.window_types.iter().find(|x| x.mark == w.mark) {
            let kind = if wt.storefront { "storefront" } else { "window" };
            openings.push(EnvOpening { kind: kind.into(), level: w.level.clone(), width_in: wt.width_in, height_in: wt.height_in });
        }
    }
    for d in arch.doors.iter().filter(|d| d.from_room.is_none() || d.swings_into.is_none()) {
        openings.push(EnvOpening { kind: "door".into(), level: d.level.clone(), width_in: d.width_in, height_in: d.height_in });
    }
    Model {
        roof_min: rf.min_ft,
        roof_max: rf.max_ft,
        parapet_in: rf.parapet_in,
        slope_in_ft: rf.slope_in_ft,
        drains: rf.drains.clone(),
        curbs: rf.curbs.clone(),
        overflows: e.overflows.clone(),
        pipes: e.pipes.clone(),
        ridges: e.ridges.clone(),
        walkways: e.walkways.clone(),
        edge: e.edge.clone(),
        details: e.details.clone(),
        callouts: e.callouts.clone(),
        roof: e.roof.clone(),
        claddings: e.claddings.clone(),
        datums: e.datums.clone(),
        outline,
        openings,
        beams: t.building.beams.iter().filter(|b| b.level == "R").map(|b| (b.from_ft, b.to_ft, b.grid.clone())).collect(),
        spec: Some(e.spec.clone()),
    }
}

// ---- Takeoff -----------------------------------------------------------------------------------

/// Waste allowance, percent, by takeoff item (the usual estimating allowances).
pub fn waste_pct(code: &str) -> f64 {
    let last = code.rsplit('.').next().unwrap_or(code);
    match last {
        "membrane" | "edge" | "coping" | "edge_metal" | "base_flashing" | "sheathing" | "barrier" | "siding" | "panel" | "furring" | "sealant" => {
            10.0
        }
        "cover_board" | "insulation" | "tapered" | "ridges" | "brick" | "eifs" | "ci" | "walkway" | "air_space" | "head_flashing" | "sill_pan" => 5.0,
        _ => 0.0,
    }
}

fn item(code: String, description: String, unit: &str, net: f64) -> QtyItem {
    // A hostile model (infinite or not-a-number sizes) gives nothing rather than nonsense.
    let net = if net.is_finite() { r2(net.max(0.0)) } else { 0.0 };
    let waste_pct = waste_pct(&code);
    let order = (net * (1.0 + waste_pct / 100.0) - 1e-6).ceil().max(0.0);
    QtyItem { code, description, unit: unit.into(), net, waste_pct, order }
}

fn polygon_area(pts: &[[f64; 2]]) -> f64 {
    let n = pts.len();
    let mut a = 0.0;
    for i in 0..n {
        if let (Some(p), Some(q)) = (pts.get(i), pts.get((i + 1) % n)) {
            a += p[0] * q[1] - q[0] * p[1];
        }
    }
    (a / 2.0).abs()
}

/// The tapered insulation's average and greatest thickness, inches: its least at the drains,
/// rising at the roof's slope with the distance to the nearest drain (across, then along the
/// valley).
pub fn tapered_thickness(m: &Model, min_in: f64) -> (f64, f64) {
    let (w, h) = (m.roof_max[0] - m.roof_min[0], m.roof_max[1] - m.roof_min[1]);
    let drains: Vec<[f64; 2]> = m.drains.iter().filter(|d| d[0].is_finite() && d[1].is_finite()).take(64).copied().collect();
    let usable = !drains.is_empty() && min_in.is_finite() && m.slope_in_ft.is_finite() && w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0;
    if !usable {
        return (min_in, min_in);
    }
    // At most about 40 000 sample cells, however big the roof.
    let step = ((w * h) / 40_000.0).sqrt().max(1.0);
    let (nx, ny) = (((w / step).ceil() as usize).clamp(1, 2000), ((h / step).ceil() as usize).clamp(1, 2000));
    let (sx, sy) = (w / nx as f64, h / ny as f64);
    let (mut sum, mut max) = (0.0, min_in);
    for i in 0..nx {
        for j in 0..ny {
            let p = [m.roof_min[0] + (i as f64 + 0.5) * sx, m.roof_min[1] + (j as f64 + 0.5) * sy];
            let d = drains.iter().map(|d| (p[0] - d[0]).abs() + (p[1] - d[1]).abs()).fold(f64::MAX, f64::min);
            let t = min_in + m.slope_in_ft * d;
            sum += t;
            max = max.max(t);
        }
    }
    (sum / (nx * ny) as f64, max)
}

fn datum(m: &Model, code: &str) -> Option<f64> {
    m.datums.iter().find(|d| d.code == code).map(|d| d.elevation_ft)
}

/// The cladding zone's height, feet: its levels' storeys, and up to the top datum if it goes on
/// up the parapet.
pub fn zone_height(m: &Model, c: &Cladding) -> f64 {
    let mut order: Vec<&Datum> = m.datums.iter().filter(|d| d.code != "P").collect();
    order.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    let mut h = 0.0;
    for l in &c.levels {
        let Some(i) = order.iter().position(|d| &d.code == l) else { continue };
        if let (Some(a), Some(b)) = (order.get(i), order.get(i + 1)) {
            h += b.elevation_ft - a.elevation_ft;
        }
    }
    if c.to_top
        && let (Some(r), Some(p)) = (datum(m, "R"), datum(m, "P"))
    {
        h += (p - r).max(0.0);
    }
    h
}

/// Every takeoff line, from the model.
pub fn takeoff(m: &Model) -> Vec<QtyItem> {
    let mut out = Vec::new();
    let (w, h) = (m.roof_max[0] - m.roof_min[0], m.roof_max[1] - m.roof_min[1]);
    let (area, perim) = if w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite() { (w * h, 2.0 * (w + h)) } else { (0.0, 0.0) };
    let parapet_ft = m.parapet_in.map_or(0.0, |p| p / 12.0);
    let curb_perim = |c: &Curb| 2.0 * ((c.max_ft[0] - c.min_ft[0]) + (c.max_ft[1] - c.min_ft[1]));
    let curb_flash: f64 = m.curbs.iter().map(|c| curb_perim(c) * c.height_in.min(24.0) / 12.0).sum();
    let desc = |comp: &str| m.roof.layers.iter().find(|l| l.component == comp).map(|l| l.description.clone()).unwrap_or_default();

    // ---- Roof ----
    if area > 0.0 {
        let flash_up = perim * parapet_ft + curb_flash;
        out.push(item(
            "roof.membrane".into(),
            format!("{} (field {area:.0} SF + flashings {flash_up:.0} SF)", desc("membrane")),
            "SQ",
            (area + flash_up) / 100.0,
        ));
        if m.roof.layers.iter().any(|l| l.component == "cover_board") {
            out.push(item("roof.cover_board".into(), desc("cover_board"), "SF", area));
        }
        for l in m.roof.layers.iter().filter(|l| l.component == "insulation") {
            out.push(item("roof.insulation".into(), format!("({}) {}", l.count, l.description), "SF", area * l.count.max(1) as f64));
        }
        if let Some(l) = m.roof.layers.iter().find(|l| l.component == "tapered") {
            let (avg, max) = tapered_thickness(m, l.thickness_in);
            out.push(item(
                "roof.tapered".into(),
                format!("{}: {:.2}\" average, {:.1}\" at the high points", l.description, avg, max),
                "BF",
                area * avg / 12.0,
            ));
        }
        if m.roof.layers.iter().any(|l| l.component == "vapor_retarder") {
            out.push(item("roof.vapor_retarder".into(), desc("vapor_retarder"), "SF", area));
        }
        if m.roof.layers.iter().any(|l| l.component == "deck") {
            out.push(item("roof.deck".into(), desc("deck"), "SF", area));
        }
        for kind in ["cricket", "saddle"] {
            let rs: Vec<&RoofRidge> = m.ridges.iter().filter(|r| r.kind == kind).collect();
            if !rs.is_empty() {
                out.push(item(format!("roof.{kind}s"), format!("Tapered {kind}s"), "EA", rs.len() as f64));
                out.push(item(
                    format!("roof.{kind}s.ridges"),
                    format!("Tapered {kind}s, plan area"),
                    "SF",
                    rs.iter().map(|r| polygon_area(&r.outline)).sum(),
                ));
            }
        }
        if m.edge == "coping" {
            out.push(item("roof.coping".into(), "Prefinished metal coping".into(), "LF", perim));
            out.push(item("roof.parapet.base_flashing".into(), "Membrane base flashing at the parapet".into(), "LF", perim));
        } else {
            out.push(item("roof.edge_metal".into(), "Prefinished metal roof edge and fascia".into(), "LF", perim));
        }
        if !m.curbs.is_empty() {
            out.push(item("roof.curbs.base_flashing".into(), "Membrane base flashing at curbs".into(), "LF", m.curbs.iter().map(curb_perim).sum()));
        }
        let wl: f64 = m.walkways.iter().map(|w| (w.max_ft[0] - w.min_ft[0]).max(w.max_ft[1] - w.min_ft[1])).sum();
        if wl > 0.0 {
            out.push(item("roof.walkway".into(), "Walkway pads 30\" x 30\"".into(), "EA", (wl * 12.0 / 30.0).ceil()));
        }
    }
    // ---- Penetrations and flashings ----
    out.push(item("pen.drain".into(), "Roof drains with clamping ring".into(), "EA", m.drains.len() as f64));
    for (kind, words) in [("scupper", "Overflow scuppers"), ("drain", "Overflow drains")] {
        let n = m.overflows.iter().filter(|o| o.kind == kind).count();
        if n > 0 {
            out.push(item(format!("pen.overflow_{kind}"), words.into(), "EA", n as f64));
        }
    }
    for (kind, words) in [("equipment", "Equipment curb flashings"), ("hatch", "Roof hatch curb flashings"), ("overrun", "Overrun wall flashings")] {
        let n = m.curbs.iter().filter(|c| c.kind == kind).count();
        if n > 0 {
            out.push(item(format!("pen.curb_{kind}"), words.into(), "EA", n as f64));
        }
    }
    if !m.pipes.is_empty() {
        out.push(item("pen.pipe_boot".into(), "Pipe boot flashings".into(), "EA", m.pipes.len() as f64));
    }
    let pens = m.drains.len() + m.overflows.len() + m.curbs.len() + m.pipes.len();
    out.push(item("pen.total".into(), "Roof penetrations to flash".into(), "EA", pens as f64));

    // ---- Walls ----
    if let Some((lo, hi)) = m.outline {
        let wall_perim = 2.0 * ((hi[0] - lo[0]) + (hi[1] - lo[1]));
        let mut seal = 0.0;
        for c in &m.claddings {
            let gross = wall_perim * zone_height(m, c);
            let open: Vec<&EnvOpening> = m.openings.iter().filter(|o| c.levels.contains(&o.level)).collect();
            let open_sf: f64 = open.iter().map(|o| o.width_in * o.height_in / 144.0).sum();
            let net = (gross - open_sf).max(0.0);
            for l in &c.layers {
                let comp = if l.component == "cladding" { c.kind.as_str() } else { l.component.as_str() };
                let (unit, q) = if comp == "siding" { ("SQ", net / 100.0) } else { ("SF", net) };
                out.push(item(format!("wall.{}.{comp}", c.tag), format!("{}: {}", c.tag, l.description), unit, q));
            }
            if matches!(c.kind.as_str(), "brick" | "eifs") {
                // Vertical control joints every 25 ft.
                seal += (wall_perim / 25.0).ceil() * zone_height(m, c);
            }
            seal += open.iter().map(|o| 2.0 * (o.width_in + o.height_in) / 12.0).sum::<f64>();
            let heads: f64 = open.iter().map(|o| o.width_in / 12.0).sum();
            if heads > 0.0 {
                out.push(item(format!("wall.{}.head_flashing", c.tag), format!("{}: head flashings", c.tag), "LF", heads));
            }
            let sills: f64 = open.iter().filter(|o| o.kind != "door").map(|o| o.width_in / 12.0).sum();
            if sills > 0.0 {
                out.push(item(format!("wall.{}.sill_pan", c.tag), format!("{}: sill pans", c.tag), "LF", sills));
            }
        }
        if seal > 0.0 {
            out.push(item("wall.sealant".into(), "Joint sealant and backer rod at openings and control joints".into(), "LF", seal));
        }
    }
    for kind in ["window", "storefront"] {
        let os: Vec<&EnvOpening> = m.openings.iter().filter(|o| o.kind == kind).collect();
        if !os.is_empty() {
            out.push(item(format!("open.{kind}"), format!("{kind}s"), "EA", os.len() as f64));
            out.push(item(format!("open.{kind}.area"), format!("{kind}s, area"), "SF", os.iter().map(|o| o.width_in * o.height_in / 144.0).sum()));
        }
    }
    out
}

/// The roof insulation's average thermal resistance.
pub fn roof_r(m: &Model) -> Option<f64> {
    let r = m.spec.as_ref()?.r_per_in;
    let flat: f64 = m.roof.layers.iter().filter(|l| l.component == "insulation").map(|l| l.thickness_in * l.count.max(1) as f64).sum();
    let tap = m.roof.layers.iter().find(|l| l.component == "tapered").map_or(0.0, |l| tapered_thickness(m, l.thickness_in).0);
    Some((flat + tap) * r)
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 1e-12 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
}

/// A drain closer than this to a roof beam's centreline sits on it (sump and bowl vs flange), ft.
pub const DRAIN_BEAM_FT: f64 = 1.0;

/// The coordination flags.
pub fn flags(m: &Model) -> Vec<EnvFlag> {
    let mut out = Vec::new();
    let detail_numbers: Vec<u32> = m.details.iter().map(|d| d.number).collect();
    for c in m.curbs.iter().filter(|c| c.kind == "equipment") {
        match m.callouts.iter().find(|k| k.subject == c.tag) {
            None => out.push(EnvFlag {
                kind: "no_curb_detail".into(),
                subject: c.tag.clone(),
                message: format!("Rooftop equipment {} has no curb detail callout", c.tag),
            }),
            Some(k) if !detail_numbers.contains(&k.number) => out.push(EnvFlag {
                kind: "no_curb_detail".into(),
                subject: c.tag.clone(),
                message: format!("Rooftop equipment {} points to detail {}/{}, which is not on the sheet", c.tag, k.number, k.sheet),
            }),
            _ => {}
        }
    }
    for d in &m.drains {
        if let Some((_, _, g)) = m.beams.iter().find(|(a, b, _)| seg_dist(*d, *a, *b) < DRAIN_BEAM_FT) {
            out.push(EnvFlag {
                kind: "drain_structure".into(),
                subject: format!("RD@{:.1},{:.1}", d[0], d[1]),
                message: format!("Roof drain at ({:.1}, {:.1}) sits on the roof beam on grid {g}: move the drain or frame its sump", d[0], d[1]),
            });
        }
    }
    if m.parapet_in.is_some() {
        for d in &m.drains {
            if !m.overflows.iter().any(|o| (o.at_ft[0] - d[0]).abs() + (o.at_ft[1] - d[1]).abs() < 30.0 && (o.at_ft[0] - d[0]).abs() < 6.0) {
                out.push(EnvFlag {
                    kind: "no_overflow".into(),
                    subject: format!("RD@{:.1},{:.1}", d[0], d[1]),
                    message: format!("Roof drain at ({:.1}, {:.1}) inside a parapet has no overflow scupper or drain", d[0], d[1]),
                });
            }
        }
    }
    if let Some(s) = &m.spec {
        if let (Some(a), Some(b)) = (m.roof.mil, s.membrane_mil)
            && (a - b).abs() > 0.5
        {
            out.push(EnvFlag {
                kind: "spec_mil".into(),
                subject: m.roof.tag.clone(),
                message: format!("Roof {} membrane is {a:.0} mil on the drawings but {b:.0} mil in the spec", m.roof.tag),
            });
        }
        if let Some(r) = roof_r(m)
            && s.min_r > 0.0
            && r < s.min_r - 0.05
        {
            out.push(EnvFlag {
                kind: "r_value".into(),
                subject: m.roof.tag.clone(),
                message: format!("Roof {} insulation averages R-{r:.1}; the spec asks for R-{:.0}", m.roof.tag, s.min_r),
            });
        }
    }
    let y_mid = m.drains.first().map(|d| d[1]);
    for c in m.curbs.iter().filter(|c| c.max_ft[0] - c.min_ft[0] > 2.0) {
        let has = m.ridges.iter().any(|r| r.kind == "cricket" && r.at == c.tag);
        // Only where a cricket fits: there is roof upslope of the curb.
        let up_room =
            y_mid.map_or(0.0, |y| if (c.min_ft[1] + c.max_ft[1]) / 2.0 > y { m.roof_max[1] - c.max_ft[1] } else { c.min_ft[1] - m.roof_min[1] });
        if !has && up_room > 1.0 {
            out.push(EnvFlag {
                kind: "no_cricket".into(),
                subject: c.tag.clone(),
                message: format!("{} is wider than 24\" across the slope and has no cricket", c.tag),
            });
        }
    }
    out
}

// ---- Drawing -----------------------------------------------------------------------------------

/// The roof drainage and flashing plan: edge, drains, overflows, crickets and saddles, pipes,
/// walkway pads, the tapered insulation and edge notes, and the detail callouts.
pub fn draw_drainage(pen: &mut Pen, t: &Truth) {
    let (Some(arch), Some(e)) = (t.arch.as_ref(), t.envelope.as_ref()) else { return };
    let rot = crate::walls::readable(pen.rot());
    let rf = &arch.roof;
    let txt = |pen: &mut Pen, layer: &str, p: [f64; 2], h: f64, s: &str| {
        let at = pen.b(p);
        pen.text(layer, at, h, s, rot, HAlign::Center, VAlign::Middle);
    };
    pen.brect("A-ROOF", rf.min_ft, rf.max_ft);
    if rf.parapet_in.is_some() {
        pen.brect("A-ROOF", [rf.min_ft[0] + 0.67, rf.min_ft[1] + 0.67], [rf.max_ft[0] - 0.67, rf.max_ft[1] - 0.67]);
    }
    for d in &rf.drains {
        let c = pen.b(*d);
        let r = pen.bl(0.75);
        pen.circle("A-ROOF-DRAN", c, r);
        txt(pen, "A-ROOF-DRAN", [d[0] + 1.2, d[1] - 1.2], 0.06, "RD");
    }
    for c in &rf.curbs {
        pen.brect("A-ROOF-CURB", c.min_ft, c.max_ft);
    }
    for o in &e.overflows {
        if o.kind == "scupper" {
            pen.brect("A-ROOF-OVFL", [o.at_ft[0] - 0.5, o.at_ft[1] - 0.4], [o.at_ft[0] + 0.5, o.at_ft[1] + 0.4]);
            let off = if o.at_ft[1] > (rf.min_ft[1] + rf.max_ft[1]) / 2.0 { 1.6 } else { -1.6 };
            txt(pen, "A-ROOF-OVFL", [o.at_ft[0], o.at_ft[1] + off], 0.06, "OS");
        } else {
            let c = pen.b(o.at_ft);
            let r = pen.bl(0.6);
            pen.circle("A-ROOF-OVFL", c, r);
            txt(pen, "A-ROOF-OVFL", [o.at_ft[0] + 1.4, o.at_ft[1] + 1.0], 0.06, "OD");
        }
    }
    for r in &e.ridges {
        let pts: Vec<Vec2> = r.outline.iter().map(|p| pen.b(*p)).collect();
        pen.poly("A-ROOF-CRKT", &pts, true);
        let (label, at, at2) = if r.kind == "cricket" {
            let (base, apex) = (r.outline.first().copied().unwrap_or_default(), r.outline.get(2).copied().unwrap_or_default());
            let dir = if apex[1] > base[1] { 1.0 } else { -1.0 };
            ("CRICKET", [apex[0], apex[1] + dir * 1.2], [apex[0], apex[1] + dir * 2.4])
        } else {
            let top = r.outline.get(3).copied().unwrap_or_default();
            ("SADDLE", [top[0], top[1] + 1.2], [top[0], top[1] + 2.4])
        };
        txt(pen, "A-ROOF-CRKT", at, 0.06, label);
        txt(pen, "A-ROOF-CRKT", at2, 0.06, &format!("{} HIGH", crate::walls::frac(r.height_in, false)));
    }
    for p in &e.pipes {
        let c = pen.b(p.at_ft);
        let r = pen.bl((p.size_in / 24.0).max(0.25));
        pen.circle("A-ROOF-PENE", c, r);
        txt(pen, "A-ROOF-PENE", [p.at_ft[0] + 1.6, p.at_ft[1]], 0.06, &format!("{} {}", crate::walls::frac(p.size_in, false), p.tag));
    }
    for (i, w) in e.walkways.iter().enumerate() {
        pen.brect("A-ROOF-WALK", w.min_ft, w.max_ft);
        if i == 0 {
            txt(pen, "A-ROOF-WALK", [(w.min_ft[0] + w.max_ft[0]) / 2.0, w.max_ft[1] + 1.0], 0.06, "WALKWAY PADS");
        }
    }
    if let Some(l) = e.roof.layers.iter().find(|l| l.component == "tapered") {
        let s = format!(
            "TAPERED INSULATION SYSTEM: {}/FT TO DRAINS, {} MIN. AT DRAINS",
            crate::walls::frac(rf.slope_in_ft, false),
            crate::walls::frac(l.thickness_in, false)
        );
        let at = pen.b([rf.min_ft[0] + 1.0, rf.min_ft[1] - 3.0]);
        pen.text("A-ROOF", at, 0.08, &s, rot, HAlign::Left, VAlign::Middle);
    }
    let edge = if e.edge == "coping" {
        format!("PREFINISHED METAL COPING, {} GA.", e.spec.metal_ga)
    } else {
        format!("PREFINISHED METAL EDGE AND FASCIA, {} GA.", e.spec.metal_ga)
    };
    let at = pen.b([rf.min_ft[0] + 12.0, rf.max_ft[1] + 3.0]);
    pen.text("A-ROOF", at, 0.08, &edge, rot, HAlign::Left, VAlign::Middle);
    // Detail callouts: a bubble with the detail over the sheet, and a leader to what it details.
    for k in &e.callouts {
        let c = pen.b(k.at_ft);
        let r = pen.bl(1.6);
        pen.circle("A-ANNO-DETL", c, r);
        let right = Vec2::new(rot.cos(), rot.sin());
        pen.line("A-ANNO-DETL", c - right * r, c + right * r);
        pen.block("A-ANNO-DETL", c, 0.055, &[k.number.to_string(), k.sheet.clone()], rot);
        let target = leader_target(k, rf);
        let dir = [target[0] - k.at_ft[0], target[1] - k.at_ft[1]];
        let len = dir[0].hypot(dir[1]);
        if len > 1.7 {
            let from = [k.at_ft[0] + dir[0] / len * 1.6, k.at_ft[1] + dir[1] / len * 1.6];
            let (a, b) = (pen.b(from), pen.b(target));
            pen.line("A-ANNO-DETL", a, b);
        }
    }
}

/// Where a callout's leader ends: the curb's corner, the drain, the pipe, the roof edge.
pub fn leader_target(k: &Callout, rf: &Roof) -> [f64; 2] {
    match k.subject.as_str() {
        "edge" => [k.at_ft[0], rf.max_ft[1]],
        "drain" => rf.drains.first().copied().unwrap_or(k.at_ft),
        "pipe" => [k.at_ft[0] - 3.0, k.at_ft[1] + 3.5],
        tag => rf.curbs.iter().find(|c| c.tag == tag).map_or(k.at_ft, |c| c.max_ft),
    }
}

fn component_words(c: &str) -> &'static str {
    match c {
        "membrane" => "MEMBRANE",
        "cover_board" => "COVER BOARD",
        "tapered" => "TAPERED INSULATION",
        "insulation" => "INSULATION",
        "vapor_retarder" => "VAPOR RETARDER",
        "deck" => "ROOF DECK",
        "cladding" => "CLADDING",
        "air_space" => "AIR SPACE",
        "furring" => "FURRING",
        "ci" => "CONTINUOUS INSULATION",
        "barrier" => "AIR/WATER BARRIER",
        "sheathing" => "SHEATHING",
        _ => "OTHER",
    }
}

fn thickness_words(l: &EnvLayer, mil: Option<f64>) -> String {
    let f = frac16;
    match l.component.as_str() {
        "membrane" => mil.map_or_else(|| "--".to_string(), |m| format!("{m:.0} MIL")),
        "tapered" => format!("{} MIN.", f(l.thickness_in)),
        "insulation" if l.count > 1 => format!("({}) {}", l.count, f(l.thickness_in)),
        _ if l.thickness_in <= 0.0 => "--".into(),
        _ => f(l.thickness_in),
    }
}

/// Where a cladding zone goes, as its table's title says it.
pub fn zone_words(c: &Cladding, all: usize, parapet: bool) -> String {
    let top = if parapet { "TOP OF PARAPET" } else { "ROOF EDGE" };
    match (c.levels.first(), c.levels.len()) {
        (_, n) if n == all && c.to_top => "ALL LEVELS".into(),
        (Some(l), 1) if !c.to_top => format!("LEVEL {l}"),
        (Some(l), _) if c.to_top => format!("LEVEL {l} TO {top}"),
        (Some(l), _) => format!("LEVEL {l} AND ABOVE"),
        _ => String::new(),
    }
}

/// The assemblies sheet: the roof and wall assembly tables, the typical wall section and the
/// detail index.
pub fn draw_assemblies(pen: &mut Pen, t: &Truth, st: &EnvStyle, area: [[f64; 2]; 2]) {
    let (Some(arch), Some(e)) = (t.arch.as_ref(), t.envelope.as_ref()) else { return };
    let [[x0, y0], [x1, y1]] = area;
    let head: Vec<String> =
        if st.headers == 1 { ["LAYER", "MATERIAL", "THK."] } else { ["COMPONENT", "DESCRIPTION", "THICKNESS"] }.map(String::from).to_vec();
    let mut rows = vec![head.clone()];
    for l in &e.roof.layers {
        rows.push(vec![component_words(&l.component).into(), l.description.clone(), thickness_words(l, e.roof.mil)]);
    }
    let x = x0 + 0.3;
    let mut y = y1 - 0.6;
    let title = if st.headers == 1 { format!("ROOF TYPE {} (TOP DOWN)", e.roof.tag) } else { format!("ROOF ASSEMBLY {}", e.roof.tag) };
    y -= table(pen, [x, y], &title, &rows) + 0.4;
    let floors = t.building.levels.iter().filter(|l| l.code != "R").count();
    let backup = t.wall_types.iter().find(|w| w.external).map(|w| w.tag.clone()).unwrap_or_default();
    for c in &e.claddings {
        let mut rows = vec![head.clone()];
        for l in &c.layers {
            rows.push(vec![component_words(&l.component).into(), l.description.clone(), thickness_words(l, None)]);
        }
        rows.push(vec!["BACKUP".into(), format!("SEE WALL TYPE {backup}"), "--".into()]);
        let title = format!("EXTERIOR WALL ASSEMBLY {}: {}", c.tag, zone_words(c, floors, arch.roof.parapet_in.is_some()));
        y -= table(pen, [x, y], &title, &rows) + 0.4;
    }

    // The typical wall section: a wall line from level 1 to the top, with its datums.
    let top = e.datums.iter().map(|d| d.elevation_ft).fold(1.0, f64::max);
    let sx = x1 - 3.0;
    let (sy0, sy1) = (y0 + 4.2, y1 - 1.2);
    let k = (sy1 - sy0) / top;
    pen.ptext("TEXT", [sx - 2.0, y1 - 0.6], 0.13, "TYPICAL WALL SECTION", HAlign::Left);
    pen.pline("DETAIL", [sx, sy0], [sx, sy0 + top * k]);
    pen.pline("DETAIL", [sx + 0.12, sy0], [sx + 0.12, sy0 + top * k]);
    for d in &e.datums {
        let yy = sy0 + d.elevation_ft * k;
        pen.pline("A-ANNO-DATM", [sx - 0.6, yy], [sx + 0.8, yy]);
        let name = match d.code.as_str() {
            "R" => "ROOF".to_string(),
            "P" if arch.roof.parapet_in.is_some() => "TOP OF PARAPET".to_string(),
            "P" => "TOP OF ROOF EDGE".to_string(),
            c => format!("LEVEL {c}"),
        };
        if st.datum_words == 1 {
            pen.ptext("A-ANNO-DATM", [sx - 0.7, yy + 0.05], 0.08, &format!("{name} = {}", feet_inches(d.elevation_ft)), HAlign::Right);
        } else {
            pen.ptext("A-ANNO-DATM", [sx - 0.7, yy + 0.16], 0.08, &name, HAlign::Right);
            pen.ptext("A-ANNO-DATM", [sx - 0.7, yy + 0.03], 0.08, &format!("EL. {}", feet_inches(d.elevation_ft)), HAlign::Right);
        }
    }

    // The detail index along the bottom: a frame, a bubble with the number, the title.
    let (mut dx, dw) = (x0 + 0.3, 3.4);
    let mut dy = y0 + 0.4;
    for d in &e.details {
        if dx + dw > sx - 2.5 {
            dx = x0 + 0.3;
            dy += 1.9;
        }
        pen.prect("DETAIL", [dx, dy + 0.5], [dx + dw - 0.2, dy + 1.7]);
        // A stand-in sketch: the membrane turning up a curb or wall.
        pen.pline("DETAIL", [dx + 0.3, dy + 0.8], [dx + 1.6, dy + 0.8]);
        pen.pline("DETAIL", [dx + 1.6, dy + 0.8], [dx + 1.6, dy + 1.4]);
        pen.pline("DETAIL", [dx + 1.7, dy + 0.8], [dx + 1.7, dy + 1.5]);
        let c = pen.p([dx + 0.2, dy + 0.2]);
        let r = pen.pl(0.17);
        pen.circle("A-ANNO-DETL", c, r);
        pen.text("A-ANNO-DETL", c, 0.1, &d.number.to_string(), 0.0, HAlign::Center, VAlign::Middle);
        pen.ptext("TEXT", [dx + 0.5, dy + 0.22], 0.08, &d.title, HAlign::Left);
        pen.ptext("TEXT", [dx + 0.5, dy + 0.06], 0.06, "SCALE: 1 1/2\" = 1'-0\"", HAlign::Left);
        dx += dw;
    }
}

/// The Division 07 spec text that goes with the set.
pub fn spec_text(t: &Truth, st: &EnvStyle) -> String {
    let Some(e) = t.envelope.as_ref() else { return String::new() };
    let s = &e.spec;
    let mut out = String::new();
    let mut put = |l: String| {
        out.push_str(&l);
        out.push('\n');
    };
    put(format!("PROJECT MANUAL - {}", t.project.to_ascii_uppercase()));
    put("DIVISION 07 - THERMAL AND MOISTURE PROTECTION".into());
    put(String::new());
    put("SECTION 07 22 00 - ROOF AND DECK INSULATION".into());
    put("PART 2 - PRODUCTS".into());
    put("2.1 POLYISOCYANURATE BOARD INSULATION".into());
    if st.spec_words == 1 {
        put(format!("  A. LONG-TERM THERMAL RESISTANCE (LTTR): R-{:.1} PER INCH OF THICKNESS.", s.r_per_in));
        put(format!("  B. ROOF INSULATION, AVERAGE THERMAL RESISTANCE: NOT LESS THAN R-{:.0}.", s.min_r));
    } else {
        put(format!("  A. THERMAL RESISTANCE: R-{:.1} PER INCH (LTTR).", s.r_per_in));
        put(format!("  B. PROVIDE R-{:.0} MINIMUM AVERAGE FOR THE ROOF ASSEMBLY.", s.min_r));
    }
    put("  C. TAPERED INSULATION: FACTORY-CUT, SLOPED TO DRAINS, WITH CRICKETS AND SADDLES AS SHOWN.".into());
    put(String::new());
    let (num, name) = match e.roof.membrane.as_str() {
        "tpo" => ("07 54 23", "THERMOPLASTIC POLYOLEFIN (TPO) ROOFING"),
        "pvc" => ("07 54 19", "POLYVINYL CHLORIDE (PVC) ROOFING"),
        "epdm" => ("07 53 23", "ETHYLENE-PROPYLENE-DIENE-MONOMER (EPDM) ROOFING"),
        _ => ("07 52 16", "STYRENE-BUTADIENE-STYRENE (SBS) MODIFIED BITUMINOUS MEMBRANE ROOFING"),
    };
    put(format!("SECTION {num} - {name}"));
    put("PART 2 - PRODUCTS".into());
    if let Some(m) = s.membrane_mil {
        if st.spec_words == 1 {
            put(format!("  A. SHEET: {m:.0} MIL NOMINAL THICKNESS, REINFORCED."));
        } else {
            put(format!("  A. MEMBRANE THICKNESS: {m:.0} MILS."));
        }
    } else {
        put("  A. BASE SHEET AND GRANULE-SURFACED CAP SHEET, TORCH APPLIED.".into());
    }
    put(format!("  B. ATTACHMENT: {}.", e.roof.attachment.to_ascii_uppercase()));
    put("  C. WALKWAY PADS: MANUFACTURER'S 30 BY 30 INCH PADS.".into());
    put(String::new());
    put("SECTION 07 62 00 - SHEET METAL FLASHING AND TRIM".into());
    put(format!("  A. COPINGS AND ROOF EDGES: {} GAUGE PREFINISHED GALVANIZED STEEL.", s.metal_ga));
    put("  B. OVERFLOW SCUPPERS AND COUNTERFLASHINGS: SAME AS COPING.".into());
    put(String::new());
    put("SECTION 07 27 26 - FLUID-APPLIED MEMBRANE AIR BARRIERS".into());
    put("  A. VAPOR-PERMEABLE, FLUID-APPLIED AIR AND WATER-RESISTIVE BARRIER.".into());
    put(String::new());
    put("SECTION 07 92 00 - JOINT SEALANTS".into());
    put("  A. EXTERIOR JOINTS: SILICONE, LOW MODULUS, WITH BACKER ROD.".into());
    out
}
