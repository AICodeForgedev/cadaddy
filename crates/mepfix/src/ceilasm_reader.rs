//! Ceiling assembly reader: ceiling types, soffits and head of wall framing from the prints.
//!
//! - **Details sheet** (`… CEILING TYPES …` / `… SOFFIT … DETAILS`): three kinds of entry, each a
//!   tag in a symbol (ceiling types) or a detail number in a bubble, with its lines beside it,
//!   sorted by the heading of the column they are in — `CEILING TYPES` (boards per layer, tile and
//!   grid, hat or furring channel, suspended main runners, wood furring, rating), `SOFFIT DETAILS`
//!   (bottom and side boards, framing, built under the ceiling or hung from the structure, how far
//!   the side boards go past the ceiling) and `HEAD OF WALL DETAILS` (slotted deflection track,
//!   nested double track, top track at the ceiling, steel angles; the deflection; the wall types
//!   it is for). A general note on bottom track gives the stud walls theirs.
//! - **Room finish schedule**: each room's ceiling type and height; a room it says `SEE RCP`
//!   for gets the type its RCP ceiling (Q264's reader: tile, gypsum, open) and its use point to,
//!   flagged as a guess.
//! - **Soffits** (Q264's reader finds them on the RCP): the detail callout under each (`SEE
//!   3/A-502`) gives its type, else the ceiling it is in does (gypsum: built under it; tile:
//!   hung beside it), flagged; its depth below the ceiling round it; the length of its sides that
//!   show (the edges not on the room's walls).
//!
//! Everything is in building feet (sheet alignment from Q255).

use crate::ceiling_reader::{CeilingResult, FoundRoom};
use crate::sheet_reader::{self, SheetMap};
use crate::truth::{CeilingType, HeadDetail, SoffitType, Truth, WallLayer};
use crate::wall_reader::{WallResult, dist, feet_inches, inch_values, side_item, symbol_centres};
use cadaddy_doc::{Drawing, EntityKind};
use serde_json::{Value, json};

/// Most texts read from one sheet (hostile input).
const MAX_TEXTS: usize = 50_000;
/// Most rows read from a schedule.
const MAX_ROWS: usize = 5_000;

/// A room's ceiling as read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoomRead {
    pub level: String,
    pub room: String,
    /// Building feet (between the room's faces).
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub ceiling_type: Option<String>,
    pub height_ft: Option<f64>,
    /// Why the type is a guess (the schedule didn't name it), when it is.
    pub guess: Option<String>,
}

/// A soffit as read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoffitRead {
    pub level: String,
    pub room: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    /// Its bottom above the floor, feet.
    pub height_ft: Option<f64>,
    pub soffit_type: Option<String>,
    pub guess: Option<String>,
    /// The ceiling round it, minus its bottom, feet.
    pub depth_ft: Option<f64>,
    /// Its sides that show, feet.
    pub sides_ft: f64,
}

/// A wall type's head and bottom as read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WallHead {
    pub wall_type: String,
    pub head: Option<String>,
    pub bottom_track: bool,
}

#[derive(Clone, Debug, Default)]
pub struct AsmResult {
    pub ceiling_types: Vec<CeilingType>,
    pub soffit_types: Vec<SoffitType>,
    pub heads: Vec<HeadDetail>,
    pub rooms: Vec<RoomRead>,
    pub soffits: Vec<SoffitRead>,
    pub walls: Vec<WallHead>,
    /// The details sheet's note gives stud walls a bottom track.
    pub bottom_track_note: bool,
    /// Every detail callout on the RCPs, whether the details sheet has the detail or not.
    pub callouts: Vec<CalloutRead>,
}

/// A detail callout on an RCP (`SEE 3/A-502`): the detail number, where, and the soffit it is
/// beside (an index into [`AsmResult::soffits`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalloutRead {
    pub level: String,
    pub detail: String,
    /// The sheet it points to (`A-502`).
    pub sheet: String,
    pub at_ft: [f64; 2],
    pub soffit: Option<usize>,
}

// ---- Text --------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct T {
    s: String,
    at: [f64; 2],
    h: f64,
}

/// Texts on the drawing's annotation layers (not the title block or its legend).
fn texts(d: &Drawing) -> Vec<T> {
    d.model
        .iter()
        .filter(|e| {
            let l = e.common.layer.to_ascii_uppercase();
            !(l.contains("TITLE") || l.contains("LEGEND") || l.contains("BORDER") || l.contains("GRID"))
        })
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => {
                let p = t.align_pt.unwrap_or(t.insert);
                Some(T { s: t.value.trim().to_string(), at: [p.x, p.y], h: t.height })
            }
            EntityKind::MText(t) => Some(T { s: t.contents.trim().to_string(), at: [t.insert.x, t.insert.y], h: t.height }),
            _ => None,
        })
        .filter(|t| !t.s.is_empty() && t.s.len() < 400 && t.at.iter().all(|v| v.is_finite()) && t.h.is_finite() && t.h > 0.0)
        .take(MAX_TEXTS)
        .collect()
}

/// A tag or detail number: short, no spaces.
fn short_tag(s: &str) -> bool {
    let n = s.chars().count();
    (1..=6).contains(&n) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !s.starts_with('-')
}

// ---- Parsing -----------------------------------------------------------------------------------

/// The inch value after `@` (a spacing), if any.
fn spacing(u: &str) -> Option<f64> {
    let at = u.find('@')?;
    inch_values(u).into_iter().find(|(i, _)| *i > at).map(|(_, v)| v)
}

fn spaced(material: &str, thickness_in: f64, spacing_in: Option<f64>) -> WallLayer {
    WallLayer { material: material.into(), thickness_in, side: "framing".into(), spacing_in, ..WallLayer::default() }
}

/// A framing line → its layer.
fn framing(u: &str) -> Option<WallLayer> {
    let th = inch_values(u).first().map(|x| x.1);
    let o = spacing(u);
    if u.contains("WOOD") {
        Some(spaced("wood_furring", 1.5, o))
    } else if u.contains("STUD") {
        let gauge = u.split(|c: char| !c.is_ascii_alphanumeric() && c != '.').collect::<Vec<_>>().windows(2).find_map(|w| match w {
            [n, g] if g.starts_with("GA") => n.parse::<u32>().ok(),
            _ => None,
        });
        Some(WallLayer { gauge, ..spaced("stud", th.unwrap_or(3.625), o) })
    } else if u.contains("MAIN RUNNER") || u.contains("COLD-ROLLED") || u.contains("CARRYING") {
        Some(spaced("main_runner", th.unwrap_or(1.5), o))
    } else if u.contains("HAT CHANNEL") {
        Some(spaced("hat_channel", th.unwrap_or(0.875), o))
    } else if u.contains("FURRING") {
        Some(spaced("furring", th.unwrap_or(0.875), o))
    } else if u.contains("TEE GRID") || u.contains("SUSPENSION SYSTEM") || (u.contains("GRID") && !u.contains("TILE")) {
        // The tees' depth.
        Some(spaced("act_grid", 1.5, None))
    } else {
        None
    }
}

/// Boards: `(2) LAYERS 5/8" TYPE X GYP. BD.` or several joined with `+`, face first.
fn boards(u: &str) -> Vec<WallLayer> {
    u.split('+')
        .flat_map(|p| side_item(p.trim()))
        .map(|mut l| {
            l.side = "finish".into();
            l
        })
        .collect()
}

fn rating(u: &str) -> Option<Option<String>> {
    if !(u.contains("RATING") || u.contains("RATED") || u.contains("HOUR") || u.contains(" HR")) {
        return None;
    }
    if u.contains("NONE") || u.contains("NON-RATED") {
        return Some(None);
    }
    Some(u.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse::<u32>().ok()).find(|n| (1..=4).contains(n)).map(|h| format!("{h} HR")))
}

/// A ceiling type's legend lines (the first is its name).
pub fn parse_ceiling(tag: &str, lines: &[String]) -> CeilingType {
    let mut c = CeilingType { tag: tag.into(), ..CeilingType::default() };
    let all = lines.join(" ").to_ascii_uppercase();
    c.kind = if all.contains("EXPOSED STRUCTURE") || all.contains("OPEN TO STRUCTURE") || all.contains("NO CEILING") {
        "OPEN"
    } else if all.contains("TILE") || all.contains("ACT") || all.contains("LAY-IN") {
        "ACT"
    } else {
        "GYP"
    }
    .into();
    let (mut finish, mut frame) = (Vec::new(), Vec::new());
    for (i, line) in lines.iter().enumerate() {
        let u = line.to_ascii_uppercase();
        let u = u.trim();
        if i == 0 {
            c.name = line.trim().to_string();
            continue;
        }
        if c.kind == "OPEN" {
            continue;
        }
        if u.contains("TILE") || u.contains("LAY-IN") {
            let th = inch_values(u).first().map_or(0.625, |x| x.1);
            finish.push(WallLayer { material: "act_tile".into(), thickness_in: th, side: "finish".into(), ..WallLayer::default() });
            // `2'x4'`
            let feet: Vec<f64> = u
                .split(['X', 'x'])
                .filter_map(|p| {
                    let p = p.trim();
                    let end = p.find('\'')?;
                    let head = p.get(..end)?;
                    let digits: String = head.chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
                    digits.parse::<f64>().ok()
                })
                .collect();
            if let [a, b, ..] = feet.as_slice()
                && (1.0..=10.0).contains(a)
                && (1.0..=10.0).contains(b)
            {
                c.grid_ft = Some([*a, *b]);
            }
        } else if let Some(r) = rating(u) {
            c.fire_rating = r;
        } else if let Some(l) = framing(u) {
            frame.push(l);
        } else if u.contains("GYP") || u.contains("GWB") || u.contains("BOARD") {
            finish.extend(boards(u));
        }
    }
    c.layers = finish.into_iter().chain(frame).collect();
    c
}

/// A soffit detail's lines.
pub fn parse_soffit(tag: &str, lines: &[String]) -> SoffitType {
    let mut s = SoffitType { tag: tag.into(), hangs_from: "ceiling".into(), ..SoffitType::default() };
    for (i, line) in lines.iter().enumerate() {
        let u = line.to_ascii_uppercase();
        let u = u.trim();
        if i == 0 {
            s.name = line.trim().to_string();
            if u.contains("STRUCTURE") || u.contains("HUNG") {
                s.hangs_from = "structure".into();
            }
            continue;
        }
        if let Some(rest) = u.strip_prefix("BOTTOM:") {
            s.bottom = boards(rest);
        } else if let Some(rest) = u.strip_prefix("SIDES:").or_else(|| u.strip_prefix("SIDE:")) {
            s.sides = boards(rest);
        } else if let Some(rest) = u.strip_prefix("FRAMING:") {
            s.framing.extend(framing(rest.trim()));
        } else if u.contains("ABOVE CEILING") || u.contains("CEILING STOPS") {
            s.hangs_from = "structure".into();
            s.above_ceiling_in = inch_values(u).first().map_or(0.0, |x| x.1);
        }
    }
    s
}

/// A head of wall detail's lines.
pub fn parse_head(tag: &str, lines: &[String]) -> HeadDetail {
    let mut h = HeadDetail { tag: tag.into(), ..HeadDetail::default() };
    let all = lines.join(" ").to_ascii_uppercase();
    h.name = lines.first().map(|l| l.trim().to_string()).unwrap_or_default();
    let title = h.name.to_ascii_uppercase();
    h.to = if title.contains("CEILING") { "ceiling" } else { "deck" }.into();
    h.track = if all.contains("DOUBLE") || all.contains("NESTED") {
        "double"
    } else if all.contains("DEFLECTION") || all.contains("SLOTTED") {
        "deflection"
    } else if all.contains("ANGLE") {
        "angle"
    } else {
        "single"
    }
    .into();
    for line in lines.iter().skip(1) {
        let u = line.to_ascii_uppercase();
        if u.contains("DEFLECTION") && h.deflection_in.is_none() && matches!(h.track.as_str(), "deflection" | "double") {
            // The deflection is the inch value right before the word.
            let at = u.rfind("DEFLECTION").unwrap_or(0);
            h.deflection_in = inch_values(&u).into_iter().filter(|(i, _)| *i < at).map(|(_, v)| v).next_back();
        }
        if u.contains("TYPES") {
            let rest =
                u.split_once(':').map(|(_, r)| r.to_string()).or_else(|| u.split_once("TYPES").map(|(_, r)| r.to_string())).unwrap_or_default();
            h.wall_types = rest.split([',', '&']).flat_map(|p| p.split(" AND ")).map(|p| p.trim().to_string()).filter(|p| short_tag(p)).collect();
            h.wall_types.sort();
        }
    }
    h
}

// ---- The details sheet -------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Ceilings,
    Soffits,
    Heads,
}

fn read_details(d: &Drawing, r: &mut AsmResult) {
    let all = texts(d);
    let headings: Vec<(Section, f64, f64)> = all
        .iter()
        .filter_map(|t| {
            let u = t.s.to_ascii_uppercase();
            let s = if u == "CEILING TYPES" || u == "CEILING LEGEND" {
                Section::Ceilings
            } else if u.starts_with("SOFFIT") && u.contains("DETAIL") {
                Section::Soffits
            } else if u.starts_with("HEAD OF WALL") && u.contains("DETAIL") {
                Section::Heads
            } else {
                return None;
            };
            Some((s, t.at[0], t.at[1]))
        })
        .collect();
    if all.iter().any(|t| {
        let u = t.s.to_ascii_uppercase();
        u.contains("BOTTOM TRACK") && u.contains("STUD")
    }) {
        r.bottom_track_note = true;
    }
    let symbols = symbol_centres(d);
    let tags: Vec<&T> = all.iter().filter(|t| short_tag(&t.s) && symbols.iter().any(|(c, rad)| dist(*c, t.at) < rad.max(t.h) * 0.6)).collect();
    let mut blocks: Vec<Vec<&T>> = vec![Vec::new(); tags.len()];
    for t in all.iter().filter(|t| !short_tag(&t.s)) {
        let owner = tags
            .iter()
            .enumerate()
            .filter(|(_, g)| t.at[0] > g.at[0] && t.at[0] - g.at[0] < 6.0 * g.h && g.at[1] >= t.at[1] - g.h)
            .min_by(|(_, a), (_, b)| (a.at[1] - t.at[1]).total_cmp(&(b.at[1] - t.at[1])))
            .map(|(i, _)| i);
        if let Some(b) = owner.and_then(|i| blocks.get_mut(i)) {
            b.push(t);
        }
    }
    for (g, mut lines) in tags.iter().zip(blocks) {
        // The column's heading: the nearest one above, at or left of the tag.
        let Some(section) = headings
            .iter()
            .filter(|(_, x, y)| *x <= g.at[0] + g.h && *y > g.at[1])
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|h| h.0)
        else {
            continue;
        };
        lines.sort_by(|a, b| b.at[1].total_cmp(&a.at[1]));
        let mut kept: Vec<String> = Vec::new();
        let mut last: Option<f64> = None;
        for l in lines {
            if let Some(y) = last
                && y - l.at[1] > 4.0 * l.h
            {
                break;
            }
            last = Some(l.at[1]);
            kept.push(l.s.clone());
        }
        if kept.is_empty() {
            continue;
        }
        match section {
            Section::Ceilings if !r.ceiling_types.iter().any(|c| c.tag == g.s) => r.ceiling_types.push(parse_ceiling(&g.s, &kept)),
            Section::Soffits if !r.soffit_types.iter().any(|c| c.tag == g.s) => r.soffit_types.push(parse_soffit(&g.s, &kept)),
            Section::Heads if !r.heads.iter().any(|c| c.tag == g.s) => r.heads.push(parse_head(&g.s, &kept)),
            _ => {}
        }
    }
}

// ---- The finish schedule -----------------------------------------------------------------------

/// Room number → (ceiling type cell, height cell).
fn read_schedule(d: &Drawing) -> Vec<(String, String, String)> {
    let all = texts(d);
    let mut out = Vec::new();
    // Each table: a `ROOM` header cell, the header cells on its row to its right (up to the next
    // table's `ROOM`).
    let rooms: Vec<&T> = all.iter().filter(|t| t.s.eq_ignore_ascii_case("ROOM")).collect();
    for head in &rooms {
        let next = rooms.iter().map(|r| r.at[0]).filter(|x| *x > head.at[0] + head.h).fold(f64::INFINITY, f64::min);
        let tol = head.h * 0.5;
        let cols: Vec<&T> = all.iter().filter(|t| (t.at[1] - head.at[1]).abs() < tol && t.at[0] >= head.at[0] - tol && t.at[0] < next).collect();
        let find = |pred: &dyn Fn(&str) -> bool| cols.iter().find(|t| pred(&t.s.to_ascii_uppercase())).map(|t| t.at[0]);
        let Some(type_x) = find(&|u| u == "CEILING" || u.contains("CLG. TYPE") || u.contains("CLG TYPE") || u == "CLG.") else { continue };
        let height_x = find(&|u| u.contains("HT") || u.contains("HEIGHT"));
        let mut xs: Vec<f64> = cols.iter().map(|t| t.at[0]).collect();
        xs.sort_by(f64::total_cmp);
        let column = |x: f64| xs.iter().rposition(|c| *c <= x + tol);
        let type_col = column(type_x);
        let height_col = height_x.and_then(column);
        let room_col = column(head.at[0]);
        let mut rows: Vec<(f64, Vec<&T>)> = Vec::new();
        for t in all.iter().filter(|t| t.at[1] < head.at[1] - tol && t.at[0] >= head.at[0] - tol && t.at[0] < next) {
            if let Some((_, v)) = rows.iter_mut().find(|(y, _)| (y - t.at[1]).abs() < tol) {
                v.push(t);
            } else if rows.len() < MAX_ROWS {
                rows.push((t.at[1], vec![t]));
            }
        }
        for (_, cells) in rows {
            let cell = |c: Option<usize>| cells.iter().find(|t| c.is_some() && column(t.at[0]) == c).map(|t| t.s.clone());
            let Some(num) = cell(room_col) else { continue };
            if !num.chars().any(|c| c.is_ascii_digit()) {
                continue;
            }
            out.push((num, cell(type_col).unwrap_or_default(), cell(height_col).unwrap_or_default()));
        }
    }
    out
}

// ---- Putting it together -----------------------------------------------------------------------

fn area(r: &FoundRoom) -> f64 {
    (r.max_ft[0] - r.min_ft[0]) * (r.max_ft[1] - r.min_ft[1])
}

/// The smallest room of `level` holding `p` (the ceiling reader also finds the building's outline).
fn room_at<'a>(rooms: &'a [FoundRoom], level: &str, p: [f64; 2]) -> Option<&'a FoundRoom> {
    rooms
        .iter()
        .filter(|r| r.level == level && p[0] > r.min_ft[0] - 0.3 && p[0] < r.max_ft[0] + 0.3 && p[1] > r.min_ft[1] - 0.3 && p[1] < r.max_ft[1] + 0.3)
        .min_by(|a, b| area(a).total_cmp(&area(b)))
}

fn wet(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    ["MEN", "WOMEN", "TOILET", "RESTROOM", "SHOWER", "LOCKER"].iter().any(|w| u.split_whitespace().any(|x| x == *w))
}

/// The type a room the schedule doesn't name most likely has, and why.
fn guess_type(kind: &str, open: bool, name: &str, types: &[CeilingType]) -> Option<(String, String)> {
    if open {
        return types.iter().find(|t| t.kind == "OPEN").map(|t| (t.tag.clone(), "not scheduled: open to structure on the RCP".into()));
    }
    if kind == "ACT" {
        return types.iter().find(|t| t.kind == "ACT").map(|t| (t.tag.clone(), "not scheduled: a tile grid on the RCP".into()));
    }
    let gyp: Vec<&CeilingType> = types.iter().filter(|t| t.kind == "GYP").collect();
    let mr = |t: &CeilingType| t.layers.iter().any(|l| l.material == "gyp_mr");
    if wet(name)
        && let Some(t) = gyp.iter().find(|t| mr(t))
    {
        return Some((t.tag.clone(), format!("not scheduled: gypsum in a wet room ({name}), the moisture-resistant type")));
    }
    gyp.iter()
        .filter(|t| !mr(t))
        .min_by_key(|t| t.layers.len())
        .or_else(|| gyp.first())
        .map(|t| (t.tag.clone(), "not scheduled: gypsum on the RCP, the plain gypsum type".into()))
}

/// The ceiling type (not open) the schedule names most often.
fn common_type(schedule: &[(String, String, String)], types: &[CeilingType]) -> Option<String> {
    types
        .iter()
        .filter(|t| t.kind != "OPEN")
        .map(|t| (schedule.iter().filter(|(_, x, _)| x.trim() == t.tag).count(), t))
        .max_by_key(|(n, _)| *n)
        .map(|(_, t)| t.tag.clone())
}

/// The sheet a callout points to: `3/A-502` → `A-502`.
fn callout_sheet(s: &str) -> String {
    let u = s.to_ascii_uppercase();
    let Some(at) = u.find("/A-") else { return String::new() };
    u.get(at + 1..).unwrap_or_default().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.').collect()
}

/// A soffit callout: `3/A-502` somewhere in the text → `3`.
fn callout(s: &str) -> Option<String> {
    let u = s.to_ascii_uppercase();
    let at = u.find("/A-")?;
    let head = u.get(..at)?;
    let num: String = head.chars().rev().take_while(char::is_ascii_alphanumeric).collect::<Vec<_>>().into_iter().rev().collect();
    (!num.is_empty()).then_some(num)
}

/// The length of a soffit's edges that aren't on its room's walls, feet.
fn sides_showing(min: [f64; 2], max: [f64; 2], room: Option<&FoundRoom>) -> f64 {
    let near = |a: f64, b: f64| (a - b).abs() < 0.6;
    let (w, h) = (max[0] - min[0], max[1] - min[1]);
    let Some(r) = room else { return 2.0 * (w + h) };
    let mut s = 0.0;
    if !near(min[1], r.min_ft[1]) {
        s += w;
    }
    if !near(max[1], r.max_ft[1]) {
        s += w;
    }
    if !near(min[0], r.min_ft[0]) {
        s += h;
    }
    if !near(max[0], r.max_ft[0]) {
        s += h;
    }
    s
}

/// Read the ceiling assemblies of a set: the details sheet, the finish schedule, the RCPs'
/// soffit callouts; rooms and soffits from Q264's ceiling reader, wall types from Q293's.
pub fn read_assemblies(dxf: &[(String, Vec<u8>)], map: &SheetMap, ceilings: &CeilingResult, walls: &WallResult) -> AsmResult {
    let mut r = AsmResult::default();
    let mut schedule: Vec<(String, String, String)> = Vec::new();
    let mut callouts: Vec<(String, String, [f64; 2])> = Vec::new();
    let mut callout_sheets: Vec<String> = Vec::new();
    for (name, bytes) in dxf {
        let stem = name.trim_end_matches(".dxf");
        let Some(sh) = map.sheets.iter().find(|s| s.number == stem) else { continue };
        if sh.discipline != "A" {
            continue;
        }
        let title = sh.title.to_ascii_uppercase();
        let details = sh.level.is_none() && (title.contains("CEILING TYPE") || title.contains("SOFFIT") || title.contains("HEAD OF WALL"));
        let finishes = title.contains("FINISH SCHEDULE");
        let rcp = title.contains("CEILING") && sh.level.is_some();
        if !(details || finishes || rcp) {
            continue;
        }
        let Ok(d) = cadaddy_io::read_dxf(bytes) else { continue };
        if details {
            read_details(&d, &mut r);
        } else if finishes {
            schedule.extend(read_schedule(&d));
        } else if let (Some(level), Some(al)) = (&sh.level, &sh.alignment) {
            for t in texts(&d) {
                if let Some(n) = callout(&t.s) {
                    callouts.push((level.clone(), n, sheet_reader::building_from_model(al, t.at)));
                    callout_sheets.push(callout_sheet(&t.s));
                }
            }
        }
    }
    // Rooms: the smallest rectangles (the outline isn't a room), each with its schedule row.
    let rooms: Vec<&FoundRoom> = ceilings
        .rooms
        .iter()
        .filter(|rm| !rm.number.is_empty())
        .filter(|rm| !ceilings.rooms.iter().any(|o| o.level == rm.level && o.number == rm.number && area(o) < area(rm) - 1e-6))
        .collect();
    for rm in &rooms {
        let row = schedule.iter().find(|(n, _, _)| *n == rm.number);
        let found = ceilings.ceilings.iter().find(|c| c.level == rm.level && c.room_number == rm.number);
        let named = row.map(|x| x.1.trim().to_string()).filter(|t| r.ceiling_types.iter().any(|c| &c.tag == t));
        let height = row.and_then(|x| feet_inches(&x.2)).or_else(|| found.and_then(|c| c.height_ft));
        let (ceiling_type, guess) = match named {
            Some(t) => (Some(t), None),
            None => match found {
                // The RCP reads open but the schedule gives a height: not open.
                Some(c) if !(c.open_to_structure && row.and_then(|x| feet_inches(&x.2)).is_some()) => {
                    match guess_type(&c.kind, c.open_to_structure, &rm.name, &r.ceiling_types) {
                        Some((t, why)) => (Some(t), Some(why)),
                        None => (None, Some("not scheduled and no ceiling type fits".into())),
                    }
                }
                // Nothing read on the RCP, but the schedule gives a height: the commonest type
                // the schedule names for rooms with a ceiling.
                _ if height.is_some() => (
                    common_type(&schedule, &r.ceiling_types),
                    Some("not scheduled, no ceiling read on the RCP but a height in the schedule: the commonest ceiling type".into()),
                ),
                _ => (
                    r.ceiling_types.iter().find(|t| t.kind == "OPEN").map(|t| t.tag.clone()),
                    Some("not scheduled, no ceiling or height read: open to structure".into()),
                ),
            },
        };
        r.rooms.push(RoomRead {
            level: rm.level.clone(),
            room: rm.number.clone(),
            min_ft: rm.min_ft,
            max_ft: rm.max_ft,
            ceiling_type,
            height_ft: height,
            guess,
        });
    }
    // Soffits: their callout (the nearest within a few feet), else the ceiling they are in.
    for sf in &ceilings.soffits {
        let mid = [(sf.min_ft[0] + sf.max_ft[0]) / 2.0, (sf.min_ft[1] + sf.max_ft[1]) / 2.0];
        let room = room_at(&ceilings.rooms, &sf.level, mid);
        let read = room.and_then(|rm| r.rooms.iter().find(|x| x.level == rm.level && x.room == rm.number));
        let gap = |p: [f64; 2]| {
            let dx = (sf.min_ft[0] - p[0]).max(p[0] - sf.max_ft[0]).max(0.0);
            let dy = (sf.min_ft[1] - p[1]).max(p[1] - sf.max_ft[1]).max(0.0);
            dx.hypot(dy)
        };
        let called = callouts
            .iter()
            .filter(|(lv, n, p)| *lv == sf.level && gap(*p) < 2.5 && r.soffit_types.iter().any(|s| &s.tag == n))
            .min_by(|a, b| gap(a.2).total_cmp(&gap(b.2)))
            .map(|(_, n, _)| n.clone());
        let ceiling_kind =
            read.and_then(|x| x.ceiling_type.as_ref()).and_then(|t| r.ceiling_types.iter().find(|c| &c.tag == t)).map(|c| c.kind.clone());
        let (soffit_type, guess) = match called {
            Some(n) => (Some(n), None),
            None => {
                let want = if ceiling_kind.as_deref() == Some("GYP") { "ceiling" } else { "structure" };
                let t = r.soffit_types.iter().find(|s| s.hangs_from == want).map(|s| s.tag.clone());
                let why = if want == "ceiling" {
                    "no callout: under a gypsum ceiling, built under it"
                } else {
                    "no callout: beside a tile ceiling, hung from the structure"
                };
                (t, Some(why.to_string()))
            }
        };
        let depth = match (read.and_then(|x| x.height_ft), sf.height_ft) {
            (Some(c), Some(b)) if c > b => Some(c - b),
            _ => None,
        };
        r.soffits.push(SoffitRead {
            level: sf.level.clone(),
            room: room.map(|x| x.number.clone()).unwrap_or_else(|| sf.room_number.clone()),
            min_ft: sf.min_ft,
            max_ft: sf.max_ft,
            height_ft: sf.height_ft,
            soffit_type,
            guess,
            depth_ft: depth,
            sides_ft: sides_showing(sf.min_ft, sf.max_ft, room),
        });
    }
    // Every callout, with the soffit it is nearest (within a few feet).
    for (i, (lv, n, p)) in callouts.iter().enumerate().take(MAX_ROWS) {
        let gap = |sf: &SoffitRead| {
            let dx = (sf.min_ft[0] - p[0]).max(p[0] - sf.max_ft[0]).max(0.0);
            let dy = (sf.min_ft[1] - p[1]).max(p[1] - sf.max_ft[1]).max(0.0);
            dx.hypot(dy)
        };
        // Of the soffits it is beside, the one whose middle is nearest (a callout sits under its
        // soffit's middle; a soffit next door may be as close to it).
        let centre = |sf: &SoffitRead| ((sf.min_ft[0] + sf.max_ft[0]) / 2.0 - p[0]).hypot((sf.min_ft[1] + sf.max_ft[1]) / 2.0 - p[1]);
        let soffit = r
            .soffits
            .iter()
            .enumerate()
            .filter(|(_, sf)| sf.level == *lv && gap(sf) < 2.5)
            .min_by(|a, b| centre(a.1).total_cmp(&centre(b.1)))
            .map(|(k, _)| k);
        r.callouts.push(CalloutRead {
            level: lv.clone(),
            detail: n.clone(),
            sheet: callout_sheets.get(i).cloned().unwrap_or_default(),
            at_ft: *p,
            soffit,
        });
    }
    // Wall types: the head detail listing each, the bottom track note for stud walls.
    for t in &walls.types {
        let head = r.heads.iter().find(|h| h.wall_types.contains(&t.tag)).map(|h| h.tag.clone());
        let studs = t.layers.iter().any(|l| l.material == "stud");
        r.walls.push(WallHead { wall_type: t.tag.clone(), head, bottom_track: r.bottom_track_note && studs });
    }
    r
}

/// Generate a set, recognise its sheets, read ceilings, structure, walls, then assemblies.
pub fn read_fixture_assemblies(seed: u64) -> Result<(Truth, WallResult, AsmResult), String> {
    let set = crate::generate(seed)?;
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = sheet_reader::recognise_fixture_set(&dxf);
    let ceilings = crate::ceiling_reader::read_ceilings(&dxf, &map);
    let drawings: Vec<(String, Drawing)> =
        dxf.iter().filter_map(|(n, b)| cadaddy_io::read_dxf(b).ok().map(|d| (n.trim_end_matches(".dxf").to_string(), d))).collect();
    let refs: Vec<(String, &Drawing)> = drawings.iter().map(|(n, d)| (n.clone(), d)).collect();
    let structure = crate::struct_reader::read_structure(&map, &refs);
    let walls = crate::wall_reader::read_walls(&dxf, &map, &ceilings, &structure);
    let asm = read_assemblies(&dxf, &map, &ceilings, &walls);
    Ok((set.truth, walls, asm))
}

/// What a perfect read of `t` would give (rooms between their faces): to compare takeoffs with.
pub fn result_from_truth(t: &Truth) -> AsmResult {
    let mut r = AsmResult {
        ceiling_types: t.ceiling_types.clone(),
        soffit_types: t.soffit_types.clone(),
        heads: t.head_details.clone(),
        bottom_track_note: true,
        ..AsmResult::default()
    };
    for rc in &t.room_ceilings {
        let Some(rm) = t.building.rooms.iter().find(|x| x.level == rc.level && x.number == rc.room) else { continue };
        r.rooms.push(RoomRead {
            level: rc.level.clone(),
            room: rc.room.clone(),
            min_ft: [rm.min_ft[0] + 0.25, rm.min_ft[1] + 0.25],
            max_ft: [rm.max_ft[0] - 0.25, rm.max_ft[1] - 0.25],
            ceiling_type: Some(rc.ceiling_type.clone()),
            height_ft: rm.ceiling.as_ref().map(|c| c.height_ft),
            guess: None,
        });
    }
    for sf in &t.soffits {
        r.soffits.push(SoffitRead {
            level: sf.level.clone(),
            room: sf.room.clone(),
            min_ft: sf.min_ft,
            max_ft: sf.max_ft,
            height_ft: Some(sf.height_ft),
            soffit_type: sf.soffit_type.clone(),
            guess: None,
            depth_ft: sf.depth_ft,
            sides_ft: sf.sides_ft.unwrap_or(0.0),
        });
    }
    for wt in &t.wall_types {
        r.walls.push(WallHead { wall_type: wt.tag.clone(), head: wt.head.clone(), bottom_track: wt.bottom_track });
    }
    r
}

// ---- Scoring -----------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct AsmScore {
    pub ceiling_types_total: usize,
    pub ceiling_types_found: usize,
    /// Layer stacks exactly right (material, thickness, spacing, in order), kind and rating.
    pub ceiling_stacks_correct: usize,
    pub ceiling_kinds_correct: usize,
    pub ceiling_ratings_correct: usize,
    pub soffit_types_total: usize,
    /// Found with bottom, sides, framing and what they hang from all right.
    pub soffit_types_correct: usize,
    pub heads_total: usize,
    /// Head details right (track, to, deflection, wall types).
    pub heads_correct: usize,
    /// Wall types given the right head and bottom track.
    pub wall_heads_total: usize,
    pub wall_heads_correct: usize,
    pub rooms_total: usize,
    pub rooms_correct: usize,
    /// Rooms the schedule says `SEE RCP` for: flagged, and guessed right.
    pub rooms_unscheduled: usize,
    pub rooms_unscheduled_flagged: usize,
    pub rooms_unscheduled_correct: usize,
    pub soffits_total: usize,
    pub soffits_found: usize,
    pub soffit_type_correct: usize,
    /// Soffits without a callout: flagged.
    pub soffits_uncalled: usize,
    pub soffits_uncalled_flagged: usize,
    /// Depth within 1", sides within 1'.
    pub soffit_depth_correct: usize,
    pub soffit_sides_correct: usize,
}

fn same_layers(a: &[WallLayer], b: &[WallLayer]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.material == y.material
                && (x.thickness_in - y.thickness_in).abs() < 1e-3
                && x.spacing_in.map(|v| (v * 8.0).round()) == y.spacing_in.map(|v| (v * 8.0).round())
        })
}

pub fn score_assemblies(t: &Truth, r: &AsmResult) -> AsmScore {
    let mut sc = AsmScore {
        ceiling_types_total: t.ceiling_types.len(),
        soffit_types_total: t.soffit_types.len(),
        heads_total: t.head_details.len(),
        wall_heads_total: t.wall_types.len(),
        soffits_total: t.soffits.len(),
        ..AsmScore::default()
    };
    for c in &t.ceiling_types {
        let Some(f) = r.ceiling_types.iter().find(|x| x.tag == c.tag) else { continue };
        sc.ceiling_types_found += 1;
        sc.ceiling_stacks_correct += usize::from(same_layers(&c.layers, &f.layers) && c.grid_ft == f.grid_ft);
        sc.ceiling_kinds_correct += usize::from(c.kind == f.kind);
        sc.ceiling_ratings_correct += usize::from(c.fire_rating == f.fire_rating);
    }
    for s in &t.soffit_types {
        let Some(f) = r.soffit_types.iter().find(|x| x.tag == s.tag) else { continue };
        sc.soffit_types_correct += usize::from(
            same_layers(&s.bottom, &f.bottom)
                && same_layers(&s.sides, &f.sides)
                && same_layers(&s.framing, &f.framing)
                && s.hangs_from == f.hangs_from
                && (s.above_ceiling_in - f.above_ceiling_in).abs() < 1e-3,
        );
    }
    for h in &t.head_details {
        let Some(f) = r.heads.iter().find(|x| x.tag == h.tag) else { continue };
        let defl = match (h.deflection_in, f.deflection_in) {
            (Some(a), Some(b)) => (a - b).abs() < 1e-3,
            (None, None) => true,
            _ => false,
        };
        sc.heads_correct += usize::from(h.track == f.track && h.to == f.to && defl && h.wall_types == f.wall_types);
    }
    for w in &t.wall_types {
        let f = r.walls.iter().find(|x| x.wall_type == w.tag);
        sc.wall_heads_correct += usize::from(f.is_some_and(|f| f.head == w.head && f.bottom_track == w.bottom_track));
    }
    for rc in &t.room_ceilings {
        sc.rooms_total += 1;
        let f = r.rooms.iter().find(|x| x.level == rc.level && x.room == rc.room);
        let right = f.is_some_and(|f| f.ceiling_type.as_deref() == Some(rc.ceiling_type.as_str()));
        sc.rooms_correct += usize::from(right);
        if !rc.scheduled {
            sc.rooms_unscheduled += 1;
            sc.rooms_unscheduled_flagged += usize::from(f.is_some_and(|f| f.guess.is_some()));
            sc.rooms_unscheduled_correct += usize::from(right);
        }
    }
    for s in &t.soffits {
        let overlap = |f: &SoffitRead| {
            let w = (s.max_ft[0].min(f.max_ft[0]) - s.min_ft[0].max(f.min_ft[0])).max(0.0);
            let h = (s.max_ft[1].min(f.max_ft[1]) - s.min_ft[1].max(f.min_ft[1])).max(0.0);
            w * h / ((s.max_ft[0] - s.min_ft[0]) * (s.max_ft[1] - s.min_ft[1])).max(1e-9)
        };
        let f = r.soffits.iter().filter(|f| f.level == s.level && overlap(f) > 0.5).max_by(|a, b| overlap(a).total_cmp(&overlap(b)));
        if !s.called_out {
            sc.soffits_uncalled += 1;
        }
        let Some(f) = f else { continue };
        sc.soffits_found += 1;
        sc.soffit_type_correct += usize::from(f.soffit_type == s.soffit_type);
        if !s.called_out {
            sc.soffits_uncalled_flagged += usize::from(f.guess.is_some());
        }
        if let (Some(a), Some(b)) = (s.depth_ft, f.depth_ft) {
            sc.soffit_depth_correct += usize::from((a - b).abs() <= 1.0 / 12.0 + 1e-6);
        }
        sc.soffit_sides_correct += usize::from(s.sides_ft.is_some_and(|v| (v - f.sides_ft).abs() <= 1.0));
    }
    sc
}

// ---- Into the building spec --------------------------------------------------------------------

fn category(m: &str) -> &'static str {
    match m {
        "act_tile" => "tile",
        "act_grid" => "grid",
        "stud" | "main_runner" | "wood_furring" => "framing",
        "hat_channel" | "furring" | "resilient_channel" => "furring",
        _ => "board",
    }
}

/// A ceiling or soffit layer in plain words.
pub fn layer_words(l: &WallLayer) -> String {
    let d = crate::walls::frac(l.thickness_in, false);
    let o = l.spacing_in.map(|s| format!(" @ {} o.c.", crate::walls::frac(s, false))).unwrap_or_default();
    match l.material.as_str() {
        "act_tile" => format!("{d} acoustic ceiling tile"),
        "act_grid" => "exposed tee grid".into(),
        "hat_channel" => format!("{d} hat channel{o}"),
        "furring" => format!("{d} furring channel{o}"),
        "main_runner" => format!("{d} cold-rolled main runners{o}"),
        "wood_furring" => format!("2x wood furring{o}"),
        _ => crate::wall_reader::layer_words(l),
    }
}

fn layers_json(ls: &[WallLayer]) -> Value {
    Value::Array(
        ls.iter()
            .map(|l| {
                let mut v = json!({"material": layer_words(l), "thickness_in": l.thickness_in, "category": category(&l.material)});
                if let Some(s) = l.spacing_in {
                    v["spacing_in"] = json!(s);
                }
                v
            })
            .collect(),
    )
}

/// A head detail as the building spec takes it.
fn head_json(h: &HeadDetail, bottom_track: bool) -> Value {
    let mut v = json!({"track": h.track, "to": h.to, "bottom_track": bottom_track, "detail": h.tag});
    if let Some(d) = h.deflection_in {
        v["deflection_in"] = json!(d);
    }
    v
}

/// The building spec patch for what was read: ceiling and soffit types, the RCP's ceilings
/// (each room's outline, height and type) and soffits (type, sides that show, how high their
/// sides go). `to_plan` maps a level's building feet to its plan drawing's coordinates (none:
/// the level isn't built). Merge it with the wall reader's patch, then [`add_heads`].
pub fn spec_patch(r: &AsmResult, to_plan: &dyn Fn(&str, [f64; 2]) -> Option<[f64; 2]>) -> Value {
    let ctypes: Vec<Value> = r
        .ceiling_types
        .iter()
        .map(|c| {
            let kind = match c.kind.as_str() {
                "ACT" => "grid",
                "OPEN" => "open",
                _ => "gypsum",
            };
            let mut v = json!({"name": c.tag, "description": c.name, "kind": kind, "layers": layers_json(&c.layers)});
            if let Some([a, b]) = c.grid_ft {
                v["grid_in"] = json!([a * 12.0, b * 12.0]);
            }
            if let Some(f) = &c.fire_rating {
                v["fire_rating"] = json!(f);
            }
            v
        })
        .collect();
    let stypes: Vec<Value> = r
        .soffit_types
        .iter()
        .map(|s| {
            json!({"name": s.tag, "description": s.name, "bottom": layers_json(&s.bottom), "sides": layers_json(&s.sides),
                "framing": layers_json(&s.framing), "hangs_from": s.hangs_from, "above_ceiling_in": s.above_ceiling_in})
        })
        .collect();
    let rect = |level: &str, min: [f64; 2], max: [f64; 2]| -> Option<Vec<[f64; 2]>> {
        [[min[0], min[1]], [max[0], min[1]], [max[0], max[1]], [min[0], max[1]]].iter().map(|p| to_plan(level, *p)).collect()
    };
    let ceilings: Vec<Value> = r
        .rooms
        .iter()
        .filter_map(|rm| {
            let t = rm.ceiling_type.as_ref()?;
            let ct = r.ceiling_types.iter().find(|c| &c.tag == t)?;
            if ct.kind == "OPEN" {
                return None;
            }
            let h = rm.height_ft?;
            let mut v =
                json!({"level": rm.level, "room": rm.room, "outline": rect(&rm.level, rm.min_ft, rm.max_ft)?, "height_ft": h, "ceiling_type": t});
            if let Some(g) = &rm.guess {
                v["guess"] = json!(g);
            }
            Some(v)
        })
        .collect();
    let soffits: Vec<Value> = r
        .soffits
        .iter()
        .filter_map(|s| {
            let h = s.height_ft?;
            let mut v = json!({"level": s.level, "outline": rect(&s.level, s.min_ft, s.max_ft)?, "height_ft": h, "sides_ft": s.sides_ft});
            if let Some(t) = &s.soffit_type {
                v["soffit_type"] = json!(t);
            }
            if let Some(d) = s.depth_ft {
                v["top_ft"] = json!(h + d);
            }
            if let Some(g) = &s.guess {
                v["guess"] = json!(g);
            }
            Some(v)
        })
        .collect();
    json!({"ceiling_types": ctypes, "soffit_types": stypes, "rcp": {"ceilings": ceilings, "soffits": soffits}})
}

/// Give the wall reader's patch's wall types their heads: `assembly.head` from the head detail
/// each is listed in, with its bottom track.
pub fn add_heads(wall_patch: &mut Value, r: &AsmResult) {
    let Some(types) = wall_patch.get_mut("wall_types").and_then(Value::as_array_mut) else { return };
    for t in types {
        let Some(name) = t.get("name").and_then(Value::as_str).map(str::to_string) else { continue };
        let Some(w) = r.walls.iter().find(|w| w.wall_type == name) else { continue };
        let Some(h) = w.head.as_ref().and_then(|h| r.heads.iter().find(|x| &x.tag == h)) else { continue };
        if let Some(a) = t.get_mut("assembly").and_then(Value::as_object_mut) {
            a.insert("head".into(), head_json(h, w.bottom_track));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callouts_and_tags() {
        assert_eq!(callout("SEE 3/A-502").as_deref(), Some("3"));
        assert_eq!(callout("SOFFIT DETAIL 12/A-502").as_deref(), Some("12"));
        assert_eq!(callout("4/A-502").as_deref(), Some("4"));
        assert_eq!(callout("SOFFIT @ 7'-0\""), None);
        assert!(short_tag("GYP-2") && short_tag("3") && !short_tag("SEE RCP"));
    }
}
