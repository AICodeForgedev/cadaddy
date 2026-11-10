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
//! into the drawing. Headless sessions (the CLI, the job pipeline, MCP servers) never park: a
//! drawing there holds everything, as it always did.

use std::sync::Arc;

use cadaddy_doc::sheetstore::{ParkedSheet, SheetRect, StoredChunk};
use cadaddy_doc::{ChunkRef, Drawing, Entity};

use crate::{Result, Session};

/// Sheets a print set keeps in memory in the interface: the open one and the most recent others.
pub const CACHE: usize = 3;

/// Commands that work on the open sheet alone (or on what is in memory) and so never need the
/// sheets out of memory: moving around the set and the view, drawing and editing what is
/// picked, undo, and saving (a .cdy save keeps the parked sheets as stored). Any other command
/// loads the whole set before it runs.
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
    let mut put = 0usize;
    for (c, items) in dec.chunks {
        let Some(p) = parked.iter_mut().find(|p| p.chunks.iter().any(|x| Arc::ptr_eq(x, &c))) else { continue };
        p.chunks.retain(|x| !Arc::ptr_eq(x, &c));
        put = put.saturating_add(items.len());
        if items.is_empty() {
            continue;
        }
        if let Some(r) = d.model.push_one_chunk(items) {
            origins.insert(r.key(), (r, c));
        }
    }
    parked.retain(|p| !p.chunks.is_empty());
    d.sheet_store.parked = Arc::new(parked);
    d.sheet_store.origins = Arc::new(origins);
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
fn to_park(d: &Drawing, sheets: &[SheetRect], keep: &[SheetRect]) -> Vec<(ChunkRef, Vec<(usize, usize, usize)>, bool)> {
    let slack = cadaddy_io::cdy::sheet_slack(sheets);
    let refs = d.model.chunk_refs();
    let one = |c: &ChunkRef| cadaddy_io::cdy::sheet_pieces(d, &slack, c.items());
    #[cfg(not(target_arch = "wasm32"))]
    let filed: Vec<Vec<(Option<u32>, usize, usize)>> = {
        use rayon::prelude::*;
        refs.par_iter().map(one).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let filed: Vec<Vec<(Option<u32>, usize, usize)>> = refs.iter().map(one).collect();
    let kept = |k: usize| sheets.get(k).is_none_or(|s| keep.iter().any(|x| x.number.eq_ignore_ascii_case(&s.number) && cadaddy_doc::sheetstore::same_rect(&x.rect, &s.rect)));
    refs.into_iter()
        .zip(filed)
        .filter_map(|(c, runs)| {
            let whole = runs.len() == 1;
            let out: Vec<(usize, usize, usize)> = runs.into_iter().filter_map(|(k, from, to)| Some((k? as usize, from, to))).filter(|(k, _, _)| !kept(*k)).collect();
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
    let jobs: Vec<(usize, ChunkRef, usize, usize, bool)> =
        picked.iter().flat_map(|(c, runs, whole)| runs.iter().map(move |(k, from, to)| (*k, c.clone(), *from, *to, *whole))).collect();
    let one = |(_, c, from, to, whole): &(usize, ChunkRef, usize, usize, bool)| -> std::result::Result<Arc<StoredChunk>, String> {
        match d.sheet_store.origin_of(c).filter(|_| *whole) {
            Some(s) => Ok(s.clone()),
            None => cadaddy_io::cdy::encode_stored(c.items().get(*from..*to).unwrap_or_default()).map(Arc::new),
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let stored: Vec<_> = {
        use rayon::prelude::*;
        jobs.par_iter().map(one).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let stored: Vec<_> = jobs.iter().map(one).collect();
    let stored: Vec<Arc<StoredChunk>> = stored.into_iter().collect::<std::result::Result<_, _>>().map_err(crate::EngineError::Other)?;
    // Whole chunks leave the store whole; the runs of the others object by object.
    let whole: std::collections::HashSet<usize> = picked.iter().filter(|(_, _, w)| *w).map(|(c, _, _)| c.key()).collect();
    let taken = d.model.take_chunks(|c| whole.contains(&c.key()));
    let mut parked = (*d.sheet_store.parked).clone();
    let mut n = 0usize;
    for ((k, c, from, to, is_whole), s) in jobs.into_iter().zip(stored) {
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
    let live = d.model.chunk_refs();
    d.sheet_store.prune_origins(&live);
    Ok(n)
}

impl Session {
    /// Whether the active drawing has sheets out of memory.
    pub fn has_parked_sheets(&self) -> bool {
        self.state().is_ok_and(|st| st.doc.sheet_store.has_parked())
    }

    /// Whether running command `id` must load the sheets out of memory first: it may read the
    /// whole set ([`SHEET_LOCAL`]).
    pub fn needs_whole_set(&self, id: &str) -> bool {
        self.has_parked_sheets() && !is_sheet_local(id)
    }

    /// Change the active drawing's sheets in memory with `f`: not an edit (no undo step; a
    /// drawing without unsaved changes still has none). Only outside the Block Editor.
    fn change_sheets<R>(&mut self, f: impl FnOnce(&mut Drawing) -> R) -> Result<Option<R>> {
        let st = self.state_mut()?;
        if st.block_edit.is_some() || st.block_test.is_some() {
            return Ok(None);
        }
        let clean = Arc::ptr_eq(&st.doc, &st.saved);
        let r = f(Arc::make_mut(&mut st.doc));
        if clean {
            st.saved = st.doc.clone();
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
        for n in notes {
            self.echo(n);
        }
        Ok(self.state()?.doc.model.len().saturating_sub(before))
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

    /// With sheets on demand: the open sheet in memory (loaded now when it is out), and every
    /// sheet but it and the [`CACHE`] most recent ones out. Nothing otherwise.
    pub(crate) fn keep_open_sheets(&mut self, opened: usize) -> Result<()> {
        if !self.sheets_on_demand {
            return Ok(());
        }
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
        let keep: Vec<SheetRect> = recent.iter().filter_map(|i| sheets.get(*i)).map(|s| SheetRect { number: s.number.clone(), rect: s.rect }).collect();
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
        Ok(())
    }
}
