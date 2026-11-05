//! G-PRESENT's expected sets read off the prints (Q6133): what each room should hold, so the
//! prescreen checks every room the prints show something in, not only the room types a project
//! rule names.
//!
//! - A room of a unit type (`unittypes`, [`super::validate_prescreen::UnitOf`]) expects what its
//!   type's enlarged plans show: the symbols the job read inside each of the type's plan views
//!   (its elevations, sections and details are not plans; a view of one floor's rooms counts
//!   only for a room on that floor). Each view is laid into the room turned and mirrored as the
//!   room's layout fit says for the view it was fitted by; another view of the type takes the
//!   turn that matches the most of the room's elements (the fit's turn on a tie, else the
//!   room's hand). The view's middle goes on the room's middle, then moves by the offset (within
//!   the slack the view's box leaves round the room) that matches the most of the room's
//!   elements, so a view whose box is wider than the room on one side still lands right. A
//!   symbol two views show (the same kind within the tolerance) counts once.
//! - Any other room, or a typical room whose type's views hold nothing the job read, expects the
//!   symbols of its level's trade plans inside it: each sheet (or view) placed on the level's
//!   plan as its registration says (the plan itself; or a sheet the level's plan overlaps, as
//!   drawn). Stored prints (`bim.prints`) give their symbols in plan feet.
//!
//! Symbols are the job's read devices (`job.devices`: tag, kind, sheet point), each read as what
//! its tag is ([`super::validate_kind`]), the way the model's elements are. Nothing here reads
//! the job's placing code; it reads what the job stored.

use std::collections::BTreeSet;

use cadaddy_doc::Drawing;
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::levels::{SheetLink, SheetRegistration};
use serde_json::{Value, json};

use super::validate::Frame;
use super::validate_kind::{self as vk, Kinds};
use super::validate_packet::{
    Area, bbox, corners, grow, inside, intersect, norm_rect, point_in, r2, seg_dist, sheet_kind, shows_level, trade_of_discipline, trade_of_kind,
};
use super::validate_prescreen::UnitOf;
use super::validate_rules::Effective;

/// Symbols read, at most.
const MAX_SYMBOLS: usize = 200_000;
/// Views of one unit type looked at, at most.
const MAX_VIEWS: usize = 40;
/// Symbols of one room, at most.
const MAX_ROOM_SYMBOLS: usize = 1_000;
/// Offsets tried when laying one view into a room, at most.
const MAX_CANDIDATES: usize = 400;
/// The most a view is moved off the room's middle, feet.
const MAX_SLACK_FT: f64 = 15.0;
/// Slack round a view's box beyond the size difference, feet.
const BASE_SLACK_FT: f64 = 2.0;
/// How far outside a room's outline a typical view's symbol may land and still be the room's
/// (a device on its wall), feet.
pub(crate) const WALL_REACH_FT: f64 = 1.0;
/// How far a plan mark of an element's kind (a tag on its leader, a keynote marker) may stand
/// from it and still be its, feet (G-PRESENT's `mark_ft`).
pub(crate) const MARK_REACH_FT: f64 = 6.0;

/// One symbol a print shows.
#[derive(Clone, Debug)]
pub(crate) struct Sym {
    /// What it is, read as the model's elements are.
    pub kind: String,
    pub kind_from: &'static str,
    /// Its tag (the type mark printed by it).
    pub mark: Option<String>,
    pub sheet: String,
    /// Its point on its sheet (drawing units); none for a stored prints symbol.
    pub sheet_at: Option<[f64; 2]>,
    /// A stored prints symbol: its level and plan point (feet).
    pub plan: Option<(String, [f64; 2])>,
    /// Its tag reads as another trade's kind than its sheet's (a plumbing sheet's `L` read as a
    /// luminaire): not expected.
    pub foreign: bool,
}

/// One symbol a room expects, at its place in the room (plan feet).
#[derive(Clone, Debug)]
pub(crate) struct Expected {
    /// Index into [`Symbols::syms`].
    pub sym: usize,
    pub at: [f64; 2],
    /// The view (`sheet title`) it came from, for a unit type's.
    pub view: Option<String>,
    /// The view or sheet it came from (a symbol two of them show counts once).
    pub from: String,
    /// The sheet it lies on (the job's label may differ: [`Sym::sheet`]).
    pub sheet: String,
}

/// A room's expected set off the prints.
#[derive(Clone, Debug, Default)]
pub(crate) struct RoomSet {
    /// `enlarged_plan` (its unit type's views) or `trade_plan` (its level's plans).
    pub basis: &'static str,
    pub items: Vec<Expected>,
    /// The trades whose plans gave the set: only their elements can be `extra`.
    pub trades: BTreeSet<&'static str>,
    /// How each view or sheet was laid: sheet, title, symbols, turn, offset.
    pub sources: Vec<Value>,
    /// Symbols two views or sheets show, counted once.
    pub doubled: usize,
    /// Symbols whose tag reads as another trade's kind than their sheet's, left out.
    pub foreign: usize,
    /// The sheets (numbers, upper case, no view suffix) this room is compared with: its level's
    /// placed plans and its type's views. An element read from another sheet is not compared.
    pub compared: BTreeSet<String>,
}

impl RoomSet {
    /// Is an element read from `sheet` compared here (none known: it is)?
    pub(crate) fn compares(&self, sheet: Option<&str>) -> bool {
        sheet.is_none_or(|s| self.compared.contains(&sheet_base(s)))
    }
}

/// A model element as the matching sees it.
pub(crate) struct Held<'a> {
    pub kind: &'a str,
    pub mark: Option<&'a str>,
    pub at: [f64; 2],
}

/// Are two things (kind and mark each) the same kind? The same tag, or kinds whose words hold
/// each other (`receptacle`, `duplex_receptacle`) or that one's family holds.
pub(crate) fn same_kind(a: &str, a_mark: Option<&str>, b: &str, b_mark: Option<&str>) -> bool {
    if let (Some(x), Some(y)) = (a_mark.map(str::trim), b_mark.map(str::trim))
        && !x.is_empty()
        && x.eq_ignore_ascii_case(y)
    {
        return true;
    }
    let (a, b) = (vk::words(a), vk::words(b));
    if a.is_empty() || b.is_empty() || vk::is_generic(&a) || vk::is_generic(&b) || a == "unresolved" || b == "unresolved" {
        return false;
    }
    let holds = |h: &str, n: &str| format!("_{h}_").contains(&format!("_{n}_"));
    a == b || holds(&a, &b) || holds(&b, &a) || cadaddy_walk3d::validate_kinds::in_family(&a, &b) || cadaddy_walk3d::validate_kinds::in_family(&b, &a)
}

/// `v` mirrored across its first axis for `k >= 4`, then turned `k % 4` quarters
/// counter-clockwise (as the job's layout fit lays a typical view into a room).
pub(crate) fn turn(v: [f64; 2], k: u8) -> [f64; 2] {
    let v = if k >= 4 { [-v[0], v[1]] } else { v };
    match k % 4 {
        1 => [-v[1], v[0]],
        2 => [-v[0], -v[1]],
        3 => [v[1], -v[0]],
        _ => v,
    }
}

/// The turn a layout fit (`{rotation_deg, mirrored}`) names.
pub(crate) fn fit_turn(fit: &Value) -> Option<u8> {
    let deg = fit.get("rotation_deg").and_then(Value::as_f64).filter(|d| d.is_finite())?;
    let q = (deg / 90.0).round().rem_euclid(4.0) as u8 % 4;
    let mirrored = fit.get("mirrored").and_then(Value::as_bool).unwrap_or(false);
    Some(if mirrored { q + 4 } else { q })
}

fn sheet_base(s: &str) -> String {
    s.split('/').next().unwrap_or(s).trim().to_ascii_uppercase()
}

fn rect4(v: &Value) -> Option<[f64; 4]> {
    let r = v.as_array()?;
    let n = |i: usize| r.get(i).and_then(Value::as_f64).filter(|x| x.is_finite());
    Some(norm_rect([n(0)?, n(1)?, n(2)?, n(3)?]))
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Whether a symbol of `kind` belongs on `sheet` by trade: a sheet of no one trade (an
/// architectural one) shows anything, a kind of no one trade goes anywhere, an electrical sheet
/// also shows the fire alarm.
fn fits_sheet(sheet: &str, kind: &str, rules: &[Effective]) -> bool {
    let st = trade_of_discipline(&cadaddy_mepfix::sheet_facts::discipline_of(&sheet_base(sheet)));
    let kt = trade_of_kind(kind, rules);
    st == kt || matches!(st, "general" | "architecture") || kt == "general" || (st == "electrical" && kt == "fire")
}

/// Is `p` in the room (one of `parts`), or within `reach` feet outside its outline?
fn in_room(parts: &[&Area], p: [f64; 2], reach: f64) -> bool {
    parts.iter().any(|a| {
        inside(grow(a.bbox, reach), p)
            && (a.outline.len() < 3
                || point_in(&a.outline, p)
                || (reach > 0.0 && a.outline.iter().zip(a.outline.iter().cycle().skip(1)).any(|(x, y)| seg_dist(p, *x, *y).0 <= reach)))
    })
}

/// How a sheet's point goes onto the level's plan.
enum Place<'a> {
    Same,
    Fit(&'a SheetRegistration),
}

impl Place<'_> {
    fn to_plan(&self, p: [f64; 2]) -> [f64; 2] {
        match self {
            Place::Same => p,
            Place::Fit(r) => r.to_project(p),
        }
    }
}

/// The job's read symbols and the sheets they lie on, read once per screen.
pub(crate) struct Symbols<'a> {
    pub syms: Vec<Sym>,
    /// Each placed sheet (or view of one): the link, its part of the drawing, the symbols in it.
    links: Vec<(&'a SheetLink, [f64; 4], Vec<usize>)>,
    frame: &'a Frame,
    upf: f64,
}

/// What the job read off its sheets, as it stored it: its devices (`job.devices`) and its
/// mechanical and plumbing plans' equipment and terminals (`job.mep`), each as (kind, tag, sheet,
/// sheet point); a sheet left empty is the one the point lies on.
fn job_symbols(d: &Drawing) -> Vec<(String, String, String, [f64; 2])> {
    let Some(agent) = d.agent.as_deref().filter(|t| !t.trim().is_empty()).and_then(|t| serde_json::from_str::<Value>(t).ok()) else {
        return Vec::new();
    };
    let Some(job) = agent.get("guided_job") else { return Vec::new() };
    let list = |k: &str| job.get(k).and_then(Value::as_array).into_iter().flatten().take(MAX_SYMBOLS);
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let point = |v: Option<&Value>| -> Option<[f64; 2]> {
        let a = v?.as_array()?;
        let p = [a.first()?.as_f64()?, a.get(1)?.as_f64()?];
        p.iter().all(|x| x.is_finite()).then_some(p)
    };
    let mut out: Vec<(String, String, String, [f64; 2])> = list("devices")
        .filter_map(|v| serde_json::from_value::<super::readers::ReadDevice>(v.clone()).ok())
        .filter(|dv| dv.at.iter().all(|x| x.is_finite()))
        .map(|dv| (if dv.kind.trim().is_empty() { "device".to_string() } else { dv.kind }, dv.tag.trim().to_string(), dv.sheet, dv.at))
        .collect();
    let mep = job.get("mep");
    let mep_list = |k: &str| mep.and_then(|m| m.get(k)).and_then(Value::as_array).into_iter().flatten().take(MAX_SYMBOLS);
    for e in mep_list("equipment") {
        let (Some(a), Some(b)) = (point(e.get("min")), point(e.get("max"))) else { continue };
        let sheet = e.get("sources").map(|s| text(s, "sheet")).unwrap_or_default();
        out.push((text(e, "kind"), text(e, "tag"), sheet, [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]));
    }
    for e in mep_list("terminals") {
        let Some(at) = point(e.get("at")) else { continue };
        out.push((text(e, "terminal_kind"), text(e, "tag"), text(e, "sheet"), at));
    }
    out.truncate(MAX_SYMBOLS);
    out
}

impl<'a> Symbols<'a> {
    pub(crate) fn of(d: &Drawing, sp: &'a BuildingSpec, frame: &'a Frame, kinds: &Kinds, rules: &[Effective]) -> Symbols<'a> {
        let sheets: Vec<(&str, [f64; 4])> =
            sp.sheets.iter().filter_map(|l| Some((l.number.as_str(), norm_rect(l.rect.filter(|r| r.iter().all(|v| v.is_finite()))?)))).collect();
        let mut syms = Vec::new();
        for (kind, tag, sheet, at) in job_symbols(d) {
            let tag = Some(tag).filter(|t| !t.is_empty());
            let kind = if kind.is_empty() { "device".to_string() } else { kind };
            let r = kinds.resolve(&kind, tag.as_deref(), None);
            let sheet = if sheet.trim().is_empty() {
                sheets.iter().find(|(_, rect)| inside(*rect, at)).map(|(n, _)| (*n).to_string()).unwrap_or_default()
            } else {
                sheet
            };
            let foreign = !fits_sheet(&sheet, &r.kind, rules);
            syms.push(Sym { kind: r.kind, kind_from: r.from, mark: tag, sheet, sheet_at: Some(at), plan: None, foreign });
        }
        for s in sp.prints.as_deref().map_or(&[][..], |p| p.symbols.as_slice()).iter().take(MAX_SYMBOLS.saturating_sub(syms.len())) {
            if !s.at.iter().all(|x| x.is_finite()) {
                continue;
            }
            let mark = s.mark.clone().or_else(|| s.tag.clone()).filter(|m| !m.trim().is_empty());
            let r = kinds.resolve(if s.kind.trim().is_empty() { "device" } else { &s.kind }, mark.as_deref(), None);
            let foreign = !fits_sheet(&s.sheet, &r.kind, rules);
            syms.push(Sym {
                kind: r.kind,
                kind_from: r.from,
                mark,
                sheet: s.sheet.clone(),
                sheet_at: None,
                plan: Some((s.level.clone(), s.at)),
                foreign,
            });
        }
        let mut links = Vec::new();
        for l in sp.sheets.iter().take(10_000) {
            let Some(rect) = l.rect.filter(|r| r.iter().all(|v| v.is_finite())).map(norm_rect) else { continue };
            let clip = l.view.as_ref().map_or(rect, |v| norm_rect(v.rect));
            let ids: Vec<usize> =
                syms.iter().enumerate().filter(|(_, s)| s.sheet_at.is_some_and(|p| inside(clip, p) && inside(rect, p))).map(|(i, _)| i).collect();
            links.push((l, clip, ids));
        }
        let upf = frame.upf();
        Symbols { syms, links, frame, upf: if upf.is_finite() && upf > 1e-9 { upf } else { 12.0 } }
    }

    /// The sheets placed on `level`'s plan (by a registration, or the plan itself).
    fn placed(&self, level: &str) -> BTreeSet<String> {
        self.links.iter().filter(|(l, _, _)| l.registration.is_some() && shows_level(l, level)).map(|(l, _, _)| sheet_base(&l.number)).collect()
    }

    /// Whether any symbol was read.
    pub(crate) fn is_empty(&self) -> bool {
        self.syms.is_empty()
    }

    fn trade_of(&self, i: usize) -> &'static str {
        let sheet = self.syms.get(i).map_or("", |s| s.sheet.as_str());
        trade_of_discipline(&cadaddy_mepfix::sheet_facts::discipline_of(&sheet_base(sheet)))
    }

    /// Add `cand` to `out` unless a symbol of the same kind is already expected within `tol`.
    fn add(&self, out: &mut RoomSet, cand: Expected, tol: f64) {
        let Some(s) = self.syms.get(cand.sym) else { return };
        let twice = out.items.iter().any(|e| {
            e.from != cand.from
                && self.syms.get(e.sym).is_some_and(|o| same_kind(&o.kind, o.mark.as_deref(), &s.kind, s.mark.as_deref()))
                && dist(e.at, cand.at) <= tol
        });
        if twice {
            out.doubled += 1;
        } else if out.items.len() < MAX_ROOM_SYMBOLS {
            out.items.push(cand);
        }
    }

    /// What a room's level's trade plans show inside it (`parts`: the room's areas). None when
    /// no placed plan of the level covers the room.
    pub(crate) fn trade_plans(&self, parts: &[&Area], tol: f64) -> Option<RoomSet> {
        let first = parts.first()?;
        let level = first.level.as_str();
        let room_box = parts.iter().map(|a| a.bbox).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])?;
        let centre = [(room_box[0] + room_box[2]) / 2.0, (room_box[1] + room_box[3]) / 2.0];
        let on_plan: Vec<[f64; 2]> = corners(room_box)
            .iter()
            .map(|q| {
                let p = self.frame.to_sheet(level, *q);
                [p.x, p.y]
            })
            .collect();
        let proj_box = bbox(&on_plan)?;
        let mut out = RoomSet { basis: "trade_plan", compared: self.placed(level), ..RoomSet::default() };
        let mut found: Vec<(usize, [f64; 2], &'static str, String, String)> = Vec::new();
        for (l, clip, ids) in self.links.iter().filter(|(l, _, ids)| !ids.is_empty() && shows_level(l, level)) {
            let title = format!("{} {}", l.title, l.view.as_ref().map_or("", |v| v.title.as_str()));
            if !matches!(sheet_kind(&title, true), "plan" | "rcp" | "enlarged_plan") {
                continue;
            }
            let place = match &l.registration {
                Some(r) if r.method.eq_ignore_ascii_case("reference") => Place::Same,
                Some(r) => Place::Fit(r),
                None if intersect(proj_box, *clip).is_some() => {
                    out.compared.insert(sheet_base(&l.number));
                    Place::Same
                }
                None => continue,
            };
            // The plan covers the room when the room's middle lands inside its part.
            let covers = {
                let p = self.frame.to_sheet(level, centre);
                let mid = [p.x, p.y];
                let back = match &place {
                    Place::Same => Some(mid),
                    Place::Fit(r) => r.to_sheet(mid),
                };
                back.is_some_and(|b| inside(*clip, b))
            };
            let trade = trade_of_discipline(&l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)));
            let name = match &l.view {
                Some(v) => format!("{} {}", l.number, v.title),
                None => l.number.clone(),
            };
            let mut n = 0usize;
            for &i in ids {
                let Some(s) = self.syms.get(i) else { continue };
                let Some(p) = s.sheet_at else { continue };
                let q = place.to_plan(p);
                let ft = self.frame.to_ft_on(level, cadaddy_geom::Vec2::new(q[0], q[1]));
                if ft.iter().all(|v| v.is_finite()) && in_room(parts, ft, 0.0) {
                    if s.foreign {
                        out.foreign += 1;
                        continue;
                    }
                    found.push((i, ft, trade, name.clone(), l.number.clone()));
                    n += 1;
                }
            }
            if covers || n > 0 {
                out.trades.insert(trade);
                if out.sources.len() < MAX_VIEWS {
                    out.sources.push(json!({"sheet": name, "symbols": n, "placed_by": match place { Place::Same => "same_plan", Place::Fit(_) => "registration" }}));
                }
            }
        }
        // Stored prints: plan feet already.
        for (i, s) in self.syms.iter().enumerate() {
            if let Some((lv, at)) = &s.plan
                && lv.eq_ignore_ascii_case(level)
                && in_room(parts, *at, 0.0)
            {
                if s.foreign {
                    out.foreign += 1;
                    continue;
                }
                found.push((i, *at, self.trade_of(i), s.sheet.clone(), s.sheet.clone()));
            }
        }
        if out.trades.is_empty() && found.is_empty() {
            return None;
        }
        for (i, at, trade, from, sheet) in found {
            out.trades.insert(trade);
            self.add(&mut out, Expected { sym: i, at, view: None, from, sheet }, tol);
        }
        Some(out)
    }

    /// What a typical room's unit type's enlarged plans show, laid into the room. None when the
    /// type's plan views hold nothing the job read.
    pub(crate) fn enlarged(&self, parts: &[&Area], unit: &UnitOf, held: &[Held], tol: f64) -> Option<RoomSet> {
        let first = parts.first()?;
        let room_box = parts.iter().map(|a| a.bbox).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])?;
        let centre = [(room_box[0] + room_box[2]) / 2.0, (room_box[1] + room_box[3]) / 2.0];
        let (rw, rh) = (room_box[2] - room_box[0], room_box[3] - room_box[1]);
        let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
        let floor = digits(&first.level);
        let fit_view = unit.fit.get("view").and_then(Value::as_str).map(|v| v.trim().to_ascii_uppercase());
        let fit_k = fit_turn(&unit.fit);
        let hand_k: u8 = if unit.hand == "mirrored" { 4 } else { 0 };
        let mut views: Vec<(String, String, [f64; 4])> = unit
            .ty
            .get("views")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(MAX_VIEWS)
            .filter(|v| {
                let levels: Vec<String> =
                    v.get("levels").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(digits).collect();
                levels.is_empty() || levels.iter().any(|l| !l.is_empty() && *l == floor)
            })
            .filter_map(|v| {
                let sheet = v.get("sheet")?.as_str()?.trim().to_string();
                let title = v.get("title")?.as_str()?.trim().to_string();
                let up = title.to_ascii_uppercase();
                if [" WALL", "ELEVATION", "SECTION", "DETAIL"].iter().any(|w| up.contains(w)) {
                    return None;
                }
                let mut r = rect4(v.get("rect")?)?;
                if let Some(sr) = v.get("sheet_rect").and_then(rect4) {
                    r = intersect(r, sr)?;
                }
                Some((sheet, title, r))
            })
            .collect();
        // The fitted view first.
        let is_fit = |s: &str, t: &str| fit_view.as_deref().is_some_and(|f| f == format!("{s} {t}").to_ascii_uppercase());
        views.sort_by_key(|(s, t, _)| !is_fit(s, t));
        let mut out = RoomSet { basis: "enlarged_plan", compared: self.placed(&first.level), ..RoomSet::default() };
        let mut used: BTreeSet<usize> = BTreeSet::new();
        for (sheet, title, r) in &views {
            let ids: Vec<usize> = self
                .syms
                .iter()
                .enumerate()
                .filter(|(i, s)| {
                    !used.contains(i) && !s.foreign && sheet_base(&s.sheet) == sheet_base(sheet) && s.sheet_at.is_some_and(|p| inside(*r, p))
                })
                .map(|(i, _)| i)
                .take(MAX_ROOM_SYMBOLS)
                .collect();
            if ids.is_empty() {
                continue;
            }
            let mid = [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0];
            let local: Vec<(usize, [f64; 2])> = ids
                .iter()
                .filter_map(|&i| {
                    let p = self.syms.get(i)?.sheet_at?;
                    Some((i, [(p[0] - mid[0]) / self.upf, (p[1] - mid[1]) / self.upf]))
                })
                .collect();
            let (vw, vh) = ((r[2] - r[0]) / self.upf, (r[3] - r[1]) / self.upf);
            let fitted = is_fit(sheet, title) && fit_k.is_some();
            // Turns to try, the preferred first (it wins a tie).
            let turns: Vec<u8> = match (fitted, fit_k) {
                (true, Some(k)) => vec![k],
                (_, Some(k)) => std::iter::once(k).chain(std::iter::once(hand_k)).chain(0..8).collect(),
                (_, None) => std::iter::once(hand_k).chain(0..8).collect(),
            };
            // Another turn than the preferred one must match clearly more (two more, and a
            // quarter more): a few elements match some turn by chance.
            let mut best: Option<(u8, [f64; 2], usize)> = None;
            let mut preferred = 0usize;
            let mut tried: BTreeSet<u8> = BTreeSet::new();
            for k in turns {
                if !tried.insert(k) {
                    continue;
                }
                let (tw, th) = if k % 2 == 1 { (vh, vw) } else { (vw, vh) };
                let slack = (0.5 * (tw - rw).abs().max((th - rh).abs()) + BASE_SLACK_FT).min(MAX_SLACK_FT);
                let (offset, score) = self.lay(&local, k, centre, slack, held, tol);
                match best {
                    None => {
                        preferred = score;
                        best = Some((k, offset, score));
                    }
                    Some((_, _, b)) if score > b && score >= preferred + 2 && score * 4 >= preferred * 5 => best = Some((k, offset, score)),
                    _ => {}
                }
            }
            let Some((k, offset, matched)) = best else { continue };
            let view = format!("{sheet} {title}");
            out.compared.insert(sheet_base(sheet));
            let trade = trade_of_discipline(&cadaddy_mepfix::sheet_facts::discipline_of(&sheet_base(sheet)));
            out.trades.insert(trade);
            // Only what lands in the room (the view may draw the whole unit: its bath, its entry).
            let mut outside = 0usize;
            for (i, l) in &local {
                let t = turn(*l, k);
                used.insert(*i);
                let at = [centre[0] + t[0] + offset[0], centre[1] + t[1] + offset[1]];
                if !in_room(parts, at, WALL_REACH_FT) {
                    outside += 1;
                    continue;
                }
                self.add(&mut out, Expected { sym: *i, at, view: Some(view.clone()), from: view.clone(), sheet: sheet.clone() }, tol);
            }
            if out.sources.len() < MAX_VIEWS {
                out.sources.push(json!({
                    "sheet": view,
                    "symbols": local.len(),
                    "outside_room": outside,
                    "rotation_deg": f64::from(k % 4) * 90.0,
                    "mirrored": k >= 4,
                    "turn_by": if fitted { "layout_fit" } else { "symbols" },
                    "offset_ft": [r2(offset[0]), r2(offset[1])],
                    "matched": matched,
                }));
            }
        }
        (!out.items.is_empty()).then_some(out)
    }

    /// The offset (feet, off the room's middle, within `slack`) that lays the view's symbols
    /// (`local`: feet off the view's middle) turned by `k` onto the most of `held` of the same
    /// kind within `tol`, and how many it matches. Ties keep the smaller offset.
    fn lay(&self, local: &[(usize, [f64; 2])], k: u8, centre: [f64; 2], slack: f64, held: &[Held], tol: f64) -> ([f64; 2], usize) {
        let base: Vec<[f64; 2]> = local
            .iter()
            .map(|(_, l)| {
                let t = turn(*l, k);
                [centre[0] + t[0], centre[1] + t[1]]
            })
            .collect();
        // Which held elements each symbol may be.
        let same: Vec<Vec<usize>> = local
            .iter()
            .map(|(i, _)| {
                let Some(s) = self.syms.get(*i) else { return Vec::new() };
                held.iter().enumerate().filter(|(_, h)| same_kind(&s.kind, s.mark.as_deref(), h.kind, h.mark)).map(|(j, _)| j).take(200).collect()
            })
            .collect();
        let mut cands: Vec<[f64; 2]> = vec![[0.0, 0.0]];
        for (b, js) in base.iter().zip(&same) {
            for &j in js {
                if let Some(h) = held.get(j) {
                    let d = [h.at[0] - b[0], h.at[1] - b[1]];
                    if d[0].hypot(d[1]) <= slack && d.iter().all(|v| v.is_finite()) {
                        cands.push(d);
                    }
                }
            }
        }
        cands.sort_by(|a, b| a[0].hypot(a[1]).total_cmp(&b[0].hypot(b[1])));
        cands.truncate(MAX_CANDIDATES);
        // One to one: each element stands for one symbol, nearest pairs first.
        let score = |d: [f64; 2]| -> (usize, [f64; 2]) {
            let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
            for (i, (b, js)) in base.iter().zip(&same).enumerate() {
                let p = [b[0] + d[0], b[1] + d[1]];
                for &j in js {
                    if let Some(h) = held.get(j) {
                        let dd = dist(h.at, p);
                        if dd <= tol {
                            pairs.push((dd, i, j));
                        }
                    }
                }
            }
            pairs.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
            let (mut si, mut sj) = (BTreeSet::new(), BTreeSet::new());
            let (mut n, mut sx, mut sy) = (0usize, 0.0, 0.0);
            for (_, i, j) in pairs {
                if si.contains(&i) || sj.contains(&j) {
                    continue;
                }
                si.insert(i);
                sj.insert(j);
                if let (Some(b), Some(h)) = (base.get(i), held.get(j)) {
                    n += 1;
                    sx += h.at[0] - (b[0] + d[0]);
                    sy += h.at[1] - (b[1] + d[1]);
                }
            }
            let mean = if n > 0 { [sx / n as f64, sy / n as f64] } else { [0.0, 0.0] };
            (n, mean)
        };
        let mut best = ([0.0, 0.0], 0usize, [0.0, 0.0]);
        for d in cands {
            let (n, mean) = score(d);
            if n > best.1 {
                best = (d, n, mean);
            }
        }
        // One step to the matched pairs' mean, when it holds as many.
        let refined = [best.0[0] + best.2[0], best.0[1] + best.2[1]];
        if best.1 > 0 && refined[0].hypot(refined[1]) <= slack && score(refined).0 >= best.1 {
            return (refined, score(refined).0);
        }
        (best.0, best.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_mirror_then_rotate_counter_clockwise() {
        assert_eq!(turn([1.0, 2.0], 0), [1.0, 2.0]);
        assert_eq!(turn([1.0, 2.0], 1), [-2.0, 1.0]);
        assert_eq!(turn([1.0, 2.0], 2), [-1.0, -2.0]);
        assert_eq!(turn([1.0, 2.0], 3), [2.0, -1.0]);
        assert_eq!(turn([1.0, 2.0], 4), [-1.0, 2.0]);
        assert_eq!(turn([1.0, 2.0], 6), [1.0, -2.0]);
        assert_eq!(fit_turn(&json!({"rotation_deg": 180.0, "mirrored": false})), Some(2));
        assert_eq!(fit_turn(&json!({"rotation_deg": 270.0, "mirrored": true})), Some(7));
        assert_eq!(fit_turn(&json!({"rotation_deg": -90.0})), Some(3));
        assert_eq!(fit_turn(&json!({"rotation_deg": f64::NAN})), None);
        assert_eq!(fit_turn(&json!({})), None);
    }

    #[test]
    fn kinds_match_by_tag_words_or_family() {
        assert!(same_kind("receptacle", Some("A"), "device", Some("a")));
        assert!(same_kind("duplex_receptacle", None, "receptacle", None));
        assert!(same_kind("receptacle", None, "duplex receptacle", None));
        assert!(!same_kind("receptacle", None, "switch", None));
        assert!(!same_kind("device", Some("A"), "device", Some("B")));
        assert!(!same_kind("device", None, "device", None));
        assert!(!same_kind("", None, "", None));
    }
}
