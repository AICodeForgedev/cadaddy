//! Revisions of the prints (Q270): a revised sheet — a bulletin, an ASI — compared with the one
//! the drawing holds, and what changed shown, taken in and logged.
//!
//! - `printrev.compare {against}` aligns the revised sheet with this one (a reissued sheet is
//!   often plotted shifted: the offset comes from the notes and tags the two sheets share), then
//!   compares them object by object — geometry and text, after alignment. What was added,
//!   removed and retexted (a tag that now reads `28x12`) is grouped into regions, and each region
//!   gets a revision cloud with the revision's label on the `REV-CLOUD` layer.
//! - With `apply` (the default) the drawing takes the revised sheet's changes, and when it has
//!   MEP runs the runs are read again and matched with the ones before: resized, raised or
//!   lowered (a height note), rerouted, added and removed runs. Only those runs change in the
//!   building spec, and the 3D model is rebuilt only for them: the other runs and their 3D stay
//!   as they are. An edit someone made to a run (`mep.edit`) stays when the revision didn't
//!   change that part of it; when it did, the print wins and the edit is listed as a conflict.
//!   The rebuilt runs are selected, so the 3D view shows them.
//! - Every run gets an id that stays with it from one revision to the next, and every change
//!   goes into the run's revision log (`printrev.log`), saved with the drawing's building spec.

use std::collections::{BTreeMap, HashMap, HashSet};

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, RawTag, Space, Text};
use cadaddy_geom::{Bounds2, Mat3, Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, MepRevision, MepRun};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Objects compared, at most, on each sheet (hostile drawings).
const MAX_OBJECTS: usize = 200_000;
/// Changes listed (all are counted, clouded and taken in).
const MAX_LISTED: usize = 2_000;
/// Change boxes grouped into regions, at most; past it, one region holds the rest.
const MAX_BOXES: usize = 5_000;
/// Revision log entries kept, at most.
const MAX_LOG: usize = 100_000;
/// The layer the clouds go on.
pub const CLOUD_LAYER: &str = "REV-CLOUD";
/// The clouds' xdata application name.
pub(super) const APP: &str = "CADADDY_PRINTREV";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("printrev.compare", "Compare Revised Print", run_compare)
            .menu(&["Draw", "Modeling", "Compare Revised Print"])
            .interactive(|_| Ok(Box::new(CompareM::default())))
            .params(
            "{against: path (the revised sheet, DXF or DWG), label?: text (the revision, e.g. ASI-01; default Rev <n>), apply?: bool (default true: the drawing takes the revised sheet's changes, the changed MEP runs are updated and rebuilt), clouds?: bool (default true), tolerance?: drawing units (default 0.001), gap?: drawing units (changes this close share a cloud; default a fiftieth of the sheet)} → {revision, offset: [dx, dy], aligned_by, added, removed, retexted, changes: [{kind: added|removed|text, layer, at, from?, to?}], regions: [{min, max, changes, cloud?}], runs: [{id, system, level, change: resized|raised|rerouted|added|removed, from?, to?}], kept: [{id, part}], conflicts: [{id, part, edit, print}], rebuilt, highlighted: [handles]}",
        ),
        CommandSpec::new("printrev.log", "Revision Log", run_log)
            .params("{item?: run id (R12), revision?: label} → {entries: [{revision, item, system, level, change, from?, to?}], revisions: [labels]}")
            .noundo(),
    ]
}

/// Picked from the menu: the revised sheet's file, then the revision's label, on the command line.
#[derive(Default)]
struct CompareM {
    path: Option<String>,
}

impl crate::Interactive for CompareM {
    fn name(&self) -> &'static str {
        "PRINTREV"
    }
    fn prompt(&self, _s: &Session) -> crate::Prompt {
        use crate::{Accept, Prompt};
        match &self.path {
            None => Prompt::new("Enter name of the revised sheet's file", Accept::TEXT),
            Some(_) => Prompt::new("Enter the revision's label or <next number>", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: crate::Input) -> Result<crate::Step> {
        use crate::{Input, Step};
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        let Some(path) = self.path.clone() else {
            let (Input::Text(t) | Input::Keyword(t)) = i else { return Ok(Step::Cancel) };
            let path = t.trim().trim_matches('"').to_string();
            if path.is_empty() {
                return Ok(Step::Cancel);
            }
            let Some(found) = cadaddy_geom::filename::find_file(&path) else {
                s.echo(format!("Can't find the file {path}"));
                return Ok(Step::Continue);
            };
            self.path = Some(found.to_string_lossy().into_owned());
            return Ok(Step::Continue);
        };
        let label = match i {
            Input::Text(t) | Input::Keyword(t) => Value::String(t.trim().to_string()),
            _ => Value::Null,
        };
        run_compare(s, &json!({"against": path, "label": label}))?;
        Ok(Step::Done)
    }
}

fn read_file(cmd: &str, path: &str) -> Result<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::readfile::read(std::path::Path::new(path)).map_err(|e| bad(cmd, format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err(bad(cmd, format!("{path}: files can't be read in the web build")))
    }
}

// ---------------- alignment ----------------

/// A revision cloud or label this command drew.
fn ours(e: &Entity) -> bool {
    e.common.xdata.iter().any(|t| t.code == 1001 && t.value == APP)
}

/// The sheet's objects a comparison looks at: not the 3D model, not the revision clouds.
fn sheet_objects(d: &Drawing) -> Vec<&std::sync::Arc<Entity>> {
    super::bspec::plan_objects(d).into_iter().filter(|e| !ours(e)).take(MAX_OBJECTS).collect()
}

/// The notes and tags of a sheet: text and where it sits.
fn notes(d: &Drawing) -> Vec<(String, Vec2)> {
    let ents = sheet_objects(d);
    super::perceive::texts(&ents).into_iter().map(|(_, t, at, _)| (t, at)).filter(|(t, at)| t.chars().count() >= 2 && at.is_finite()).collect()
}

/// How far the revised sheet's drawing is moved from this one's, and how many shared notes say
/// so: each note that appears once on both sheets votes for its own offset, and the offset most
/// of them agree on wins. None share: no offset.
pub fn alignment(old: &Drawing, new: &Drawing, q: f64) -> (Vec2, usize) {
    let once = |v: Vec<(String, Vec2)>| {
        let mut m: HashMap<String, (usize, Vec2)> = HashMap::new();
        for (t, at) in v {
            let e = m.entry(t).or_insert((0, at));
            e.0 += 1;
        }
        m.into_iter().filter(|(_, (n, _))| *n == 1).map(|(t, (_, at))| (t, at)).collect::<HashMap<_, _>>()
    };
    let (a, b) = (once(notes(old)), once(notes(new)));
    let step = q.max(1e-9);
    let mut votes: HashMap<(i64, i64), (usize, Vec2)> = HashMap::new();
    for (t, pa) in &a {
        let Some(pb) = b.get(t) else { continue };
        let off = *pb - *pa;
        let key = (round_to(off.x, step), round_to(off.y, step));
        let e = votes.entry(key).or_insert((0, off));
        e.0 += 1;
    }
    votes.into_values().max_by(|x, y| x.0.cmp(&y.0).then(y.1.len().total_cmp(&x.1.len()))).map_or((Vec2::ZERO, 0), |(n, off)| (off, n))
}

fn round_to(v: f64, q: f64) -> i64 {
    let r = (v / q).round();
    if r.is_finite() { r.clamp(-9e18, 9e18) as i64 } else { 0 }
}

// ---------------- object signatures ----------------

/// `s` with every decimal number in it rounded to `q`: two objects whose numbers differ by less
/// than the tolerance read the same.
fn rounded(s: &str, q: f64) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        let prev_word = i > 0 && b.get(i - 1).is_some_and(|p| p.is_ascii_alphanumeric() || *p == b'_');
        let starts = c.is_ascii_digit() || (c == '-' && b.get(i + 1).is_some_and(u8::is_ascii_digit));
        if starts && !prev_word {
            let start = i;
            i += 1;
            while i < b.len() {
                let d = b[i];
                let exp_sign = (d == b'-' || d == b'+') && b.get(i - 1).is_some_and(|p| *p == b'e' || *p == b'E');
                if d.is_ascii_digit() || d == b'.' || d == b'e' || d == b'E' || exp_sign {
                    i += 1;
                } else {
                    break;
                }
            }
            let tok = s.get(start..i).unwrap_or_default();
            match tok.parse::<f64>() {
                Ok(v) if tok.contains(['.', 'e', 'E']) => {
                    out.push('#');
                    out.push_str(&round_to(v, q).to_string());
                }
                _ => out.push_str(tok),
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// An object as a comparison sees it: its layer, colour and geometry, rounded.
fn signature(e: &Entity, q: f64) -> String {
    format!("{}|{:?}|{}", e.common.layer, e.common.color, rounded(&format!("{:?}", e.kind), q))
}

fn text_of(e: &Entity) -> Option<(String, Vec2, f64)> {
    match &e.kind {
        EntityKind::Text(t) => Some((t.value.trim().to_string(), t.justify_point(), t.height)),
        EntityKind::MText(t) => {
            Some((cadaddy_fonts::plain_text(&t.contents).split_whitespace().collect::<Vec<_>>().join(" "), t.insert.xy(), t.height))
        }
        _ => None,
    }
}

/// What changed between two sheets, the revised one already moved onto this one.
#[derive(Default)]
struct SheetDiff {
    /// This sheet's objects the revision doesn't have.
    removed: Vec<Handle>,
    /// The revised sheet's objects this one doesn't have (moved onto it).
    added: Vec<Entity>,
    /// Pairs of a removed and an added text: a note or tag that reads differently now.
    retexted: Vec<(Handle, usize)>,
}

fn compare(old: &Drawing, new: &[Entity], q: f64) -> SheetDiff {
    let mut have: HashMap<String, Vec<Handle>> = HashMap::new();
    for e in sheet_objects(old) {
        have.entry(signature(e, q)).or_default().push(e.handle);
    }
    let mut d = SheetDiff::default();
    for e in new.iter().take(MAX_OBJECTS) {
        match have.get_mut(&signature(e, q)).and_then(Vec::pop) {
            Some(_) => {}
            None => d.added.push(e.clone()),
        }
    }
    let mut removed: Vec<Handle> = have.into_values().flatten().collect();
    removed.sort();
    d.removed = removed;
    // A text taken out and one put in at the same place, on the same layer: retexted.
    let mut used = HashSet::new();
    for h in &d.removed {
        let Some(old_e) = old.model.get(*h) else { continue };
        let Some((_, at, height)) = text_of(old_e) else { continue };
        let near = d
            .added
            .iter()
            .enumerate()
            .filter(|(i, _)| !used.contains(i))
            .filter(|(_, e)| e.common.layer == old_e.common.layer)
            .filter_map(|(i, e)| text_of(e).map(|(_, p, hh)| (i, p.dist(at), height.max(hh))))
            .filter(|(_, dist, h)| *dist <= h * 2.0 + q)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _, _)) = near {
            used.insert(i);
            d.retexted.push((*h, i));
        }
    }
    d
}

// ---------------- regions and clouds ----------------

/// Boxes that come within `gap` of each other, merged.
pub(super) fn regions(mut boxes: Vec<(Bounds2, usize)>, gap: f64) -> Vec<(Bounds2, usize)> {
    if boxes.len() > MAX_BOXES {
        let rest = boxes.split_off(MAX_BOXES);
        let mut all = Bounds2::EMPTY;
        let mut n = 0;
        for (b, k) in rest {
            all = all.union(&b);
            n += k;
        }
        boxes.push((all, n));
    }
    let grown = |b: &Bounds2| Bounds2::new(b.min - Vec2::new(gap, gap), b.max + Vec2::new(gap, gap));
    loop {
        let mut merged = false;
        let mut out: Vec<(Bounds2, usize)> = Vec::with_capacity(boxes.len());
        for (b, n) in boxes {
            match out.iter_mut().find(|(o, _)| grown(o).intersects(&b)) {
                Some(o) => {
                    o.0 = o.0.union(&b);
                    o.1 += n;
                    merged = true;
                }
                None => out.push((b, n)),
            }
        }
        boxes = out;
        if !merged {
            return boxes;
        }
    }
}

pub(super) fn ensure_layer(d: &mut Drawing, name: &str) {
    if d.layer(name).is_none() {
        d.layers.push(cadaddy_doc::Layer { name: name.into(), color: cadaddy_doc::color::Color::Index(1), ..cadaddy_doc::Layer::default() });
    }
}

/// On `layer`, tagged with `app`'s xdata carrying `label`.
pub(super) fn tagged_as(layer: &str, app: &str, label: &str) -> Common {
    Common {
        layer: layer.into(),
        xdata: vec![RawTag { code: 1001, value: app.into() }, RawTag { code: 1000, value: label.into() }],
        ..Common::default()
    }
}

fn tagged(label: &str) -> Common {
    tagged_as(CLOUD_LAYER, APP, label)
}

/// The label an object tagged by `app` carries.
pub(super) fn label_of<'a>(e: &'a Entity, app: &str) -> Option<&'a str> {
    let at = e.common.xdata.iter().position(|t| t.code == 1001 && t.value == app)?;
    e.common.xdata.get(at + 1).filter(|t| t.code == 1000).map(|t| t.value.as_str())
}

/// A revision cloud round `b` (padded by `pad`) and the revision's label at its top right.
fn cloud(d: &mut Drawing, b: &Bounds2, pad: f64, label: &str) -> Option<Handle> {
    cloud_with(d, b, pad, label, &tagged)
}

/// A cloud round `b` and `label` at its top right, both with the `common` it gives.
pub(super) fn cloud_with(d: &mut Drawing, b: &Bounds2, pad: f64, label: &str, common: &dyn Fn(&str) -> Common) -> Option<Handle> {
    let (lo, hi) = (b.min - Vec2::new(pad, pad), b.max + Vec2::new(pad, pad));
    let pts = [lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)];
    let size = (hi.x - lo.x).max(hi.y - lo.y);
    let arc = (size / 8.0).max(1e-6);
    let vs = super::draw2::cloud_vertices(&pts, arc, false, false)?;
    let h = d.add(&Space::Model, common(label), super::helpers::lwpoly(vs, true)).ok()?;
    let th = (size / 12.0).max(1e-6);
    let text = EntityKind::Text(Text {
        insert: Vec3::new(hi.x + th * 0.5, hi.y + th * 0.5, 0.0),
        align_pt: None,
        height: th,
        value: label.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: Default::default(),
        valign: Default::default(),
    });
    d.add(&Space::Model, common(label), text).ok();
    Some(h)
}

// ---------------- MEP runs ----------------

fn size_text(s: [f64; 2]) -> String {
    let n = |v: f64| {
        let r = (v * 100.0).round() / 100.0;
        if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") }
    };
    if s[1] > 0.0 { format!("{}x{}", n(s[0]), n(s[1])) } else { format!("{}\"", n(s[0])) }
}

fn heights_text(b: &[f64]) -> String {
    let mut v: Vec<String> = b.iter().map(|h| cadaddy_walk3d::feet(*h)).collect();
    v.dedup();
    v.join(", ")
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + d * t)
}

/// How far apart two runs' paths are: the farthest any point of one is from the other.
fn apart(a: &MepRun, b: &MepRun) -> f64 {
    let pts = |r: &MepRun| r.points.iter().map(|p| Vec2::new(p[0], p[1])).collect::<Vec<_>>();
    let (pa, pb) = (pts(a), pts(b));
    let one_way = |from: &[Vec2], to: &[Vec2]| {
        from.iter()
            .map(|p| to.windows(2).filter_map(|w| Some(seg_dist(*p, *w.first()?, *w.get(1)?))).fold(f64::INFINITY, f64::min))
            .fold(0.0f64, f64::max)
    };
    one_way(&pa, &pb).max(one_way(&pb, &pa))
}

fn same_path(a: &MepRun, b: &MepRun, q: f64) -> bool {
    a.points.len() == b.points.len() && a.points.iter().zip(&b.points).all(|(p, r)| (p[0] - r[0]).abs() <= q && (p[1] - r[1]).abs() <= q)
        || a.points.len() == b.points.len() && a.points.iter().rev().zip(&b.points).all(|(p, r)| (p[0] - r[0]).abs() <= q && (p[1] - r[1]).abs() <= q)
}

fn same_heights(a: &[f64], b: &[f64]) -> bool {
    let (mut x, mut y) = (a.to_vec(), b.to_vec());
    x.dedup_by(|p, r| (*p - *r).abs() < 1e-6);
    y.dedup_by(|p, r| (*p - *r).abs() < 1e-6);
    x.len() == y.len() && x.iter().zip(&y).all(|(p, r)| (p - r).abs() < 1e-6)
}

/// The runs before and after, paired: same system and level, the nearest path within `reach`.
fn pair(before: &[MepRun], after: &[MepRun], reach: f64) -> (Vec<(usize, usize)>, Vec<usize>, Vec<usize>) {
    let mut cands: Vec<(f64, usize, usize)> = Vec::new();
    for (i, a) in before.iter().enumerate() {
        for (j, b) in after.iter().enumerate() {
            if a.system == b.system && a.level.eq_ignore_ascii_case(&b.level) {
                let d = apart(a, b);
                if d <= reach {
                    cands.push((d, i, j));
                }
            }
        }
        if cands.len() > 1_000_000 {
            break;
        }
    }
    cands.sort_by(|x, y| x.0.total_cmp(&y.0));
    let (mut ui, mut uj) = (HashSet::new(), HashSet::new());
    let mut pairs = Vec::new();
    for (_, i, j) in cands {
        if !ui.contains(&i) && !uj.contains(&j) {
            ui.insert(i);
            uj.insert(j);
            pairs.push((i, j));
        }
    }
    pairs.sort();
    let gone = (0..before.len()).filter(|i| !ui.contains(i)).collect();
    let new = (0..after.len()).filter(|j| !uj.contains(j)).collect();
    (pairs, gone, new)
}

/// The next unused run id.
fn next_id(runs: &[MepRun], n: &mut usize) -> String {
    loop {
        *n += 1;
        let id = format!("R{n}");
        if !runs.iter().any(|r| r.id == id) {
            return id;
        }
    }
}

/// The spec run a run read from the plan stands for: the one read from the same objects, else
/// the one on the same path.
fn spec_index(spec: &[MepRun], read: &MepRun, q: f64) -> Option<usize> {
    spec.iter()
        .position(|r| !read.objects.is_empty() && r.objects.iter().filter(|o| o.as_str() != "reviewed").eq(read.objects.iter()))
        .or_else(|| spec.iter().position(|r| r.system == read.system && r.level.eq_ignore_ascii_case(&read.level) && same_path(r, read, q)))
}

/// The source plan object a generated MEP mesh came from (its xdata), as hex.
fn mesh_source(e: &Entity) -> Option<String> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP))?;
    (x.get(at + 1)?.value == "mep").then(|| x.get(at + 3).map(|t| t.value.clone()))?
}

struct RunOutcome {
    report: Vec<Value>,
    kept: Vec<Value>,
    conflicts: Vec<Value>,
    rebuilt: usize,
    highlighted: Vec<Handle>,
}

/// Read the runs again after the drawing took the revision, update the spec's changed runs
/// (keeping edits the revision didn't touch), log the changes and rebuild those runs' 3D.
fn update_runs(s: &mut Session, mut sp: BuildingSpec, before: Vec<MepRun>, label: &str, q: f64) -> Result<RunOutcome> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let (_, mut after) = super::mep::read(s, &sp, None)?;
    // Runs on layers the spec has no system for yet (a trade the revision brings in).
    if !sp.mep.systems.is_empty() {
        let mut open = sp.clone();
        open.mep.systems.clear();
        let (drafted, runs) = super::mep::read(s, &open, None)?;
        for sy in drafted {
            let known = sp.mep.systems.iter().any(|o| o.name == sy.name || super::qselect::wildcard(&o.layer, &sy.layer));
            if !known {
                after.extend(runs.iter().filter(|r| r.system == sy.name).cloned());
                sp.mep.systems.push(sy);
            }
        }
    }
    let (pairs, gone, new) = pair(&before, &after, 2.0 * upf);
    let mut out = RunOutcome { report: Vec::new(), kept: Vec::new(), conflicts: Vec::new(), rebuilt: 0, highlighted: Vec::new() };
    let mut log: Vec<MepRevision> = Vec::new();
    let mut n = 0;
    // Every run in the spec gets an id now, so the next revision can follow it.
    for i in 0..sp.mep.runs.len() {
        if sp.mep.runs.get(i).is_some_and(|r| r.id.is_empty()) {
            let id = next_id(&sp.mep.runs, &mut n);
            if let Some(r) = sp.mep.runs.get_mut(i) {
                r.id = id;
            }
        }
    }
    let entry = |id: &str, r: &MepRun, change: &str, from: String, to: String| MepRevision {
        revision: label.into(),
        item: id.into(),
        system: r.system.clone(),
        level: r.level.clone(),
        change: change.into(),
        from,
        to,
    };
    // Old sources whose 3D goes, new runs whose 3D is built.
    let mut stale: HashSet<String> = HashSet::new();
    let mut build: Vec<MepRun> = Vec::new();
    let mut remove: Vec<usize> = Vec::new();
    for (i, j) in &pairs {
        let (Some(a), Some(b)) = (before.get(*i), after.get(*j)) else { continue };
        let resized = a.size_in != b.size_in;
        let raised = !same_heights(&a.bottoms_ft, &b.bottoms_ft);
        let rerouted = !same_path(a, b, q);
        let relinked = a.objects != b.objects;
        if !(resized || raised || rerouted || relinked) {
            continue;
        }
        let at = spec_index(&sp.mep.runs, a, q);
        let id = match at.and_then(|k| sp.mep.runs.get(k)) {
            Some(r) => r.id.clone(),
            None => next_id(&sp.mep.runs, &mut n),
        };
        let mut changes = Vec::new();
        if resized {
            changes.push(entry(&id, b, "resized", size_text(a.size_in), size_text(b.size_in)));
        }
        if raised {
            changes.push(entry(&id, b, "raised", heights_text(&a.bottoms_ft), heights_text(&b.bottoms_ft)));
        }
        if rerouted {
            changes.push(entry(&id, b, "rerouted", format!("{} points", a.points.len()), format!("{} points", b.points.len())));
        }
        let mut run = b.clone();
        run.id = id.clone();
        if let Some(k) = at
            && let Some(sr) = sp.mep.runs.get(k)
        {
            // What someone changed by hand since the runs were read: keep it where the print
            // didn't change it; where it did, the print wins and the edit is listed.
            let size_edit = sr.size_in != a.size_in;
            let height_edit = !same_heights(&sr.bottoms_ft, &a.bottoms_ft) && sr.bottoms_ft.len() == a.bottoms_ft.len();
            for (edited, changed, part, edit, print) in [
                (size_edit, resized, "size", size_text(sr.size_in), size_text(b.size_in)),
                (height_edit, raised, "height", heights_text(&sr.bottoms_ft), heights_text(&b.bottoms_ft)),
            ] {
                if !edited {
                    continue;
                }
                if changed {
                    out.conflicts.push(json!({"id": id, "part": part, "edit": edit, "print": print}));
                } else {
                    out.kept.push(json!({"id": id, "part": part}));
                    match part {
                        "size" => run.size_in = sr.size_in,
                        _ if sr.bottoms_ft.len() == run.bottoms_ft.len() => run.bottoms_ft.clone_from(&sr.bottoms_ft),
                        _ => {}
                    }
                }
            }
            if sr.objects.last().is_some_and(|o| o == "reviewed") && !(resized || raised || rerouted) {
                run.objects.push("reviewed".into());
            }
            if let Some(src) = sr.objects.first() {
                stale.insert(src.clone());
            }
            if let Some(slot) = sp.mep.runs.get_mut(k) {
                *slot = run.clone();
            }
        } else {
            sp.mep.runs.push(run.clone());
        }
        for c in &changes {
            out.report.push(json!({"id": c.item, "system": c.system, "level": c.level, "change": c.change, "from": c.from, "to": c.to}));
        }
        log.extend(changes);
        build.push(run);
    }
    for i in &gone {
        let Some(a) = before.get(*i) else { continue };
        let Some(k) = spec_index(&sp.mep.runs, a, q) else { continue };
        let Some(sr) = sp.mep.runs.get(k) else { continue };
        let e = entry(&sr.id, sr, "removed", size_text(sr.size_in), String::new());
        out.report.push(json!({"id": e.item, "system": e.system, "level": e.level, "change": "removed", "from": e.from}));
        log.push(e);
        if let Some(src) = sr.objects.first() {
            stale.insert(src.clone());
        }
        remove.push(k);
    }
    for j in &new {
        let Some(b) = after.get(*j) else { continue };
        let mut run = b.clone();
        run.id = next_id(&sp.mep.runs, &mut n);
        let e = entry(&run.id, &run, "added", String::new(), size_text(run.size_in));
        out.report.push(json!({"id": e.item, "system": e.system, "level": e.level, "change": "added", "to": e.to}));
        log.push(e);
        sp.mep.runs.push(run.clone());
        build.push(run);
    }
    remove.sort_unstable();
    remove.dedup();
    for k in remove.into_iter().rev() {
        if k < sp.mep.runs.len() {
            sp.mep.runs.remove(k);
        }
    }
    sp.mep.revisions.extend(log);
    let over = sp.mep.revisions.len().saturating_sub(MAX_LOG);
    sp.mep.revisions.drain(..over);
    // The 3D of only the changed runs, when the model has MEP 3D.
    let d = s.doc_mut()?;
    let had_3d = d.model.iter().any(|e| mesh_source(e).is_some());
    if had_3d {
        let old: Vec<Handle> = d.model.iter().filter(|e| mesh_source(e).is_some_and(|src| stale.contains(&src))).map(|e| e.handle).collect();
        for h in old {
            d.model.remove(h);
        }
        let mut part = sp.clone();
        part.mep.runs = build;
        part.mep.terminals.clear();
        part.mep.equipment.clear();
        let before: HashSet<Handle> = d.model.handles().into_iter().collect();
        super::mep::build(d, &part);
        out.highlighted = d.model.handles().into_iter().filter(|h| !before.contains(h)).collect();
        out.rebuilt = part.mep.runs.len();
    }
    d.building_spec = Some(sp.to_json());
    if !out.highlighted.is_empty() {
        s.set_selection(out.highlighted.clone());
    }
    Ok(out)
}

// ---------------- the commands ----------------

fn run_compare(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "printrev.compare";
    let path = str_param(p, "against").ok_or_else(|| bad(cmd, "`against` (the revised sheet's file) is required"))?;
    let q = match p.get("tolerance") {
        None | Some(Value::Null) => 1e-3,
        Some(v) => v.as_f64().filter(|t| t.is_finite() && *t > 0.0 && *t <= 1e6).ok_or_else(|| bad(cmd, "`tolerance` must be a distance above 0"))?,
    };
    let apply = bool_or(p, "apply", true);
    let clouds = bool_or(p, "clouds", true);
    let hooks = super::file::io().ok_or_else(|| bad(cmd, "file formats are not available in this build"))?;
    let bytes = read_file(cmd, path)?;
    let other = (hooks.read)(&bytes, path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    let (has_spec, sp) = match super::bspec::current(s, cmd) {
        Ok((sp, stored)) => (stored, Some(sp)),
        Err(_) => (false, None),
    };
    let label = match str_param(p, "label").map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) => l.chars().take(64).collect::<String>(),
        None => {
            let mut seen: Vec<&str> = sp.as_ref().map(|sp| sp.mep.revisions.iter().map(|r| r.revision.as_str()).collect()).unwrap_or_default();
            seen.sort_unstable();
            seen.dedup();
            format!("Rev {}", seen.len() + 1)
        }
    };
    let d = s.doc()?;
    let ext = d.extents(&Space::Model);
    let gap = match p.get("gap") {
        None | Some(Value::Null) => (ext.width().max(ext.height()) / 50.0).max(q),
        Some(v) => v.as_f64().filter(|g| g.is_finite() && *g >= 0.0).ok_or_else(|| bad(cmd, "`gap` must be a distance"))?,
    };
    let (offset, aligned_by) = alignment(d, &other, q);
    let back = Mat3::translate(Vec2::new(-offset.x, -offset.y));
    let moved: Vec<Entity> = sheet_objects(&other)
        .into_iter()
        .map(|e| {
            let mut e = (**e).clone();
            e.kind.transform(&back);
            e
        })
        .collect();
    let diff = compare(d, &moved, q);
    let retexted_old: HashSet<Handle> = diff.retexted.iter().map(|(h, _)| *h).collect();
    let retexted_new: HashSet<usize> = diff.retexted.iter().map(|(_, i)| *i).collect();
    // The changes, listed and boxed.
    let mut changes = Vec::new();
    let mut boxes = Vec::new();
    let at = |b: &Bounds2| json!([b.center().x, b.center().y]);
    for (h, i) in &diff.retexted {
        let (Some(old_e), Some(new_e)) = (d.model.get(*h), diff.added.get(*i)) else { continue };
        let b = cadaddy_doc::entity_bounds(d, old_e, 0).union(&cadaddy_doc::entity_bounds(&other, new_e, 0));
        if changes.len() < MAX_LISTED {
            changes.push(json!({"kind": "text", "layer": old_e.common.layer, "at": at(&b), "from": text_of(old_e).map(|t| t.0), "to": text_of(new_e).map(|t| t.0)}));
        }
        boxes.push((b, 1));
    }
    for h in diff.removed.iter().filter(|h| !retexted_old.contains(h)) {
        let Some(e) = d.model.get(*h) else { continue };
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() {
            continue;
        }
        if changes.len() < MAX_LISTED {
            changes.push(json!({"kind": "removed", "layer": e.common.layer, "at": at(&b), "from": text_of(e).map(|t| t.0)}));
        }
        boxes.push((b, 1));
    }
    for (_, e) in diff.added.iter().enumerate().filter(|(i, _)| !retexted_new.contains(i)) {
        let b = cadaddy_doc::entity_bounds(&other, e, 0);
        if b.is_empty() {
            continue;
        }
        if changes.len() < MAX_LISTED {
            changes.push(json!({"kind": "added", "layer": e.common.layer, "at": at(&b), "to": text_of(e).map(|t| t.0)}));
        }
        boxes.push((b, 1));
    }
    let (n_added, n_removed, n_retexted) = (diff.added.len() - diff.retexted.len(), diff.removed.len() - diff.retexted.len(), diff.retexted.len());
    let found = regions(boxes, gap);
    // The runs as read before the drawing takes the revision.
    let before = match (&sp, has_spec, apply) {
        (Some(sp), true, true) if !sp.levels.is_empty() => super::mep::read(s, sp, None).map(|(_, r)| r).ok(),
        _ => None,
    };
    let mut region_json = Vec::new();
    let d = s.doc_mut()?;
    if apply {
        // The layers, styles and blocks the revised sheet's new objects need, then the objects.
        super::exchange::merge_tables(d, &other);
        let renames = super::exchange::block_names(d, &other);
        for (from, to) in &renames {
            let Some(b) = other.blocks.get(from) else { continue };
            let mut nb = (**b).clone();
            nb.name = to.clone();
            nb.entities = Default::default();
            for e in b.entities.iter() {
                let e = super::exchange::adopt(d, e, &renames);
                nb.entities.push(e);
            }
            d.blocks.insert(to.clone(), std::sync::Arc::new(nb));
        }
        for h in &diff.removed {
            d.model.remove(*h);
        }
        for e in &diff.added {
            let e = super::exchange::adopt(d, e, &renames);
            d.model.push(e);
        }
    }
    if clouds && !found.is_empty() {
        ensure_layer(d, CLOUD_LAYER);
    }
    for (b, n) in &found {
        let h = if clouds { cloud(d, b, (gap / 4.0).max(q), &label) } else { None };
        region_json.push(json!({"min": [b.min.x, b.min.y], "max": [b.max.x, b.max.y], "changes": n, "cloud": h.map(|h| h.hex())}));
    }
    let runs = match (before, sp) {
        (Some(before), Some(sp)) => Some(update_runs(s, sp, before, &label, q)?),
        _ => None,
    };
    let mut out = json!({
        "revision": label,
        "offset": [offset.x, offset.y],
        "aligned_by": aligned_by,
        "added": n_added,
        "removed": n_removed,
        "retexted": n_retexted,
        "changes": changes,
        "regions": region_json,
        "applied": apply,
    });
    if let Some(r) = runs {
        out["runs"] = json!(r.report);
        out["kept"] = json!(r.kept);
        out["conflicts"] = json!(r.conflicts);
        out["rebuilt"] = json!(r.rebuilt);
        out["highlighted"] = json!(r.highlighted.iter().map(|h| h.hex()).collect::<Vec<_>>());
    }
    s.echo(format!(
        "{label}: {} added, {} removed, {} retexted in {} region(s){}.",
        n_added,
        n_removed,
        n_retexted,
        found.len(),
        if aligned_by > 0 { format!(", aligned by {aligned_by} shared notes") } else { String::new() }
    ));
    Ok(out)
}

fn run_log(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = super::bspec::current(s, "printrev.log")?;
    let item = str_param(p, "item");
    let rev = str_param(p, "revision");
    let entries: Vec<&MepRevision> = sp
        .mep
        .revisions
        .iter()
        .filter(|e| item.is_none_or(|i| e.item.eq_ignore_ascii_case(i)))
        .filter(|e| rev.is_none_or(|r| e.revision.eq_ignore_ascii_case(r)))
        .collect();
    let mut labels: BTreeMap<&str, ()> = BTreeMap::new();
    let mut order: Vec<&str> = Vec::new();
    for e in &sp.mep.revisions {
        if labels.insert(e.revision.as_str(), ()).is_none() {
            order.push(e.revision.as_str());
        }
    }
    Ok(json!({ "entries": serde_json::to_value(&entries).unwrap_or(Value::Null), "revisions": order }))
}
