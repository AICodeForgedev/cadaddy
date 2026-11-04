//! E-GEAR-SPACE, E-GEAR-FILL and E-GEAR-SIZE (Q6151): Validate checks the electrical gear (panels,
//! switchboards, switchgear, transformers, disconnects: `mep.equipment` of a gear kind) against the
//! documents and the code, recomputed here from the documents — never read back from the build's
//! own gear review (`gear.check`), whose RFIs are only listed beside the answer.
//!
//! - **E-GEAR-SPACE** (NEC 110.26, the depths, width and height as data in `coderules.json`
//!   E-WORKSPACE): in front of each piece of gear (the side away from the wall it stands on), a
//!   space as deep as its voltage to ground calls for, as wide as the gear or 30 in, clear of walls,
//!   other equipment and door swings; headroom over it (the ceiling at least 6.5 ft or the gear's
//!   top); and its dedicated space (its footprint from the floor to 6 ft over it) free of ducts and
//!   pipes. A depth that holds for condition 1 but not condition 3 is `unsure` (what faces the gear
//!   is not known); a door whose swing direction nothing says is `unsure` where it could swing in.
//! - **E-GEAR-FILL**: per enclosure, the raceways every feeder brings in through each side (the
//!   feeders' conduits and trade sizes) packed on that side's conduit area (drawn in the submittal,
//!   else the box less the rule's margins: then a pass needs half the area to spare), the
//!   conductors in the wiring gutter against its fill (NEC 312.8, 40%), the wire-bending space at
//!   each lug (NEC 312.6) and the lugs against each feeder's parallel sets, size and metal. Gutter
//!   or bending space the documents don't give is `unsure` (missing), never a pass; feeders a
//!   panel's schedule names that no feeder record gives conduits for make its count `unsure`.
//! - **E-GEAR-SIZE**: the model's enclosure (its built box: height, width, depth) against the
//!   submittal's, re-read from the job's gear submittals when they are on this machine, else as the
//!   build recorded it (`BOX`, citing the submittal).
//!
//! The build's own `conflict` review items about gear come in as `conflict` findings (source
//! `build`). Packets for areas holding gear carry each piece's facts, its gear submittal pages,
//! the riser (one-line) and the panel and feeder schedules ([`for_packet`]).

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::validate::Model;
use cadaddy_walk3d::{BuildingSpec, Feeder, MepEquipment, MepKind, Termination};
use serde_json::{Value, json};

use super::gearbuild::SubmittedGear;
use super::validate::Frame;
use super::validate_packet::{Area, inside, point_in, pt, r2};
use super::validate_rules::Effective;

pub const SPACE: &str = "E-GEAR-SPACE";
pub const FILL: &str = "E-GEAR-FILL";
pub const SIZE: &str = "E-GEAR-SIZE";
/// The verdicts the gear rules give.
pub const VERDICTS: [&str; 4] = ["pass", "fail", "unsure", "conflict"];

/// Gear checked per screen, at most.
const MAX_GEAR: usize = 5_000;
/// Feeder ends and raceways counted per piece of gear, at most (hostile specs ask for billions).
const MAX_ENDS: usize = 400;
const MAX_COUNT: u32 = 1_000;
/// Obstructions looked at per level, at most.
const MAX_OBSTACLES: usize = 200_000;
/// Mesh vertices read for a built box.
const MAX_VERTS: usize = 64;
/// How far in front of a piece of gear its working space is measured, feet.
const REACH_FT: f64 = 15.0;
/// A side wall touching the working space's edge is no obstruction (half an inch, feet).
const EDGE_FT: f64 = 0.5 / 12.0;
/// A wall this near a face is the wall the gear stands on (feet).
const BACK_WALL_FT: f64 = 1.5;
/// The gear submittals read, at most, and the files looked through for them.
const MAX_FILES: usize = 24;
const MAX_FILES_LOOKED: usize = 2_000;
/// Pages of a submittal looked through, elevation drawings read, pages kept per tag.
const MAX_PAGES: usize = 2_000;
const MAX_ELEVATIONS: usize = 12;
const MAX_PAGES_PER_TAG: usize = 4;
/// Words in a submittal's file name that make it a gear submittal.
const GEAR_FILE_WORDS: [&str; 5] = ["GEAR", "SWITCHGEAR", "PANELBOARD", "SWITCHBOARD", "PANEL"];
/// Default size tolerance (inches) when the rule gives none.
const DEFAULT_SIZE_IN: f64 = 0.5;
/// Sheets cropped whole for a packet's gear, at most.
const MAX_GEAR_SHEETS: usize = 6;
const QUOTE_CHARS: usize = 300;

// ---------------------------------------------------------------- the documents, re-read

/// What the job's gear submittals say, read again here (not the build's copy).
#[derive(Clone, Debug, Default)]
pub(crate) struct GearDocs {
    /// Per tag (upper case): box, feed, lugs and conduit areas.
    pub gear: BTreeMap<String, SubmittedGear>,
    /// Per tag (upper case): the submittal pages that name it (`<file> p.<n>`).
    pub pages: BTreeMap<String, Vec<String>>,
    /// The gear submittals read (file names) and those that could not be, with why.
    pub read: Vec<String>,
    pub unread: Vec<(String, String)>,
}

impl GearDocs {
    fn of(&self, tag: &str) -> Option<&SubmittedGear> {
        self.gear.get(&tag.trim().to_ascii_uppercase())
    }
}

/// Does a screen run any gear rule (`only` empty: every enabled rule)?
pub(crate) fn wanted(rules: &[Effective], only: &[String]) -> bool {
    [SPACE, FILL, SIZE]
        .iter()
        .any(|id| rules.iter().any(|r| r.enabled && r.rule.id == *id) && (only.is_empty() || only.iter().any(|o| o.eq_ignore_ascii_case(id))))
}

/// Electrical gear: an equipment kind that is one, or anything with terminations.
fn is_gear(e: &MepEquipment) -> bool {
    super::gear::is_gear(e)
        || matches!(e.kind.to_ascii_lowercase().as_str(), "load_center" | "mdp" | "distribution_panel" | "panel_board" | "mcc" | "meter_center")
}

/// The gear tags of the spec: its gear and the panels its schedules list.
fn gear_tags(sp: &BuildingSpec) -> Vec<String> {
    let mut out: BTreeSet<String> = sp.mep.equipment.iter().filter(|e| is_gear(e)).map(|e| e.tag.trim().to_string()).collect();
    out.extend(sp.mep.panel_schedules.iter().filter(|p| p.typical_of.is_none()).map(|p| p.panel.trim().to_string()));
    out.into_iter().filter(|t| !t.is_empty()).take(MAX_GEAR).collect()
}

fn stem(path: &str) -> String {
    std::path::Path::new(path).file_stem().map_or_else(|| path.to_string(), |n| n.to_string_lossy().to_string())
}

/// The job's gear submittals read again: each panel's box, feed, lugs and drawn conduit areas
/// (the text; for a panel the text gives no box, its elevation drawing read by position), and the
/// pages that name each tag. None when there is no job or no gear.
pub(crate) fn read_docs(s: &crate::Session, sp: &BuildingSpec) -> Option<GearDocs> {
    if !sp.mep.equipment.iter().any(is_gear) {
        return None;
    }
    let tags = gear_tags(sp);
    let job = super::guided::load_job(s).ok().flatten()?;
    let mut docs = GearDocs::default();
    let read_pdf = super::file::io().and_then(|h| h.read_pdf);
    let mut elevations = 0usize;
    for f in super::submittals::pdf_files(&job.submittals).into_iter().take(MAX_FILES_LOOKED) {
        let name = stem(&f);
        let up = name.to_ascii_uppercase();
        if !GEAR_FILE_WORDS.iter().any(|w| up.contains(w)) {
            continue;
        }
        if docs.read.len() >= MAX_FILES {
            docs.unread.push((name, format!("more than {MAX_FILES} gear submittals")));
            continue;
        }
        let bytes = match crate::readfile::read(&f) {
            Ok(b) => b,
            Err(e) => {
                docs.unread.push((name, e.to_string().chars().take(200).collect()));
                continue;
            }
        };
        let text = match super::spec_import::pdf_text(&bytes, &f) {
            Ok(t) => t,
            Err(e) => {
                docs.unread.push((name, e.to_string().chars().take(200).collect()));
                continue;
            }
        };
        for (k, v) in super::gearbuild::submittal_gear(&text, &tags, &name) {
            docs.gear.entry(k.trim().to_ascii_uppercase()).or_insert(v);
        }
        for page in text.split("--- page ").skip(1).take(MAX_PAGES) {
            let n: u32 = page.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
            if n == 0 {
                continue;
            }
            let words: BTreeSet<String> = page
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '#' || c == '-'))
                .filter(|w| !w.is_empty())
                .map(str::to_ascii_uppercase)
                .collect();
            let named: Vec<&String> = tags.iter().filter(|t| words.contains(&t.to_ascii_uppercase())).collect();
            for t in &named {
                let list = docs.pages.entry(t.to_ascii_uppercase()).or_default();
                if list.len() < MAX_PAGES_PER_TAG {
                    list.push(format!("{name} p.{n}"));
                }
            }
            // A gear elevation drawing (`SECTION#` labels) of a panel the text gives no box for.
            let Some(read) = read_pdf else { continue };
            if !words.contains("SECTION#") || elevations >= MAX_ELEVATIONS {
                continue;
            }
            let wanting: Vec<&String> = named.iter().copied().filter(|t| docs.of(t).is_none_or(|g| g.size_in == [0.0; 3])).collect();
            if wanting.is_empty() {
                continue;
            }
            elevations += 1;
            let Ok((d, _)) = read(&bytes, &json!({"page": n, "text": true, "images": false})) else { continue };
            let Some(e) = super::gearbuild::elevation_at(&cadaddy_mepfix::sheet_facts::texts(&d)) else { continue };
            for t in wanting {
                let g = docs.gear.entry(t.to_ascii_uppercase()).or_default();
                g.size_in = e.size_in;
                g.top_plate_in = e.top_plate_in.or(g.top_plate_in);
                g.bottom_plate_in = e.bottom_plate_in.or(g.bottom_plate_in);
                if g.kind.is_empty() {
                    g.kind = e.kind.clone();
                }
                g.source = format!("{name} p.{n} (elevation drawing)");
            }
        }
        docs.read.push(name);
    }
    // An agent's reading of a submittal the job kept as an answer (`gear:<TAG>`).
    for t in &tags {
        let key = format!("gear:{}", t.to_ascii_uppercase());
        if docs.of(t).is_some_and(|g| g.size_in != [0.0; 3]) {
            continue;
        }
        let Some(a) = job.rules.iter().find(|r| r.key == key).and_then(|r| serde_json::from_str::<Value>(&r.value).ok()) else { continue };
        let n = |k: &str| a.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0 && *v < 1_000.0);
        let pair = |k: &str| {
            a.get(k)
                .and_then(Value::as_array)
                .and_then(|v| Some([v.first()?.as_f64()?, v.get(1)?.as_f64()?]))
                .filter(|p| p.iter().all(|x| x.is_finite()))
        };
        if let (Some(h), Some(w), Some(dp)) = (n("height_in"), n("width_in"), n("depth_in")) {
            let g = docs.gear.entry(t.to_ascii_uppercase()).or_default();
            g.size_in = [h, w, dp];
            g.top_plate_in = pair("top_entry_in").or(g.top_plate_in);
            g.bottom_plate_in = pair("bottom_entry_in").or(g.bottom_plate_in);
            if let Some(feed) = a.get("feed").and_then(Value::as_str) {
                g.feed = feed.trim().to_ascii_lowercase().chars().take(20).collect();
            }
            g.source = "an agent's reading of the submittal (job answer)".into();
        }
    }
    Some(docs)
}

// ---------------------------------------------------------------- plan geometry

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn fin(p: [f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

/// A box on the plan: its centre, the unit direction of its width (`u`; its depth is across),
/// half its width and half its depth (feet).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Obb {
    pub c: [f64; 2],
    pub u: [f64; 2],
    pub hw: f64,
    pub hd: f64,
}

impl Obb {
    fn v(&self) -> [f64; 2] {
        [-self.u[1], self.u[0]]
    }
    fn at(&self, s: f64, t: f64) -> [f64; 2] {
        let v = self.v();
        [self.c[0] + self.u[0] * s + v[0] * t, self.c[1] + self.u[1] * s + v[1] * t]
    }
    fn corners(&self) -> [[f64; 2]; 4] {
        [self.at(-self.hw, -self.hd), self.at(self.hw, -self.hd), self.at(self.hw, self.hd), self.at(-self.hw, self.hd)]
    }
    fn local(&self, p: [f64; 2]) -> [f64; 2] {
        let d = sub(p, self.c);
        [dot(d, self.u), dot(d, self.v())]
    }
    fn contains(&self, p: [f64; 2]) -> bool {
        let l = self.local(p);
        l[0].abs() <= self.hw && l[1].abs() <= self.hd
    }
    /// An axis-aligned box.
    fn aligned(lo: [f64; 2], hi: [f64; 2]) -> Option<Obb> {
        let (w, h) = ((hi[0] - lo[0]).abs(), (hi[1] - lo[1]).abs());
        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        if !fin(c) || !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
            return None;
        }
        Some(if w >= h { Obb { c, u: [1.0, 0.0], hw: w / 2.0, hd: h / 2.0 } } else { Obb { c, u: [0.0, 1.0], hw: h / 2.0, hd: w / 2.0 } })
    }
    /// The smallest box round plan points, turned along one of their pairs (a built box's
    /// corners): its width the longer side.
    pub(crate) fn round(pts: &[[f64; 2]]) -> Option<Obb> {
        let pts: Vec<[f64; 2]> = pts.iter().copied().filter(|p| fin(*p)).take(MAX_VERTS).collect();
        let mut best: Option<(f64, Obb)> = None;
        for (i, a) in pts.iter().enumerate() {
            for b in pts.iter().skip(i + 1) {
                let d = sub(*b, *a);
                let len = d[0].hypot(d[1]);
                if len < 1e-9 {
                    continue;
                }
                let u = [d[0] / len, d[1] / len];
                let v = [-u[1], u[0]];
                let (mut s0, mut s1, mut t0, mut t1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
                for p in &pts {
                    let (s, t) = (dot(*p, u), dot(*p, v));
                    s0 = s0.min(s);
                    s1 = s1.max(s);
                    t0 = t0.min(t);
                    t1 = t1.max(t);
                }
                let area = (s1 - s0) * (t1 - t0);
                if !area.is_finite() || best.as_ref().is_some_and(|(a, _)| *a <= area + 1e-12) {
                    continue;
                }
                let (cs, ct) = ((s0 + s1) / 2.0, (t0 + t1) / 2.0);
                let c = [u[0] * cs + v[0] * ct, u[1] * cs + v[1] * ct];
                let o = if s1 - s0 >= t1 - t0 {
                    Obb { c, u, hw: (s1 - s0) / 2.0, hd: (t1 - t0) / 2.0 }
                } else {
                    Obb { c, u: v, hw: (t1 - t0) / 2.0, hd: (s1 - s0) / 2.0 }
                };
                best = Some((area, o));
            }
        }
        best.map(|(_, o)| o).filter(|o| o.hw > 0.0 && o.hd > 0.0)
    }
}

/// Clip segment `a`–`b` (in a frame's local coordinates) to `[s0, s1] × [t0, t1]`: the clipped
/// ends' parameters (Liang–Barsky).
fn clip(a: [f64; 2], b: [f64; 2], s0: f64, s1: f64, t0: f64, t1: f64) -> Option<(f64, f64)> {
    let d = sub(b, a);
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for (p, q) in [(-d[0], a[0] - s0), (d[0], s1 - a[0]), (-d[1], a[1] - t0), (d[1], t1 - a[1])] {
        if p.abs() < 1e-12 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            lo = lo.max(r);
        } else {
            hi = hi.min(r);
        }
        if lo > hi {
            return None;
        }
    }
    Some((lo, hi))
}

/// Do two convex quads overlap (separating axes)?
fn overlap(a: &[[f64; 2]; 4], b: &[[f64; 2]; 4]) -> bool {
    for poly in [a, b] {
        for k in 0..4 {
            let (Some(p), Some(q)) = (poly.get(k), poly.get((k + 1) % 4)) else { continue };
            let axis = [-(q[1] - p[1]), q[0] - p[0]];
            if axis[0].abs() + axis[1].abs() < 1e-12 {
                continue;
            }
            let span =
                |x: &[[f64; 2]; 4]| x.iter().map(|v| dot(*v, axis)).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), d| (lo.min(d), hi.max(d)));
            let (a0, a1) = span(a);
            let (b0, b1) = span(b);
            if a1 <= b0 + 1e-9 || b1 <= a0 + 1e-9 {
                return false;
            }
        }
    }
    true
}

/// Something that blocks working space: a segment with a half thickness (feet) and the heights it
/// takes (inches above the floor; none: floor to ceiling).
#[derive(Clone, Debug)]
pub(crate) struct Obstacle {
    pub what: String,
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub half_ft: f64,
    pub z_in: Option<[f64; 2]>,
}

/// A door's opening on the plan (feet) and what the documents say of its swing.
#[derive(Clone, Debug)]
pub(crate) struct Door {
    pub id: String,
    pub min: [f64; 2],
    pub max: [f64; 2],
    /// The room it swings into, when a door tag says so.
    pub swings_into: Option<String>,
    pub source: Option<String>,
}

/// The working space's verdict and what was measured.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Space {
    pub verdict: &'static str,
    pub clear_in: f64,
    pub depth_in: f64,
    pub width_in: f64,
    pub height_in: f64,
    /// Depth needed for condition 1 and condition 3.
    pub need_in: [f64; 2],
    pub blocked_by: Option<String>,
    pub front: &'static str,
    pub doors: Vec<Value>,
    pub why: String,
}

/// The working space in front of `g` (feet): `need_in` the depths condition 1 and 3 call for (none:
/// the voltage isn't known, so no depth passes), `width_in` its least width, `height_in` its
/// height, against the obstacles and doors of its level; `room` the room it is in (a door that
/// swings into it clashes). The front is the face away from the nearest wall (both faces measured
/// when no wall is behind either; the clearer one is the front).
pub(crate) fn working_space(
    g: &Obb,
    need_in: Option<[f64; 2]>,
    width_in: f64,
    height_in: f64,
    obstacles: &[Obstacle],
    doors: &[Door],
    room: Option<&str>,
) -> Space {
    let width_ft = (width_in / 12.0).max(2.0 * g.hw);
    // The wall it stands on or is set in: the full-height wall along its width nearest its
    // centre, within its depth and BACK_WALL_FT. Its front is the face farther from that wall's
    // centreline; that wall line (every piece of it) is behind or round the gear, never in front.
    let along = |o: &Obstacle| {
        let d = sub(o.b, o.a);
        let len = d[0].hypot(d[1]);
        len > 1e-9 && (d[0] * g.u[1] - d[1] * g.u[0]).abs() / len < 0.1
    };
    let host = obstacles
        .iter()
        .filter(|o| o.z_in.is_none() && along(o))
        .map(|o| (o, super::validate_packet::seg_dist(g.c, o.a, o.b).0 - o.half_ft))
        .filter(|(_, d)| *d <= g.hd + BACK_WALL_FT)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(o, _)| o);
    // The distance of a point to the host wall's line (feet).
    let off_line = |h: &Obstacle, p: [f64; 2]| {
        let d = sub(h.b, h.a);
        let len = d[0].hypot(d[1]).max(1e-12);
        ((p[0] - h.a[0]) * d[1] - (p[1] - h.a[1]) * d[0]).abs() / len
    };
    let on_host_line = |o: &Obstacle| host.is_some_and(|h| along(o) && off_line(h, o.a) <= 1.0 / 12.0 && off_line(h, o.b) <= 1.0 / 12.0);
    let sides: Vec<(f64, &'static str)> = match host {
        Some(h) => {
            let (pos, neg) = (off_line(h, g.at(0.0, g.hd)), off_line(h, g.at(0.0, -g.hd)));
            if pos > neg + 1e-3 {
                vec![(1.0, "the face away from the wall it is on")]
            } else if neg > pos + 1e-3 {
                vec![(-1.0, "the face away from the wall it is on")]
            } else {
                vec![(1.0, "the clearer long face (it sits centred in its wall)"), (-1.0, "the clearer long face (it sits centred in its wall)")]
            }
        }
        None => vec![(1.0, "the clearer long face (no wall behind it)"), (-1.0, "the clearer long face (no wall behind it)")],
    };
    // The clear depth in front of one face over a band of its width.
    let clear = |sign: f64, s0: f64, s1: f64| -> (f64, Option<String>) {
        let mut best = (REACH_FT, None);
        let face = g.at(0.0, sign * g.hd);
        let u = g.u;
        let t_dir = {
            let v = g.v();
            [v[0] * sign, v[1] * sign]
        };
        let local = |p: [f64; 2]| {
            let d = sub(p, face);
            [dot(d, u), dot(d, t_dir)]
        };
        for o in obstacles {
            if o.z_in.is_some_and(|z| z[0] >= height_in || z[1] <= 0.0) || on_host_line(o) {
                continue;
            }
            let (a, b) = (local(o.a), local(o.b));
            let h = o.half_ft.max(0.0);
            let Some((l0, l1)) = clip(a, b, s0 + EDGE_FT - h, s1 - EDGE_FT + h, -h + EDGE_FT, REACH_FT + h) else { continue };
            let t = |k: f64| a[1] + (b[1] - a[1]) * k;
            let near = (t(l0).min(t(l1)) - h).max(0.0);
            if near < best.0 {
                best = (near, Some(o.what.clone()));
            }
        }
        best
    };
    let mut best: Option<(f64, Option<String>, f64, &'static str, f64)> = None;
    for (sign, front) in &sides {
        // A space wider than the gear may sit off its centre: centred, or flush with either end.
        let half = width_ft / 2.0;
        let mut offsets = vec![0.0];
        if half > g.hw + 1e-9 {
            offsets.push(g.hw - half);
            offsets.push(half - g.hw);
        }
        for o in offsets {
            let (c, by) = clear(*sign, o - half, o + half);
            if best.as_ref().is_none_or(|b| c > b.0 + 1e-9) {
                best = Some((c, by, *sign, front, o));
            }
        }
    }
    let (clear_ft, blocked_by, sign, front, off) = best.unwrap_or((0.0, None, 1.0, "", 0.0));
    let clear_in = (clear_ft * 12.0).min(REACH_FT * 12.0);
    // The doors: a swing over the space (as deep as it needs, else what is clear).
    let depth_ft = need_in.map_or(clear_ft, |n| n[1] / 12.0).min(REACH_FT);
    let space = Obb { c: g.at(off, sign * (g.hd + depth_ft / 2.0)), u: g.u, hw: width_ft / 2.0, hd: depth_ft / 2.0 };
    let quad = space.corners();
    let mut door_verdict = "pass";
    let mut door_json = Vec::new();
    for d in doors {
        let Some(op) = Obb::aligned(d.min, d.max) else { continue };
        let leaf = 2.0 * op.hw;
        // Its swing, either side of the opening: the leaf's width out from the opening's line.
        let swing = [
            Obb { c: op.at(0.0, op.hd + leaf / 2.0), u: op.u, hw: op.hw, hd: leaf / 2.0 },
            Obb { c: op.at(0.0, -op.hd - leaf / 2.0), u: op.u, hw: op.hw, hd: leaf / 2.0 },
        ];
        if !swing.iter().any(|sw| overlap(&sw.corners(), &quad)) {
            continue;
        }
        let into_room = match (&d.swings_into, room) {
            (Some(into), Some(r)) => Some(into.trim().eq_ignore_ascii_case(r.trim())),
            (Some(_), None) | (None, _) => None,
        };
        let v = match into_room {
            Some(true) => "fail",
            Some(false) => "pass",
            None => "unsure",
        };
        if v == "fail" || (v == "unsure" && door_verdict == "pass") {
            door_verdict = v;
        }
        door_json.push(json!({
            "door": d.id, "swings_into": d.swings_into, "source": d.source, "verdict": v,
            "why": match v {
                "fail" => "its tag says it swings into this room: its leaf sweeps the working space",
                "pass" => "its tag says it swings into the other room: its leaf stays out of the working space",
                _ => "its leaf would sweep the working space if it swings into this room; nothing says which way it swings",
            },
        }));
    }
    let (depth_verdict, why) = match need_in {
        None if clear_in + 1e-6 < 36.0 => (
            "fail",
            format!(
                "{} in clear in front: short of the least depth any voltage needs (36 in); its voltage to ground isn't in its schedule or record",
                r2(clear_in)
            ),
        ),
        None => ("unsure", "the gear's voltage to ground isn't in its schedule or record: the depth it needs isn't known".to_string()),
        Some([c1, _]) if clear_in + 1e-6 < c1 => ("fail", format!("{} in clear in front, {c1} in needed (condition 1)", r2(clear_in))),
        Some([c1, c3]) if clear_in + 1e-6 < c3 => (
            "unsure",
            format!("{} in clear: enough for condition 1 ({c1} in) but not condition 3 ({c3} in); what faces the gear isn't known", r2(clear_in)),
        ),
        Some(_) => ("pass", format!("{} in clear in front", r2(clear_in))),
    };
    let verdict = if depth_verdict == "fail" || door_verdict == "fail" {
        "fail"
    } else if depth_verdict == "unsure" || door_verdict == "unsure" {
        "unsure"
    } else {
        "pass"
    };
    Space {
        verdict,
        clear_in: r2(clear_in),
        depth_in: r2(depth_ft * 12.0),
        width_in: r2(width_ft * 12.0),
        height_in,
        need_in: need_in.unwrap_or([0.0, 0.0]),
        blocked_by: if clear_in < REACH_FT * 12.0 { blocked_by } else { None },
        front,
        doors: door_json,
        why,
    }
}

// ---------------------------------------------------------------- fill and lugs (recomputed)

/// One side's raceways on its conduit area: do they pack (rows across its width, the largest
/// first), the area they need (each pitch squared) and the area there is (square inches).
pub(crate) fn pack(pitches_in: &[f64], plate_in: [f64; 2]) -> (bool, f64, f64) {
    let (w, d) = (plate_in[0].max(0.0), plate_in[1].max(0.0));
    let mut p: Vec<f64> = pitches_in.iter().copied().filter(|x| x.is_finite() && *x > 0.0).take(MAX_ENDS * 4).collect();
    p.sort_by(|a, b| b.total_cmp(a));
    let need: f64 = p.iter().map(|x| x * x).sum();
    let mut fits = true;
    let (mut used, mut row_w, mut row_d) = (0.0f64, 0.0f64, 0.0f64);
    for x in &p {
        if *x > w || *x > d {
            fits = false;
            break;
        }
        if row_w + x > w + 1e-9 {
            used += row_d;
            row_w = 0.0;
            row_d = 0.0;
        }
        row_w += x;
        row_d = row_d.max(*x);
    }
    used += row_d;
    (fits && used <= d + 1e-9, need, w * d)
}

/// A conductor's area with its insulation (its size's row of the table, else the next size up).
fn wire_area(rules: &super::gear::Rules, c: u64) -> Option<f64> {
    rules.areas.iter().find(|(x, _)| *x >= c).map(|(_, a)| *a)
}

/// The bending space a lug needs for `c` circular mils, `n` per terminal (the table's last column
/// given past its own).
fn bending_need(rules: &super::gear::Rules, c: u64, n: u32, opposite: bool) -> Option<f64> {
    let table = if opposite { &rules.bending_opposite } else { &rules.bending_adjacent };
    let row = table.iter().find(|(x, _)| *x >= c)?;
    let k = usize::try_from(n.clamp(1, 5) - 1).unwrap_or(0);
    row.1.iter().take(k + 1).rev().find(|v| **v > 0.0).copied()
}

fn cap(v: u32) -> u32 {
    v.min(MAX_COUNT)
}

/// Volts to ground of a system as printed: `208Y/120V` → 120, `480Y/277` → 277, `120/240V, 1PH` →
/// 120, `480V` → 480.
fn volts_to_ground(v: &str) -> Option<f64> {
    let u = v.to_ascii_uppercase();
    let first = u.split(',').next().unwrap_or_default();
    let nums: Vec<f64> =
        first.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<f64>().ok()).filter(|n| (100.0..=40_000.0).contains(n)).take(4).collect();
    match nums.as_slice() {
        [_, g, ..] => Some(*g),
        [only] => Some(*only),
        _ => None,
    }
}

/// A piece of gear's voltage to ground and where it says so: its panel schedule's header, else
/// its own record.
fn gear_volts(sp: &BuildingSpec, e: &MepEquipment) -> Option<(f64, String)> {
    // Its own schedule, else the typical schedule its record names (`PANEL`: one schedule for
    // every unit's panel).
    let typical = e.properties.iter().find(|(k, _)| k.eq_ignore_ascii_case("PANEL")).map(|(_, v)| v.trim());
    let schedule = sp
        .mep
        .panel_schedules
        .iter()
        .find(|p| p.panel.trim().eq_ignore_ascii_case(e.tag.trim()))
        .or_else(|| typical.and_then(|t| sp.mep.panel_schedules.iter().find(|p| !t.is_empty() && p.panel.trim().eq_ignore_ascii_case(t))));
    if let Some(p) = schedule {
        for (k, v) in p.header.iter().take(100) {
            if k.to_ascii_lowercase().contains("volt")
                && let Some(g) = volts_to_ground(v)
            {
                return Some((g, format!("panel {} schedule on {} ({k}: {})", p.panel, p.source, quote(v))));
            }
        }
    }
    for (k, v) in e.properties.iter().take(200) {
        if k.to_ascii_uppercase().contains("VOLT")
            && let Some(g) = volts_to_ground(v)
        {
            return Some((g, format!("its record ({k}: {})", quote(v))));
        }
    }
    None
}

fn quote(t: &str) -> String {
    let q: String = t.chars().take(QUOTE_CHARS).collect();
    if t.chars().count() > QUOTE_CHARS { format!("{q}…") } else { q }
}

/// `{h} H x {w} W x {d} D in` (the build's record of the submittal's box).
fn box_prop(v: &str) -> Option<[f64; 3]> {
    let mut out = [0.0; 3];
    for part in v.split(['x', 'X']).take(3) {
        let p = part.trim();
        let n: String = p.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let x: f64 = n.parse().ok().filter(|x: &f64| x.is_finite() && *x > 0.0 && *x < 1_000.0)?;
        let u = p.to_ascii_uppercase();
        let slot = if u.contains('H') {
            0
        } else if u.contains('W') {
            1
        } else if u.contains('D') {
            2
        } else {
            return None;
        };
        if let Some(o) = out.get_mut(slot) {
            *o = x;
        }
    }
    out.iter().all(|x| *x > 0.0).then_some(out)
}

/// The box the build recorded from the submittal (`BOX`; a `SIZE` record counts only when its
/// source is a submittal), and any other size record with its source (shown, never compared).
fn recorded_box(e: &MepEquipment) -> (Option<[f64; 3]>, Option<Value>) {
    if let Some(b) = e.properties.get("BOX").and_then(|b| box_prop(b)) {
        return (Some(b), None);
    }
    let Some(v) = e.properties.get("SIZE") else { return (None, None) };
    let from = e.sources.get("size").cloned().unwrap_or_default();
    let low = from.to_ascii_lowercase();
    match box_prop(v) {
        Some(b) if low.contains("submittal") || low.contains(".pdf") => (Some(b), None),
        parsed => (None, Some(json!({"size": v, "in": parsed, "from": if from.is_empty() { Value::Null } else { json!(from) }}))),
    }
}

// ---------------------------------------------------------------- the check

pub(crate) struct Ctx<'a> {
    pub d: &'a Drawing,
    pub sp: &'a BuildingSpec,
    pub model: &'a Model,
    pub frame: &'a Frame,
    pub areas: &'a [Area],
    pub whole_levels: bool,
    pub docs: Option<&'a GearDocs>,
}

/// What the gear rules found.
#[derive(Default)]
pub(crate) struct Checked {
    /// (rank, rule, verdict, finding).
    pub findings: Vec<(u8, &'static str, &'static str, Value)>,
    /// Per rule: why → gear not measured.
    pub unmeasured: BTreeMap<String, BTreeMap<String, u64>>,
    pub gear: usize,
}

/// A piece of gear as the model has it: its box on its level's plan (feet), its bottom and top
/// (inches above its floor), its built element.
pub(crate) struct Placed<'a> {
    pub e: &'a MepEquipment,
    pub level: String,
    pub obb: Obb,
    pub z_in: [f64; 2],
    pub handle: Option<String>,
    /// `built` (its 3D box) or `spec` (its record's plan box: nothing was built).
    pub from: &'static str,
}

/// The gear's built boxes: tag (upper case) → (handle, plan corners in feet, bottom and top
/// inches above the floor).
fn built_boxes(d: &Drawing, frame: &Frame) -> BTreeMap<(String, String), (String, Vec<[f64; 2]>, [f64; 2])> {
    let upf = frame.upf();
    let mut out = BTreeMap::new();
    for e in d.model.iter().take(super::validate::MAX_ENTITIES) {
        if super::bimlook::made_tag(e, "kind") != Some("equipment") {
            continue;
        }
        let Some(tag) = super::bimlook::made_tag(e, "tag").map(|t| t.trim().to_ascii_uppercase()).filter(|t| !t.is_empty()) else { continue };
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let Some(level) = super::bspec::made_level(e).filter(|l| !l.trim().is_empty()) else { continue };
        let floor = frame.floor_z(&level).unwrap_or(0.0);
        let pts: Vec<[f64; 2]> = m.vertices.iter().take(MAX_VERTS).map(|v| frame.built_on(&level, e, Vec2::new(v.x, v.y))).collect();
        let (z0, z1) = m.vertices.iter().take(MAX_VERTS).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v.z), b.max(v.z)));
        if !(z0.is_finite() && z1.is_finite()) || upf <= 0.0 {
            continue;
        }
        let z = [(z0 - floor) / upf * 12.0, (z1 - floor) / upf * 12.0];
        out.entry((level.to_ascii_lowercase(), tag)).or_insert((e.handle.hex(), pts, z));
    }
    out
}

/// The gear of the spec as the model has it (its built box, else its record's plan box).
pub(crate) fn placed<'a>(d: &Drawing, sp: &'a BuildingSpec, frame: &Frame) -> (Vec<Placed<'a>>, Vec<(String, String)>) {
    let built = built_boxes(d, frame);
    let mut out = Vec::new();
    let mut not = Vec::new();
    for e in sp.mep.equipment.iter().filter(|e| is_gear(e)).take(MAX_GEAR) {
        let key = (e.level.to_ascii_lowercase(), e.tag.trim().to_ascii_uppercase());
        let from_built = built.get(&key).and_then(|(h, pts, z)| Obb::round(pts).map(|o| (h.clone(), o, *z)));
        match from_built {
            Some((h, obb, z)) => out.push(Placed { e, level: e.level.clone(), obb, z_in: z, handle: Some(h), from: "built" }),
            None => {
                let lo = frame.to_ft_on(&e.level, Vec2::new(e.min[0], e.min[1]));
                let hi = frame.to_ft_on(&e.level, Vec2::new(e.max[0], e.max[1]));
                let z = [e.bottom_ft * 12.0, (e.bottom_ft + e.height_ft) * 12.0];
                match Obb::aligned(lo, hi).filter(|_| z.iter().all(|v| v.is_finite())) {
                    Some(obb) => out.push(Placed { e, level: e.level.clone(), obb, z_in: z, handle: None, from: "spec" }),
                    None => not.push((e.tag.clone(), "no plan box in the model".to_string())),
                }
            }
        }
    }
    (out, not)
}

/// The area a point of a level is in, and whether the scope takes it.
fn area_for<'a>(areas: &'a [Area], whole_levels: bool, level: &str, at: [f64; 2]) -> Option<Option<&'a Area>> {
    let mine: Vec<&Area> = areas.iter().filter(|a| a.level.eq_ignore_ascii_case(level)).collect();
    if mine.is_empty() {
        return None;
    }
    let a = mine.iter().copied().find(|a| inside(a.bbox, at) && (a.outline.len() < 3 || point_in(&a.outline, at)));
    if a.is_some() || whole_levels { Some(a) } else { None }
}

fn tol(r: &Effective, name: &str, default: f64) -> f64 {
    r.rule.tolerance.get(name).copied().filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(default)
}

fn rank(verdict: &str, severity: &str) -> u8 {
    match verdict {
        "pass" => 9,
        "unsure" => 5,
        "conflict" => 3,
        _ => match severity {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        },
    }
}

/// The walls, other equipment and door openings of each level (feet), for the working space.
fn level_things(cx: &Ctx, gear: &[Placed]) -> (BTreeMap<String, Vec<Obstacle>>, BTreeMap<String, Vec<Door>>) {
    let mut obs: BTreeMap<String, Vec<Obstacle>> = BTreeMap::new();
    for w in cx.model.walls.iter().take(MAX_OBSTACLES) {
        obs.entry(w.level.to_ascii_lowercase()).or_default().push(Obstacle {
            what: format!("wall {}", w.id),
            a: w.a,
            b: w.b,
            half_ft: (w.thickness_in / 24.0).max(0.0),
            z_in: None,
        });
    }
    for s in cx.model.wall_spans.iter().take(MAX_OBSTACLES) {
        obs.entry(s.wall.level.to_ascii_lowercase()).or_default().push(Obstacle {
            what: format!("wall {} over an opening", s.wall.id),
            a: s.wall.a,
            b: s.wall.b,
            half_ft: (s.wall.thickness_in / 24.0).max(0.0),
            z_in: Some([s.bottom_in, s.top_in]),
        });
    }
    // Every other piece of equipment: its box's sides.
    let mut boxes: Vec<(String, String, Obb, [f64; 2])> = gear.iter().map(|g| (g.level.clone(), g.e.tag.clone(), g.obb, g.z_in)).collect();
    for e in cx.sp.mep.equipment.iter().filter(|e| !is_gear(e)).take(MAX_GEAR) {
        let lo = cx.frame.to_ft_on(&e.level, Vec2::new(e.min[0], e.min[1]));
        let hi = cx.frame.to_ft_on(&e.level, Vec2::new(e.max[0], e.max[1]));
        if let Some(o) = Obb::aligned(lo, hi) {
            boxes.push((e.level.clone(), e.tag.clone(), o, [e.bottom_ft * 12.0, (e.bottom_ft + e.height_ft) * 12.0]));
        }
    }
    for (level, tag, o, z) in boxes {
        let c = o.corners();
        let list = obs.entry(level.to_ascii_lowercase()).or_default();
        for k in 0..4 {
            if let (Some(a), Some(b)) = (c.get(k), c.get((k + 1) % 4)) {
                list.push(Obstacle { what: format!("equipment {tag}"), a: *a, b: *b, half_ft: 0.0, z_in: Some(z) });
            }
        }
    }
    let mut doors: BTreeMap<String, Vec<Door>> = BTreeMap::new();
    let tags: Vec<(Option<String>, [f64; 2], &cadaddy_walk3d::OpeningTag)> = cx
        .sp
        .schedule
        .tags
        .iter()
        .take(MAX_OBSTACLES)
        .filter(|t| t.swings_into.is_some())
        .map(|t| {
            let lv = t.level.clone();
            let at = match &lv {
                Some(l) => cx.frame.to_ft_on(l, Vec2::new(t.at[0], t.at[1])),
                None => cx.frame.to_ft(Vec2::new(t.at[0], t.at[1])).1,
            };
            (lv, at, t)
        })
        .collect();
    for o in cx.model.openings.iter().filter(|o| o.kind == "door").take(MAX_OBSTACLES) {
        let c = [(o.min[0] + o.max[0]) / 2.0, (o.min[1] + o.max[1]) / 2.0];
        let reach = (o.max[0] - o.min[0]).abs().max((o.max[1] - o.min[1]).abs()).max(3.0);
        let tag = tags
            .iter()
            .filter(|(l, _, _)| l.as_ref().is_none_or(|l| l.eq_ignore_ascii_case(&o.level)))
            .map(|(_, at, t)| ((at[0] - c[0]).hypot(at[1] - c[1]), *t))
            .filter(|(d, _)| *d <= reach)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, t)| t);
        doors.entry(o.level.to_ascii_lowercase()).or_default().push(Door {
            id: o.id.clone(),
            min: o.min,
            max: o.max,
            swings_into: tag.and_then(|t| t.swings_into.clone()),
            source: tag.map(|t| format!("door tag {}{}", t.mark, t.source.as_ref().map(|s| format!(" on {s}")).unwrap_or_default())),
        });
    }
    (obs, doors)
}

/// The ceiling height over a point of a level (feet): the reflected ceiling plan's there, else the
/// level's ceiling.
fn ceiling_at(cx: &Ctx, level: &str, at: [f64; 2]) -> Option<(f64, String)> {
    for c in cx.sp.rcp.ceilings.iter().filter(|c| c.level.eq_ignore_ascii_case(level) && c.outline.len() >= 3).take(MAX_OBSTACLES) {
        let o: Vec<[f64; 2]> = c.outline.iter().take(10_000).map(|p| cx.frame.to_ft_on(level, Vec2::new(p[0], p[1]))).collect();
        if c.height_ft.is_finite() && c.height_ft > 0.0 && point_in(&o, at) {
            return Some((c.height_ft, format!("the reflected ceiling plan{}", c.room.as_ref().map(|r| format!(" (room {r})")).unwrap_or_default())));
        }
    }
    cx.sp.level(level).map(|l| l.ceiling_ft).filter(|h| h.is_finite() && *h > 0.0).map(|h| (h, format!("level {level}'s ceiling")))
}

/// Check every piece of gear in scope (E-GEAR-SPACE, E-GEAR-FILL, E-GEAR-SIZE).
pub(crate) fn check(cx: &Ctx, space: Option<&Effective>, fill: Option<&Effective>, size: Option<&Effective>) -> Checked {
    let mut out = Checked::default();
    if space.is_none() && fill.is_none() && size.is_none() {
        return out;
    }
    let (gear, not) = placed(cx.d, cx.sp, cx.frame);
    for (_, why) in &not {
        for id in [space.map(|_| SPACE), fill.map(|_| FILL), size.map(|_| SIZE)].into_iter().flatten() {
            *out.unmeasured.entry(id.to_string()).or_default().entry(why.clone()).or_default() += 1;
        }
    }
    let (obstacles, doors) = if space.is_some() { level_things(cx, &gear) } else { (BTreeMap::new(), BTreeMap::new()) };
    let code = super::coderules::rules().into_iter().find(|r| r.id == "E-WORKSPACE").unwrap_or_default();
    let tables = super::gear::Rules::of(cx.sp);
    let systems: BTreeMap<String, MepKind> = cx.sp.mep.systems.iter().map(|s| (s.name.to_ascii_lowercase(), s.kind)).collect();
    for g in &gear {
        let at = g.obb.c;
        let Some(area) = area_for(cx.areas, cx.whole_levels, &g.level, at) else { continue };
        out.gear += 1;
        let tag = g.e.tag.trim();
        let status = g.e.sources.get("status").cloned().unwrap_or_default();
        let position = g.e.sources.get("position").cloned();
        let proposed = status.eq_ignore_ascii_case("proposed");
        let base = json!({
            "gear": tag, "gear_kind": g.e.kind, "model_from": g.from, "position": position, "position_status": if status.is_empty() { Value::Null } else { json!(status) },
        });
        let mut push = |rule: &'static str,
                        r: &Effective,
                        check: &str,
                        verdict: &'static str,
                        issue: String,
                        expected: String,
                        found: String,
                        measured: Value| {
            let mut m = base.clone();
            m["check"] = json!(check);
            if let (Some(o), Some(extra)) = (m.as_object_mut(), measured.as_object()) {
                for (k, v) in extra {
                    o.insert(k.clone(), v.clone());
                }
            }
            let issue = if proposed && rule == SPACE { format!("{issue} (CADaddy's proposed position: the plans don't locate it)") } else { issue };
            let id = format!("PS-{}", super::validate_run::hash8(&format!("{rule}|{}|{tag}|{check}|{verdict}", g.level)));
            out.findings.push((
                rank(verdict, &r.rule.severity),
                rule,
                verdict,
                json!({
                    "id": id, "rule": rule, "handle": g.handle, "kind": g.e.kind, "tag": tag, "verdict": verdict,
                    "issue": issue, "expected": expected, "found": found, "measured": m, "evidence": [],
                    "confidence": match verdict { "unsure" => 0.5, _ if proposed || g.from == "spec" => 0.8, _ => 1.0 },
                    "severity": r.rule.severity, "area": area.map(|a| a.id.clone()), "level": g.level, "at": pt([r2(at[0]), r2(at[1])]),
                    "source": "prescreen",
                }),
            ));
        };
        // ---- E-GEAR-SPACE
        if let Some(r) = space {
            let volts = gear_volts(cx.sp, g.e);
            let need = volts.as_ref().map(|(v, _)| {
                let row = code.depth_in.iter().find(|row| *v <= row[0]).or(code.depth_in.last()).copied().unwrap_or([600.0, 36.0, 42.0, 48.0]);
                [row[1], row[3].max(row[1])]
            });
            let height_in = code.height_in.unwrap_or(78.0).max(g.z_in[1]);
            let lv = g.level.to_ascii_lowercase();
            let mine: Vec<Obstacle> = obstacles
                .get(&lv)
                .map(|v| v.iter().filter(|o| !o.what.eq_ignore_ascii_case(&format!("equipment {tag}"))).cloned().collect())
                .unwrap_or_default();
            let room = area.and_then(|a| a.number.clone());
            let ws = working_space(
                &g.obb,
                need,
                code.width_in.unwrap_or(30.0),
                height_in,
                &mine,
                doors.get(&lv).map_or(&[][..], Vec::as_slice),
                room.as_deref(),
            );
            let expected = match need {
                Some([c1, c3]) => format!(
                    "{} in deep ({c1} in for condition 1, {c3} for condition 3 at {} V to ground), {} in wide and {} in high clear in front ({}), no door swinging into it",
                    ws.need_in[1],
                    volts.as_ref().map_or(0.0, |v| v.0),
                    ws.width_in,
                    r2(height_in),
                    code.cite
                ),
                None => {
                    format!("the depth its voltage calls for, {} in wide and {} in high clear in front ({})", ws.width_in, r2(height_in), code.cite)
                }
            };
            let found = format!(
                "{} in clear{}{}",
                ws.clear_in,
                ws.blocked_by.as_ref().map(|b| format!(" to {b}")).unwrap_or_default(),
                if ws.doors.is_empty() { String::new() } else { format!("; {} door swing(s) over it", ws.doors.len()) }
            );
            let issue = match ws.verdict {
                "pass" => format!("{tag}'s working space is clear"),
                "fail" if ws.doors.iter().any(|d| d["verdict"] == "fail") && ws.clear_in + 1e-6 >= ws.need_in[0] => {
                    format!("a door swings into {tag}'s working space")
                }
                "fail" => format!("{tag}'s working space is short: {}", ws.why),
                _ => format!(
                    "{tag}'s working space can't be confirmed: {}",
                    if ws.doors.iter().any(|d| d["verdict"] == "unsure") { "a door may swing into it".to_string() } else { ws.why.clone() }
                ),
            };
            push(
                SPACE,
                r,
                "working space",
                ws.verdict,
                issue,
                expected,
                found,
                json!({
                    "clear_in": ws.clear_in, "need_in": {"condition_1": need.map(|n| n[0]), "condition_3": need.map(|n| n[1])},
                    "width_in": ws.width_in, "height_in": ws.height_in, "blocked_by": ws.blocked_by, "front": ws.front, "doors": ws.doors,
                    "volts_to_ground": volts.as_ref().map(|v| v.0), "volts_from": volts.as_ref().map(|v| v.1.clone()), "code": code.cite,
                    "gear_in": {"width": r2(g.obb.hw * 24.0), "depth": r2(g.obb.hd * 24.0), "bottom": r2(g.z_in[0]), "top": r2(g.z_in[1])},
                }),
            );
            // Headroom.
            let ceiling = ceiling_at(cx, &g.level, at);
            let (v, issue, found) = match &ceiling {
                Some((h, _)) if h * 12.0 + 1e-6 < height_in => {
                    ("fail", format!("the ceiling over {tag} is lower than its working space's height"), format!("{} in ceiling", r2(h * 12.0)))
                }
                Some((h, _)) => ("pass", format!("{tag} has its headroom"), format!("{} in ceiling", r2(h * 12.0))),
                None => ("unsure", format!("the ceiling height over {tag} isn't known"), "no ceiling height".to_string()),
            };
            push(
                SPACE,
                r,
                "headroom",
                v,
                issue,
                format!("{} in clear to the ceiling (6.5 ft or the gear's top, {})", r2(height_in), code.cite),
                found,
                json!({"ceiling_in": ceiling.as_ref().map(|c| r2(c.0 * 12.0)), "ceiling_from": ceiling.map(|c| c.1), "need_in": r2(height_in)}),
            );
            // Dedicated space: no duct or pipe over the footprint to 6 ft above the gear.
            let level_top = cx.sp.level(&g.level).map(|l| l.height_ft * 12.0).filter(|h| h.is_finite() && *h > 0.0);
            let top = level_top.map_or(g.z_in[1] + 72.0, |h| h.min(g.z_in[1] + 72.0));
            let mut through: Vec<Value> = Vec::new();
            let mut unknown: Vec<Value> = Vec::new();
            for (i, run) in cx.sp.mep.runs.iter().enumerate().take(MAX_OBSTACLES) {
                if !run.level.eq_ignore_ascii_case(&g.level) {
                    continue;
                }
                let kind = systems.get(&run.system.to_ascii_lowercase()).copied();
                if !matches!(kind, Some(MepKind::Duct | MepKind::Pipe)) {
                    continue;
                }
                let path: Vec<[f64; 2]> = run.points.iter().take(10_000).map(|p| cx.frame.to_ft_on(&run.level, Vec2::new(p[0], p[1]))).collect();
                let crosses = match path.as_slice() {
                    [one] => g.obb.contains(*one),
                    more => more.windows(2).any(|w| match w {
                        [a, b] => {
                            let (la, lb) = (g.obb.local(*a), g.obb.local(*b));
                            clip(la, lb, -g.obb.hw, g.obb.hw, -g.obb.hd, g.obb.hd).is_some()
                        }
                        _ => false,
                    }),
                };
                if !crosses {
                    continue;
                }
                let bottom = run.bottoms_ft.iter().copied().filter(|b| b.is_finite()).fold(None, |m: Option<f64>, b| Some(m.map_or(b, |m| m.min(b))));
                let id = if run.id.is_empty() { format!("mep.runs[{i}]") } else { run.id.clone() };
                let j = json!({"run": id, "system": run.system, "bottom_in": bottom.map(|b| r2(b * 12.0))});
                match bottom {
                    Some(b) if b * 12.0 < top => through.push(j),
                    Some(_) => {}
                    None => unknown.push(j),
                }
                if through.len() + unknown.len() >= 20 {
                    break;
                }
            }
            let (v, issue) = if !through.is_empty() {
                ("fail", format!("a duct or pipe passes through {tag}'s dedicated space"))
            } else if !unknown.is_empty() {
                ("unsure", format!("a duct or pipe crosses over {tag} at a height nothing gives"))
            } else {
                ("pass", format!("{tag}'s dedicated space is free of ducts and pipes"))
            };
            push(
                SPACE,
                r,
                "dedicated space",
                v,
                issue,
                format!("no duct or pipe over its footprint from the floor to {} in (6 ft over it, NEC 110.26(E))", r2(top)),
                format!("{} through it, {} at an unknown height", through.len(), unknown.len()),
                json!({"zone_top_in": r2(top), "through": through, "unknown_height": unknown}),
            );
        }
        // ---- E-GEAR-SIZE
        if let Some(r) = size {
            let t = tol(r, "size_in", DEFAULT_SIZE_IN);
            let model = [r2(g.z_in[1] - g.z_in[0]), r2(g.obb.hw * 24.0), r2(g.obb.hd * 24.0)];
            let reread = cx.docs.and_then(|d| d.of(tag)).filter(|s| s.size_in.iter().all(|v| *v > 0.0));
            let (recorded, other_size) = recorded_box(g.e);
            let recorded_from = g.e.sources.get("size").cloned();
            let (want, from): (Option<[f64; 3]>, String) = match (reread, recorded) {
                (Some(s), _) => (Some(s.size_in), format!("the submittal, re-read: {}", s.source)),
                (None, Some(b)) => {
                    (Some(b), format!("the build's record of the submittal ({})", recorded_from.clone().unwrap_or_else(|| "no source given".into())))
                }
                (None, None) => (None, String::new()),
            };
            let words = |s: [f64; 3]| format!("{} H x {} W x {} D in", s[0], s[1], s[2]);
            let (v, issue, expected) = match want {
                None => ("unsure", format!("no submittal gives {tag}'s box"), "the enclosure the gear submittal gives".to_string()),
                Some(w) => {
                    let off: Vec<String> = ["height", "width", "depth"]
                        .iter()
                        .zip(w.iter().zip(model.iter()))
                        .filter(|(_, (a, b))| (*a - *b).abs() > t)
                        .map(|(n, (a, b))| format!("{n} {b} in, the submittal's {a} in"))
                        .collect();
                    if off.is_empty() {
                        ("pass", format!("{tag}'s box is the submittal's"), words(w))
                    } else {
                        ("fail", format!("{tag}'s box isn't the submittal's: {}", off.join("; ")), words(w))
                    }
                }
            };
            let differs = match (reread, recorded) {
                (Some(s), Some(b)) => s.size_in.iter().zip(b.iter()).any(|(a, b)| (a - b).abs() > t),
                _ => false,
            };
            push(
                SIZE,
                r,
                "enclosure size",
                v,
                issue,
                expected,
                words(model),
                json!({
                    "model_in": {"height": model[0], "width": model[1], "depth": model[2]}, "submittal_in": want, "submittal_from": from,
                    "recorded_box": g.e.properties.get("BOX"), "other_size_record": other_size, "recorded_disagrees_with_submittal": differs, "tolerance_in": t,
                }),
            );
        }
        // ---- E-GEAR-FILL
        if let Some(r) = fill {
            fill_checks(cx, g, &tables, &code, &mut |check, verdict, issue, expected, found, measured| {
                push(FILL, r, check, verdict, issue, expected, found, measured)
            });
        }
    }
    carried(cx, space, fill, size, &mut out);
    out
}

/// One feeder end at a piece of gear: the feeder and whether it lands here (its load end).
type End<'a> = (&'a Feeder, bool);

/// E-GEAR-FILL's checks of one piece of gear, each through `emit(check, verdict, issue, expected,
/// found, measured)`.
#[allow(clippy::type_complexity)]
fn fill_checks(
    cx: &Ctx,
    g: &Placed,
    tables: &super::gear::Rules,
    code: &super::coderules::CodeRule,
    emit: &mut dyn FnMut(&str, &'static str, String, String, String, Value),
) {
    let tag = g.e.tag.trim();
    let doc = cx.docs.and_then(|d| d.of(tag));
    let ends: Vec<End> = cx
        .sp
        .mep
        .feeders
        .iter()
        .filter_map(|f| {
            if f.to.trim().eq_ignore_ascii_case(tag) {
                Some((f, true))
            } else if f.from.trim().eq_ignore_ascii_case(tag) {
                Some((f, false))
            } else {
                None
            }
        })
        .take(MAX_ENDS)
        .collect();
    let feed = doc.map(|d| d.feed.trim().to_ascii_lowercase()).filter(|f| !f.is_empty());
    // The feeders the gear's own schedule names (rows naming another panel), against the feeder
    // records that give their conduits and conductors.
    if let Some(ps) = cx.sp.mep.panel_schedules.iter().find(|p| p.panel.trim().eq_ignore_ascii_case(tag)) {
        let panels: Vec<&str> = cx
            .sp
            .mep
            .panel_schedules
            .iter()
            .map(|p| p.panel.trim())
            .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case(tag))
            .take(MAX_GEAR)
            .collect();
        let mut named: BTreeMap<String, String> = BTreeMap::new();
        for c in ps.circuits.iter().take(2_000) {
            let u = c.description.to_ascii_uppercase();
            let words: Vec<&str> = u.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-').filter(|w| !w.is_empty()).collect();
            for p in &panels {
                let pu = p.to_ascii_uppercase();
                if words.contains(&pu.as_str()) && (u.trim() == pu || u.contains("PANEL") || u.contains("PNL") || words.first() == Some(&pu.as_str()))
                {
                    named.entry(pu).or_insert_with(|| format!("circuit {}", c.number));
                }
            }
        }
        if !named.is_empty() {
            let fed: BTreeSet<String> = ends.iter().filter(|(_, incoming)| !incoming).map(|(f, _)| f.to.trim().to_ascii_uppercase()).collect();
            let missing: Vec<String> = named.iter().filter(|(p, _)| !fed.contains(*p)).map(|(p, c)| format!("{p} ({c})")).collect();
            let (v, issue) = if missing.is_empty() {
                ("pass", format!("every feeder {tag}'s schedule names has its conduits and conductors"))
            } else {
                (
                    "unsure",
                    format!(
                        "{} feeder(s) {tag}'s schedule names have no conduits or conductors on record: its fill can't be counted whole",
                        missing.len()
                    ),
                )
            };
            emit(
                "feeders",
                v,
                issue,
                format!("a feeder record (conduits, conductors) for each of the {} panels {tag}'s schedule ({}) feeds", named.len(), ps.source),
                format!("{} of {}", named.len() - missing.len(), named.len()),
                json!({"schedule": ps.source, "named": named, "without_record": missing}),
            );
        }
    }
    if ends.is_empty() {
        emit(
            "raceways",
            "unsure",
            format!("no feeder record lands on {tag}: its raceways and conductors can't be counted"),
            "the feeders the schedules and one-line give, with conduits and conductors".into(),
            "none".into(),
            json!({}),
        );
        return;
    }
    // ---- Raceways through each side.
    let mut sides: BTreeMap<&'static str, (Vec<f64>, Vec<Value>, bool)> = BTreeMap::new();
    let mut unknown_size: Vec<String> = Vec::new();
    for (f, incoming) in &ends {
        let given = if *incoming { f.entry.trim().to_ascii_lowercase() } else { f.from_entry.trim().to_ascii_lowercase() };
        let (side, assumed): (&'static str, bool) = match given.as_str() {
            "top" => ("top", false),
            "bottom" => ("bottom", false),
            _ => match feed.as_deref() {
                Some("bottom") => ("bottom", true),
                _ => ("top", true),
            },
        };
        if !(f.conduit_in.is_finite() && f.conduit_in > 0.0) {
            unknown_size.push(f.tag.clone());
            continue;
        }
        let n = cap(f.conduits.max(if f.conduits == 0 { f.sets.max(1) } else { 0 }));
        let e = sides.entry(side).or_default();
        for _ in 0..n.min(MAX_ENDS as u32) {
            e.0.push(tables.od(f.conduit_in) + tables.spacing_in);
        }
        e.1.push(json!({"feeder": f.tag, "from": f.from, "to": f.to, "conduits": n, "trade_in": f.conduit_in, "side": side, "side_assumed": assumed, "source": f.source}));
        e.2 |= assumed;
    }
    let box_in = doc.map(|d| d.size_in).filter(|s| s.iter().all(|v| *v > 0.0)).or_else(|| recorded_box(g.e).0);
    let margin = code.plate_margin_in.unwrap_or([2.0, 1.5]);
    for (side, (pitches, feeders, assumed)) in &sides {
        let drawn = doc
            .and_then(|d| if *side == "top" { d.top_plate_in } else { d.bottom_plate_in })
            .filter(|p| p.iter().all(|v| v.is_finite() && *v >= 0.0));
        let recorded_drawn =
            g.e.properties
                .get(&format!("{} CONDUIT AREA", side.to_ascii_uppercase()))
                .is_some_and(|v| v.contains("drawn"))
                .then(|| g.e.enclosure.as_ref().map(|e| if *side == "top" { e.top_entry_in } else { e.bottom_entry_in }))
                .flatten();
        let (plate, plate_from): (Option<[f64; 2]>, String) = match (drawn, recorded_drawn, box_in) {
            (Some(p), _, _) => (Some(p), format!("drawn in the submittal ({})", doc.map(|d| d.source.clone()).unwrap_or_default())),
            (None, Some(p), _) => (Some(p), "drawn in the submittal, as the build recorded it".into()),
            (None, None, Some(b)) => (
                Some([(b[1] - 2.0 * margin[0]).max(0.0), (b[2] - margin[1]).max(0.0)]),
                format!("derived: the box's {} W x {} D in less {} x {} in margins (coderules {})", b[1], b[2], margin[0], margin[1], code.id),
            ),
            (None, None, None) => (None, String::new()),
        };
        let check = format!("raceways {side}");
        let Some(plate) = plate else {
            emit(
                &check,
                "unsure",
                format!("{tag}'s {side} conduit area isn't known (no submittal box or drawn area)"),
                format!("{} raceways through the {side}", pitches.len()),
                "no conduit area".into(),
                json!({"raceways": pitches.len(), "feeders": feeders}),
            );
            continue;
        };
        let (fits, need, have) = pack(pitches, plate);
        let derived = plate_from.starts_with("derived");
        let v = if !fits {
            "fail"
        } else if (derived && need > have / 2.0) || *assumed {
            "unsure"
        } else {
            "pass"
        };
        let issue = match v {
            "fail" => {
                format!("{} raceways don't fit {tag}'s {side}: about {} sq in needed, {} sq in there", pitches.len(), need.round(), have.round())
            }
            "pass" => format!("{} raceways fit {tag}'s {side}", pitches.len()),
            _ if *assumed => format!("{} raceways fit {tag}'s {side}, but which side some come in isn't given", pitches.len()),
            _ => format!(
                "{} raceways fit {tag}'s {side} only on an area the submittal doesn't draw, with less than half of it to spare",
                pitches.len()
            ),
        };
        emit(
            &check,
            v,
            issue,
            format!("{} raceways (each its outside diameter and {} in round it) packed on the {side} conduit area", pitches.len(), tables.spacing_in),
            format!("{} x {} in, {} sq in ({plate_from})", r2(plate[0]), r2(plate[1]), have.round()),
            json!({"raceways": pitches.len(), "need_sq_in": r2(need), "plate_in": [r2(plate[0]), r2(plate[1])], "plate_sq_in": r2(have), "plate_from": plate_from, "feeders": feeders}),
        );
    }
    if !unknown_size.is_empty() {
        emit(
            "raceway sizes",
            "unsure",
            format!("{} feeder(s) at {tag} have no conduit size: their raceways can't be placed", unknown_size.len()),
            "each feeder's conduit trade size (its feeder schedule row)".into(),
            unknown_size.join(", "),
            json!({"feeders": unknown_size}),
        );
    }
    // ---- Gutter fill (NEC 312.8).
    let mut conductors: Vec<Value> = Vec::new();
    let mut fill_sq = 0.0f64;
    let mut unread: Vec<String> = Vec::new();
    for (f, _) in &ends {
        let sets = cap(f.sets.max(1));
        let parts = [
            (cap(f.phases).saturating_mul(sets), f.size.as_str()),
            (cap(f.neutrals).saturating_mul(sets), if f.neutral_size.trim().is_empty() { f.size.as_str() } else { f.neutral_size.as_str() }),
            (cap(f.grounds).saturating_mul(cap(if f.conduits > 0 { f.conduits } else { sets })), f.ground_size.as_str()),
        ];
        for (n, size) in parts {
            if n == 0 {
                continue;
            }
            match super::gear::cmil(size).and_then(|c| wire_area(tables, c)) {
                Some(a) => fill_sq += a * f64::from(n),
                None => unread.push(format!("{} {size}", f.tag)),
            }
        }
        if f.phases == 0 {
            unread.push(format!("{} (no phase count)", f.tag));
        }
        conductors.push(json!({"feeder": f.tag, "sets": sets, "phases": f.phases, "size": f.size, "neutrals": f.neutrals, "grounds": f.grounds, "ground_size": f.ground_size}));
    }
    let gutter = g.e.enclosure.as_ref().map(|e| e.gutter_in).filter(|g| g.iter().all(|v| v.is_finite() && *v > 0.0));
    let (v, issue, found) = match gutter {
        None => (
            "unsure",
            format!("{tag}'s wiring gutter isn't in the submittal: missing, so its fill can't be checked"),
            "no gutter dimensions".to_string(),
        ),
        Some(gi) => {
            let allowed = gi[0] * gi[1] * tables.gutter_fill;
            if fill_sq > allowed + 1e-9 {
                (
                    "fail",
                    format!("{tag}'s gutter is over {}% full", r2(tables.gutter_fill * 100.0)),
                    format!("{} sq in of conductors in {} x {} in", r2(fill_sq), gi[0], gi[1]),
                )
            } else if !unread.is_empty() {
                ("unsure", format!("{tag}'s gutter fill can't be counted whole: sizes not read"), format!("{} sq in of conductors read", r2(fill_sq)))
            } else {
                ("pass", format!("{tag}'s gutter is within its fill"), format!("{} sq in of conductors in {} x {} in", r2(fill_sq), gi[0], gi[1]))
            }
        }
    };
    emit(
        "gutter",
        v,
        issue,
        format!("conductors at most {}% of the gutter's cross-section (NEC 312.8)", r2(tables.gutter_fill * 100.0)),
        found,
        json!({"conductor_sq_in": r2(fill_sq), "gutter_in": gutter, "allowed_sq_in": gutter.map(|g| r2(g[0] * g[1] * tables.gutter_fill)), "conductors": conductors, "not_read": unread}),
    );
    // ---- Lugs and bending space, per feeder end.
    for (f, incoming) in &ends {
        let given = if *incoming { f.to_point.trim() } else { f.from_point.trim() };
        let point = if !given.is_empty() {
            given.to_ascii_lowercase()
        } else {
            match (g.e.kind.to_ascii_lowercase().as_str(), incoming) {
                ("transformer", true) => "primary".into(),
                ("transformer", false) => "secondary".into(),
                ("disconnect", true) => "line".into(),
                ("disconnect", false) => "load".into(),
                (_, true) => "main".into(),
                (_, false) => "breaker".into(),
            }
        };
        let term: Option<&Termination> = g.e.terminations.iter().find(|t| t.point.trim().eq_ignore_ascii_case(&point));
        // The lugs: the submittal's, re-read; else the termination record (its source).
        let lugs: Option<(u32, u32, String, String, String)> = match (doc, point.as_str()) {
            (Some(d), "main") if d.main_lugs.is_some() => {
                d.main_lugs.as_ref().map(|(n, range)| (*n, 1, range.clone(), String::new(), format!("the submittal, re-read: {}", d.source)))
            }
            (Some(d), "breaker") if d.branch_lug.is_some() => {
                d.branch_lug.as_ref().map(|range| (1, 1, range.clone(), String::new(), format!("the submittal, re-read: {}", d.source)))
            }
            _ => term.map(|t| {
                (
                    t.lugs,
                    t.wires_per_lug.max(1),
                    t.range.clone(),
                    t.material.clone(),
                    if t.source.is_empty() { "its lug record".into() } else { t.source.clone() },
                )
            }),
        };
        let sets = cap(f.sets.max(1));
        let check = format!("lugs {} {point}", f.tag);
        let size = super::gear::cmil(&f.size);
        match &lugs {
            None => emit(
                &check,
                "unsure",
                format!("{tag}'s {point} lugs aren't in the submittal or its record: {} can't be checked on them", f.tag),
                format!("{sets} set(s) of {} per phase on {tag}'s {point} lugs", f.size),
                "no lug data".into(),
                json!({"feeder": f.tag, "point": point, "sets": sets, "size": f.size, "material": f.material}),
            ),
            Some((n, per, range, metal, from)) => {
                let holds = cap(*n).saturating_mul(cap(*per));
                let mut wrong: Vec<String> = Vec::new();
                let mut unknown: Vec<String> = Vec::new();
                if sets > holds {
                    wrong.push(format!("{sets} parallel sets need {sets} lug positions per phase; it has {holds}"));
                }
                match (size, super::gear::range(range)) {
                    (Some(c), Some((lo, hi))) if c < lo || c > hi => {
                        wrong.push(format!("{} is outside the lugs' range {range}", super::gear::size_label(c)))
                    }
                    (Some(_), Some(_)) => {}
                    (None, _) => unknown.push(format!("the size {} isn't one read here", f.size)),
                    (_, None) => unknown.push(format!("the lug range {range} isn't one read here")),
                }
                if f.material.trim().eq_ignore_ascii_case("al") && metal.trim().eq_ignore_ascii_case("cu") {
                    wrong.push("aluminium conductors on CU-only lugs".into());
                }
                let v = if !wrong.is_empty() {
                    "fail"
                } else if !unknown.is_empty() {
                    "unsure"
                } else {
                    "pass"
                };
                let issue = match v {
                    "fail" => format!("{} doesn't fit {tag}'s {point} lugs: {}", f.tag, wrong.join("; ")),
                    "unsure" => format!("{} on {tag}'s {point} lugs can't be checked: {}", f.tag, unknown.join("; ")),
                    _ => format!("{} fits {tag}'s {point} lugs", f.tag),
                };
                emit(
                    &check,
                    v,
                    issue,
                    format!("{sets} set(s) of {} {} per phase", f.size, f.material.to_ascii_uppercase()),
                    format!("{n} lug(s) per phase{}, {range}", if *per > 1 { format!(" x {per} wires") } else { String::new() }),
                    json!({"feeder": f.tag, "feeder_source": f.source, "point": point, "sets": sets, "size": f.size, "material": f.material, "lugs_per_phase": n, "wires_per_lug": per, "range": range, "lugs_from": from}),
                );
            }
        }
        // Bending space at the lugs (NEC 312.6).
        let bcheck = format!("bending {} {point}", f.tag);
        let per_terminal = lugs.as_ref().map_or(1, |l| l.1).min(sets).max(1);
        match (term.filter(|t| t.bending_in.is_finite() && t.bending_in > 0.0), size) {
            (Some(t), Some(c)) => match bending_need(tables, c, per_terminal, t.opposite) {
                Some(need) => {
                    let v = if need > t.bending_in + 1e-9 { "fail" } else { "pass" };
                    emit(
                        &bcheck,
                        v,
                        if v == "fail" {
                            format!(
                                "{}: {} needs {need} in to bend at {tag}'s {point} lugs; {} in there",
                                f.tag,
                                super::gear::size_label(c),
                                t.bending_in
                            )
                        } else {
                            format!("{}: room to bend at {tag}'s {point} lugs", f.tag)
                        },
                        format!("{need} in ({per_terminal} per terminal, NEC 312.6{})", if t.opposite { "(B)" } else { "(A)" }),
                        format!("{} in", t.bending_in),
                        json!({"feeder": f.tag, "point": point, "need_in": need, "given_in": t.bending_in, "opposite": t.opposite, "from": t.source}),
                    );
                }
                None => emit(
                    &bcheck,
                    "unsure",
                    format!("{}: no bending-space row for {} here", f.tag, super::gear::size_label(c)),
                    "the bending space NEC 312.6 gives".into(),
                    format!("{} in", t.bending_in),
                    json!({"feeder": f.tag, "point": point}),
                ),
            },
            _ => emit(
                &bcheck,
                "unsure",
                format!("the bending space at {tag}'s {point} lugs isn't in the submittal: missing, so {} can't be checked", f.tag),
                format!("the bending space NEC 312.6 gives for {} ({per_terminal} per terminal)", f.size),
                "no bending space given".into(),
                json!({"feeder": f.tag, "point": point}),
            ),
        }
    }
}

/// The build's own `conflict` review items about gear, as `conflict` findings (source `build`).
fn carried(cx: &Ctx, space: Option<&Effective>, fill: Option<&Effective>, size: Option<&Effective>, out: &mut Checked) {
    let tags: BTreeSet<String> = gear_tags(cx.sp).into_iter().map(|t| t.to_ascii_uppercase()).collect();
    let levels: BTreeSet<String> = cx.areas.iter().map(|a| a.level.to_ascii_lowercase()).collect();
    for item in cx.sp.rfis.iter().take(100_000) {
        if !matches!(item.status, cadaddy_walk3d::rfi::RfiStatus::Open | cadaddy_walk3d::rfi::RfiStatus::Sent)
            || !item.kind_name().contains("conflict")
        {
            continue;
        }
        let text = format!("{} {} {}", item.about, item.issue, item.assumed);
        let up = text.to_ascii_uppercase();
        let words: BTreeSet<&str> = up.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').filter(|w| !w.is_empty()).collect();
        // A tag counts alone when it can't be a word or a type letter (three characters or more).
        let named_tag = tags.iter().filter(|t| t.chars().count() >= 3).any(|t| words.contains(t.as_str()));
        let gear_word = ["SWITCHBOARD", "SWITCHGEAR", "PANELBOARD", "PANEL", "FEEDER", "FEEDERS", "LUGS", "LUG", "GEAR", "TRANSFORMER"]
            .iter()
            .any(|w| words.contains(w));
        if !(named_tag || gear_word) {
            continue;
        }
        let (id, r) = if up.contains("WORKING SPACE") || up.contains("CLEARANCE") || up.contains("110.26") || words.contains("DOOR") {
            (SPACE, space)
        } else if words.contains("SIZE") || words.contains("DIMENSION") || words.contains("DIMENSIONS") || words.contains("BOX") {
            (SIZE, size)
        } else {
            (FILL, fill)
        };
        let Some(r) = r else { continue };
        if !item.level.is_empty() && !levels.contains(&item.level.to_ascii_lowercase()) {
            continue;
        }
        let at = (!item.level.is_empty()).then(|| cx.frame.to_ft_on(&item.level, Vec2::new(item.at[0], item.at[1])));
        let area = at.and_then(|p| {
            cx.areas
                .iter()
                .find(|a| a.level.eq_ignore_ascii_case(&item.level) && inside(a.bbox, p) && (a.outline.len() < 3 || point_in(&a.outline, p)))
        });
        if cx.areas.len() == 1 && area.is_none() {
            continue;
        }
        let key = format!("{}|{}|{}", item.id, item.about, item.issue);
        out.findings.push((
            rank("conflict", &r.rule.severity),
            id,
            "conflict",
            json!({
                "id": format!("PS-{}", super::validate_run::hash8(&format!("build|{id}|{key}"))),
                "rule": id, "verdict": "conflict", "issue": quote(&item.issue), "expected": "the documents agree", "found": quote(&item.about),
                "measured": {"review_item": item.id, "kind": item.kind_name(), "assumed": quote(&item.assumed)},
                "documents": item.sheets.iter().take(8).map(|s| json!({"document": s})).collect::<Vec<_>>(),
                "evidence": [], "confidence": 1.0, "severity": r.rule.severity, "area": area.map(|a| a.id.clone()), "level": item.level,
                "at": at.map(|p| pt([r2(p[0]), r2(p[1])])), "source": "build",
            }),
        ));
    }
}

// ---------------------------------------------------------------- packets

/// Does the area hold any gear?
pub(crate) fn has_gear(d: &Drawing, sp: &BuildingSpec, frame: &Frame, area: &Area) -> bool {
    sp.mep.equipment.iter().any(is_gear) && in_area(&placed(d, sp, frame).0, area).next().is_some()
}

fn in_area<'a, 'b>(gear: &'a [Placed<'b>], area: &'a Area) -> impl Iterator<Item = &'a Placed<'b>> + 'a {
    gear.iter().filter(move |g| {
        g.level.eq_ignore_ascii_case(&area.level) && inside(area.bbox, g.obb.c) && (area.outline.len() < 3 || point_in(&area.outline, g.obb.c))
    })
}

/// A sheet of the job's set that the drawing doesn't hold (a schedule or diagram): its number,
/// title and type words, and its PDF file (in the job's prints folder) and page.
#[derive(Clone, Debug, Default)]
pub(crate) struct JobPage {
    pub number: String,
    /// Its discipline letters as the index gives them (empty: read off its number).
    pub discipline: String,
    pub words: String,
    pub file: String,
    pub page: u32,
}

/// The job's sheet index (`job.sheets`): every numbered sheet with its file and page.
pub(crate) fn job_pages(sheets: Option<&Value>) -> Vec<JobPage> {
    let list = sheets.and_then(|v| v.get("sheets")).and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    list.iter()
        .take(10_000)
        .filter_map(|c| {
            let text = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").trim().chars().take(200).collect::<String>();
            let number = text("number");
            let file = text("file");
            let page = u32::try_from(c.get("page")?.as_u64()?).ok().filter(|p| *p > 0)?;
            (!number.is_empty() && !file.is_empty()).then(|| JobPage {
                words: format!("{} {} {}", text("title"), text("type"), text("sheet_type")),
                discipline: text("discipline"),
                number,
                file,
                page,
            })
        })
        .collect()
}

/// Is a sheet's title a riser or one-line diagram?
fn riser_title(t: &str) -> bool {
    let u = t.to_ascii_uppercase().replace('-', " ");
    u.contains("RISER") || u.contains("ONE LINE") || u.contains("SINGLE LINE") || u.contains("ONELINE")
}

/// What a packet carries of the gear in its area: each piece's facts (its box as built and as the
/// submittal gives it, its working space, its placement, its lugs and enclosure, the feeders that
/// land on it with their sources), the sheets to show whole (the riser or one-line, the panel
/// and feeder schedules) and the gear submittal pages (`<file> p.<n>`, why). None without gear.
pub(crate) fn for_packet(
    d: &Drawing,
    sp: &BuildingSpec,
    frame: &Frame,
    area: &Area,
    docs: Option<&GearDocs>,
    regions: &[(String, String, [f64; 4])],
    job_pages: &[JobPage],
) -> Option<(Value, Vec<super::validate_keynote::CropAsk>, Vec<(String, String)>, BTreeSet<String>)> {
    let (gear, _) = placed(d, sp, frame);
    let here: Vec<&Placed> = in_area(&gear, area).collect();
    if here.is_empty() {
        return None;
    }
    let mut kinds: BTreeSet<String> = BTreeSet::new();
    let mut sheets: Vec<(String, &'static str, String)> = Vec::new();
    let mut pages: Vec<(String, String)> = Vec::new();
    // The sheets a packet can show: the spec's sheet links, and the job's other imported sheets
    // (schedules and diagrams are no plan: the job keeps only their rectangles).
    let numbers: Vec<&str> = sp
        .sheets
        .iter()
        .map(|l| l.number.as_str())
        .chain(regions.iter().map(|r| r.0.as_str()))
        .chain(job_pages.iter().map(|p| p.number.as_str()))
        .filter(|n| !n.trim().is_empty())
        .collect();
    let mut want_sheet = |text: &str, kind: &'static str, why: String| {
        let words: BTreeSet<String> =
            text.to_ascii_uppercase().split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')')).map(str::to_string).collect();
        for n in &numbers {
            if words.contains(&n.to_ascii_uppercase()) && !sheets.iter().any(|(s, _, _)| s.eq_ignore_ascii_case(n)) && sheets.len() < MAX_GEAR_SHEETS
            {
                sheets.push((n.to_string(), kind, why.clone()));
            }
        }
    };
    let mut list = Vec::new();
    for g in &here {
        let tag = g.e.tag.trim();
        kinds.insert(g.e.kind.to_ascii_lowercase());
        let sched = sp.mep.panel_schedules.iter().find(|p| p.panel.trim().eq_ignore_ascii_case(tag));
        if let Some(p) = sched {
            want_sheet(&p.source, "panel_schedule", format!("panel {tag}'s schedule"));
        }
        let feeders: Vec<Value> = sp
            .mep
            .feeders
            .iter()
            .filter(|f| f.to.trim().eq_ignore_ascii_case(tag) || f.from.trim().eq_ignore_ascii_case(tag))
            .take(MAX_ENDS)
            .map(|f| {
                json!({
                    "tag": f.tag, "from": f.from, "to": f.to, "sets": f.sets, "phases": f.phases, "size": f.size, "material": f.material,
                    "neutrals": f.neutrals, "grounds": f.grounds, "ground_size": f.ground_size, "conduits": f.conduits, "conduit_in": f.conduit_in,
                    "entry": f.entry, "from_entry": f.from_entry, "source": f.source,
                })
            })
            .collect();
        for f in sp.mep.feeders.iter().filter(|f| f.to.trim().eq_ignore_ascii_case(tag)).take(4) {
            want_sheet(&f.source, "feeder_schedule", format!("the feeder schedule of {}", f.tag));
        }
        if let Some(ps) = docs.and_then(|d| d.pages.get(&tag.to_ascii_uppercase())) {
            for p in ps.iter().take(MAX_PAGES_PER_TAG) {
                pages.push((p.clone(), format!("the gear submittal for {tag}")));
            }
        }
        let sub = docs.and_then(|d| d.of(tag)).map(|s| {
            json!({"size_in": s.size_in, "feed": s.feed, "main_lugs": s.main_lugs, "branch_lug": s.branch_lug, "top_plate_in": s.top_plate_in, "bottom_plate_in": s.bottom_plate_in, "kind": s.kind, "source": s.source})
        });
        let props: BTreeMap<&String, &String> = g.e.properties.iter().take(60).collect();
        list.push(json!({
            "tag": tag, "kind": g.e.kind, "handle": g.handle, "level": g.level, "at_ft": pt([r2(g.obb.c[0]), r2(g.obb.c[1])]),
            "model_in": {"height": r2(g.z_in[1] - g.z_in[0]), "width": r2(g.obb.hw * 24.0), "depth": r2(g.obb.hd * 24.0), "bottom": r2(g.z_in[0])},
            "model_from": g.from, "properties": props, "sources": g.e.sources, "submittal": sub,
            "terminations": g.e.terminations.iter().take(20).map(|t| json!({"point": t.point, "lugs": t.lugs, "wires_per_lug": t.wires_per_lug, "range": t.range, "material": t.material, "bending_in": t.bending_in, "source": t.source})).collect::<Vec<_>>(),
            "enclosure": g.e.enclosure.as_ref().map(|e| json!({"top_entry_in": e.top_entry_in, "bottom_entry_in": e.bottom_entry_in, "gutter_in": e.gutter_in})),
            "schedule": sched.map(|p| json!({"sheet": p.source, "header": p.header})),
            "feeders": feeders,
        }));
    }
    // The riser or one-line diagrams: a sheet titled so, or a job sheet of that kind.
    // An electrical one (a fire alarm or low-voltage riser is another system's).
    let electrical = |number: &str, given: Option<&str>| {
        let d = given.map(str::trim).filter(|d| !d.is_empty()).map_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(number), str::to_string);
        d.eq_ignore_ascii_case("E")
    };
    let risers: Vec<(String, String)> = sp
        .sheets
        .iter()
        .filter(|l| l.view.is_none() && riser_title(&l.title) && electrical(&l.number, l.discipline.as_deref()))
        .map(|l| (l.number.clone(), format!("{} {}", l.number, l.title)))
        .chain(
            regions
                .iter()
                .filter(|r| riser_title(&r.1.replace('_', " ")) && electrical(&r.0, None))
                .map(|r| (r.0.clone(), format!("{} ({})", r.0, r.1))),
        )
        .chain(
            job_pages
                .iter()
                .filter(|p| riser_title(&p.words.replace('_', " ")) && electrical(&p.number, Some(&p.discipline)))
                .map(|p| (p.number.clone(), format!("{} {}", p.number, p.words.trim()))),
        )
        .take(2)
        .collect();
    for (n, why) in risers {
        if !sheets.iter().any(|(s, _, _)| s.eq_ignore_ascii_case(&n)) && sheets.len() < MAX_GEAR_SHEETS {
            sheets.insert(0, (n, "gear_riser", why));
        }
    }
    // A sheet the drawing doesn't hold is shown as its PDF page (`<file> p.<n>`).
    for (n, _, why) in &sheets {
        let in_drawing =
            sp.sheets.iter().any(|l| l.number.eq_ignore_ascii_case(n) && l.rect.is_some()) || regions.iter().any(|r| r.0.eq_ignore_ascii_case(n));
        if in_drawing {
            continue;
        }
        if let Some(p) = job_pages.iter().find(|p| p.number.eq_ignore_ascii_case(n)) {
            let file = p.file.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(&p.file).to_string();
            pages.push((format!("{file} p.{}", p.page), format!("sheet {n}: {why}")));
        }
    }
    let asks: Vec<super::validate_keynote::CropAsk> = sheets
        .iter()
        .filter_map(|(n, kind, why)| {
            let link = sp
                .sheets
                .iter()
                .find(|l| l.number.eq_ignore_ascii_case(n) && l.view.is_none())
                .or_else(|| sp.sheets.iter().find(|l| l.number.eq_ignore_ascii_case(n)))
                .and_then(|l| l.rect.map(|r| (l.number.clone(), r)));
            let (number, r) = link.or_else(|| regions.iter().find(|r| r.0.eq_ignore_ascii_case(n)).map(|r| (r.0.clone(), r.2)))?;
            let r = Some(r).filter(|r| r.iter().all(|v| v.is_finite()))?;
            Some(super::validate_keynote::CropAsk { sheet: number, kind, rect: super::validate_packet::norm_rect(r), shows: why.clone() })
        })
        .collect();
    let v = json!({
        "about": "the electrical gear in this area: its box as built (`model_in`) and as the gear submittal gives it (`submittal`, re-read; `properties.BOX` the build's record), its working space and placement (`properties`, `sources.status`: proposed when CADaddy laid it out), its lugs and enclosure, and every feeder that lands on it with its source (schedule row, one-line tag). E-GEAR-SPACE, E-GEAR-FILL and E-GEAR-SIZE in `prescreen` recomputed space, fill and size from these; the riser, the panel and feeder schedules (`gear_riser`, `panel_schedule`, `feeder_schedule` prints) and the gear submittal pages are in the packet",
        "gear": list,
        "sheets": sheets.iter().map(|(n, k, w)| json!({"sheet": n, "kind": k, "why": w})).collect::<Vec<_>>(),
        "submittal_pages": pages.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
        "submittals_read": docs.map(|d| d.read.clone()).unwrap_or_default(),
        "submittals_unread": docs.map(|d| d.unread.iter().map(|(n, w)| json!({"submittal": n, "why": w})).collect::<Vec<_>>()).unwrap_or_default(),
    });
    Some((v, asks, pages, kinds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(what: &str, a: [f64; 2], b: [f64; 2], t_in: f64) -> Obstacle {
        Obstacle { what: what.into(), a, b, half_ft: t_in / 24.0, z_in: None }
    }

    /// A 20" × 6" panel on the south wall of a 10' × 8' room (inside 0..10 × 0..8), centred at x 5.
    fn panel() -> Obb {
        Obb { c: [5.0, 0.25], u: [1.0, 0.0], hw: 10.0 / 24.0, hd: 0.25 }
    }

    fn room(north: f64) -> Vec<Obstacle> {
        vec![
            wall("S", [-1.0, -0.25], [11.0, -0.25], 6.0),
            wall("N", [-1.0, north + 0.25], [11.0, north + 0.25], 6.0),
            wall("W", [-0.25, -1.0], [-0.25, north + 1.0], 6.0),
            wall("E", [10.25, -1.0], [10.25, north + 1.0], 6.0),
        ]
    }

    #[test]
    fn a_box_round_its_corners_turned_or_not() {
        let o = Obb::round(&[[0.0, 0.0], [2.0, 0.0], [2.0, 0.5], [0.0, 0.5]]).unwrap();
        assert!((o.hw - 1.0).abs() < 1e-9 && (o.hd - 0.25).abs() < 1e-9 && (o.c[0] - 1.0).abs() < 1e-9, "{o:?}");
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let pts: Vec<[f64; 2]> = [[0.0, 0.0], [2.0, 0.0], [2.0, 0.5], [0.0, 0.5]].iter().map(|p| [s * (p[0] - p[1]), s * (p[0] + p[1])]).collect();
        let o = Obb::round(&pts).unwrap();
        assert!((o.hw - 1.0).abs() < 1e-6 && (o.hd - 0.25).abs() < 1e-6, "{o:?}");
        assert!(Obb::round(&[[0.0, 0.0], [f64::NAN, 1.0]]).is_none());
    }

    #[test]
    fn working_space_is_measured_from_the_face_away_from_its_wall() {
        // 8 ft to the north wall: clear for 120 V (36") and for 277 V condition 3 (48").
        let ws = working_space(&panel(), Some([36.0, 36.0]), 30.0, 78.0, &room(8.0), &[], Some("101"));
        assert_eq!(ws.verdict, "pass", "{ws:?}");
        assert!((ws.clear_in - 90.0).abs() < 0.6, "{ws:?}");
        // A 30" deep room: short.
        let ws = working_space(&panel(), Some([36.0, 36.0]), 30.0, 78.0, &room(3.0), &[], None);
        assert_eq!(ws.verdict, "fail", "{ws:?}");
        assert_eq!(ws.blocked_by.as_deref(), Some("N"), "{ws:?}");
        // 277 V: 42" clear is enough for condition 1, not condition 3.
        let ws = working_space(&panel(), Some([36.0, 48.0]), 30.0, 78.0, &room(4.0), &[], None);
        assert_eq!(ws.verdict, "unsure", "{ws:?}");
        // The voltage not known: never a pass.
        let ws = working_space(&panel(), None, 30.0, 78.0, &room(8.0), &[], None);
        assert_eq!(ws.verdict, "unsure", "{ws:?}");
    }

    #[test]
    fn a_recessed_panel_is_measured_from_its_face_not_against_its_own_wall() {
        // Set in the south wall (y -0.5..0), its front flush with the face, 4 in deep.
        let recessed = Obb { c: [5.0, -1.0 / 6.0], u: [1.0, 0.0], hw: 7.0 / 12.0, hd: 1.0 / 6.0 };
        let mut obs = room(8.0);
        // The wall in pieces either side of it, on the same line.
        obs.push(wall("S2", [-1.0, -0.25], [4.0, -0.25], 6.0));
        let ws = working_space(&recessed, Some([36.0, 36.0]), 30.0, 78.0, &obs, &[], None);
        assert_eq!(ws.verdict, "pass", "{ws:?}");
        assert!((ws.clear_in - 96.0).abs() < 0.6, "{ws:?}");
        assert_eq!(ws.front, "the face away from the wall it is on", "{ws:?}");
    }

    #[test]
    fn equipment_in_front_blocks_and_one_above_the_space_does_not() {
        let mut obs = room(8.0);
        let c = Obb { c: [5.0, 2.5], u: [1.0, 0.0], hw: 1.0, hd: 0.5 }.corners();
        for k in 0..4 {
            obs.push(Obstacle { what: "equipment X".into(), a: c[k], b: c[(k + 1) % 4], half_ft: 0.0, z_in: Some([0.0, 40.0]) });
        }
        let ws = working_space(&panel(), Some([36.0, 36.0]), 30.0, 78.0, &obs, &[], None);
        assert_eq!((ws.verdict, ws.blocked_by.as_deref()), ("fail", Some("equipment X")), "{ws:?}");
        for o in obs.iter_mut().filter(|o| o.what == "equipment X") {
            o.z_in = Some([96.0, 110.0]);
        }
        assert_eq!(working_space(&panel(), Some([36.0, 36.0]), 30.0, 78.0, &obs, &[], None).verdict, "pass");
    }

    #[test]
    fn a_door_swinging_into_the_room_over_the_space_fails_and_one_that_may_is_unsure() {
        // A 3 ft door in the east wall, 1 to 4 ft north: its leaf sweeps x 7.25..10 in the room.
        let wide = Obb { c: [8.0, 0.25], u: [1.0, 0.0], hw: 1.5, hd: 0.25 };
        let door =
            |into: Option<&str>| Door { id: "D1".into(), min: [10.0, 1.0], max: [10.5, 4.0], swings_into: into.map(str::to_string), source: None };
        let ws = working_space(&wide, Some([36.0, 36.0]), 30.0, 78.0, &room(8.0), &[door(Some("101"))], Some("101"));
        assert_eq!(ws.verdict, "fail", "{ws:?}");
        let ws = working_space(&wide, Some([36.0, 36.0]), 30.0, 78.0, &room(8.0), &[door(None)], Some("101"));
        assert_eq!(ws.verdict, "unsure", "{ws:?}");
        let ws = working_space(&wide, Some([36.0, 36.0]), 30.0, 78.0, &room(8.0), &[door(Some("102"))], Some("101"));
        assert_eq!(ws.verdict, "pass", "{ws:?}");
    }

    #[test]
    fn raceways_pack_in_rows_or_fail() {
        // Six 2" conduits (2.375 + 0.75 pitch) on 16 x 4: one row of five fits, six don't.
        let p = vec![3.125; 5];
        assert!(pack(&p, [16.0, 4.0]).0);
        let p = vec![3.125; 6];
        let (fits, need, have) = pack(&p, [16.0, 4.0]);
        assert!(!fits && need > 58.0 && (have - 64.0).abs() < 1e-9);
        assert!(!pack(&[5.0], [4.0, 10.0]).0);
    }

    #[test]
    fn a_size_record_counts_only_when_a_submittal_gave_it() {
        let mut e = MepEquipment::default();
        e.properties.insert("SIZE".into(), "14\" W x 4\" D x 24\" H".into());
        let (b, other) = recorded_box(&e);
        assert!(b.is_none() && other.is_some_and(|o| o["in"] == json!([24.0, 14.0, 4.0])));
        e.sources.insert("size".into(), "Panelboard submittal p.3".into());
        assert_eq!(recorded_box(&e).0, Some([24.0, 14.0, 4.0]));
        e.properties.insert("BOX".into(), "50 H x 20 W x 5.75 D in".into());
        assert_eq!(recorded_box(&e).0, Some([50.0, 20.0, 5.75]));
    }

    #[test]
    fn the_job_sheet_index_gives_each_numbered_sheet_its_page() {
        let v = json!({"sheets": [
            {"number": "X-1", "title": "ONE-LINE DIAGRAM", "type": "diagram", "discipline": "E", "file": "set/Elec.pdf", "page": 2},
            {"number": "X-2", "file": "Elec.pdf", "page": 0},
            {"number": "", "file": "Elec.pdf", "page": 3},
            {"number": "X-3", "page": 4},
            {"number": "X-4", "file": "Elec.pdf", "page": 99999999999u64}
        ]});
        let pages = job_pages(Some(&v));
        assert_eq!(pages.len(), 1, "{pages:?}");
        assert!(pages[0].number == "X-1" && pages[0].page == 2 && riser_title(&pages[0].words), "{pages:?}");
        assert!(job_pages(None).is_empty() && job_pages(Some(&json!("x"))).is_empty());
    }

    #[test]
    fn volts_box_and_hostile_text() {
        assert_eq!(volts_to_ground("208Y/120V, 3PH, 4W"), Some(120.0));
        assert_eq!(volts_to_ground("480Y/277V"), Some(277.0));
        assert_eq!(volts_to_ground("480V"), Some(480.0));
        assert_eq!(volts_to_ground("N/A"), None);
        assert_eq!(box_prop("50 H x 20 W x 5.75 D in"), Some([50.0, 20.0, 5.75]));
        assert_eq!(box_prop("x x x"), None);
        assert_eq!(box_prop(&"9".repeat(10_000)), None);
    }
}
