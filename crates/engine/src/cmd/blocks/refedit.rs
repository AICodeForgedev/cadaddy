//! REFEDIT, REFSET and REFCLOSE: edit a block reference in place.
//!
//! REFEDIT hides the picked reference and puts the block's objects (all, or the ones named) into
//! the drawing where the reference shows them: the *working set*, ordinary objects that every
//! command edits. The rest of the block stands there as faded copies, and everything else in the
//! drawing is locked ([`crate::cmd::curves::is_locked`], enforced by [`after_command`]). Objects
//! drawn while editing join the working set. REFSET adds objects to or removes them from it.
//! REFCLOSE writes the working set back into the block definition, so every reference to the
//! block shows the change, or discards the edit.
//!
//! The state is [`Drawing::refedit`]: it travels with undo, is never written to a file (files
//! are written from [`Drawing::committed`]) and ends with the drawing it belongs to.
//!
//! An xref is edited the same way; REFCLOSE Save then writes it back to its source drawing
//! ([`super::xref::writeback`]) and reloads it. A nested reference is edited by its path of
//! references (`nested`), the blocks around it shown faded. Saving a block with attribute
//! definitions synchronises its references' attributes (ATTSYNC). While editing, the rest of the
//! drawing is drawn faded (XFADECTL).
//!
//! Not yet: annotative scale contexts.

use std::collections::HashSet;
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity, EntityStore, Handle, RefEdit, Transparency, WorkItem};
use cadaddy_geom::Mat3;
use serde_json::{Value, json};

use super::super::curves::handle_value;
use super::super::machines::{SelOutcome, SelectPhase, SelectRun};
use super::references_block;
use crate::cmd::{CommandSpec, bad, bool_or, has_doc, str_param, targets};
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Transparency of the faded copies of the block's other objects.
const GHOST_FADE: u8 = 70;

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("refedit", "Edit Reference In-Place", run_refedit)
            .menu(&["Tools", "Xref and Block In-place Editing", "Edit Reference In-Place"])
            .params("{handle? | handles? | name? (a block reference, default the selection), nested?: [handles of references inside it, outermost first] (edit that nested block), objects?: [block object handles] (default all)} → {block, insert, xref, working: [handles], faded}: an xref is written back to its source drawing by REFCLOSE Save")
            .enabled(has_doc)
            .interactive(|_| Ok(Box::new(SelectRun::single("refedit", "REFEDIT", "Select reference")))),
        CommandSpec::new("refedit.tree", "Nested References", run_tree)
            .params("{handle (a block reference)} → {handle, block, xref, editable, nested: [...]}: the references inside it, for REFEDIT's `nested` path")
            .enabled(has_doc)
            .noundo(),
        CommandSpec::new("refset", "Working Set", run_refset)
            .menu(&["Tools", "Xref and Block In-place Editing", "Add to Working Set"])
            .params("{add?: [handles], remove?: [handles]} (or {handles, mode?: \"add\"|\"remove\"}); with neither: lists the working set → {working: [handles], added, removed}")
            .enabled(editing)
            .interactive(|_| Ok(Box::new(RefsetM::default()))),
        CommandSpec::new("refclose", "Save Reference Edits", run_refclose)
            .menu(&["Tools", "Xref and Block In-place Editing", "Save Reference Edits"])
            .params("{option?: \"save\"|\"discard\" | save?: bool (default true)} → {block, saved, objects}")
            .enabled(editing)
            .interactive(|_| Ok(Box::new(RefcloseM))),
    ]
}

fn editing(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    if s.doc().is_ok_and(|d| d.refedit.is_some()) { Ok(()) } else { Err("no block reference is being edited (REFEDIT)".into()) }
}

fn active(s: &Session, cmd: &str) -> Result<RefEdit> {
    s.doc()?.refedit.clone().ok_or_else(|| bad(cmd, "no block reference is being edited (REFEDIT)"))
}

/// A block object as the drawing shows it through the edited reference.
fn to_host(to_world: &Mat3, e: &Entity, h: Handle, ghost: bool) -> Entity {
    let mut c = e.clone();
    c.handle = h;
    c.kind.transform(to_world);
    if ghost {
        c.common.transparency = Transparency::Percent(GHOST_FADE);
    }
    c
}

/// A drawing object as it belongs in the block, with the block object's handle `h`.
fn to_block(inv: &Mat3, e: &Entity, h: Handle) -> Entity {
    let mut c = e.clone();
    c.handle = h;
    c.kind.transform(inv);
    c
}

fn put(d: &mut Drawing, r: &RefEdit, e: Entity) -> Result<()> {
    d.ensure_layer(&e.common.layer);
    d.space_mut(&r.space).ok_or_else(|| bad("refedit", "the space of the reference is gone"))?.push(e);
    Ok(())
}

fn hexes(hs: impl IntoIterator<Item = Handle>) -> Vec<String> {
    hs.into_iter().map(|h| h.hex()).collect()
}

// ---------------- REFEDIT ----------------

/// Most nesting levels a nested reference may be picked at.
const MAX_NESTING: usize = 32;

/// Whether `m` keeps shapes (no stretching or flattening): REFEDIT shows the block through it.
fn conformal(m: &Mat3) -> bool {
    let finite = [m.a, m.b, m.c, m.d, m.e, m.f].iter().all(|v| v.is_finite());
    let (sx, sy) = (m.a.hypot(m.b), m.c.hypot(m.d));
    finite && m.det().abs() > 1e-12 && (sx - sy).abs() <= 1e-9 * sx.max(sy) && (m.a * m.c + m.b * m.d).abs() <= 1e-9 * sx * sy
}

/// Why block `b` can't be edited in place (`nested`: picked inside another reference), if it can't.
fn refuse(b: &cadaddy_doc::Block, nested: bool) -> Option<String> {
    if b.xref_path.is_some() && nested {
        return Some(format!("\"{}\" is an xref nested in another reference; edit it from a reference of its own", b.name));
    }
    if b.name.contains('|') {
        let owner = b.name.split('|').next().unwrap_or_default();
        return Some(format!("\"{}\" belongs to xref \"{owner}\"; open that drawing to edit it", b.name));
    }
    if b.name.starts_with('*') || b.array.is_some() {
        return Some(format!("\"{}\" is generated geometry and can't be edited in place", b.name));
    }
    None
}

fn run_refedit(s: &mut Session, p: &Value) -> Result<Value> {
    if s.doc()?.refedit.is_some() {
        return Err(bad("refedit", "a reference is already being edited; finish with REFCLOSE"));
    }
    let d = s.doc()?;
    let picked = match str_param(p, "name") {
        Some(n) => d
            .model
            .iter()
            .chain(d.layouts.iter().flat_map(|l| l.entities.iter()))
            .find(|e| matches!(&e.kind, cadaddy_doc::EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(n)))
            .map(|e| e.handle),
        None => targets(s, p)?.into_iter().find(|h| d.entity(*h).is_some_and(|e| matches!(e.kind, cadaddy_doc::EntityKind::Insert(_)))),
    };
    let ih = picked.ok_or_else(|| bad("refedit", "select a block reference"))?;
    let (ins, space) = match d.entity(ih).map(|e| &e.kind) {
        Some(cadaddy_doc::EntityKind::Insert(i)) => (i.clone(), d.space_of(ih).ok_or_else(|| bad("refedit", "the reference is in no space"))?),
        _ => return Err(bad("refedit", "select a block reference")),
    };
    let mut blk = d.block(&ins.block).cloned().ok_or_else(|| bad("refedit", format!("block \"{}\" not found", ins.block)))?;
    let mut to_world = ins.transform(blk.base.xy());
    // A nested reference: the path of references from the picked one inward. The objects of the
    // blocks around it show faded.
    let path: Vec<Handle> = match p.get("nested") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) if a.len() <= MAX_NESTING => a
            .iter()
            .map(|v| handle_value(v).ok_or_else(|| bad("refedit", format!("`nested`: {v} is not an object handle"))))
            .collect::<Result<_>>()?,
        Some(_) => {
            return Err(bad("refedit", format!("`nested` must be an array of at most {MAX_NESTING} handles of references inside the picked one")));
        }
    };
    if !path.is_empty()
        && let Some(why) = refuse(&blk, false)
            .or_else(|| blk.xref_path.as_ref().map(|_| format!("\"{}\" is an xref; its nested blocks are edited in its own drawing", blk.name)))
    {
        return Err(bad("refedit", why));
    }
    let mut context: Vec<(Mat3, Entity)> = Vec::new();
    for h in &path {
        let inner = match blk.entities.get(*h).map(|e| &e.kind) {
            Some(cadaddy_doc::EntityKind::Insert(i)) => i.clone(),
            _ => return Err(bad("refedit", format!("`nested`: {} is not a block reference in block \"{}\"", h.hex(), blk.name))),
        };
        let nb = d.block(&inner.block).cloned().ok_or_else(|| bad("refedit", format!("block \"{}\" not found", inner.block)))?;
        if let Some(why) = refuse(&nb, true) {
            return Err(bad("refedit", why));
        }
        context.extend(blk.entities.iter().filter(|e| e.handle != *h).map(|e| (to_world, (**e).clone())));
        to_world = to_world.then_before(inner.transform(nb.base.xy()));
        blk = nb;
    }
    if let Some(why) = refuse(&blk, !path.is_empty()) {
        return Err(bad("refedit", why));
    }
    if blk.xref_path.is_some() {
        let host = s.state().ok().and_then(|st| st.path.clone());
        super::xref::writeback::source(d, &blk.name, host.as_deref()).map_err(|e| bad("refedit", e))?;
    }
    if !conformal(&to_world) {
        return Err(bad("refedit", "the reference must be scaled equally in X and Y (and not flat) to be edited in place"));
    }
    let chosen: Option<HashSet<Handle>> = match p.get("objects") {
        None | Some(Value::Null) => None,
        Some(Value::String(a)) if a.eq_ignore_ascii_case("all") => None,
        Some(Value::Array(a)) => {
            let mut set = HashSet::new();
            for v in a {
                let h = handle_value(v).ok_or_else(|| bad("refedit", format!("`objects`: {v} is not an object handle")))?;
                if blk.entities.get(h).is_none() {
                    return Err(bad("refedit", format!("object {} is not in block \"{}\"", h.hex(), blk.name)));
                }
                set.insert(h);
            }
            if set.is_empty() {
                return Err(bad("refedit", "`objects` is empty"));
            }
            Some(set)
        }
        Some(v) => return Err(bad("refedit", format!("`objects` must be \"all\" or an array of handles, got {v}"))),
    };
    let was_visible = d.entity(ih).is_some_and(|e| e.common.visible);
    let mut r = RefEdit {
        block: blk.name.clone(),
        insert: ih,
        insert_was_visible: was_visible,
        space,
        to_world,
        items: Vec::new(),
        ghosts: Vec::new(),
        released: Vec::new(),
        context: Vec::new(),
    };
    let doc = s.doc_mut()?;
    for (m, e) in &context {
        let h = doc.new_handle();
        put(doc, &r, to_host(m, e, h, true))?;
        r.context.push(h);
    }
    for e in blk.entities.iter() {
        let working = chosen.as_ref().is_none_or(|c| c.contains(&e.handle));
        let h = doc.new_handle();
        put(doc, &r, to_host(&to_world, e, h, !working))?;
        if working {
            r.items.push(WorkItem { copy: h, source: Some(e.handle) });
        } else {
            r.ghosts.push((h, e.handle));
        }
    }
    doc.modify_entity(ih, |e| e.common.visible = false).map_err(|e| bad("refedit", e.to_string()))?;
    let out = json!({
        "block": r.block,
        "insert": ih.hex(),
        "xref": blk.xref_path.is_some(),
        "working": hexes(r.items.iter().map(|i| i.copy)),
        "faded": r.ghosts.len() + r.context.len(),
    });
    let sel: Vec<Handle> = r.items.iter().map(|i| i.copy).collect();
    doc.refedit = Some(r);
    s.set_selection(sel);
    Ok(out)
}

/// The references nested in block reference `handle`, as a tree: {handle, block, xref,
/// editable, nested: [...]} (what REFEDIT's `nested` path picks from).
fn run_tree(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let h = super::super::curves::handle_param(p, "handle").ok_or_else(|| bad("refedit.tree", "`handle` (a block reference) is required"))?;
    let Some(cadaddy_doc::EntityKind::Insert(i)) = d.entity(h).map(|e| &e.kind) else {
        return Err(bad("refedit.tree", "`handle` is not a block reference"));
    };
    let mut budget = 10_000usize;
    Ok(tree(d, h, &i.block, 0, &mut budget))
}

fn tree(d: &Drawing, h: Handle, block: &str, depth: usize, budget: &mut usize) -> Value {
    let b = d.block(block);
    let mut nested = Vec::new();
    if let Some(b) = b.filter(|b| depth < MAX_NESTING && b.xref_path.is_none()) {
        for e in b.entities.iter() {
            if let cadaddy_doc::EntityKind::Insert(i) = &e.kind {
                if *budget == 0 {
                    break;
                }
                *budget -= 1;
                nested.push(tree(d, e.handle, &i.block, depth + 1, budget));
            }
        }
    }
    let editable = b.is_some_and(|b| refuse(b, depth > 0).is_none());
    json!({ "handle": h.hex(), "block": block, "xref": b.is_some_and(|b| b.xref_path.is_some()), "editable": editable, "nested": nested })
}

// ---------------- REFSET ----------------

fn handle_list(p: &Value, key: &str) -> Result<Vec<Handle>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => {
            a.iter().map(|v| handle_value(v).ok_or_else(|| bad("refset", format!("`{key}`: {v} is not an object handle")))).collect()
        }
        Some(v) => Err(bad("refset", format!("`{key}` must be an array of object handles, got {v}"))),
    }
}

fn run_refset(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut add, mut remove) = (handle_list(p, "add")?, handle_list(p, "remove")?);
    let hs = handle_list(p, "handles")?;
    if str_param(p, "mode").is_some_and(|m| m.to_ascii_lowercase().starts_with('r')) {
        remove.extend(hs)
    } else {
        add.extend(hs)
    }
    apply_refset(s, &add, &remove)
}

fn apply_refset(s: &mut Session, add: &[Handle], remove: &[Handle]) -> Result<Value> {
    let mut r = active(s, "refset")?;
    let inv = r.to_world.inverse().ok_or_else(|| bad("refset", "the reference can't be inverted"))?;
    let d = s.doc()?;
    for h in add.iter().chain(remove) {
        if d.entity(*h).is_none() {
            return Err(bad("refset", format!("no object {}", h.hex())));
        }
    }
    if let Some(h) = remove.iter().find(|h| !r.is_working(**h)) {
        return Err(bad("refset", format!("object {} is not in the working set", h.hex())));
    }
    let blk = d.block(&r.block).cloned().ok_or_else(|| bad("refset", format!("block \"{}\" is gone; REFCLOSE Discard", r.block)))?;
    let (mut added, mut removed) = (0usize, 0usize);
    let doc = s.doc_mut()?;
    if let Some(h) = add.iter().find(|h| r.context.contains(h)) {
        return Err(bad("refset", format!("object {} belongs to a block around the edited one", h.hex())));
    }
    for h in add {
        if *h == r.insert || r.is_working(*h) {
            continue;
        }
        if let Some(pos) = r.ghosts.iter().position(|(g, _)| g == h) {
            // A faded block object joins the working set (with the edits it had before).
            let (g, src) = r.ghosts.remove(pos);
            doc.remove_entity(g);
            let cur = match r.released.iter().position(|e| e.handle == src) {
                Some(i) => Some(r.released.remove(i)),
                None => blk.entities.get(src).map(|e| (**e).clone()),
            };
            if let Some(cur) = cur {
                let c = doc.new_handle();
                put(doc, &r, to_host(&r.to_world, &cur, c, false))?;
                r.items.push(WorkItem { copy: c, source: Some(src) });
                added += 1;
            }
        } else {
            r.items.push(WorkItem { copy: *h, source: None });
            added += 1;
        }
    }
    for h in remove {
        let Some(pos) = r.items.iter().position(|i| i.copy == *h) else { continue };
        let item = r.items.remove(pos);
        if let Some(src) = item.source
            && let Some(e) = doc.entity(*h).map(|e| (**e).clone())
        {
            // Back to the block, edits kept, shown faded.
            let kept = to_block(&inv, &e, src);
            doc.remove_entity(*h);
            let g = doc.new_handle();
            put(doc, &r, to_host(&r.to_world, &kept, g, true))?;
            r.ghosts.push((g, src));
            r.released.push(kept);
        }
        removed += 1;
    }
    let working = hexes(r.items.iter().map(|i| i.copy));
    doc.refedit = Some(r);
    Ok(json!({ "working": working, "added": added, "removed": removed }))
}

// ---------------- REFCLOSE ----------------

fn run_refclose(s: &mut Session, p: &Value) -> Result<Value> {
    let save = match str_param(p, "option") {
        Some(o) if o.to_ascii_lowercase().starts_with('d') => false,
        Some(o) if o.to_ascii_lowercase().starts_with('s') => true,
        Some(o) => return Err(bad("refclose", format!("unknown option \"{o}\" (save or discard)"))),
        None => bool_or(p, "save", true),
    };
    close(s, save)
}

fn close(s: &mut Session, save: bool) -> Result<Value> {
    let r = active(s, "refclose")?;
    // Work on a copy: the drawing changes only when everything went through.
    let mut nd: Drawing = s.doc()?.clone();
    let mut objects = 0;
    let mut out = json!({ "block": r.block, "saved": save });
    if save {
        let loaded = nd.block(&r.block).cloned();
        objects = commit_block(&mut nd, &r)?;
        if let Some(loaded) = loaded.filter(|b| b.xref_path.is_some()) {
            // An xref: its source drawing gets the change (the previous file kept as .bak), then
            // the xref is loaded again from it.
            let host = s.state().ok().and_then(|st| st.path.clone());
            let file = super::xref::writeback::source(&nd, &r.block, host.as_deref()).map_err(|e| bad("refclose", e))?;
            let edited = nd.block(&r.block).cloned().ok_or_else(|| bad("refclose", "the xref is gone; use REFCLOSE Discard"))?;
            super::xref::writeback::write_back(&file, &r.block, &loaded, &edited, &nd)
                .map_err(|e| bad("refclose", format!("{e}. The edit is still open: fix the problem and save again, or discard it")))?;
            let reload = super::xref::load::load(&mut nd, &r.block, host.as_deref(), false);
            out["file"] = json!(file.to_string_lossy());
            out["reloaded"] = json!(reload.first().is_some_and(|o| o.status == cadaddy_doc::XrefStatus::Loaded));
            objects = nd.block(&r.block).map_or(objects, |b| b.entities.len());
        }
    }
    nd.end_refedit_discard(&r);
    *s.doc_mut()? = nd;
    // Other references of a block with attributes follow its definitions (ATTSYNC).
    if save && super::attribs::has_attdefs(s, &r.block) {
        let (_, n) = super::attribs::sync(s, &r.block)?;
        out["synced"] = json!(n);
    }
    s.set_selection(Vec::new());
    out["objects"] = json!(objects);
    Ok(out)
}

/// Replace the block's objects by the working set (and move the drawing objects added to it
/// into the block). Returns the number of objects the block has now.
fn commit_block(nd: &mut Drawing, r: &RefEdit) -> Result<usize> {
    let blk = nd.block(&r.block).cloned().ok_or_else(|| bad("refclose", format!("block \"{}\" is gone; use REFCLOSE Discard", r.block)))?;
    let inv = r.to_world.inverse().ok_or_else(|| bad("refclose", "the reference can't be inverted; use REFCLOSE Discard"))?;
    let mut ents: Vec<Entity> = Vec::new();
    for e in blk.entities.iter() {
        match r.items.iter().find(|i| i.source == Some(e.handle)) {
            // A working object the user erased leaves the block.
            Some(i) => {
                if let Some(c) = nd.entity(i.copy) {
                    ents.push(to_block(&inv, c, e.handle));
                }
            }
            None => match r.released.iter().find(|x| x.handle == e.handle) {
                Some(x) => ents.push(x.clone()),
                None => ents.push((**e).clone()),
            },
        }
    }
    let mut moved = Vec::new();
    for i in r.items.iter().filter(|i| i.source.is_none()) {
        if let Some(c) = nd.entity(i.copy).map(|c| (**c).clone()) {
            let h = nd.new_handle();
            ents.push(to_block(&inv, &c, h));
            moved.push(i.copy);
        }
    }
    if references_block(nd, &ents, &r.block) {
        return Err(bad("refclose", format!("Block \"{}\" would reference itself; remove that reference or use REFCLOSE Discard.", r.block)));
    }
    let n = ents.len();
    let mut store = EntityStore::new();
    for e in ents {
        store.push(e);
    }
    let key = nd.blocks.keys().find(|k| k.eq_ignore_ascii_case(&r.block)).cloned().unwrap_or_else(|| r.block.clone());
    let mut nb = (*blk).clone();
    nb.entities = store;
    nd.blocks.insert(key, Arc::new(nb));
    for h in moved {
        nd.remove_entity(h);
    }
    Ok(n)
}

// ---------------- while editing ----------------

/// After a command ran: objects it created join the working set, and a command that changed
/// anything outside the working set is undone (returns the reason).
pub(crate) fn after_command(s: &mut Session, before: &Arc<Drawing>, id: &str) -> Option<String> {
    if matches!(id, "refedit" | "refset" | "refclose") || !crate::cmd::find_command(id).is_some_and(|c| c.undoable) {
        return None;
    }
    let rb = before.refedit.as_ref()?;
    let st = s.state_mut().ok()?;
    if Arc::ptr_eq(before, &st.doc) {
        return None;
    }
    let ra = st.doc.refedit.as_ref().filter(|ra| ra.insert == rb.insert)?;
    let owned: HashSet<Handle> =
        rb.items.iter().map(|i| i.copy).chain(rb.ghosts.iter().map(|g| g.0)).chain(rb.context.iter().copied()).chain([rb.insert]).collect();
    let stores = |d: &Drawing| std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities)).cloned().collect::<Vec<_>>();
    let touched = stores(before)
        .iter()
        .any(|store| store.iter().any(|e| !owned.contains(&e.handle) && !st.doc.entity(e.handle).is_some_and(|a| Arc::ptr_eq(a, e))));
    if touched {
        st.doc = before.clone();
        return Some("Only the working set can be edited during REFEDIT (REFSET adds objects, REFCLOSE ends the edit).".into());
    }
    let seed = before.handseed;
    let mut new: Vec<Handle> = Vec::new();
    for store in stores(&st.doc) {
        new.extend(store.iter().map(|e| e.handle).filter(|h| h.0 >= seed && !ra.owns(*h)));
    }
    if !new.is_empty()
        && let Some(r) = Arc::make_mut(&mut st.doc).refedit.as_mut()
    {
        r.items.extend(new.into_iter().map(|copy| WorkItem { copy, source: None }));
    }
    None
}

/// Whether `h` may not be edited because a reference is being edited and it is not in the working set.
pub(crate) fn locked(d: &Drawing, h: Handle) -> bool {
    d.refedit.as_ref().is_some_and(|r| !r.is_working(h))
}

// ---------------- interactive ----------------

#[derive(Default)]
enum RStage {
    #[default]
    Mode,
    Pick(bool),
}

#[derive(Default)]
struct RefsetM {
    stage: RStage,
    sel: SelectPhase,
}

impl Interactive for RefsetM {
    fn name(&self) -> &'static str {
        "REFSET"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        active(s, "refset").map(|_| Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            RStage::Mode => {
                let mut p = Prompt::new("Enter an option [Add/Remove] <Add>", Accept::TEXT);
                p.keywords = vec!["Add".into(), "Remove".into()];
                p.default = Some("Add".into());
                p
            }
            RStage::Pick(remove) => {
                let mut p = self.sel.prompt();
                p.message = format!("Select objects to {}", if remove { "remove" } else { "add" });
                p
            }
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.stage {
            RStage::Mode => {
                let remove = match i {
                    Input::Keyword(k) | Input::Text(k) => k.to_ascii_lowercase().starts_with('r'),
                    Input::Enter => false,
                    _ => return Ok(Step::Cancel),
                };
                self.sel = SelectPhase::begin(s);
                self.stage = RStage::Pick(remove);
                if self.sel.done {
                    let hs = self.sel.picked.clone();
                    return self.finish(s, remove, &hs);
                }
                Ok(Step::Continue)
            }
            RStage::Pick(remove) => match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => self.finish(s, remove, &hs),
            },
        }
    }
}

impl RefsetM {
    fn finish(&mut self, s: &mut Session, remove: bool, hs: &[Handle]) -> Result<Step> {
        let r = if remove { apply_refset(s, &[], hs) } else { apply_refset(s, hs, &[]) }?;
        s.echo(format!("{} object(s) in the working set.", r.get("working").and_then(Value::as_array).map_or(0, Vec::len)));
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
}

struct RefcloseM;

impl Interactive for RefcloseM {
    fn name(&self) -> &'static str {
        "REFCLOSE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        let mut p = Prompt::new("Enter option [Save/Discard reference changes] <Save>", Accept::TEXT);
        p.keywords = vec!["Save".into(), "Discard".into()];
        p.default = Some("Save".into());
        p
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let save = match i {
            Input::Keyword(k) | Input::Text(k) => !k.to_ascii_lowercase().starts_with('d'),
            Input::Enter => true,
            _ => return Ok(Step::Cancel),
        };
        let r = close(s, save)?;
        s.echo(if save { "Reference changes saved." } else { "Reference changes discarded." });
        let _ = r;
        Ok(Step::Done)
    }
}
