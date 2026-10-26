//! The sheets of the set and where everything goes on them: scale, paper, how the plan sits in
//! the drawing (rotated, shifted far from the origin), and the size and height tags on the runs.

use crate::rng::Rng;
use crate::style::{SCALES, Style};
use crate::truth::*;
use std::collections::BTreeMap;

/// The layer name used for callout symbols on the sheets.
pub const CALLOUT_LAYER: &str = "CALLOUT";

/// The layer name used for site elements on the civil sheets.
pub const SITE_LAYER: &str = "C-SITE";

/// The layer name used for underground utilities on the civil sheets.
pub const UNDERGROUND_LAYER: &str = "C-UNDR";

/// What a sheet shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Cover,
    Framing(String),
    Rcp(String),
    Mech(String),
    Plumb(String),
    Elec(String),
    Fire(String),
    /// An enlarged plumbing plan of a level: `TOILET` rooms or `MECH` room.
    Enlarged(String, String),
    /// Electrical lighting plan for a level.
    Lighting(String),
    /// Electrical power plan for a level.
    Power(String),
    Schedules(String),
    /// An architectural floor plan: walls, doors, wall type tags.
    Floor(String),
    /// The wall type legend.
    WallTypes,
    /// Ceiling types, soffit and head of wall details.
    CeilingDetails,
    /// The room finish schedule.
    FinishSchedule,
    /// An enlarged architectural plan of a level's `TOILET` rooms (sets with planted gaps only):
    /// walls and their tags again, bigger.
    EnlargedFloor(String, String),
    /// The roof plan.
    Roof,
    /// The door, hardware and window schedules.
    DoorSchedule,
    /// The architecture's room finishes and finish legend (Q307; A-601's room finish schedule is
    /// the ceilings').
    ArchFinishes,
    /// The roof drainage and flashing plan (Q337).
    RoofDrainage,
    /// The roof and exterior wall assemblies, wall section and detail index (Q337).
    Assemblies,
    /// The civil site plan: parking, light poles, transformer pad, underground utilities.
    Site,
    /// The grading and earthwork plan (C-102).
    Grading,
    /// The site utility plan (C-103): storm, sanitary, water structures.
    SiteUtility,
    /// The sitework schedule (C-601): quantities, paving, landscaping, fencing, furnishings.
    SiteSchedules,
    /// The enlarged guest unit plans (Q305): one plan per unit type, the tag legend and the
    /// unit overrides table.
    UnitPlans,
}

impl Kind {
    pub fn plan(&self) -> bool {
        !matches!(
            self,
            Kind::Cover
                | Kind::Schedules(_)
                | Kind::WallTypes
                | Kind::CeilingDetails
                | Kind::FinishSchedule
                | Kind::DoorSchedule
                | Kind::ArchFinishes
                | Kind::Assemblies
                | Kind::SiteSchedules
                | Kind::UnitPlans
        )
    }
}

/// Electrical conduit size label: `3/4"C`, `1"C`.
pub fn conduit_label(size_in: f64) -> String {
    format!("{}\"C", crate::truth::pipe_size(size_in))
}

/// Where the title block goes and the room left for the drawing, paper inches.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub paper: [f64; 2],
    /// Border rectangle.
    pub border: [[f64; 2]; 2],
    /// Title block rectangle.
    pub title: [[f64; 2]; 2],
    /// Drawing area.
    pub area: [[f64; 2]; 2],
}

pub fn frame(st: &Style) -> Frame {
    let [w, h] = st.paper.1;
    let border = [[1.0, 0.5], [w - 0.5, h - 0.5]];
    if st.title_right {
        let tw = 4.5;
        Frame { paper: [w, h], border, title: [[w - 0.5 - tw, 0.5], [w - 0.5, h - 0.5]], area: [[1.5, 1.0], [w - 1.0 - tw, h - 1.0]] }
    } else {
        let th = 3.0;
        Frame { paper: [w, h], border, title: [[1.0, 0.5], [w - 0.5, 0.5 + th]], area: [[1.5, 1.0 + th], [w - 1.0, h - 1.0]] }
    }
}

/// The sheets of a set: number, title, discipline, what it shows.
pub fn sheet_list() -> Vec<(String, String, String, Kind)> {
    let mut v = vec![("G-001".to_string(), "COVER SHEET".to_string(), "G".to_string(), Kind::Cover)];
    v.push(("S-102".into(), "LEVEL 2 FRAMING PLAN".into(), "S".into(), Kind::Framing("2".into())));
    v.push(("S-103".into(), "ROOF FRAMING PLAN".into(), "S".into(), Kind::Framing("R".into())));
    for l in ["1", "2"] {
        v.push((format!("A-12{l}"), format!("LEVEL {l} REFLECTED CEILING PLAN"), "A".into(), Kind::Rcp(l.into())));
    }
    for l in ["1", "2"] {
        v.push((format!("M-10{l}"), format!("LEVEL {l} MECHANICAL PLAN"), "M".into(), Kind::Mech(l.into())));
    }
    for l in ["1", "2"] {
        v.push((format!("P-10{l}"), format!("LEVEL {l} PLUMBING PLAN"), "P".into(), Kind::Plumb(l.into())));
    }
    v.push(("P-401".into(), "ENLARGED TOILET ROOM PLUMBING PLAN - LEVEL 1".into(), "P".into(), Kind::Enlarged("1".into(), "TOILET".into())));
    v.push(("P-402".into(), "ENLARGED TOILET ROOM PLUMBING PLAN - LEVEL 2".into(), "P".into(), Kind::Enlarged("2".into(), "TOILET".into())));
    v.push(("P-403".into(), "ENLARGED MECHANICAL ROOM PLUMBING PLAN - LEVEL 1".into(), "P".into(), Kind::Enlarged("1".into(), "MECH".into())));
    for l in ["1", "2"] {
        v.push((format!("E-30{l}"), format!("LEVEL {l} IN-WALL DEVICE PLAN"), "E".into(), Kind::Elec(l.into())));
    }
    for l in ["1", "2"] {
        v.push((format!("FP-10{l}"), format!("LEVEL {l} FIRE PROTECTION PLAN"), "FP".into(), Kind::Fire(l.into())));
    }
    for l in ["1", "2"] {
        v.push((format!("E-10{l}"), format!("LEVEL {l} LIGHTING PLAN"), "E".into(), Kind::Lighting(l.into())));
    }
    for l in ["1", "2"] {
        v.push((format!("E-20{l}"), format!("LEVEL {l} POWER PLAN"), "E".into(), Kind::Power(l.into())));
    }
    v.push(("M-601".into(), "MECHANICAL SCHEDULES".into(), "M".into(), Kind::Schedules("M".into())));
    v.push(("P-601".into(), "PLUMBING SCHEDULES".into(), "P".into(), Kind::Schedules("P".into())));
    v.push(("E-601".into(), "ELECTRICAL SCHEDULES".into(), "E".into(), Kind::Schedules("E".into())));
    // Added after the others, so the sheets above keep their random draws.
    for l in ["1", "2"] {
        v.push((format!("A-10{l}"), format!("LEVEL {l} FLOOR PLAN"), "A".into(), Kind::Floor(l.into())));
    }
    v.push(("A-501".into(), "WALL TYPES AND PARTITION SCHEDULE".into(), "A".into(), Kind::WallTypes));
    v.push((crate::ceilings::DETAILS_SHEET.into(), "CEILING TYPES, SOFFIT AND HEAD OF WALL DETAILS".into(), "A".into(), Kind::CeilingDetails));
    v.push(("A-601".into(), "ROOM FINISH SCHEDULE".into(), "A".into(), Kind::FinishSchedule));
    // Architectural detail/elevation/section sheets (non-plan, for callout targets)
    v.push(("A-201".into(), "EXTERIOR ELEVATIONS".into(), "A".into(), Kind::Schedules("A".into())));
    v.push(("A-301".into(), "BUILDING SECTIONS".into(), "A".into(), Kind::Schedules("A".into())));
    v.push((WALL_DETAILS_SHEET.into(), "WALL SECTIONS AND DETAILS".into(), "A".into(), Kind::Schedules("A".into())));
    // Added after every other sheet, so every sheet above keeps its random draws.
    v.push(("A-103".into(), "ROOF PLAN".into(), "A".into(), Kind::Roof));
    v.push(("A-602".into(), "DOOR AND WINDOW SCHEDULES".into(), "A".into(), Kind::DoorSchedule));
    v.push(("A-603".into(), "ROOM FINISHES SCHEDULE AND FINISH LEGEND".into(), "A".into(), Kind::ArchFinishes));
    // Added after the architecture's sheets, so every sheet above keeps its random draws.
    v.push(("A-104".into(), "ROOF DRAINAGE AND FLASHING PLAN".into(), "A".into(), Kind::RoofDrainage));
    v.push((crate::envelope::ASSEMBLIES_SHEET.into(), "ROOF AND EXTERIOR WALL ASSEMBLIES".into(), "A".into(), Kind::Assemblies));
    v
}

/// The wall sections and details sheet the enlarged plans' detail bubbles point at (A-501 is the
/// wall types sheet, A-502 the ceiling details).
pub const WALL_DETAILS_SHEET: &str = "A-503";

/// The building's plan extents with what is drawn outside it (grid bubbles, the meter, the
/// sewer), feet.
fn extents(t: &Truth) -> ([f64; 2], [f64; 2]) {
    let xs: Vec<f64> = t.building.grid.x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = t.building.grid.y.iter().map(|g| g.at_ft).collect();
    let (x0, x1) = (xs.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    let (y0, y1) = (ys.iter().copied().fold(f64::INFINITY, f64::min), ys.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    ([x0 - 3.0, y0 - 4.0], [x1 + 3.0, y1 + 6.0])
}

/// Extent that includes the building grid AND all site elements, feet, with margin.
pub fn site_extents(t: &Truth) -> ([f64; 2], [f64; 2]) {
    let (mut lo, mut hi) = extents(t);
    for se in &t.site_elements {
        lo[0] = lo[0].min(se.at_ft[0]);
        lo[1] = lo[1].min(se.at_ft[1]);
        hi[0] = hi[0].max(se.at_ft[0]);
        hi[1] = hi[1].max(se.at_ft[1]);
        if let (Some(mn), Some(mx)) = (se.min_ft, se.max_ft) {
            lo[0] = lo[0].min(mn[0]);
            lo[1] = lo[1].min(mn[1]);
            hi[0] = hi[0].max(mx[0]);
            hi[1] = hi[1].max(mx[1]);
        }
    }
    for ss in &t.site_structures {
        lo[0] = lo[0].min(ss.at_ft[0]);
        lo[1] = lo[1].min(ss.at_ft[1]);
        hi[0] = hi[0].max(ss.at_ft[0]);
        hi[1] = hi[1].max(ss.at_ft[1]);
    }
    for gc in &t.grading_contours {
        for pt in &gc.points {
            lo[0] = lo[0].min(pt[0]);
            lo[1] = lo[1].min(pt[1]);
            hi[0] = hi[0].max(pt[0]);
            hi[1] = hi[1].max(pt[1]);
        }
    }
    // Margin around the site
    ([lo[0] - 5.0, lo[1] - 5.0], [hi[0] + 5.0, hi[1] + 5.0])
}

/// The part of a level an enlarged plan shows, feet.
pub fn window(t: &Truth, area: &str) -> ([f64; 2], [f64; 2]) {
    let xs: Vec<f64> = t.building.grid.x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = t.building.grid.y.iter().map(|g| g.at_ft).collect();
    let n = xs.len();
    let x = |i: usize| xs.get(i).copied().unwrap_or(0.0);
    let y = |i: usize| ys.get(i).copied().unwrap_or(0.0);
    if area == "MECH" {
        ([x(0) - 4.0, y(0) - 3.5], [x(1) + 1.5, y(2) + 0.5])
    } else {
        ([x(n.saturating_sub(2)) - 1.5, y(1) + 0.5], [x(n.saturating_sub(1)) + 2.0, y(3) + 6.5])
    }
}

/// Enlarged plans' scales.
/// Plan scales for a building too big for [`SCALES`] on the sheet, largest first.
pub const SMALL_SCALES: &[(&str, f64)] = &[("1/16\" = 1'-0\"", 0.0625), ("1/32\" = 1'-0\"", 0.03125)];
pub const ENLARGED_SCALES: &[(&str, f64)] = &[("1/4\" = 1'-0\"", 0.25), ("3/8\" = 1'-0\"", 0.375), ("1/2\" = 1'-0\"", 0.5)];

/// Grid bubbles and labels outside the extents need this much paper, inches, each side.
pub const PLAN_MARGIN_IN: f64 = 1.4;

/// Plan a sheet set: one entry per sheet in the truth (files named `<number>.pdf/.dxf`).
pub fn plan_sheets(t: &mut Truth, st: &Style, r: &mut Rng) {
    let fr = frame(st);
    let (lo, hi) = extents(t);
    let (bw, bh) = (hi[0] - lo[0], hi[1] - lo[1]);
    let aw = fr.area[1][0] - fr.area[0][0] - 2.0 * PLAN_MARGIN_IN;
    // Room under the plan for its title and scale bar.
    let ah = fr.area[1][1] - fr.area[0][1] - 2.0 * PLAN_MARGIN_IN - 1.0;
    let fits = |s: f64, rot: bool| if rot { bh * s <= aw && bw * s <= ah } else { bw * s <= aw && bh * s <= ah };
    let common = r.chance(0.6);
    let fitting: Vec<(&str, f64)> = SCALES.iter().copied().filter(|(_, s)| fits(*s, false)).collect();
    let set_scale = r.pick(&fitting);
    let set_scale = if set_scale.1 > 0.0 { set_scale } else { SCALES.first().copied().unwrap_or(("1/8\" = 1'-0\"", 0.125)) };
    for (number, title, disc, kind) in sheet_list() {
        let level = match &kind {
            Kind::Framing(l)
            | Kind::Rcp(l)
            | Kind::Mech(l)
            | Kind::Plumb(l)
            | Kind::Elec(l)
            | Kind::Fire(l)
            | Kind::Enlarged(l, _)
            | Kind::Lighting(l)
            | Kind::Power(l)
            | Kind::Floor(l) => Some(l.clone()),
            Kind::Roof | Kind::RoofDrainage => Some("R".to_string()),
            _ => None,
        };
        let plan = kind.plan();
        let win = match &kind {
            Kind::Enlarged(_, area) => Some(window(t, area)),
            _ => None,
        };
        let (lo, hi) = win.unwrap_or((lo, hi));
        let (bw, bh) = (hi[0] - lo[0], hi[1] - lo[1]);
        let fits = |s: f64, rot: bool| if rot { bh * s <= aw && bw * s <= ah } else { bw * s <= aw && bh * s <= ah };
        // A plan is sometimes turned on its sheet (when it still fits), and drawn at a scale
        // of its own.
        let rot = plan && r.chance(0.2) && fits(SCALES.first().map_or(0.125, |s| s.1), true);
        let rot = rot && win.is_none();
        let (scale_text, scale) = if !plan {
            ("", 0.125)
        } else if win.is_some() {
            // The largest that fits: that is what an enlarged plan is for.
            let s = ENLARGED_SCALES.iter().copied().rfind(|(_, s)| fits(*s, false)).unwrap_or_default();
            if s.1 > 0.0 { s } else { ENLARGED_SCALES.first().copied().unwrap_or(("1/4\" = 1'-0\"", 0.25)) }
        } else if common && fits(set_scale.1, rot) {
            set_scale
        } else {
            let options: Vec<(&str, f64)> = SCALES.iter().copied().filter(|(_, s)| fits(*s, rot)).collect();
            let s = r.pick(&options);
            if s.1 > 0.0 { s } else { SCALES.first().copied().unwrap_or(("1/8\" = 1'-0\"", 0.125)) }
        };
        let k = 12.0 / scale; // model inches per paper inch
        let origin = if st.world_offsets { [r.range(0, 400) as f64 * 120.0, r.range(0, 200) as f64 * 120.0] } else { [0.0, 0.0] };
        let rotation_deg = if rot { 90.0 } else { 0.0 };
        // The plan's centre goes to the middle of the drawing area (above its title).
        let centre_paper = [(fr.area[0][0] + fr.area[1][0]) / 2.0, (fr.area[0][1] + 1.0 + fr.area[1][1]) / 2.0];
        let centre_b = [(lo[0] + hi[0]) / 2.0 * 12.0, (lo[1] + hi[1]) / 2.0 * 12.0];
        let rc = rotate(centre_b, rotation_deg);
        let offset = [origin[0] + centre_paper[0] * k - rc[0], origin[1] + centre_paper[1] * k - rc[1]];
        t.sheets.push(Sheet {
            number: number.clone(),
            title,
            discipline: disc,
            level,
            pdf: format!("{number}.pdf"),
            dxf: format!("{number}.dxf"),
            paper_in: fr.paper,
            scale_text: plan.then(|| scale_text.to_string()),
            window_ft: win.map(|(a, b)| [a, b]),
            scale,
            model_from_building: Placement { rotation_deg, offset_in: [round(offset[0]), round(offset[1])] },
            paper_from_model: PaperMap { origin_in: origin, scale: 1.0 / k },
            legend: Vec::new(),
            abbreviations: Vec::new(),
        });
    }
}

fn round(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

pub fn rotate(p: [f64; 2], deg: f64) -> [f64; 2] {
    let (s, c) = deg.to_radians().sin_cos();
    [p[0] * c - p[1] * s, p[0] * s + p[1] * c]
}

/// Building feet → drawing inches on a sheet.
pub fn to_model(sh: &Sheet, p: [f64; 2]) -> [f64; 2] {
    let r = rotate([p[0] * 12.0, p[1] * 12.0], sh.model_from_building.rotation_deg);
    [r[0] + sh.model_from_building.offset_in[0], r[1] + sh.model_from_building.offset_in[1]]
}

/// Paper inches → drawing inches on a sheet.
pub fn paper_to_model(sh: &Sheet, p: [f64; 2]) -> [f64; 2] {
    let k = 1.0 / sh.paper_from_model.scale;
    [sh.paper_from_model.origin_in[0] + p[0] * k, sh.paper_from_model.origin_in[1] + p[1] * k]
}

/// Drawing inches → paper inches.
pub fn model_to_paper(sh: &Sheet, m: [f64; 2]) -> [f64; 2] {
    let s = sh.paper_from_model.scale;
    [(m[0] - sh.paper_from_model.origin_in[0]) * s, (m[1] - sh.paper_from_model.origin_in[1]) * s]
}

/// Groups of horizontal segments that read as one run on a plan: collinear, joined, the same
/// system, shape and size.
fn runs(t: &Truth) -> Vec<Vec<usize>> {
    let n = t.segments.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while let Some(&q) = p.get(i) {
            if q == i {
                break;
            }
            let g = p.get(q).copied().unwrap_or(q);
            if let Some(slot) = p.get_mut(i) {
                *slot = g;
            }
            i = q;
        }
        i
    }
    let index: BTreeMap<&str, usize> = t.segments.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();
    let dir = |s: &Segment| {
        let [a, b] = s.path_ft;
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-12);
        [dx / l, dy / l]
    };
    for node in &t.nodes {
        let segs: Vec<usize> = node.segments.iter().filter_map(|id| index.get(id.as_str()).copied()).collect();
        for (i, &a) in segs.iter().enumerate() {
            for &b in segs.iter().skip(i + 1) {
                let (Some(sa), Some(sb)) = (t.segments.get(a), t.segments.get(b)) else { continue };
                if sa.vertical || sb.vertical || sa.system != sb.system || sa.shape != sb.shape || sa.size_in != sb.size_in {
                    continue;
                }
                let (da, db) = (dir(sa), dir(sb));
                // Drains slope along their run; everything else in a run hangs at one height.
                let level = sa.system == "SAN" || (sa.path_ft[0][2] - sb.path_ft[0][2]).abs() < 1e-6;
                if (da[0] * db[0] + da[1] * db[1]).abs() > 0.999 && level {
                    let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                    if let Some(slot) = parent.get_mut(ra) {
                        *slot = rb;
                    }
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }
    groups.into_values().collect()
}

/// The hotel's sheet list: all disciplines.
pub fn hotel_sheet_list() -> Vec<(String, String, String, Kind)> {
    let mut v = vec![("G-001".to_string(), "COVER SHEET AND SHEET INDEX".to_string(), "G".to_string(), Kind::Cover)];
    // Architectural plans
    for l in ["1", "2", "3", "4"] {
        v.push((format!("A-10{l}"), format!("LEVEL {l} FLOOR PLAN"), "A".into(), Kind::Rcp(l.into()))); // reuse Rcp kind for arch plans
    }
    // Reflected ceiling plans
    for l in ["1", "2", "3", "4"] {
        v.push((format!("A-20{l}"), format!("LEVEL {l} REFLECTED CEILING PLAN"), "A".into(), Kind::Rcp(l.into())));
    }
    // Enlarged guest unit plans
    v.push((crate::units::UNIT_SHEET.into(), crate::units::UNIT_SHEET_TITLE.into(), "A".into(), Kind::UnitPlans));
    // Structural framing
    for (num, title, code) in [
        ("S-102", "LEVEL 2 FRAMING PLAN", "2"),
        ("S-103", "LEVEL 3 FRAMING PLAN", "3"),
        ("S-104", "LEVEL 4 FRAMING PLAN", "4"),
        ("S-105", "ROOF FRAMING PLAN", "R"),
    ] {
        v.push((num.into(), title.into(), "S".into(), Kind::Framing(code.into())));
    }
    // Mechanical
    for l in ["1", "2", "3", "4"] {
        v.push((format!("M-10{l}"), format!("LEVEL {l} MECHANICAL PLAN"), "M".into(), Kind::Mech(l.into())));
    }
    // Plumbing
    for l in ["1", "2", "3", "4"] {
        v.push((format!("P-10{l}"), format!("LEVEL {l} PLUMBING PLAN"), "P".into(), Kind::Plumb(l.into())));
    }
    // Fire protection
    for l in ["1", "2", "3", "4"] {
        v.push((format!("FP-10{l}"), format!("LEVEL {l} FIRE PROTECTION PLAN"), "FP".into(), Kind::Fire(l.into())));
    }
    // Electrical lighting
    for l in ["1", "2", "3", "4"] {
        v.push((format!("E-10{l}"), format!("LEVEL {l} LIGHTING PLAN"), "E".into(), Kind::Lighting(l.into())));
    }
    // Electrical power
    for l in ["1", "2", "3", "4"] {
        v.push((format!("E-20{l}"), format!("LEVEL {l} POWER PLAN"), "E".into(), Kind::Power(l.into())));
    }
    // Fire alarm
    for l in ["1", "2", "3", "4"] {
        v.push((format!("FA-10{l}"), format!("LEVEL {l} FIRE ALARM PLAN"), "FA".into(), Kind::Fire(l.into())));
    }
    // Telecom
    for l in ["1", "2", "3", "4"] {
        v.push((format!("T-10{l}"), format!("LEVEL {l} TELECOM PLAN"), "T".into(), Kind::Mech(l.into()))); // reuse Mech kind for layout
    }
    // Civil
    v.push(("C-101".into(), "SITE PLAN".into(), "C".into(), Kind::Site));
    v.push(("C-102".into(), "GRADING AND EARTHWORK PLAN".into(), "C".into(), Kind::Grading));
    v.push(("C-103".into(), "SITE UTILITY PLAN".into(), "C".into(), Kind::SiteUtility));
    v.push(("C-601".into(), "SITEWORK SCHEDULES AND QUANTITIES".into(), "C".into(), Kind::SiteSchedules));
    // Schedules
    v.push(("M-601".into(), "MECHANICAL SCHEDULES".into(), "M".into(), Kind::Schedules("M".into())));
    v.push(("P-601".into(), "PLUMBING SCHEDULES".into(), "P".into(), Kind::Schedules("P".into())));
    v.push(("E-601".into(), "ELECTRICAL SCHEDULES".into(), "E".into(), Kind::Schedules("E".into())));
    v.push(("A-601".into(), "ARCHITECTURAL SCHEDULES".into(), "A".into(), Kind::Schedules("A".into())));
    v
}

/// Plan sheets for the hotel set.
pub fn plan_hotel_sheets(t: &mut Truth, st: &Style, r: &mut Rng) {
    let fr = frame(st);
    let (lo, hi) = extents(t);
    let aw = fr.area[1][0] - fr.area[0][0] - 2.0 * PLAN_MARGIN_IN;
    let ah = fr.area[1][1] - fr.area[0][1] - 2.0 * PLAN_MARGIN_IN - 1.0;
    let (bw, bh) = (hi[0] - lo[0], hi[1] - lo[1]);
    let fits = |s: f64, rot: bool| if rot { bh * s <= aw && bw * s <= ah } else { bw * s <= aw && bh * s <= ah };
    let fitting: Vec<(&str, f64)> = SCALES.iter().copied().filter(|(_, s)| fits(*s, false)).collect();
    let set_scale = r.pick(&fitting);
    // A building too long for the usual plan scales (the double-size hotel) drops to 1/16" or
    // 1/32" rather than running off the sheet.
    let small = SMALL_SCALES.iter().copied().find(|(_, s)| fits(*s, false));
    let set_scale = if set_scale.1 > 0.0 { set_scale } else { small.or(SCALES.first().copied()).unwrap_or(("1/8\" = 1'-0\"", 0.125)) };
    for (number, title, disc, kind) in hotel_sheet_list() {
        let level = match &kind {
            Kind::Framing(l)
            | Kind::Rcp(l)
            | Kind::Mech(l)
            | Kind::Plumb(l)
            | Kind::Fire(l)
            | Kind::Enlarged(l, _)
            | Kind::Lighting(l)
            | Kind::Power(l) => Some(l.clone()),
            _ => None,
        };
        let plan = kind.plan();
        // Site plans need to fit the wider site extent.
        let is_site = matches!(kind, Kind::Site | Kind::Grading | Kind::SiteUtility);
        let (s_lo, s_hi) = if is_site { site_extents(t) } else { (lo, hi) };
        let (sw, sh_ext) = (s_hi[0] - s_lo[0], s_hi[1] - s_lo[1]);
        let site_fits = |s: f64| sw * s <= aw && sh_ext * s <= ah;
        let (scale_text, scale) = if !plan {
            ("", 0.125)
        } else if is_site {
            let options: Vec<(&str, f64)> = SCALES.iter().copied().filter(|(_, s)| site_fits(*s)).collect();
            let s = r.pick(&options);
            if s.1 > 0.0 { s } else { SCALES.first().copied().unwrap_or(("1/8\" = 1'-0\"", 0.125)) }
        } else if fits(set_scale.1, false) {
            set_scale
        } else {
            let options: Vec<(&str, f64)> = SCALES.iter().copied().filter(|(_, s)| fits(*s, false)).collect();
            let s = r.pick(&options);
            if s.1 > 0.0 { s } else { small.or(SCALES.first().copied()).unwrap_or(("1/8\" = 1'-0\"", 0.125)) }
        };
        let k = 12.0 / scale;
        let origin = if st.world_offsets { [r.range(0, 400) as f64 * 120.0, r.range(0, 200) as f64 * 120.0] } else { [0.0, 0.0] };
        let centre_paper = [(fr.area[0][0] + fr.area[1][0]) / 2.0, (fr.area[0][1] + 1.0 + fr.area[1][1]) / 2.0];
        let centre_b = if is_site {
            [(s_lo[0] + s_hi[0]) / 2.0 * 12.0, (s_lo[1] + s_hi[1]) / 2.0 * 12.0]
        } else {
            [(lo[0] + hi[0]) / 2.0 * 12.0, (lo[1] + hi[1]) / 2.0 * 12.0]
        };
        let rc = rotate(centre_b, 0.0);
        let offset = [origin[0] + centre_paper[0] * k - rc[0], origin[1] + centre_paper[1] * k - rc[1]];
        t.sheets.push(Sheet {
            number: number.clone(),
            title,
            discipline: disc,
            level,
            pdf: format!("{number}.pdf"),
            dxf: format!("{number}.dxf"),
            paper_in: fr.paper,
            scale_text: plan.then(|| scale_text.to_string()),
            window_ft: None,
            scale,
            model_from_building: Placement { rotation_deg: 0.0, offset_in: [round(offset[0]), round(offset[1])] },
            paper_from_model: PaperMap { origin_in: origin, scale: 1.0 / k },
            legend: Vec::new(),
            abbreviations: Vec::new(),
        });
    }
    // Assign segments to sheets
    assign_segments_to_hotel_sheets(t);
    // Assign electrical devices and fire alarm devices to their sheets
    assign_devices_to_hotel_sheets(t);
}

/// Assign each segment to the sheet it should appear on (hotel).
fn assign_segments_to_hotel_sheets(t: &mut Truth) {
    let sheet_map: BTreeMap<(String, String), String> =
        t.sheets.iter().filter_map(|sh| sh.level.as_ref().map(|lv| ((sh.discipline.clone(), lv.clone()), sh.number.clone()))).collect();

    for seg in &mut t.segments {
        if !seg.sheet.is_empty() {
            continue;
        }
        let disc = match seg.system.as_str() {
            "SA" | "RA" | "EA" | "OA" => "M",
            "CW" | "HW" | "HWR" | "SAN" | "V" | "G" => "P",
            "FP" => "FP",
            _ => "M",
        };
        if let Some(num) = sheet_map.get(&(disc.to_string(), seg.level.clone())) {
            seg.sheet = num.clone();
        }
    }
}

/// Assign electrical and fire alarm devices to their sheets.
fn assign_devices_to_hotel_sheets(t: &mut Truth) {
    let sheet_map: BTreeMap<(String, String), String> =
        t.sheets.iter().filter_map(|sh| sh.level.as_ref().map(|lv| ((sh.discipline.clone(), lv.clone()), sh.number.clone()))).collect();

    for dev in &mut t.elec_devices {
        if !dev.sheet.is_empty() {
            continue;
        }
        if let Some(num) = sheet_map.get(&("E".to_string(), dev.level.clone())) {
            dev.sheet = num.clone();
        }
    }
    for panel in &mut t.elec_panels {
        if !panel.sheet.is_empty() {
            continue;
        }
        if let Some(num) = sheet_map.get(&("E".to_string(), panel.level.clone())) {
            panel.sheet = num.clone();
        }
    }
    for ckt in &mut t.elec_circuits {
        if !ckt.sheet.is_empty() {
            continue;
        }
        if let Some(num) = sheet_map.get(&("E".to_string(), ckt.level.clone())) {
            ckt.sheet = num.clone();
        }
    }
    for cond in &mut t.elec_conduits {
        if !cond.sheet.is_empty() {
            continue;
        }
        if let Some(num) = sheet_map.get(&("E".to_string(), cond.level.clone())) {
            cond.sheet = num.clone();
        }
    }
    for fa in &mut t.fire_alarm_devices {
        if !fa.sheet.is_empty() {
            continue;
        }
        if let Some(num) = sheet_map.get(&("FA".to_string(), fa.level.clone())) {
            fa.sheet = num.clone();
        }
    }
    for tc in &mut t.telecom_devices {
        if !tc.sheet.is_empty() {
            continue;
        }
        if let Some(num) = sheet_map.get(&("T".to_string(), tc.level.clone())) {
            tc.sheet = num.clone();
        }
    }
}

/// Tags of the cable tray (`12x4 CT` / `BOT @ 9'-2"`, north of it) and of each feeder conduit
/// bank (`(2) 2"C` / `BOC @ 9'-2"`, south of it, on the southmost conduit), on the power plans.
pub fn tag_trays(t: &mut Truth, st: &Style) {
    let scale_of: BTreeMap<String, f64> = t.sheets.iter().map(|s| (s.number.clone(), s.scale)).collect();
    let mut tags: Vec<(String, Tag, String)> = Vec::new();
    for s in t.segments.iter().filter(|s| !s.vertical && (s.kind == "tray" || s.kind == "conduit")) {
        let th = st.text_in / scale_of.get(&s.sheet).copied().unwrap_or(0.125);
        let [a, b] = s.path_ft;
        let h = s.height.clone().unwrap_or_default();
        let half = crate::design::width_in(&s.shape, s.size_in) / 24.0;
        if s.kind == "tray" {
            let lines = vec![
                format!("{}x{} CT", crate::truth::fmt_in(s.size_in[0]), crate::truth::fmt_in(s.size_in[1])),
                format!("BOT @ {}", feet_inches(h.bottom_ft)),
            ];
            let at = [a[0] + (b[0] - a[0]) * 0.5, a[1] + half + th * 2.2];
            tags.push((
                s.id.clone(),
                Tag { text: lines, at_ft: [round(at[0]), round(at[1])], leader: false },
                format!("BOT @ {}", feet_inches(h.bottom_ft)),
            ));
        } else {
            // One tag per bank: on the conduit with no other south of it on the same run.
            let bank: Vec<&Segment> = t
                .segments
                .iter()
                .filter(|o| {
                    o.kind == "conduit"
                        && !o.vertical
                        && o.level == s.level
                        && (o.path_ft[0][0] - a[0]).abs() < 1e-6
                        && (o.path_ft[1][0] - b[0]).abs() < 1e-6
                })
                .collect();
            if bank.iter().any(|o| o.path_ft[0][1] < a[1] - 1e-6) {
                continue;
            }
            let lines = vec![format!("({}) {}\"C", bank.len(), pipe_size(s.size_in[0])), format!("BOC @ {}", feet_inches(h.bottom_ft))];
            let at = [a[0] + (b[0] - a[0]) * 0.62, a[1] - half - th * 2.2];
            tags.push((
                s.id.clone(),
                Tag { text: lines, at_ft: [round(at[0]), round(at[1])], leader: false },
                format!("BOC @ {}", feet_inches(h.bottom_ft)),
            ));
        }
    }
    for (id, tag, note) in tags {
        if let Some(s) = t.segments.iter_mut().find(|s| s.id == id) {
            s.tag = Some(tag);
            if let Some(h) = s.height.as_mut() {
                h.note = Some(note);
            }
        }
    }
}

/// Size tags (and height notes, slopes) on the runs, placed for the sheet they are drawn on.
pub fn tag_runs(t: &mut Truth, st: &Style, planted: &[String], r: &mut Rng) {
    let scale_of: BTreeMap<String, f64> = t.sheets.iter().map(|s| (s.number.clone(), s.scale)).collect();
    let mut placed: BTreeMap<String, Vec<[f64; 4]>> = BTreeMap::new();
    let printed: BTreeMap<String, String> = t.systems.iter().map(|s| (s.code.clone(), s.printed.clone())).collect();
    // Some sprinkler branch lines go untagged: they are sized by the FP sheets' pipe schedule from
    // the heads they carry. A stream of its own, so the other tags don't move.
    let mut untag = Rng::new(t.seed ^ 0x554E_5441_4746);
    for group in runs(t) {
        let members: Vec<&Segment> = group.iter().filter_map(|i| t.segments.get(*i)).collect();
        let Some(first) = members.first() else { continue };
        // Tray and conduit banks are tagged as a whole below.
        if first.vertical || first.kind == "tray" || first.kind == "conduit" {
            continue;
        }
        let total: f64 = members.iter().map(|s| s.length_ft()).sum();
        let is_planted = members.iter().any(|s| planted.contains(&s.id));
        // Short connections at fixtures and devices aren't tagged (they show in the schedules).
        if total < 2.5 && !is_planted {
            continue;
        }
        // Sprinkler pipe exposed in a room open to structure always carries its size and height.
        let open_room = first.system == "FP" && {
            let [p, q] = first.path_ft;
            let m = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
            t.building.rooms.iter().any(|rm| {
                rm.level == first.level
                    && rm.ceiling.is_none()
                    && m[0] > rm.min_ft[0]
                    && m[0] < rm.max_ft[0]
                    && m[1] > rm.min_ft[1]
                    && m[1] < rm.max_ft[1]
            })
        };
        if first.system == "FP" && first.size_in[0] < 2.0 && !is_planted && !open_room && untag.chance(0.4) {
            continue;
        }
        let Some(longest) = members.iter().copied().max_by(|a, b| a.length_ft().total_cmp(&b.length_ft())) else { continue };
        let id = longest.id.clone();
        let scale = scale_of.get(&longest.sheet).copied().unwrap_or(0.125);
        let th = st.text_in / scale; // text height, feet
        let duct = longest.kind == "duct";
        let size = match longest.shape.as_str() {
            "rect" => st.rect_size(longest.size_in[0], longest.size_in[1]),
            "round" => st.round_size(longest.size_in[0]),
            _ => st.pipe_size(longest.size_in[0]),
        };
        let sys = printed.get(&longest.system).cloned().unwrap_or_default();
        let mut lines = vec![if duct || longest.system == "FP" { size } else { format!("{size} {sys}") }];
        let h = longest.height.clone().unwrap_or_default();
        let main = longest.shape == "rect" && longest.system != "OA";
        // A branch leaves a rectangular main; runouts past the VAV boxes aren't height-noted
        // (they hang with the box), mains always are.
        let branch = [&longest.from, &longest.to].iter().any(|n| {
            t.node(n).is_some_and(|n| {
                n.segments.iter().any(|id| t.segment(id).is_some_and(|o| o.shape == "rect" && o.system == longest.system && !o.vertical))
            })
        });
        let share = if !duct {
            st.noted_share * 0.5
        } else if branch {
            st.noted_share
        } else {
            0.0
        };
        // (The open-room rule comes last so the random stream draws as it always did.)
        let noted = is_planted || (!longest.below_floor && longest.system != "SAN" && (main || r.chance(share))) || open_room;
        let mut note = None;
        if noted {
            let n = if duct { st.height_note(h.bottom_ft, h.top_ft, h.centre_ft) } else { st.pipe_height_note(h.bottom_ft, h.centre_ft) };
            lines.push(n.clone());
            note = Some(n);
        }
        let mut slope = None;
        if longest.system == "SAN" {
            // Every drain in the set falls 1/4" per foot.
            let s = st.slope_text(0.25);
            lines.push(format!("@ {s}"));
            slope = Some(s);
        }
        // Inside a duct wide enough to hold it (as drafters write sizes in big ducts), else
        // beside the run's middle (west of runs going north-south, north of the others), or out
        // on a leader.
        // Pipes running side by side get their tags staggered along the run, by system.
        let [a, b] = longest.path_ft;
        let along = if duct {
            0.5
        } else {
            ["CW", "HW", "HWR", "SAN", "G", "V"].iter().position(|c| *c == longest.system).map_or(0.5, |k| 0.25 + 0.12 * k as f64)
        };
        let mid = [a[0] + (b[0] - a[0]) * along, a[1] + (b[1] - a[1]) * along];
        let (mut dx, mut dy) = (b[0] - a[0], b[1] - a[1]);
        if dx < -1e-9 || (dx.abs() < 1e-9 && dy < 0.0) {
            (dx, dy) = (-dx, -dy);
        }
        let l = (dx * dx + dy * dy).sqrt().max(1e-9);
        let normal = [-dy / l, dx / l];
        let half = crate::design::width_in(&longest.shape, longest.size_in) / 24.0;
        let block = lines.len() as f64 * th * 1.4;
        let inside = duct && 2.0 * half >= block * 1.1;
        // Parallel runs beside it on the tag's side (a bundle of pipes): the tag goes past the
        // outermost, on a leader.
        let u = [dx / l, dy / l];
        let proj = |p: [f64; 3]| (p[0] - a[0]) * u[0] + (p[1] - a[1]) * u[1];
        let (s0, s1) = (proj(a).min(proj(b)), proj(a).max(proj(b)));
        let beside = t
            .segments
            .iter()
            .filter(|o| o.id != longest.id && !o.vertical && o.level == longest.level && o.sheet == longest.sheet)
            .filter(|o| {
                let [p, q] = o.path_ft;
                let parallel = ((q[0] - p[0]) * u[1] - (q[1] - p[1]) * u[0]).abs() < 1e-6;
                let (o0, o1) = (proj(p).min(proj(q)), proj(p).max(proj(q)));
                parallel && o1 > s0 + 0.5 && o0 < s1 - 0.5
            })
            .map(|o| {
                let p = o.path_ft[0];
                let d = (p[0] - mid[0]) * normal[0] + (p[1] - mid[1]) * normal[1];
                (d, crate::design::width_in(&o.shape, o.size_in) / 24.0)
            })
            .filter(|(d, w)| *d > 0.0 && *d - w < 3.0)
            .map(|(d, w)| d + w)
            .fold(0.0, f64::max);
        let mut leader = !inside && ((st.leaders && duct) || beside > 0.0);
        let clear = if beside > 0.0 { beside - half + th * 1.5 } else { 0.0 };
        let off = if inside { 0.0 } else { half + th * 0.8 + block / 2.0 + clear + if st.leaders && duct { th * 4.0 } else { 0.0 } };
        // The tag's footprint (building feet), its text running along x or y.
        let rot = t.sheets.iter().find(|x| x.number == longest.sheet).map_or(0.0, |x| x.model_from_building.rotation_deg);
        let runs_y = if st.tags_along || inside { u[1].abs() > 0.5 } else { (rot - 90.0).abs() < 1.0 };
        let wide = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f64 * th * 0.85;
        let (bw, bh) = if runs_y { (block, wide) } else { (wide, block) };
        let boxed = |c: [f64; 2]| [c[0] - bw / 2.0, c[1] - bh / 2.0, c[0] + bw / 2.0, c[1] + bh / 2.0];
        let hits = |b: [f64; 4], placed: &[[f64; 4]]| placed.iter().any(|p| b[0] < p[2] && b[2] > p[0] && b[1] < p[3] && b[3] > p[1]);
        let taken = placed.entry(longest.sheet.clone()).or_default();
        // Slide along the run, then step out, until it is clear of the tags already placed.
        let mut at = [mid[0] + normal[0] * off, mid[1] + normal[1] * off];
        'search: for step in 0..6 {
            for slide in [0.0, 0.15, -0.15, 0.3, -0.3] {
                let f = (along + slide).clamp(0.08, 0.92);
                let base = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
                let o = if inside && step == 0 { 0.0 } else { off.max(half + th * 0.8 + block / 2.0) + step as f64 * block * 1.1 };
                let c = [base[0] + normal[0] * o, base[1] + normal[1] * o];
                if !hits(boxed(c), taken) {
                    at = c;
                    if step > 0 {
                        leader = true;
                    }
                    break 'search;
                }
            }
        }
        let inside = inside && (at[0] - mid[0]).abs() < 1e-9 && (at[1] - mid[1]).abs() < 1e-9;
        let leader = leader && !inside;
        taken.push(boxed(at));
        let ids: Vec<String> = members.iter().map(|s| s.id.clone()).collect();
        for s in t.segments.iter_mut().filter(|s| ids.contains(&s.id)) {
            if let Some(h) = s.height.as_mut() {
                h.note = note.clone();
                if note.is_some() {
                    h.source = crate::truth::HeightSource::Note;
                }
            }
            if slope.is_some() {
                s.slope = slope.clone();
            }
        }
        if let Some(s) = t.segments.iter_mut().find(|s| s.id == id) {
            s.tag = Some(Tag { text: lines, at_ft: [round(at[0]), round(at[1])], leader });
        }
    }
}

/// Generate callout entries for the office fixture set.
///
/// Creates section marks, elevation tags, detail bubbles and enlarged-plan boundary callouts
/// on plan sheets, each pointing at a target sheet. One or two broken callouts are planted
/// (pointing at sheets that don't exist in the set).
pub fn plan_callouts(t: &mut Truth, _r: &mut Rng) {
    let mut callouts = Vec::new();
    let mut next_id = 0u32;
    let mut id = || {
        next_id += 1;
        format!("co-{next_id:03}")
    };
    let xs: Vec<f64> = t.building.grid.x.iter().map(|g| g.at_ft).collect();
    let ys: Vec<f64> = t.building.grid.y.iter().map(|g| g.at_ft).collect();
    let x_max = xs.last().copied().unwrap_or(100.0);
    let y_max = ys.last().copied().unwrap_or(60.0);

    // The A-sheets in the set for callout targets.
    let has_sheet = |n: &str| t.sheets.iter().any(|s| s.number == n);

    // --- Section marks on A-RCP sheets (reflected ceiling plans) ---
    // For each section in the truth, put a section mark on each RCP sheet for that level.
    let mut detail_num = 0u32;
    for sect in t.sections.clone() {
        detail_num += 1;
        let detail_label = format!("{detail_num}");
        let target = "A-301".to_string();
        // Put section marks on every plan sheet that has a level.
        for sh in t.sheets.clone() {
            if sh.level.is_none() || sh.window_ft.is_some() {
                continue;
            }
            let mid_ft = [(sect.cut_from_ft[0] + sect.cut_to_ft[0]) / 2.0, (sect.cut_from_ft[1] + sect.cut_to_ft[1]) / 2.0];
            callouts.push(CalloutEntry {
                id: id(),
                kind: "section_mark".into(),
                on_sheet: sh.number.clone(),
                detail: detail_label.clone(),
                target_sheet: target.clone(),
                at_ft: [round(mid_ft[0]), round(mid_ft[1])],
                cut_from_ft: Some(sect.cut_from_ft),
                cut_to_ft: Some(sect.cut_to_ft),
                looking: Some(sect.looking.clone()),
                broken: false,
            });
        }
    }

    // --- Elevation tags on A-RCP sheets ---
    // Put elevation callouts along the building edges on A-RCP sheets.
    let elev_positions: Vec<([f64; 2], &str)> = vec![
        ([x_max / 2.0, y_max + 2.0], "north"),
        ([x_max / 2.0, -2.0], "south"),
        ([x_max + 2.0, y_max / 2.0], "east"),
        ([-2.0, y_max / 2.0], "west"),
    ];
    let mut elev_detail = 0u32;
    for (pos, dir) in &elev_positions {
        elev_detail += 1;
        let detail_label = format!("{elev_detail}");
        let target = "A-201".to_string();
        // Place on level 1 A-RCP
        for sh in t.sheets.clone() {
            if sh.discipline != "A" || sh.level.as_deref() != Some("1") || sh.window_ft.is_some() {
                continue;
            }
            callouts.push(CalloutEntry {
                id: id(),
                kind: "elevation_tag".into(),
                on_sheet: sh.number.clone(),
                detail: detail_label.clone(),
                target_sheet: target.clone(),
                at_ft: [round(pos[0]), round(pos[1])],
                cut_from_ft: None,
                cut_to_ft: None,
                looking: Some((*dir).to_string()),
                broken: false,
            });
        }
    }

    // --- Detail bubbles on enlarged plumbing plans pointing to the wall details sheet ---
    for sh in t.sheets.clone() {
        if sh.window_ft.is_none() {
            continue;
        }
        let win = sh.window_ft.unwrap_or_default();
        let cx = (win[0][0] + win[1][0]) / 2.0;
        let cy = (win[0][1] + win[1][1]) / 2.0;
        detail_num += 1;
        callouts.push(CalloutEntry {
            id: id(),
            kind: "detail_bubble".into(),
            on_sheet: sh.number.clone(),
            detail: format!("{detail_num}"),
            target_sheet: WALL_DETAILS_SHEET.into(),
            at_ft: [round(cx), round(cy)],
            cut_from_ft: None,
            cut_to_ft: None,
            looking: None,
            broken: false,
        });
    }

    // --- Enlarged-plan boundary callouts on base plumbing plans ---
    let enlarged_sheets: Vec<(String, String, [[f64; 2]; 2])> =
        t.sheets.iter().filter_map(|s| s.window_ft.map(|w| (s.number.clone(), s.level.clone().unwrap_or_default(), w))).collect();
    for (en_number, en_level, _win) in &enlarged_sheets {
        for sh in t.sheets.clone() {
            if sh.discipline != "P" || sh.level.as_deref() != Some(en_level.as_str()) || sh.window_ft.is_some() {
                continue;
            }
            let win = _win;
            let cx = (win[0][0] + win[1][0]) / 2.0;
            let cy = (win[0][1] + win[1][1]) / 2.0;
            callouts.push(CalloutEntry {
                id: id(),
                kind: "enlarged_plan".into(),
                on_sheet: sh.number.clone(),
                detail: en_number.clone(),
                target_sheet: en_number.clone(),
                at_ft: [round(cx), round(cy)],
                cut_from_ft: Some(win[0]),
                cut_to_ft: Some(win[1]),
                looking: None,
                broken: false,
            });
        }
    }

    // --- Key plans on mechanical sheets ---
    // Key plans go in the title block area, not at building coordinates.
    // We record them with position [0, 0] (the reader identifies them by kind + layer).
    for sh in t.sheets.clone() {
        if sh.discipline == "M" && sh.level.is_some() && sh.window_ft.is_none() {
            callouts.push(CalloutEntry {
                id: id(),
                kind: "key_plan".into(),
                on_sheet: sh.number.clone(),
                detail: sh.number.clone(),
                target_sheet: sh.number.clone(),
                at_ft: [0.0, 0.0],
                cut_from_ft: None,
                cut_to_ft: None,
                looking: None,
                broken: false,
            });
        }
    }

    // --- Planted broken callouts ---
    // A section mark on level 1 RCP pointing to a nonexistent sheet.
    if let Some(rcp) = t.sheets.iter().find(|s| s.number == "A-121") {
        let mid_x = x_max * 0.75;
        let mid_y = y_max * 0.75;
        callouts.push(CalloutEntry {
            id: id(),
            kind: "section_mark".into(),
            on_sheet: rcp.number.clone(),
            detail: "99".into(),
            target_sheet: "A-999".into(),
            at_ft: [round(mid_x), round(mid_y)],
            cut_from_ft: Some([mid_x, 0.0]),
            cut_to_ft: Some([mid_x, y_max]),
            looking: Some("east".into()),
            broken: true,
        });
    }
    // A detail bubble on P-401 pointing to nonexistent detail sheet.
    if has_sheet("P-401") {
        callouts.push(CalloutEntry {
            id: id(),
            kind: "detail_bubble".into(),
            on_sheet: "P-401".into(),
            detail: "77".into(),
            target_sheet: "P-777".into(),
            at_ft: [round(xs.first().copied().unwrap_or(0.0) + 1.0), round(ys.get(1).copied().unwrap_or(20.0) + 1.0)],
            cut_from_ft: None,
            cut_to_ft: None,
            looking: None,
            broken: true,
        });
    }

    t.callouts = callouts;
}

/// Generate callout entries for the hotel fixture set.
pub fn plan_hotel_callouts(t: &mut Truth, r: &mut Rng) {
    // Reuse the same logic — the hotel has more sheets but the callout structure is the same.
    plan_callouts(t, r);
}
