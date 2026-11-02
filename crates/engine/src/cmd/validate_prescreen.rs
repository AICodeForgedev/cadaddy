//! The deterministic prescreen (Q6124): what a Validate run can measure without an agent, so the
//! judge starts from numbers instead of reading meshes.
//!
//! - `validate.prescreen {scope?, area?, room?, level?, rules?, all?, cursor?, limit?, path?,
//!   force?}` → findings in the shape of `validate.report`'s (rule, handle, verdict, issue,
//!   expected, found, confidence, severity) with what was measured:
//!   - **G-HOST**: every wall-hosted element (the kinds the rule names) against the nearest built
//!     wall of its level: which wall, which face, how far off that face (inches; negative inside
//!     the wall). Off by more than the rule's `plan_in`, or no wall within 10 ft, is a `fail`.
//!   - **G-HEIGHT**: every wall-height element's height above its floor against the height the
//!     documents give ([`super::validate_height`]: an AFF note tied to it on its plan, the
//!     interior elevation that shows it, the legend row of its mark or kind, the schedule row of
//!     its mark, the general notes of its trade, then the project rules `validate.height.<mark|kind>`
//!     and `mounting_height`), each with its source (sheet, point, words). Off by more than
//!     `height_in` is a `fail`; a model height that is a default no document gives is a
//!     `default` (severity low).
//!   - **G-PRESENT**: every room in scope against what the prints show in it
//!     ([`super::validate_present`]): a typical room's unit type's enlarged plans (turned and
//!     mirrored as the room's layout fit says), else its level's trade plans. Each symbol is
//!     matched to the nearest model element of the same resolved kind within the rule's
//!     `plan_in` (a `pass`, naming the element and how far off it is), else it is `missing` with
//!     its sheet, its sheet point and its place in the room; an element of a trade those plans
//!     show that no symbol stands for is `extra` (low), unless it was read from a sheet not
//!     placed on its level's plan (then it is counted as not compared in `unmeasured`). An
//!     element on a wall's line, outside every room, is the room on its side of the wall. Then
//!     each room type's project-rule set: each item counted in the room (`missing` when fewer,
//!     `extra` when more than an exact count). `present` counts the rooms and verdicts per room
//!     type and per level, and the rooms by what gave their set.
//!
//! Project-rule sets are never built in: domain `custom`, key
//! `prescreen.expect.<room type>`, value a list such as
//! `panel=1; ptac=1; exhaust_fan@ceiling=1; receptacle=4+; water_closet|toilet=1`. An item is
//! one or more names (`|`: any of them; a kind word, a type mark, `EF-*` a mark pattern), an
//! optional mounting (`@wall`, `@ceiling`, `@floor`) and a count (`N`, or `N+` for at least N).
//! The room type is matched against the room's name (`guest room` matches `GUEST ROOM 214`)
//! and its unit type (`unittypes`: the type's id or name); the longest key that matches wins.
//!
//! A rule applies to an element by its kind, its BIM category or its mark. A generic element
//! (the job's devices are kind `device` with the plan tag as their mark) is read as what its
//! records, the job's legend and tables or the project's `validate.kind.<mark>` rules say it is
//! ([`super::validate_kind`]). `checked` lists every rule run (0 when it found nothing to
//! check); `unmatched` counts, by kind and mark, the elements no rule naming kinds applies to,
//! and `unmeasured` the ones a rule applies to but could not measure, by why (G-HEIGHT: kind by
//! kind, `<kind>: no height in any document …`, so it lists only kinds no document gives a
//! height).
//!
//! The same numbers go into each packet's `objects.json` (`prescreen`) and the Validate run's
//! pre-screen counts. The tolerances, severities and on/off of G-HOST, G-HEIGHT and G-PRESENT
//! are the project's (`validate.rules.set`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::projrules::{ProjectRules, RuleDomain};
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::validate::{self as v, Model, ModelWall, Mounting, Prints, WallSpan};
use serde_json::{Value, json};

use super::agentready::ANSWER_MAX;
use super::bspec::current;
use super::validate::{Frame, model_of, prints_of, stored_host};
use super::validate_height::{self as vh, DocHeights};
use super::validate_kind::Kinds;
use super::validate_packet::{Area, compass, grow, inside, point_in, pt, r2, seg_dist};
use super::validate_present as vp;
use super::validate_rules::{self as vr, Effective};
use super::*;
use crate::{Result, Session};

pub const CMD: &str = "validate.prescreen";
/// The project-rule key prefix of a room type's expected set.
pub const EXPECT_PREFIX: &str = "prescreen.expect.";
/// The rules the prescreen measures.
pub const RULES: [&str; 3] = ["G-HOST", "G-HEIGHT", "G-PRESENT"];
/// How far a wall is looked for round an element, feet.
pub const HOST_SEARCH_FT: f64 = 10.0;
/// Items of one expected set, and the largest count, at most.
const MAX_EXPECT_ITEMS: usize = 50;
const MAX_COUNT: u32 = 1_000;
/// Longest room type and item name.
const MAX_NAME: usize = 60;
/// Elements and built parts looked at, at most.
const MAX_ELEMENTS: usize = 200_000;
/// Objects looked at, at most: every one (as [`super::validate::MAX_ENTITIES`]).
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 50;
/// Findings written to `path`, at most.
const MAX_WRITTEN: usize = 200_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Validation Prescreen", run_prescreen)
            .params("{scope?: area|room|level|building|selection (from what is given; building when nothing is), area?: id from validate.areas | {area: [x0,y0,x1,y1] (feet), level}, room?: number or name, level?: name, rules?: [G-HOST|G-HEIGHT|G-PRESENT], all?: bool (list the passes too), cursor?: n, limit?: n (20, at most 50), path?: JSON file for every finding, force?: bool} → {scope, areas, rooms, elements, checked: {rule: {pass, fail, missing, extra}} (every rule run, 0 when it found nothing to check), unmatched: {elements, kinds: {kind: n}, marks: {mark: n}, handles, why} (elements no rule naming kinds applies to), unmeasured: {rule: {why: n}} (G-HEIGHT: kind by kind), total, problems, findings: [{id, rule, handle?, kind?, kind_from?, mark?, verdict (G-HEIGHT: pass|fail|default), issue, expected, found, measured (G-HEIGHT: aff_in, to: centre|top|bottom (the point of the element compared: a height to its top at its top), expected_in, off_in, expected_from, expected_source: {in, from: plan_note|elevation|legend|schedule|general_note|project_rule, what, sheet, at, text}, model_height_from: document|default|rule|user|computed|model), expected_source?, confidence, severity, area?, level, at?, source: \"prescreen\"}], next?, walls: {elements, pieces, over_openings, unmeasured: {walls, why: {why: n}, first}} (the walls G-HOST measures against: pieces on the floor, pieces over an opening, walls whose footprint gives no centreline), outside_floors: {count, flagged, by_part: {part: n}, elements?: [{handle, part, level, at_ft, outside_ft, flagged}] (in the path file)} (every built element more than 2 ft outside its level's footprint, its floor and its shell inside the exterior walls, and whether the build flagged it misplaced), rooms_without_set, present: {by_room_type: {type: {rooms, pass, missing, extra}}, by_level: {level: {…}}, rooms_by_basis: {enlarged_plan, trade_plan, project_rule}}, disabled, ms}: the deterministic checks that need no agent — each wall-hosted element's distance to its wall face (G-HOST: its stored host wall first, else the nearest wall there at its height; under a wall piece over an opening it is in the opening), each element's height above its floor against the height the documents give (an AFF note by it, the interior elevation, the legend, a schedule, the general notes, project rules validate.height.<mark|kind>) (G-HEIGHT), and each room against what the prints show in it — its unit type's enlarged plans turned as its layout fit says, else its level's trade plans: each symbol matched to an element of the same kind within plan_in, else missing (sheet, sheet point, place in the room); elements no symbol stands for extra — and each room type's project-rule set (`prescreen.expect.<room type>`) (G-PRESENT)")
            .noundo()
            .files(),
    ]
}

// ---------------------------------------------------------------- small helpers

/// Lower case, words joined by `_`.
fn words(s: &str) -> String {
    let t: String = s.trim().chars().take(200).map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    t.split('_').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("_")
}

/// Does `hay` (words) hold the words of `needle` in a row?
fn has_words(hay: &str, needle: &str) -> bool {
    let (h, n) = (words(hay), words(needle));
    !n.is_empty() && format!("_{h}_").contains(&format!("_{n}_"))
}

fn hash8(s: &str) -> String {
    super::validate_run::hash8(s)
}

// ---------------------------------------------------------------- the host wall

/// Where an element sits against a wall.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WallFace {
    pub wall: String,
    pub wall_type: Option<String>,
    /// The side of the wall the element is on (the face's outward direction).
    pub face: &'static str,
    /// From the face, inches: positive in front of it, negative inside the wall.
    pub off_face_in: f64,
    pub off_centreline_in: f64,
    pub thickness_in: f64,
    /// Along the wall from its first end, feet.
    pub along_ft: f64,
}

impl WallFace {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "wall": self.wall,
            "type": self.wall_type,
            "face": self.face,
            "off_face_in": r2(self.off_face_in),
            "off_centreline_in": r2(self.off_centreline_in),
            "thickness_in": r2(self.thickness_in),
            "along_ft": r2(self.along_ft),
        })
    }
}

/// The wall whose face is nearest `at` (feet) among `walls`, within `reach` feet of the face.
pub(crate) fn nearest_wall<'a>(at: [f64; 2], walls: impl IntoIterator<Item = &'a ModelWall>, reach: f64) -> Option<WallFace> {
    let mut best: Option<(f64, WallFace)> = None;
    for w in walls {
        let half = (w.thickness_in / 24.0).max(0.0);
        let (lo, hi) = ([w.a[0].min(w.b[0]), w.a[1].min(w.b[1])], [w.a[0].max(w.b[0]), w.a[1].max(w.b[1])]);
        if !inside(grow([lo[0], lo[1], hi[0], hi[1]], reach + half), at) {
            continue;
        }
        let (dist, q) = seg_dist(at, w.a, w.b);
        let off = dist - half;
        if !off.is_finite() || off > reach {
            continue;
        }
        let better = best.as_ref().is_none_or(|(b, f)| off.abs() < *b - 1e-9 || ((off.abs() - *b).abs() <= 1e-9 && w.id < f.wall));
        if !better {
            continue;
        }
        let (dx, dy) = (w.b[0] - w.a[0], w.b[1] - w.a[1]);
        let (nx, ny) = (at[0] - q[0], at[1] - q[1]);
        // On the centreline: the wall's left-hand side.
        let face = if nx.hypot(ny) > 1e-6 { compass(nx, ny) } else { compass(-dy, dx) };
        let len = dx.hypot(dy);
        let along = if len > 1e-9 { ((q[0] - w.a[0]) * dx + (q[1] - w.a[1]) * dy) / len } else { 0.0 };
        best = Some((
            off.abs(),
            WallFace {
                wall: w.id.clone(),
                wall_type: w.wall_type.clone(),
                face,
                off_face_in: off * 12.0,
                off_centreline_in: dist * 12.0,
                thickness_in: w.thickness_in,
                along_ft: along,
            },
        ));
    }
    best.map(|(_, f)| f)
}

/// How far above or below a wall piece over an opening an element may be and still be on it,
/// inches.
const SPAN_SLACK_IN: f64 = 2.0;

/// How an element sits against the walls of its level.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Hosting {
    /// The face it is measured to: its stored host wall's when that is within the tolerance (or
    /// no wall's is), else the nearest face of a wall that is there at its height.
    pub face: Option<WallFace>,
    /// Where `face` comes from: `stored` (its own host wall record) or `nearest`.
    pub from: &'static str,
    /// No wall is there at its height within the tolerance, but it is on the face line of a wall
    /// piece over an opening (below or above it): that face, and the piece's bottom and top,
    /// inches above the floor.
    pub opening: Option<(WallFace, f64, f64)>,
    /// Its stored host wall when the walls of its level don't have it.
    pub stored_missing: Option<String>,
    /// Its stored host wall when it is on another wall: the host's face (none: no piece of it
    /// within reach at its height) and the host's piece over an opening it is under, if any.
    pub stored_elsewhere: Option<(String, Option<WallFace>, Option<(WallFace, f64, f64)>)>,
}

impl Hosting {
    /// Its findings' `measured`: the face, where it comes from, the opening and the stored host.
    pub(crate) fn to_json(&self, z_in: Option<f64>) -> Value {
        let mut m = match (&self.opening, &self.face) {
            (Some((f, _, _)), _) | (None, Some(f)) => f.to_json(),
            (None, None) => json!({"wall": null, "searched_ft": HOST_SEARCH_FT}),
        };
        m["host_from"] = json!(self.from);
        if let Some((_, bottom, top)) = &self.opening {
            m["opening"] = json!({"wall_from_aff_in": r2(*bottom), "wall_to_aff_in": r2(*top), "element_aff_in": z_in.map(r2)});
        }
        if let Some(h) = &self.stored_missing {
            m["stored_host"] = json!({"wall": h, "why": "not among the built walls of its level; measured to the nearest wall"});
        }
        if let Some((h, face, open)) = &self.stored_elsewhere {
            let why = match (face, open) {
                (_, Some((_, bottom, _))) => format!("its record's wall {h} is only over an opening here (from {:.0} in above the floor)", bottom),
                (Some(f), None) => format!("its record's wall {h} is {:.1} in off; it is on another wall", f.off_face_in),
                (None, None) => format!("its record's wall {h} is not within {HOST_SEARCH_FT} ft at its height"),
            };
            m["stored_host"] = json!({"wall": h, "face": face.as_ref().map(WallFace::to_json), "why": why});
        }
        m
    }
}

/// Where an element at `at` (feet), `z_in` above its floor, sits against `walls` and `spans`
/// ([`Hosting`]): on its stored host wall's face when it is within `tol_in` (G-HOST's plan
/// tolerance) of it; else on the nearest face of a wall there at its height; else, when no
/// such face is within the tolerance, in the opening under a wall piece whose face line it is
/// on, or off its stored host's face (else the nearest one).
pub(crate) fn host_of(at: [f64; 2], z_in: Option<f64>, stored: Option<&str>, walls: &[&ModelWall], spans: &[&WallSpan], tol_in: f64) -> Hosting {
    let at_height = |s: &WallSpan| z_in.is_none_or(|z| z >= s.bottom_in - SPAN_SLACK_IN && z <= s.top_in + SPAN_SLACK_IN);
    let of = |id: Option<&str>, w: &ModelWall| id.is_none_or(|h| w.id.eq_ignore_ascii_case(h));
    let solid = |id: Option<&str>| {
        nearest_wall(
            at,
            walls.iter().copied().filter(|w| of(id, w)).chain(spans.iter().filter(|s| at_height(s) && of(id, &s.wall)).map(|s| &s.wall)),
            HOST_SEARCH_FT,
        )
    };
    let open = |id: Option<&str>| {
        spans
            .iter()
            .filter(|s| !at_height(s) && of(id, &s.wall))
            .filter_map(|s| nearest_wall(at, [&s.wall], HOST_SEARCH_FT).map(|f| (f, s.bottom_in, s.top_in)))
            .min_by(|a, b| a.0.off_face_in.abs().total_cmp(&b.0.off_face_in.abs()).then(a.0.wall.cmp(&b.0.wall)))
    };
    let within = |f: &WallFace| f.off_face_in.abs() <= tol_in;
    let mut out = Hosting { from: "nearest", ..Hosting::default() };
    let mut host = None;
    if let Some(h) = stored.map(str::trim).filter(|h| !h.is_empty()) {
        if walls.iter().any(|w| w.id.eq_ignore_ascii_case(h)) || spans.iter().any(|s| s.wall.id.eq_ignore_ascii_case(h)) {
            host = Some(h);
        } else {
            out.stored_missing = Some(h.to_string());
        }
    }
    let on_host = host.map(|h| (h, solid(Some(h))));
    if let Some((_, Some(f))) = on_host.as_ref().filter(|(_, f)| f.as_ref().is_some_and(within)) {
        out.face = Some(f.clone());
        out.from = "stored";
        return out;
    }
    let near = solid(None);
    if let Some((h, f)) = on_host {
        out.stored_elsewhere = Some((h.to_string(), f.clone(), open(Some(h)).filter(|(o, _, _)| within(o))));
        if near.as_ref().is_some_and(within) {
            out.face = near;
            return out;
        }
        out.opening = open(Some(h)).or_else(|| open(None)).filter(|(o, _, _)| within(o));
        if out.opening.is_some() {
            out.stored_elsewhere = None;
        }
        out.face = f.or(near);
        out.from = if out.face.as_ref().is_some_and(|x| x.wall.eq_ignore_ascii_case(h)) { "stored" } else { "nearest" };
        return out;
    }
    out.opening = open(None).filter(|(o, _, _)| within(o) && near.as_ref().is_none_or(|n| !within(n)));
    out.face = near;
    out
}

// ---------------------------------------------------------------- expected sets

/// One item of a room type's expected set.
#[derive(Clone, Debug, PartialEq)]
pub struct Expect {
    /// Any of these: kind words, type marks, `EF-*` mark patterns (lower case).
    pub names: Vec<String>,
    pub mount: Option<Mounting>,
    pub count: u32,
    /// `N+`: at least N.
    pub at_least: bool,
    /// As written.
    pub text: String,
}

impl Expect {
    fn label(&self) -> String {
        let mut s = format!("{}{} {}", self.count, if self.at_least { "+" } else { "" }, self.names.join(" or "));
        if let Some(m) = self.mount {
            s.push_str(match m {
                Mounting::Wall => " on a wall",
                Mounting::Ceiling => " in the ceiling",
                Mounting::Floor => " on the floor",
                _ => "",
            });
        }
        s
    }
}

/// Is `key` (a project rule key) a room type's expected set?
pub fn is_expect_key(key: &str) -> bool {
    key.trim().to_ascii_lowercase().starts_with(EXPECT_PREFIX)
}

/// The room type an expected-set key names (words).
fn room_type_of(key: &str) -> String {
    words(key.trim().get(EXPECT_PREFIX.len()..).unwrap_or_default())
}

/// Read an expected set: `kind[|kind…][@wall|@ceiling|@floor]=N[+]`, separated by `;` or `,`.
pub fn parse_expect(value: &str) -> std::result::Result<Vec<Expect>, String> {
    let mut out = Vec::new();
    for part in value.split([';', ',', '\n']).map(str::trim).filter(|p| !p.is_empty()) {
        if out.len() >= MAX_EXPECT_ITEMS {
            return Err(format!("an expected set has at most {MAX_EXPECT_ITEMS} items"));
        }
        let (what, n) = part.split_once('=').ok_or_else(|| format!("`{part}`: write it as kind=count (e.g. ptac=1, receptacle=4+)"))?;
        let n = n.trim();
        let (n, at_least) = match n.strip_suffix('+') {
            Some(x) => (x.trim(), true),
            None => (n, false),
        };
        let count: u32 =
            n.parse().ok().filter(|c| *c <= MAX_COUNT).ok_or_else(|| format!("`{part}`: the count must be a whole number from 0 to {MAX_COUNT}"))?;
        let (names, mount) = match what.split_once('@') {
            Some((a, m)) => {
                let mount = match m.trim().to_ascii_lowercase().as_str() {
                    "wall" => Mounting::Wall,
                    "ceiling" => Mounting::Ceiling,
                    "floor" => Mounting::Floor,
                    other => return Err(format!("`{part}`: the mounting `@{other}` is not @wall, @ceiling or @floor")),
                };
                (a, Some(mount))
            }
            None => (what, None),
        };
        let names: Vec<String> = names.split('|').map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty()).collect();
        if names.is_empty() {
            return Err(format!("`{part}`: name what is expected (a kind such as ptac or exhaust_fan, or a type mark)"));
        }
        if let Some(bad) = names.iter().find(|x| x.chars().count() > MAX_NAME || x.chars().any(|c| c.is_control())) {
            return Err(format!("`{}`: a name is at most {MAX_NAME} printable characters", bad.chars().take(MAX_NAME).collect::<String>()));
        }
        out.push(Expect { names, mount, count, at_least, text: part.chars().take(200).collect() });
    }
    if out.is_empty() {
        return Err("an expected set lists at least one item (e.g. `panel=1; ptac=1; exhaust_fan@ceiling=1`)".into());
    }
    Ok(out)
}

/// Check a project rule that is an expected set (`project.rules.set` refuses a bad one).
pub fn check_expect_rule(domain: RuleDomain, key: &str, value: &str) -> std::result::Result<(), String> {
    if domain != RuleDomain::Custom {
        return Err(format!("`{key}` is an expected set: set it in domain `custom`"));
    }
    let ty = room_type_of(key);
    if ty.is_empty() || ty.len() > MAX_NAME {
        return Err(format!("`{key}`: name the room type after `{EXPECT_PREFIX}` (e.g. `{EXPECT_PREFIX}guest room`), at most {MAX_NAME} characters"));
    }
    parse_expect(value).map(|_| ())
}

/// The project's expected sets: (room type words, items), and the ones that don't read.
pub(crate) fn expected_sets(pr: &ProjectRules) -> (Vec<(String, Vec<Expect>)>, Vec<(String, String)>) {
    let mut sets = Vec::new();
    let mut bad = Vec::new();
    for e in pr.list_domain(RuleDomain::Custom) {
        if !is_expect_key(&e.key) {
            continue;
        }
        match check_expect_rule(RuleDomain::Custom, &e.key, &e.active.value).and_then(|()| parse_expect(&e.active.value)) {
            Ok(items) => sets.push((room_type_of(&e.key), items)),
            Err(why) => bad.push((e.key.clone(), why)),
        }
    }
    sets.sort_by(|a, b| a.0.cmp(&b.0));
    (sets, bad)
}

// ---------------------------------------------------------------- unit types

/// A room's unit type (`unittypes`): its id, name and hand, and the type as the spec keeps it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UnitOf {
    pub id: String,
    pub name: String,
    pub hand: String,
    pub ty: Value,
    /// How the type's plan lies in this room, as the job laid it (`{rotation_deg, mirrored,
    /// view, by}`), when it was fitted.
    pub fit: Value,
}

/// The building spec's unit instances by level and room number (read once).
pub(crate) struct Units(HashMap<(String, String), UnitOf>);

impl Units {
    pub(crate) fn of(d: &Drawing) -> Units {
        let mut out = HashMap::new();
        let Some(spec) = d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()) else { return Units(out) };
        let text = |v: &Value, k: &str| v.get(k).map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string)).unwrap_or_default();
        let types: Vec<&Value> = spec.get("unit_types").and_then(Value::as_array).into_iter().flatten().take(1_000).collect();
        for inst in spec.get("unit_instances").and_then(Value::as_array).into_iter().flatten().take(100_000) {
            let number = Some(text(inst, "number")).filter(|n| !n.is_empty()).unwrap_or_else(|| text(inst, "room"));
            let id = Some(text(inst, "type")).filter(|t| !t.is_empty()).unwrap_or_else(|| text(inst, "unit_type"));
            if number.trim().is_empty() || id.is_empty() {
                continue;
            }
            let ty = types
                .iter()
                .find(|t| text(t, "id").eq_ignore_ascii_case(&id) || text(t, "tag").eq_ignore_ascii_case(&id))
                .map_or(Value::Null, |t| (*t).clone());
            let name = Some(text(&ty, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
            let hand = if text(inst, "hand").eq_ignore_ascii_case("mirrored") { "mirrored" } else { "normal" }.to_string();
            let key = (text(inst, "level").trim().to_ascii_lowercase(), number.trim().to_ascii_lowercase());
            let fit = inst.get("fit").cloned().unwrap_or(Value::Null);
            out.entry(key).or_insert(UnitOf { id, name, hand, ty, fit });
        }
        Units(out)
    }

    /// The unit type of room `number` on `level` (an instance with no level is any level's).
    pub(crate) fn get(&self, level: &str, number: Option<&str>) -> Option<&UnitOf> {
        let n = number.map(str::trim).filter(|n| !n.is_empty())?.to_ascii_lowercase();
        self.0.get(&(level.trim().to_ascii_lowercase(), n.clone())).or_else(|| self.0.get(&(String::new(), n)))
    }
}

/// The unit type of room `number` on `level`, from the building spec's unit instances.
pub(crate) fn unit_of(d: &Drawing, level: &str, number: Option<&str>) -> Option<UnitOf> {
    Units::of(d).get(level, number).cloned()
}

/// The expected set of a room: the longest room-type key matching its name or unit type.
fn set_for<'a>(sets: &'a [(String, Vec<Expect>)], name: Option<&str>, unit: Option<&UnitOf>) -> Option<&'a (String, Vec<Expect>)> {
    sets.iter()
        .filter(|(ty, _)| {
            name.is_some_and(|n| has_words(n, ty)) || unit.is_some_and(|u| has_words(&u.id, ty) || has_words(&u.name, ty) || words(&u.id) == *ty)
        })
        .max_by(|a, b| {
            (a.0.split('_').count(), a.0.len(), std::cmp::Reverse(&a.0)).cmp(&(b.0.split('_').count(), b.0.len(), std::cmp::Reverse(&b.0)))
        })
}

// ---------------------------------------------------------------- what is measured

/// One element the prescreen looks at: a plan element of the model, or a part the build made
/// that no plan element stands for.
#[derive(Clone, Debug)]
struct Item {
    handle: String,
    /// What it is: its own kind, else what its records, the job's tables or the project say
    /// ([`super::validate_kind`]).
    kind: String,
    /// Where `kind` came from (`element`, `stored`, `bim_type`, `legend`, `symbol_table`,
    /// `project_rule`, `unresolved`).
    kind_from: &'static str,
    /// Its BIM category (IFC class).
    category: Option<String>,
    mark: Option<String>,
    name: Option<String>,
    level: String,
    at: [f64; 2],
    /// Its mounting height (a wall item's centre), inches above the floor.
    z_in: Option<f64>,
    /// A built part's bottom and top, inches above the floor (a height measured to its top or
    /// bottom is compared there).
    z_span_in: Option<(f64, f64)>,
    mounting: Mounting,
    built: bool,
    /// The wall its record says it is on (its own `host_wall`, or its built twin's).
    host: Option<String>,
    /// Its own height record (`mount_height`: read off a print, a note, a legend or a schedule,
    /// or a default the job assumed).
    record: Option<vh::Record>,
    /// Its symbol's point on its sheet (model coordinates): plan elements only.
    sheet_at: Option<cadaddy_geom::Vec2>,
    /// The sheet it was read from, when its record says.
    source: Option<String>,
}

/// An element's mounting height as its record has it from the documents (not a rule's or a
/// default): inches, and the record's source.
#[cfg(test)]
fn documented_height(e: &cadaddy_doc::Entity) -> Option<(f64, String)> {
    let r = vh::record_of(e).filter(|r| !r.assumed)?;
    let kind = format!("{:?}", r.kind).to_ascii_lowercase();
    Some((r.inches, format!("its record: {kind} {}", r.doc).trim().to_string()))
}

impl Item {
    /// Does item name `n` (lower case) stand for this element?
    fn is(&self, n: &str) -> bool {
        if let Some(prefix) = n.strip_suffix('*') {
            return self.mark.as_deref().is_some_and(|m| m.to_ascii_lowercase().starts_with(prefix));
        }
        has_words(&self.kind, n)
            || self.mark.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(n))
            || self.name.as_deref().is_some_and(|x| has_words(x, n))
    }
}

/// The plan elements of the model and the parts the build made that no plan element stands for,
/// each with its kind resolved.
fn items(d: &Drawing, sp: &BuildingSpec, model: &Model, frame: &Frame, kinds: &Kinds) -> Vec<Item> {
    let mut out: Vec<Item> = model
        .elements
        .iter()
        .take(MAX_ELEMENTS)
        .map(|e| {
            let ent = cadaddy_doc::Handle::parse_hex(&e.id).and_then(|h| d.model.get(h)).map(|a| a.as_ref());
            let r = kinds.resolve(&e.kind, e.mark.as_deref().or(e.tag.as_deref()), ent);
            Item {
                handle: e.id.clone(),
                mounting: if e.mounting == Mounting::Unknown { v::mounting_of(&r.kind) } else { e.mounting },
                kind: r.kind,
                kind_from: r.from,
                category: r.category,
                mark: e.mark.clone(),
                name: e.fields.get("description").or_else(|| e.fields.get("name")).cloned(),
                level: e.level.clone(),
                at: e.at,
                z_in: e.z_in,
                z_span_in: None,
                built: false,
                host: ent.and_then(stored_host),
                record: ent.and_then(vh::record_of),
                sheet_at: ent.and_then(|x| vh::sheet_point(d, x)),
                source: e.source.clone().filter(|s| !s.trim().is_empty()),
            }
        })
        .collect();
    let plan: BTreeSet<String> = model.elements.iter().map(|e| e.id.to_ascii_uppercase()).collect();
    // A plan element's built twin may hold its host wall.
    let mut twin_host: HashMap<String, String> = HashMap::new();
    const MEP_KINDS: [&str; 5] = ["equipment", "fixture", "device", "light_fixture", "terminal"];
    let upf = frame.upf();
    for e in d.model.iter().take(MAX_ENTITIES) {
        if out.len() >= MAX_ELEMENTS {
            break;
        }
        if super::wallgen::made_as(e).is_none() {
            continue;
        }
        if let Some(src) = super::bimlook::made_source(e).map(|h| h.hex().to_ascii_uppercase()).filter(|h| plan.contains(h)) {
            if let Some(h) = stored_host(e) {
                twin_host.insert(src, h);
            }
            continue;
        }
        let Some(kind) = super::elemprops::element_kind(e).filter(|k| MEP_KINDS.contains(&k.as_str())) else { continue };
        let kind = super::elemprops::reported_kind(e, kind);
        let Some(level) = super::elemprops::level_of(e) else { continue };
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() {
            continue;
        }
        let at = frame.built_on(&level, e, b.center());
        let (floor, ceiling) = sp.level(&level).map_or((0.0, 9.0), |l| (l.elevation_ft, if l.ceiling_ft > 0.0 { l.ceiling_ft } else { l.height_ft }));
        let (z_in, z_span_in, mounting) = match &e.kind {
            EntityKind::Mesh(m) if !m.vertices.is_empty() => {
                let (z0, z1) = m.vertices.iter().take(1_000_000).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.z), b.max(p.z)));
                let (lo, hi) = ((z0 / upf - floor) * 12.0, (z1 / upf - floor) * 12.0);
                if !lo.is_finite() || !hi.is_finite() {
                    (None, None, Mounting::Other)
                } else if hi >= ceiling * 12.0 - 12.0 {
                    (Some(lo), Some((lo, hi)), Mounting::Ceiling)
                } else if lo <= 6.0 {
                    (Some(lo), Some((lo, hi)), Mounting::Floor)
                } else {
                    (Some((lo + hi) / 2.0), Some((lo, hi)), Mounting::Wall)
                }
            }
            _ => (None, None, Mounting::Other),
        };
        let c = super::bimlook::classify(d, e);
        let mark = super::validate_kind::mark_of(e);
        let r = kinds.resolve(&kind, mark.as_deref(), Some(e));
        out.push(Item {
            handle: e.handle.hex(),
            kind: r.kind,
            kind_from: r.from,
            category: r.category,
            mark,
            // What it is in words: its type as read (`water closet P-1`), else its class's.
            name: cadaddy_doc::props::read(&e.common.xdata)
                .get("type")
                .map(|p| p.text())
                .filter(|t| !t.trim().is_empty())
                .or_else(|| c.map(|c| c.ty)),
            level,
            at,
            z_in,
            z_span_in,
            mounting,
            built: true,
            host: stored_host(e),
            // A built part's height record is its own, else the plan element's it was built from.
            record: vh::record_of(e).or_else(|| super::bimlook::made_source(e).and_then(|h| d.model.get(h)).and_then(|x| vh::record_of(x))),
            sheet_at: None,
            source: None,
        });
    }
    for it in out.iter_mut().filter(|it| !it.built && it.host.is_none()) {
        it.host = twin_host.get(&it.handle.to_ascii_uppercase()).cloned();
    }
    out
}

/// The tolerance `name` of a rule, else `default`.
fn tol(r: &Effective, name: &str, default: f64) -> f64 {
    r.rule.tolerance.get(name).copied().filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(default)
}

/// What the prescreen found.
pub(crate) struct Screened {
    /// Problems first (by severity), then passes; each a finding.
    pub findings: Vec<Value>,
    /// Per rule: verdict → count.
    pub checked: BTreeMap<String, BTreeMap<String, u64>>,
    pub elements: usize,
    pub rooms: usize,
    /// Room names (or types) with no expected set.
    pub without_set: BTreeSet<String>,
    pub disabled: Vec<String>,
    pub bad_sets: Vec<(String, String)>,
    /// The elements no rule naming kinds applies to (only the rules for every element do):
    /// how many, by kind and by mark, and the first few.
    pub unmatched: Unmatched,
    /// Per rule: elements it applies to but could not measure, by why.
    pub unmeasured: BTreeMap<String, BTreeMap<String, u64>>,
    /// G-PRESENT's counts per room type and per level.
    pub present: Present,
}

/// G-PRESENT's counts: rooms checked and verdicts, per room type and per level, and the rooms
/// by what gave their set.
#[derive(Clone, Debug, Default)]
pub(crate) struct Present {
    by_type: BTreeMap<String, BTreeMap<&'static str, u64>>,
    by_level: BTreeMap<String, BTreeMap<&'static str, u64>>,
    by_basis: BTreeMap<&'static str, u64>,
}

impl Present {
    /// Kinds of rows listed per table, at most.
    const MAX_ROWS: usize = 60;

    fn empty() -> BTreeMap<&'static str, u64> {
        [("rooms", 0), ("pass", 0), ("missing", 0), ("extra", 0)].into_iter().collect()
    }
    /// A room checked, once, and what gave its set (each source once).
    fn room(&mut self, room_type: &str, level: &str, basis: &'static str, first: bool) {
        *self.by_basis.entry(basis).or_default() += 1;
        if first {
            *self.by_type.entry(room_type.to_string()).or_insert_with(Self::empty).entry("rooms").or_default() += 1;
            *self.by_level.entry(level.to_string()).or_insert_with(Self::empty).entry("rooms").or_default() += 1;
        }
    }
    fn add(&mut self, room_type: &str, level: &str, verdict: &'static str) {
        *self.by_type.entry(room_type.to_string()).or_insert_with(Self::empty).entry(verdict).or_default() += 1;
        *self.by_level.entry(level.to_string()).or_insert_with(Self::empty).entry(verdict).or_default() += 1;
    }
    pub(crate) fn to_json(&self) -> Value {
        let table = |m: &BTreeMap<String, BTreeMap<&'static str, u64>>| -> serde_json::Map<String, Value> {
            m.iter().take(Self::MAX_ROWS).map(|(k, v)| (k.clone(), json!(v))).collect()
        };
        json!({"by_room_type": table(&self.by_type), "by_level": table(&self.by_level), "rooms_by_basis": self.by_basis})
    }
}

/// Elements no rule naming kinds applies to.
#[derive(Clone, Debug, Default)]
pub(crate) struct Unmatched {
    pub elements: u64,
    pub kinds: BTreeMap<String, u64>,
    pub marks: BTreeMap<String, u64>,
    pub handles: Vec<String>,
}

impl Unmatched {
    /// Handles listed, at most.
    const MAX_HANDLES: usize = 20;

    fn add(&mut self, kind: &str, mark: Option<&str>, handle: &str) {
        self.elements += 1;
        *self.kinds.entry(kind.to_string()).or_default() += 1;
        *self.marks.entry(mark.map_or_else(|| "(none)".to_string(), str::to_string)).or_default() += 1;
        if self.handles.len() < Self::MAX_HANDLES {
            self.handles.push(handle.to_string());
        }
    }

    /// Kinds and marks listed, at most (the most common).
    const MAX_LISTED: usize = 30;

    pub(crate) fn to_json(&self) -> Value {
        let top = |m: &BTreeMap<String, u64>| -> serde_json::Map<String, Value> {
            let mut v: Vec<(&String, &u64)> = m.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            v.into_iter().take(Self::MAX_LISTED).map(|(k, n)| (k.clone(), json!(n))).collect()
        };
        json!({
            "elements": self.elements,
            "kinds": top(&self.kinds),
            "marks": top(&self.marks),
            "handles": self.handles,
            "why": if self.elements == 0 { Value::Null } else { json!(format!("no rule but the ones for every element applies to these kinds; name what each mark is with the project rule `{}<mark>` (domain custom), e.g. `{}REC = receptacle`", super::validate_kind::KIND_PREFIX, super::validate_kind::KIND_PREFIX)) },
        })
    }
}

impl Screened {
    pub(crate) fn problems(&self) -> usize {
        self.findings.iter().filter(|f| f["verdict"] != "pass").count()
    }
    pub(crate) fn summary(&self) -> Value {
        json!({"call": CMD, "problems": self.problems(), "checked": self.checked, "unmatched": self.unmatched.elements, "rooms_without_set": self.without_set.len(), "present": self.present.to_json()})
    }
}

/// What is screened: the model, its frame and the areas in scope.
pub(crate) struct Scope<'a> {
    pub d: &'a Drawing,
    pub sp: &'a BuildingSpec,
    pub model: &'a Model,
    pub frame: &'a Frame,
    pub prints: Option<&'a Prints>,
    pub rules: &'a [Effective],
    pub areas: &'a [Area],
    /// The whole of the areas' levels (scope level or building), not only inside the areas.
    pub whole_levels: bool,
    /// Only these rules (empty: all three).
    pub only: &'a [String],
}

/// Run the three checks over a scope.
pub(crate) fn screen(sc: &Scope) -> Screened {
    let rule = |id: &str| sc.rules.iter().find(|r| r.rule.id == id).filter(|_| sc.only.is_empty() || sc.only.iter().any(|o| o == id));
    let disabled: Vec<String> = RULES.iter().filter(|id| rule(id).is_some_and(|r| !r.enabled)).map(|s| s.to_string()).collect();
    let on = |id: &str| rule(id).filter(|r| r.enabled);
    let levels: BTreeSet<String> = sc.areas.iter().map(|a| a.level.to_ascii_lowercase()).collect();
    let mut by_level: HashMap<String, Vec<&Area>> = HashMap::new();
    for a in sc.areas {
        by_level.entry(a.level.to_ascii_lowercase()).or_default().push(a);
    }
    let area_of = |it: &Item| -> Option<&Area> {
        let list = by_level.get(&it.level.to_ascii_lowercase())?;
        list.iter().find(|a| inside(a.bbox, it.at) && (a.outline.len() < 3 || point_in(&a.outline, it.at))).copied()
    };
    let kinds = Kinds::of(sc.d, sc.sp, sc.prints);
    let all = items(sc.d, sc.sp, sc.model, sc.frame, &kinds);
    let mine: Vec<(&Item, Option<&Area>)> = all
        .iter()
        .filter(|it| levels.contains(&it.level.to_ascii_lowercase()))
        .map(|it| (it, area_of(it)))
        .filter(|(it, a)| {
            a.is_some()
                || sc.whole_levels
                || by_level.get(&it.level.to_ascii_lowercase()).is_some_and(|l| l.iter().any(|x| inside(grow(x.bbox, 0.5), it.at)))
        })
        .collect();
    let mut walls: HashMap<String, Vec<&ModelWall>> = HashMap::new();
    for w in &sc.model.walls {
        walls.entry(w.level.to_ascii_lowercase()).or_default().push(w);
    }
    let mut spans: HashMap<String, Vec<&WallSpan>> = HashMap::new();
    for w in &sc.model.wall_spans {
        spans.entry(w.wall.level.to_ascii_lowercase()).or_default().push(w);
    }
    let mut findings: Vec<(u8, Value)> = Vec::new();
    let mut checked: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    // Every rule run is listed, with its counts (0 when it found nothing to check).
    for (id, verdicts) in
        [("G-HOST", &["pass", "fail"][..]), ("G-HEIGHT", &["pass", "fail", "default"][..]), ("G-PRESENT", &["pass", "missing", "extra"][..])]
    {
        if on(id).is_some() {
            checked.insert(id.to_string(), verdicts.iter().map(|v| (v.to_string(), 0)).collect());
        }
    }
    let mut unmeasured: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut unmatched = Unmatched::default();
    let specific: Vec<&Effective> = sc.rules.iter().filter(|r| r.enabled && r.rule.names_kinds()).collect();
    for (it, _) in &mine {
        if !specific.iter().any(|r| r.rule.applies_to_element(&it.kind, it.category.as_deref(), it.mark.as_deref())) {
            unmatched.add(&it.kind, it.mark.as_deref(), &it.handle);
        }
    }
    let applies = |r: &Effective, it: &Item| r.rule.applies_to_element(&it.kind, it.category.as_deref(), it.mark.as_deref());
    let mut tally = |rule: &str, verdict: &str| *checked.entry(rule.to_string()).or_default().entry(verdict.to_string()).or_default() += 1;
    let sev_rank = |s: &str| match s {
        "high" => 0u8,
        "medium" => 1,
        _ => 2,
    };
    let base = |r: &Effective, it: &Item, a: Option<&Area>, verdict: &str, issue: String, expected: String, found: String, measured: Value| {
        let id = format!("PS-{}", hash8(&format!("{}|{}|{verdict}", r.rule.id, it.handle)));
        json!({
            "id": id,
            "rule": r.rule.id,
            "handle": it.handle,
            "kind": it.kind,
            "kind_from": it.kind_from,
            "mark": it.mark,
            "built": it.built,
            "verdict": verdict,
            "issue": issue,
            "expected": expected,
            "found": found,
            "measured": measured,
            "evidence": [],
            "confidence": 1.0,
            "severity": r.rule.severity,
            "area": a.map(|a| a.id.clone()),
            "level": it.level,
            "at": pt([r2(it.at[0]), r2(it.at[1])]),
            "source": "prescreen",
        })
    };
    // G-HOST: the wall face.
    if let Some(r) = on("G-HOST") {
        let t = tol(r, "plan_in", 6.0);
        // A kind whose mounting nothing says (a fire alarm device: a wall horn or a ceiling
        // detector) is not measured against a wall; the judge checks its host.
        let unknown = mine.iter().filter(|(it, _)| it.mounting == Mounting::Other && applies(r, it)).count() as u64;
        if unknown > 0 {
            *unmeasured
                .entry(r.rule.id.clone())
                .or_default()
                .entry("mounting not known (wall or ceiling): the judge checks its host".into())
                .or_default() += unknown;
        }
        for (it, a) in mine.iter().filter(|(it, _)| !matches!(it.mounting, Mounting::Ceiling | Mounting::Other) && applies(r, it)) {
            let lv = it.level.to_ascii_lowercase();
            let h = host_of(
                it.at,
                it.z_in,
                it.host.as_deref(),
                walls.get(&lv).map_or(&[][..], Vec::as_slice),
                spans.get(&lv).map_or(&[][..], Vec::as_slice),
                t,
            );
            let its = if h.from == "stored" { "its host wall" } else { "wall" };
            let (verdict, issue, found) = match (&h.opening, &h.face) {
                (Some((f, bottom, top)), _) => (
                    "fail",
                    format!(
                        "{} is on the {} face line of wall {} but in the opening under it: the wall there spans {:.0} to {:.0} in above the floor, the {} is at {:.1} in",
                        it.kind,
                        f.face,
                        f.wall,
                        bottom,
                        top,
                        it.kind,
                        it.z_in.unwrap_or(0.0)
                    ),
                    format!(
                        "in the opening of wall {} ({:.1} in off its {} face line; the wall there starts {:.0} in above the floor)",
                        f.wall, f.off_face_in, f.face, bottom
                    ),
                ),
                (None, Some(f)) if f.off_face_in.abs() <= t => (
                    "pass",
                    match &h.stored_elsewhere {
                        Some((other, _, _)) => format!("on the {} face of wall {} (its record names wall {other})", f.face, f.wall),
                        None => format!("on the {} face of {its} {}", f.face, f.wall),
                    },
                    format!("{:.1} in off the face", f.off_face_in),
                ),
                (None, Some(f)) => (
                    "fail",
                    if f.off_face_in > 0.0 {
                        format!("{} floats {:.1} in in front of the {} face of {its} {}", it.kind, f.off_face_in, f.face, f.wall)
                    } else {
                        format!("{} is buried {:.1} in inside {its} {}", it.kind, -f.off_face_in, f.wall)
                    },
                    format!("{:.1} in off the {} face of {its} {}", f.off_face_in, f.face, f.wall),
                ),
                (None, None) => {
                    ("fail", format!("{} has no wall within {HOST_SEARCH_FT} ft: it floats", it.kind), format!("no wall within {HOST_SEARCH_FT} ft"))
                }
            };
            let measured = h.to_json(it.z_in);
            tally(&r.rule.id, verdict);
            let mut f = base(r, it, *a, verdict, issue, format!("on a wall face, within {t} in"), found, measured);
            f["tolerance_in"] = json!(t);
            findings.push((if verdict == "pass" { 9 } else { sev_rank(&r.rule.severity) }, f));
        }
    }
    // G-HEIGHT: the height above the floor against the documents.
    if let Some(r) = on("G-HEIGHT") {
        let t = tol(r, "height_in", 2.0);
        let marks: BTreeSet<String> = all.iter().filter_map(|it| it.mark.as_deref()).map(|m| m.trim().to_ascii_uppercase()).collect();
        let mut docs = DocHeights::of(sc.d, sc.sp, &marks);
        let points: Vec<(String, String, cadaddy_geom::Vec2)> =
            mine.iter().filter(|(it, _)| !it.built).filter_map(|(it, _)| it.sheet_at.map(|p| (it.handle.clone(), it.kind.clone(), p))).collect();
        docs.tie(&points);
        // The room an element is in, for an elevation titled with one: its area, else (a device
        // in a wall) the room whose outline is nearest within a foot.
        let room_near = |it: &Item, a: Option<&Area>| -> Option<String> {
            if let Some(n) = a.and_then(|a| a.number.clone()) {
                return Some(n);
            }
            let list = by_level.get(&it.level.to_ascii_lowercase())?;
            let mut near: Vec<(f64, &Area)> = list
                .iter()
                .filter(|x| x.kind != "zone" && x.number.is_some() && x.outline.len() >= 3)
                .filter_map(|x| {
                    let n = x.outline.len();
                    let d = (0..n)
                        .filter_map(|i| Some(seg_dist(it.at, *x.outline.get(i)?, *x.outline.get((i + 1) % n)?).0))
                        .fold(f64::INFINITY, f64::min);
                    (d <= 1.0).then_some((d, *x))
                })
                .collect();
            near.sort_by(|p, q| p.0.total_cmp(&q.0));
            match (near.first(), near.get(1)) {
                (Some(f), Some(s)) if (s.0 - f.0).abs() < 1e-6 && s.1.number != f.1.number => None,
                (Some(f), _) => f.1.number.clone(),
                _ => None,
            }
        };
        let mut why_kind: BTreeMap<String, u64> = BTreeMap::new();
        let mut skip = |why: String| *why_kind.entry(why).or_default() += 1;
        for (it, a) in mine.iter().filter(|(it, _)| it.mounting == Mounting::Wall && applies(r, it)) {
            let Some(z) = it.z_in else {
                skip("no height in the model".to_string());
                continue;
            };
            let origin = vh::model_origin(it.record.as_ref());
            let room = room_near(it, *a);
            let ask = vh::Ask {
                handle: &it.handle,
                kind: &it.kind,
                mark: it.mark.as_deref(),
                room_number: room.as_deref(),
                room_name: a.and_then(|a| a.name.as_deref()),
                trade: super::validate_packet::trade_of_kind(&it.kind, sc.rules),
                record: it.record.as_ref(),
                name: it.name.as_deref(),
                ada: vh::is_ada(it.mark.as_deref(), it.name.as_deref()),
            };
            let Some(want) = docs.expected(&ask, sc.prints, &sc.d.project_rules) else {
                let kind = it.kind.replace('_', " ");
                if origin == Some("default") {
                    // A height nobody documented: reported as the default it is.
                    skip(format!("{kind}: no height in any document or project rule; its model height is a default"));
                    tally(&r.rule.id, "default");
                    let said = it.record.as_ref().map(|x| x.doc.trim().to_string()).filter(|x| !x.is_empty());
                    let issue = format!(
                        "{kind} at {z:.1} in AFF: a default height{}, not from any document",
                        said.as_deref().map_or_else(String::new, |x| format!(" ({})", x.chars().take(80).collect::<String>()))
                    );
                    let measured = json!({"aff_in": r2(z), "expected_in": null, "model_height_from": "default", "model_height_note": said});
                    let mut f = base(
                        r,
                        it,
                        *a,
                        "default",
                        issue,
                        format!(
                            "a height from the documents (an AFF note, the interior elevation, the legend, a schedule, the general notes) or a project rule {}{}",
                            vh::HEIGHT_PREFIX,
                            it.mark.as_deref().unwrap_or(&it.kind)
                        ),
                        format!("{z:.1} in AFF (a default)"),
                        measured,
                    );
                    f["severity"] = json!("low");
                    findings.push((2, f));
                } else {
                    skip(format!(
                        "{kind}: no height in any document (an AFF note by it, an interior elevation, the legend, a schedule, the general notes) or project rule {}<mark|kind>",
                        vh::HEIGHT_PREFIX
                    ));
                }
                continue;
            };
            // A height to its top (or bottom) is compared at its top (bottom).
            let z = match (want.datum, it.z_span_in) {
                (vh::Datum::Top, Some((_, hi))) => hi,
                (vh::Datum::Bottom, Some((lo, _))) => lo,
                _ => z,
            };
            let off = z - want.inches;
            let verdict = if off.abs() <= t { "pass" } else { "fail" };
            tally(&r.rule.id, verdict);
            let src = &want.what;
            let defaulted = if origin == Some("default") { "; its model height is a default, not from a document" } else { "" };
            let issue = if verdict == "pass" {
                format!("{} at {:.1} in AFF as {src} gives{defaulted}", it.kind, z)
            } else {
                format!("{} is at {:.1} in AFF; {src} gives {:.1} in{defaulted}", it.kind, z, want.inches)
            };
            let measured = json!({
                "aff_in": r2(z),
                "to": want.datum.name(),
                "expected_in": r2(want.inches),
                "off_in": r2(off),
                "expected_from": src,
                "expected_source": want.to_json(),
                "model_height_from": origin.unwrap_or("model"),
            });
            let mut f = base(r, it, *a, verdict, issue, format!("{:.1} in AFF (±{t} in)", want.inches), format!("{z:.1} in AFF"), measured);
            f["tolerance_in"] = json!(t);
            f["expected_source"] = want.to_json();
            findings.push((if verdict == "pass" { 9 } else { sev_rank(&r.rule.severity) }, f));
        }
        // Why, kind by kind (the most common first when there are many kinds).
        let mut whys: Vec<(String, u64)> = why_kind.into_iter().collect();
        whys.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
        let listed = unmeasured.entry(r.rule.id.clone()).or_default();
        for (i, (why, n)) in whys.into_iter().enumerate() {
            let key = if i < vh::MAX_KINDS_LISTED { why } else { "other kinds: no height in any document or project rule".to_string() };
            *listed.entry(key).or_default() += n;
        }
        if listed.is_empty() {
            unmeasured.remove(&r.rule.id);
        }
    }
    // G-PRESENT: each room's expected set — what its unit type's enlarged plans (else its
    // level's trade plans) show in it, and its room type's project-rule set.
    let (sets, bad_sets) = expected_sets(&sc.d.project_rules);
    let mut rooms = 0usize;
    let mut without_set = BTreeSet::new();
    let mut room_ids = BTreeSet::new();
    let mut present = Present::default();
    if let Some(r) = on("G-PRESENT") {
        let tol_ft = tol(r, "plan_in", 6.0) / 12.0;
        let units = Units::of(sc.d);
        let symbols = vp::Symbols::of(sc.d, sc.sp, sc.frame, &kinds, sc.rules);
        let base_of = |id: &str| id.split('#').next().unwrap_or(id).to_string();
        let mut by_room: HashMap<String, Vec<&Item>> = HashMap::new();
        for (it, a) in &mine {
            match a {
                Some(a) => by_room.entry(base_of(&a.id)).or_default().push(*it),
                // On a wall (a device the build put on the wall's line, outside every room's
                // outline): the room on its side of the wall, a step past the wall's face.
                None => {
                    let side = walls.get(&it.level.to_ascii_lowercase()).and_then(|ws| {
                        let (dd, q, w) = ws
                            .iter()
                            .map(|w| {
                                let (dd, q) = seg_dist(it.at, w.a, w.b);
                                (dd, q, *w)
                            })
                            .filter(|x| x.0.is_finite())
                            .min_by(|x, y| x.0.total_cmp(&y.0))?;
                        let n = [it.at[0] - q[0], it.at[1] - q[1]];
                        let len = n[0].hypot(n[1]);
                        if dd > vp::WALL_REACH_FT || len < 1e-3 {
                            return None;
                        }
                        let step = (w.thickness_in / 24.0).max(0.0) + 0.3;
                        let p = [q[0] + n[0] / len * step, q[1] + n[1] / len * step];
                        by_level
                            .get(&it.level.to_ascii_lowercase())?
                            .iter()
                            .find(|x| x.kind != "zone" && inside(x.bbox, p) && (x.outline.len() < 3 || point_in(&x.outline, p)))
                            .copied()
                    });
                    if let Some(x) = side {
                        by_room.entry(base_of(&x.id)).or_default().push(*it);
                    }
                }
            }
        }
        let mut parts_of: HashMap<String, Vec<&Area>> = HashMap::new();
        for a in sc.areas.iter().filter(|a| a.kind != "zone") {
            parts_of.entry(base_of(&a.id)).or_default().push(a);
        }
        for a in sc.areas.iter().filter(|a| a.kind != "zone") {
            let room_id = base_of(&a.id);
            if !room_ids.insert(room_id.clone()) {
                continue;
            }
            rooms += 1;
            let parts: &[&Area] = parts_of.get(&room_id).map_or(&[], Vec::as_slice);
            let unit = units.get(&a.level, a.number.as_deref());
            let held: &[&Item] = by_room.get(&room_id).map_or(&[], Vec::as_slice);
            let room_label = a.number.as_deref().map_or_else(|| room_id.clone(), |n| format!("{} {n}", a.name.as_deref().unwrap_or("room")));
            let room_type = unit.map(|u| u.name.clone()).or_else(|| a.name.clone()).unwrap_or_else(|| "room".into()).trim().to_ascii_lowercase();
            let mut counted = false;
            // 1. What the prints show in the room.
            let printed = if symbols.is_empty() {
                None
            } else {
                let look: Vec<vp::Held> = held.iter().map(|it| vp::Held { kind: &it.kind, mark: it.mark.as_deref(), at: it.at }).collect();
                unit.and_then(|u| symbols.enlarged(parts, u, &look, tol_ft)).or_else(|| symbols.trade_plans(parts, tol_ft))
            };
            if let Some(set) = &printed {
                present.room(&room_type, &a.level, set.basis, !counted);
                counted = true;
                let held_trade: Vec<&'static str> = held.iter().map(|it| super::validate_packet::trade_of_kind(&it.kind, sc.rules)).collect();
                // Each expected symbol to its nearest element of the same kind within the
                // tolerance, nearest pairs first.
                let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
                for (i, ex) in set.items.iter().enumerate() {
                    let Some(sym) = symbols.syms.get(ex.sym) else { continue };
                    for (j, it) in held.iter().enumerate() {
                        let dd = (it.at[0] - ex.at[0]).hypot(it.at[1] - ex.at[1]);
                        if dd <= tol_ft && vp::same_kind(&sym.kind, sym.mark.as_deref(), &it.kind, it.mark.as_deref()) {
                            pairs.push((dd, i, j));
                        }
                    }
                }
                pairs.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
                let mut got: HashMap<usize, (usize, f64)> = HashMap::new();
                let mut taken: BTreeSet<usize> = BTreeSet::new();
                for (dd, i, j) in pairs {
                    if got.contains_key(&i) || taken.contains(&j) {
                        continue;
                    }
                    got.insert(i, (j, dd));
                    taken.insert(j);
                }
                let sources = json!(set.sources);
                let tol_in = r2(tol_ft * 12.0);
                for (i, ex) in set.items.iter().enumerate() {
                    let Some(sym) = symbols.syms.get(ex.sym) else { continue };
                    let what = match &sym.mark {
                        Some(m) if !sym.kind.eq_ignore_ascii_case(m) => format!("{} ({m})", sym.kind.replace('_', " ")),
                        _ => sym.kind.replace('_', " "),
                    };
                    let shown_on = ex.view.clone().unwrap_or_else(|| ex.from.clone());
                    let sheet_at = sym.sheet_at.map(|p| pt([r2(p[0]), r2(p[1])]));
                    let source = json!({"sheet": ex.sheet, "view": ex.view, "at": sheet_at, "from": set.basis});
                    let matched = got.get(&i).and_then(|(j, dd)| held.get(*j).map(|it| (*it, *dd)));
                    let verdict = if matched.is_some() { "pass" } else { "missing" };
                    tally(&r.rule.id, verdict);
                    present.add(&room_type, &a.level, verdict);
                    let key = sym.sheet_at.or(sym.plan.as_ref().map(|p| p.1)).unwrap_or(ex.at);
                    let id = format!("PS-{}", hash8(&format!("G-PRESENT|{room_id}|{}|{:.1},{:.1}|{verdict}", ex.sheet, key[0], key[1])));
                    let mut measured = json!({
                        "basis": set.basis, "room_type": room_type, "unit_type": unit.map(|u| u.id.clone()), "symbol": what,
                        "kind": sym.kind, "kind_from": sym.kind_from, "mark": sym.mark, "sheet": ex.sheet, "view": ex.view, "sheet_at": sheet_at,
                        "expected_at": pt([r2(ex.at[0]), r2(ex.at[1])]), "tolerance_in": tol_in,
                    });
                    if !sym.sheet.trim().is_empty() && !sym.sheet.eq_ignore_ascii_case(&ex.sheet) {
                        measured["read_as_sheet"] = json!(sym.sheet);
                    }
                    let (issue, found, handle) = match matched {
                        Some((it, dd)) => {
                            measured["off_in"] = json!(r2(dd * 12.0));
                            (
                                format!("{room_label} has the {what} {shown_on} shows"),
                                format!("{} {:.1} in off", it.kind, dd * 12.0),
                                Some(it.handle.clone()),
                            )
                        }
                        None => (
                            format!("{shown_on} shows a {what} in {room_label}; the model has none of that kind within {tol_in} in of it"),
                            "nothing of that kind there".to_string(),
                            None,
                        ),
                    };
                    let f = json!({
                        "id": id,
                        "rule": r.rule.id,
                        "handle": handle,
                        "kind": sym.kind,
                        "mark": sym.mark,
                        "verdict": verdict,
                        "issue": issue,
                        "expected": format!("a {what} where {shown_on} shows it"),
                        "found": found,
                        "measured": measured,
                        "expected_source": source,
                        "evidence": [],
                        "confidence": 1.0,
                        "severity": r.rule.severity,
                        "area": room_id,
                        "level": a.level,
                        "at": pt([r2(ex.at[0]), r2(ex.at[1])]),
                        "source": "prescreen",
                    });
                    findings.push((if verdict == "pass" { 9 } else { sev_rank(&r.rule.severity) }, f));
                }
                // Elements of the trades those plans show that no symbol stands for.
                let mut uncompared: BTreeMap<String, u64> = BTreeMap::new();
                for (j, it) in held.iter().enumerate() {
                    let trade = held_trade.get(j).copied().unwrap_or("general");
                    if taken.contains(&j) || !set.trades.contains(trade) {
                        continue;
                    }
                    let source_sheet = it.source.clone().or_else(|| print_sheet(sc.d, &it.handle, sc.frame.upf()));
                    // Read from a sheet not placed on this level's plan (nor the room's type's
                    // view): what that sheet shows was not compared, so it is not called extra.
                    if !set.compares(source_sheet.as_deref()) {
                        let sheet = source_sheet.as_deref().unwrap_or_default();
                        *uncompared
                            .entry(format!("elements read from {sheet}, a sheet not placed on its level's plan: not compared"))
                            .or_default() += 1;
                        continue;
                    }
                    tally(&r.rule.id, "extra");
                    present.add(&room_type, &a.level, "extra");
                    let what = match &it.mark {
                        Some(m) if !it.kind.eq_ignore_ascii_case(m) => format!("{} ({m})", it.kind.replace('_', " ")),
                        _ => it.kind.replace('_', " "),
                    };
                    let from = source_sheet.as_deref().map(|s| format!(" (read from {s})")).unwrap_or_default();
                    let mut f = base(
                        r,
                        it,
                        Some(a),
                        "extra",
                        format!("{room_label} has a {what}{from} that no {trade} plan compared here shows there"),
                        format!("only what the {trade} plans show"),
                        format!("a {what} no symbol stands for"),
                        json!({"basis": set.basis, "room_type": room_type, "unit_type": unit.map(|u| u.id.clone()), "trade": trade, "source_sheet": source_sheet, "sources": sources, "tolerance_in": tol_in}),
                    );
                    f["id"] = json!(format!("PS-{}", hash8(&format!("G-PRESENT|{}|extra", it.handle))));
                    f["area"] = json!(room_id);
                    f["severity"] = json!("low");
                    findings.push((2, f));
                }
                let notes = unmeasured.entry(r.rule.id.clone()).or_default();
                for (why, n) in std::mem::take(&mut uncompared) {
                    *notes.entry(why).or_default() += n;
                }
                if set.doubled > 0 {
                    *notes.entry("symbols two views or sheets show, counted once".into()).or_default() += set.doubled as u64;
                }
                if set.foreign > 0 {
                    *notes.entry("symbols whose tag reads as another trade's kind than their sheet's, not expected".into()).or_default() +=
                        set.foreign as u64;
                }
                if notes.is_empty() {
                    unmeasured.remove(&r.rule.id);
                }
            }
            // 2. The room type's project-rule set (counts).
            if let Some((ty, set)) = set_for(&sets, a.name.as_deref(), unit) {
                present.room(&room_type, &a.level, "project_rule", !counted);
                counted = true;
                for ex in set {
                    let found: Vec<&&Item> =
                        held.iter().filter(|it| ex.names.iter().any(|n| it.is(n)) && ex.mount.is_none_or(|m| it.mounting == m)).collect();
                    let n = found.len() as u32;
                    let verdict = if n < ex.count {
                        "missing"
                    } else if !ex.at_least && n > ex.count {
                        "extra"
                    } else {
                        "pass"
                    };
                    tally(&r.rule.id, verdict);
                    present.add(&room_type, &a.level, verdict);
                    let issue = match verdict {
                        "missing" => format!("{room_label} holds {n} of the {} its type ({}) expects", ex.label(), ty.replace('_', " ")),
                        "extra" => format!("{room_label} holds {n} where its type ({}) expects {}", ty.replace('_', " "), ex.label()),
                        _ => format!("{room_label} holds {}", ex.label()),
                    };
                    let handles: Vec<&str> = found.iter().take(50).map(|it| it.handle.as_str()).collect();
                    let id = format!("PS-{}", hash8(&format!("G-PRESENT|{room_id}|{}|{verdict}", ex.text)));
                    let centre = [(a.bbox[0] + a.bbox[2]) / 2.0, (a.bbox[1] + a.bbox[3]) / 2.0];
                    let f = json!({
                        "id": id,
                        "rule": r.rule.id,
                        "verdict": verdict,
                        "issue": issue,
                        "expected": ex.label(),
                        "found": format!("{n}"),
                        "measured": {"basis": "project_rule", "room_type": ty.replace('_', " "), "item": ex.text, "count": n, "handles": handles, "unit_type": unit.map(|u| u.id.clone())},
                        "evidence": [],
                        "confidence": 1.0,
                        "severity": if verdict == "extra" { "low".to_string() } else { r.rule.severity.clone() },
                        "area": room_id,
                        "level": a.level,
                        "at": pt([r2(centre[0]), r2(centre[1])]),
                        "source": "prescreen",
                    });
                    let rank = match verdict {
                        "pass" => 9,
                        "extra" => 2,
                        _ => sev_rank(&r.rule.severity),
                    };
                    findings.push((rank, f));
                }
            }
            if !counted {
                without_set.insert(a.name.clone().or_else(|| unit.map(|u| u.name.clone())).unwrap_or_else(|| room_id.clone()));
            }
        }
    }
    findings.sort_by(|a, b| {
        let k = |f: &Value| {
            (
                f["rule"].as_str().unwrap_or_default().to_string(),
                f["area"].as_str().unwrap_or_default().to_string(),
                f["id"].as_str().unwrap_or_default().to_string(),
            )
        };
        a.0.cmp(&b.0).then_with(|| k(&a.1).cmp(&k(&b.1)))
    });
    Screened {
        findings: findings.into_iter().map(|(_, f)| f).collect(),
        checked,
        elements: mine.len(),
        rooms,
        without_set,
        disabled,
        bad_sets,
        unmatched,
        unmeasured,
        present,
    }
}

/// The sheet an element's properties say they were read from (its first print-sourced one).
fn print_sheet(d: &Drawing, handle: &str, upf: f64) -> Option<String> {
    let e = cadaddy_doc::Handle::parse_hex(handle).and_then(|h| d.model.get(h))?;
    let kind = super::elemprops::element_kind(e).unwrap_or_else(|| "device".into());
    super::elemprops::effective(e, &kind, upf)
        .into_iter()
        .take(200)
        .filter(|(_, pr, _)| pr.has_value() && pr.source.kind == cadaddy_doc::props::SourceKind::Print)
        .find_map(|(_, pr, _)| pr.source.doc.filter(|s| !s.trim().is_empty()).map(|s| s.chars().take(60).collect()))
}

// ---------------------------------------------------------------- the command

fn run_prescreen(s: &mut Session, p: &Value) -> Result<Value> {
    let start = web_time::Instant::now();
    let cursor = p.get("cursor").and_then(Value::as_u64).map_or(0, |c| c.min(10_000_000) as usize);
    let limit = match p.get("limit") {
        None | Some(Value::Null) => DEFAULT_LIMIT,
        Some(l) => l
            .as_u64()
            .map(|l| (l as usize).clamp(1, MAX_LIMIT))
            .ok_or_else(|| bad(CMD, format!("`limit` is a whole number from 1 to {MAX_LIMIT}")))?,
    };
    let only: Vec<String> = match p.get("rules") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .take(10)
            .map(|r| {
                r.as_str()
                    .map(|x| x.trim().to_ascii_uppercase())
                    .filter(|x| RULES.contains(&x.as_str()))
                    .ok_or_else(|| bad(CMD, format!("unknown prescreen rule {r} ({})", RULES.join(", "))))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(_) => return Err(bad(CMD, format!("`rules` is a list of {}", RULES.join(", ")))),
    };
    let all = bool_or(p, "all", false);
    let path = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).map(str::to_string);
    if let Some(f) = &path {
        super::exchange::guard_output(s, CMD, p, f)?;
    }
    let doc = std::sync::Arc::clone(&s.state()?.doc);
    let d: &Drawing = &doc;
    let (sp, _) = current(s, CMD)?;
    let (model, frame) = model_of(d, &sp);
    // The scope: as `validate` takes it, inferred from what is given.
    let (key, areas) = super::validate_run::scope_areas(s, CMD, p, d, &sp, &model, &frame)?;
    let whole = matches!(key["scope"].as_str(), Some("building" | "level"));
    let (rules, _) = vr::effective(&d.project_rules).map_err(|e| bad(CMD, e))?;
    let prints: Option<Prints> = prints_of(&sp).cloned().or_else(|| super::validate::prints_from_job(s, &frame));
    let sc =
        Scope { d, sp: &sp, model: &model, frame: &frame, prints: prints.as_ref(), rules: &rules, areas: &areas, whole_levels: whole, only: &only };
    let out = screen(&sc);
    let wall_list = walls_summary(&model, &areas);
    // Every built element outside its level's footprint (the scope's levels): none may be there
    // unsaid.
    let levels_in: BTreeSet<String> = areas.iter().map(|a| a.level.to_ascii_lowercase()).collect();
    let outside: Vec<Value> = super::propfill::outside_floors(d, &sp)
        .into_iter()
        .filter(|o| levels_in.contains(&o.level.to_ascii_lowercase()))
        .map(|o| {
            let mut v = o.to_json();
            let flagged = d.model.get(o.handle).and_then(|e| cadaddy_doc::props::read(&e.common.xdata).get("misplaced").map(|p| p.text()));
            v["flagged"] = json!(flagged);
            v
        })
        .collect();
    let mut outside_parts: BTreeMap<String, u64> = BTreeMap::new();
    for o in &outside {
        *outside_parts.entry(o["part"].as_str().unwrap_or("").to_string()).or_default() += 1;
    }
    let outside_floors = json!({
        "count": outside.len(),
        "flagged": outside.iter().filter(|o| !o["flagged"].is_null()).count(),
        "by_part": outside_parts,
    });
    let listed: Vec<&Value> = out.findings.iter().filter(|f| all || f["verdict"] != "pass").collect();
    let total = listed.len();
    let mut written = Value::Null;
    if let Some(f) = &path {
        let mut of = outside_floors.clone();
        of["elements"] = json!(outside.iter().take(MAX_WRITTEN).collect::<Vec<_>>());
        let body = json!({"scope": key, "checked": out.checked, "unmatched": out.unmatched.to_json(), "unmeasured": out.unmeasured, "walls": wall_list, "outside_floors": of, "findings": listed.iter().take(MAX_WRITTEN).collect::<Vec<_>>()});
        let text = serde_json::to_string_pretty(&body).map_err(|e| bad(CMD, e.to_string()))?;
        super::exchange::write_file(CMD, f, text.as_bytes())?;
        written = json!(f);
    }
    let mut page: Vec<Value> = listed.iter().skip(cursor).take(limit).map(|f| (*f).clone()).collect();
    let mut r = json!({
        "scope": key,
        "areas": areas.len(),
        "rooms": out.rooms,
        "elements": out.elements,
        "checked": out.checked,
        "unmatched": out.unmatched.to_json(),
        "unmeasured": out.unmeasured,
        "walls": wall_list,
        "outside_floors": outside_floors,
        "total": total,
        "problems": out.problems(),
        "cursor": cursor,
        "findings": [],
        "rooms_without_set": out.without_set.iter().take(20).collect::<Vec<_>>(),
        "present": out.present.to_json(),
        "disabled": out.disabled,
        "ms": start.elapsed().as_millis() as u64,
    });
    if !out.bad_sets.is_empty() {
        r["bad_sets"] = json!(out.bad_sets.iter().take(20).map(|(k, why)| json!({"key": k, "why": why})).collect::<Vec<_>>());
    }
    if !written.is_null() {
        r["path"] = written;
    }
    // A page that fits the answer.
    loop {
        r["findings"] = json!(page);
        let shown = page.len();
        if cursor + shown < total {
            r["next"] = json!({"cursor": cursor + shown});
        } else if let Some(o) = r.as_object_mut() {
            o.remove("next");
        }
        if shown <= 1 || r.to_string().len() <= ANSWER_MAX {
            break;
        }
        page.truncate(shown - 1);
    }
    if r.to_string().len() > ANSWER_MAX {
        r["rooms_without_set"] = json!(out.without_set.len());
    }
    Ok(r)
}

/// The walls G-HOST measures against on the scope's levels: the pieces standing on the floor,
/// the pieces over an opening, and the walls that can't be measured, with why (the first few).
fn walls_summary(model: &Model, areas: &[Area]) -> Value {
    let levels: BTreeSet<String> = areas.iter().map(|a| a.level.to_ascii_lowercase()).collect();
    let on = |l: &str| levels.contains(&l.to_ascii_lowercase());
    let mut why: BTreeMap<&str, u64> = BTreeMap::new();
    let mut first: Vec<Value> = Vec::new();
    for u in model.unmeasured_walls.iter().filter(|u| on(&u.level)) {
        *why.entry(u.why.as_str()).or_default() += 1;
        if first.len() < 10 {
            first.push(json!({"handle": u.id, "level": u.level, "at": pt([r2(u.at[0]), r2(u.at[1])]), "why": u.why}));
        }
    }
    let elements: BTreeSet<&str> = model
        .walls
        .iter()
        .filter(|w| on(&w.level))
        .map(|w| w.id.as_str())
        .chain(model.wall_spans.iter().filter(|s| on(&s.wall.level)).map(|s| s.wall.id.as_str()))
        .chain(model.unmeasured_walls.iter().filter(|u| on(&u.level)).map(|u| u.id.as_str()))
        .collect();
    json!({
        "elements": elements.len(),
        "pieces": model.walls.iter().filter(|w| on(&w.level)).count(),
        "over_openings": model.wall_spans.iter().filter(|s| on(&s.wall.level)).count(),
        "unmeasured": {"walls": why.values().sum::<u64>(), "why": why, "first": first},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(id: &str, a: [f64; 2], b: [f64; 2], t: f64) -> ModelWall {
        ModelWall { id: id.into(), level: "L1".into(), a, b, thickness_in: t, stud_in: None, wall_type: Some("Partition".into()) }
    }

    #[test]
    fn host_of_reads_the_stored_host_then_the_wall_there_at_its_height() {
        // W1 along y = 0 (6"), with a door from x 4 to 7 under its header D1 (from 84"); W2 a
        // partition 2' north.
        let walls =
            [wall("W1", [0.0, 0.25], [4.0, 0.25], 6.0), wall("W1", [7.0, 0.25], [10.0, 0.25], 6.0), wall("W2", [0.0, 2.75], [10.0, 2.75], 6.0)];
        let header = WallSpan { wall: wall("W1", [4.0, 0.25], [7.0, 0.25], 6.0), bottom_in: 84.0, top_in: 108.0 };
        let ws: Vec<&ModelWall> = walls.iter().collect();
        let ss = [&header];
        // Beside the door, at 18": on W1.
        let h = host_of([2.0, 0.55], Some(18.0), None, &ws, &ss, 6.0);
        assert_eq!((h.face.as_ref().map(|f| f.wall.as_str()), h.from, h.opening.is_none()), (Some("W1"), "nearest", true), "{h:?}");
        // Under the header at 18": in the opening.
        let h = host_of([5.5, 0.55], Some(18.0), None, &ws, &ss, 6.0);
        let (f, bottom, _) = h.opening.clone().unwrap_or_else(|| panic!("{h:?}"));
        assert_eq!(f.wall, "W1");
        assert!((f.off_face_in - 0.6).abs() < 1e-6 && bottom == 84.0, "{h:?}");
        // Over the door at 90": on the header.
        let h = host_of([5.5, 0.55], Some(90.0), None, &ws, &ss, 6.0);
        assert!(h.opening.is_none() && h.face.as_ref().is_some_and(|f| f.wall == "W1" && f.off_face_in.abs() < 1.0), "{h:?}");
        // No height known: the header counts.
        let h = host_of([5.5, 0.55], None, None, &ws, &ss, 6.0);
        assert!(h.opening.is_none() && h.face.is_some(), "{h:?}");
        // Its record names W2, which it is on: measured to W2 even where W1 is nearer.
        let h = host_of([2.0, 2.45], Some(18.0), Some("w2"), &ws, &ss, 6.0);
        assert_eq!((h.face.as_ref().map(|f| f.wall.as_str()), h.from), (Some("W2"), "stored"), "{h:?}");
        // Its record names W2, but it is on W1: on W1, the record noted.
        let h = host_of([2.0, 0.55], Some(18.0), Some("W2"), &ws, &ss, 6.0);
        assert_eq!((h.face.as_ref().map(|f| f.wall.as_str()), h.from), (Some("W1"), "nearest"), "{h:?}");
        assert_eq!(h.stored_elsewhere.as_ref().map(|s| s.0.as_str()), Some("W2"), "{h:?}");
        // Its record names a wall the model doesn't have: the nearest, the record noted.
        let h = host_of([2.0, 0.55], Some(18.0), Some("ABC"), &ws, &ss, 6.0);
        assert_eq!((h.face.as_ref().map(|f| f.wall.as_str()), h.stored_missing.as_deref()), (Some("W1"), Some("ABC")), "{h:?}");
        // Floating 1.5' off its stored host with nothing nearer: measured to the host.
        let h = host_of([2.0, 1.25], Some(18.0), Some("W2"), &ws, &ss, 6.0);
        assert_eq!((h.face.as_ref().map(|f| f.wall.as_str()), h.from), (Some("W2"), "stored"), "{h:?}");
        assert!(h.face.as_ref().is_some_and(|f| f.off_face_in > 6.0), "{h:?}");
    }

    #[test]
    fn the_nearest_face_is_measured_signed_from_the_face() {
        let ws = [wall("W1", [0.0, 0.0], [20.0, 0.0], 6.0), wall("W2", [0.0, 0.0], [0.0, 20.0], 6.0)];
        // On the north face of W1 (3 in from the centreline).
        let f = nearest_wall([5.0, 0.25], &ws, 10.0).unwrap();
        assert_eq!(f.wall, "W1");
        assert_eq!(f.face, "north");
        assert!(f.off_face_in.abs() < 1e-6, "{f:?}");
        assert!((f.along_ft - 5.0).abs() < 1e-9);
        // Two feet into the room.
        let f = nearest_wall([5.0, 2.25], &ws, 10.0).unwrap();
        assert!((f.off_face_in - 24.0).abs() < 1e-6, "{f:?}");
        // Inside the wall: negative.
        let f = nearest_wall([5.0, -0.1], &ws, 10.0).unwrap();
        assert_eq!(f.face, "south");
        assert!(f.off_face_in < 0.0);
        // The nearer of two walls; none past the reach.
        assert_eq!(nearest_wall([0.5, 8.0], &ws, 10.0).unwrap().wall, "W2");
        assert!(nearest_wall([50.0, 50.0], &ws, 10.0).is_none());
    }

    #[test]
    fn expected_sets_read_and_bad_ones_are_refused() {
        let set = parse_expect("panel=1; ptac=1; exhaust_fan|ef@ceiling=1, receptacle=4+").unwrap();
        assert_eq!(set.len(), 4);
        assert_eq!(set[2].names, vec!["exhaust_fan", "ef"]);
        assert_eq!(set[2].mount, Some(Mounting::Ceiling));
        assert!(set[3].at_least && set[3].count == 4);
        assert_eq!(set[2].label(), "1 exhaust_fan or ef in the ceiling");
        for bad in ["", "ptac", "ptac=x", "ptac=-1", "ptac=5000", "ptac@roof=1", "=1", "|=1"] {
            assert!(parse_expect(bad).is_err(), "{bad}");
        }
        assert!(check_expect_rule(RuleDomain::Custom, "prescreen.expect.guest room", "ptac=1").is_ok());
        assert!(check_expect_rule(RuleDomain::Tolerance, "prescreen.expect.guest room", "ptac=1").is_err());
        assert!(check_expect_rule(RuleDomain::Custom, "prescreen.expect.", "ptac=1").is_err());
        assert_eq!(room_type_of("prescreen.expect.Guest Room"), "guest_room");
    }

    #[test]
    fn the_longest_matching_room_type_wins() {
        let sets = vec![("room".to_string(), parse_expect("receptacle=1").unwrap()), ("guest_room".to_string(), parse_expect("ptac=1").unwrap())];
        assert_eq!(set_for(&sets, Some("GUEST ROOM"), None).map(|s| s.0.as_str()), Some("guest_room"));
        assert_eq!(set_for(&sets, Some("Storage Room"), None).map(|s| s.0.as_str()), Some("room"));
        assert!(set_for(&sets, Some("Office"), None).is_none());
        let unit = UnitOf { id: "ut-king".into(), name: "Guest Room King".into(), hand: "normal".into(), ty: Value::Null, fit: Value::Null };
        assert_eq!(set_for(&sets, Some("UNIT 214"), Some(&unit)).map(|s| s.0.as_str()), Some("guest_room"));
    }

    /// A hotel's worth (6 levels, 600 rooms, 15,000 elements, 2,400 walls) is screened in
    /// well under the 30 s a whole building may take (the model read is `bim.validate`'s own).
    #[test]
    fn a_hotel_sized_model_is_screened_fast() {
        use cadaddy_doc::projrules::RuleValue;
        use cadaddy_walk3d::validate::Element;
        let sp = BuildingSpec::from_json(
            &json!({"units_per_foot": 12, "levels": (1..=6).map(|i| json!({"name": format!("L{i}"), "elevation_ft": (i - 1) * 10, "height_ft": 10, "ceiling_ft": 9})).collect::<Vec<_>>()})
                .to_string(),
        )
        .unwrap();
        let mut d = Drawing::default();
        d.project_rules.set(RuleDomain::Custom, "prescreen.expect.guest room", RuleValue::new("ptac=1; receptacle=4+; switch=1"), "test");
        d.project_rules.set(RuleDomain::MountingHeight, "receptacle/general", RuleValue::new("18"), "test");
        let frame = Frame::of_spec(&d, &sp);
        let mut model = Model::default();
        let mut areas = Vec::new();
        for l in 1..=6 {
            let level = format!("L{l}");
            for r in 0..100 {
                let (x0, y0) = (f64::from(r % 20) * 15.0, f64::from(r / 20) * 12.0);
                let b = [x0, y0, x0 + 15.0, y0 + 12.0];
                areas.push(Area {
                    id: format!("{level}/{l}{r:02}"),
                    kind: "room",
                    level: level.clone(),
                    number: Some(format!("{l}{r:02}")),
                    name: Some("Guest Room".into()),
                    room: None,
                    stretch: None,
                    outline: vec![[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]],
                    bbox: b,
                });
                for (k, (a, bb)) in [
                    ([x0, y0], [x0 + 15.0, y0]),
                    ([x0, y0], [x0, y0 + 12.0]),
                    ([x0 + 15.0, y0], [x0 + 15.0, y0 + 12.0]),
                    ([x0, y0 + 12.0], [x0 + 15.0, y0 + 12.0]),
                ]
                .into_iter()
                .enumerate()
                {
                    model.walls.push(ModelWall {
                        id: format!("W{l}-{r}-{k}"),
                        level: level.clone(),
                        a,
                        b: bb,
                        thickness_in: 5.0,
                        stud_in: None,
                        wall_type: None,
                    });
                }
                for e in 0..25 {
                    let kind = ["receptacle", "switch", "ptac", "data_outlet", "thermostat"][e % 5];
                    let t = f64::from(e as u32) / 25.0;
                    model.elements.push(Element {
                        id: format!("{l:X}{r:03X}{e:02X}"),
                        kind: kind.into(),
                        level: level.clone(),
                        at: [x0 + 0.5 + 14.0 * t, y0 + 0.3 + if e % 2 == 0 { 0.0 } else { 3.0 }],
                        z_in: Some(18.0),
                        mounting: Mounting::Wall,
                        ..Element::default()
                    });
                }
            }
        }
        let (rules, _) = vr::effective(&d.project_rules).unwrap();
        let t = web_time::Instant::now();
        let out = screen(&Scope {
            d: &d,
            sp: &sp,
            model: &model,
            frame: &frame,
            prints: None,
            rules: &rules,
            areas: &areas,
            whole_levels: true,
            only: &[],
        });
        let ms = t.elapsed().as_millis();
        assert_eq!(out.rooms, 600);
        assert_eq!(out.elements, 15_000);
        let host = out.checked.get("G-HOST").map_or(0, |m| m.values().sum::<u64>());
        assert!(host >= 9_000, "{:?}", out.checked);
        assert!(out.checked.get("G-HEIGHT").is_some_and(|m| m.get("pass").copied().unwrap_or(0) > 0), "{:?}", out.checked);
        assert_eq!(out.checked.get("G-PRESENT").map_or(0, |m| m.values().sum::<u64>()), 1_800);
        assert!(ms < 10_000, "screened in {ms} ms");
        eprintln!("hotel-sized prescreen: {ms} ms, {} findings", out.findings.len());
    }

    /// The kinds the PDF→BIM job builds each get a rule of their own (none is `unmatched`), and a
    /// device whose mounting nothing says is not measured against a wall.
    #[test]
    fn the_jobs_kinds_all_have_rules() {
        use cadaddy_walk3d::validate::Element;
        let sp = BuildingSpec::from_json(
            &json!({"units_per_foot": 12, "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}]}).to_string(),
        )
        .unwrap();
        let d = Drawing::default();
        let frame = Frame::of_spec(&d, &sp);
        let mut model = Model::default();
        for (k, (a, b)) in
            [([0.0, 0.0], [20.0, 0.0]), ([20.0, 0.0], [20.0, 12.0]), ([20.0, 12.0], [0.0, 12.0]), ([0.0, 12.0], [0.0, 0.0])].into_iter().enumerate()
        {
            model.walls.push(ModelWall { id: format!("W{k}"), level: "L1".into(), a, b, thickness_in: 6.0, stud_in: None, wall_type: None });
        }
        let kinds = [
            ("fire_alarm_device", Mounting::Unknown),
            ("horn_strobe", Mounting::Wall),
            ("ptac", Mounting::Unknown),
            ("equipment_connection", Mounting::Unknown),
            ("occupancy_sensor", Mounting::Unknown),
            ("plumbing_fixture", Mounting::Unknown),
            ("return_grille", Mounting::Unknown),
            ("exhaust_grille", Mounting::Unknown),
            ("exhaust_fan", Mounting::Unknown),
        ];
        for (i, (kind, mounting)) in kinds.iter().enumerate() {
            model.elements.push(Element {
                id: format!("E{i:02X}"),
                kind: kind.to_string(),
                level: "L1".into(),
                at: [1.0 + 2.0 * i as f64, 0.25],
                z_in: Some(80.0),
                mounting: *mounting,
                ..Element::default()
            });
        }
        let b = [0.0, 0.0, 20.0, 12.0];
        let areas = vec![Area {
            id: "L1/101".into(),
            kind: "room",
            level: "L1".into(),
            number: Some("101".into()),
            name: Some("Room".into()),
            room: None,
            stretch: None,
            outline: vec![[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]],
            bbox: b,
        }];
        let (rules, _) = vr::effective(&d.project_rules).unwrap();
        let out = screen(&Scope {
            d: &d,
            sp: &sp,
            model: &model,
            frame: &frame,
            prints: None,
            rules: &rules,
            areas: &areas,
            whole_levels: false,
            only: &[],
        });
        assert_eq!(out.elements, kinds.len());
        assert_eq!(out.unmatched.elements, 0, "every kind has a rule: {:?}", out.unmatched.kinds);
        // The horn strobe is measured on its wall; the fire alarm device of no mounting is left to the judge.
        let measured: Vec<&str> = out.findings.iter().filter(|f| f["rule"] == "G-HOST").filter_map(|f| f["handle"].as_str()).collect();
        assert!(measured.contains(&"E01") && !measured.contains(&"E00"), "{measured:?}");
        let why = out.unmeasured.get("G-HOST").map(|m| m.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
        assert!(why.iter().any(|w| w.contains("mounting not known")), "{:?}", out.unmeasured);
    }

    #[test]
    fn an_elements_printed_height_is_its_documented_height_a_default_is_not() {
        use cadaddy_doc::props::{Prop, Props, Source, SourceKind};
        let with = |kind: SourceKind, doc: &str| {
            let mut store = Props::default();
            store.set("mount_height", Prop::new(44.0, Source::from_doc(kind, doc)).with_unit("in"));
            let mut e = cadaddy_doc::Entity::new(
                cadaddy_doc::Handle(1),
                cadaddy_doc::EntityKind::Line(cadaddy_doc::Line {
                    a: cadaddy_geom::Vec3::new(0.0, 0.0, 0.0),
                    b: cadaddy_geom::Vec3::new(1.0, 0.0, 0.0),
                }),
            );
            e.common.xdata = cadaddy_doc::props::write(&[], &store).into();
            e
        };
        let noted = documented_height(&with(SourceKind::Note, "44\" AFF on E-6")).unwrap();
        assert!((noted.0 - 44.0).abs() < 1e-9 && noted.1.contains("E-6"), "{noted:?}");
        assert_eq!(documented_height(&with(SourceKind::Default, "the usual height")), None);
        assert_eq!(documented_height(&with(SourceKind::Rule, "project rule")), None);
    }

    #[test]
    fn items_match_by_kind_words_marks_and_patterns() {
        let it = Item {
            handle: "1A".into(),
            kind: "gfci_receptacle".into(),
            kind_from: "element",
            category: None,
            mark: Some("EF-2".into()),
            name: Some("Duplex Receptacle".into()),
            level: "L1".into(),
            at: [0.0, 0.0],
            z_in: None,
            z_span_in: None,
            mounting: Mounting::Wall,
            built: false,
            host: None,
            record: None,
            sheet_at: None,
            source: None,
        };
        assert!(it.is("receptacle") && it.is("ef-2") && it.is("ef-*") && it.is("duplex"));
        assert!(!it.is("ptac") && !it.is("e*x") && !it.is("panel"));
    }
}
