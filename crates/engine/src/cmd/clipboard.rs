//! The clipboard every open drawing shares (COPYCLIP, CUTCLIP, COPYBASE; PASTECLIP, PASTEORIG,
//! PASTEBLOCK).
//!
//! Like AutoCAD's, it holds the copied objects together with what they refer to in their drawing:
//! layers, linetypes, text, dimension, multileader and table styles, and the blocks they reference
//! (nested ones too). Pasting adds the ones the target drawing doesn't have; a name the target
//! already has keeps the target's definition, and a generated (anonymous `*…`) block whose name is
//! taken gets a fresh one. Pasted objects get new handles.
//!
//! The clipboard can also travel as DXF text ([`system_text`], [`load_system_text`]), so a front
//! end can put it on the system clipboard and paste it into another CADaddy window, and as a
//! picture ([`system_image`]) that pastes into documents and email. What other programs put on
//! the system clipboard ([`SystemContents`]: an image, text) pastes as a raster image saved beside
//! the drawing (`paste_image`) or as multiline text (`paste_text`).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use cadaddy_doc::{Block, Drawing, Entity, EntityKind, HVal, Handle};
use cadaddy_geom::{Mat3, Vec2};

use super::{bad, has_doc};
use crate::{Result, Session};

/// More objects than this aren't mirrored to (or read from) the system clipboard.
const MAX_SYSTEM_OBJECTS: usize = 50_000;
/// Bounds the walk through nested block references.
const MAX_WALK: usize = 5_000_000;

/// Where the clipboard's objects came from, and the definitions they need.
#[derive(Clone, Debug)]
pub struct ClipSource {
    /// `DocState::uid` of the drawing they were copied from; `None` when they came from the system
    /// clipboard.
    pub uid: Option<u64>,
    /// The layers, linetypes, styles and blocks the objects reference (and the source's header).
    pub defs: Arc<Drawing>,
    /// Counts copies, so a front end mirrors each new clipboard to the system clipboard once.
    pub serial: u64,
}

/// Put copies of `ents` from the active drawing on the clipboard, with `base` as base point.
pub(crate) fn copy(s: &mut Session, ents: Vec<Entity>, base: Vec2) -> Result<()> {
    let uid = s.state()?.uid;
    let mut defs = defs_for(s.doc()?, &ents);
    // Whole groups among the objects travel with them: pasting makes unnamed groups of the copies.
    let copied: HashSet<Handle> = ents.iter().map(|e| e.handle).collect();
    defs.groups = super::groups::whole_groups(&s.doc()?.groups, &copied);
    set(s, ents, base, defs, Some(uid));
    Ok(())
}

fn set(s: &mut Session, ents: Vec<Entity>, base: Vec2, defs: Drawing, uid: Option<u64>) {
    let serial = s.clipboard_source.as_ref().map_or(0, |c| c.serial).wrapping_add(1);
    s.clipboard = ents;
    s.clipboard_base = base;
    s.clipboard_source = Some(ClipSource { uid, defs: Arc::new(defs), serial });
}

/// Enabled check of PASTECLIP and PASTEBLOCK: something to paste.
pub(crate) fn has_clip(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    if s.clipboard.is_empty() { Err("the clipboard is empty".into()) } else { Ok(()) }
}

/// Enabled check of PASTECLIP and PASTESPEC: CADaddy objects, or an image or text copied in
/// another program.
pub(crate) fn has_paste(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    if s.clipboard.is_empty() && s.system_clipboard.is_empty() { Err("the clipboard is empty".into()) } else { Ok(()) }
}

/// Enabled check of PASTEORIG: as in AutoCAD, it pastes objects copied from another drawing.
pub(crate) fn can_paste_orig(s: &Session) -> std::result::Result<(), String> {
    has_clip(s)?;
    if from_active(s) {
        return Err("the clipboard holds objects from this drawing; Paste to Original Coordinates pastes into another drawing".into());
    }
    Ok(())
}

/// Whether the clipboard was copied from the active drawing.
pub fn from_active(s: &Session) -> bool {
    let active = s.state().ok().map(|st| st.uid);
    s.clipboard_source.as_ref().is_some_and(|c| c.uid.is_some() && c.uid == active)
}

/// Paste the clipboard into the active drawing's current space, moved by `m`; selects and
/// returns the new objects.
pub(crate) fn paste(s: &mut Session, m: &Mat3) -> Result<Vec<Handle>> {
    let ents = s.clipboard.clone();
    let defs = s.clipboard_source.as_ref().map(|c| c.defs.clone());
    let space = s.space();
    let d = s.doc_mut()?;
    let groups = defs.as_ref().map(|x| x.groups.clone()).unwrap_or_default();
    let renames = defs.map(|x| merge(d, &x)).unwrap_or_default();
    let mut out = Vec::new();
    let mut copies = std::collections::HashMap::new();
    for e in &ents {
        let old = e.handle;
        let mut e = adopt(d, e, &renames);
        e.kind.transform(m);
        d.ensure_layer(&e.common.layer);
        out.push(e.handle);
        copies.insert(old, e.handle);
        if let Some(st) = d.space_mut(&space) {
            st.push(e);
        }
    }
    super::groups::copy_groups(d, &groups, &copies);
    s.set_selection(out.clone());
    Ok(out)
}

/// PASTEBLOCK: the clipboard becomes a new block (named `A$C…`, base point = the clipboard's), and
/// a reference to it is inserted at `at`. Returns the block reference.
pub(crate) fn paste_block(s: &mut Session, at: Vec2) -> Result<Handle> {
    let ents = s.clipboard.clone();
    if ents.is_empty() {
        return Err(bad("pasteblock", "the clipboard is empty"));
    }
    let defs = s.clipboard_source.as_ref().map(|c| c.defs.clone());
    let base = s.clipboard_base;
    let d = s.doc_mut()?;
    let renames = defs.map(|x| merge(d, &x)).unwrap_or_default();
    let seed = d.handseed;
    let name = (0..1_000_000u64)
        .map(|i| format!("A$C{:08X}", seed.wrapping_add(i) & 0xFFFF_FFFF))
        .find(|n| d.block(n).is_none())
        .ok_or_else(|| bad("pasteblock", "no free block name"))?;
    let mut b = Block::new(&name);
    b.base = base.to3(0.0);
    for e in &ents {
        let e = adopt(d, e, &renames);
        d.ensure_layer(&e.common.layer);
        b.entities.push(e);
    }
    d.blocks.insert(name.clone(), Arc::new(b));
    let h = super::blocks::insert(s, &name, at, 1.0, 0.0, &serde_json::Map::new())?;
    s.set_selection(vec![h]);
    Ok(h)
}

/// The clipboard as DXF text (base point in `$INSBASE`), for the system clipboard. `None` when
/// it is empty, very large, or this build has no DXF writer.
pub fn system_text(s: &Session) -> Option<String> {
    if s.clipboard.is_empty() || s.clipboard.len() > MAX_SYSTEM_OBJECTS {
        return None;
    }
    let hooks = super::file::io()?;
    let d = clip_drawing(s);
    let bytes = (hooks.write)(&d, "clipboard.dxf").ok()?;
    String::from_utf8(bytes).ok()
}

/// Make DXF text (from the system clipboard) the clipboard: its model space objects with what
/// they reference, the base point from `$INSBASE`. `false` (clipboard unchanged) when the text
/// isn't a DXF drawing with objects.
pub fn load_system_text(s: &mut Session, text: &str) -> bool {
    let mut lines = text.lines().map(str::trim);
    if lines.next() != Some("0") || lines.next() != Some("SECTION") || text.len() > 256 << 20 {
        return false;
    }
    let Some(hooks) = super::file::io() else { return false };
    let Ok(src) = (hooks.read)(text.as_bytes(), "clipboard.dxf") else { return false };
    if src.model.is_empty() || src.model.len() > MAX_SYSTEM_OBJECTS {
        return false;
    }
    let ents: Vec<Entity> = src.model.iter().map(|e| (**e).clone()).collect();
    let base = src.header.point("INSBASE").map(|p| p.xy()).filter(|p| p.is_finite()).unwrap_or(Vec2::ZERO);
    let defs = defs_for(&src, &ents);
    set(s, ents, base, defs, None);
    true
}

/// What the system clipboard holds that isn't CADaddy objects, as the front end read it.
#[derive(Clone, Debug, Default)]
pub struct SystemContents {
    /// An image copied in another program.
    pub image: Option<Arc<cadaddy_render::images::Raster>>,
    /// Text copied in another program (not a CADaddy drawing).
    pub text: Option<String>,
}

impl SystemContents {
    pub fn is_empty(&self) -> bool {
        self.image.is_none() && self.text.as_deref().is_none_or(|t| t.trim().is_empty())
    }
}

/// Longest side of the picture of the clipboard put on the system clipboard, in pixels.
pub const IMAGE_SIDE: u32 = 1600;
/// More characters than this don't paste as text.
const MAX_PASTE_TEXT: usize = 100_000;
/// Pasted images saved beside one drawing: names tried before giving up.
const MAX_IMAGE_NAMES: u32 = 10_000;

/// The clipboard as a drawing of its own: its objects in model space with what they reference,
/// the base point in `$INSBASE`.
fn clip_drawing(s: &Session) -> Drawing {
    let mut d = Drawing::new_imperial();
    let renames = match &s.clipboard_source {
        Some(c) => {
            d.header = c.defs.header.clone();
            merge(&mut d, &c.defs)
        }
        None => BTreeMap::new(),
    };
    d.header.set("INSBASE", HVal::Point(s.clipboard_base.to3(0.0)));
    for e in &s.clipboard {
        let e = adopt(&mut d, e, &renames);
        d.ensure_layer(&e.common.layer);
        d.model.push(e);
    }
    d
}

/// The clipboard drawn as a picture on white, its longer side `side` pixels (16 to
/// [`IMAGE_SIDE`]): what COPYCLIP puts on the system clipboard beside the objects, so they paste
/// into documents and email. `None` when the clipboard is empty or very large.
pub fn system_image(s: &Session, side: u32) -> Option<cadaddy_render::images::Raster> {
    if s.clipboard.is_empty() || s.clipboard.len() > MAX_SYSTEM_OBJECTS {
        return None;
    }
    let d = clip_drawing(s);
    let list = cadaddy_render::build(&d, &cadaddy_doc::Space::Model, &cadaddy_render::Options::default());
    let b = list.bounds;
    if b.is_empty() || !(b.width().is_finite() && b.height().is_finite()) {
        return None;
    }
    let side = side.clamp(16, IMAGE_SIDE);
    let (bw, bh) = (b.width().max(1e-9), b.height().max(1e-9));
    let short = |k: f64| ((f64::from(side) * k).round() as u32).clamp(16, side);
    let (w, h) = if bw >= bh { (side, short(bh / bw)) } else { (short(bw / bh), side) };
    let view = cadaddy_render::raster::View::fit(&b, w, h, 0.06);
    let ro = cadaddy_render::raster::RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 1.5, px_per_mm: 0.0, antialias: true };
    let png = cadaddy_render::raster::render_png(&list, &view, &ro)?;
    cadaddy_render::images::decode(&png)
}

/// Paste `image` as a raster IMAGE: saved as a PNG beside the (saved) drawing and attached with a
/// relative path, its lower-left corner at `at` and `width` drawing units wide; by default half
/// the view tall and centred in it.
pub(crate) fn paste_image(
    s: &mut Session,
    image: &cadaddy_render::images::Raster,
    at: Option<Vec2>,
    width: Option<f64>,
) -> Result<serde_json::Value> {
    let host = s.state()?.path.clone().ok_or_else(|| bad("pastespec", "save the drawing first: a pasted image is saved as a PNG file beside it"))?;
    let host = std::path::Path::new(&host);
    let dir = host.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let stem = host.file_stem().map_or_else(|| "drawing".to_string(), |s| s.to_string_lossy().into_owned());
    let file = (1..=MAX_IMAGE_NAMES)
        .map(|n| dir.join(format!("{stem}-pasted-{n}.png")))
        .find(|f| !f.exists())
        .ok_or_else(|| bad("pastespec", "too many pasted images beside this drawing"))?;
    let png = image.encode_png().ok_or_else(|| bad("pastespec", "the image couldn't be encoded"))?;
    std::fs::write(&file, png).map_err(|e| bad("pastespec", format!("the pasted image couldn't be saved beside the drawing: {e}")))?;
    let view = s.state()?.view();
    let aspect = f64::from(image.width) / f64::from(image.height.max(1));
    let width = width.filter(|w| w.is_finite() && *w > 0.0).unwrap_or(view.height * 0.5 * aspect);
    let at = at.unwrap_or(view.center - Vec2::new(width / 2.0, width / aspect / 2.0));
    super::imageattach::attach(s, &file.to_string_lossy(), at, width, 0.0)
}

/// `text` as MTEXT contents: backslashes and braces kept as typed, line breaks as paragraphs.
fn mtext_contents(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.replace("\r\n", "\n").chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' | '\r' => out.push_str("\\P"),
            c => out.push(c),
        }
    }
    out
}

/// Paste `text` as multiline text at `at` (default: the view's centre) in the current text style
/// and height.
pub(crate) fn paste_text(s: &mut Session, text: &str, at: Option<Vec2>) -> Result<serde_json::Value> {
    let text = text.trim_end();
    if text.trim().is_empty() {
        return Err(bad("pastespec", "the clipboard holds no text"));
    }
    if text.chars().count() > MAX_PASTE_TEXT {
        return Err(bad("pastespec", "the clipboard's text is too long to paste as text"));
    }
    let at = at.unwrap_or(s.state()?.view().center);
    super::draw::run_mtext(s, &serde_json::json!({ "at": [at.x, at.y], "text": mtext_contents(text) }))
}

/// Upper-case names of the table records and blocks some objects use.
#[derive(Default)]
struct Used {
    layers: HashSet<String>,
    linetypes: HashSet<String>,
    text_styles: HashSet<String>,
    dim_styles: HashSet<String>,
    mleader_styles: HashSet<String>,
    table_styles: HashSet<String>,
    blocks: HashSet<String>,
}

fn up(n: &str) -> String {
    n.to_ascii_uppercase()
}

impl Used {
    /// Note what `e` uses; returns the blocks it references.
    fn note(&mut self, e: &Entity) -> Vec<String> {
        self.layers.insert(up(&e.common.layer));
        self.linetypes.insert(up(&e.common.linetype));
        let mut blocks = Vec::new();
        match &e.kind {
            EntityKind::Text(t) => {
                self.text_styles.insert(up(&t.style));
            }
            EntityKind::MText(t) => {
                self.text_styles.insert(up(&t.style));
            }
            EntityKind::AttDef(a) => {
                self.text_styles.insert(up(&a.text.style));
            }
            EntityKind::Insert(i) => {
                self.text_styles.extend(i.attribs.iter().map(|a| up(&a.text.style)));
                blocks.push(i.block.clone());
            }
            EntityKind::Dimension(dm) => {
                self.dim_styles.insert(up(&dm.style));
                blocks.extend(dm.block.clone());
            }
            EntityKind::Leader(l) => {
                self.dim_styles.insert(up(&l.style));
            }
            EntityKind::Tolerance(t) => {
                self.dim_styles.insert(up(&t.style));
            }
            EntityKind::MLeader(m) => {
                self.mleader_styles.insert(up(&m.style));
                self.text_styles.extend(m.text.iter().map(|t| up(&t.style)));
                blocks.extend(m.block.as_ref().map(|b| b.block.clone()));
            }
            EntityKind::Table(t) => {
                self.table_styles.insert(up(&t.style));
            }
            _ => {}
        }
        blocks
    }
}

/// The definitions `ents` need from `src`: the layers, linetypes, styles and blocks they (and the
/// blocks they reference, nested ones too) use, and `src`'s header.
pub(crate) fn defs_for(src: &Drawing, ents: &[Entity]) -> Drawing {
    let mut used = Used::default();
    let mut frontier: Vec<String> = ents.iter().flat_map(|e| used.note(e)).collect();
    let mut walked = 0usize;
    while let Some(name) = frontier.pop() {
        if !used.blocks.insert(up(&name)) {
            continue;
        }
        let Some(b) = src.block(&name) else { continue };
        for e in b.entities.iter() {
            walked += 1;
            if walked > MAX_WALK {
                break;
            }
            frontier.extend(used.note(e));
        }
    }
    // Records referenced by other records.
    for l in src.layers.iter().filter(|l| used.layers.contains(&up(&l.name))) {
        used.linetypes.insert(up(&l.linetype));
    }
    for t in src.linetypes.iter().filter(|t| used.linetypes.contains(&up(&t.name))) {
        used.text_styles.extend(t.pattern.iter().filter_map(|p| p.style.as_deref()).map(up));
    }
    for ds in src.dim_styles.iter().filter(|t| used.dim_styles.contains(&up(&t.name))) {
        used.text_styles.insert(up(&ds.text_style));
    }
    for ms in src.mleader_styles.iter().filter(|t| used.mleader_styles.contains(&up(&t.name))) {
        used.text_styles.insert(up(&ms.text_style));
    }
    fn pick<T: Clone>(all: &[T], used: &HashSet<String>, name: fn(&T) -> &str) -> Vec<T> {
        all.iter().filter(|t| used.contains(&up(name(t)))).cloned().collect()
    }
    let mut d = Drawing::new_imperial();
    d.header = src.header.clone();
    d.layers = pick(&src.layers, &used.layers, |l| &l.name);
    d.linetypes = pick(&src.linetypes, &used.linetypes, |l| &l.name);
    d.text_styles = pick(&src.text_styles, &used.text_styles, |t| &t.name);
    d.dim_styles = pick(&src.dim_styles, &used.dim_styles, |t| &t.name);
    d.mleader_styles = pick(&src.mleader_styles, &used.mleader_styles, |t| &t.name);
    d.table_styles = pick(&src.table_styles, &used.table_styles, |t| &t.name);
    d.blocks = src.blocks.iter().filter(|(k, _)| used.blocks.contains(&up(k))).map(|(k, b)| (k.clone(), b.clone())).collect();
    d.layouts.clear();
    d
}

/// Add `defs`' table records and blocks that `d` doesn't have. Returns the blocks copied, from
/// their name in `defs` to their name in `d`.
pub(crate) fn merge(d: &mut Drawing, defs: &Drawing) -> BTreeMap<String, String> {
    fn add<T: Clone>(ours: &mut Vec<T>, theirs: &[T], name: fn(&T) -> &str) {
        for t in theirs {
            if !ours.iter().any(|o| name(o).eq_ignore_ascii_case(name(t))) {
                ours.push(t.clone());
            }
        }
    }
    add(&mut d.layers, &defs.layers, |l| &l.name);
    add(&mut d.linetypes, &defs.linetypes, |l| &l.name);
    add(&mut d.text_styles, &defs.text_styles, |t| &t.name);
    add(&mut d.dim_styles, &defs.dim_styles, |t| &t.name);
    add(&mut d.mleader_styles, &defs.mleader_styles, |t| &t.name);
    add(&mut d.table_styles, &defs.table_styles, |t| &t.name);
    let renames = block_names(d, defs);
    for (from, to) in &renames {
        let Some(b) = defs.blocks.get(from) else { continue };
        let mut nb = (**b).clone();
        nb.name = to.clone();
        nb.entities = Default::default();
        for e in b.entities.iter() {
            let e = adopt(d, e, &renames);
            d.ensure_layer(&e.common.layer);
            nb.entities.push(e);
        }
        d.blocks.insert(to.clone(), Arc::new(nb));
    }
    renames
}

/// Which blocks of `defs` to copy, and under which name: a new name keeps its own, a generated
/// (anonymous `*…`) block whose name is taken gets a fresh generated name, and a named block `d`
/// already has keeps `d`'s definition (not copied).
fn block_names(d: &Drawing, defs: &Drawing) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut taken: BTreeSet<String> = d.blocks.keys().map(|k| up(k)).collect();
    for (name, b) in &defs.blocks {
        let upper = up(name);
        if upper.starts_with("*MODEL_SPACE") || upper.starts_with("*PAPER_SPACE") {
            continue;
        }
        let to = if !taken.contains(&upper) {
            name.clone()
        } else if b.anonymous || name.starts_with('*') {
            let prefix: String = name.chars().take_while(|c| !c.is_ascii_digit()).collect();
            match (1..=1_000_000).map(|i| format!("{prefix}{i}")).find(|n| !taken.contains(&up(n))) {
                Some(n) => n,
                None => continue,
            }
        } else {
            continue;
        };
        taken.insert(up(&to));
        out.insert(name.clone(), to);
    }
    out
}

/// A copy of a clipboard object for `d`: a fresh handle, block references renamed, and links to
/// objects of the source drawing (associative dimension points) dropped.
pub(crate) fn adopt(d: &mut Drawing, e: &Entity, renames: &BTreeMap<String, String>) -> Entity {
    let mut e = e.clone();
    e.handle = d.new_handle();
    match &mut e.kind {
        EntityKind::Insert(i) => {
            if let Some(n) = renames.get(&i.block) {
                i.block = n.clone();
            }
        }
        EntityKind::Dimension(dm) => {
            dm.assoc.clear();
            if let Some(n) = dm.block.as_ref().and_then(|b| renames.get(b)) {
                dm.block = Some(n.clone());
            }
        }
        _ => {}
    }
    e
}
