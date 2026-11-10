//! A print set one sheet at a time (Q6416): the sheets of a drawing that holds a whole print set
//! side by side in model space, and the one being worked on.
//!
//! A job imports every sheet of a set into one model space, each page to the right of the one
//! before, and records where each landed (the job's sheet regions, the building spec's sheet
//! links) and what it is (the job's sheet index: number and title). On a whole hotel set that is
//! over a million objects, and a view of all of it is more than anyone can use and more than a
//! laptop can draw. With a sheet open (the app opens a print set on its first sheet; Page Up and
//! Page Down step through them, [`specs`]) the screen works on that sheet alone: only the objects
//! whose bounds touch it are drawn ([`Session::view_doc`]), picked, selected and snapped to, and
//! ZOOM Extents fits the sheet. That is the view only: every command still reads the whole
//! drawing, nothing of it is saved, and a drawing without such records (an ordinary drawing, one
//! imported page) has no sheets here and works as it always did.

use std::sync::{Arc, Mutex, Weak};

use cadaddy_doc::{Drawing, Entity, EntityStore, Handle, Space, entity_bounds};
use cadaddy_geom::{Bounds2, Vec2};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, bad};
use crate::{Result, Session, View};

/// Most sheets listed (a set's index runs to a few hundred).
pub const MAX_SHEETS: usize = 10_000;
/// The largest thumbnail side, in pixels.
const MAX_THUMB: u32 = 512;

/// One sheet of the set: its number and title (empty when the records don't say) and where it
/// lies in model space.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintSheet {
    pub number: String,
    pub title: String,
    pub rect: Bounds2,
}

impl PrintSheet {
    fn to_json(&self, index: usize, active: bool) -> Value {
        let r = &self.rect;
        json!({ "index": index, "number": self.number, "title": self.title, "rect": [r.min.x, r.min.y, r.max.x, r.max.y], "active": active })
    }
}

// What the records hold of the sheets, read without the rest of the job state (tens of
// megabytes of JSON on a whole set).
#[derive(Deserialize, Default)]
struct AgentPart {
    #[serde(default)]
    guided_job: Option<JobPart>,
}

#[derive(Deserialize, Default)]
struct JobPart {
    #[serde(default)]
    sheet_regions: Vec<Value>,
    /// The set's sheet index (one entry per sheet, in set order), and the classified sheets.
    #[serde(default)]
    index: Option<Value>,
    #[serde(default)]
    sheets: Option<Value>,
}

#[derive(Deserialize, Default)]
struct SpecPart {
    #[serde(default)]
    sheets: Vec<Value>,
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").trim().chars().take(200).collect()
}

fn rect_of(v: &Value) -> Option<Bounds2> {
    let a = v.as_array()?;
    let n: Vec<f64> = a.iter().take(5).filter_map(Value::as_f64).collect();
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(n).ok()?;
    let b = Bounds2::new(Vec2::new(x0, y0), Vec2::new(x1, y1));
    (b.min.is_finite() && b.max.is_finite() && b.width() > 0.0 && b.height() > 0.0).then_some(b)
}

/// A sheet number without the view a several-view sheet's links add (`Q-201/3` is `Q-201`).
fn base(number: &str) -> &str {
    number.split('/').next().unwrap_or(number).trim()
}

fn same_rect(a: &Bounds2, b: &Bounds2) -> bool {
    let tol = 1e-6 * (a.width() + a.height()).max(1.0);
    a.min.near(b.min, tol) && a.max.near(b.max, tol)
}

/// The sheets `d` holds, in the set's order: the job's sheet regions (where each imported page
/// landed), then the building spec's sheet links that have a place; numbers and titles from the
/// job's sheet index where it has them. Several-view sheets (one record per view, one page) are
/// one sheet. The order is the index's where a sheet is in it, else page order (pages are laid
/// left to right as they were imported). Empty for a drawing without such records.
pub fn print_sheets(d: &Drawing) -> Vec<PrintSheet> {
    let job = d.agent.as_deref().and_then(|t| serde_json::from_str::<AgentPart>(t).ok()).and_then(|a| a.guided_job).unwrap_or_default();
    let spec = d.building_spec.as_deref().and_then(|t| serde_json::from_str::<SpecPart>(t).ok()).unwrap_or_default();
    let listed = |v: &Option<Value>| -> Vec<(String, String)> {
        v.as_ref()
            .and_then(|v| v.get("sheets"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(MAX_SHEETS)
            .map(|e| (base(&text(e, "number")).to_string(), text(e, "title")))
            .filter(|(n, _)| !n.is_empty())
            .collect()
    };
    // The index first (one entry per real sheet), then the classified sheets for any it lacks.
    let mut index = listed(&job.index);
    for (n, t) in listed(&job.sheets) {
        if !index.iter().any(|(m, _)| m.eq_ignore_ascii_case(&n)) {
            index.push((n, t));
        }
    }
    let records = job
        .sheet_regions
        .iter()
        .take(MAX_SHEETS)
        .filter_map(|r| Some((text(r, "number"), String::new(), rect_of(r.get("rect")?)?)))
        .chain(spec.sheets.iter().take(MAX_SHEETS).filter_map(|s| Some((text(s, "number"), text(s, "title"), rect_of(s.get("rect")?)?))));
    let mut out: Vec<(usize, PrintSheet)> = Vec::new();
    for (number, title, rect) in records {
        let number = base(&number).to_string();
        if out.iter().any(|(_, s)| same_rect(&s.rect, &rect) && s.number.eq_ignore_ascii_case(&number)) || out.len() >= MAX_SHEETS {
            continue;
        }
        let at = index.iter().position(|(n, _)| !number.is_empty() && n.eq_ignore_ascii_case(&number));
        let title = at.and_then(|i| index.get(i)).map(|(_, t)| t.clone()).filter(|t| !t.is_empty()).unwrap_or(title);
        out.push((at.unwrap_or(usize::MAX), PrintSheet { number, title, rect }));
    }
    out.sort_by(|(a, s), (b, t)| a.cmp(b).then(s.rect.min.x.total_cmp(&t.rect.min.x)).then(t.rect.max.y.total_cmp(&s.rect.max.y)));
    out.into_iter().map(|(_, s)| s).collect()
}

/// The objects of `d`'s model space whose bounds touch `rect` (and the infinite lines), in
/// drawing order: through the spatial index when there is one.
fn model_in(d: &Drawing, rect: &Bounds2) -> Vec<Arc<Entity>> {
    let ix = crate::spatial::index(d, &Space::Model);
    if let Some(ix) = &ix
        && let Some(found) = ix.query(rect, true)
    {
        return found.into_iter().filter_map(|i| ix.entity(i).cloned()).collect();
    }
    d.model.iter().filter(|e| crate::select::is_infinite(e) || entity_bounds(d, e, 0).intersects(rect)).cloned().collect()
}

/// `d` with only the objects of model space that touch `rect` (sharing them and everything else).
fn scoped(d: &Drawing, rect: &Bounds2) -> Drawing {
    let mut model = EntityStore::new();
    for e in model_in(d, rect) {
        model.push_shared(e);
    }
    let mut out = d.clone();
    out.model = model;
    out
}

/// Most changed objects [`follow`] applies one by one; a bigger change makes the view anew.
const MAX_FOLLOWED: usize = 20_000;

/// The view `shown` of drawing `was` (its objects touching `rect`) made into the view of `now`
/// by what changed between them: the erased objects go, the changed ones are replaced (or go,
/// moved off the sheet) and the new ones touching the sheet come in. An edit then costs what it
/// changed, not a search of the whole set. `None` (make it anew) when that could put objects in
/// another order than the drawing's (a changed object moved onto the sheet, objects added other
/// than on top), when the block definitions changed (a reference's extent with them), or for a
/// large change.
fn follow(was: &Drawing, shown: &Drawing, now: &Drawing, rect: &Bounds2) -> Option<Drawing> {
    if !was.blocks.same(&now.blocks) {
        return None;
    }
    let (added, modified, erased) = was.model.changes(&now.model);
    if added.len() + modified.len() + erased.len() > MAX_FOLLOWED {
        return None;
    }
    // New objects come in on top only where the drawing has them on top.
    let fresh: std::collections::HashSet<Handle> = added.iter().copied().collect();
    let on_top = || now.model.iter().skip(now.model.len().saturating_sub(added.len()));
    if !on_top().all(|e| fresh.contains(&e.handle)) {
        return None;
    }
    let touches = |e: &Entity| crate::select::is_infinite(e) || entity_bounds(now, e, 0).intersects(rect);
    let mut model = shown.model.clone();
    for h in erased {
        model.remove(h);
    }
    for h in modified {
        let e = now.model.get(h)?;
        match (model.contains(h), touches(e)) {
            (true, true) => {
                model.replace((**e).clone());
            }
            (true, false) => {
                model.remove(h);
            }
            (false, true) => return None,
            (false, false) => {}
        }
    }
    for e in on_top() {
        if touches(e) {
            model.push_shared(Arc::clone(e));
        }
    }
    let mut out = now.clone();
    out.model = model;
    Some(out)
}

fn rect_key(b: &Bounds2) -> [u64; 4] {
    [b.min.x.to_bits(), b.min.y.to_bits(), b.max.x.to_bits(), b.max.y.to_bits()]
}

/// The sheets as last listed, and the records they were read from.
struct Listed {
    agent: cadaddy_doc::shared::Shared<Option<String>>,
    spec: cadaddy_doc::shared::Shared<Option<String>>,
    sheets: Arc<Vec<PrintSheet>>,
}

/// The drawing as the open sheet shows it, and what it was made from.
struct Scoped {
    from: Weak<Drawing>,
    rect: [u64; 4],
    doc: Arc<Drawing>,
}

#[derive(Default)]
struct Cache {
    listed: Option<Listed>,
    scoped: Option<Scoped>,
}

/// A drawing's sheet being worked on ([`crate::DocState::sheet_nav`]): view state, never saved.
#[derive(Clone, Default)]
pub struct SheetNav {
    /// The open sheet, an index into [`Session::print_sheets`].
    pub active: Option<usize>,
    /// The sheets opened last, the latest first (with sheets on demand, those stay in memory:
    /// [`crate::sheetstore::CACHE`]).
    pub recent: Vec<usize>,
    cache: Arc<Mutex<Cache>>,
}

impl std::fmt::Debug for SheetNav {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SheetNav").field("active", &self.active).finish()
    }
}

impl SheetNav {
    /// `d`'s sheets ([`print_sheets`]), listed again only when its records changed.
    fn sheets(&self, d: &Drawing) -> Arc<Vec<PrintSheet>> {
        let mut cache = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(l) = &cache.listed
            && l.agent.same(&d.agent)
            && l.spec.same(&d.building_spec)
        {
            return l.sheets.clone();
        }
        let sheets = Arc::new(print_sheets(d));
        cache.listed = Some(Listed { agent: d.agent.clone(), spec: d.building_spec.clone(), sheets: sheets.clone() });
        sheets
    }

    /// `d` holding only what touches `rect`, made again only when the drawing changed: from the
    /// last one by what the edit changed when it can be ([`follow`]), else anew.
    fn scoped(&self, d: &Arc<Drawing>, rect: &Bounds2) -> Arc<Drawing> {
        let key = rect_key(rect);
        let mut cache = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let last = cache.scoped.as_ref().filter(|s| s.rect == key);
        if let Some(s) = last
            && std::ptr::eq(s.from.as_ptr(), Arc::as_ptr(d))
            && s.from.strong_count() > 0
        {
            return s.doc.clone();
        }
        let followed = last.and_then(|s| Some((s.from.upgrade()?, s.doc.clone()))).and_then(|(was, shown)| follow(&was, &shown, d, rect));
        let doc = Arc::new(followed.unwrap_or_else(|| scoped(d, rect)));
        cache.scoped = Some(Scoped { from: Arc::downgrade(d), rect: key, doc: doc.clone() });
        doc
    }
}

impl Session {
    /// The sheets of the current drawing's print set in the set's order ([`print_sheets`]);
    /// empty for an ordinary drawing.
    pub fn print_sheets(&self) -> Arc<Vec<PrintSheet>> {
        match self.state() {
            Ok(st) => st.sheet_nav.sheets(&st.doc),
            Err(_) => Arc::new(Vec::new()),
        }
    }

    /// The open sheet and its index, when the drawing is a print set and one is open.
    pub fn active_sheet(&self) -> Option<(usize, PrintSheet)> {
        let i = self.state().ok()?.sheet_nav.active?;
        self.print_sheets().get(i).map(|s| (i, s.clone()))
    }

    /// Where the screen works with a sheet open: the sheet's rectangle, in model space's plan
    /// view (a 3D view shows the building, a layout its own sheet). `None` otherwise.
    pub fn sheet_scope(&self) -> Option<Bounds2> {
        let st = self.state().ok()?;
        if st.space != Space::Model || st.mspace.is_some() || st.projection().is_some() || st.block_edit.is_some() {
            return None;
        }
        self.active_sheet().map(|(_, s)| s.rect)
    }

    /// The drawing as the screen shows it: with a sheet open, only the model-space objects whose
    /// bounds touch it (the same objects, shared); else the drawing itself. Drawing, picking,
    /// selecting and snapping use it; commands use the drawing.
    pub fn view_doc(&self) -> Result<Arc<Drawing>> {
        let st = self.state()?;
        Ok(match self.sheet_scope() {
            Some(rect) => st.sheet_nav.scoped(&st.doc, &rect),
            None => st.doc.clone(),
        })
    }

    /// The detail level the screen draws the current view at (see [`crate::DocState::screen_detail`]):
    /// only in model space's plan view, where the screen sets it.
    pub fn screen_detail(&self) -> Option<cadaddy_render::detail::Detail> {
        let st = self.state().ok()?;
        let plan = st.space == Space::Model && st.mspace.is_none() && st.projection().is_none();
        st.screen_detail.clone().filter(|_| plan)
    }

    /// Say what detail level the screen drew the current view at (`None`: everything in full).
    pub fn set_screen_detail(&mut self, detail: Option<cadaddy_render::detail::Detail>) {
        if let Ok(st) = self.state_mut()
            && st.screen_detail != detail
        {
            st.screen_detail = detail;
        }
    }

    /// Of `hs`, those in the open sheet's view (all of them when no sheet is open).
    pub fn in_sheet_view(&self, hs: Vec<Handle>) -> Vec<Handle> {
        if self.sheet_scope().is_none() {
            return hs;
        }
        let Ok(d) = self.view_doc() else { return hs };
        hs.into_iter().filter(|h| d.model.contains(*h)).collect()
    }

    /// Open sheet `i`: model space's plan view fits it, and the screen works on it alone.
    pub fn open_sheet(&mut self, i: usize) -> Result<PrintSheet> {
        let sheets = self.print_sheets();
        let sheet = sheets.get(i).cloned().ok_or_else(|| bad("sheet.open", format!("no sheet {} (the drawing has {})", i + 1, sheets.len())))?;
        let (w, h) = self.viewport_px;
        let inset = self.viewport_inset_px;
        let st = self.state_mut()?;
        if st.space != Space::Model {
            st.space = Space::Model;
            st.mspace = None;
        }
        // A sheet is read in plan.
        if st.projection().is_some() {
            st.view_dir = cadaddy_geom::Vec3::Z;
            st.view_twist = 0.0;
        }
        st.sheet_nav.active = Some(i);
        let view: View = crate::framed(sheet.rect, (w, h), inset, |a| a, 1.02);
        st.set_zoom_frame(view);
        // With sheets on demand, the sheet is in memory and all but the recent ones are not.
        self.keep_open_sheets(i)?;
        Ok(sheet)
    }

    /// Open the sheet `by` places after the open one (before it when negative), stopping at the
    /// first and last. `None` for a drawing with no sheets.
    pub fn step_sheet(&mut self, by: i64) -> Result<Option<PrintSheet>> {
        let n = self.print_sheets().len();
        if n == 0 {
            return Ok(None);
        }
        let at = self.active_sheet().map_or(0, |(i, _)| i as i64);
        let to = at.saturating_add(by).clamp(0, n as i64 - 1) as usize;
        self.open_sheet(to).map(Some)
    }

    /// A print set with no sheet open opens on its first (the app asks every frame, so a set
    /// opened from a file or made by a job starts on a sheet). True when it opened one.
    pub fn open_first_sheet(&mut self) -> bool {
        let Ok(st) = self.state() else { return false };
        if st.space != Space::Model || st.mspace.is_some() || st.block_edit.is_some() {
            return false;
        }
        let n = self.print_sheets().len();
        match self.active_sheet() {
            Some(_) => false,
            None if n > 0 => self.open_sheet(0).is_ok(),
            None => {
                // The set's records went (a new drawing in their place): nothing is open.
                if let Ok(st) = self.state_mut() {
                    st.sheet_nav.active = None;
                }
                false
            }
        }
    }

    /// A `w` × `h` picture of sheet `i` (premultiplied RGBA pixels, row by row), for the sheet
    /// list.
    pub fn sheet_thumbnail_rgba(&self, i: usize, w: u32, h: u32) -> Option<Vec<u8>> {
        use cadaddy_render::raster::{self, RasterOptions};
        let (w, h) = (w.clamp(1, MAX_THUMB), h.clamp(1, MAX_THUMB));
        let st = self.state().ok()?;
        let sheets = st.sheet_nav.sheets(&st.doc);
        let rect = sheets.get(i)?.rect;
        // The open sheet's objects are its view's (an edit there needs no search of the set).
        let ents: Vec<Arc<Entity>> = match self.sheet_scope().filter(|_| st.sheet_nav.active == Some(i)) {
            Some(_) => self.view_doc().ok()?.model.iter().cloned().collect(),
            None => model_in(&st.doc, &rect),
        };
        let k = (f64::from(w) / rect.width()).min(f64::from(h) / rect.height()) * 0.96;
        if !(k.is_finite() && k > 0.0) {
            return None;
        }
        // Half a pixel of the picture: the coarsest curves and text that still look right.
        let px = 1.0 / k;
        let opts = cadaddy_render::Options { tolerance: px * 0.5, min_dash: px * 2.0, view: Some(rect), ..Default::default() };
        let list = cadaddy_render::build_entities(&st.doc, ents.iter().map(|e| e.as_ref()), &opts);
        let view = raster::View { center: rect.center(), scale: k, width: w, height: h };
        raster::render(&list, &view, &RasterOptions::default()).map(|pm| pm.data().to_vec())
    }
}

/// Where a sheet named by a command's `sheet` (its number, or its place in the list from 1) is.
fn find(s: &Session, v: &Value) -> Result<usize> {
    let sheets = s.print_sheets();
    if let Some(n) = v.as_u64() {
        return usize::try_from(n)
            .ok()
            .filter(|n| *n >= 1 && *n <= sheets.len())
            .map(|n| n - 1)
            .ok_or_else(|| bad("sheet.open", format!("no sheet {n}: the drawing has {} (sheets count from 1)", sheets.len())));
    }
    let name = v.as_str().map(str::trim).unwrap_or("");
    sheets
        .iter()
        .position(|sh| !name.is_empty() && sh.number.eq_ignore_ascii_case(name))
        .ok_or_else(|| bad("sheet.open", format!("no sheet `{name}` in this drawing: `sheet.list` lists them")))
}

fn listing(s: &Session) -> Value {
    let active = s.active_sheet().map(|(i, _)| i);
    let sheets = s.print_sheets();
    let list: Vec<Value> = sheets.iter().enumerate().map(|(i, sh)| sh.to_json(i + 1, active == Some(i))).collect();
    let mut out = json!({ "sheets": list, "active": active.map(|i| i + 1) });
    // What the screen works on of the drawing: the open sheet's objects of all of them.
    if s.sheet_scope().is_some()
        && let (Ok(shown), Ok(d)) = (s.view_doc(), s.doc())
    {
        out["shown"] = json!(shown.model.len());
        out["objects"] = json!(d.model.len());
    }
    out
}

fn run_open(s: &mut Session, p: &Value) -> Result<Value> {
    let sheet = match (p.get("sheet"), p.get("step").and_then(Value::as_str)) {
        (Some(v), _) => s.open_sheet(find(s, v)?)?,
        (None, Some(step)) => {
            let by = match step.trim().to_ascii_lowercase().as_str() {
                "next" => 1,
                "previous" | "prev" => -1,
                "first" => i64::MIN / 2,
                "last" => i64::MAX / 2,
                other => return Err(bad("sheet.open", format!("`step` is next, previous, first or last, not `{other}`"))),
            };
            s.step_sheet(by)?.ok_or_else(|| bad("sheet.open", "this drawing has no sheets: it isn't a print set"))?
        }
        (None, None) => return Err(bad("sheet.open", "`sheet` (a number such as \"Q-101\", or its place in the list from 1) or `step` is required")),
    };
    let mut out = listing(s);
    out["message"] = json!(format!("Sheet {} {}", sheet.number, sheet.title).trim().to_string());
    Ok(out)
}

fn run_step(s: &mut Session, by: i64) -> Result<Value> {
    if s.step_sheet(by)?.is_none() {
        return Ok(json!({ "sheets": [], "active": null, "message": "This drawing has no sheets." }));
    }
    run_open_message(s)
}

fn run_open_message(s: &Session) -> Result<Value> {
    let mut out = listing(s);
    if let Some((_, sh)) = s.active_sheet() {
        out["message"] = json!(format!("Sheet {} {}", sh.number, sh.title).trim().to_string());
    }
    Ok(out)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sheet.list", "List Sheets", |s, _| Ok(listing(s)))
            .params("{} → {sheets: [{index (from 1), number, title, rect: [x0,y0,x1,y1], active}], active: index | null, shown?: objects of model space the open sheet's view holds, objects?: all of them} (a print set's sheets in the set's order; empty for an ordinary drawing)")
            .noundo()
            .transparent(),
        CommandSpec::new("sheet.open", "Open Sheet", run_open)
            .params("{sheet?: number (\"Q-101\") | index from 1, step?: next|previous|first|last} → the sheet.list reply and a message (the view fits the sheet; only its objects are drawn, picked and snapped to; commands still read the whole drawing)")
            .noundo()
            .transparent(),
        CommandSpec::new("sheet.next", "Next Sheet", |s, _| run_step(s, 1)).params("{} → the sheet.list reply (Page Down)").noundo().transparent(),
        CommandSpec::new("sheet.previous", "Previous Sheet", |s, _| run_step(s, -1)).params("{} → the sheet.list reply (Page Up)").noundo().transparent(),
    ]
}
