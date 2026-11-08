//! Several buildings from one set of plans (Q6292): a set drawn once for buildings built alike
//! says so in its titles (`WATER PIPING PLAN - EAST & WEST BLDGS`), and a trade plan drawn for one of
//! them names it (`POWER PLAN E. BLDG`). The model is built once from the plans, as for any
//! set; then each building gets its own copy:
//!
//! - an element built from a sheet that names some of the buildings is those buildings' only (a
//!   one-building electrical plan's devices go to that building's copy);
//! - an element built from a sheet that names none (the floor plan the set draws once) stands for
//!   every building the set names, so it is copied into each;
//! - an element with no sheet of its own (a device's box, an atom) goes where the element it
//!   serves goes.
//!
//! Every element says which building it is in (`building`, read from the titles that name it or
//! assumed from the rule above) and, on the first building's, which buildings it stands for
//! (`building_stands_for`); a copy says what it is a copy of (`building_copy_of`). The first
//! building stays where the plans draw it. The others stand in the directions the site plan's
//! labels give, one footprint and a gap apart; with no site plan to say, side by side (`place`
//! `inferred`). The place is never passed off as drawn: each building's `place` and `basis` say how
//! it was found.
//!
//! - `model.buildings {}`: place the spec's buildings in the model again (after edits, or a build
//!   that made one building): the copies are made anew, elements already placed stay placed.
//!   `model.build` does it at its end when the spec has several buildings; a guided job reads the
//!   buildings from the titles and does it at the end of its build.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, Buildings, SiteBuilding};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD: &str = "model.buildings";
/// The property naming the building an element is in.
pub const BUILDING: &str = "building";
/// On a first building's element: every building it stands for (comma separated).
pub const STANDS_FOR: &str = "building_stands_for";
/// On a copy: the handle of the element it copies.
pub const COPY_OF: &str = "building_copy_of";
/// The properties that name other elements by handle: a copy names the copies in its building.
const REFERENCE_KEYS: [&str; 2] = ["box_of", "connects"];
/// Most model objects looked at, copies made, texts read (hostile or huge drawings).
const MAX_ELEMENTS: usize = 2_000_000;
const MAX_COPIES: usize = 2_000_000;
const MAX_TEXTS: usize = 400_000;
/// Most buildings placed.
pub const MAX_BUILDINGS: usize = 16;
/// Longest text read as a title line, characters: notes are longer.
const MAX_TITLE_CHARS: usize = 90;
/// The clear space left between buildings the documents give no distance for, feet: room to
/// walk between them, never a dimension of the job (the place is marked inferred).
const GAP_FT: f64 = 20.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new(CMD, "Place Buildings", run).long().params(
        "{} → {buildings: [{name, offset_ft: [x, y], place: drawn | site plan | inferred, basis, elements}], sheets: {sheet: [building]}, copied, moved, removed}: the spec's buildings (`buildings`) placed in the model: each element in the buildings its sheet names, an element of a sheet naming none copied into every building; copies made anew each time",
    )]
}

fn run(s: &mut Session, _p: &Value) -> Result<Value> {
    let (mut sp, stored) = super::bspec::current(s, CMD)?;
    if !stored {
        return Err(bad(CMD, "the drawing has no building spec: build the model first (model.build or a guided job)"));
    }
    let done = apply(s.doc_mut()?, &mut sp);
    s.doc_mut()?.building_spec = Some(sp.to_json()).into();
    Ok(done.to_json(&sp))
}

/// What placing the buildings did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Applied {
    /// Copies made into the buildings after the first.
    pub copied: usize,
    /// Elements moved from where the plans draw them into their own building.
    pub moved: usize,
    /// Copies of an earlier placing taken out first.
    pub removed: usize,
    /// Elements per building, copies included.
    pub by_building: BTreeMap<String, usize>,
    /// Elements that stand for every building (no sheet naming fewer).
    pub shared: usize,
}

impl Applied {
    pub fn to_json(&self, sp: &BuildingSpec) -> Value {
        let list: Vec<Value> = sp
            .buildings
            .list
            .iter()
            .map(|b| {
                json!({
                    "name": b.name,
                    "offset_ft": b.offset_ft.map(super::printmep::r1),
                    "place": b.place,
                    "basis": b.basis,
                    "elements": self.by_building.get(&b.name).copied().unwrap_or(0),
                })
            })
            .collect();
        json!({
            "buildings": list,
            "sheets": sp.buildings.sheets,
            "copied": self.copied,
            "moved": self.moved,
            "removed": self.removed,
            "shared": self.shared,
        })
    }
}

// ─── Reading the set ─────────────────────────────────────────────────────────

/// One sheet of the set as the buildings' reading needs it.
#[derive(Debug, Clone, Default)]
pub struct SheetTitles {
    pub number: String,
    /// A plan of a building's floor (not a site or civil sheet, not details or schedules).
    pub plan: bool,
    /// A site or civil sheet: its labels place the buildings.
    pub site: bool,
    /// The sheet's title and the title lines read on it.
    pub titles: Vec<String>,
}

/// The buildings a set names, and the sheets that name some of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetBuildings {
    /// In the set's order (the sheets', then each title's).
    pub names: Vec<String>,
    /// The plans naming buildings, and which.
    pub sheets: BTreeMap<String, Vec<String>>,
}

impl SetBuildings {
    pub fn several(&self) -> bool {
        self.names.len() > 1
    }
}

/// The buildings the set's plans name. Only plans count: a site plan's `EXISTING BUILDING` or a
/// detail's `BUILDING A` names no building that is built from the plans.
pub fn read_set(sheets: &[SheetTitles]) -> SetBuildings {
    let mut out = SetBuildings::default();
    for sh in sheets.iter().filter(|s| s.plan && !s.site) {
        let mut named: Vec<String> = Vec::new();
        for t in &sh.titles {
            for n in cadaddy_mepfix::buildings::named_in(t) {
                if !named.contains(&n) {
                    named.push(n);
                }
            }
        }
        if named.is_empty() || sh.number.trim().is_empty() {
            continue;
        }
        for n in &named {
            if !out.names.contains(n) && out.names.len() < MAX_BUILDINGS {
                out.names.push(n.clone());
            }
        }
        named.retain(|n| out.names.contains(n));
        let e = out.sheets.entry(sh.number.clone()).or_default();
        for n in named {
            if !e.contains(&n) {
                e.push(n);
            }
        }
    }
    out
}

/// A text of the drawing: its words, where it starts, its height.
fn text_of(e: &Entity) -> Option<(String, Vec2, f64)> {
    match &e.kind {
        EntityKind::Text(t) => Some((t.value.trim().to_string(), t.insert.xy(), t.height)),
        EntityKind::MText(t) => {
            Some((cadaddy_fonts::plain_text(&t.contents).split_whitespace().collect::<Vec<_>>().join(" "), t.insert.xy(), t.height))
        }
        _ => None,
    }
}

fn inside(r: &[f64; 4], p: Vec2) -> bool {
    p.x >= r[0].min(r[2]) && p.x <= r[0].max(r[2]) && p.y >= r[1].min(r[3]) && p.y <= r[1].max(r[3])
}

/// The short lines of text in each rectangle (texts on one baseline, close together, joined:
/// a PDF often prints a title word by word), by rectangle.
pub fn text_lines(d: &Drawing, rects: &[[f64; 4]]) -> Vec<Vec<(String, Vec2)>> {
    let mut by: Vec<Vec<(String, Vec2, f64)>> = vec![Vec::new(); rects.len()];
    for e in d.model.iter().take(MAX_ELEMENTS) {
        let Some((t, at, h)) = text_of(e) else { continue };
        if t.is_empty() || !at.x.is_finite() || !at.y.is_finite() {
            continue;
        }
        for (i, r) in rects.iter().enumerate() {
            if inside(r, at)
                && let Some(v) = by.get_mut(i)
                && v.len() < MAX_TEXTS
            {
                v.push((t.clone(), at, if h.is_finite() && h > 0.0 { h } else { 1.0 }));
            }
        }
    }
    by.into_iter()
        .map(|mut v| {
            v.sort_by(|a, b| b.1.y.total_cmp(&a.1.y).then(a.1.x.total_cmp(&b.1.x)));
            let mut lines: Vec<(String, Vec2, f64, f64)> = Vec::new(); // text, start, height, end x
            for (t, at, h) in v {
                let joined = lines
                    .iter_mut()
                    .rev()
                    .take(8)
                    .find(|l| (l.1.y - at.y).abs() <= 0.5 * h.max(l.2) && at.x >= l.3 - h && at.x - l.3 <= 3.0 * h.max(l.2));
                match joined {
                    Some(l) if l.0.chars().count() + t.chars().count() < 4 * MAX_TITLE_CHARS => {
                        l.0.push(' ');
                        l.0.push_str(&t);
                        l.3 = at.x + 0.6 * h * t.chars().count() as f64;
                    }
                    _ => {
                        let end = at.x + 0.6 * h * t.chars().count() as f64;
                        lines.push((t, at, h, end));
                    }
                }
            }
            lines.into_iter().filter(|l| l.0.chars().count() <= MAX_TITLE_CHARS).map(|l| (l.0, l.1)).collect()
        })
        .collect()
}

/// Whether a line reads as a view's or sheet's title: a view title's word in it (`PLAN`,
/// `FLOOR`, the trade), not a note.
pub fn title_like(line: &str) -> bool {
    let words = cadaddy_mepfix::wordrules::words("sheets.view_title_words");
    line.split(|c: char| !c.is_alphanumeric()).any(|w| words.iter().any(|v| v.eq_ignore_ascii_case(w)))
}

/// Where the site sheets' labels put each named building (the middle of the labels naming it
/// alone), sheet units, and the sheet that shows it.
pub fn site_labels(lines: &[(String, Vec<(String, Vec2)>)], names: &[String]) -> BTreeMap<String, (Vec2, String)> {
    let mut sum: BTreeMap<String, (Vec2, usize, String)> = BTreeMap::new();
    for (sheet, ls) in lines {
        for (t, at) in ls {
            let n = cadaddy_mepfix::buildings::named_in(t);
            let [one] = n.as_slice() else { continue };
            if !names.contains(one) {
                continue;
            }
            let e = sum.entry(one.clone()).or_insert((Vec2::new(0.0, 0.0), 0, sheet.clone()));
            // One sheet places them all: labels on another sheet are its own picture.
            if e.2 == *sheet {
                e.0 += *at;
                e.1 += 1;
            }
        }
    }
    let first_sheet = sum.values().next().map(|v| v.2.clone());
    sum.into_iter().filter(|(_, v)| v.1 > 0 && Some(&v.2) == first_sheet.as_ref()).map(|(k, v)| (k, (v.0 * (1.0 / v.1 as f64), v.2))).collect()
}

/// Where each building stands. The first where the plans draw it; the others in the direction the
/// site plan's labels put them from the first (spaced in proportion, the nearest one footprint and
/// a gap away), else side by side to the east, a footprint and a gap apart.
/// `footprint_ft`: the first building's plan size `[x, y]`, feet.
pub fn place(set: &SetBuildings, footprint_ft: [f64; 2], site: &BTreeMap<String, (Vec2, String)>) -> Vec<SiteBuilding> {
    let names: Vec<&String> = set.names.iter().take(MAX_BUILDINGS).collect();
    let named_by = |n: &str| -> String {
        let s: Vec<&str> = set.sheets.iter().filter(|(_, v)| v.iter().any(|x| x == n)).map(|(k, _)| k.as_str()).collect();
        if s.is_empty() { "no sheet".into() } else { s.join(", ") }
    };
    let w = footprint_ft.map(|v| if v.is_finite() && v > 0.0 { v } else { 0.0 });
    let mut out: Vec<SiteBuilding> = Vec::new();
    let Some(first) = names.first() else { return out };
    out.push(SiteBuilding {
        name: (*first).clone(),
        offset_ft: [0.0, 0.0],
        place: "drawn".into(),
        basis: format!("named by {}; stands where the plans draw it", named_by(first)),
    });
    // The site plan's directions, when it labels the first and others.
    let origin = site.get(*first).map(|v| v.0);
    let dirs: Vec<(String, Vec2, f64, String)> = names
        .iter()
        .skip(1)
        .filter_map(|n| {
            let (o, (p, sheet)) = (origin?, site.get(*n)?);
            let d = *p - o;
            let len = d.len();
            (len.is_finite() && len > 1e-9).then(|| ((*n).clone(), d * (1.0 / len), len, sheet.clone()))
        })
        .collect();
    let nearest = dirs.iter().map(|d| d.2).fold(f64::INFINITY, f64::min);
    let mut east = 0.0_f64;
    for n in names.iter().skip(1) {
        if let Some((_, dir, len, sheet)) = dirs.iter().find(|d| d.0 == **n) {
            // Clear of the first's footprint in that direction, then a gap.
            let across = (dir.x.abs() * w[0] + dir.y.abs() * w[1]).max(1.0);
            let step = (across + GAP_FT) * (len / nearest).clamp(1.0, 100.0);
            let off = *dir * step;
            out.push(SiteBuilding {
                name: (*n).clone(),
                offset_ft: [off.x, off.y],
                place: "site plan".into(),
                basis: format!(
                    "named by {}; its direction from {} as the labels on {sheet} put it; the distance is not dimensioned: {:.0}' (its footprint and {GAP_FT:.0}' clear, inferred)",
                    named_by(n),
                    first,
                    step
                ),
            });
            east = east.max(off.x);
        } else {
            east += w[0].max(1.0) + GAP_FT;
            out.push(SiteBuilding {
                name: (*n).clone(),
                offset_ft: [east, 0.0],
                place: "inferred".into(),
                basis: format!(
                    "named by {}; no site plan places it: laid beside {} to the east, its footprint and {GAP_FT:.0}' clear (inferred)",
                    named_by(n),
                    out.last().map_or(String::new(), |b| b.name.clone())
                ),
            });
        }
    }
    out
}

// ─── Placing the model ───────────────────────────────────────────────────────

/// The sheet an element was built from: the sheet its properties name, else the sheet holding the
/// print object it was made from, else the first sheet a property was read from.
fn element_sheet(d: &Drawing, e: &Entity, ps: &Props, sheets: &[(String, [f64; 4])]) -> Option<String> {
    let known = |n: &str| sheets.iter().any(|(s, _)| s.eq_ignore_ascii_case(n.trim()));
    if let Some(s) = ps.get("sheet").map(Prop::text).filter(|s| known(s)) {
        return Some(s.trim().to_string());
    }
    // Made from another element of the model (a device's box): that element says, not where it is.
    if let Some(src) = made_from(d, e).and_then(|h| d.entity(h)).filter(|x| super::elemprops::element_kind(x).is_none()) {
        let b = cadaddy_doc::entity_bounds(d, src, 0);
        let c = Vec2::new((b.min.x + b.max.x) * 0.5, (b.min.y + b.max.y) * 0.5);
        if c.x.is_finite() && c.y.is_finite() {
            // The smallest sheet holding it (a view inside a sheet's rectangle).
            let hit = sheets
                .iter()
                .filter(|(_, r)| inside(r, c))
                .min_by(|a, b| ((a.1[2] - a.1[0]).abs() * (a.1[3] - a.1[1]).abs()).total_cmp(&((b.1[2] - b.1[0]).abs() * (b.1[3] - b.1[1]).abs())));
            if let Some((n, _)) = hit {
                return Some(n.clone());
            }
        }
    }
    ps.iter().filter(|(_, p)| p.source.kind == SourceKind::Print).find_map(|(_, p)| p.source.doc.clone().filter(|s| known(s)))
}

/// The object an element was made from (its build tag's source), when there is one.
fn made_from(d: &Drawing, e: &Entity) -> Option<Handle> {
    let h = super::bimlook::made_tags(e).and_then(|t| t.get(2)).and_then(|t| Handle::parse_hex(t.value.trim())).filter(|h| h.0 != 0)?;
    d.entity(h).is_some().then_some(h)
}

/// The handles an element's reference properties name.
fn references(ps: &Props) -> Vec<Handle> {
    REFERENCE_KEYS
        .iter()
        .filter_map(|k| ps.get(k))
        .flat_map(|p| p.text().split([',', ' ']).filter_map(|t| Handle::parse_hex(t.trim())).take(64).collect::<Vec<_>>())
        .collect()
}

/// An element the placing looks at.
struct Item {
    h: Handle,
    /// Already placed by an earlier placing: its building and what it stands for are its own.
    placed: Option<(String, Vec<String>)>,
    /// The buildings its sheet names (none: no sheet of its own, or one naming none).
    sheet: Option<String>,
    refs: Vec<Handle>,
}

/// Place the spec's buildings in the model: copies of an earlier placing out, then every element
/// in the buildings it belongs to (see the module doc). Each level's shell gets the other
/// buildings' boxes too, so their elements are inside a building.
pub fn apply(d: &mut Drawing, sp: &mut BuildingSpec) -> Applied {
    let mut done = Applied::default();
    // Copies of an earlier placing and the shell boxes it added: out.
    let old: Vec<Handle> = d
        .model
        .iter()
        .take(MAX_ELEMENTS)
        .filter(|e| props::has_store(&e.common.xdata) && props::read(&e.common.xdata).get(COPY_OF).is_some())
        .map(|e| e.handle)
        .collect();
    for h in old {
        if d.remove_entity(h).is_some() {
            done.removed += 1;
        }
    }
    for (level, boxes) in std::mem::take(&mut sp.buildings.added_shells) {
        if let Some(have) = sp.shells.get_mut(&level) {
            have.retain(|b| !boxes.contains(b));
        }
    }
    if !sp.buildings.several() {
        if done.removed > 0 {
            super::mepatoms::reindex(d);
        }
        return done;
    }
    let names: Vec<String> = sp.buildings.list.iter().take(MAX_BUILDINGS).map(|b| b.name.clone()).collect();
    let Some(first) = names.first().cloned() else { return done };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let offset: BTreeMap<String, Vec2> = sp
        .buildings
        .list
        .iter()
        .map(|b| (b.name.clone(), Vec2::new(b.offset_ft[0], b.offset_ft[1]) * upf))
        .filter(|(_, v)| v.x.is_finite() && v.y.is_finite())
        .collect();
    let at = |n: &str| offset.get(n).copied().unwrap_or(Vec2::new(0.0, 0.0));
    let sheets: Vec<(String, [f64; 4])> = sp.sheets.iter().filter_map(|l| Some((l.number.clone(), l.rect?))).collect();
    let named = |sheet: &str| -> Option<Vec<String>> {
        let v = sp.buildings.sheets.iter().find(|(k, _)| k.eq_ignore_ascii_case(sheet))?.1;
        let v: Vec<String> = names.iter().filter(|n| v.contains(n)).cloned().collect();
        (!v.is_empty()).then_some(v)
    };

    // Each element: where it was placed before, its sheet, what it serves.
    let mut items: Vec<Item> = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS) {
        if super::elemprops::element_kind(e).is_none() {
            continue;
        }
        let ps = props::read(&e.common.xdata);
        let placed = ps.get(BUILDING).map(Prop::text).filter(|b| names.contains(b)).map(|b| {
            let stands: Vec<String> = ps
                .get(STANDS_FOR)
                .map(|p| p.text().split(',').map(|x| x.trim().to_string()).filter(|x| names.contains(x)).collect())
                .unwrap_or_default();
            let stands = if stands.is_empty() { vec![b.clone()] } else { stands };
            (b, stands)
        });
        let sheet = if placed.is_some() { None } else { element_sheet(d, e, &ps, &sheets) };
        let mut refs = references(&ps);
        if let Some(src) = made_from(d, e).filter(|h| d.entity(*h).is_some_and(|x| super::elemprops::element_kind(x).is_some())) {
            refs.insert(0, src);
        }
        items.push(Item { h: e.handle, placed, sheet, refs });
    }
    // Which buildings each element is in: its own placing, its sheet's, the element it serves'.
    let mut of: HashMap<Handle, Vec<String>> = HashMap::new();
    for it in &items {
        if let Some((_, stands)) = &it.placed {
            of.insert(it.h, stands.clone());
        } else if let Some(v) = it.sheet.as_deref().and_then(named) {
            of.insert(it.h, v);
        }
    }
    for _ in 0..2 {
        for it in items.iter().filter(|i| i.placed.is_none() && i.sheet.as_deref().and_then(named).is_none()) {
            if of.contains_key(&it.h) {
                continue;
            }
            if let Some(v) = it.refs.iter().find_map(|r| of.get(r).filter(|v| v.len() < names.len()).cloned()) {
                of.insert(it.h, v);
            }
        }
    }

    let mut copies: Vec<(Entity, String)> = Vec::new();
    let mut copy_of: HashMap<(Handle, String), Handle> = HashMap::new();
    for it in &items {
        let stands = of.get(&it.h).cloned().unwrap_or_else(|| names.clone());
        if stands.len() == names.len() {
            done.shared += 1;
        }
        let own = if stands.contains(&first) { first.clone() } else { stands.first().cloned().unwrap_or_else(|| first.clone()) };
        if it.placed.is_none() {
            let why = match it.sheet.as_deref().filter(|s| named(s).is_some()) {
                Some(s) => Source::print(s, None),
                None => Source::from_doc(SourceKind::Rule, "a plan that names no building stands for every building the set names"),
            };
            let shift = at(&own) - at(&first);
            let moved = shift.len() > 1e-9;
            let stands_text = stands.join(",");
            let _ = d.modify_entity(it.h, |e| {
                if moved {
                    e.kind.transform3(&cadaddy_doc::translation(Vec3::new(shift.x, shift.y, 0.0)));
                }
                let mut ps = props::read(&e.common.xdata);
                ps.set(BUILDING, Prop::new(own.clone(), why));
                ps.set(STANDS_FOR, Prop::new(stands_text, Source::computed()));
                e.common.xdata = props::write(&e.common.xdata, &ps).into();
            });
            if moved {
                done.moved += 1;
            }
        }
        *done.by_building.entry(own.clone()).or_default() += 1;
        for b in stands.iter().filter(|b| **b != own) {
            if copies.len() >= MAX_COPIES {
                break;
            }
            let Some(src) = d.entity(it.h) else { continue };
            let mut c = (**src).clone();
            c.handle = d.new_handle();
            let shift = at(b) - at(&own);
            c.kind.transform3(&cadaddy_doc::translation(Vec3::new(shift.x, shift.y, 0.0)));
            let mut ps = props::read(&c.common.xdata);
            ps.set(
                BUILDING,
                Prop::new(b.clone(), Source::from_doc(SourceKind::Rule, "copied from the plan that stands for every building the set names")),
            );
            ps.remove(STANDS_FOR);
            ps.set(COPY_OF, Prop::new(it.h.hex(), Source::computed()));
            c.common.xdata = props::write(&c.common.xdata, &ps).into();
            copy_of.insert((it.h, b.clone()), c.handle);
            *done.by_building.entry(b.clone()).or_default() += 1;
            copies.push((c, b.clone()));
        }
    }
    // A copy names the copies in its own building (a box of the copied device).
    for (c, b) in &mut copies {
        let mut ps = props::read(&c.common.xdata);
        let mut changed = false;
        for k in REFERENCE_KEYS {
            let Some(p) = ps.get(k).cloned() else { continue };
            let text = p.text();
            let mapped: Vec<String> = text
                .split(',')
                .map(|t| Handle::parse_hex(t.trim()).and_then(|h| copy_of.get(&(h, b.clone()))).map_or_else(|| t.trim().to_string(), |h| h.hex()))
                .collect();
            let mapped = mapped.join(",");
            if mapped != text {
                ps.set(k, Prop { value: Value::String(mapped), ..p });
                changed = true;
            }
        }
        if changed {
            c.common.xdata = props::write(&c.common.xdata, &ps).into();
        }
    }
    done.copied = copies.len();
    for (c, _) in copies {
        d.model.push(c);
    }
    // The other buildings' shells, so what stands in them is inside a building.
    let mut added: BTreeMap<String, Vec<[f64; 4]>> = BTreeMap::new();
    for (level, boxes) in &mut sp.shells {
        let base: Vec<[f64; 4]> = boxes.clone();
        for n in names.iter().skip(1) {
            let o = at(n) - at(&first);
            for b in &base {
                let m = [b[0] + o.x, b[1] + o.y, b[2] + o.x, b[3] + o.y];
                boxes.push(m);
                added.entry(level.clone()).or_default().push(m);
            }
        }
    }
    sp.buildings.added_shells = added;
    // The model's piece index counts the copies too (the BOM and the takeoff read it).
    if done.copied + done.removed > 0 {
        super::mepatoms::reindex(d);
    }
    done
}

/// The spec as the buildings count it (Q6292), for the bill of materials: every MEP run,
/// terminal, piece of equipment and tagged element read off a plan that stands for several
/// buildings is there once per building (moved to that building's place; a copy's id ends in
/// `@<building>`); one read off a plan naming one building is there once. The model's elements are
/// already one per building, the spec's items are read once: this is what counts them alike.
/// `None`: one building.
pub fn per_building(sp: &BuildingSpec) -> Option<BuildingSpec> {
    if !sp.buildings.several() {
        return None;
    }
    let names: Vec<String> = sp.buildings.list.iter().take(MAX_BUILDINGS).map(|b| b.name.clone()).collect();
    let first = names.first()?.clone();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let off: BTreeMap<&str, [f64; 2]> =
        sp.buildings.list.iter().map(|b| (b.name.as_str(), b.offset_ft)).filter(|(_, o)| o.iter().all(|v| v.is_finite())).collect();
    let ft = |n: &str| off.get(n).copied().unwrap_or([0.0, 0.0]);
    // The other buildings an item of `sheet` is in besides its own, and the shift to each
    // (feet), its own being the first it stands for.
    let others = |sheet: &str| -> Vec<(String, [f64; 2])> {
        let stands: Vec<String> = match sp.buildings.sheets.iter().find(|(k, _)| k.eq_ignore_ascii_case(sheet.trim())) {
            Some((_, v)) => names.iter().filter(|n| v.contains(n)).cloned().collect(),
            None => names.clone(),
        };
        let own = if stands.contains(&first) { first.clone() } else { stands.first().cloned().unwrap_or_else(|| first.clone()) };
        let o = ft(&own);
        stands.into_iter().filter(|b| *b != own).map(|b| (b.clone(), [ft(&b)[0] - o[0], ft(&b)[1] - o[1]])).collect()
    };
    let mut out = sp.clone();
    let budget = |n: usize| n < MAX_COPIES;
    for r in &sp.mep.runs {
        for (b, d) in others(&r.source_sheet) {
            if !budget(out.mep.runs.len()) {
                break;
            }
            let mut c = r.clone();
            c.points = c.points.iter().map(|p| [p[0] + d[0] * upf, p[1] + d[1] * upf]).collect();
            c.id = format!("{}@{b}", c.id);
            out.mep.runs.push(c);
        }
    }
    for t in &sp.mep.terminals {
        for (_, d) in others(&t.source_sheet) {
            if !budget(out.mep.terminals.len()) {
                break;
            }
            let mut c = t.clone();
            c.at = [c.at[0] + d[0] * upf, c.at[1] + d[1] * upf];
            out.mep.terminals.push(c);
        }
    }
    for e in &sp.mep.equipment {
        let sheet = e
            .sources
            .get("sheet")
            .cloned()
            .or_else(|| e.sources.get("position").and_then(|p| p.split_whitespace().next()).map(str::to_string))
            .unwrap_or_default();
        for (_, d) in others(&sheet) {
            if !budget(out.mep.equipment.len()) {
                break;
            }
            let mut c = e.clone();
            c.min = [c.min[0] + d[0] * upf, c.min[1] + d[1] * upf];
            c.max = [c.max[0] + d[0] * upf, c.max[1] + d[1] * upf];
            out.mep.equipment.push(c);
        }
    }
    for t in &sp.tags {
        let sheet = t.fields.get("sheet").map(|f| f.value.clone()).unwrap_or_default();
        for (b, d) in others(&sheet) {
            if !budget(out.tags.len()) {
                break;
            }
            let mut c = t.clone();
            c.at_ft = [c.at_ft[0] + d[0], c.at_ft[1] + d[1]];
            c.id = format!("{}@{b}", c.id);
            out.tags.push(c);
        }
    }
    Some(out)
}

/// Place the stored spec's buildings (when it has several) and keep the spec: `model.build`'s
/// last step.
pub fn apply_stored(s: &mut Session) -> Result<Option<Applied>> {
    let Ok((mut sp, true)) = super::bspec::current(s, CMD) else { return Ok(None) };
    if sp.buildings.is_empty() {
        return Ok(None);
    }
    let done = apply(s.doc_mut()?, &mut sp);
    s.doc_mut()?.building_spec = Some(sp.to_json()).into();
    Ok(Some(done))
}

/// The first building's plan size, feet: its levels' shells, else its walls.
pub fn footprint_ft(d: &Drawing, sp: &BuildingSpec) -> [f64; 2] {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut b = cadaddy_geom::Bounds2::EMPTY;
    for boxes in sp.shells.values() {
        for r in boxes.iter().filter(|r| r.iter().all(|v| v.is_finite())) {
            b = b.union(&cadaddy_geom::Bounds2::from_points([Vec2::new(r[0], r[1]), Vec2::new(r[2], r[3])]));
        }
    }
    if b.is_empty() {
        for e in d.model.iter().take(MAX_ELEMENTS).filter(|e| super::wallgen::made_as(e).is_some_and(|p| p.eq_ignore_ascii_case("wall"))) {
            b = b.union(&cadaddy_doc::entity_bounds(d, e, 0));
        }
    }
    if b.is_empty() || !b.min.x.is_finite() || !b.max.x.is_finite() {
        return [0.0, 0.0];
    }
    [(b.max.x - b.min.x) / upf, (b.max.y - b.min.y) / upf]
}

/// The buildings a set names, for a spec: the list placed, the sheets naming them.
pub fn to_spec(set: &SetBuildings, list: Vec<SiteBuilding>) -> Buildings {
    Buildings { list, sheets: set.sheets.clone(), added_shells: BTreeMap::new() }
}

/// What the model holds per building (copies included), from the elements' `building`.
pub fn count(d: &Drawing) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    for e in d.model.iter().take(MAX_ELEMENTS) {
        if !props::has_store(&e.common.xdata) || super::elemprops::element_kind(e).is_none() {
            continue;
        }
        if let Some(b) = props::read(&e.common.xdata).get(BUILDING).map(Prop::text) {
            *out.entry(b).or_default() += 1;
        }
    }
    out
}

/// The names of the buildings an element list mentions (for reports), in order.
pub fn names_of(list: &[SiteBuilding]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    list.iter().filter(|b| seen.insert(b.name.clone())).map(|b| b.name.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, Line};
    use cadaddy_walk3d::Part;

    fn sheet(number: &str, plan: bool, site: bool, titles: &[&str]) -> SheetTitles {
        SheetTitles { number: number.into(), plan, site, titles: titles.iter().map(|t| t.to_string()).collect() }
    }

    #[test]
    fn the_set_names_its_buildings_on_its_plans_only() {
        let set = read_set(&[
            sheet("A-1", true, false, &["FLOOR PLAN"]),
            sheet("P-1", true, false, &["PLUMBING PLAN", "PLUMBING PLAN - EAST & WEST BLDGS"]),
            sheet("E-11", true, false, &["POWER PLAN W. BLDG"]),
            sheet("E-12", true, false, &["POWER PLAN E. BLDG"]),
            // A site plan's or a detail sheet's buildings are not the set's.
            sheet("C-1", true, true, &["SITE PLAN - BUILDING C"]),
            sheet("A-5", false, false, &["BUILDING D WALL SECTIONS"]),
        ]);
        assert_eq!(set.names, ["EAST", "WEST"]);
        assert!(set.several());
        assert_eq!(set.sheets.get("P-1"), Some(&vec!["EAST".to_string(), "WEST".to_string()]));
        assert_eq!(set.sheets.get("E-11"), Some(&vec!["WEST".to_string()]));
        assert!(!set.sheets.contains_key("A-1") && !set.sheets.contains_key("C-1") && !set.sheets.contains_key("A-5"));
        // One building named: one building.
        assert!(!read_set(&[sheet("E-11", true, false, &["POWER PLAN - BUILDING A"])]).several());
        assert!(!read_set(&[]).several());
    }

    fn two() -> SetBuildings {
        SetBuildings { names: vec!["A".into(), "B".into()], sheets: BTreeMap::from([("E-12".to_string(), vec!["B".to_string()])]) }
    }

    #[test]
    fn with_no_site_plan_the_others_stand_beside_the_first_marked_inferred() {
        let three = SetBuildings { names: vec!["A".into(), "B".into(), "C".into()], sheets: BTreeMap::new() };
        let p = place(&three, [40.0, 25.0], &BTreeMap::new());
        assert_eq!(p.iter().map(|b| b.offset_ft).collect::<Vec<_>>(), [[0.0, 0.0], [60.0, 0.0], [120.0, 0.0]]);
        assert_eq!(p.iter().map(|b| b.place.as_str()).collect::<Vec<_>>(), ["drawn", "inferred", "inferred"]);
        assert!(p[1].basis.contains("inferred") && p[1].basis.contains("no site plan"), "{}", p[1].basis);
        // No footprint known: still apart.
        let q = place(&two(), [f64::NAN, 0.0], &BTreeMap::new());
        assert!(q[1].offset_ft[0] > GAP_FT, "{q:?}");
    }

    #[test]
    fn the_site_plans_labels_give_the_direction() {
        // B is labelled due north of A on the site plan.
        let lines = vec![(
            "C-1".to_string(),
            vec![
                ("BUILDING A".to_string(), Vec2::new(100.0, 100.0)),
                ("BUILDING B".to_string(), Vec2::new(100.0, 300.0)),
                ("PARKING".into(), Vec2::new(0.0, 0.0)),
            ],
        )];
        let labels = site_labels(&lines, &two().names);
        assert_eq!(labels.len(), 2);
        let p = place(&two(), [40.0, 25.0], &labels);
        assert_eq!(p[1].place, "site plan");
        let o = p[1].offset_ft;
        assert!(o[0].abs() < 1e-9 && (o[1] - (25.0 + GAP_FT)).abs() < 1e-9, "{o:?}");
        assert!(p[1].basis.contains("C-1") && p[1].basis.contains("inferred"), "{}", p[1].basis);
        // A label naming a building not in the set, or two at once, places nothing.
        let other =
            vec![("C-1".to_string(), vec![("BUILDING Q".to_string(), Vec2::new(0.0, 0.0)), ("BUILDINGS A & B".to_string(), Vec2::new(5.0, 5.0))])];
        assert!(site_labels(&other, &two().names).is_empty());
    }

    /// A model element as the build makes one, made from `src`, with `props`.
    fn element(d: &mut Drawing, part: Part, src: Handle, at: [f64; 2], ps: &[(&str, &str)]) -> Handle {
        let h = d.new_handle();
        let mut x = super::super::wallgen::tag(part, "Level 1", src);
        let mut store = Props::default();
        for (k, v) in ps {
            store.set(k, Prop::new(*v, Source::computed()));
        }
        x = props::write(&x, &store);
        let kind = EntityKind::Line(Line { a: Vec3::new(at[0], at[1], 0.0), b: Vec3::new(at[0] + 10.0, at[1], 0.0) });
        d.model.push(Entity { handle: h, common: Common { xdata: x.into(), ..Common::default() }, kind });
        h
    }

    fn print_object(d: &mut Drawing, at: [f64; 2]) -> Handle {
        let h = d.new_handle();
        d.model.push(Entity {
            handle: h,
            common: Common::default(),
            kind: EntityKind::Line(Line { a: Vec3::new(at[0], at[1], 0.0), b: Vec3::new(at[0] + 1.0, at[1], 0.0) }),
        });
        h
    }

    fn link(n: &str, rect: [f64; 4]) -> cadaddy_walk3d::levels::SheetLink {
        cadaddy_walk3d::levels::SheetLink {
            number: n.into(),
            title: String::new(),
            discipline: None,
            level: Some("Level 1".into()),
            source: String::new(),
            rect: Some(rect),
            levels: Vec::new(),
            registration: None,
            view: None,
        }
    }

    fn building_of(d: &Drawing, h: Handle) -> String {
        d.entity(h).map(|e| props::read(&e.common.xdata).get(BUILDING).map(Prop::text).unwrap_or_default()).unwrap_or_default()
    }

    fn x_of(d: &Drawing, h: Handle) -> f64 {
        match d.entity(h).map(|e| &e.kind) {
            Some(EntityKind::Line(l)) => l.a.x,
            _ => f64::NAN,
        }
    }

    #[test]
    fn each_element_goes_to_the_buildings_its_sheet_names() {
        let mut d = Drawing::default();
        // The floor plan (no building named) at 0..1000, the trade plans beside it.
        let wall_src = print_object(&mut d, [100.0, 100.0]);
        let wall = element(&mut d, Part::Wall, wall_src, [100.0, 100.0], &[]);
        let dev_b = element(&mut d, Part::Device, Handle(0), [200.0, 200.0], &[("sheet", "E-12")]);
        let dev_shared = element(&mut d, Part::Device, Handle(0), [300.0, 300.0], &[("sheet", "E-13")]);
        // A device's box: made from the device, it goes where the device goes.
        let box_b = element(&mut d, Part::Device, dev_b, [205.0, 200.0], &[("box_of", &dev_b.hex())]);
        let box_shared = element(&mut d, Part::Device, Handle(0), [305.0, 300.0], &[("box_of", &dev_shared.hex())]);
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        sp.sheets =
            vec![link("A-1", [0.0, 0.0, 1000.0, 1000.0]), link("E-12", [2000.0, 0.0, 3000.0, 1000.0]), link("E-13", [4000.0, 0.0, 5000.0, 1000.0])];
        sp.shells.insert("Level 1".into(), vec![[0.0, 0.0, 480.0, 300.0]]);
        let set = two();
        sp.buildings = to_spec(&set, place(&set, footprint_ft(&d, &sp), &BTreeMap::new()));
        assert_eq!(sp.buildings.list[1].offset_ft, [60.0, 0.0], "the shell is 40' wide");
        let before = d.model.len();
        let done = apply(&mut d, &mut sp);
        // The wall, the shared device and its box stand for both: copied; the B device and its box
        // move to B.
        assert_eq!((done.copied, done.moved, done.shared), (3, 2, 3), "{done:?}");
        assert_eq!(d.model.len(), before + 3);
        assert_eq!((building_of(&d, wall), building_of(&d, dev_shared)), ("A".into(), "A".into()));
        assert_eq!((building_of(&d, dev_b), building_of(&d, box_b)), ("B".into(), "B".into()));
        assert!((x_of(&d, dev_b) - (200.0 + 720.0)).abs() < 1e-9, "moved 60' east");
        assert!((x_of(&d, wall) - 100.0).abs() < 1e-9, "the first stays where it is drawn");
        assert_eq!(count(&d), BTreeMap::from([("A".to_string(), 3), ("B".to_string(), 5)]));
        // The copy of the shared box names the copy of the shared device.
        let copies: Vec<Props> = d.model.iter().map(|e| props::read(&e.common.xdata)).filter(|p| p.get(COPY_OF).is_some()).collect();
        let box_copy =
            copies.iter().find(|p| p.get(COPY_OF).map(Prop::text) == Some(box_shared.hex())).unwrap_or_else(|| panic!("no copy of the box"));
        let dev_copy_handle = d
            .model
            .iter()
            .find(|e| props::read(&e.common.xdata).get(COPY_OF).map(Prop::text) == Some(dev_shared.hex()))
            .map(|e| e.handle.hex())
            .unwrap_or_default();
        assert_eq!(box_copy.get("box_of").map(Prop::text), Some(dev_copy_handle));
        // B's shell is there too, so its elements are in a building.
        assert_eq!(sp.shells["Level 1"], vec![[0.0, 0.0, 480.0, 300.0], [720.0, 0.0, 1200.0, 300.0]]);

        // Placed again: the same model, nothing moved twice.
        let again = apply(&mut d, &mut sp);
        assert_eq!((again.copied, again.moved, again.removed), (3, 0, 3), "{again:?}");
        assert_eq!(d.model.len(), before + 3);
        assert!((x_of(&d, dev_b) - 920.0).abs() < 1e-9);
        assert_eq!(sp.shells["Level 1"].len(), 2);
        // One building again: the copies and the added shell go.
        sp.buildings.list.truncate(1);
        let one = apply(&mut d, &mut sp);
        assert_eq!((one.removed, d.model.len()), (3, before));
        assert_eq!(sp.shells["Level 1"].len(), 1);
    }

    #[test]
    fn the_bill_of_materials_counts_a_shared_plans_items_in_every_building() {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        assert!(per_building(&sp).is_none(), "one building: the spec as it is");
        let run = |sheet: &str| -> cadaddy_walk3d::MepRun {
            serde_json::from_value(json!({"system": "V", "level": "Level 1", "points": [[0.0, 0.0], [120.0, 0.0]], "size_in": [2.0, 0.0], "bottoms_ft": [9.0, 9.0], "objects": [], "id": "R1", "source_sheet": sheet}))
                .unwrap_or_else(|e| panic!("{e}"))
        };
        sp.mep.runs = vec![run("P-1"), run("E-12")];
        let mut tag = cadaddy_walk3d::tags::ElementTag { id: "device:1".into(), kind: "receptacle".into(), ..Default::default() };
        tag.fields.insert("sheet".into(), cadaddy_walk3d::tags::Field { value: "E-12".into(), source: "plan".into() });
        sp.tags = vec![tag];
        sp.buildings = to_spec(&two(), place(&two(), [40.0, 25.0], &BTreeMap::new()));
        let pb = per_building(&sp).unwrap_or_else(|| panic!("two buildings"));
        // P-1 names none: once per building, the copy at B's place; E-12 names B: once.
        assert_eq!(pb.mep.runs.len(), 3);
        let copy = pb.mep.runs.iter().find(|r| r.id == "R1@B").unwrap_or_else(|| panic!("{:?}", pb.mep.runs));
        assert_eq!((copy.source_sheet.as_str(), copy.points[0]), ("P-1", [720.0, 0.0]));
        assert_eq!(pb.tags.len(), 1);
    }
}
