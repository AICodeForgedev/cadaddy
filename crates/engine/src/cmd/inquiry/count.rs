//! COUNT, COUNTLIST and COUNTTABLE: block references counted in model space, in the layouts and
//! nested inside other blocks (multiplied by the references holding them), or in a selection; and
//! the objects shaped like a picked one.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Insert, MAX_BLOCK_DEPTH, Space};
use cadaddy_geom::{Mat3, Vec2};
use serde_json::{Value, json};

use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Merges done while expanding nested references; a hostile file stops here (and says so).
const MAX_WORK: u64 = 5_000_000;
/// Top-level objects reported (and highlighted) per count.
const MAX_HANDLES: usize = 100_000;

/// COUNT's JSON parameters.
pub(super) const PARAMS: &str = "{block?, object?: handle (count objects like this one), space?: current|model|all|<layout> (default current), \
     handles?: [...] (count in these objects), area?: [[x,y],[x,y]] | polygon (references inserted inside it), nested?: bool (default true), \
     highlight?: bool, similar?: bool (list: also similar: {name: {scaled: [handles], exploded: [[handles]]}})} → \
     {blocks: {name: n}, nested: {name: n}, spaces: {space: {name: n}}, total} | {block, count, nested, handles}, plus `message`";

pub(super) fn machine() -> Box<dyn Interactive> {
    Box::new(CountM::default())
}

pub(super) fn table_machine() -> Box<dyn Interactive> {
    Box::new(CountTableM)
}

/// Where COUNT looks.
#[derive(Clone, Debug, PartialEq)]
enum Scope {
    Space(Space),
    All,
    /// These top-level objects (a selection).
    Objects(Vec<Handle>),
    /// The objects of a scope whose insertion point is inside a polygon (COUNT in an area).
    Area(Box<Scope>, Vec<Vec2>),
}

/// Where an object sits for COUNT in an area: a reference's insertion point, else its first grip.
fn anchor(e: &Entity) -> Option<Vec2> {
    match &e.kind {
        EntityKind::Insert(i) => Some(Vec2::new(i.insert.x, i.insert.y)),
        k => k.grips().first().copied(),
    }
    .filter(|p| p.is_finite())
}

/// `area`: two corners (a rectangle) or a polygon's points.
fn area_param(cmd: &str, v: &Value) -> Result<Vec<Vec2>> {
    let pts: Vec<Vec2> = v.as_array().map(|a| a.iter().take(10_000).filter_map(point_value).collect()).unwrap_or_default();
    match pts.as_slice() {
        [a, b] => Ok(vec![*a, Vec2::new(b.x, a.y), *b, Vec2::new(a.x, b.y)]),
        p if p.len() >= 3 => Ok(pts),
        _ => Err(bad(cmd, "`area` is two corners [[x,y],[x,y]] or a polygon of 3 or more points")),
    }
}

impl Scope {
    fn parse(s: &Session, cmd: &str, p: &Value, default_all: bool) -> Result<Scope> {
        let base = Self::parse_base(s, cmd, p, default_all)?;
        match p.get("area").filter(|v| !v.is_null()) {
            Some(v) => Ok(Scope::Area(Box::new(base), area_param(cmd, v)?)),
            None => Ok(base),
        }
    }
    fn parse_base(s: &Session, cmd: &str, p: &Value, default_all: bool) -> Result<Scope> {
        if p.get("handles").is_some_and(|v| !v.is_null()) || p.get("handle").is_some_and(|v| !v.is_null()) {
            return Ok(Scope::Objects(targets(s, p)?));
        }
        let given = match p.get("space") {
            None | Some(Value::Null) => None,
            Some(Value::String(t)) => Some(t.trim().to_string()),
            Some(v) => return Err(bad(cmd, format!("`space` must be current, model, all or a layout name, got {v}"))),
        };
        match given.as_deref().map(str::to_ascii_lowercase).as_deref() {
            None if default_all => Ok(Scope::All),
            None | Some("current") => Ok(Scope::Space(s.space())),
            Some("all") => Ok(Scope::All),
            Some("model") => Ok(Scope::Space(Space::Model)),
            Some(_) => {
                let name = given.unwrap_or_default();
                let d = s.doc()?;
                let l =
                    d.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(&name)).ok_or_else(|| bad(cmd, format!("no layout named \"{name}\"")))?;
                Ok(Scope::Space(Space::Paper(l.name.clone())))
            }
        }
    }
    /// "model space", "layout Layout1", "all spaces", "the selection".
    fn label(&self) -> String {
        match self {
            Scope::Space(Space::Model) => "model space".into(),
            Scope::Space(Space::Paper(n)) => format!("layout {n}"),
            Scope::All => "all spaces".into(),
            Scope::Objects(_) => "the selection".into(),
            Scope::Area(inner, _) => format!("the area in {}", inner.label()),
        }
    }
    /// The link a count table keeps to work its counts out again; `None` for a selection.
    fn link(&self, nested: bool) -> Option<cadaddy_doc::CountLink> {
        let (space, area) = match self {
            Scope::Space(Space::Model) => ("Model".to_string(), Vec::new()),
            Scope::Space(Space::Paper(n)) => (n.clone(), Vec::new()),
            Scope::All => ("*".to_string(), Vec::new()),
            Scope::Objects(_) => return None,
            Scope::Area(inner, poly) => (inner.link(nested)?.space, poly.iter().map(|p| [p.x, p.y]).collect()),
        };
        Some(cadaddy_doc::CountLink { space, nested, area })
    }
    fn from_link(l: &cadaddy_doc::CountLink) -> Scope {
        let base = match l.space.as_str() {
            "*" => Scope::All,
            n if n.eq_ignore_ascii_case("Model") => Scope::Space(Space::Model),
            n => Scope::Space(Space::Paper(n.to_string())),
        };
        if l.area.len() >= 3 { Scope::Area(Box::new(base), l.area.iter().map(|p| Vec2::new(p[0], p[1])).collect()) } else { base }
    }
    /// The top-level objects in scope, each with the name of its space.
    fn objects<'a>(&self, d: &'a Drawing) -> Vec<(String, &'a std::sync::Arc<Entity>)> {
        let name = |sp: &Space| sp.tab_name().to_string();
        let mut out = Vec::new();
        let mut add = |sp: &Space| {
            if let Some(store) = d.space(sp) {
                out.extend(store.iter().map(|e| (name(sp), e)));
            }
        };
        match self {
            Scope::Space(sp) => add(sp),
            Scope::All => {
                add(&Space::Model);
                for l in &d.layouts {
                    add(&Space::Paper(l.name.clone()));
                }
            }
            Scope::Objects(hs) => {
                return hs.iter().filter_map(|h| Some((d.space_of(*h).map_or_else(|| "Model".into(), |sp| name(&sp)), d.entity(*h)?))).collect();
            }
            Scope::Area(inner, poly) => {
                return inner.objects(d).into_iter().filter(|(_, e)| anchor(e).is_some_and(|p| cadaddy_geom::point_in_polygon(poly, p))).collect();
            }
        }
        out
    }
}

/// Copies one reference places (a MINSERT's rows × columns).
fn copies(i: &Insert) -> u64 {
    u64::from(i.cols.max(1)).saturating_mul(u64::from(i.rows.max(1)))
}

/// Nested block references per block definition, memoised by (block, levels left) so shared and
/// cyclic definitions don't expand exponentially.
struct Expander<'a> {
    d: &'a Drawing,
    memo: HashMap<(String, usize), Rc<BTreeMap<String, u64>>>,
    work: u64,
    truncated: bool,
}

impl<'a> Expander<'a> {
    fn new(d: &'a Drawing) -> Self {
        Expander { d, memo: HashMap::new(), work: 0, truncated: false }
    }
    /// The definition's own name for a referenced block (names are case-insensitive).
    fn canonical(&self, name: &str) -> String {
        self.d.block(name).map_or_else(|| name.to_string(), |b| b.name.clone())
    }
    /// References inside block `name` and its nested blocks, `levels` deep: name → copies.
    fn expand(&mut self, name: &str, levels: usize) -> Rc<BTreeMap<String, u64>> {
        let key = (name.to_ascii_lowercase(), levels);
        if let Some(m) = self.memo.get(&key) {
            return m.clone();
        }
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        if levels > 0
            && let Some(b) = self.d.block(name)
        {
            let b = b.clone();
            for e in b.entities.iter() {
                let EntityKind::Insert(i) = &e.kind else { continue };
                if self.work >= MAX_WORK {
                    self.truncated = true;
                    break;
                }
                let m = copies(i);
                let inner = self.canonical(&i.block);
                let sub = self.expand(&inner, levels - 1);
                self.work = self.work.saturating_add(1).saturating_add(sub.len() as u64);
                add(&mut out, &inner, m);
                for (k, v) in sub.iter() {
                    add(&mut out, k, m.saturating_mul(*v));
                }
            }
        }
        let out = Rc::new(out);
        self.memo.insert(key, out.clone());
        out
    }
}

fn add(m: &mut BTreeMap<String, u64>, k: &str, n: u64) {
    let v = m.entry(k.to_string()).or_default();
    *v = v.saturating_add(n);
}

/// Anonymous blocks (`*U3`: arrays, dynamic block copies) are traversed but not reported.
fn reported(name: &str) -> bool {
    !name.starts_with('*')
}

#[derive(Default)]
struct Tally {
    /// Every reference: top-level and nested.
    blocks: BTreeMap<String, u64>,
    /// The nested part of `blocks`.
    nested: BTreeMap<String, u64>,
    /// Per space (Model or a layout): name → every reference there.
    spaces: BTreeMap<String, BTreeMap<String, u64>>,
    /// Top-level objects holding `want` (when one block is asked for).
    handles: Vec<Handle>,
    truncated: bool,
}

/// Count block references in `scope`; with `want`, also collect the top-level objects holding it.
fn tally(d: &Drawing, scope: &Scope, nested: bool, want: Option<&str>) -> Tally {
    let mut x = Expander::new(d);
    let mut t = Tally::default();
    for (space, e) in scope.objects(d) {
        let EntityKind::Insert(i) = &e.kind else { continue };
        let name = x.canonical(&i.block);
        let m = copies(i);
        let mut holds = want.is_some_and(|w| w.eq_ignore_ascii_case(&name));
        let per = t.spaces.entry(space).or_default();
        if reported(&name) {
            add(&mut t.blocks, &name, m);
            add(per, &name, m);
        }
        if nested {
            let sub = x.expand(&name, MAX_BLOCK_DEPTH.saturating_sub(1));
            for (k, v) in sub.iter().filter(|(k, _)| reported(k)) {
                let n = m.saturating_mul(*v);
                add(&mut t.blocks, k, n);
                add(&mut t.nested, k, n);
                add(per, k, n);
                holds |= want.is_some_and(|w| w.eq_ignore_ascii_case(k));
            }
        }
        if holds && t.handles.len() < MAX_HANDLES {
            t.handles.push(e.handle);
        }
    }
    t.spaces.retain(|_, m| !m.is_empty());
    t.truncated = x.truncated;
    t
}

/// A translation-free signature of an object's geometry: objects with equal keys are copies of
/// each other (moved, not rotated or scaled). Coordinates are rounded to 1e-6.
fn shape_key(k: &EntityKind) -> Option<String> {
    let origin = k.grips().first().copied().filter(|p| p.is_finite())?;
    let mut k = k.clone();
    k.transform(&Mat3::translate(Vec2::ZERO - origin));
    let mut v = serde_json::to_value(&k).ok()?;
    round(&mut v, 0);
    Some(v.to_string())
}

fn round(v: &mut Value, depth: usize) {
    if depth > 64 {
        return;
    }
    match v {
        Value::Number(n) if n.is_f64() => {
            if let Some(f) = n.as_f64() {
                let r = (f * 1e6).round() / 1e6;
                *v = json!(if r == 0.0 { 0.0 } else { r });
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| round(x, depth + 1)),
        Value::Object(o) => o.values_mut().for_each(|x| round(x, depth + 1)),
        _ => {}
    }
}

/// Loose (non-reference) objects in scope by (type, shape): their anchor and handle.
type ShapeIndex = HashMap<(&'static str, String), Vec<(Vec2, Handle)>>;

fn shape_index(d: &Drawing, scope: &Scope) -> ShapeIndex {
    let mut idx: ShapeIndex = HashMap::new();
    for (_, e) in scope.objects(d).into_iter().take(200_000) {
        if matches!(e.kind, EntityKind::Insert(_)) {
            continue;
        }
        let (Some(key), Some(at)) = (shape_key(&e.kind), e.kind.grips().first().copied().filter(|p| p.is_finite())) else { continue };
        idx.entry((e.kind.type_name(), key)).or_default().push((at, e.handle));
    }
    idx
}

/// Groups of loose objects that repeat block `name`'s definition moved (an exploded copy, at
/// its own scale and unrotated).
fn exploded(d: &Drawing, idx: &ShapeIndex, name: &str, work: &mut u64) -> Vec<Vec<Handle>> {
    let Some(b) = d.block(name) else { return Vec::new() };
    let parts: Vec<(&'static str, String, Vec2)> = b
        .entities
        .iter()
        .filter(|e| !matches!(e.kind, EntityKind::AttDef(_)))
        .take(501)
        .filter_map(|e| Some((e.kind.type_name(), shape_key(&e.kind)?, e.kind.grips().first().copied().filter(|p| p.is_finite())?)))
        .collect();
    if parts.len() < 2 || parts.len() > 500 {
        return Vec::new();
    }
    let Some((t0, k0, a0)) = parts.first().cloned() else { return Vec::new() };
    let mut used: std::collections::HashSet<Handle> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (at, h) in idx.get(&(t0, k0)).into_iter().flatten() {
        if used.contains(h) {
            continue;
        }
        let off = *at - a0;
        let mut group = vec![*h];
        let mut ok = true;
        for (t, k, a) in parts.iter().skip(1) {
            *work = work.saturating_add(1);
            if *work > MAX_WORK {
                return out;
            }
            let want = *a + off;
            let hit = idx
                .get(&(*t, k.clone()))
                .and_then(|v| v.iter().find(|(p, hh)| (*p - want).len() < 1e-6 && !used.contains(hh) && !group.contains(hh)).map(|(_, hh)| *hh));
            match hit {
                Some(hh) => group.push(hh),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            used.extend(group.iter().copied());
            out.push(group);
        }
    }
    out
}

/// Similar but not identical: per block, its references at another scale and its exploded
/// copies.
fn similar(d: &Drawing, scope: &Scope) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    let idx = shape_index(d, scope);
    let mut work = 0u64;
    let mut scaled: BTreeMap<String, Vec<Handle>> = BTreeMap::new();
    for (_, e) in scope.objects(d) {
        if let EntityKind::Insert(i) = &e.kind
            && ((i.scale.x - 1.0).abs() > 1e-9 || (i.scale.y - 1.0).abs() > 1e-9)
        {
            let name = d.block(&i.block).map_or_else(|| i.block.clone(), |b| b.name.clone());
            scaled.entry(name).or_default().push(e.handle);
        }
    }
    let mut names: Vec<String> =
        d.blocks.values().filter(|b| reported(&b.name) && b.xref_path.is_none() && !b.name.contains('|')).map(|b| b.name.clone()).collect();
    names.sort();
    for n in names {
        let ex = exploded(d, &idx, &n, &mut work);
        let sc = scaled.remove(&n).unwrap_or_default();
        if ex.is_empty() && sc.is_empty() {
            continue;
        }
        out.insert(n, json!({ "scaled": hex(&sc), "exploded": ex.iter().map(|g| hex(g)).collect::<Vec<_>>() }));
    }
    out
}

fn hex(hs: &[Handle]) -> Vec<String> {
    hs.iter().map(|h| h.hex()).collect()
}

fn count_line(name: &str, n: u64, nested: u64) -> String {
    if nested > 0 { format!("  {name}: {n} ({nested} nested)") } else { format!("  {name}: {n}") }
}

/// The per-block list as JSON plus the command-line text.
fn list(t: &Tally, scope: &Scope) -> Value {
    let wh = scope.label();
    let message = if t.blocks.is_empty() {
        format!("No block references in {wh}.")
    } else {
        let mut m = format!("Block references in {wh}:");
        for (name, n) in &t.blocks {
            m.push('\n');
            m.push_str(&count_line(name, *n, t.nested.get(name).copied().unwrap_or(0)));
        }
        m.push_str(&format!("\n  Total: {}", t.blocks.values().fold(0u64, |a, b| a.saturating_add(*b))));
        if t.truncated {
            m.push_str("\n  (nesting too large: counts are partial)");
        }
        m
    };
    let total = t.blocks.values().fold(0u64, |a, b| a.saturating_add(*b));
    json!({ "blocks": t.blocks, "nested": t.nested, "spaces": t.spaces, "total": total, "truncated": t.truncated, "message": message })
}

/// Every block's references in every space (nested ones included), and the nested part.
pub(super) fn all_counts(d: &Drawing) -> (BTreeMap<String, u64>, BTreeMap<String, u64>) {
    let t = tally(d, &Scope::All, true, None);
    (t.blocks, t.nested)
}

/// Count block `b` (its references, nested ones included) in `scope`.
fn count_block(d: &Drawing, scope: &Scope, nested: bool, b: &str) -> Value {
    let t = tally(d, scope, nested, Some(b));
    let name = d.block(b).map_or_else(|| b.to_string(), |blk| blk.name.clone());
    let n = t.blocks.get(&name).copied().unwrap_or(0);
    let k = t.nested.get(&name).copied().unwrap_or(0);
    let wh = scope.label();
    let message = if k > 0 { format!("Block {name}: {n} in {wh} ({k} nested).") } else { format!("Block {name}: {n} in {wh}.") };
    json!({ "block": name, "count": n, "nested": k, "handles": hex(&t.handles), "truncated": t.truncated, "message": message })
}

/// Count the objects in `scope` shaped like `h` (a block reference: references of its block).
fn count_like(d: &Drawing, scope: &Scope, nested: bool, h: Handle) -> Result<Value> {
    let e = d.entity(h).ok_or_else(|| bad("count", format!("no object with handle {}", h.hex())))?;
    if let EntityKind::Insert(i) = &e.kind {
        return Ok(count_block(d, scope, nested, &i.block));
    }
    let ty = e.kind.type_name();
    let key = shape_key(&e.kind).ok_or_else(|| bad("count", format!("a {ty} can't be counted by shape")))?;
    let hs: Vec<Handle> = scope
        .objects(d)
        .into_iter()
        .filter(|(_, o)| o.kind.type_name() == ty && shape_key(&o.kind).as_deref() == Some(key.as_str()))
        .map(|(_, o)| o.handle)
        .take(MAX_HANDLES)
        .collect();
    let n = hs.len();
    let message = format!("{ty}: {n} like the selected one in {}.", scope.label());
    Ok(json!({ "object": ty, "count": n, "handles": hex(&hs), "message": message }))
}

fn run(s: &mut Session, cmd: &str, p: &Value, default_all: bool) -> Result<Value> {
    let scope = Scope::parse(s, cmd, p, default_all)?;
    let nested = bool_or(p, "nested", true);
    let d = s.doc()?;
    let r = if let Some(b) = str_param(p, "block") {
        count_block(d, &scope, nested, b)
    } else if let Some(v) = p.get("object").filter(|v| !v.is_null()) {
        let h = v
            .as_str()
            .and_then(Handle::parse_hex)
            .or_else(|| v.as_u64().map(Handle))
            .ok_or_else(|| bad(cmd, format!("`object`: {v} is not an object handle")))?;
        count_like(d, &scope, nested, h)?
    } else {
        let mut r = list(&tally(d, &scope, nested, None), &scope);
        if bool_or(p, "similar", false)
            && let Some(o) = r.as_object_mut()
        {
            o.insert("similar".into(), Value::Object(similar(d, &scope)));
        }
        r
    };
    if bool_or(p, "highlight", false)
        && let Some(hs) = r.get("handles").and_then(Value::as_array)
    {
        let hs = hs.iter().filter_map(|v| v.as_str().and_then(Handle::parse_hex)).collect();
        s.set_selection(hs);
    }
    Ok(r)
}

pub(super) fn run_count(s: &mut Session, p: &Value) -> Result<Value> {
    run(s, "count", p, false)
}

pub(super) fn run_countlist(s: &mut Session, p: &Value) -> Result<Value> {
    run(s, "countlist", p, true)
}

/// The current table style (CTABLESTYLE, else the first).
fn table_style(s: &Session) -> cadaddy_doc::TableStyle {
    let Ok(d) = s.doc() else { return Default::default() };
    let cur = d.header.str("CTABLESTYLE", "Standard");
    d.table_styles.iter().find(|t| t.name.eq_ignore_ascii_case(&cur)).or(d.table_styles.first()).cloned().unwrap_or_default()
}

/// The rows of a count table: (block, count) in name order.
fn count_cells(t: &Tally) -> Vec<Vec<String>> {
    t.blocks.iter().map(|(name, n)| vec![name.clone(), n.to_string()]).collect()
}

pub(super) fn run_counttable(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "counttable";
    if bool_or(p, "update", false) {
        let n = update_tables(s, p.get("handles").is_some().then(|| targets(s, p)).transpose()?)?;
        return Ok(json!({ "updated": n, "message": format!("{n} count table(s) updated.") }));
    }
    let at = point_req(CMD, p, "at")?;
    let scope = Scope::parse(s, CMD, p, false)?;
    let nested = bool_or(p, "nested", true);
    let t = tally(s.doc()?, &scope, nested, None);
    if t.blocks.is_empty() {
        return Err(bad(CMD, format!("No block references in {}.", scope.label())));
    }
    let cells = count_cells(&t);
    let header = Some(vec!["Name".to_string(), "Count".to_string()]);
    let mut table = super::super::table::make_table(&table_style(s), at, cells.len(), 2, None, None, Some("Block Count"), header, &cells);
    table.count = scope.link(nested);
    let h = s.add_entity(EntityKind::Table(Box::new(table)))?;
    Ok(
        json!({ "handle": h.hex(), "rows": cells.len(), "linked": scope.link(nested).is_some(), "message": format!("Table of {} block(s) inserted.", cells.len()) }),
    )
}

/// Whether a count table shows other counts than the drawing has now.
pub(crate) fn stale(d: &Drawing, t: &cadaddy_doc::Table) -> Option<Vec<Vec<String>>> {
    let link = t.count.as_ref()?;
    let want = count_cells(&tally(d, &Scope::from_link(link), link.nested, None));
    // Data rows follow the title and header rows.
    let skip = usize::from(t.title) + usize::from(t.header);
    let have: Vec<Vec<String>> = t.cells.iter().skip(skip).map(|r| r.iter().map(|c| c.text.clone()).collect()).collect();
    (have != want).then_some(want)
}

/// Work out the counts of the linked count tables again (`only` these, else all); how many changed.
pub(crate) fn update_tables(s: &mut Session, only: Option<Vec<Handle>>) -> Result<usize> {
    let style = table_style(s);
    let d = s.doc()?;
    let mut changes: Vec<(Handle, cadaddy_doc::Table)> = Vec::new();
    let all = d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter()));
    for e in all.take(5_000_000) {
        let EntityKind::Table(t) = &e.kind else { continue };
        if t.count.is_none() || only.as_ref().is_some_and(|o| !o.contains(&e.handle)) {
            continue;
        }
        let Some(cells) = stale(d, t) else { continue };
        let st = d.table_styles.iter().find(|x| x.name.eq_ignore_ascii_case(&t.style)).cloned().unwrap_or_else(|| style.clone());
        let at = Vec2::new(t.insert.x, t.insert.y);
        let title = t.title.then(|| t.cells.first().and_then(|r| r.first()).map(|c| c.text.clone()).unwrap_or_default());
        let header = t.header.then(|| vec!["Name".to_string(), "Count".to_string()]);
        let mut nt = super::super::table::make_table(&st, at, cells.len().max(1), 2, None, None, title.as_deref(), header, &cells);
        nt.count = t.count.clone();
        nt.col_widths = t.col_widths.clone();
        changes.push((e.handle, nt));
    }
    let n = changes.len();
    if n == 0 {
        // Untouched: the drawing stays as saved.
        return Ok(0);
    }
    let d = s.doc_mut()?;
    for (h, t) in changes {
        d.modify_entity(h, |e| e.kind = EntityKind::Table(Box::new(t))).map_err(|e| bad("counttable", e.to_string()))?;
    }
    Ok(n)
}

fn echo_lines(s: &mut Session, r: &Value) {
    if let Some(m) = r.get("message").and_then(Value::as_str) {
        for l in m.lines() {
            s.echo(l.to_string());
        }
    }
}

/// COUNT at the command line: pick an object to count the ones like it (highlighted), select
/// several to list the blocks among them, list the blocks in this space or in all of them, or
/// count in an area (two corners).
#[derive(Default)]
struct CountM {
    /// Area: None before the keyword, then the first corner once picked.
    area: Option<Option<Vec2>>,
}

impl Interactive for CountM {
    fn name(&self) -> &'static str {
        "COUNT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.area {
            Some(None) => Prompt::new("Specify first corner of area", Accept::POINT),
            Some(Some(_)) => Prompt::new("Specify opposite corner", Accept::POINT),
            None => Prompt::new("Select object to count", Accept::SELECT).kw(&["List", "Spaces", "Area"]).default("List"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if let Some(first) = self.area {
            let Input::Point(p) = i else { return Ok(if matches!(i, Input::Enter) { Step::Done } else { Step::Continue }) };
            let Some(a) = first else {
                self.area = Some(Some(p));
                return Ok(Step::Continue);
            };
            let r = run(s, "count", &json!({ "area": [[a.x, a.y], [p.x, p.y]] }), false)?;
            echo_lines(s, &r);
            return Ok(Step::Done);
        }
        let p = match i {
            Input::Pick(hs) => match hs.as_slice() {
                [] => return Ok(Step::Continue),
                [h] => json!({ "object": h.hex(), "highlight": true }),
                many => json!({ "handles": hex(many) }),
            },
            Input::Keyword(k) if k == "Area" => {
                self.area = Some(None);
                return Ok(Step::Continue);
            }
            Input::Keyword(k) if k == "Spaces" => json!({ "space": "all" }),
            Input::Keyword(_) | Input::Enter => json!({}),
            _ => return Ok(Step::Continue),
        };
        let r = run(s, "count", &p, false)?;
        echo_lines(s, &r);
        Ok(Step::Done)
    }
}

struct CountTableM;

impl Interactive for CountTableM {
    fn name(&self) -> &'static str {
        "COUNTTABLE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Specify insertion point", Accept::POINT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Point(at) => {
                let r = run_counttable(s, &json!({ "at": [at.x, at.y] }))?;
                echo_lines(s, &r);
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}
