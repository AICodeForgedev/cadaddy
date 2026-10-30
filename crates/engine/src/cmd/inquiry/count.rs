//! COUNT, COUNTLIST and COUNTTABLE: block references counted in model space, in the layouts and
//! nested inside other blocks (multiplied by the references holding them), or in a selection; and
//! the objects shaped like a picked one.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Insert, MAX_BLOCK_DEPTH, Space};
use cadaddy_geom::{Mat3, Vec2};
use serde_json::{Value, json};

use super::super::*;
use super::SelfCheck;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Merges done while expanding nested references; a hostile file stops here (and says so).
const MAX_WORK: u64 = 5_000_000;
/// Top-level objects reported (and highlighted) per count.
const MAX_HANDLES: usize = 100_000;

/// COUNT's JSON parameters.
pub(super) const PARAMS: &str = "{block?, object?: handle (count objects like this one), space?: current|model|all|<layout> (default current), \
     handles?: [...] (count in these objects), area?: [[x,y],[x,y]] | polygon (references inserted inside it), nested?: bool (default true), \
     highlight?: bool, similar?: bool (list: also similar: {name: {scaled: [handles], exploded: [[handles]]}})} → \
     {blocks: {name: n}, nested: {name: n}, spaces: {space: {name: n}}, total} | {block, count, nested, handles}, plus `message` and \
     `selfCheck` (found/analyzed/skipped counts with reasons, and the caps hit: work, handles, blockDepth)";

pub(super) fn machine() -> Box<dyn Interactive> {
    Box::new(CountM::default())
}

pub(super) fn table_machine() -> Box<dyn Interactive> {
    Box::new(CountTableM::default())
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
    /// A block at the end of the depth cap still held references.
    depth_capped: bool,
}

impl<'a> Expander<'a> {
    /// An expander that has not traversed any block yet.
    fn new(d: &'a Drawing) -> Self {
        Expander { d, memo: HashMap::new(), work: 0, truncated: false, depth_capped: false }
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
        if levels == 0 {
            // More references than MAX_BLOCK_DEPTH covers: say so rather than silently stop.
            if self.d.block(name).is_some_and(|b| b.entities.iter().any(|e| matches!(&e.kind, EntityKind::Insert(_)))) {
                self.depth_capped = true;
            }
            let out = Rc::new(BTreeMap::new());
            self.memo.insert(key, out.clone());
            return out;
        }
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        if let Some(b) = self.d.block(name) {
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
    /// Top-level objects scanned, and the block references among them.
    found: u64,
    analyzed: u64,
    /// Top-level objects that are not block references, by type.
    skipped: BTreeMap<&'static str, u64>,
    /// More top-level objects held the block than `MAX_HANDLES`.
    handles_capped: bool,
    depth_capped: bool,
}

/// Count block references in `scope`; with `want`, also collect the top-level objects holding it.
fn tally(d: &Drawing, scope: &Scope, nested: bool, want: Option<&str>) -> Tally {
    let mut x = Expander::new(d);
    let mut t = Tally::default();
    for (space, e) in scope.objects(d) {
        t.found = t.found.saturating_add(1);
        let EntityKind::Insert(i) = &e.kind else {
            let n = t.skipped.entry(e.kind.type_name()).or_default();
            *n = n.saturating_add(1);
            continue;
        };
        t.analyzed = t.analyzed.saturating_add(1);
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
        if holds {
            if t.handles.len() < MAX_HANDLES {
                t.handles.push(e.handle);
            } else {
                t.handles_capped = true;
            }
        }
    }
    t.spaces.retain(|_, m| !m.is_empty());
    t.truncated = x.truncated;
    t.depth_capped = x.depth_capped;
    t
}

/// The self-check for a COUNT-style result: what was scanned, counted and skipped, and the caps
/// that stopped it.
fn self_check(t: &Tally) -> Value {
    let mut check = SelfCheck::new();
    check.found(t.found);
    check.analyzed(t.analyzed);
    for (ty, n) in &t.skipped {
        check.skip(ty, "not a block reference", *n);
    }
    if t.truncated {
        check.cap("work", MAX_WORK);
    }
    if t.handles_capped {
        check.cap("handles", u64::try_from(MAX_HANDLES).unwrap_or(u64::MAX));
    }
    if t.depth_capped {
        check.cap("blockDepth", u64::try_from(MAX_BLOCK_DEPTH).unwrap_or(u64::MAX));
    }
    check.to_json()
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

/// Loose objects of one (type, shape): in scan order, and by anchor cell so a part lookup is a
/// hash lookup. Cells are 2e-6 wide; the 3x3 neighbourhood of a wanted anchor covers the 1e-6
/// match radius.
#[derive(Default)]
struct Bucket {
    list: Vec<(Vec2, Handle)>,
    grid: HashMap<(i64, i64), Vec<usize>>,
}

fn cell(p: Vec2) -> (i64, i64) {
    // Saturating float-to-int casts keep hostile coordinates harmless.
    ((p.x / 2e-6).floor() as i64, (p.y / 2e-6).floor() as i64)
}

impl Bucket {
    fn push(&mut self, at: Vec2, h: Handle) {
        self.grid.entry(cell(at)).or_default().push(self.list.len());
        self.list.push((at, h));
    }

    /// The first object (in scan order) anchored within 1e-6 of `want` that `skip` does not refuse.
    fn find(&self, want: Vec2, skip: impl Fn(Handle) -> bool) -> Option<Handle> {
        let (cx, cy) = cell(want);
        let mut best: Option<(usize, Handle)> = None;
        for dx in -1..=1i64 {
            for dy in -1..=1i64 {
                let key = (cx.saturating_add(dx), cy.saturating_add(dy));
                for &i in self.grid.get(&key).into_iter().flatten() {
                    let Some(&(p, h)) = self.list.get(i) else { continue };
                    if (p - want).len() < 1e-6 && !skip(h) && best.is_none_or(|(j, _)| i < j) {
                        best = Some((i, h));
                    }
                }
            }
        }
        best.map(|(_, h)| h)
    }
}

/// Loose (non-reference) objects in scope by (type, shape): their anchor and handle.
struct ShapeIndex {
    shapes: HashMap<(&'static str, String), Bucket>,
    /// The scan stopped at the object cap.
    capped: bool,
}

const MAX_INDEXED: usize = 200_000;

fn shape_index(d: &Drawing, scope: &Scope) -> ShapeIndex {
    let mut idx = ShapeIndex { shapes: HashMap::new(), capped: false };
    let objects = scope.objects(d);
    idx.capped = objects.len() > MAX_INDEXED;
    for (_, e) in objects.into_iter().take(MAX_INDEXED) {
        if matches!(e.kind, EntityKind::Insert(_)) {
            continue;
        }
        let (Some(key), Some(at)) = (shape_key(&e.kind), e.kind.grips().first().copied().filter(|p| p.is_finite())) else { continue };
        idx.shapes.entry((e.kind.type_name(), key)).or_default().push(at, e.handle);
    }
    idx
}

/// Groups of loose objects that repeat block `name`'s definition moved (an exploded copy, at
/// its own scale and unrotated). Every candidate and part lookup is charged to `work`; when it
/// runs out the groups found so far come back and `truncated` is set.
fn exploded(d: &Drawing, idx: &ShapeIndex, name: &str, work: &mut u64, truncated: &mut bool) -> Vec<Vec<Handle>> {
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
    let Some(first) = idx.shapes.get(&(t0, k0)) else { return out };
    // Look the later parts' buckets up once, not once per candidate.
    let rest: Vec<(Option<&Bucket>, Vec2)> = parts.iter().skip(1).map(|(t, k, a)| (idx.shapes.get(&(*t, k.clone())), *a)).collect();
    for (at, h) in &first.list {
        *work = work.saturating_add(1);
        if *work > MAX_WORK {
            *truncated = true;
            return out;
        }
        if used.contains(h) {
            continue;
        }
        let off = *at - a0;
        let mut group = vec![*h];
        let mut ok = true;
        for (bucket, a) in &rest {
            *work = work.saturating_add(1);
            if *work > MAX_WORK {
                *truncated = true;
                return out;
            }
            let hit = bucket.and_then(|bk| bk.find(*a + off, |hh| used.contains(&hh) || group.contains(&hh)));
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
fn similar(d: &Drawing, scope: &Scope) -> (serde_json::Map<String, Value>, bool) {
    let mut out = serde_json::Map::new();
    let idx = shape_index(d, scope);
    let mut work = 0u64;
    let mut truncated = idx.capped;
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
        let ex = exploded(d, &idx, &n, &mut work, &mut truncated);
        let sc = scaled.remove(&n).unwrap_or_default();
        if ex.is_empty() && sc.is_empty() {
            continue;
        }
        out.insert(n, json!({ "scaled": hex(&sc), "exploded": ex.iter().map(|g| hex(g)).collect::<Vec<_>>() }));
    }
    (out, truncated)
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
    json!({
        "blocks": t.blocks,
        "nested": t.nested,
        "spaces": t.spaces,
        "total": total,
        "truncated": t.truncated,
        "selfCheck": self_check(t),
        "message": message,
    })
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
    json!({
        "block": name,
        "count": n,
        "nested": k,
        "handles": hex(&t.handles),
        "truncated": t.truncated,
        "selfCheck": self_check(&t),
        "message": message,
    })
}

/// Count the objects in `scope` shaped like `h` (a block reference: references of its block).
fn count_like(d: &Drawing, scope: &Scope, nested: bool, h: Handle) -> Result<Value> {
    let e = d.entity(h).ok_or_else(|| bad("count", format!("no object with handle {}", h.hex())))?;
    if let EntityKind::Insert(i) = &e.kind {
        return Ok(count_block(d, scope, nested, &i.block));
    }
    let ty = e.kind.type_name();
    let key = shape_key(&e.kind).ok_or_else(|| bad("count", format!("a {ty} can't be counted by shape")))?;
    let mut check = SelfCheck::new();
    let mut hs: Vec<Handle> = Vec::new();
    let mut capped = false;
    for (_, o) in scope.objects(d) {
        check.found(1);
        if o.kind.type_name() != ty {
            check.skip(o.kind.type_name(), "a different type", 1);
            continue;
        }
        check.analyzed(1);
        if shape_key(&o.kind).as_deref() == Some(key.as_str()) {
            if hs.len() < MAX_HANDLES {
                hs.push(o.handle);
            } else {
                capped = true;
            }
        }
    }
    if capped {
        check.cap("handles", u64::try_from(MAX_HANDLES).unwrap_or(u64::MAX));
    }
    let n = hs.len();
    let message = format!("{ty}: {n} like the selected one in {}.", scope.label());
    Ok(json!({ "object": ty, "count": n, "handles": hex(&hs), "selfCheck": check.to_json(), "message": message }))
}

/// The body of COUNT and COUNTLIST: list the blocks in scope, count one block, or count the
/// objects shaped like a picked one. `default_all` makes an omitted `space` mean every space
/// (COUNTLIST) instead of the current one (COUNT).
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
            let (sim, cut) = similar(d, &scope);
            o.insert("similar".into(), Value::Object(sim));
            if cut {
                // The similar-copies scan ran out of work: say so, never a silent partial.
                o.insert("truncated".into(), json!(true));
                if let Some(caps) = o.get_mut("selfCheck").and_then(|c| c.get_mut("caps")).and_then(Value::as_object_mut) {
                    caps.insert("work".into(), json!(MAX_WORK));
                }
                if let Some(m) = o.get("message").and_then(Value::as_str).map(|m| format!("{m}\n  (similar-copies scan too large: partial)")) {
                    o.insert("message".into(), json!(m));
                }
            }
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
    // Readable at the drawing's scale: the size the style gives, times `scale` (default: the
    // annotation scale in model space, 1 on a layout).
    let scale = match p.get("scale").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|k| k.is_finite() && *k > 0.0 && *k <= 1e6).ok_or_else(|| bad(CMD, "`scale` must be a positive number"))?,
        None => default_scale(s),
    };
    scale_table(&mut table, scale);
    let h = s.add_entity(EntityKind::Table(Box::new(table)))?;
    Ok(json!({
        "handle": h.hex(),
        "rows": cells.len(),
        "linked": scope.link(nested).is_some(),
        "selfCheck": self_check(&t),
        "message": format!("Table of {} block(s) inserted.", cells.len()),
    }))
}

/// A new count table's scale: the annotation scale (drawing units per paper unit) in model
/// space, 1 on a layout (paper units already).
fn default_scale(s: &Session) -> f64 {
    let Ok(d) = s.doc() else { return 1.0 };
    if s.space() == cadaddy_doc::Space::Model { d.current_anno_scale().factor() } else { 1.0 }
}

/// A table's sizes times `k`: columns, rows and text together.
fn scale_table(t: &mut cadaddy_doc::Table, k: f64) {
    if !(k.is_finite() && k > 0.0) || (k - 1.0).abs() < 1e-12 {
        return;
    }
    t.col_widths.iter_mut().for_each(|w| *w *= k);
    t.row_heights.iter_mut().for_each(|h| *h *= k);
    t.text_height *= k;
    for c in t.cells.iter_mut().flatten() {
        if let Some(h) = &mut c.text_height {
            *h *= k;
        }
    }
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

/// `t` with its data rows set to `cells`: the title and header rows, the sizes (a scaled table
/// stays scaled) and each row's formatting kept; a new row is like the last data row (or, with
/// none, sized from the table's text height).
fn refill(t: &cadaddy_doc::Table, cells: &[Vec<String>], style: &cadaddy_doc::TableStyle) -> cadaddy_doc::Table {
    let mut nt = t.clone();
    let skip = (usize::from(t.title) + usize::from(t.header)).min(t.cells.len());
    let cols = t.col_widths.len().max(2);
    let template_row: Vec<cadaddy_doc::TableCell> =
        t.cells.get(skip..).and_then(|r| r.last()).cloned().unwrap_or_else(|| vec![cadaddy_doc::TableCell::default(); cols]);
    let template_h = t.row_heights.get(skip..).and_then(|r| r.last()).copied().unwrap_or(t.text_height * 5.0 / 3.0 + 2.0 * style.margin);
    let template_style = t.row_styles.get(skip..).and_then(|r| r.last()).cloned();
    nt.cells.truncate(skip);
    nt.row_heights.truncate(skip);
    if nt.row_styles.len() > skip {
        nt.row_styles.truncate(skip);
    }
    for (i, row) in cells.iter().enumerate() {
        let mut r = t.cells.get(skip + i).cloned().unwrap_or_else(|| template_row.clone());
        r.resize(cols, cadaddy_doc::TableCell::default());
        for (c, text) in r.iter_mut().zip(row) {
            c.text = text.clone();
            c.formula = None;
        }
        nt.cells.push(r);
        nt.row_heights.push(t.row_heights.get(skip + i).copied().unwrap_or(template_h));
        if !t.row_styles.is_empty() {
            nt.row_styles.push(t.row_styles.get(skip + i).cloned().or_else(|| template_style.clone()).unwrap_or_default());
        }
    }
    nt
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
        changes.push((e.handle, refill(t, &cells, &style)));
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

/// A scale as typed: up to four decimals, no trailing zeros.
fn num(k: f64) -> String {
    let t = format!("{k:.4}");
    t.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// COUNTTABLE typed: the insertion point, or Scale first (default: the annotation scale).
#[derive(Default)]
struct CountTableM {
    scale: Option<f64>,
    asking_scale: bool,
}

impl Interactive for CountTableM {
    fn name(&self) -> &'static str {
        "COUNTTABLE"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        if self.asking_scale {
            let k = self.scale.unwrap_or_else(|| default_scale(s));
            return Prompt::new("Specify table scale", Accept::NUMBER).default(num(k));
        }
        Prompt::new("Specify insertion point or", Accept::POINT).kw(&["Scale"])
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.asking_scale {
            match i {
                Input::Text(t) => match t.trim().parse::<f64>() {
                    Ok(k) if k.is_finite() && k > 0.0 => {
                        self.scale = Some(k);
                        self.asking_scale = false;
                    }
                    _ => s.echo("Requires a positive number."),
                },
                Input::Enter => self.asking_scale = false,
                Input::Cancel => return Ok(Step::Cancel),
                _ => {}
            }
            return Ok(Step::Continue);
        }
        match i {
            Input::Point(at) => {
                let mut p = json!({ "at": [at.x, at.y] });
                if let Some(k) = self.scale {
                    p["scale"] = json!(k);
                }
                let r = run_counttable(s, &p)?;
                echo_lines(s, &r);
                Ok(Step::Done)
            }
            Input::Keyword(_) => {
                self.asking_scale = true;
                Ok(Step::Continue)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}
