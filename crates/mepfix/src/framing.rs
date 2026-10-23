//! Framing layout from prints (Q303): the studs (and joists) the prints imply, every member
//! labelled by how sure we are.
//!
//! - **Fixed** (exact from the prints): wall end studs, corner and intersection studs where walls
//!   meet, posts and the studs beside them where a column stands in a wall, king and jack studs and
//!   headers at openings, backing where details call for it.
//! - **Rule**: field studs at the wall type's spacing, laid out so sheet edges land on centres
//!   (first mark 15 1/4" for 16" o.c.: the mark is 3/4" short of each multiple of the spacing and
//!   the stud stands on the far side of it), carried through openings as cripples. Which end the
//!   layout was pulled from is unknown, so each wall keeps BOTH layouts; where they coincide the
//!   member is effectively fixed (**agreed**). Walls stacked storey over storey share one layout.
//! - **Shop drawing**: a metal stud (or joist/truss) layout replaces the rule members where it
//!   covers; post-tensioning tendons become no-core zones.
//! - **Field verified**: one measured stud per wall (a tape distance, or a pick on a site photo)
//!   snaps the run, and the walls stacked with it, from rule to verified.
//!
//! Then: field-change flags (plumbing walls, drains, recessed items), devices snapped to the
//! predicted stud ("lands on the stud at 31-1/4", 4" from the lav drain"), raceway crossings
//! checked with [`crate::bore`], recessed cans against joist bays and beams, takeoff by status,
//! and drawings (plan, elevation) with fixed members solid and rule members ghosted.
//!
//! Positions along a wall are inches from its `a` end; heights are inches above its floor.

use crate::bore::{self, BoreCheck, HoleAt, Raceway};
use crate::rng::Rng;
use crate::truth::{Truth, WallTypeLayer};
use crate::wall_reader::WallResult;
use cadaddy_color::Color;
use cadaddy_doc::{Common, Drawing, EntityKind, HAlign, Layer, Line, Lineweight, LwPolyline, Space, Text, VAlign};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// Layout marks fall this far short of each multiple of the spacing (the stud stands past the
/// mark), so 4' sheets break on stud centres.
pub const LAYOUT_OFFSET_IN: f64 = 0.75;
/// The two layouts agree when their studs are this close.
pub const AGREE_TOL_IN: f64 = 0.5;
/// Metal stud flange width (along the wall).
pub const METAL_FLANGE_IN: f64 = 1.625;
/// Sawn stud thickness.
pub const WOOD_STUD_IN: f64 = 1.5;
/// Door head height above the floor when the prints don't give one.
pub const DOOR_HEAD_IN: f64 = 84.0;
/// Two studs may touch or overlap this much and both stay (a tight fit is still two studs; and
/// published section sizes vs. rounded ones must not flip which members exist).
pub const OVERLAP_OK_IN: f64 = 0.25;
/// No-core zone each side of a post-tensioning tendon.
pub const TENDON_CLEAR_IN: f64 = 12.0;

const MAX_WALLS: usize = 20_000;
const MAX_LEN_IN: f64 = 12_000.0;
const MAX_OPENINGS: usize = 200;
const MAX_POSTS: usize = 20_000;
const MAX_SHOP: usize = 200_000;

// ---- Members -----------------------------------------------------------------------------------

/// How sure we are of a member.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Exact from the prints.
    Fixed,
    /// A rule member both layouts put in the same place.
    Agreed,
    /// A rule member of one layout.
    Rule,
    /// From a shop drawing.
    ShopDrawing,
    /// Snapped to a field measurement.
    FieldVerified,
}

impl Status {
    pub fn words(self) -> &'static str {
        match self {
            Status::Fixed => "fixed",
            Status::Agreed => "rule (both layouts agree)",
            Status::Rule => "rule",
            Status::ShopDrawing => "shop drawing",
            Status::FieldVerified => "field verified",
        }
    }

    /// Solid in drawings (anything but a one-layout rule member).
    pub fn sure(self) -> bool {
        self != Status::Rule
    }
}

/// The end of a wall (or the side of a bay) a layout was pulled from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    A,
    B,
}

impl End {
    pub fn other(self) -> End {
        match self {
            End::A => End::B,
            End::B => End::A,
        }
    }
    pub fn letter(self) -> &'static str {
        match self {
            End::A => "a",
            End::B => "b",
        }
    }
    pub fn parse(s: &str) -> Option<End> {
        match s {
            "a" | "A" => Some(End::A),
            "b" | "B" => Some(End::B),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    EndStud,
    /// In the through wall where another wall meets it near its end.
    CornerStud,
    /// In the through wall either side of a wall that tees into it.
    IntersectionStud,
    /// A column standing in the wall.
    Post,
    /// The studs either side of a post.
    PostStud,
    KingStud,
    JackStud,
    Header,
    Cripple,
    FieldStud,
    Backing,
    /// Bays: the members at the bay's edges.
    Rim,
    /// Bays: a beam running with the joists.
    Beam,
    Joist,
}

impl Kind {
    /// Floor to top: studs and posts (a device can mount on these).
    pub fn full_height(self) -> bool {
        matches!(self, Kind::EndStud | Kind::CornerStud | Kind::IntersectionStud | Kind::PostStud | Kind::KingStud | Kind::FieldStud)
    }
    pub fn words(self) -> &'static str {
        match self {
            Kind::EndStud => "end stud",
            Kind::CornerStud => "corner stud",
            Kind::IntersectionStud => "intersection stud",
            Kind::Post => "post",
            Kind::PostStud => "stud at post",
            Kind::KingStud => "king stud",
            Kind::JackStud => "jack stud",
            Kind::Header => "header",
            Kind::Cripple => "cripple",
            Kind::FieldStud => "field stud",
            Kind::Backing => "backing",
            Kind::Rim => "rim",
            Kind::Beam => "beam",
            Kind::Joist => "joist",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub kind: Kind,
    pub status: Status,
    /// Rule members: the layout they belong to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<End>,
    /// Centre along the wall from its `a` end (bays: from the bay's low side), inches.
    pub s_in: f64,
    /// Along the wall, inches.
    pub width_in: f64,
    /// Bottom and top above the floor, inches.
    pub z_in: [f64; 2],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Member {
    fn new(kind: Kind, status: Status, s_in: f64, width_in: f64, z_in: [f64; 2]) -> Member {
        Member { kind, status, layout: None, s_in, width_in, z_in, note: String::new() }
    }
    /// Part of the framing when the layout was pulled from `end`.
    pub fn in_layout(&self, end: End) -> bool {
        self.layout.is_none_or(|l| l == end)
    }
}

// ---- Walls to frame ----------------------------------------------------------------------------

/// A wall as framing needs it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameWall {
    pub id: String,
    pub level: String,
    pub a_ft: [f64; 2],
    pub b_ft: [f64; 2],
    pub thickness_in: f64,
    pub wall_type: String,
    /// Stud depth; none = no studs (masonry).
    pub stud_in: Option<f64>,
    /// Stud width along the wall.
    pub flange_in: f64,
    pub spacing_in: f64,
    pub height_ft: f64,
    /// Openings: from and to along it, feet.
    pub openings: Vec<[f64; 2]>,
    /// Moisture-resistant board: a plumbing wall.
    pub plumbing: bool,
    /// Rooms either side.
    pub rooms: Vec<String>,
    /// The plan didn't tag it: why its type is the likeliest (its studs follow that guess).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_guess: Option<String>,
}

impl FrameWall {
    pub fn len_in(&self) -> f64 {
        ((self.b_ft[0] - self.a_ft[0]).hypot(self.b_ft[1] - self.a_ft[1]) * 12.0).min(MAX_LEN_IN)
    }
    fn dir(&self) -> [f64; 2] {
        let l = (self.b_ft[0] - self.a_ft[0]).hypot(self.b_ft[1] - self.a_ft[1]).max(1e-9);
        [(self.b_ft[0] - self.a_ft[0]) / l, (self.b_ft[1] - self.a_ft[1]) / l]
    }
    /// A plan point → (along from `a`, off the centreline), inches.
    pub fn local(&self, p: [f64; 2]) -> (f64, f64) {
        let u = self.dir();
        let d = [(p[0] - self.a_ft[0]) * 12.0, (p[1] - self.a_ft[1]) * 12.0];
        (d[0] * u[0] + d[1] * u[1], d[0] * u[1] - d[1] * u[0])
    }
    /// Inches along from `a` → plan feet.
    pub fn at(&self, s_in: f64) -> [f64; 2] {
        let u = self.dir();
        [self.a_ft[0] + u[0] * s_in / 12.0, self.a_ft[1] + u[1] * s_in / 12.0]
    }
    fn end_pt(&self, e: End) -> [f64; 2] {
        match e {
            End::A => self.a_ft,
            End::B => self.b_ft,
        }
    }
}

fn stud_of(layers: &[WallTypeLayer]) -> Option<(f64, f64)> {
    layers.iter().find(|l| l.material == "stud").map(|l| (l.thickness_in, l.spacing_in.unwrap_or(16.0)))
}

fn sane(v: f64, lo: f64, hi: f64, default: f64) -> f64 {
    if v.is_finite() && v >= lo && v <= hi { v } else { default }
}

fn frame_wall(
    id: String,
    level: &str,
    a: [f64; 2],
    b: [f64; 2],
    th: f64,
    tag: &str,
    layers: &[WallTypeLayer],
    h: f64,
    openings: &[[f64; 2]],
    rooms: &[String],
) -> FrameWall {
    let stud = stud_of(layers);
    FrameWall {
        id,
        level: level.into(),
        a_ft: a,
        b_ft: b,
        thickness_in: sane(th, 0.5, 48.0, 4.875),
        wall_type: tag.into(),
        stud_in: stud.map(|s| sane(s.0, 1.0, 16.0, 3.625)),
        flange_in: METAL_FLANGE_IN,
        spacing_in: stud.map_or(16.0, |s| sane(s.1, 8.0, 48.0, 16.0)),
        height_ft: sane(h, 1.0, 100.0, 10.0),
        openings: openings.iter().take(MAX_OPENINGS).copied().filter(|o| o.iter().all(|v| v.is_finite()) && o[1] > o[0]).collect(),
        plumbing: layers.iter().any(|l| l.material == "gyp_mr"),
        rooms: rooms.to_vec(),
        type_guess: None,
    }
}

/// The truth's walls.
pub fn walls_from_truth(t: &Truth) -> Vec<FrameWall> {
    t.plan_walls
        .iter()
        .take(MAX_WALLS)
        .map(|w| {
            let wt = t.wall_types.iter().find(|x| x.tag == w.wall_type);
            let layers = wt.map(|x| x.layers.as_slice()).unwrap_or(&[]);
            frame_wall(
                w.id.clone(),
                &w.level,
                w.a_ft,
                w.b_ft,
                wt.map_or(4.875, |x| x.thickness_in),
                &w.wall_type,
                layers,
                w.height_ft,
                &w.openings,
                &w.rooms,
            )
        })
        .collect()
}

/// The walls a wall reader found (typed from tags or guesses).
pub fn walls_from_reader(r: &WallResult) -> Vec<FrameWall> {
    r.walls
        .iter()
        .take(MAX_WALLS)
        .enumerate()
        .map(|(i, w)| {
            let tag = w.wall_type.clone().unwrap_or_default();
            let layers = r.types.iter().find(|x| x.tag == tag).map(|x| x.layers.as_slice()).unwrap_or(&[]);
            let mut fw = frame_wall(
                format!("F{}-{:03}", w.level, i + 1),
                &w.level,
                w.a_ft,
                w.b_ft,
                w.thickness_in,
                &tag,
                layers,
                w.height_ft.unwrap_or(10.0),
                &w.openings,
                &w.rooms,
            );
            fw.type_guess = if w.tagged { None } else { Some(w.guess.clone().unwrap_or_else(|| "no tag and no guess".into())) };
            fw
        })
        .collect()
}

/// A column (post) in plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub at_ft: [f64; 2],
    /// Along x and y, inches.
    pub footprint_in: [f64; 2],
}

pub fn posts_from_truth(t: &Truth) -> Vec<Post> {
    t.building.columns.iter().take(MAX_POSTS).map(|c| Post { at_ft: c.at_ft, footprint_in: c.footprint_in }).collect()
}

/// The columns a structural reader found. Plans draw columns as symbols (often bigger than the
/// steel at small scales), so the footprint comes from the general notes' column size when it is
/// a known section (strong axis along x, as the framing plans run), else the drawn size.
pub fn posts_from_structure(s: &crate::struct_reader::StructuralModel) -> Vec<Post> {
    let section = s.column_size.as_deref().and_then(section_footprint);
    s.columns.iter().take(MAX_POSTS).map(|c| Post { at_ft: c.at_ft, footprint_in: section.unwrap_or(c.footprint_in) }).collect()
}

/// A steel column's footprint (depth along x, flange width along y), inches: `HSS6x6`,
/// `HSS8x6x1/2`, or a common wide-flange size (published section depths and flange widths).
pub fn section_footprint(size: &str) -> Option<[f64; 2]> {
    let s = size.trim().to_ascii_uppercase().replace(' ', "");
    if let Some(rest) = s.strip_prefix("HSS") {
        let mut it = rest.split('X');
        let a: f64 = it.next()?.parse().ok()?;
        let b: f64 = it.next()?.parse().ok()?;
        return (a > 0.0 && b > 0.0 && a < 60.0 && b < 60.0).then_some([a, b]);
    }
    const W: [(&str, f64, f64); 15] = [
        ("W8X24", 7.93, 6.50),
        ("W8X31", 8.00, 8.00),
        ("W8X35", 8.12, 8.02),
        ("W8X40", 8.25, 8.07),
        ("W10X33", 9.73, 7.96),
        ("W10X39", 9.92, 7.99),
        ("W10X45", 10.1, 8.02),
        ("W10X49", 10.0, 10.0),
        ("W10X60", 10.2, 10.1),
        ("W12X40", 11.9, 8.01),
        ("W12X53", 12.1, 10.0),
        ("W12X65", 12.1, 12.0),
        ("W14X48", 13.8, 8.03),
        ("W14X61", 13.9, 10.0),
        ("W14X90", 14.0, 14.5),
    ];
    W.iter().find(|w| w.0 == s).map(|w| [w.1, w.2])
}

/// A backing request from a detail (grab bar, TV, cabinet, handrail).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BackingCall {
    pub level: String,
    /// Plan points the backing runs between (on the wall).
    pub from_ft: [f64; 2],
    pub to_ft: [f64; 2],
    /// Bottom and top above the floor, inches.
    pub z_in: [f64; 2],
    pub reason: String,
}

/// A header size from the structural header schedule: up to `max_span_in`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeaderRow {
    pub max_span_in: f64,
    pub size: String,
    pub depth_in: f64,
}

/// When the prints have no header schedule: box headers of studs by span (a placeholder size,
/// flagged as such).
pub fn default_headers() -> Vec<HeaderRow> {
    let row = |max_span_in, size: &str, depth_in| HeaderRow { max_span_in, size: size.into(), depth_in };
    vec![
        row(48.0, "box header, 2 studs 6\" deep", 6.0),
        row(72.0, "box header, 2 studs 8\" deep", 8.0),
        row(f64::MAX, "box header, 2 studs 10\" deep", 10.0),
    ]
}

// ---- Framing a wall ----------------------------------------------------------------------------

/// A framed wall.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WallFrame {
    pub wall: FrameWall,
    pub members: Vec<Member>,
    /// Expected to change in the field (plumbing, recessed items), and why.
    pub field_changes: Vec<String>,
    pub flags: Vec<String>,
    /// Which layout a field point or shop drawing settled on (none: both still open).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<String>,
}

impl WallFrame {
    /// The members when the layout was pulled from `end` (fixed, agreed, shop, verified and that
    /// layout's rule members).
    pub fn resolved(&self, end: End) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(move |m| m.in_layout(end))
    }

    /// Where the first field stud's near edge is from the `from` end under each layout: the
    /// "next stud" dimension (`15-1/4"` from the corner when laid out from it).
    pub fn next_stud_in(&self, from: End) -> [Option<f64>; 2] {
        let len = self.wall.len_in();
        let f = self.wall.flange_in;
        let near = |m: &Member| match from {
            End::A => m.s_in - m.width_in / 2.0,
            End::B => len - m.s_in - m.width_in / 2.0,
        };
        let first = |end: End| {
            self.members
                .iter()
                .filter(|m| {
                    matches!(m.status, Status::Rule | Status::Agreed) && m.in_layout(end) && matches!(m.kind, Kind::FieldStud | Kind::Cripple)
                })
                .map(near)
                .filter(|d| *d > f)
                .reduce(f64::min)
        };
        [first(End::A), first(End::B)]
    }

    /// The dimension string for one end: `next stud 16" o.c. → 15-1/4" from the a end if laid
    /// out from it; 9-3/4" if laid out from the b end`.
    pub fn next_stud_words(&self, from: End) -> String {
        let [a, b] = self.next_stud_in(from);
        let (own, other) = match from {
            End::A => (a, b),
            End::B => (b, a),
        };
        let sp = inch(self.wall.spacing_in);
        match (own, other) {
            (Some(o), Some(t)) if (o - t).abs() <= AGREE_TOL_IN => {
                format!("next stud {sp} o.c. → {} from the {} end either way", inch(o), from.letter())
            }
            (Some(o), Some(t)) => format!(
                "next stud {sp} o.c. → {} from the {} end if laid out from it; {} if laid out from the {} end",
                inch(o),
                from.letter(),
                inch(t),
                from.other().letter()
            ),
            _ => match self.settled_next(from) {
                Some(v) => {
                    format!("next stud {sp} o.c. → {} from the {} end ({})", inch(v), from.letter(), self.settled.as_deref().unwrap_or("settled"))
                }
                None => String::new(),
            },
        }
    }

    fn settled_next(&self, from: End) -> Option<f64> {
        let len = self.wall.len_in();
        let f = self.wall.flange_in;
        self.members
            .iter()
            .filter(|m| matches!(m.status, Status::FieldVerified | Status::ShopDrawing) && matches!(m.kind, Kind::FieldStud | Kind::Cripple))
            .map(|m| match from {
                End::A => m.s_in - m.width_in / 2.0,
                End::B => len - m.s_in - m.width_in / 2.0,
            })
            .filter(|d| *d > f)
            .reduce(f64::min)
    }
}

/// Inches to the nearest 1/16: `15-1/4"`, `9-3/4"`, `3"`.
pub fn inch(v: f64) -> String {
    let n = (v.abs() * 16.0).round() as i64;
    let (whole, rest) = (n / 16, n % 16);
    let sign = if v < 0.0 && n != 0 { "-" } else { "" };
    if rest == 0 {
        return format!("{sign}{whole}\"");
    }
    let (mut num, mut den) = (rest, 16);
    while num % 2 == 0 {
        num /= 2;
        den /= 2;
    }
    if whole == 0 { format!("{sign}{num}/{den}\"") } else { format!("{sign}{whole}-{num}/{den}\"") }
}

/// Where another wall meets this one: along it, the other's thickness.
fn junctions(w: &FrameWall, all: &[FrameWall]) -> Vec<(f64, f64)> {
    let len = w.len_in();
    let u = w.dir();
    let mut out = Vec::new();
    for o in all.iter().filter(|o| o.level == w.level && o.id != w.id) {
        let ou = o.dir();
        if (u[0] * ou[0] + u[1] * ou[1]).abs() > 0.5 {
            continue;
        }
        for p in [o.a_ft, o.b_ft] {
            let (s, off) = w.local(p);
            if off.abs() <= w.thickness_in / 2.0 + 1.0 && s > -o.thickness_in / 2.0 && s < len + o.thickness_in / 2.0 {
                out.push((s, o.thickness_in));
            }
        }
    }
    out
}

/// Frame every wall: fixed members from the plans, both rule layouts, stacked walls grouped.
pub fn frame(walls: &[FrameWall], posts: &[Post], backing: &[BackingCall], headers: &[HeaderRow]) -> Framing {
    let walls = walls.get(..walls.len().min(MAX_WALLS)).unwrap_or(walls);
    let mut out = Framing::default();
    let headers = if headers.is_empty() { default_headers() } else { headers.to_vec() };
    let schedule_given = headers != default_headers();
    for w in walls {
        let mut wf = WallFrame { wall: w.clone(), ..WallFrame::default() };
        if let Some(g) = &w.type_guess {
            wf.flags.push(format!("untagged wall, type guessed ({g}): its stud size and spacing are the guess's"));
        }
        if w.stud_in.is_none() {
            wf.flags.push("no studs (masonry)".into());
            out.walls.push(wf);
            continue;
        }
        let len = w.len_in();
        if len < 2.0 * w.flange_in {
            wf.flags.push("too short to frame".into());
            out.walls.push(wf);
            continue;
        }
        let f = w.flange_in;
        let top = w.height_ft * 12.0;
        let full = [0.0, top];
        let mut fixed: Vec<Member> =
            vec![Member::new(Kind::EndStud, Status::Fixed, f / 2.0, f, full), Member::new(Kind::EndStud, Status::Fixed, len - f / 2.0, f, full)];
        // Posts standing in the wall.
        let mut post_spans: Vec<[f64; 2]> = Vec::new();
        for p in posts {
            let (s, off) = w.local(p.at_ft);
            let u = w.dir();
            let along = (u[0] * p.footprint_in[0]).abs() + (u[1] * p.footprint_in[1]).abs();
            if !(off.abs() <= w.thickness_in / 2.0 && s > -along / 2.0 && s < len + along / 2.0 && along.is_finite() && along > 0.0) {
                continue;
            }
            let span = [(s - along / 2.0).max(0.0), (s + along / 2.0).min(len)];
            fixed.push(Member::new(Kind::Post, Status::Fixed, (span[0] + span[1]) / 2.0, span[1] - span[0], full));
            post_spans.push(span);
            for c in [span[0] - f / 2.0, span[1] + f / 2.0] {
                fixed.push(Member::new(Kind::PostStud, Status::Fixed, c, f, full));
            }
        }
        // Walls meeting this one.
        for (s, oth) in junctions(w, walls) {
            let near_end = s < oth + f || s > len - oth - f;
            let kind = if near_end { Kind::CornerStud } else { Kind::IntersectionStud };
            let cs = if oth < 2.0 * f { vec![s] } else { vec![s - oth / 2.0 + f / 2.0, s + oth / 2.0 - f / 2.0] };
            for c in cs {
                fixed.push(Member::new(kind, Status::Fixed, c, f, full));
            }
        }
        // Openings.
        let mut heads: Vec<([f64; 2], f64)> = Vec::new();
        for o in &w.openings {
            let (s0, s1) = (o[0] * 12.0, o[1] * 12.0);
            let span = s1 - s0 + 2.0 * f;
            let hdr = headers.iter().find(|h| span <= h.max_span_in).or(headers.last());
            let depth = hdr.map_or(8.0, |h| sane(h.depth_in, 1.0, 48.0, 8.0));
            let head = DOOR_HEAD_IN.min(top - depth).max(0.0);
            fixed.push(Member::new(Kind::JackStud, Status::Fixed, s0 - f / 2.0, f, [0.0, head]));
            fixed.push(Member::new(Kind::JackStud, Status::Fixed, s1 + f / 2.0, f, [0.0, head]));
            fixed.push(Member::new(Kind::KingStud, Status::Fixed, s0 - 1.5 * f, f, full));
            fixed.push(Member::new(Kind::KingStud, Status::Fixed, s1 + 1.5 * f, f, full));
            let mut h = Member::new(Kind::Header, Status::Fixed, (s0 + s1) / 2.0, s1 - s0 + 2.0 * f, [head, head + depth]);
            h.note = match hdr {
                Some(r) if schedule_given => format!("{} (header schedule)", r.size),
                Some(r) => format!("{} (no header schedule: placeholder size)", r.size),
                None => String::new(),
            };
            fixed.push(h);
            heads.push(([s0, s1], head + depth));
        }
        // Backing from details.
        for bk in backing.iter().filter(|b| b.level == w.level) {
            let (sa, oa) = w.local(bk.from_ft);
            let (sb, ob) = w.local(bk.to_ft);
            if oa.abs() > w.thickness_in / 2.0 + 6.0 || ob.abs() > w.thickness_in / 2.0 + 6.0 {
                continue;
            }
            let (lo, hi) = (sa.min(sb).max(0.0), sa.max(sb).min(len));
            if hi - lo < 1.0 {
                continue;
            }
            let mut m = Member::new(Kind::Backing, Status::Fixed, (lo + hi) / 2.0, hi - lo, bk.z_in);
            m.note = bk.reason.clone();
            fixed.push(m);
        }
        // Keep members inside the wall; one stud per place.
        fixed.retain(|m| m.s_in - m.width_in / 2.0 >= -1e-6 && m.s_in + m.width_in / 2.0 <= len + 1e-6 && m.s_in.is_finite());
        fixed.sort_by(|a, b| a.s_in.total_cmp(&b.s_in).then(a.kind.cmp(&b.kind)));
        let mut kept: Vec<Member> = Vec::new();
        for m in fixed {
            let dup = m.kind.full_height()
                && kept.iter().any(|k| {
                    (k.kind.full_height() || k.kind == Kind::Post) && (k.s_in - m.s_in).abs() < (k.width_in + m.width_in) / 2.0 - OVERLAP_OK_IN
                });
            if !dup {
                kept.push(m);
            }
        }
        wf.members = kept;
        for end in [End::A, End::B] {
            let rule = layout_members(w, end, phase_of(w, end), &wf.members, &post_spans, &heads, Status::Rule);
            wf.members.extend(rule);
        }
        merge_agreed(&mut wf.members);
        wf.members.sort_by(|a, b| a.s_in.total_cmp(&b.s_in).then(a.kind.cmp(&b.kind)));
        if w.plumbing {
            wf.field_changes.push("plumbing wall: studs shift or double around drains and stacks".into());
        }
        out.walls.push(wf);
    }
    out.stacks = stacks(&out.walls);
    out
}

/// A layout's stud centres are `phase + k × spacing`; this is its phase for a layout pulled
/// from `end`.
fn phase_of(w: &FrameWall, end: End) -> f64 {
    let first = w.spacing_in - LAYOUT_OFFSET_IN + w.flange_in / 2.0;
    match end {
        End::A => first,
        End::B => w.len_in() - first,
    }
}

/// The field studs and cripples of a layout with centres at `phase + k × spacing`.
fn layout_members(
    w: &FrameWall,
    end: End,
    phase: f64,
    fixed: &[Member],
    posts: &[[f64; 2]],
    heads: &[([f64; 2], f64)],
    status: Status,
) -> Vec<Member> {
    let len = w.len_in();
    let (f, sp) = (w.flange_in, w.spacing_in);
    let top = w.height_ft * 12.0;
    let k0 = ((f - phase) / sp).ceil();
    let k1 = ((len - f - phase) / sp).floor();
    let mut out = Vec::new();
    if !(k0.is_finite() && k1.is_finite()) || k1 - k0 > 4000.0 {
        return out;
    }
    let mut k = k0;
    while k <= k1 {
        let c = phase + k * sp;
        k += 1.0;
        if c - f / 2.0 < f - 1e-6 || c + f / 2.0 > len - f + 1e-6 {
            continue;
        }
        if posts.iter().any(|p| c + f / 2.0 > p[0] && c - f / 2.0 < p[1]) {
            continue;
        }
        if let Some((_, z)) = heads.iter().find(|(o, _)| c > o[0] - 1e-6 && c < o[1] + 1e-6) {
            let mut m = Member::new(Kind::Cripple, status, c, f, [*z, top]);
            m.layout = (status == Status::Rule).then_some(end);
            if z < &top {
                out.push(m);
            }
            continue;
        }
        // A fixed stud already stands there.
        if fixed.iter().any(|x| {
            x.status == Status::Fixed
                && (x.kind.full_height() || x.kind == Kind::Post || x.kind == Kind::JackStud)
                && (x.s_in - c).abs() < (x.width_in + f) / 2.0
        }) {
            continue;
        }
        let mut m = Member::new(Kind::FieldStud, status, c, f, [0.0, top]);
        m.layout = (status == Status::Rule).then_some(end);
        out.push(m);
    }
    out
}

/// Rule members of the two layouts within [`AGREE_TOL_IN`] of each other become one agreed
/// member.
fn merge_agreed(ms: &mut Vec<Member>) {
    let a: Vec<usize> = (0..ms.len()).filter(|i| ms.get(*i).is_some_and(|m| m.layout == Some(End::A))).collect();
    let mut drop = vec![false; ms.len()];
    for i in a {
        let Some(ma) = ms.get(i).cloned() else { continue };
        let j = (0..ms.len()).find(|j| {
            !drop.get(*j).copied().unwrap_or(true)
                && ms.get(*j).is_some_and(|mb| mb.layout == Some(End::B) && mb.kind == ma.kind && (mb.s_in - ma.s_in).abs() <= AGREE_TOL_IN)
        });
        if let Some(j) = j {
            let sb = ms.get(j).map_or(ma.s_in, |m| m.s_in);
            if let Some(m) = ms.get_mut(i) {
                m.status = Status::Agreed;
                m.layout = None;
                m.s_in = (ma.s_in + sb) / 2.0;
            }
            if let Some(d) = drop.get_mut(j) {
                *d = true;
            }
        }
    }
    let mut i = 0;
    ms.retain(|_| {
        let keep = !drop.get(i).copied().unwrap_or(false);
        i += 1;
        keep
    });
}

/// Walls stacked storey over storey (same line, the same ends either way round).
fn stacks(walls: &[WallFrame]) -> Vec<Vec<usize>> {
    let near = |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).hypot(p[1] - q[1]) < 0.25;
    let mut seen = vec![false; walls.len()];
    let mut out = Vec::new();
    for i in 0..walls.len() {
        if seen.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(wi) = walls.get(i) else { continue };
        let mut g = vec![i];
        for (j, wj) in walls.iter().enumerate().skip(i + 1) {
            if seen.get(j).copied().unwrap_or(true) || wj.wall.level == wi.wall.level || wj.wall.stud_in.is_none() || wi.wall.stud_in.is_none() {
                continue;
            }
            let (a, b, c, d) = (wi.wall.a_ft, wi.wall.b_ft, wj.wall.a_ft, wj.wall.b_ft);
            if ((near(a, c) && near(b, d)) || (near(a, d) && near(b, c))) && (wi.wall.spacing_in - wj.wall.spacing_in).abs() < 1e-6 {
                g.push(j);
                if let Some(s) = seen.get_mut(j) {
                    *s = true;
                }
            }
        }
        if g.len() > 1 {
            out.push(g);
        }
    }
    out
}

// ---- The whole framing -------------------------------------------------------------------------

/// Every framed wall and bay.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Framing {
    pub walls: Vec<WallFrame>,
    /// Indices of walls stacked over each other (they share one layout).
    pub stacks: Vec<Vec<usize>>,
    pub bays: Vec<BayFrame>,
    /// Post-tensioning tendons' no-core zones: plan polygons by level.
    pub no_core: Vec<(String, Vec<[f64; 2]>)>,
}

/// What one field point told us.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldSnap {
    /// The layout it matches (none: neither end — pulled from somewhere else).
    pub matches: Option<End>,
    /// Off the nearer layout, inches.
    pub off_in: f64,
    /// The walls it snapped (the measured one and those stacked with it).
    pub walls: Vec<usize>,
}

/// A tape measurement: from an end of the wall to the near edge of a stud.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tape {
    pub from: End,
    pub to_near_edge_in: f64,
}

/// A stud picked on a site photo of the wall: three reference points along the wall picked in
/// the photo with their known positions (corners, jambs: inches from `a`), and the stud. The
/// mapping along a straight line in a perspective photo is projective, so three references fix
/// it exactly.
pub fn photo_to_wall(refs: [([f64; 2], f64); 3], pick: [f64; 2]) -> Option<f64> {
    let [(p0, s0), (p1, s1), (p2, s2)] = refs;
    let dir = [p2[0] - p0[0], p2[1] - p0[1]];
    let l2 = dir[0] * dir[0] + dir[1] * dir[1];
    if !(l2.is_finite() && l2 > 1e-12) {
        return None;
    }
    let t = |p: [f64; 2]| ((p[0] - p0[0]) * dir[0] + (p[1] - p0[1]) * dir[1]) / l2;
    let (t1, t2, tx) = (t(p1), 1.0, t(pick));
    // s = (α t + β) / (γ t + 1) through (0, s0), (t1, s1), (1, s2): β = s0, then two equations.
    let m = [[t1, -t1 * s1], [t2, -t2 * s2]];
    let r = [s1 - s0, s2 - s0];
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    if det.abs() < 1e-12 {
        return None;
    }
    let alpha = (r[0] * m[1][1] - m[0][1] * r[1]) / det;
    let gamma = (m[0][0] * r[1] - r[0] * m[1][0]) / det;
    let den = gamma * tx + 1.0;
    let s = (alpha * tx + s0) / den;
    (den.abs() > 1e-9 && s.is_finite()).then_some(s)
}

impl Framing {
    /// One measured stud centre on wall `wall` (inches from `a`): its run and the walls stacked
    /// with it snap from rule to verified.
    pub fn field_point(&mut self, wall: usize, centre_in: f64) -> Result<FieldSnap, String> {
        let w = self.walls.get(wall).ok_or_else(|| format!("no wall {wall}"))?;
        if w.wall.stud_in.is_none() {
            return Err(format!("{} has no studs", w.wall.id));
        }
        let len = w.wall.len_in();
        if !(centre_in.is_finite() && centre_in > 0.0 && centre_in < len) {
            return Err(format!("{} is not on the wall (0 to {})", inch(centre_in), inch(len)));
        }
        let sp = w.wall.spacing_in;
        let wrap = |d: f64| d - (d / sp).round() * sp;
        let offs = [End::A, End::B].map(|e| (e, wrap(centre_in - phase_of(&w.wall, e)).abs()));
        let (best, off) = offs.into_iter().fold((End::A, f64::MAX), |acc, x| if x.1 < acc.1 { x } else { acc });
        let matches = (off <= AGREE_TOL_IN).then_some(best);
        let world = w.wall.at(centre_in);
        let group = self.stack_of(wall);
        for i in &group {
            let Some(wf) = self.walls.get_mut(*i) else { continue };
            let (s, _) = wf.wall.local(world);
            relayout(wf, s, Status::FieldVerified);
            wf.settled = Some(match matches {
                Some(e) if *i == wall => format!("field verified: laid out from the {} end", e.letter()),
                Some(_) => "field verified on the wall stacked with it".into(),
                None => format!("field verified: neither end ({} off)", inch(off)),
            });
        }
        Ok(FieldSnap { matches, off_in: off, walls: group })
    }

    /// [`Self::field_point`] from a tape measurement.
    pub fn field_tape(&mut self, wall: usize, tape: Tape) -> Result<FieldSnap, String> {
        let w = self.walls.get(wall).ok_or_else(|| format!("no wall {wall}"))?;
        let f = w.wall.flange_in;
        let c = match tape.from {
            End::A => tape.to_near_edge_in + f / 2.0,
            End::B => w.wall.len_in() - tape.to_near_edge_in - f / 2.0,
        };
        self.field_point(wall, c)
    }

    fn stack_of(&self, wall: usize) -> Vec<usize> {
        self.stacks.iter().find(|g| g.contains(&wall)).cloned().unwrap_or_else(|| vec![wall])
    }

    /// Shop drawing studs (plan points, stud centres) replace the rule members of the walls they
    /// fall in. Returns how many walls changed.
    pub fn shop_studs(&mut self, level: &str, studs: &[[f64; 2]]) -> usize {
        let mut changed = 0;
        for wf in self.walls.iter_mut().filter(|w| w.wall.level == level && w.wall.stud_in.is_some()) {
            let len = wf.wall.len_in();
            let f = wf.wall.flange_in;
            let mut ss: Vec<f64> = studs
                .iter()
                .take(MAX_SHOP)
                .map(|p| wf.wall.local(*p))
                .filter(|(s, off)| off.abs() <= wf.wall.thickness_in / 2.0 + 1.0 && *s > f && *s < len - f)
                .map(|(s, _)| s)
                .collect();
            if ss.is_empty() {
                continue;
            }
            ss.sort_by(f64::total_cmp);
            wf.members.retain(|m| !matches!(m.status, Status::Rule | Status::Agreed | Status::ShopDrawing | Status::FieldVerified));
            let heads = heads_of(wf);
            let posts: Vec<[f64; 2]> =
                wf.members.iter().filter(|m| m.kind == Kind::Post).map(|m| [m.s_in - m.width_in / 2.0, m.s_in + m.width_in / 2.0]).collect();
            let top = wf.wall.height_ft * 12.0;
            let mut add = Vec::new();
            for c in ss {
                if posts.iter().any(|p| c > p[0] && c < p[1]) {
                    continue;
                }
                if let Some((_, z)) = heads.iter().find(|(o, _)| c > o[0] - 1e-6 && c < o[1] + 1e-6) {
                    add.push(Member::new(Kind::Cripple, Status::ShopDrawing, c, f, [*z, top]));
                } else if !wf
                    .members
                    .iter()
                    .any(|x| (x.kind.full_height() || x.kind == Kind::JackStud) && (x.s_in - c).abs() < (x.width_in + f) / 2.0)
                {
                    add.push(Member::new(Kind::FieldStud, Status::ShopDrawing, c, f, [0.0, top]));
                }
            }
            wf.members.extend(add);
            wf.members.sort_by(|a, b| a.s_in.total_cmp(&b.s_in).then(a.kind.cmp(&b.kind)));
            wf.settled = Some("shop drawing".into());
            changed += 1;
        }
        changed
    }

    /// Field-change flags from drains and stacks (plan points with a label) and recessed items
    /// (cabinets, panels, in-wall tanks) near the walls.
    pub fn flag_field_changes(&mut self, level: &str, drains: &[(String, [f64; 2])], recessed: &[(String, [f64; 2])]) {
        for wf in self.walls.iter_mut().filter(|w| w.wall.level == level) {
            let len = wf.wall.len_in();
            for (what, list) in [("drain/stack", drains), ("recessed", recessed)] {
                for (label, p) in list.iter().take(MAX_SHOP) {
                    let (s, off) = wf.wall.local(*p);
                    if off.abs() <= wf.wall.thickness_in / 2.0 + 9.0 && s > 0.0 && s < len {
                        let note = format!("{what} {label} at {}: studs shift or double here", inch(s));
                        if !wf.field_changes.contains(&note) {
                            wf.field_changes.push(note);
                        }
                    }
                }
            }
        }
    }

    /// The stud a device placed at `s_in` on wall `wall` mounts to: the nearest stud within
    /// `zone_in`, settled ones first; for open walls, each layout's.
    pub fn snap_device(&self, wall: usize, s_in: f64, zone_in: f64, drains: &[(String, [f64; 2])]) -> Option<DeviceSnap> {
        let wf = self.walls.get(wall)?;
        let studs = |pred: &dyn Fn(&Member) -> bool| {
            wf.members
                .iter()
                .filter(|m| m.kind.full_height() && pred(m) && (m.s_in - s_in).abs() <= zone_in)
                .min_by(|a, b| (a.s_in - s_in).abs().total_cmp(&(b.s_in - s_in).abs()))
                .cloned()
        };
        let sure = studs(&|m| m.status.sure());
        let a = studs(&|m| m.in_layout(End::A));
        let b = studs(&|m| m.in_layout(End::B));
        let pick = match (&sure, &a, &b) {
            (_, Some(x), Some(y)) if (x.s_in - y.s_in).abs() <= AGREE_TOL_IN => Some(x.clone()),
            (Some(x), _, _) => Some(x.clone()),
            _ => None,
        };
        let drain = |c: f64| {
            drains
                .iter()
                .take(MAX_SHOP)
                .map(|(l, p)| (l, wf.wall.local(*p)))
                .filter(|(_, (_, off))| off.abs() <= wf.wall.thickness_in / 2.0 + 24.0)
                .map(|(l, (s, _))| (l.clone(), (s - c).abs()))
                .min_by(|x, y| x.1.total_cmp(&y.1))
        };
        let words = |m: &Member| {
            let mut s = format!("lands on the {} at {} from the {} end", m.kind.words(), inch(m.s_in), "a");
            if let Some((l, d)) = drain(m.s_in) {
                s.push_str(&format!(", {} from the {l}", inch(d)));
            }
            s
        };
        match pick {
            Some(m) => Some(DeviceSnap { text: format!("{} ({})", words(&m), m.status.words()), stud: Some(m), by_layout: [None, None] }),
            None if a.is_some() || b.is_some() => {
                let t = |m: &Option<Member>, e: End| {
                    m.as_ref().map_or(format!("no stud within {} if laid out from the {} end", inch(zone_in), e.letter()), |m| {
                        format!("{} if laid out from the {} end", words(m), e.letter())
                    })
                };
                Some(DeviceSnap { text: format!("{}; {}", t(&a, End::A), t(&b, End::B)), stud: None, by_layout: [a, b] })
            }
            None => None,
        }
    }

    /// Takeoff: members by status and kind (rule members counted per layout).
    pub fn takeoff(&self) -> Takeoff {
        let mut t = Takeoff::default();
        for m in self.walls.iter().flat_map(|w| &w.members).chain(self.bays.iter().flat_map(|b| &b.members)) {
            let key = (m.status, m.kind);
            match m.layout {
                Some(End::A) => *t.rule_a.entry(m.kind).or_default() += 1,
                Some(End::B) => *t.rule_b.entry(m.kind).or_default() += 1,
                None => *t.counts.entry(key).or_default() += 1,
            }
        }
        t
    }
}

/// The walls' framing as the building spec's takeoff rows (what the job's bill of materials
/// counts): members by level, kind, section and length, with the doctrine's status (fixed, rule —
/// a one-layout or agreed rule member —, field verified; a shop drawing's member is fixed). A wall
/// whose layout is still open orders its larger layout's rule members.
pub fn spec_rows(fr: &Framing) -> Vec<cadaddy_walk3d::FramingRow> {
    let mut rows: std::collections::BTreeMap<(String, String, String, i64, &'static str), u32> = std::collections::BTreeMap::new();
    for wf in &fr.walls {
        let open = wf.members.iter().any(|m| m.layout.is_some());
        let end = if open && wf.resolved(End::B).count() > wf.resolved(End::A).count() { End::B } else { End::A };
        let depth = wf.wall.stud_in.map_or_else(|| "solid".to_string(), inch);
        for m in wf.resolved(end) {
            let status = match m.status {
                Status::Fixed | Status::ShopDrawing => "fixed",
                Status::Agreed | Status::Rule => "rule",
                Status::FieldVerified => "field_verified",
            };
            let (section, length_in) = match m.kind {
                Kind::Header | Kind::Backing | Kind::Rim => (format!("{depth} {}", m.kind.words()), m.width_in),
                Kind::Post => ("post".to_string(), m.z_in[1] - m.z_in[0]),
                _ => (format!("{depth} stud"), m.z_in[1] - m.z_in[0]),
            };
            if !length_in.is_finite() || length_in <= 0.0 {
                continue;
            }
            let key = (wf.wall.level.clone(), m.kind.words().to_string(), section, (length_in * 8.0).round() as i64, status);
            *rows.entry(key).or_default() += 1;
        }
    }
    rows.into_iter()
        .map(|((level, member, section, eighths, status), count)| cadaddy_walk3d::FramingRow {
            level,
            member,
            section,
            length_in: eighths as f64 / 8.0,
            count,
            status: status.into(),
        })
        .collect()
}

fn heads_of(wf: &WallFrame) -> Vec<([f64; 2], f64)> {
    let f = wf.wall.flange_in;
    wf.members
        .iter()
        .filter(|m| m.kind == Kind::Header)
        .map(|m| ([m.s_in - m.width_in / 2.0 + f, m.s_in + m.width_in / 2.0 - f], m.z_in[1]))
        .collect()
}

/// Replace a wall's rule (and earlier shop/verified) members with a layout through `centre_in`.
fn relayout(wf: &mut WallFrame, centre_in: f64, status: Status) {
    wf.members.retain(|m| !matches!(m.status, Status::Rule | Status::Agreed | Status::ShopDrawing | Status::FieldVerified));
    let posts: Vec<[f64; 2]> =
        wf.members.iter().filter(|m| m.kind == Kind::Post).map(|m| [m.s_in - m.width_in / 2.0, m.s_in + m.width_in / 2.0]).collect();
    let heads = heads_of(wf);
    let add = layout_members(&wf.wall, End::A, centre_in, &wf.members, &posts, &heads, status);
    wf.members.extend(add);
    wf.members.sort_by(|a, b| a.s_in.total_cmp(&b.s_in).then(a.kind.cmp(&b.kind)));
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceSnap {
    /// The stud when it's settled (or both layouts agree).
    pub stud: Option<Member>,
    /// Otherwise each layout's nearest stud.
    pub by_layout: [Option<Member>; 2],
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Takeoff {
    pub counts: std::collections::BTreeMap<(Status, Kind), usize>,
    pub rule_a: std::collections::BTreeMap<Kind, usize>,
    pub rule_b: std::collections::BTreeMap<Kind, usize>,
}

impl Takeoff {
    pub fn by_status(&self, s: Status) -> usize {
        self.counts.iter().filter(|((st, _), _)| *st == s).map(|(_, n)| n).sum()
    }
    /// Rule members to order: the larger layout.
    pub fn rule_max(&self) -> usize {
        self.rule_a.values().sum::<usize>().max(self.rule_b.values().sum())
    }
}

// ---- Device boxes and crossings ----------------------------------------------------------------

/// A box deeper than the stud cavity.
pub fn box_depth_flag(w: &FrameWall, box_depth_in: f64) -> Option<String> {
    let d = w.stud_in?;
    (box_depth_in > d + 1e-9).then(|| format!("{} deep box won't fit a {} stud cavity", inch(box_depth_in), inch(d)))
}

/// A horizontal raceway in plan crossing the walls of `level`: inside a wall it bores through
/// every stud it passes (checked against the stud's limit; metal studs: punchouts); across a wall
/// it may land on a stud (flagged, per layout).
pub fn wall_crossings(fr: &Framing, level: &str, a_ft: [f64; 2], b_ft: [f64; 2], r: Raceway) -> Vec<WallCrossing> {
    let mut out = Vec::new();
    for (i, wf) in fr.walls.iter().enumerate().filter(|(_, w)| w.wall.level == level) {
        let Some(depth) = wf.wall.stud_in else { continue };
        let (sa, oa) = wf.wall.local(a_ft);
        let (sb, ob) = wf.wall.local(b_ft);
        let half = wf.wall.thickness_in / 2.0;
        let len = wf.wall.len_in();
        let member = bore::Member::MetalStud { depth_in: depth, punchout_in: if depth >= 3.5 { 1.5 } else { 0.75 } };
        if oa.abs() <= half && ob.abs() <= half {
            let (lo, hi) = (sa.min(sb), sa.max(sb));
            let n = |e: End| wf.resolved(e).filter(|m| m.kind.full_height() && m.s_in > lo && m.s_in < hi).count();
            if hi > 0.0 && lo < len {
                out.push(WallCrossing {
                    wall: i,
                    along: true,
                    studs: [n(End::A), n(End::B)],
                    check: Some(bore::check(r, &member, HoleAt { centre_from_edge_in: None, from_support_ft: None }, bore::CLEARANCE_IN)),
                    note: String::new(),
                });
            }
        } else if oa.signum() != ob.signum() && (oa - ob).abs() > 1e-9 {
            let t = oa / (oa - ob);
            let s = sa + (sb - sa) * t;
            if s <= 0.0 || s >= len {
                continue;
            }
            let rad = r.od_in().unwrap_or(0.5) / 2.0;
            let hits = |e: End| wf.resolved(e).find(|m| m.kind.full_height() && (m.s_in - s).abs() < m.width_in / 2.0 + rad).map(|m| m.s_in);
            let (ha, hb) = (hits(End::A), hits(End::B));
            let note = match (ha, hb) {
                (Some(x), Some(_)) => format!("crossing at {} hits the stud at {} either way: shift it", inch(s), inch(x)),
                (Some(x), None) => format!("crossing at {} hits the stud at {} if laid out from the a end", inch(s), inch(x)),
                (None, Some(x)) => format!("crossing at {} hits the stud at {} if laid out from the b end", inch(s), inch(x)),
                (None, None) => String::new(),
            };
            out.push(WallCrossing { wall: i, along: false, studs: [usize::from(ha.is_some()), usize::from(hb.is_some())], check: None, note });
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WallCrossing {
    pub wall: usize,
    /// Runs inside the wall (through its studs) rather than across it.
    pub along: bool,
    /// Studs crossed (inside) or hit (across) under each layout.
    pub studs: [usize; 2],
    pub check: Option<BoreCheck>,
    pub note: String,
}

// ---- Joist bays --------------------------------------------------------------------------------

/// A framed floor or roof bay: joists (or trusses) between two supports at a spacing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JoistBay {
    pub id: String,
    pub level: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Joists run along x (laid out along y), else along y.
    pub along_x: bool,
    pub spacing_in: f64,
    /// Width of a joist, inches.
    pub width_in: f64,
    pub member: bore::Member,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BayFrame {
    pub bay: JoistBay,
    /// `s_in` from the bay's low side across the joists; `z_in` the member depth band.
    pub members: Vec<Member>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<String>,
}

fn bay_width_in(b: &JoistBay) -> f64 {
    let w = if b.along_x { b.max_ft[1] - b.min_ft[1] } else { b.max_ft[0] - b.min_ft[0] };
    (w * 12.0).clamp(0.0, MAX_LEN_IN)
}

fn bay_s(b: &JoistBay, p: [f64; 2]) -> f64 {
    if b.along_x { (p[1] - b.min_ft[1]) * 12.0 } else { (p[0] - b.min_ft[0]) * 12.0 }
}

/// Frame bays: rims at both sides, beams running with the joists (plan lines), both layouts.
pub fn frame_bays(bays: &[JoistBay], beams: &[([f64; 2], [f64; 2], f64)]) -> Vec<BayFrame> {
    let mut out = Vec::new();
    for b in bays.iter().take(MAX_WALLS) {
        let wd = bay_width_in(b);
        let w = sane(b.width_in, 0.5, 12.0, 1.5);
        let sp = sane(b.spacing_in, 8.0, 96.0, 16.0);
        let depth = match &b.member {
            bore::Member::SawnJoist { depth_in } | bore::Member::IJoist { depth_in, .. } => *depth_in,
            _ => 12.0,
        };
        let z = [0.0, depth];
        let mut ms = vec![Member::new(Kind::Rim, Status::Fixed, w / 2.0, w, z), Member::new(Kind::Rim, Status::Fixed, wd - w / 2.0, w, z)];
        for (p, q, fl) in beams {
            let parallel = if b.along_x { (p[1] - q[1]).abs() < 0.1 } else { (p[0] - q[0]).abs() < 0.1 };
            let s = bay_s(b, *p);
            if parallel && s > 0.0 && s < wd && fl.is_finite() {
                ms.push(Member::new(Kind::Beam, Status::Fixed, s, fl.clamp(1.0, 24.0), z));
            }
        }
        for (end, phase) in [(End::A, sp - LAYOUT_OFFSET_IN + w / 2.0), (End::B, wd - (sp - LAYOUT_OFFSET_IN + w / 2.0))] {
            let k0 = ((w - phase) / sp).ceil();
            let k1 = ((wd - w - phase) / sp).floor();
            if !(k0.is_finite() && k1.is_finite()) || k1 - k0 > 4000.0 {
                continue;
            }
            let mut k = k0;
            while k <= k1 {
                let c = phase + k * sp;
                k += 1.0;
                if c - w / 2.0 < w - 1e-6 || c + w / 2.0 > wd - w + 1e-6 {
                    continue;
                }
                if ms.iter().any(|m| m.status == Status::Fixed && (m.s_in - c).abs() < (m.width_in + w) / 2.0) {
                    continue;
                }
                let mut m = Member::new(Kind::Joist, Status::Rule, c, w, z);
                m.layout = Some(end);
                ms.push(m);
            }
        }
        merge_agreed(&mut ms);
        ms.sort_by(|a, b| a.s_in.total_cmp(&b.s_in));
        out.push(BayFrame { bay: b.clone(), members: ms, settled: None });
    }
    out
}

impl BayFrame {
    /// A recessed can (or anything round in the ceiling) at a plan point: whether it falls
    /// clear between members under each layout, and what it hits.
    pub fn can_check(&self, at_ft: [f64; 2], dia_in: f64) -> [Result<(), String>; 2] {
        let s = bay_s(&self.bay, at_ft);
        let r = dia_in.max(0.0) / 2.0;
        [End::A, End::B].map(|e| match self.members.iter().filter(|m| m.in_layout(e)).find(|m| (m.s_in - s).abs() < m.width_in / 2.0 + r) {
            Some(m) => Err(format!("{} can hits the {} at {} if laid out from the {} side", inch(dia_in), m.kind.words(), inch(m.s_in), e.letter())),
            None => Ok(()),
        })
    }

    /// A raceway across the joists (plan line): members crossed under each layout and the bore
    /// check for them.
    pub fn crossing(&self, a_ft: [f64; 2], b_ft: [f64; 2], r: Raceway, at: HoleAt) -> ([usize; 2], BoreCheck) {
        let (sa, sb) = (bay_s(&self.bay, a_ft), bay_s(&self.bay, b_ft));
        let (lo, hi) = (sa.min(sb), sa.max(sb));
        let n = |e: End| self.members.iter().filter(|m| m.in_layout(e) && m.kind != Kind::Beam && m.s_in > lo && m.s_in < hi).count();
        ([n(End::A), n(End::B)], bore::check(r, &self.bay.member, at, bore::CLEARANCE_IN))
    }

    /// Shop drawing joists or trusses (plan lines) replace the rule joists.
    pub fn shop_joists(&mut self, lines: &[([f64; 2], [f64; 2])]) -> usize {
        let wd = bay_width_in(&self.bay);
        let w = self.bay.width_in;
        let ss: Vec<f64> =
            lines.iter().take(MAX_SHOP).map(|(p, q)| (bay_s(&self.bay, *p) + bay_s(&self.bay, *q)) / 2.0).filter(|s| *s > w && *s < wd - w).collect();
        if ss.is_empty() {
            return 0;
        }
        self.members.retain(|m| m.status == Status::Fixed);
        let z = self.members.first().map_or([0.0, 12.0], |m| m.z_in);
        for s in &ss {
            if !self.members.iter().any(|m| (m.s_in - s).abs() < (m.width_in + w) / 2.0) {
                self.members.push(Member::new(Kind::Joist, Status::ShopDrawing, *s, w, z));
            }
        }
        self.members.sort_by(|a, b| a.s_in.total_cmp(&b.s_in));
        self.settled = Some("shop drawing".into());
        ss.len()
    }
}

// ---- Shop drawings -----------------------------------------------------------------------------

/// What a shop drawing gives: stud centres, joist/truss lines and tendons, in building feet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShopDrawing {
    pub studs: Vec<[f64; 2]>,
    pub joists: Vec<([f64; 2], [f64; 2])>,
    pub tendons: Vec<Vec<[f64; 2]>>,
}

/// Read a shop drawing (DXF): layers with `STUD` (small rectangles or short lines: their
/// centres), `JOIST`/`TRUSS` (lines), `TENDON`/`PT` (polylines). `to_building` maps its
/// coordinates to building feet.
pub fn read_shop_drawing(d: &Drawing, to_building: &dyn Fn([f64; 2]) -> [f64; 2]) -> ShopDrawing {
    let mut out = ShopDrawing::default();
    for e in d.model.iter().take(MAX_SHOP) {
        let l = e.common.layer.to_ascii_uppercase();
        let pts: Vec<[f64; 2]> = match &e.kind {
            EntityKind::Line(x) => vec![[x.a.x, x.a.y], [x.b.x, x.b.y]],
            EntityKind::LwPolyline(p) => p.vertices.iter().take(10_000).map(|v| [v.p.x, v.p.y]).collect(),
            _ => continue,
        };
        let pts: Vec<[f64; 2]> = pts.into_iter().map(to_building).filter(|p| p.iter().all(|v| v.is_finite())).collect();
        if pts.is_empty() {
            continue;
        }
        if l.contains("TENDON") || l.contains("-PT") || l == "PT" {
            out.tendons.push(pts);
        } else if l.contains("JOIST") || l.contains("TRUSS") {
            if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
                out.joists.push((*a, *b));
            }
        } else if l.contains("STUD") {
            let n = pts.len() as f64;
            out.studs.push([pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n]);
        }
    }
    out
}

impl Framing {
    /// Apply a shop drawing on `level`: studs into walls, joists into bays, tendons as no-core
    /// zones ([`TENDON_CLEAR_IN`] each side).
    pub fn apply_shop_drawing(&mut self, level: &str, sd: &ShopDrawing) {
        self.shop_studs(level, &sd.studs);
        for b in self.bays.iter_mut().filter(|b| b.bay.level == level) {
            let (lo, hi) = (b.bay.min_ft, b.bay.max_ft);
            let inside: Vec<([f64; 2], [f64; 2])> = sd
                .joists
                .iter()
                .filter(|(p, q)| {
                    let m = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
                    m[0] >= lo[0] && m[0] <= hi[0] && m[1] >= lo[1] && m[1] <= hi[1]
                })
                .copied()
                .collect();
            b.shop_joists(&inside);
        }
        let c = TENDON_CLEAR_IN / 12.0;
        for t in &sd.tendons {
            for w in t.windows(2) {
                let (Some(p), Some(q)) = (w.first(), w.get(1)) else { continue };
                let l = (q[0] - p[0]).hypot(q[1] - p[1]);
                if l < 1e-9 {
                    continue;
                }
                let n = [-(q[1] - p[1]) / l * c, (q[0] - p[0]) / l * c];
                self.no_core.push((
                    level.into(),
                    vec![[p[0] + n[0], p[1] + n[1]], [q[0] + n[0], q[1] + n[1]], [q[0] - n[0], q[1] - n[1]], [p[0] - n[0], p[1] - n[1]]],
                ));
            }
        }
    }

    /// A core (or post-installed anchor) at a plan point lands in a tendon's no-core zone.
    pub fn in_no_core(&self, level: &str, p: [f64; 2]) -> bool {
        self.no_core.iter().filter(|(l, _)| l == level).any(|(_, poly)| inside(poly, p))
    }
}

fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n.max(1))) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

// ---- The fixtures' truth -----------------------------------------------------------------------

/// Which end each wall's studs were laid out from: one draw per stack of walls (they share it),
/// from a stream of its own so nothing else in a set changes.
pub fn choose_layouts(t: &mut Truth) {
    let mut r = Rng::new(t.seed ^ 0x4652_414D_494E_4721).fork(6);
    let fw = walls_from_truth(t);
    let fr = frame(&fw, &[], &[], &[]);
    let mut done = vec![false; t.plan_walls.len()];
    let groups: Vec<Vec<usize>> =
        (0..t.plan_walls.len()).map(|i| fr.stacks.iter().find(|g| g.contains(&i)).cloned().unwrap_or_else(|| vec![i])).collect();
    for (i, g) in groups.iter().enumerate() {
        if done.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(first) = fw.get(i) else { continue };
        let end = if r.chance(0.5) { End::A } else { End::B };
        let world = first.end_pt(end);
        for j in g {
            let Some(wj) = fw.get(*j) else { continue };
            let e = if (wj.a_ft[0] - world[0]).hypot(wj.a_ft[1] - world[1]) < (wj.b_ft[0] - world[0]).hypot(wj.b_ft[1] - world[1]) {
                End::A
            } else {
                End::B
            };
            if let Some(w) = t.plan_walls.get_mut(*j) {
                w.layout_from = Some(e.letter().into());
            }
            if let Some(d) = done.get_mut(*j) {
                *d = true;
            }
        }
    }
}

/// The true framing of a fixture set: every wall resolved to the layout it was pulled from.
pub fn truth_framing(t: &Truth) -> Framing {
    let mut fr = frame(&walls_from_truth(t), &posts_from_truth(t), &[], &[]);
    for (wf, w) in fr.walls.iter_mut().zip(&t.plan_walls) {
        if let Some(e) = w.layout_from.as_deref().and_then(End::parse) {
            wf.members.retain(|m| m.in_layout(e));
            for m in wf.members.iter_mut().filter(|m| m.layout.is_some()) {
                m.layout = None;
            }
            wf.settled = Some(format!("laid out from the {} end", e.letter()));
        }
    }
    fr
}

/// Read a fixture set's walls and structure, then frame them.
pub fn read_fixture_framing(seed: u64) -> Result<(Truth, Framing), String> {
    let set = crate::generate(seed)?;
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = crate::sheet_reader::recognise_fixture_set(&dxf);
    let ceilings = crate::ceiling_reader::read_ceilings(&dxf, &map);
    let drawings: Vec<(String, Drawing)> =
        dxf.iter().filter_map(|(n, b)| cadaddy_io::read_dxf(b).ok().map(|d| (n.trim_end_matches(".dxf").to_string(), d))).collect();
    let refs: Vec<(String, &Drawing)> = drawings.iter().map(|(n, d)| (n.clone(), d)).collect();
    let structure = crate::struct_reader::read_structure(&map, &refs);
    let walls = crate::wall_reader::read_walls(&dxf, &map, &ceilings, &structure);
    let fr = frame(&walls_from_reader(&walls), &posts_from_structure(&structure), &[], &[]);
    Ok((set.truth, fr))
}

// ---- Scoring -----------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FramingScore {
    pub walls_total: usize,
    pub walls_matched: usize,
    /// True fixed members, and those found within 1/8" as fixed.
    pub fixed_total: usize,
    pub fixed_found: usize,
    /// Read fixed members with no true one there.
    pub fixed_extra: usize,
    /// True rule members (the layout actually pulled), and those found within 1/8" under the
    /// read wall's matching layout (or as shop drawing / field verified members).
    pub rule_total: usize,
    pub rule_found: usize,
    /// Read rule members of the right layout with no true member there.
    pub rule_extra: usize,
    /// Worst miss of a matched member, inches.
    pub worst_in: f64,
}

/// Score read framing against the truth: walls matched by line; members compared in plan.
pub fn score_framing(t: &Truth, truth: &Framing, read: &Framing) -> FramingScore {
    const TOL: f64 = 0.125;
    let mut sc = FramingScore::default();
    for (tw, w) in truth.walls.iter().zip(&t.plan_walls) {
        if tw.wall.stud_in.is_none() {
            continue;
        }
        sc.walls_total += 1;
        let tl = tw.wall.len_in();
        let Some(rw) = read.walls.iter().filter(|r| r.wall.level == tw.wall.level && r.wall.stud_in.is_some()).find(|r| {
            let (s0, o0) = tw.wall.local(r.wall.a_ft);
            let (s1, o1) = tw.wall.local(r.wall.b_ft);
            o0.abs() < 6.0 && o1.abs() < 6.0 && s0.max(s1).min(tl) - s0.min(s1).max(0.0) > 0.5 * tl
        }) else {
            continue;
        };
        sc.walls_matched += 1;
        // The read wall's layout matching the truth's pulled end.
        let Some(te) = w.layout_from.as_deref().and_then(End::parse) else { continue };
        let world = tw.wall.end_pt(te);
        let re = if (rw.wall.a_ft[0] - world[0]).hypot(rw.wall.a_ft[1] - world[1]) < (rw.wall.b_ft[0] - world[0]).hypot(rw.wall.b_ft[1] - world[1]) {
            End::A
        } else {
            End::B
        };
        let to_truth = |m: &Member| tw.wall.local(rw.wall.at(m.s_in)).0;
        let read_fixed: Vec<(Kind, f64)> = rw.members.iter().filter(|m| m.status == Status::Fixed).map(|m| (m.kind, to_truth(m))).collect();
        let read_rule: Vec<(Kind, f64)> =
            rw.members.iter().filter(|m| m.status != Status::Fixed && m.in_layout(re)).map(|m| (m.kind, to_truth(m))).collect();
        let true_fixed: Vec<&Member> = tw.members.iter().filter(|m| m.status == Status::Fixed).collect();
        let true_rule: Vec<&Member> = tw.members.iter().filter(|m| m.status != Status::Fixed).collect();
        let mut hit = |list: &[(Kind, f64)], m: &Member| {
            let best = list.iter().filter(|(k, _)| *k == m.kind).map(|(_, s)| (s - m.s_in).abs()).reduce(f64::min);
            if let Some(b) = best
                && b <= 1.0
            {
                sc.worst_in = sc.worst_in.max(b);
            }
            best.is_some_and(|b| b <= TOL)
        };
        for m in &true_fixed {
            sc.fixed_total += 1;
            sc.fixed_found += usize::from(hit(&read_fixed, m));
        }
        for m in &true_rule {
            sc.rule_total += 1;
            sc.rule_found += usize::from(hit(&read_rule, m));
        }
        let extra = |list: &[(Kind, f64)], truths: &[&Member]| {
            list.iter().filter(|(k, s)| !truths.iter().any(|m| m.kind == *k && (m.s_in - s).abs() <= TOL)).count()
        };
        sc.fixed_extra += extra(&read_fixed, &true_fixed);
        sc.rule_extra += extra(&read_rule, &true_rule);
    }
    sc
}

// ---- Drawings ----------------------------------------------------------------------------------

/// Layers: fixed members solid, rule members ghosted (grey, dashed; one linetype per layout).
fn layer_for(m: &Member) -> &'static str {
    match (m.status, m.layout) {
        (Status::Fixed, _) => "FRAMING-FIXED",
        (Status::Agreed, _) => "FRAMING-AGREED",
        (Status::Rule, Some(End::B)) => "FRAMING-RULE-B",
        (Status::Rule, _) => "FRAMING-RULE-A",
        (Status::ShopDrawing, _) => "FRAMING-SHOP",
        (Status::FieldVerified, _) => "FRAMING-VERIFIED",
    }
}

fn new_drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    for lt in cadaddy_doc::library::standard_linetypes() {
        if ["HIDDEN", "DASHED"].contains(&lt.name.as_str()) && !d.linetypes.iter().any(|l| l.name.eq_ignore_ascii_case(&lt.name)) {
            d.linetypes.push(lt);
        }
    }
    // Name, colour, pen (hundredths of mm), linetype.
    for (name, color, pen, lt) in [
        ("FRAMING-FIXED", 7u8, 50u16, "Continuous"),
        ("FRAMING-AGREED", 5, 35, "Continuous"),
        ("FRAMING-RULE-A", 8, 18, "DASHED"),
        ("FRAMING-RULE-B", 9, 18, "HIDDEN"),
        ("FRAMING-SHOP", 4, 35, "Continuous"),
        ("FRAMING-VERIFIED", 3, 35, "Continuous"),
        ("FRAMING-WALL", 8, 13, "Continuous"),
        ("FRAMING-DIMS", 7, 18, "Continuous"),
        ("FRAMING-FLAGS", 1, 25, "Continuous"),
    ] {
        let mut l = Layer::new(name);
        l.color = Color::Index(color);
        l.lineweight = Lineweight::Mm100(pen);
        l.linetype = lt.into();
        d.layers.push(l);
    }
    d
}

fn add(d: &mut Drawing, layer: &str, kind: EntityKind, ltscale: f64) {
    let _ = d.add(&Space::Model, Common { layer: layer.into(), ltscale, ..Common::default() }, kind);
}

fn rect(d: &mut Drawing, layer: &str, pts: [[f64; 2]; 4], ltscale: f64) {
    let vertices = pts.iter().map(|p| PolyVertex { p: Vec2::new(p[0], p[1]), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
    add(d, layer, EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }), ltscale);
}

fn text(d: &mut Drawing, layer: &str, at: [f64; 2], h: f64, s: &str, rot: f64, ha: HAlign) {
    let p = Vec3::new(at[0], at[1], 0.0);
    add(
        d,
        layer,
        EntityKind::Text(Text {
            insert: p,
            align_pt: Some(p),
            height: h,
            value: s.into(),
            rotation: rot,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: ha,
            valign: VAlign::Middle,
        }),
        1.0,
    );
}

/// A wall's framing elevation (inches; x along from `a`, y up), seen from its left side: every
/// member, rule members of both layouts ghosted, the "next stud" dimension at each end, opening
/// and field-change notes.
pub fn draw_elevation(wf: &WallFrame) -> Drawing {
    let mut d = new_drawing();
    let len = wf.wall.len_in();
    let top = wf.wall.height_ft * 12.0;
    rect(&mut d, "FRAMING-WALL", [[0.0, 0.0], [len, 0.0], [len, top], [0.0, top]], 4.0);
    for m in &wf.members {
        let (x0, x1) = (m.s_in - m.width_in / 2.0, m.s_in + m.width_in / 2.0);
        // The two layouts drawn a little apart in height so both show where they overlap.
        let (z0, z1) = match m.layout {
            Some(End::A) => (m.z_in[0] + 2.0, m.z_in[1] - 2.0),
            Some(End::B) => (m.z_in[0] + 4.0, m.z_in[1] - 4.0),
            None => (m.z_in[0], m.z_in[1]),
        };
        rect(&mut d, layer_for(m), [[x0, z0], [x1, z0], [x1, z1], [x0, z1]], 4.0);
    }
    let h = 3.0;
    let lines = [wf.next_stud_words(End::A), wf.next_stud_words(End::B)];
    text(
        &mut d,
        "FRAMING-DIMS",
        [0.0, -8.0],
        h,
        &format!("{}  {}  {}  {}", wf.wall.id, wf.wall.wall_type, inch(len), wf.settled.as_deref().unwrap_or("layout end unknown: both shown")),
        0.0,
        HAlign::Left,
    );
    text(&mut d, "FRAMING-DIMS", [0.0, -14.0], h, lines.first().map_or("", |s| s.as_str()), 0.0, HAlign::Left);
    text(&mut d, "FRAMING-DIMS", [0.0, -20.0], h, lines.get(1).map_or("", |s| s.as_str()), 0.0, HAlign::Left);
    // Tick marks under each member, a dimension line along the bottom.
    add(&mut d, "FRAMING-DIMS", EntityKind::Line(Line { a: Vec3::new(0.0, -3.0, 0.0), b: Vec3::new(len, -3.0, 0.0) }), 1.0);
    for m in wf.members.iter().filter(|m| m.kind.full_height()) {
        let dz = if m.layout == Some(End::B) { -1.5 } else { 0.0 };
        add(&mut d, layer_for(m), EntityKind::Line(Line { a: Vec3::new(m.s_in, -1.5 + dz, 0.0), b: Vec3::new(m.s_in, -4.5 + dz, 0.0) }), 1.0);
    }
    for (i, n) in wf.field_changes.iter().chain(&wf.flags).enumerate() {
        text(&mut d, "FRAMING-FLAGS", [0.0, -26.0 - 6.0 * i as f64], h, n, 0.0, HAlign::Left);
    }
    for m in wf.members.iter().filter(|m| m.kind == Kind::Header && !m.note.is_empty()) {
        text(&mut d, "FRAMING-DIMS", [m.s_in, m.z_in[1] + 4.0], 2.0, &m.note, 0.0, HAlign::Center);
    }
    d
}

/// One level's framing plan (inches, building coordinates × 12): walls outlined, every stud as
/// a small rectangle across the wall, rule members ghosted.
pub fn draw_plan(fr: &Framing, level: &str) -> Drawing {
    let mut d = new_drawing();
    for wf in fr.walls.iter().filter(|w| w.wall.level == level) {
        let w = &wf.wall;
        let u = w.dir();
        let n = [-u[1], u[0]];
        let half = w.thickness_in / 2.0;
        let pt = |s: f64, off: f64| {
            let p = w.at(s);
            [p[0] * 12.0 + n[0] * off, p[1] * 12.0 + n[1] * off]
        };
        let len = w.len_in();
        rect(&mut d, "FRAMING-WALL", [pt(0.0, -half), pt(len, -half), pt(len, half), pt(0.0, half)], 12.0);
        let depth = w.stud_in.unwrap_or(0.0) / 2.0;
        for m in wf.members.iter().filter(|m| m.kind.full_height() || m.kind == Kind::Post || m.kind == Kind::JackStud) {
            let (a, b) = (m.s_in - m.width_in / 2.0, m.s_in + m.width_in / 2.0);
            let dd = if m.kind == Kind::Post { half } else { depth };
            rect(&mut d, layer_for(m), [pt(a, -dd), pt(b, -dd), pt(b, dd), pt(a, dd)], 2.0);
        }
        // Text reads from below or from the right.
        let a = u[1].atan2(u[0]);
        let ang = if a > std::f64::consts::FRAC_PI_2 + 1e-9 || a <= -std::f64::consts::FRAC_PI_2 + 1e-9 { a + std::f64::consts::PI } else { a };
        let mid = pt(len / 2.0, half + 6.0);
        text(&mut d, "FRAMING-DIMS", mid, 4.0, &format!("{} {}", w.id, wf.settled.as_deref().unwrap_or("")), ang, HAlign::Center);
        if !wf.field_changes.is_empty() {
            text(&mut d, "FRAMING-FLAGS", pt(len / 2.0, -half - 6.0), 4.0, "FIELD CHANGE EXPECTED", ang, HAlign::Center);
        }
    }
    d
}

/// Framing members as 3D boxes (building feet; `elev_ft` the level's floor) for a model or
/// Walk view: (min, max, status, layout).
pub fn boxes(fr: &Framing, level: &str, elev_ft: f64) -> Vec<([f64; 3], [f64; 3], Status, Option<End>)> {
    let mut out = Vec::new();
    for wf in fr.walls.iter().filter(|w| w.wall.level == level) {
        let w = &wf.wall;
        let depth = w.stud_in.unwrap_or(0.0) / 12.0;
        let u = w.dir();
        let n = [-u[1], u[0]];
        for m in &wf.members {
            let (a, b) = (w.at(m.s_in - m.width_in / 2.0), w.at(m.s_in + m.width_in / 2.0));
            let corners = [-1.0, 1.0].map(|k| {
                [[a[0] + n[0] * k * depth / 2.0, a[1] + n[1] * k * depth / 2.0], [b[0] + n[0] * k * depth / 2.0, b[1] + n[1] * k * depth / 2.0]]
            });
            let xs = corners.iter().flatten().map(|p| p[0]);
            let ys = corners.iter().flatten().map(|p| p[1]);
            let (x0, x1) = (xs.clone().fold(f64::MAX, f64::min), xs.fold(f64::MIN, f64::max));
            let (y0, y1) = (ys.clone().fold(f64::MAX, f64::min), ys.fold(f64::MIN, f64::max));
            out.push(([x0, y0, elev_ft + m.z_in[0] / 12.0], [x1, y1, elev_ft + m.z_in[1] / 12.0], m.status, m.layout));
        }
    }
    out
}
