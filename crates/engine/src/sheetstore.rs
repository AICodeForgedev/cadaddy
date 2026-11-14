//! A print set one sheet at a time, in memory too (Q6418).
//!
//! [`crate::sheetnav`] shows one sheet of a print set; this keeps the others out of memory. In
//! the interface ([`Session::sheets_on_demand`]) a print-set drawing holds its building model,
//! the open sheet and the [`CACHE`] most recent ones; every other sheet's objects stay as the
//! .cdy file stores them, a few compressed chunks a sheet ([`cadaddy_doc::sheetstore`]).
//! Opening a .cdy print set reads the sheet index, the building model and the first sheet
//! ([`crate::cmd::file::read_file_on_demand`]); opening another sheet decodes it (the app does
//! that on a worker thread first: [`Session::parked_sheet_of`], [`decode`], [`Session::apply_decoded`]).
//!
//! Nothing that reads the drawing sees less: a command that isn't one of [`SHEET_LOCAL`] (it may
//! read the whole set: a count, a takeoff, a job, Validate, an export, a plot) loads every sheet
//! first ([`Session::needs_whole_set`]; the app loads them on a worker thread with progress before
//! it runs the command). Saving to .cdy writes the parked sheets back byte for byte; saving to
//! any other format writes the whole set from a combined copy ([`combined`]), never loading it
//! into the drawing. Headless sessions (the CLI, the job pipeline, MCP servers) never park between
//! commands: a drawing there holds everything, as it always did. Inside one command, work done
//! reading the sheets may put them out for its rest ([`Session::put_out_sheets`]: a guided job
//! after its build, Q6577); they are back when the command ends.

use std::sync::Arc;

use std::collections::HashMap;

use cadaddy_doc::sheetstore::{ParkedSheet, SheetRect, StoredChunk};
use cadaddy_doc::{ChunkRef, Drawing, Entity, Handle};

use crate::{Result, Session};

/// One sheet change made while a command ran ([`crate::DocState`]'s `sheet_moves`, Q6570), object
/// by object: the objects it put out of memory, each by its address as it was, and the objects it
/// loaded. It holds no drawing state: a guided job that put its sheets out after its build kept
/// every one of them in memory through the state before (Q6577).
///
/// An object put out is only ever looked up as an object of the drawing the command started
/// from, which still holds it: while it lives no other object has its address.
#[derive(Clone, Default)]
pub(crate) struct SheetMove {
    out: HashMap<Handle, usize>,
    loaded: HashMap<Handle, Arc<Entity>>,
}

impl std::fmt::Debug for SheetMove {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SheetMove {{ out: {}, loaded: {} }}", self.out.len(), self.loaded.len())
    }
}

impl SheetMove {
    /// The change from `old` to `now`: what left model space and what came into it.
    fn between(old: &Drawing, now: &Drawing) -> Self {
        let (added, _, erased) = old.model.changes(&now.model);
        let out = erased.into_iter().filter_map(|h| old.model.get(h).map(|e| (h, Arc::as_ptr(e) as usize))).collect();
        let loaded = added.into_iter().filter_map(|h| now.model.get(h).map(|e| (h, Arc::clone(e)))).collect();
        Self { out, loaded }
    }

    /// Whether it put object `h`, as `e` (of the drawing the command started from), out of memory.
    pub(crate) fn put_out(&self, h: Handle, e: &Arc<Entity>) -> bool {
        self.out.get(&h).is_some_and(|at| *at == Arc::as_ptr(e) as usize)
    }

    /// Whether it loaded object `h` as `e`.
    pub(crate) fn loaded(&self, h: Handle, e: &Arc<Entity>) -> bool {
        self.loaded.get(&h).is_some_and(|x| Arc::ptr_eq(x, e))
    }
}

/// Sheets a print set keeps in memory in the interface: the open one and the most recent others.
pub const CACHE: usize = 3;

/// Most sheet changes one command remembers ([`crate::DocState`]'s `sheet_moves`): past it, a
/// further change counts as an edit of the plan (the model is built again, never left stale).
const MAX_SHEET_MOVES: usize = 64;

/// Chunks encoded at once when sheets go out of memory ([`park`]). Encoding one takes several times
/// its objects' bytes for a moment (a four-storey set's largest, 15 MB of objects, about 100 MB):
/// on every core at once it added about 0.8 GB at the moment a guided job is at its largest, its
/// build just done (Q6577). On two, that set's million objects go out in about the same time
/// the job's other steps take.
const ENCODE_THREADS: usize = 2;

/// How many of the latest whole-set loads a session remembers the command of ([`Session::whole_set_loads`]).
pub const LOADS_KEPT: usize = 32;

/// Commands that work on the open sheet alone (or on what is in memory) and so never need the
/// sheets out of memory: moving around the set and the view, drawing and editing what is
/// picked, undo, saving (a .cdy save keeps the parked sheets as stored), and the reads of the
/// building model that only look at its elements or the building spec (the model tree, trade
/// colours, the MEP list: an element always carries extended data, so it is never out of
/// memory), and the 3D view's own settings (direction, visual style, fidelity tier, the BIM view
/// and look: the view asks `bim.view` whenever the drawing changes, and loading the set changes
/// it, so a whole-set load there would never end). Any other command loads the whole set before
/// it runs.
pub const SHEET_LOCAL: &[&str] = &[
    "sheet.list",
    "sheet.open",
    "sheet.next",
    "sheet.previous",
    "document.list",
    "zoom",
    "pan",
    "pan.up",
    "pan.down",
    "pan.left",
    "pan.right",
    "redraw",
    "regen",
    "regenall",
    "qsave",
    "saveas",
    "undo",
    "u",
    "redo",
    "mredo",
    "oops",
    "line",
    "pline",
    "circle",
    "arc",
    "rectang",
    "text",
    "mtext",
    "erase",
    "move",
    "copy",
    "rotate",
    "scale",
    "mirror",
    "offset",
    "trim",
    "extend",
    "stretch",
    "fillet",
    "chamfer",
    "explode",
    "matchprop",
    "select",
    "list",
    "dist",
    "id",
    "properties",
    "close",
    "bim.tree",
    "bim.trades",
    "mep.list",
    "bim.view",
    "bim.look",
    "view.bim3d",
    "view.fidelity",
    "view.dir",
    "vpoint",
    "vscurrent",
    "view.fidelity.rules", // rules kept in the drawing, beside its objects
    // Every other command that only changes how the drawing is seen (Q6572): the view
    // direction and twist (PLAN, the preset views, the navigation wheel), the zoom and pan
    // forms of the menus, named views, the UCS and its icon, cameras (records beside the
    // objects), visual styles and display settings kept in the header, the current tile. A fit
    // fits what is in memory, as ZOOM Extents does: the open sheet. Not here: what makes a
    // saved object from the extents (layout viewports: VPORTS and its menu forms, -VPORTS in a
    // layout), renders, lights and materials (objects), tours (rooms on the plans).
    "zoom.realtime",
    "zoom.dynamic",
    "zoom.previous",
    "zoom.window",
    "zoom.in",
    "zoom.out",
    "zoom.all",
    "zoom.extents",
    "zoom.object",
    "-pan",
    "view.set",
    "view.get",
    "view",
    "view.top",
    "view.bottom",
    "view.left",
    "view.right",
    "view.front",
    "view.back",
    "view.swiso",
    "view.seiso",
    "view.neiso",
    "view.nwiso",
    "view.twist",
    "plan",
    "plan.world",
    "vs.2dwireframe",
    "vs.wireframe",
    "vs.hidden",
    "vs.realistic",
    "vs.shaded",
    "vs.shadededges",
    "3dorbit",
    "3dwalk",
    "3dfly",
    "navswheel",
    "navswheel.zoom",
    "navswheel.rewind",
    "navswheel.pan",
    "navswheel.orbit",
    "navswheel.center",
    "navswheel.walk",
    "navswheel.look",
    "navvcube",
    "vports.activate",
    "ucs",
    "ucs.origin",
    "ucs.3point",
    "ucs.z",
    "ucsman",
    "ucsicon",
    "ucsicon.on",
    "ucsicon.origin",
    "camera",
    "camera.list",
    "camera.set",
    "camera.delete",
    "camera.view",
    "attdisp",
    "attdisp.normal",
    "attdisp.on",
    "attdisp.off",
    "annovisible.current",
    "annovisible.all",
    "annovisible.toggle",
    // What palettes and dialogs read on their own while they are open, none of it sheet
    // linework: the session's own lists (history, activity, the agent's review log, action
    // macros, the calculator), what the drawing keeps beside its objects (the building spec and
    // its levels, tags and rules, RFIs and BCF issues, the job's state, the model's areas,
    // rules and clashes for Validate, parts, parameters, standards files), objects that carry extended data and so never leave memory (markups, IFC objects),
    // other files (Design Center, a publish list), and picking among what is on screen (Quick
    // Select, the selection filter: as `select`).
    "undohistory",
    "commandhistory",
    "activity",
    "review.log",
    "actions",
    "cal",
    "bspec.get",
    "levels.list",
    "tags.list",
    "project.rules.list",
    "rfi.list",
    "job.status",
    "job.worklist",
    "job.rules",
    "job.report",
    "part.list",
    "parameters",
    "standards",
    "markups",
    "ifc.tree",
    "bcf.list",
    "validate.areas",
    "validate.rules",
    "model.clashes",
    "adcenter",
    "publishlist",
    "qselect.info",
    "filter",
];

/// Whether command `id` (a name or alias) works without the sheets out of memory.
pub fn is_sheet_local(id: &str) -> bool {
    let id = crate::cmd::resolve_alias(id);
    SHEET_LOCAL.iter().any(|s| s.eq_ignore_ascii_case(&id))
}

/// Print sheets as the .cdy writer files objects under them.
pub fn rects_of(sheets: &[crate::sheetnav::PrintSheet]) -> Vec<SheetRect> {
    sheets.iter().map(|s| SheetRect { number: s.number.clone(), rect: s.rect }).collect()
}

/// `d` with its sheet list set to `sheets`, its print set's (what the .cdy writer files objects
/// under), when that differs from what it holds; `d` itself (shared) otherwise.
pub fn with_sheet_list(d: &Arc<Drawing>, sheets: &[crate::sheetnav::PrintSheet]) -> Arc<Drawing> {
    let rects = rects_of(sheets);
    if *d.sheet_store.sheets == rects {
        return d.clone();
    }
    let mut out = (**d).clone();
    out.sheet_store.sheets = Arc::new(rects);
    Arc::new(out)
}

/// Parked chunks decoded into objects (off the UI thread, perhaps), to put into the drawing
/// they came from with [`apply`].
#[derive(Default)]
pub struct Decoded {
    chunks: Vec<(Arc<StoredChunk>, Vec<Entity>)>,
    /// What couldn't be read.
    pub notes: Vec<String>,
}

impl Decoded {
    /// Objects decoded.
    pub fn objects(&self) -> usize {
        self.chunks.iter().map(|(_, e)| e.len()).sum()
    }
}

/// The objects of `sheets`' chunks (on all cores). A chunk that doesn't read is left out with a
/// note, as reading a damaged file leaves a part out.
pub fn decode(sheets: &[ParkedSheet]) -> Decoded {
    let chunks: Vec<(String, Arc<StoredChunk>)> = sheets.iter().flat_map(|p| p.chunks.iter().map(|c| (p.number.clone(), c.clone()))).collect();
    let one = |(number, c): &(String, Arc<StoredChunk>)| (c.clone(), cadaddy_io::cdy::decode_stored(c).map_err(|e| format!("sheet {number}: {e}")));
    #[cfg(not(target_arch = "wasm32"))]
    let read: Vec<_> = {
        use rayon::prelude::*;
        chunks.par_iter().map(one).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let read: Vec<_> = chunks.iter().map(one).collect();
    let mut out = Decoded::default();
    for (c, r) in read {
        match r {
            Ok((items, notes)) => {
                out.notes.extend(notes);
                out.chunks.push((c, items));
            }
            Err(e) => {
                out.notes.push(format!("{e}: its objects were left out"));
                // Left out like a damaged part: it no longer waits as parked.
                out.chunks.push((c, Vec::new()));
            }
        }
    }
    out
}

/// Put decoded chunks into `d`: each one still parked there becomes a chunk of model space
/// (remembered as decoded from its stored chunk, so a save writes it as stored while it is
/// unchanged) and is no longer parked. Chunks `d` no longer parks (the drawing changed meanwhile)
/// are skipped. The objects put in.
pub fn apply(d: &mut Drawing, dec: Decoded) -> usize {
    if dec.chunks.is_empty() {
        return 0;
    }
    let mut parked = (*d.sheet_store.parked).clone();
    let mut origins = (*d.sheet_store.origins).clone();
    let mut origin_sheets = (*d.sheet_store.origin_sheets).clone();
    let mut put = 0usize;
    for (c, items) in dec.chunks {
        let Some(p) = parked.iter_mut().find(|p| p.chunks.iter().any(|x| Arc::ptr_eq(x, &c))) else { continue };
        p.chunks.retain(|x| !Arc::ptr_eq(x, &c));
        let sheet = SheetRect { number: p.number.clone(), rect: p.rect };
        put = put.saturating_add(items.len());
        if items.is_empty() {
            continue;
        }
        // Back where it was in the draw order (else on top).
        let r = match d.model.insert_chunk_at(c.order, items) {
            Ok(r) => Some(r),
            Err(items) => d.model.push_one_chunk(items),
        };
        if let Some(r) = r {
            origin_sheets.insert(r.key(), sheet);
            origins.insert(r.key(), (r, c));
        }
    }
    parked.retain(|p| !p.chunks.is_empty());
    d.sheet_store.parked = Arc::new(parked);
    d.sheet_store.origins = Arc::new(origins);
    d.sheet_store.origin_sheets = Arc::new(origin_sheets);
    put
}

/// Load every parked sheet `pick` picks into `d`. Notes about what couldn't be read.
pub fn load(d: &mut Drawing, pick: impl Fn(&ParkedSheet) -> bool) -> Vec<String> {
    let wanted: Vec<ParkedSheet> = d.sheet_store.parked.iter().filter(|p| pick(p)).cloned().collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let dec = decode(&wanted);
    let notes = dec.notes.clone();
    apply(d, dec);
    notes
}

/// `d` with every sheet in memory, for work on the whole set that must not load it into the
/// drawing being worked on (a DWG or DXF save of a print set open one sheet at a time). `d`
/// itself (shared) when nothing is parked.
pub fn combined(d: &Arc<Drawing>) -> Arc<Drawing> {
    if !d.sheet_store.has_parked() {
        return d.clone();
    }
    let mut out = (**d).clone();
    load(&mut out, |_| true);
    out.sheet_store = Default::default();
    Arc::new(out)
}

/// What to put out of memory: per chunk of model space, the runs of its objects on one sheet
/// that isn't kept (`(sheet, from, to)`, [`cadaddy_io::cdy::chunk_sheet`]'s runs), with whether
/// they are the whole chunk.
fn to_park(d: &Drawing, sheets: &[SheetRect], keep: &[SheetRect]) -> Vec<(ChunkRef, Vec<(usize, usize, usize, u32)>, bool)> {
    let slack = cadaddy_io::cdy::sheet_slack(sheets);
    let refs = d.model.chunk_refs();
    // A chunk decoded from a sheet's and unchanged since is that sheet's, whole: no need to
    // measure its objects again (after a whole-set command, that is nearly all of them).
    let one = |c: &ChunkRef| match d.sheet_store.origin_of(c).and(d.sheet_store.origin_sheets.get(&c.key())) {
        Some(s) => match sheets.iter().position(|x| x.number.eq_ignore_ascii_case(&s.number) && cadaddy_doc::sheetstore::same_rect(&x.rect, &s.rect))
        {
            Some(k) => vec![(u32::try_from(k).ok(), 0, c.items().len())],
            None => cadaddy_io::cdy::sheet_pieces(d, &slack, c.items()),
        },
        None => cadaddy_io::cdy::sheet_pieces(d, &slack, c.items()),
    };
    #[cfg(not(target_arch = "wasm32"))]
    let filed: Vec<Vec<(Option<u32>, usize, usize)>> = {
        use rayon::prelude::*;
        refs.par_iter().map(one).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let filed: Vec<Vec<(Option<u32>, usize, usize)>> = refs.iter().map(one).collect();
    let kept = |k: usize| {
        sheets
            .get(k)
            .is_none_or(|s| keep.iter().any(|x| x.number.eq_ignore_ascii_case(&s.number) && cadaddy_doc::sheetstore::same_rect(&x.rect, &s.rect)))
    };
    // The draw-order places taken: by the chunks in memory and the ones out already.
    let orders: Vec<u32> = refs.iter().map(ChunkRef::order).collect();
    let parked: std::collections::HashSet<u32> = d.sheet_store.parked.iter().flat_map(|p| p.chunks.iter().map(|c| c.order)).collect();
    refs.into_iter()
        .zip(filed)
        .enumerate()
        .filter_map(|(i, (c, runs))| {
            let whole = runs.len() == 1;
            let at = c.order();
            // Each run goes back after what is left of its chunk: at the chunk's place, plus one
            // and its own place among the runs (fewer than a chunk step). The runs before the
            // chunk's first object that stays go back just before it instead, when those places
            // are free, so the objects come back in the order they had (Q6577: a sheet's last
            // run and the first elements built after it shared a chunk, and came back the other
            // way round; what reads the plan in order read it otherwise).
            let lead = runs.iter().position(|(k, _, _)| k.is_none_or(|k| kept(k as usize))).unwrap_or(0);
            let lead_at = u32::try_from(lead).ok().and_then(|n| at.checked_sub(n)).filter(|lo| {
                lead > 0 && i.checked_sub(1).and_then(|p| orders.get(p)).is_none_or(|p| p < lo) && !(*lo..at).any(|x| parked.contains(&x))
            });
            let out: Vec<(usize, usize, usize, u32)> = runs
                .into_iter()
                .enumerate()
                .filter_map(|(j, (k, from, to))| {
                    let order = match lead_at {
                        _ if whole => at,
                        Some(lo) if j < lead => lo.saturating_add(u32::try_from(j).ok()?),
                        _ => at.saturating_add(1).saturating_add(u32::try_from(j).ok()?),
                    };
                    Some((k? as usize, from, to, order))
                })
                .filter(|(k, _, _, _)| !kept(*k))
                .collect();
            (!out.is_empty()).then_some((c, out, whole))
        })
        .collect()
}

/// Put every sheet of `d`'s print set out of memory but those in `keep`: the objects of each
/// such sheet leave model space and are parked as stored. A chunk holding only one sheet's
/// objects goes whole (as it was read when it is unchanged since, else encoded now); the runs of
/// a chunk holding the end of one sheet and the start of the next go as chunks of their own.
/// The objects parked.
pub fn park(d: &mut Drawing, sheets: &[SheetRect], keep: &[SheetRect]) -> Result<usize> {
    park_with(d, sheets, keep, &mut Encoded::new())
}

/// Runs of chunks encoded by [`park_with`], by (chunk key, from, to, draw order): drawings that
/// share a chunk (a drawing and its undo steps) park it as one stored chunk, encoded once. The
/// keys name chunks only while they are held: a map lives no longer than the drawings it parked.
type Encoded = HashMap<(usize, usize, usize, u32), Arc<StoredChunk>>;

/// Drawings parked by [`park_shared`], by the address of the drawing each replaces (shared by
/// several holders: the drawing, its undo steps, the drawing as saved, a build's record). The
/// addresses name drawings only while the caller keeps the replaced ones.
type Shared = HashMap<usize, Arc<Drawing>>;

/// `d` with the sheets not in `keep` out of memory, parked once however many hold it (`done`):
/// `d` itself when it has none in memory (or couldn't be encoded).
fn park_shared(d: &Arc<Drawing>, rects: &[SheetRect], keep: &[SheetRect], encoded: &mut Encoded, done: &mut Shared) -> Arc<Drawing> {
    let at = Arc::as_ptr(d) as usize;
    if let Some(now) = done.get(&at) {
        return Arc::clone(now);
    }
    let mut c = (**d).clone();
    let now = match park_with(&mut c, rects, keep, encoded) {
        Ok(n) if n > 0 => Arc::new(c),
        _ => Arc::clone(d),
    };
    done.insert(at, Arc::clone(&now));
    now
}

/// [`park`], with what parking other drawings sharing `d`'s chunks encoded already.
fn park_with(d: &mut Drawing, sheets: &[SheetRect], keep: &[SheetRect], encoded: &mut Encoded) -> Result<usize> {
    let sheets = sheets.to_vec();
    if sheets.is_empty() {
        return Ok(0);
    }
    let picked = to_park(d, &sheets, keep);
    if picked.is_empty() {
        d.sheet_store.sheets = Arc::new(sheets);
        return Ok(0);
    }
    // Encoded first (on all cores), so a failure leaves the drawing as it was.
    let jobs: Vec<(usize, ChunkRef, usize, usize, bool, u32)> =
        picked.iter().flat_map(|(c, runs, whole)| runs.iter().map(move |(k, from, to, order)| (*k, c.clone(), *from, *to, *whole, *order))).collect();
    let done: &Encoded = encoded;
    let one = |(_, c, from, to, whole, order): &(usize, ChunkRef, usize, usize, bool, u32)| -> std::result::Result<Arc<StoredChunk>, String> {
        match d.sheet_store.origin_of(c).filter(|s| *whole && s.order == *order) {
            Some(s) => Ok(s.clone()),
            None => match done.get(&(c.key(), *from, *to, *order)) {
                Some(s) => Ok(s.clone()),
                None => cadaddy_io::cdy::encode_stored(c.items().get(*from..*to).unwrap_or_default(), *order).map(Arc::new),
            },
        }
    };
    // On a few cores only: encoding a chunk takes several times its objects' bytes for a moment.
    #[cfg(not(target_arch = "wasm32"))]
    let stored: Vec<_> = {
        use rayon::prelude::*;
        match rayon::ThreadPoolBuilder::new().num_threads(ENCODE_THREADS).build() {
            Ok(pool) => pool.install(|| jobs.par_iter().map(one).collect()),
            Err(_) => jobs.iter().map(one).collect(),
        }
    };
    #[cfg(target_arch = "wasm32")]
    let stored: Vec<_> = jobs.iter().map(one).collect();
    let stored: Vec<Arc<StoredChunk>> = stored.into_iter().collect::<std::result::Result<_, _>>().map_err(crate::EngineError::Other)?;
    for ((_, c, from, to, _, order), s) in jobs.iter().zip(&stored) {
        encoded.entry((c.key(), *from, *to, *order)).or_insert_with(|| s.clone());
    }
    // Whole chunks leave the store whole; the runs of the others object by object.
    let whole: std::collections::HashSet<usize> = picked.iter().filter(|(_, _, w)| *w).map(|(c, _, _)| c.key()).collect();
    let taken = d.model.take_chunks(|c| whole.contains(&c.key()));
    let mut parked = (*d.sheet_store.parked).clone();
    let mut n = 0usize;
    for ((k, c, from, to, is_whole, _), s) in jobs.into_iter().zip(stored) {
        if is_whole && !taken.iter().any(|t| t.same(&c)) {
            continue;
        }
        if !is_whole {
            for e in c.items().get(from..to).unwrap_or_default() {
                d.model.remove(e.handle);
            }
        }
        let Some(sheet) = sheets.get(k) else { continue };
        n = n.saturating_add(s.objects);
        match parked.iter_mut().find(|p| p.is(&sheet.number, &sheet.rect)) {
            Some(p) => p.chunks.push(s),
            None => parked.push(ParkedSheet { number: sheet.number.clone(), rect: sheet.rect, chunks: vec![s] }),
        }
    }
    d.sheet_store.parked = Arc::new(parked);
    d.sheet_store.sheets = Arc::new(sheets);
    // The handle index sized for what is left (it was split for the whole set).
    d.model.fit_index();
    let live = d.model.chunk_refs();
    d.sheet_store.prune_origins(&live);
    Ok(n)
}

impl Session {
    /// Whether the active drawing has sheets out of memory.
    pub fn has_parked_sheets(&self) -> bool {
        self.state().is_ok_and(|st| st.doc.sheet_store.has_parked())
    }

    /// The outermost command is over: sheets its work held out of memory
    /// ([`Session::sheets_stay_out`]) are held out no longer, and a headless session has them back
    /// in memory (its drawings hold everything between commands: what reads them there, an export
    /// or an agent's selection, never asks for a sheet).
    pub(crate) fn sheets_back_after_command(&mut self) {
        self.sheets_stay_out = false;
        if self.sheets_on_demand || !self.has_parked_sheets() {
            return;
        }
        if let Err(e) = self.load_all_sheets() {
            self.echo(format!("the print sheets couldn't be loaded back: {e}"));
        }
    }

    /// Whether running command `id` must load the sheets out of memory first: it may read the
    /// whole set ([`SHEET_LOCAL`]). Never while work done with the sheets holds them out
    /// ([`Session::sheets_stay_out`]: a guided job's stages after its build read the model).
    pub fn needs_whole_set(&self, id: &str) -> bool {
        self.has_parked_sheets() && !is_sheet_local(id) && !self.sheets_stay_out
    }

    /// Change the active drawing's sheets in memory with `f`: not an edit (no undo step; a
    /// drawing without unsaved changes still has none). Only outside the Block Editor.
    fn change_sheets<R>(&mut self, f: impl FnOnce(&mut Drawing) -> R) -> Result<Option<R>> {
        // Inside a command (or while one waits for input) the change is remembered, so the
        // command's check for plan edits leaves it out (Q6570).
        let in_command = self.journal.in_call() || self.running.is_some() || self.suspended.is_some();
        let st = self.state_mut()?;
        if st.block_edit.is_some() || st.block_test.is_some() {
            return Ok(None);
        }
        let clean = Arc::ptr_eq(&st.doc, &st.saved);
        let old = Arc::clone(&st.doc);
        let r = f(Arc::make_mut(&mut st.doc));
        if clean {
            st.saved = st.doc.clone();
        }
        if !Arc::ptr_eq(&old, &st.doc) {
            // No edit of the plan or the model: the new state is behind its model as the old
            // one was, and comes from the same build (Q6570).
            let now = Arc::clone(&st.doc);
            crate::cmd::wallgen::carry_state(st, &old, &now);
            if in_command && st.sheet_moves.len() < MAX_SHEET_MOVES {
                st.sheet_moves.push(SheetMove::between(&old, &now));
            }
        }
        // Selected objects that left memory are no longer selected.
        let doc = st.doc.clone();
        st.selection.retain(|h| doc.model.contains(*h) || doc.layouts.iter().any(|l| l.entities.contains(*h)));
        Ok(Some(r))
    }

    /// Load every sheet out of memory into the active drawing (on this thread). The objects
    /// loaded; what couldn't be read is said.
    pub fn load_all_sheets(&mut self) -> Result<usize> {
        if !self.has_parked_sheets() {
            return Ok(0);
        }
        let before = self.state()?.doc.model.len();
        let notes = self.change_sheets(|d| load(d, |_| true))?.unwrap_or_default();
        // The interface puts them out again once it is idle ([`Session::whole_set_in_memory`]).
        self.whole_set_loaded = self.sheets_on_demand;
        for n in notes {
            self.echo(n);
        }
        Ok(self.state()?.doc.model.len().saturating_sub(before))
    }

    /// With sheets on demand: a command that reads the whole set is about to run (whether or not
    /// the set had to be loaded for it). The interface keeps the set in memory while such
    /// commands keep coming ([`Session::whole_set_asks`]).
    pub(crate) fn note_whole_set_ask(&mut self, id: &str) {
        if self.sheets_on_demand && !is_sheet_local(id) {
            self.whole_set_asks = self.whole_set_asks.wrapping_add(1);
            if self.whole_set_asked.len() < LOADS_KEPT && !self.whole_set_asked.iter().any(|a| a.eq_ignore_ascii_case(id)) {
                self.whole_set_asked.push(id.to_string());
            }
        }
    }

    /// A command loaded the whole set as it ran: counted, its name kept.
    pub(crate) fn note_whole_set_load(&mut self, id: &str) {
        let (n, last) = &mut self.whole_set_loads;
        *n += 1;
        if last.len() >= LOADS_KEPT {
            last.remove(0);
        }
        last.push(id.to_string());
    }

    /// The parked part of sheet `i` of the print set (`None` when it is in memory): what the app
    /// decodes on a worker thread before opening the sheet. `None` for `i` = `None` means every
    /// parked sheet.
    pub fn parked_sheet_of(&self, i: Option<usize>) -> Vec<ParkedSheet> {
        let Ok(st) = self.state() else { return Vec::new() };
        let parked = &st.doc.sheet_store.parked;
        match i {
            None => parked.iter().filter(|p| !p.chunks.is_empty()).cloned().collect(),
            Some(i) => {
                let sheets = self.print_sheets();
                let Some(s) = sheets.get(i) else { return Vec::new() };
                st.doc.sheet_store.parked_sheet(&s.number, &s.rect).cloned().into_iter().collect()
            }
        }
    }

    /// Put chunks decoded off the UI thread ([`decode`]) into the active drawing. The objects put in.
    pub fn apply_decoded(&mut self, dec: Decoded) -> Result<usize> {
        let notes = dec.notes.clone();
        let n = self.change_sheets(|d| apply(d, dec))?.unwrap_or(0);
        for note in notes {
            self.echo(note);
        }
        Ok(n)
    }

    /// With sheets on demand: whether a command that read the whole set loaded every sheet and
    /// they haven't gone out of memory since ([`Session::park_idle_sheets`] puts them out).
    pub fn whole_set_in_memory(&self) -> bool {
        self.whole_set_loaded && self.sheets_on_demand
    }

    /// With sheets on demand and nothing running: every sheet but the open one and the [`CACHE`]
    /// most recent ones out of memory again (after a command that read the whole set), from the
    /// drawing and from its undo and redo steps, and what process-wide caches kept of the set
    /// let go of ([`Session::release_unheld`]): the program holds about what it held before the
    /// load (Q6571). True when it did that.
    pub fn park_idle_sheets(&mut self) -> Result<bool> {
        let done = self.park_idle_sheets_off_thread()?;
        if done {
            self.release_unheld();
        }
        Ok(done)
    }

    /// [`Session::park_idle_sheets`] on a worker thread while a copy of the session is on screen
    /// ([`Session::view_copy`]): the caches are left as they are, since the copy's drawings still
    /// hold what goes out (letting go of it now would only have the copy's frames work it out
    /// again). The caller calls [`Session::release_unheld`] once the copy is gone.
    pub fn park_idle_sheets_off_thread(&mut self) -> Result<bool> {
        if !self.whole_set_in_memory() || self.running.is_some() || self.suspended.is_some() {
            return Ok(false);
        }
        let Some((i, _)) = self.active_sheet() else {
            self.whole_set_loaded = false;
            return Ok(false);
        };
        let (rects, keep) = self.keep_open_sheets_of(i)?;
        if !rects.is_empty() {
            self.park_history(&rects, &keep, &mut Encoded::new(), Shared::new());
        }
        Ok(true)
    }

    /// Every sheet of the print set out of memory but the ones on screen (with sheets on demand:
    /// the open one and the [`CACHE`] most recent ones; none in a headless session): from the
    /// drawing, its undo and redo steps, the drawing as saved and the drawings its build records
    /// keep, and what the process-wide caches held of them let go of. For work that is done
    /// reading the sheets: a guided job once its build has read them (Q6577: the job kept every
    /// sheet in memory to its end, about 1.1 GB on a four-storey set). A later command that may
    /// read the whole set loads it again ([`Session::needs_whole_set`]). The objects put out of
    /// the drawing (0: nothing was in memory to put out, and nothing else was done).
    pub fn put_out_sheets(&mut self) -> Result<usize> {
        let sheets = self.print_sheets();
        if sheets.is_empty() {
            return Ok(0);
        }
        let rects = rects_of(&sheets);
        let keep: Vec<SheetRect> = if self.sheets_on_demand {
            let recent = self.state()?.sheet_nav.recent.clone();
            recent.iter().filter_map(|i| sheets.get(*i)).map(|s| SheetRect { number: s.number.clone(), rect: s.rect }).collect()
        } else {
            Vec::new()
        };
        // The build's record of the plan, while the whole set is still in memory.
        let plan = crate::cmd::buildrec::plan_before_sheets_out(self);
        // The drawing as it was stays held until everything sharing its chunks has gone out:
        // `encoded` names chunks by address.
        let was = Arc::clone(&self.state()?.doc);
        let mut encoded = Encoded::new();
        let n = match self.change_sheets(|d| park_with(d, &rects, &keep, &mut encoded))? {
            Some(r) => r?,
            None => return Ok(0),
        };
        if n == 0 {
            return Ok(0);
        }
        crate::cmd::buildrec::plan_after_sheets_out(self, plan);
        let mut done = Shared::new();
        done.insert(Arc::as_ptr(&was) as usize, Arc::clone(&self.state()?.doc));
        self.park_history(&rects, &keep, &mut encoded, done);
        drop(was);
        self.whole_set_loaded = false;
        self.release_unheld();
        Ok(n)
    }

    /// The undo and redo steps of the active drawing with the sheets not in `keep` out of memory,
    /// as the drawing has them: a step recorded while the whole set was in memory (an edit just
    /// after a count) held every sheet of it, and undo counts a step's cost by what it doesn't
    /// share with the drawing, so it never saw it once the drawing let the sheets go. A step
    /// whose drawing was the saved one is still the saved one after (no change to save). The
    /// drawing as saved and the drawings the build records keep (as their builds left them) go
    /// the same way (Q6577: a job's build record kept the whole set). Each drawing state let go
    /// of is followed by the one that replaces it (behind its model, from its build, as it was).
    /// A drawing that doesn't park (it couldn't be encoded) stays as it was. `done` holds the
    /// drawings parked already (by the address of the drawing they replace, which the caller
    /// keeps alive), `encoded` their chunks.
    fn park_history(&mut self, rects: &[SheetRect], keep: &[SheetRect], encoded: &mut Encoded, mut done: Shared) {
        let Ok(st) = self.state_mut() else { return };
        if st.block_edit.is_some() || st.block_test.is_some() {
            return;
        }
        // The drawings let go of, dropped only at the end: `encoded` names chunks by address.
        let mut old: Vec<Arc<Drawing>> = Vec::new();
        // Undo steps whose cost is no longer what was recorded: the step parked, or the one after it.
        let mut costed = vec![false; st.undo.len()];
        // The drawing itself just let its sheets go: the last step's newer drawing changed.
        if let Some(c) = costed.last_mut() {
            *c = true;
        }
        for (i, snap) in st.undo.iter_mut().chain(st.redo.iter_mut()).enumerate() {
            let now = park_shared(&snap.doc, rects, keep, encoded, &mut done);
            if Arc::ptr_eq(&now, &snap.doc) {
                continue;
            }
            // Picked objects that left memory are no longer picked (as the drawing's own).
            snap.selection.retain(|h| now.model.contains(*h) || now.layouts.iter().any(|l| l.entities.contains(*h)));
            old.push(std::mem::replace(&mut snap.doc, now));
            for k in [Some(i), i.checked_sub(1)].into_iter().flatten() {
                if let Some(c) = costed.get_mut(k) {
                    *c = true;
                }
            }
        }
        if !Arc::ptr_eq(&st.saved, &st.doc) {
            let now = park_shared(&st.saved, rects, keep, encoded, &mut done);
            if !Arc::ptr_eq(&now, &st.saved) {
                old.push(std::mem::replace(&mut st.saved, now));
            }
        }
        for built in st.model_records.built_mut() {
            let now = park_shared(built, rects, keep, encoded, &mut done);
            if !Arc::ptr_eq(&now, built) {
                old.push(std::mem::replace(built, now));
            }
        }
        for was in &old {
            if let Some(now) = done.get(&(Arc::as_ptr(was) as usize)).cloned() {
                crate::cmd::wallgen::carry_state(st, was, &now);
            }
        }
        // What a parked step holds by itself (its own handle index once its sheets went out),
        // for the undo budget ([`crate::undoctl`]): what it doesn't share with the drawing after it.
        for (i, c) in costed.iter().enumerate() {
            if !c {
                continue;
            }
            let newer = st.undo.get(i + 1).map_or(&st.doc, |x| &x.doc).clone();
            if let Some(snap) = st.undo.get_mut(i) {
                snap.bytes = snap.doc.unique_bytes(&newer);
            }
        }
        drop(old);
    }

    /// Let go of what process-wide caches (the spatial indexes, the element and plan object
    /// indexes) hold of objects no open drawing holds any more: of every open drawing, what it
    /// holds now, as saved, and in its undo and redo steps (and a drawing open in the Block
    /// Editor or the block test) stays. After a print set's whole-set load went out of memory
    /// again, they held the whole set (Q6571).
    pub fn release_unheld(&self) {
        let mut states: Vec<&crate::DocState> = self.docs.iter().collect();
        states.extend(self.docs.iter().filter_map(|st| st.block_test.as_deref()));
        states.extend(self.docs.iter().filter_map(|st| st.block_edit.as_ref().map(|b| &*b.host)));
        let mut drawings: Vec<&Arc<Drawing>> = Vec::new();
        for st in states {
            drawings.push(&st.doc);
            drawings.push(&st.saved);
            drawings.extend(st.undo.iter().chain(st.redo.iter()).map(|x| &x.doc));
        }
        drawings.extend(self.docs.iter().filter_map(|st| st.block_edit.as_ref()).map(|b| &b.saved));
        // Chunk keys for the per-chunk pools, the spaces' stores for the spatial indexes.
        let mut held = std::collections::HashSet::new();
        let mut spaces: Vec<&cadaddy_doc::EntityStore> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for d in drawings {
            if !seen.insert(Arc::as_ptr(d) as usize) {
                continue;
            }
            let space = std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities));
            for st in space.clone().chain(d.blocks.values().map(|b| &b.entities)) {
                held.extend(st.chunk_slices().map(|(k, _)| k));
            }
            spaces.extend(space);
        }
        crate::spatial::retain_held(&spaces);
        crate::cmd::elemprops::retain_held(&held);
        crate::cmd::overrides::retain_held(&held);
    }

    /// With sheets on demand: the open sheet in memory (loaded now when it is out), and every
    /// sheet but it and the [`CACHE`] most recent ones out. Nothing otherwise.
    pub(crate) fn keep_open_sheets(&mut self, opened: usize) -> Result<()> {
        self.keep_open_sheets_of(opened).map(|_| ())
    }

    /// [`Session::keep_open_sheets`]: the print set's sheets and those kept in memory (none
    /// without sheets on demand).
    fn keep_open_sheets_of(&mut self, opened: usize) -> Result<(Vec<SheetRect>, Vec<SheetRect>)> {
        if !self.sheets_on_demand {
            return Ok((Vec::new(), Vec::new()));
        }
        self.whole_set_loaded = false;
        let sheets = self.print_sheets();
        let rects = rects_of(&sheets);
        let recent = {
            let st = self.state_mut()?;
            let r = &mut st.sheet_nav.recent;
            r.retain(|i| *i != opened);
            r.insert(0, opened);
            r.truncate(CACHE);
            r.clone()
        };
        let keep: Vec<SheetRect> =
            recent.iter().filter_map(|i| sheets.get(*i)).map(|s| SheetRect { number: s.number.clone(), rect: s.rect }).collect();
        let notes = match self.change_sheets(|d| {
            let notes = load(d, |p| keep.iter().any(|k| p.is(&k.number, &k.rect)));
            park(d, &rects, &keep).map(|_| notes)
        })? {
            Some(r) => r?,
            None => Vec::new(),
        };
        for n in notes {
            self.echo(n);
        }
        Ok((rects, keep))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, EntityKind, Line, RawTag, Space};
    use cadaddy_geom::{Bounds2, Vec2, Vec3};

    /// Q6577: a sheet's last objects and the first elements built after them share a chunk; put
    /// out of memory and loaded again, every object comes back in the order it had (the sheet's
    /// run went back after the elements, and what reads the plan in order read it otherwise).
    #[test]
    fn objects_put_out_and_loaded_again_keep_their_order() {
        let mut d = Drawing::default();
        let line = |x: f64| EntityKind::Line(Line { a: Vec3::new(x, 1.0, 0.0), b: Vec3::new(x + 0.5, 2.0, 0.0) });
        // The sheet's first objects fill a chunk of their own; its last ones start the next.
        for i in 0..1064 {
            d.add(&Space::Model, Common::default(), line(f64::from(i % 90))).unwrap();
        }
        // Built elements (they carry extended data: never a sheet's) right after, on the sheet.
        let element = Common {
            xdata: vec![RawTag { code: 1001, value: "CADADDY_PROPS".into() }, RawTag { code: 1000, value: "{}".into() }].into(),
            ..Default::default()
        };
        for i in 0..5 {
            d.add(&Space::Model, element.clone(), line(f64::from(i) + 0.25)).unwrap();
        }
        assert_eq!(d.model.chunk_refs().len(), 2, "the sheet's last objects and the elements share a chunk");
        let order = |d: &Drawing| d.model.iter().map(|e| e.handle).collect::<Vec<_>>();
        let before = order(&d);
        let sheets = vec![SheetRect { number: "Z-901".into(), rect: Bounds2::new(Vec2::new(-10.0, -10.0), Vec2::new(100.0, 100.0)) }];
        assert_eq!(park(&mut d, &sheets, &[]).unwrap(), 1064);
        assert_eq!(d.model.len(), 5, "the elements stay in memory");
        assert!(load(&mut d, |_| true).is_empty());
        assert_eq!(order(&d), before, "the objects came back in another order");
    }
}
