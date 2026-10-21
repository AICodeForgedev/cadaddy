//! What the prints don't say: the gaps and conflicts in a set, as an RFI list
//! ([`cadaddy_walk3d::Rfi`]) with the sheets involved, what is missing or conflicting, what the
//! readers assumed meanwhile and how much it matters.
//!
//! From the readers' results ([`PrintRead`]):
//! - walls with no type tag, tags the legend doesn't have, a wall typed differently on the floor
//!   plan and an enlarged plan;
//! - walls whose height rule clashes with the ceiling they meet (stopping at the ceiling beside a
//!   room open to the structure; under a two-layer ceiling);
//! - rated wall types stopping at the ceiling, wall types no head of wall detail covers;
//! - rooms with no ceiling type or height, soffits with no detail, callouts to details that
//!   aren't there, soffit details no soffit uses;
//! - sleeve and pathway notes with only a size (no route, destination or method), and sleeves
//!   that won't fit the framing: cored through the slab onto a beam, or bored through joists
//!   bigger than the joists allow (with the nearest fitting option, e.g. one 3" sleeve → 4 × 2"
//!   ENT; [`crate::bore`]).
//!
//! Coordinates are building feet; [`spec_patch`] puts the list into a building spec.
//! [`expected`] is what a list should say for a fixture's truth, and [`score_rfis`] compares.

use crate::bore::{self, Member, Raceway};
use crate::ceilasm_reader::{AsmResult, RoomRead, SoffitRead};
use crate::ceiling_reader::CeilingResult;
use crate::framing::JoistBay;
use crate::sheet_reader::{self, SheetMap};
use crate::struct_reader::StructuralModel;
use crate::truth::Truth;
use crate::wall_reader::{FoundWall, ReadType, WallResult};
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_walk3d::{Rfi, RfiKind, Severity};
use serde_json::{Value, json};

/// Items in a list at most, and texts looked at per sheet (hostile sets).
const MAX_ITEMS: usize = 5_000;
const MAX_TEXTS: usize = 50_000;

/// A sleeve or pathway note as read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SleeveRead {
    pub level: String,
    pub sheet: String,
    pub at_ft: [f64; 2],
    pub text: String,
    pub note: SleeveNote,
}

/// What a sleeve note says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SleeveNote {
    pub count: usize,
    pub size_in: Option<f64>,
    /// Where it goes (`CORRIDOR CEILING`, `LEVEL 1 CEILING`).
    pub route: Option<String>,
    /// How (`CORE THRU SLAB`, `BORE THRU 2X10 JOISTS`).
    pub method: Option<String>,
    /// The raceway it names (`ENT`, `EMT`, `PVC`), when it names one.
    pub raceway: Option<String>,
}

/// Everything the readers found in a set, for [`find_rfis`].
#[derive(Clone, Debug, Default)]
pub struct PrintRead {
    pub map: SheetMap,
    pub ceilings: CeilingResult,
    pub structure: StructuralModel,
    pub walls: WallResult,
    pub asm: AsmResult,
    /// The walls on each enlarged architectural plan: (sheet, walls with their tags).
    pub enlarged: Vec<(String, Vec<FoundWall>)>,
    pub sleeves: Vec<SleeveRead>,
    /// Wood joist bays (from a framing reader or entered), for sleeves bored through joists.
    pub bays: Vec<JoistBay>,
}

// ---- Sleeve notes --------------------------------------------------------------------------

const METHOD_WORDS: [&str; 10] = ["CORE", "CORED", "THRU", "THROUGH", "BORE", "BORED", "DRILL", "DRILLED", "VIA", "STUB"];

fn words(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        let w = c.is_ascii_alphanumeric() || c == '-' || c == '/' || c == '"' || c == '.';
        match (w, start) {
            (true, None) => start = Some(i),
            (false, Some(st)) => {
                out.push((st, s.get(st..i).unwrap_or_default()));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(st) = start {
        out.push((st, s.get(st..).unwrap_or_default()));
    }
    out
}

/// Read a sleeve or pathway note: `(2) 2" SLEEVES, CORE THRU SLAB TO LEVEL 1 CEILING`. Not a
/// sleeve note: `None`.
pub fn parse_sleeve(text: &str) -> Option<SleeveNote> {
    let u: String = text.to_ascii_uppercase().chars().take(400).collect();
    if !(u.contains("SLEEVE") || u.contains("PATHWAY")) {
        return None;
    }
    let size_in = crate::wall_reader::inch_values(&u).first().map(|(_, v)| *v);
    let count = u
        .trim_start()
        .strip_prefix('(')
        .and_then(|r| r.split_once(')'))
        .and_then(|(n, _)| n.trim().parse::<usize>().ok())
        .filter(|n| (1..=99).contains(n))
        .unwrap_or(1);
    let ws = words(&u);
    let stop = |from: usize| -> usize {
        // A phrase runs to the next comma, `TO`, or method word (whichever comes first).
        let comma = u.get(from..).and_then(|r| r.find([',', ';', '.'])).map_or(u.len(), |i| from + i);
        let next_word = ws.iter().filter(|(i, w)| *i > from && (*w == "TO" || METHOD_WORDS.contains(w))).map(|(i, _)| *i).min().unwrap_or(u.len());
        comma.min(next_word)
    };
    let phrase = |from: usize, to: usize| u.get(from..to).map(|s| s.trim().trim_end_matches(',').trim().to_string()).filter(|s| !s.is_empty());
    let route = ws.iter().find(|(_, w)| *w == "TO").and_then(|(i, _)| phrase(i + 2, stop(i + 2)));
    let method = ws.iter().find(|(_, w)| METHOD_WORDS.contains(w)).and_then(|(i, w)| {
        // `CORE THRU SLAB`: one phrase, though `THRU` is a method word too.
        let after = i + w.len();
        let end = ws
            .iter()
            .filter(|(j, x)| *j > after && (*x == "TO" || (METHOD_WORDS.contains(x) && !matches!(*x, "THRU" | "THROUGH"))))
            .map(|(j, _)| *j)
            .min()
            .unwrap_or(u.len());
        let comma = u.get(after..).and_then(|r| r.find([',', ';'])).map_or(u.len(), |k| after + k);
        phrase(*i, end.min(comma))
    });
    let raceway = ["ENT", "EMT", "PVC", "RMC", "IMC"].into_iter().find(|r| ws.iter().any(|(_, w)| w == r)).map(String::from);
    Some(SleeveNote { count, size_in, route, method, raceway })
}

/// A sawn joist's actual depth from `2X10` in a note.
fn joist_member(method: &str) -> Option<Member> {
    let u = method.to_ascii_uppercase();
    if !u.contains("JOIST") && !u.contains("TRUSS") {
        return None;
    }
    if u.contains("TRUSS") {
        return Some(Member::Truss);
    }
    let depth = words(&u).into_iter().find_map(|(_, w)| {
        let (a, b) = w.split_once('X')?;
        (a == "2" || a == "3").then_some(())?;
        match b {
            "6" => Some(5.5),
            "8" => Some(7.25),
            "10" => Some(9.25),
            "12" => Some(11.25),
            "14" => Some(13.25),
            _ => None,
        }
    });
    depth.map(|depth_in| Member::SawnJoist { depth_in })
}

fn raceway_of(n: &SleeveNote) -> Raceway {
    let size = n.size_in.unwrap_or(1.0);
    match n.raceway.as_deref() {
        Some("ENT") => Raceway::Ent(size),
        Some("EMT") => Raceway::Emt(size),
        Some("PVC") => Raceway::PvcSch40(size),
        // A plain sleeve: Schedule 40 pipe of that size, by its outside diameter.
        _ => Raceway::Od(Raceway::PvcSch40(size).od_in().unwrap_or(size + 0.5)),
    }
}

/// Sleeve notes on a sheet: a note with a leader to a small circle (or the note's own place).
fn read_sleeves(d: &Drawing, al: &sheet_reader::Alignment, level: &str, sheet: &str) -> Vec<SleeveRead> {
    let mut lines: Vec<([f64; 2], [f64; 2], &str)> = Vec::new();
    let mut circles: Vec<([f64; 2], &str)> = Vec::new();
    let mut notes: Vec<(String, [f64; 2], f64, &str)> = Vec::new();
    for e in d.model.iter().take(MAX_TEXTS) {
        let layer = e.common.layer.as_str();
        match &e.kind {
            EntityKind::Line(l) => lines.push(([l.a.x, l.a.y], [l.b.x, l.b.y], layer)),
            EntityKind::Circle(c) => circles.push(([c.center.x, c.center.y], layer)),
            EntityKind::Text(t) => {
                let p = t.align_pt.unwrap_or(t.insert);
                notes.push((t.value.clone(), [p.x, p.y], t.height, layer));
            }
            EntityKind::MText(t) => notes.push((t.contents.clone(), [t.insert.x, t.insert.y], t.height, layer)),
            _ => {}
        }
    }
    let dist = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let mut out = Vec::new();
    for (s, at, h, layer) in notes {
        let Some(note) = parse_sleeve(&s) else { continue };
        if !(at.iter().all(|v| v.is_finite()) && h.is_finite() && h > 0.0) {
            continue;
        }
        // The leader: a line on the note's layer with an end by the note's start; its far end,
        // snapped to a circle on that layer.
        let far = lines
            .iter()
            .filter(|(_, _, l)| *l == layer)
            .filter_map(|(a, b, _)| {
                let (da, db) = (dist(*a, at), dist(*b, at));
                let (near, far) = if da < db { (da, *b) } else { (db, *a) };
                (near < 2.0 * h && dist(far, at) > 2.0 * h).then_some((near, far))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, f)| f);
        let p = far.map_or(at, |f| {
            circles
                .iter()
                .filter(|(c, l)| *l == layer && dist(*c, f) < 3.0 * h)
                .map(|(c, _)| *c)
                .min_by(|a, b| dist(*a, f).total_cmp(&dist(*b, f)))
                .unwrap_or(f)
        });
        out.push(SleeveRead {
            level: level.into(),
            sheet: sheet.into(),
            at_ft: sheet_reader::building_from_model(al, p),
            text: s.trim().to_string(),
            note,
        });
    }
    out
}

// ---- Reading a set -------------------------------------------------------------------------

fn sheet_where(map: &SheetMap, f: impl Fn(&str, &str, Option<&str>) -> bool) -> Vec<&str> {
    map.sheets.iter().filter(|s| f(&s.title.to_ascii_uppercase(), &s.discipline, s.level.as_deref())).map(|s| s.number.as_str()).collect()
}

/// Run the readers over a set's DXF sheets: sheets, ceilings, structure, walls, assemblies,
/// enlarged plans and sleeve notes.
pub fn read_set(dxf: &[(String, Vec<u8>)]) -> PrintRead {
    let map = sheet_reader::recognise_fixture_set(dxf);
    let ceilings = crate::ceiling_reader::read_ceilings(dxf, &map);
    let drawings: Vec<(String, Drawing)> =
        dxf.iter().filter_map(|(n, b)| cadaddy_io::read_dxf(b).ok().map(|d| (n.trim_end_matches(".dxf").to_string(), d))).collect();
    let refs: Vec<(String, &Drawing)> = drawings.iter().map(|(n, d)| (n.clone(), d)).collect();
    let structure = crate::struct_reader::read_structure(&map, &refs);
    let walls = crate::wall_reader::read_walls(dxf, &map, &ceilings, &structure);
    let asm = crate::ceilasm_reader::read_assemblies(dxf, &map, &ceilings, &walls);
    let mut enlarged = Vec::new();
    let mut sleeves = Vec::new();
    for (name, d) in &drawings {
        let Some(sh) = map.sheets.iter().find(|s| &s.number == name) else { continue };
        let title = sh.title.to_ascii_uppercase();
        if let Some(level) = &sh.level
            && sh.discipline == "A"
            && title.contains("ENLARGED")
            && !title.contains("CEILING")
        {
            enlarged.push((sh.number.clone(), enlarged_walls(d, sh.alignment.as_ref(), level, &walls)));
        }
        let (Some(level), Some(al)) = (&sh.level, &sh.alignment) else { continue };
        if matches!(sh.discipline.as_str(), "A" | "E" | "T") && title.contains("PLAN") && !title.contains("CEILING") {
            sleeves.extend(read_sleeves(d, al, level, &sh.number));
        }
    }
    PrintRead { map, ceilings, structure, walls, asm, enlarged, sleeves, bays: Vec::new() }
}

/// The walls on an enlarged plan, in building feet. Its grid bubbles are usually cut off, so
/// without an alignment it is registered on the floor plan's walls: read unshifted (enlarged
/// plans aren't turned), then moved by the shift most of its walls agree on.
fn enlarged_walls(d: &Drawing, al: Option<&sheet_reader::Alignment>, level: &str, walls: &WallResult) -> Vec<FoundWall> {
    if let Some(al) = al {
        return crate::wall_reader::read_plan_walls(d, al, level, &walls.types);
    }
    let mut ws = crate::wall_reader::read_plan_walls(d, &sheet_reader::Alignment::default(), level, &walls.types);
    let main: Vec<&FoundWall> = walls.walls.iter().filter(|w| w.level == level).take(2_000).collect();
    let vertical = |w: &FoundWall| (w.a_ft[0] - w.b_ft[0]).abs() < 1e-3;
    let horizontal = |w: &FoundWall| (w.a_ft[1] - w.b_ft[1]).abs() < 1e-3;
    // Votes for a shift along x (from vertical walls) and y (horizontal ones), to 0.05'.
    let vote = |axis: usize, along: &dyn Fn(&FoundWall) -> bool| -> Option<f64> {
        let mut votes: std::collections::BTreeMap<i64, usize> = std::collections::BTreeMap::new();
        for e in ws.iter().filter(|w| along(w)).take(2_000) {
            for m in main.iter().filter(|w| along(w) && (w.thickness_in - e.thickness_in).abs() < 0.5) {
                let (Some(a), Some(b)) = (m.a_ft.get(axis), e.a_ft.get(axis)) else { continue };
                let d = a - b;
                if d.is_finite() && d.abs() < 1e7 {
                    *votes.entry((d * 20.0).round() as i64).or_default() += 1;
                }
            }
        }
        votes.into_iter().max_by_key(|(_, n)| *n).filter(|(_, n)| *n >= 2).map(|(k, _)| k as f64 / 20.0)
    };
    let (Some(dx), Some(dy)) = (vote(0, &vertical), vote(1, &horizontal)) else { return Vec::new() };
    for w in ws.iter_mut() {
        w.a_ft = [w.a_ft[0] + dx, w.a_ft[1] + dy];
        w.b_ft = [w.b_ft[0] + dx, w.b_ft[1] + dy];
    }
    ws
}

// ---- Finding the gaps ----------------------------------------------------------------------

fn mid(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

/// The point on a wall beside a room (its centre's foot on the wall): where a long wall's item
/// about that room is.
fn beside_room(w: &FoundWall, r: &RoomRead) -> [f64; 2] {
    let c = mid(r.min_ft, r.max_ft);
    let d = [w.b_ft[0] - w.a_ft[0], w.b_ft[1] - w.a_ft[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    if !(l2 > 1e-12 && l2.is_finite()) {
        return mid(w.a_ft, w.b_ft);
    }
    let s = (((c[0] - w.a_ft[0]) * d[0] + (c[1] - w.a_ft[1]) * d[1]) / l2).clamp(0.0, 1.0);
    [w.a_ft[0] + d[0] * s, w.a_ft[1] + d[1] * s]
}

fn wall_extent(w: &FoundWall) -> [[f64; 2]; 2] {
    [[w.a_ft[0].min(w.b_ft[0]), w.a_ft[1].min(w.b_ft[1])], [w.a_ft[0].max(w.b_ft[0]), w.a_ft[1].max(w.b_ft[1])]]
}

fn about_wall(w: &FoundWall) -> String {
    match w.rooms.as_slice() {
        [a, b] => format!("wall between rooms {a} and {b}"),
        [a, b, rest @ ..] => format!("wall along rooms {a}, {b} and {} more", rest.len()),
        [a] => format!("wall of room {a} (outside)"),
        [] => format!("wall at {:.1}', {:.1}'", (w.a_ft[0] + w.b_ft[0]) / 2.0, (w.a_ft[1] + w.b_ft[1]) / 2.0),
    }
}

fn boards(layers: &[crate::truth::WallTypeLayer]) -> usize {
    layers.iter().filter(|l| l.material.starts_with("gyp") || l.material == "shaft_liner").count()
}

/// The room open to the structure by the prints (schedule or RCP), not by a guess for want of
/// anything else.
fn open_room(rr: &RoomRead, asm: &AsmResult) -> bool {
    let open = rr.ceiling_type.as_ref().and_then(|t| asm.ceiling_types.iter().find(|c| &c.tag == t)).is_some_and(|c| c.kind == "OPEN");
    open && rr.guess.as_ref().is_none_or(|g| g.contains("open to structure on the RCP"))
}

fn height_words(rule: &str) -> &'static str {
    match rule {
        "ceiling" => "to the ceiling",
        "above_ceiling" => "6\" above the ceiling",
        "deck" => "to the deck",
        _ => "full height",
    }
}

struct Sheets<'a> {
    map: &'a SheetMap,
}

impl Sheets<'_> {
    fn first(&self, f: impl Fn(&str, &str, Option<&str>) -> bool) -> Option<String> {
        sheet_where(self.map, f).first().map(|s| s.to_string())
    }
    fn floor(&self, level: &str) -> Option<String> {
        self.first(|t, d, l| d == "A" && l == Some(level) && t.contains("FLOOR PLAN"))
    }
    fn rcp(&self, level: &str) -> Option<String> {
        self.first(|t, _, l| l == Some(level) && t.contains("CEILING"))
    }
    fn legend(&self) -> Option<String> {
        self.first(|t, _, _| t.contains("WALL TYPE") || t.contains("PARTITION"))
    }
    fn details(&self) -> Option<String> {
        self.first(|t, _, l| l.is_none() && (t.contains("CEILING TYPE") || t.contains("SOFFIT") || t.contains("HEAD OF WALL")))
    }
    fn finishes(&self) -> Option<String> {
        self.first(|t, _, _| t.contains("FINISH SCHEDULE"))
    }
    fn framing(&self, level: &str) -> Option<String> {
        self.first(|t, d, l| d == "S" && l == Some(level) && t.contains("FRAMING"))
    }
}

fn sheets(list: &[Option<String>]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in list.iter().flatten() {
        if !out.contains(s) {
            out.push(s.clone());
        }
    }
    out
}

/// The gaps and conflicts in a set (see the module docs), most severe first, numbered
/// `RFI-001`….
pub fn find_rfis(p: &PrintRead) -> Vec<Rfi> {
    let sh = Sheets { map: &p.map };
    let mut out: Vec<Rfi> = Vec::new();
    let types = &p.walls.types;
    let ty = |t: &str| types.iter().find(|x| x.tag == t);
    let room = |level: &str, n: &str| p.asm.rooms.iter().find(|r| r.level == level && r.room == n);
    let assumed_type = |w: &FoundWall| match (&w.wall_type, &w.guess) {
        (Some(t), Some(why)) => format!("Type {t} ({why})."),
        (Some(t), None) => format!("Type {t}."),
        (None, _) => "No type: built as the plan's wall layer.".into(),
    };

    // Walls.
    for w in p.walls.walls.iter().take(MAX_ITEMS) {
        let at = mid(w.a_ft, w.b_ft);
        let floor = sh.floor(&w.level);
        let mut push = |kind: RfiKind, at: [f64; 2], sheets_: Vec<String>, issue: String, assumed: String, proposed: Option<String>| {
            let mut r = Rfi::new(kind, &w.level, at, about_wall(w), issue, assumed);
            r.extent = Some(wall_extent(w));
            r.sheets = sheets_;
            r.proposed = proposed;
            out.push(r);
        };
        if let Some(tag) = &w.stray_tag {
            push(
                RfiKind::UnknownTag,
                at,
                sheets(&[floor.clone(), sh.legend()]),
                format!("The plan tags this wall {tag}, which the wall type legend doesn't have."),
                assumed_type(w),
                Some("Confirm the wall type, or add the type to the legend.".into()),
            );
        } else if !w.tagged {
            push(RfiKind::UntaggedWall, at, sheets(&[floor.clone(), sh.legend()]), "No wall type tag on this wall.".into(), assumed_type(w), None);
        }
        // Height rule against the ceilings it meets (the type's rule, as read or guessed).
        let Some(t) = w.wall_type.as_deref().and_then(ty) else { continue };
        let rule = t.height.as_deref().unwrap_or_default();
        if !matches!(rule, "ceiling" | "above_ceiling") || t.external {
            continue;
        }
        let beside: Vec<&RoomRead> = w.rooms.iter().filter_map(|n| room(&w.level, n)).collect();
        if let Some(o) = beside.iter().find(|r| open_room(r, &p.asm)) {
            let lit = beside.iter().filter(|r| !open_room(r, &p.asm)).filter_map(|r| r.height_ft).reduce(f64::max);
            push(
                RfiKind::HeightVsCeiling,
                beside_room(w, o),
                sheets(&[floor.clone(), sh.rcp(&w.level), sh.legend()]),
                format!(
                    "Type {} goes {}, but room {} beside it is open to the structure: there's no ceiling on that side to stop at.",
                    t.tag,
                    height_words(rule),
                    o.room
                ),
                match (lit, w.height_ft) {
                    (Some(c), Some(h)) => {
                        format!("Built to {} (the ceiling on the other side, {}).", cadaddy_walk3d::feet(h), cadaddy_walk3d::feet(c))
                    }
                    (_, Some(h)) => format!("Built to the deck, {}.", cadaddy_walk3d::feet(h)),
                    _ => "Built to the deck.".into(),
                },
                Some("Extend the wall to the deck, or confirm it stops at the ceiling.".into()),
            );
        }
        let double = beside.iter().find_map(|r| {
            let ct = p.asm.ceiling_types.iter().find(|c| Some(&c.tag) == r.ceiling_type.as_ref())?;
            (boards(&ct.layers) >= 2).then_some((r, ct))
        });
        if let Some((r, ct)) = double {
            push(
                RfiKind::DoubleCeilingShortWall,
                beside_room(w, r),
                sheets(&[floor.clone(), sh.rcp(&w.level), sh.details(), sh.legend()]),
                format!(
                    "Room {} has ceiling {} ({} board layers{}) over this type {} wall, which goes only {}.",
                    r.room,
                    ct.tag,
                    boards(&ct.layers),
                    ct.fire_rating.as_ref().map_or(String::new(), |f| format!(", {f}")),
                    t.tag,
                    height_words(rule)
                ),
                "The wall stops at its rule; the ceiling runs over it.".into(),
                Some("Confirm the ceiling runs continuous over the wall, or take the wall to the deck.".into()),
            );
        }
    }

    // The same wall on the floor plan and an enlarged plan.
    for (sheet, ews) in &p.enlarged {
        for e in ews.iter().take(MAX_ITEMS) {
            let Some(etag) = e.stray_tag.clone().or_else(|| e.tagged.then(|| e.wall_type.clone()).flatten()) else { continue };
            let Some(w) = p.walls.walls.iter().find(|w| crate::wall_reader::same_wall(w, e)) else { continue };
            let ftag = w.stray_tag.clone().or_else(|| w.tagged.then(|| w.wall_type.clone()).flatten());
            let Some(ftag) = ftag else { continue };
            if ftag == etag {
                continue;
            }
            let floor = sh.floor(&w.level);
            let mut r = Rfi::new(
                RfiKind::SheetConflict,
                &w.level,
                mid(w.a_ft, w.b_ft),
                about_wall(w),
                format!("{} tags this wall {ftag}; {sheet} tags it {etag}.", floor.as_deref().unwrap_or("The floor plan")),
                format!("Type {ftag}, from the floor plan."),
            );
            r.extent = Some(wall_extent(w));
            r.sheets = sheets(&[floor, Some(sheet.clone()), sh.legend()]);
            r.proposed = Some(format!("Confirm which type this wall is: {ftag} or {etag}."));
            out.push(r);
        }
    }

    // Wall types.
    let first_wall = |t: &ReadType| p.walls.walls.iter().find(|w| w.wall_type.as_ref() == Some(&t.tag));
    for t in types.iter().take(MAX_ITEMS) {
        let w = first_wall(t);
        let (level, at) = w.map_or((String::new(), [0.0, 0.0]), |w| (w.level.clone(), mid(w.a_ft, w.b_ft)));
        let rule = t.height.as_deref().unwrap_or_default();
        if let Some(rating) = &t.fire_rating
            && matches!(rule, "ceiling" | "above_ceiling")
        {
            let mut r = Rfi::new(
                RfiKind::RatedToCeiling,
                &level,
                at,
                format!("wall type {}", t.tag),
                format!("Type {} is rated {rating} but goes only {}.", t.tag, height_words(rule)),
                format!("Built {} as the legend says.", height_words(rule)),
            );
            r.sheets = sheets(&[sh.legend(), sh.details()]);
            r.proposed = Some("A rated wall normally runs to the deck: confirm the height, or a rated ceiling that continues the rating.".into());
            r.extent = w.map(wall_extent);
            out.push(r);
        }
        let head = p.asm.walls.iter().find(|h| h.wall_type == t.tag);
        if !p.asm.heads.is_empty() && head.is_some_and(|h| h.head.is_none()) {
            let mut r = Rfi::new(
                RfiKind::HeadUnspecified,
                &level,
                at,
                format!("wall type {}", t.tag),
                format!("No head of wall detail lists type {}.", t.tag),
                format!("A single top track {}.", height_words(rule)),
            );
            r.sheets = sheets(&[sh.details(), sh.legend()]);
            r.proposed = Some("Give the head of wall detail for this type (track, deflection, to deck or ceiling).".into());
            r.extent = w.map(wall_extent);
            out.push(r);
        }
    }

    // Rooms' ceilings.
    for r in p.asm.rooms.iter().take(MAX_ITEMS) {
        let open = open_room(r, &p.asm);
        let no_type = r.guess.is_some() || r.ceiling_type.is_none();
        let no_height = r.height_ft.is_none() && !open;
        if !(no_type || no_height) {
            continue;
        }
        let what = match (no_type, no_height) {
            (true, true) => "neither a ceiling type nor a height",
            (true, false) => "no ceiling type",
            _ => "no ceiling height",
        };
        let mut item = Rfi::new(
            RfiKind::CeilingUnspecified,
            &r.level,
            mid(r.min_ft, r.max_ft),
            format!("room {}", r.room),
            format!("The prints give {what} for room {}.", r.room),
            format!(
                "{}; {}.",
                match (&r.ceiling_type, &r.guess) {
                    (Some(t), Some(why)) => format!("Type {t} ({why})"),
                    (Some(t), None) => format!("Type {t}"),
                    (None, _) => "No type".into(),
                },
                r.height_ft.map_or("no height (walls beside it go to the deck)".to_string(), |h| format!("height {}", cadaddy_walk3d::feet(h)))
            ),
        );
        item.extent = Some([r.min_ft, r.max_ft]);
        item.sheets = sheets(&[sh.rcp(&r.level), sh.finishes()]);
        out.push(item);
    }

    // Soffits and their callouts.
    let details = &p.asm.soffit_types;
    for (i, s) in p.asm.soffits.iter().enumerate().take(MAX_ITEMS) {
        let dangling: Vec<&crate::ceilasm_reader::CalloutRead> =
            p.asm.callouts.iter().filter(|c| c.soffit == Some(i) && !details.iter().any(|d| d.tag == c.detail)).collect();
        let rcp = sh.rcp(&s.level);
        if let Some(c) = dangling.first() {
            let mut r =
                soffit_item(RfiKind::DetailMissing, s, format!("The callout {}/{} points to a detail {} doesn't have.", c.detail, c.sheet, c.sheet));
            r.sheets = sheets(&[rcp, Some(c.sheet.clone()).filter(|x| !x.is_empty()), sh.details()]);
            r.proposed = Some(format!("Issue detail {}/{}, or correct the callout.", c.detail, c.sheet));
            out.push(r);
            continue;
        }
        if s.guess.is_some() || s.soffit_type.is_none() || s.height_ft.is_none() {
            let what = match (s.guess.is_some() || s.soffit_type.is_none(), s.height_ft.is_none()) {
                (true, true) => "No section or detail shows how this soffit is built, and the RCP gives no bottom height.",
                (true, false) => "No section or detail shows how this soffit is built (no callout on the RCP).",
                _ => "The RCP gives no bottom height for this soffit.",
            };
            let mut r = soffit_item(RfiKind::SoffitNoDetail, s, what.into());
            r.sheets = sheets(&[rcp, sh.details()]);
            r.proposed = Some("Call out the soffit's detail or section on the RCP.".into());
            out.push(r);
        }
    }
    // Callouts beside no soffit, to a detail that isn't there.
    for c in p.asm.callouts.iter().filter(|c| c.soffit.is_none() && !details.iter().any(|d| d.tag == c.detail)).take(MAX_ITEMS) {
        let mut r = Rfi::new(
            RfiKind::DetailMissing,
            &c.level,
            c.at_ft,
            format!("callout {}/{}", c.detail, c.sheet),
            format!("The callout {}/{} points to a detail {} doesn't have.", c.detail, c.sheet, c.sheet),
            "Ignored.",
        );
        r.sheets = sheets(&[sh.rcp(&c.level), sh.details()]);
        out.push(r);
    }
    for d in details.iter().take(MAX_ITEMS) {
        if p.asm.callouts.iter().any(|c| c.detail == d.tag) {
            continue;
        }
        let mut r = Rfi::new(
            RfiKind::DetailUnused,
            "",
            [0.0, 0.0],
            format!("soffit detail {}", d.tag),
            format!("Soffit detail {} ({}) is on the details sheet, but no RCP calls it out.", d.tag, d.name.to_lowercase()),
            "Built only where a soffit with no callout looks like it.".to_string(),
        );
        r.sheets = sheets(&[sh.details()]);
        r.proposed = Some("Say where this soffit goes, or remove the detail.".into());
        out.push(r);
    }

    // Sleeves.
    for s in p.sleeves.iter().take(MAX_ITEMS) {
        sleeve_items(p, &sh, s, &mut out);
    }

    out.truncate(MAX_ITEMS);
    out.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(a.level.cmp(&b.level))
            .then(a.kind.cmp(&b.kind))
            .then(a.at[1].total_cmp(&b.at[1]))
            .then(a.at[0].total_cmp(&b.at[0]))
    });
    for (i, r) in out.iter_mut().enumerate() {
        r.id = format!("RFI-{:03}", i + 1);
    }
    out
}

fn soffit_item(kind: RfiKind, s: &SoffitRead, issue: String) -> Rfi {
    let assumed = match (&s.soffit_type, &s.guess) {
        (Some(t), Some(why)) => format!("Detail {t} ({why})"),
        (Some(t), None) => format!("Detail {t}"),
        (None, _) => "No detail: a plain box".into(),
    };
    let assumed = match s.height_ft {
        Some(h) => format!("{assumed}; bottom at {}.", cadaddy_walk3d::feet(h)),
        None => format!("{assumed}; bottom 1' below the ceiling."),
    };
    let mut r = Rfi::new(kind, &s.level, mid(s.min_ft, s.max_ft), format!("soffit in room {}", s.room), issue, assumed);
    r.extent = Some([s.min_ft, s.max_ft]);
    r
}

/// A sleeve's items: a size with no route or method; a sleeve that won't fit the framing.
fn sleeve_items(p: &PrintRead, sh: &Sheets, s: &SleeveRead, out: &mut Vec<Rfi>) {
    let n = &s.note;
    let size = n.size_in.map_or("a".to_string(), |v| crate::walls::frac(v, true));
    let what = if n.count > 1 { format!("({}) {size} sleeves", n.count) } else { format!("{size} sleeve") };
    let new = |kind: RfiKind, issue: String, assumed: &str| {
        let mut r = Rfi::new(kind, &s.level, s.at_ft, format!("sleeve: {}", s.text), issue, assumed);
        r.extent = Some([[s.at_ft[0] - 3.0, s.at_ft[1] - 3.0], [s.at_ft[0] + 3.0, s.at_ft[1] + 3.0]]);
        r.sheets = vec![s.sheet.clone()];
        r
    };
    let missing: Vec<&str> =
        [n.size_in.is_none().then_some("size"), n.route.is_none().then_some("route or destination"), n.method.is_none().then_some("method")]
            .into_iter()
            .flatten()
            .collect();
    if !missing.is_empty() {
        let mut r = new(
            RfiKind::SleeveNoRoute,
            format!("\"{}\" gives no {}.", s.text, missing.join(", no ")),
            "Not routed: shown where the note points, straight up and down.",
        );
        r.proposed = Some(
            "Before rough-in: say where it runs to (which floor sleeve, which ceiling), and how (cored through the slab, bored through the joists, along which wall)."
                .into(),
        );
        out.push(r);
    }
    let Some(method) = &n.method else { return };
    let m = method.to_ascii_uppercase();
    let od = raceway_of(n).od_in().or(n.size_in.map(|v| v + 0.4)).unwrap_or(1.0);
    // Through joists: the hole each joist allows.
    let member = joist_member(&m).or_else(|| {
        p.bays
            .iter()
            .find(|b| {
                b.level == s.level && s.at_ft[0] >= b.min_ft[0] && s.at_ft[0] <= b.max_ft[0] && s.at_ft[1] >= b.min_ft[1] && s.at_ft[1] <= b.max_ft[1]
            })
            .filter(|_| m.contains("JOIST") || m.contains("BORE"))
            .map(|b| b.member.clone())
    });
    if let Some(member) = member {
        let c = bore::check(raceway_of(n), &member, bore::HoleAt::default(), bore::CLEARANCE_IN);
        if !c.fits {
            let limit = c.max_hole_in.map_or("no field holes".to_string(), |v| format!("holes up to {}", crate::walls::frac(v, true)));
            let mut r = new(
                RfiKind::SleeveNoFit,
                format!("{what} bored through {} won't fit: {} ({limit}).", c.member, c.flags.first().cloned().unwrap_or_default()),
                "Shown as noted; not bored.",
            );
            r.proposed = Some(match c.proposal {
                Some((k, rw)) => format!(
                    "Use {} × {} instead (the same inside area), bored through the joists; flags only, not a structural approval.",
                    k * n.count.max(1),
                    rw.words()
                ),
                None => "Route it another way (between the joists, or through a soffit): nothing standard fits.".into(),
            });
            r.sheets = sheets(&[Some(s.sheet.clone()), sh.framing(&s.level)]);
            out.push(r);
        }
        return;
    }
    // Cored through the slab: clear of the beams under it.
    if !(m.contains("CORE") || m.contains("SLAB")) {
        return;
    }
    let group = n.count.max(1) as f64 * (od + 1.0);
    let beams = p.structure.beams.get(&s.level).map(Vec::as_slice).unwrap_or_default();
    for b in beams.iter().take(MAX_ITEMS) {
        let flange = crate::framing::section_footprint(&b.size).map_or(8.0, |f| f[1]);
        let need_ft = (flange / 2.0 + group / 2.0 + 1.0) / 12.0;
        let d = crate::gaps::to_segment(s.at_ft, b.from_ft, b.to_ft);
        if d >= need_ft {
            continue;
        }
        let along = [b.to_ft[0] - b.from_ft[0], b.to_ft[1] - b.from_ft[1]];
        let dir = if along[0].abs() >= along[1].abs() { "north or south" } else { "east or west" };
        let move_in = ((need_ft - d) * 12.0 + 2.0).ceil();
        let mut r = new(
            RfiKind::SleeveNoFit,
            format!(
                "{what} cored through the slab land on the {} beam under it ({:.0}\" from its centreline; its flange is {} wide).",
                b.size,
                d * 12.0,
                crate::walls::frac(flange, true)
            ),
            "Shown as noted; not cored.",
        );
        r.proposed = Some(format!("Move the sleeve {move_in}\" {dir}, clear of the beam's flange, and confirm the core location."));
        r.sheets = sheets(&[Some(s.sheet.clone()), sh.framing(&s.level)]);
        out.push(r);
        break;
    }
}

/// The list into a building spec (`bspec.set` / `bspec.merge`): `{"rfis": […]}` with the points
/// on each level's plan (`to_plan(level, building feet)`; items with no level keep theirs).
pub fn spec_patch(rfis: &[Rfi], to_plan: &dyn Fn(&str, [f64; 2]) -> Option<[f64; 2]>) -> Value {
    let items: Vec<Value> = rfis
        .iter()
        .take(MAX_ITEMS)
        .map(|r| {
            let mut r = r.clone();
            if !r.level.is_empty() {
                if let Some(p) = to_plan(&r.level, r.at) {
                    r.at = p;
                }
                r.extent = r.extent.and_then(|[a, b]| Some([to_plan(&r.level, a)?, to_plan(&r.level, b)?]));
            }
            serde_json::to_value(r).unwrap_or(Value::Null)
        })
        .collect();
    json!({ "rfis": items })
}

/// Read a fixture set (with or without planted gaps) and list its RFIs.
pub fn read_fixture_rfis(seed: u64, with_gaps: bool) -> Result<(Truth, PrintRead, Vec<Rfi>), String> {
    let set = if with_gaps { crate::generate_with_gaps(seed)? } else { crate::generate(seed)? };
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let read = read_set(&dxf);
    let rfis = find_rfis(&read);
    Ok((set.truth, read, rfis))
}

// ---- Scoring -------------------------------------------------------------------------------

/// An item a fixture's list should have.
#[derive(Clone, Debug, PartialEq)]
pub struct Expected {
    pub kind: RfiKind,
    pub level: String,
    /// Where, building feet: a wall's centreline, a point twice for others; type-level items:
    /// none.
    pub at: Option<[[f64; 2]; 2]>,
    /// For type-level items: `wall type P4`, `soffit detail 8`, `room 104`.
    pub about: String,
    /// What it is in the truth (a wall, soffit or sleeve id; a type tag; `level/room`).
    pub truth: String,
}

/// What the RFI list for a fixture should say: its planted gaps and the gaps every set has.
pub fn expected(t: &Truth) -> Vec<Expected> {
    let mut out = Vec::new();
    let mut add = |kind: RfiKind, level: &str, at: Option<[[f64; 2]; 2]>, about: String, truth: &str| {
        out.push(Expected { kind, level: level.into(), at, about, truth: truth.into() })
    };
    let room = |lv: &str, n: &str| t.building.rooms.iter().find(|r| r.level == lv && r.number == n);
    for w in &t.plan_walls {
        let at = Some([w.a_ft, w.b_ft]);
        if w.plan_tag.is_some() {
            add(RfiKind::UnknownTag, &w.level, at, String::new(), &w.id);
        } else if !w.tagged {
            add(RfiKind::UntaggedWall, &w.level, at, String::new(), &w.id);
        }
        if w.enlarged_tag.as_ref().is_some_and(|e| *e != w.wall_type) {
            add(RfiKind::SheetConflict, &w.level, at, String::new(), &w.id);
        }
        if !w.tagged || w.plan_tag.is_some() {
            continue;
        }
        let Some(wt) = t.wall_types.iter().find(|x| x.tag == w.wall_type) else { continue };
        if !matches!(wt.height.as_str(), "ceiling" | "above_ceiling") || wt.external {
            continue;
        }
        if w.rooms.iter().any(|n| room(&w.level, n).is_some_and(|r| r.ceiling.is_none())) {
            add(RfiKind::HeightVsCeiling, &w.level, at, String::new(), &w.id);
        }
        let two = w.rooms.iter().any(|n| {
            t.room_ceilings.iter().any(|c| {
                c.level == w.level && c.room == *n && t.ceiling_types.iter().find(|x| x.tag == c.ceiling_type).is_some_and(|x| boards(&x.layers) >= 2)
            })
        });
        if two {
            add(RfiKind::DoubleCeilingShortWall, &w.level, at, String::new(), &w.id);
        }
    }
    for wt in &t.wall_types {
        if wt.fire_rating.is_some() && matches!(wt.height.as_str(), "ceiling" | "above_ceiling") {
            add(RfiKind::RatedToCeiling, "", None, format!("wall type {}", wt.tag), &wt.tag);
        }
        if wt.head.is_none() && !t.head_details.is_empty() {
            add(RfiKind::HeadUnspecified, "", None, format!("wall type {}", wt.tag), &wt.tag);
        }
    }
    for rc in &t.room_ceilings {
        let hidden = room(&rc.level, &rc.room).and_then(|r| r.ceiling.as_ref()).is_some_and(|c| c.height_hidden);
        if !rc.scheduled || hidden {
            add(RfiKind::CeilingUnspecified, &rc.level, None, format!("room {}", rc.room), &format!("{}/{}", rc.level, rc.room));
        }
    }
    for s in &t.rcp_soffits {
        let c = mid(s.min_ft, s.max_ft);
        let at = Some([c, c]);
        if s.callout_detail.is_some() {
            add(RfiKind::DetailMissing, &s.level, at, String::new(), &s.id);
        } else if !s.called_out {
            add(RfiKind::SoffitNoDetail, &s.level, at, String::new(), &s.id);
        }
    }
    for st in &t.soffit_types {
        if !t.rcp_soffits.iter().any(|s| s.called_out && s.callout_detail.is_none() && s.soffit_type.as_ref() == Some(&st.tag)) {
            add(RfiKind::DetailUnused, "", None, format!("soffit detail {}", st.tag), &st.tag);
        }
    }
    for s in &t.sleeves {
        if s.route.is_none() || s.method.is_none() {
            add(RfiKind::SleeveNoRoute, &s.level, Some([s.at_ft, s.at_ft]), String::new(), &s.id);
        }
        if s.on_beam {
            add(RfiKind::SleeveNoFit, &s.level, Some([s.at_ft, s.at_ft]), String::new(), &s.id);
        }
    }
    out
}

/// How a list compares with [`expected`], per kind.
#[derive(Clone, Debug, Default)]
pub struct RfiScore {
    /// (kind, expected, found of those).
    pub kinds: Vec<(RfiKind, usize, usize)>,
    /// Expected items not found.
    pub missed: Vec<Expected>,
    /// Items that match nothing expected (and aren't about an untagged wall's guessed type).
    pub extra: Vec<Rfi>,
}

impl RfiScore {
    pub fn recall(&self) -> f64 {
        let (e, f) = self.kinds.iter().fold((0, 0), |(e, f), k| (e + k.1, f + k.2));
        if e == 0 { 1.0 } else { f as f64 / e as f64 }
    }
}

fn matches(e: &Expected, r: &Rfi) -> bool {
    if Some(e.kind) != r.kind {
        return false;
    }
    match e.at {
        Some([a, b]) => e.level == r.level && crate::gaps::to_segment(r.at, a, b) < 2.0,
        None => r.about == e.about && (e.level.is_empty() || e.level == r.level),
    }
}

/// Compare a list with what the truth says it should have.
pub fn score_rfis(t: &Truth, found: &[Rfi]) -> RfiScore {
    let exp = expected(t);
    let mut s = RfiScore::default();
    for k in RfiKind::ALL {
        let mine: Vec<&Expected> = exp.iter().filter(|e| e.kind == k).collect();
        let hit = mine.iter().filter(|e| found.iter().any(|r| matches(e, r))).count();
        if !mine.is_empty() {
            s.kinds.push((k, mine.len(), hit));
        }
        s.missed.extend(mine.into_iter().filter(|e| !found.iter().any(|r| matches(e, r))).cloned());
    }
    // An untagged wall's type is a guess, so what follows from its type isn't scored.
    let untagged: Vec<[[f64; 2]; 2]> = t.plan_walls.iter().filter(|w| !w.tagged || w.plan_tag.is_some()).map(|w| [w.a_ft, w.b_ft]).collect();
    for r in found {
        if exp.iter().any(|e| matches(e, r)) {
            continue;
        }
        let guessed = matches!(r.kind, Some(RfiKind::HeightVsCeiling | RfiKind::DoubleCeilingShortWall))
            && untagged.iter().any(|[a, b]| crate::gaps::to_segment(r.at, *a, *b) < 2.0);
        if !guessed {
            s.extra.push(r.clone());
        }
    }
    s
}

/// The items of a list by severity: (high, medium, low).
pub fn by_severity(items: &[Rfi]) -> (usize, usize, usize) {
    items.iter().fold((0, 0, 0), |(h, m, l), r| match r.severity {
        Severity::High => (h + 1, m, l),
        Severity::Medium => (h, m + 1, l),
        Severity::Low => (h, m, l + 1),
    })
}
