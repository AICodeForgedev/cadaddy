//! AutoLISP dialogs at run time: `load_dialog` reads a DCL file ([`super::dcl`]), `new_dialog`
//! makes one of its dialogs, `start_dialog` shows it and waits. Each click or edit in the dialog
//! runs the tile's action (`action_tile`) inside the waiting routine, with `$key`, `$value`,
//! `$reason` and `$data` set, until an action calls `done_dialog`; `start_dialog` then returns
//! the status it gave.
//!
//! The interface draws the dialog the session describes ([`Session::lisp_dialog`]) and reports
//! what the user did ([`Session::dcl_event`]); programmatic runs answer it from a script of tile
//! actions (`lisp {code, dialog: [{key, value?}]}`), so a dialog runs headless.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::LispError;
use super::builtins::{arity, int, string};
use super::dcl::{self, DclFile, Tile};
use super::eval::{Ctx, Flow, Need};
use super::value::{Sym, Value};
use crate::Session;

type F = Result<Flow, LispError>;

/// Dialogs open at once (`new_dialog` inside an action), at most.
const MAX_OPEN: usize = 8;
/// DCL files loaded at once, at most.
const MAX_LOADED: usize = 64;
/// Items in one list tile, at most.
const MAX_LIST: usize = 100_000;
/// Drawing operations in one image tile, at most.
const MAX_IMAGE_OPS: usize = 100_000;
/// Events waiting for their actions, at most (a flood of clicks is dropped, not queued forever).
const MAX_PENDING: usize = 1_000;
/// Pixels per character of a tile's `width` and `height` (`dimx_tile`, the interface).
pub const CHAR_W: f64 = 7.0;
pub const CHAR_H: f64 = 15.0;

/// What an image tile shows, in its pixels (`fill_image`, `vector_image`, `slide_image`). Colours
/// are AutoCAD colour indexes; -2 is the dialog's background, -15 its foreground.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageOp {
    Fill {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        color: i32,
    },
    Vector {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        color: i32,
    },
    /// A slide: drawn as a framed name (slide files are not read).
    Slide {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        name: String,
    },
}

/// Something the user did in a dialog: a tile clicked or changed.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub key: String,
    /// The tile's new value (an edit box's text, a list's selection); `None` keeps it (buttons),
    /// or flips a toggle.
    pub value: Option<String>,
    /// `$reason`: 1 selected, 2 an edit box lost focus, 3 a slider is being dragged, 4 a list
    /// item double-clicked.
    pub reason: i32,
}

/// A dialog made by `new_dialog`: its tiles and their state.
#[derive(Clone, Debug, Default)]
pub struct OpenDialog {
    pub name: String,
    pub tile: Tile,
    /// Tile values, by key.
    pub values: BTreeMap<String, String>,
    /// Tiles `mode_tile` disabled.
    pub disabled: BTreeSet<String>,
    /// The tile `mode_tile` gave the focus.
    pub focus: Option<String>,
    /// List tiles' items (`start_list`).
    pub lists: BTreeMap<String, Vec<String>>,
    /// Image tiles' drawings (`start_image`).
    pub images: BTreeMap<String, Vec<ImageOp>>,
    actions: BTreeMap<String, String>,
    client_data: BTreeMap<String, String>,
    /// `new_dialog`'s action: for tiles with none of their own.
    default_action: Option<String>,
    started: bool,
    status: Option<i32>,
    pending: VecDeque<Event>,
}

impl OpenDialog {
    fn new(name: &str, tile: Tile, default_action: Option<String>) -> OpenDialog {
        let mut d = OpenDialog { name: name.to_string(), default_action, ..OpenDialog::default() };
        for t in tile.walk() {
            let Some(key) = t.key() else { continue };
            if let Some(v) = t.attr("value") {
                d.values.insert(key.to_string(), v.to_string());
            } else if matches!(t.kind.as_str(), "toggle" | "radio_button") {
                // Off unless the file says otherwise.
                d.values.insert(key.to_string(), "0".into());
            }
            if let Some(items) = t.attr("list") {
                d.lists.insert(key.to_string(), items.split('\n').map(str::to_string).collect());
            }
            // A radio cluster's value is the key of its button that is on.
            if t.kind.contains("radio_")
                && let Some(on) = t.children.iter().find(|c| c.kind == "radio_button" && c.attr("value") == Some("1")).and_then(Tile::key)
            {
                d.values.insert(key.to_string(), on.to_string());
            }
        }
        d.tile = tile;
        d
    }

    /// The tile with `key`.
    pub fn find(&self, key: &str) -> Option<&Tile> {
        find(&self.tile, key)
    }

    /// The radio cluster a radio button is in.
    fn cluster_of(&self, key: &str) -> Option<&Tile> {
        fn walk<'a>(t: &'a Tile, key: &str) -> Option<&'a Tile> {
            if t.kind.contains("radio_") && t.children.iter().any(|c| c.key() == Some(key)) {
                return Some(t);
            }
            t.children.iter().find_map(|c| walk(c, key))
        }
        walk(&self.tile, key)
    }

    /// The value of tile `key` ("" when it has none).
    pub fn value(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or("")
    }

    fn set_value(&mut self, key: &str, value: &str) {
        let is_radio = self.find(key).is_some_and(|t| t.kind == "radio_button");
        if is_radio && value == "1" {
            let cluster = self
                .cluster_of(key)
                .map(|c| (c.key().map(str::to_string), c.children.iter().filter_map(Tile::key).map(str::to_string).collect::<Vec<_>>()));
            if let Some((ckey, buttons)) = cluster {
                for b in buttons {
                    self.values.insert(b, "0".into());
                }
                if let Some(c) = ckey {
                    self.values.insert(c, key.to_string());
                }
            }
        }
        // Setting a radio cluster turns on the button it names.
        let cluster_buttons = self
            .find(key)
            .filter(|t| t.kind.contains("radio_"))
            .map(|t| t.children.iter().filter_map(Tile::key).map(str::to_string).collect::<Vec<_>>());
        if let Some(buttons) = cluster_buttons {
            for b in buttons {
                let on = if b == value { "1" } else { "0" };
                self.values.insert(b, on.into());
            }
        }
        self.values.insert(key.to_string(), value.to_string());
    }

    /// The action tile `key` runs: its own (`action_tile`, then its `action` attribute), its radio
    /// cluster's, OK's and Cancel's defaults, then `new_dialog`'s.
    fn action_for(&self, key: &str) -> Option<String> {
        if let Some(a) = self.actions.get(key) {
            return Some(a.clone());
        }
        let tile = self.find(key)?;
        if let Some(a) = tile.attr("action") {
            return Some(a.to_string());
        }
        if let Some(c) = self.cluster_of(key)
            && let Some(a) = c.key().and_then(|k| self.actions.get(k)).map(String::as_str).or_else(|| c.attr("action"))
        {
            return Some(a.to_string());
        }
        if key == "accept" {
            return Some("(done_dialog 1)".into());
        }
        if is_cancel(tile) {
            return Some("(done_dialog 0)".into());
        }
        self.default_action.clone()
    }

    /// Whether the dialog ended (`done_dialog`, `term_dialog`).
    pub fn done(&self) -> Option<i32> {
        self.status
    }
}

fn find<'a>(t: &'a Tile, key: &str) -> Option<&'a Tile> {
    if t.key() == Some(key) {
        return Some(t);
    }
    t.children.iter().find_map(|c| find(c, key))
}

fn is_cancel(t: &Tile) -> bool {
    t.flag("is_cancel") || t.key() == Some("cancel")
}

/// A list or image being filled (`start_list` … `end_list`, `start_image` … `end_image`).
#[derive(Clone, Debug)]
enum Building {
    /// `op`: 1 change the item at `index`, 2 append, 3 a new list.
    List {
        key: String,
        op: i32,
        index: usize,
    },
    Image {
        key: String,
    },
}

/// The session's dialogs: loaded DCL files and the dialogs open, innermost last.
#[derive(Debug, Default)]
pub(crate) struct DialogState {
    files: BTreeMap<i32, DclFile>,
    next: i32,
    pub open: Vec<OpenDialog>,
    building: Option<Building>,
}

impl DialogState {
    /// Close every dialog (the routine ended).
    pub fn close_all(&mut self) {
        self.open.clear();
        self.building = None;
    }
}

/// What the machine does next at a `start_dialog` that waits.
pub enum Step {
    /// The dialog ended: `start_dialog` returns this status.
    Done(Value),
    /// Run an action's forms, then come back.
    Run(Vec<Value>),
    /// Wait for the user.
    Wait,
}

/// The next step of the innermost started dialog.
pub fn step(ctx: &mut Ctx<'_>) -> Result<Step, LispError> {
    let dl = &mut ctx.s.lisp.dcl;
    let Some(d) = dl.open.last_mut() else { return Ok(Step::Done(Value::Int(-1))) };
    if let Some(st) = d.status {
        dl.open.pop();
        dl.building = None;
        return Ok(Step::Done(Value::Int(st)));
    }
    while let Some(ev) = d.pending.pop_front() {
        let Some(src) = d.action_for(&ev.key) else { continue };
        let value = d.value(&ev.key).to_string();
        let data = d.client_data.get(&ev.key).cloned();
        let forms = super::read::read_all(&src).map_err(|e| LispError::new(format!("action for \"{}\": {}", ev.key, e.msg)))?;
        let set = |env: &mut super::Env, name: &str, v: Value| env.set(&Sym::new(name), v);
        set(ctx.env, "$KEY", Value::str(&ev.key));
        set(ctx.env, "$VALUE", Value::str(&value));
        set(ctx.env, "$REASON", Value::Int(ev.reason));
        set(ctx.env, "$DATA", data.map(|d| Value::str(&d)).unwrap_or_default());
        set(ctx.env, "$X", Value::Int(0));
        set(ctx.env, "$Y", Value::Int(0));
        return Ok(Step::Run(forms));
    }
    Ok(Step::Wait)
}

/// The dialog `set_tile` and friends act on: the innermost one.
fn current<'a>(ctx: &'a mut Ctx<'_>, f: &str) -> Result<&'a mut OpenDialog, LispError> {
    ctx.s.lisp.dcl.open.last_mut().ok_or_else(|| LispError::new(format!("{f}: no dialog is open")))
}

/// What `load_dialog` reads: an APPLOAD-ed file, or one on disk or in the support folder, with
/// `.dcl` added when the name has no extension.
fn dcl_source(s: &Session, name: &str) -> Option<String> {
    let name = if std::path::Path::new(name).extension().is_none() { format!("{name}.dcl") } else { name.to_string() };
    if let Some(t) = s.lisp_file(&name) {
        return Some(t);
    }
    let support = s.lisp_support_dir().map(|d| d.join(&name).to_string_lossy().into_owned());
    std::iter::once(name.clone()).chain(support).find_map(|c| super::builtins::read_text_file(&c))
}

/// `(load_dialog file)`: a positive id, or a negative number when the file can't be read.
pub fn load_dialog(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let name = string(&a[0])?;
    let Some(src) = dcl_source(ctx.s, &name) else { return Ok(Flow::Value(Value::Int(-1))) };
    if src.len() > dcl::MAX_SOURCE {
        return Ok(Flow::Value(Value::Int(-1)));
    }
    let s = &*ctx.s;
    let parsed = dcl::parse(&src, &mut |inc| dcl_source(s, inc).filter(|t| t.len() <= dcl::MAX_SOURCE));
    let file = match parsed {
        Ok(f) => f,
        Err(e) => {
            ctx.env.out.push_str(&format!("\n; DCL error in {name}: {e}\n"));
            return Ok(Flow::Value(Value::Int(-1)));
        }
    };
    let dl = &mut ctx.s.lisp.dcl;
    if dl.files.len() >= MAX_LOADED {
        return Err(LispError::new(format!("load_dialog: more than {MAX_LOADED} DCL files loaded")));
    }
    dl.next = dl.next.checked_add(1).unwrap_or(1);
    let id = dl.next;
    dl.files.insert(id, file);
    Ok(Flow::Value(Value::Int(id)))
}

/// `(unload_dialog id)`.
pub fn unload_dialog(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    ctx.s.lisp.dcl.files.remove(&int(&a[0])?);
    Ok(Flow::Value(Value::Nil))
}

/// `(new_dialog name id [default-action [screen-point]])`: T, or nil when there is no such dialog.
pub fn new_dialog(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 4)?;
    let name = string(&a[0])?;
    let id = int(&a[1])?;
    let action = match a.get(2) {
        Some(Value::Str(s)) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    };
    let Some(file) = ctx.s.lisp.dcl.files.get(&id) else { return Ok(Flow::Value(Value::Nil)) };
    let tile = match file.dialog(&name) {
        Ok(t) => t,
        Err(e) => {
            ctx.env.out.push_str(&format!("\n; new_dialog: {e}\n"));
            return Ok(Flow::Value(Value::Nil));
        }
    };
    if ctx.s.lisp.dcl.open.len() >= MAX_OPEN {
        return Err(LispError::new(format!("new_dialog: more than {MAX_OPEN} dialogs open")));
    }
    ctx.s.lisp.dcl.open.push(OpenDialog::new(&name, tile, action));
    Ok(Flow::Value(Value::t()))
}

/// `(start_dialog)`: show the dialog `new_dialog` made and wait until an action ends it.
pub fn start_dialog(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 0, 0)?;
    match ctx.s.lisp.dcl.open.last_mut() {
        Some(d) if !d.started => {
            d.started = true;
            Ok(Flow::Dialog)
        }
        _ => Ok(Flow::Value(Value::Int(-1))),
    }
}

/// `(done_dialog [status])`: end the dialog; `start_dialog` returns `status` (default 1, or 0
/// from a Cancel tile's action).
pub fn done_dialog(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 0, 1)?;
    let key = ctx.env.get("$KEY");
    let d = current(ctx, "done_dialog")?;
    let status = match a.first() {
        Some(v) => int(v)?,
        None => match key {
            Value::Str(k) if d.find(&k).is_some_and(is_cancel) => 0,
            _ => 1,
        },
    };
    d.status = Some(status);
    Ok(Flow::Value(Value::point(&[0.0, 0.0])))
}

/// `(term_dialog)`: end every open dialog; their `start_dialog`s return -1.
pub fn term_dialog(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 0, 0)?;
    let dl = &mut ctx.s.lisp.dcl;
    dl.open.retain(|d| d.started);
    for d in &mut dl.open {
        d.status = Some(-1);
    }
    Ok(Flow::Value(Value::Nil))
}

/// `(action_tile key action)`.
pub fn action_tile(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let (key, action) = (string(&a[0])?, string(&a[1])?);
    let d = current(ctx, "action_tile")?;
    if d.find(&key).is_none() {
        return Ok(Flow::Value(Value::Nil));
    }
    d.actions.insert(key.to_string(), action.to_string());
    Ok(Flow::Value(Value::t()))
}

/// `(set_tile key value)`.
pub fn set_tile(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let (key, value) = (string(&a[0])?, string(&a[1])?);
    let d = current(ctx, "set_tile")?;
    if d.find(&key).is_some() {
        d.set_value(&key, &value);
    }
    Ok(Flow::Value(Value::Str(value)))
}

/// `(get_tile key)`: the tile's value, nil when there is no such tile.
pub fn get_tile(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let key = string(&a[0])?;
    let d = current(ctx, "get_tile")?;
    Ok(Flow::Value(if d.find(&key).is_some() { Value::str(d.value(&key)) } else { Value::Nil }))
}

/// `(get_attr key attribute)`: an attribute as the DCL file gave it.
pub fn get_attr(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let (key, attr) = (string(&a[0])?, string(&a[1])?);
    let d = current(ctx, "get_attr")?;
    Ok(Flow::Value(d.find(&key).and_then(|t| t.attr(&attr.to_ascii_lowercase())).map(Value::str).unwrap_or_default()))
}

/// `(mode_tile key mode)`: 0 enable, 1 disable, 2 focus, 3 select the text, 4 flip highlight.
pub fn mode_tile(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let (key, mode) = (string(&a[0])?, int(&a[1])?);
    let d = current(ctx, "mode_tile")?;
    if d.find(&key).is_some() {
        match mode {
            0 => {
                d.disabled.remove(&*key);
            }
            1 => {
                d.disabled.insert(key.to_string());
            }
            2 | 3 => d.focus = Some(key.to_string()),
            _ => {}
        }
    }
    Ok(Flow::Value(Value::Nil))
}

/// `(client_data_tile key data)`: what `$data` holds in the tile's action.
pub fn client_data_tile(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 2, 2)?;
    let (key, data) = (string(&a[0])?, string(&a[1])?);
    let d = current(ctx, "client_data_tile")?;
    if d.find(&key).is_some() {
        d.client_data.insert(key.to_string(), data.to_string());
    }
    Ok(Flow::Value(Value::Nil))
}

/// `(start_list key [operation [index]])`: 1 change an item, 2 append, 3 a new list (default).
pub fn start_list(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 3)?;
    let key = string(&a[0])?;
    let op = a.get(1).map(int).transpose()?.unwrap_or(3);
    let index = a.get(2).map(int).transpose()?.unwrap_or(0);
    let d = current(ctx, "start_list")?;
    if d.find(&key).is_none() {
        return Ok(Flow::Value(Value::Nil));
    }
    if op == 3 {
        d.lists.insert(key.to_string(), Vec::new());
    }
    let index = usize::try_from(index).unwrap_or(0);
    ctx.s.lisp.dcl.building = Some(Building::List { key: key.to_string(), op, index });
    Ok(Flow::Value(Value::Str(key)))
}

/// `(add_list string)`.
pub fn add_list(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let item = string(&a[0])?;
    let dl = &mut ctx.s.lisp.dcl;
    let (Some(Building::List { key, op, index }), Some(d)) = (dl.building.as_mut(), dl.open.last_mut()) else {
        return Ok(Flow::Value(Value::Nil));
    };
    let list = d.lists.entry(key.clone()).or_default();
    if *op == 1 {
        match list.get_mut(*index) {
            Some(slot) => *slot = item.to_string(),
            None => return Ok(Flow::Value(Value::Nil)),
        }
        *index += 1;
    } else {
        if list.len() >= MAX_LIST {
            return Err(LispError::new(format!("add_list: more than {MAX_LIST} items")));
        }
        list.push(item.to_string());
    }
    Ok(Flow::Value(Value::Str(item)))
}

/// `(end_list)`, `(end_image)`.
pub fn end_building(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 0, 0)?;
    ctx.s.lisp.dcl.building = None;
    Ok(Flow::Value(Value::Nil))
}

/// `(start_image key)`.
pub fn start_image(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 1, 1)?;
    let key = string(&a[0])?;
    let d = current(ctx, "start_image")?;
    if d.find(&key).is_none() {
        return Ok(Flow::Value(Value::Nil));
    }
    d.images.entry(key.to_string()).or_default();
    ctx.s.lisp.dcl.building = Some(Building::Image { key: key.to_string() });
    Ok(Flow::Value(Value::Str(key)))
}

fn push_image(ctx: &mut Ctx<'_>, op: ImageOp) -> Result<bool, LispError> {
    let dl = &mut ctx.s.lisp.dcl;
    let (Some(Building::Image { key }), Some(d)) = (dl.building.as_ref(), dl.open.last_mut()) else { return Ok(false) };
    let ops = d.images.entry(key.clone()).or_default();
    if ops.len() >= MAX_IMAGE_OPS {
        return Err(LispError::new(format!("an image tile holds at most {MAX_IMAGE_OPS} drawing operations")));
    }
    ops.push(op);
    Ok(true)
}

/// `(fill_image x y width height color)`.
pub fn fill_image(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 5, 5)?;
    let n: Vec<i32> = a.iter().map(int).collect::<Result<_, _>>()?;
    let op = ImageOp::Fill { x: n[0], y: n[1], w: n[2], h: n[3], color: n[4] };
    Ok(Flow::Value(if push_image(ctx, op)? { Value::Int(n[4]) } else { Value::Nil }))
}

/// `(vector_image x1 y1 x2 y2 color)`.
pub fn vector_image(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 5, 5)?;
    let n: Vec<i32> = a.iter().map(int).collect::<Result<_, _>>()?;
    let op = ImageOp::Vector { x1: n[0], y1: n[1], x2: n[2], y2: n[3], color: n[4] };
    Ok(Flow::Value(if push_image(ctx, op)? { Value::Int(n[4]) } else { Value::Nil }))
}

/// `(slide_image x y width height slide)`.
pub fn slide_image(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    arity(&a, 5, 5)?;
    let n: Vec<i32> = a[..4].iter().map(int).collect::<Result<_, _>>()?;
    let name = string(&a[4])?;
    let op = ImageOp::Slide { x: n[0], y: n[1], w: n[2], h: n[3], name: name.to_string() };
    Ok(Flow::Value(if push_image(ctx, op)? { Value::Str(name) } else { Value::Nil }))
}

/// A tile's size in pixels: its `width`/`height` in characters times the character cell.
pub fn tile_px(t: &Tile, attr: &str, cell: f64) -> i32 {
    let chars = t.attr(attr).and_then(|w| w.trim().parse::<f64>().ok()).filter(|w| w.is_finite()).unwrap_or(0.0).clamp(0.0, 1000.0);
    // In range by the clamp: at most 1000 × 15 pixels.
    (chars * cell).round() as i32
}

/// `(dimx_tile key)`: an image tile's width in pixels.
pub fn dimx_tile(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    dim(ctx, a, "width", CHAR_W)
}

/// `(dimy_tile key)`: its height.
pub fn dimy_tile(ctx: &mut Ctx<'_>, a: Vec<Value>) -> F {
    dim(ctx, a, "height", CHAR_H)
}

fn dim(ctx: &mut Ctx<'_>, a: Vec<Value>, attr: &str, cell: f64) -> F {
    arity(&a, 1, 1)?;
    let key = string(&a[0])?;
    let d = current(ctx, "dimx_tile")?;
    Ok(Flow::Value(d.find(&key).map(|t| Value::Int(tile_px(t, attr, cell))).unwrap_or_default()))
}

// ---------------- the session side ----------------

impl Session {
    /// The dialog a routine waits in, for the interface to draw.
    pub fn lisp_dialog(&self) -> Option<&OpenDialog> {
        let waiting = self.lisp.run.as_ref().is_some_and(|r| matches!(r.waiting, Some(Need::Dialog)));
        if !waiting {
            return None;
        }
        self.lisp.dcl.open.last().filter(|d| d.started && d.status.is_none())
    }

    /// The user acted on tile `key` of the waiting dialog: its value changes (when given) and its
    /// action runs. An error names a tile that isn't there or is disabled.
    pub fn dcl_event(&mut self, ev: Event) -> std::result::Result<(), String> {
        queue_event(&mut self.lisp.dcl, ev)?;
        self.dcl_resume();
        Ok(())
    }

    /// A tile's value changes without running its action (typing in an edit box).
    pub fn dcl_set_value(&mut self, key: &str, value: &str) -> std::result::Result<(), String> {
        let d = self.lisp.dcl.open.last_mut().filter(|d| d.started).ok_or("no dialog is open")?;
        if d.find(key).is_none() {
            return Err(format!("no tile \"{key}\" in the dialog"));
        }
        d.set_value(key, value);
        Ok(())
    }

    /// Esc, or the dialog's close box: its Cancel tile's action, or status 0 when it has none.
    pub fn dcl_cancel(&mut self) {
        let Some(d) = self.lisp.dcl.open.last_mut() else { return };
        match d.tile.walk().into_iter().find(|t| is_cancel(t)).and_then(Tile::key).map(str::to_string) {
            Some(key) if d.action_for(&key).is_some() => d.pending.push_back(Event { key, value: None, reason: 1 }),
            _ => d.status = Some(0),
        }
        self.dcl_resume();
    }

    /// Carry on a routine waiting in a dialog that has something to do.
    fn dcl_resume(&mut self) {
        if let Some(r) = self.lisp.run.as_mut()
            && matches!(r.waiting, Some(Need::Dialog))
        {
            r.waiting = None;
            r.machine.resume(Value::Nil);
            self.lisp_drive();
        }
    }
}

/// Record a user's action on the innermost dialog: the tile's value, then its action to run.
pub(crate) fn queue_event(dl: &mut DialogState, ev: Event) -> std::result::Result<(), String> {
    let d = dl.open.last_mut().filter(|d| d.started).ok_or("no dialog is open")?;
    let tile = d.find(&ev.key).ok_or_else(|| format!("no tile \"{}\" in the dialog", ev.key))?;
    if d.disabled.contains(&ev.key) {
        return Err(format!("tile \"{}\" is disabled", ev.key));
    }
    let kind = tile.kind.clone();
    match (kind.as_str(), &ev.value) {
        ("toggle", None) => {
            let on = d.value(&ev.key) == "1";
            d.set_value(&ev.key, if on { "0" } else { "1" });
        }
        ("radio_button", _) => d.set_value(&ev.key, "1"),
        (_, Some(v)) => d.set_value(&ev.key, v),
        _ => {}
    }
    if d.pending.len() >= MAX_PENDING {
        return Err("too many dialog events waiting".into());
    }
    d.pending.push_back(ev);
    Ok(())
}

/// A routine with no one to answer its dialog (a programmatic run whose script ran out): the
/// dialog is cancelled with status 0.
pub(crate) fn cancel_unanswered(dl: &mut DialogState) {
    if let Some(d) = dl.open.last_mut() {
        d.status = Some(0);
    }
}

/// The waiting dialog as JSON (`lisp.dialog`): its tiles, values, lists and what is disabled.
pub fn to_json(d: &OpenDialog) -> serde_json::Value {
    fn tile(t: &Tile, d: &OpenDialog) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        o.insert("type".into(), t.kind.clone().into());
        for (k, v) in &t.attrs {
            o.insert(k.clone(), v.clone().into());
        }
        if let Some(key) = t.key() {
            o.insert("value".into(), d.value(key).into());
            if let Some(items) = d.lists.get(key) {
                o.insert("items".into(), items.clone().into());
            }
            if d.disabled.contains(key) {
                o.insert("disabled".into(), true.into());
            }
        }
        if !t.children.is_empty() {
            o.insert("tiles".into(), t.children.iter().map(|c| tile(c, d)).collect::<Vec<_>>().into());
        }
        o.into()
    }
    serde_json::json!({ "dialog": d.name, "tile": tile(&d.tile, d) })
}
