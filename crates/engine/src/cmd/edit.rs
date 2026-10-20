//! Edit menu: undo/redo, clipboard, selection.

use cadaddy_doc::{Entity, Handle, entity_bounds};
use cadaddy_geom::{Bounds2, Mat3, Vec2};
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase, SelectRun};
use super::*;
use crate::undoctl::Control;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // U undoes the last operation at once (Edit > Undo, Cmd+Z); UNDO asks how many.
        CommandSpec::new("u", "Undo", run_u).menu(&["Edit", "Undo"]).key("Cmd+Z").params("{}").noundo(),
        CommandSpec::new("undo", "Undo", run_undo)
            .params(
                "{count?: n} | {mode: mark|begin|end} | {mode: back, confirm?: bool (no mark: undo everything; default true)} | {mode: auto, on: bool} | {mode: control, control: all|none|one|combine|layer, on?: bool (combine/layer)}",
            )
            .noundo()
            .interactive(|_| Ok(Box::new(UndoM::default()))),
        CommandSpec::new("redo", "Redo", run_redo).menu(&["Edit", "Redo"]).key("Cmd+Shift+Z").params("{count?: n}").noundo(),
        CommandSpec::new("mredo", "Redo", run_mredo).params("{count?: n} | {all: true}").noundo().interactive(|_| Ok(Box::new(MredoM))),
        CommandSpec::new("cutclip", "Cut", run_cut)
            .menu(&["Edit", "Cut"])
            .key("Cmd+X")
            .params("{handles?}")
            .interactive(|_| Ok(Box::new(SelectRun::new("cutclip", "CUTCLIP")))),
        CommandSpec::new("copyclip", "Copy", run_copyclip)
            .menu(&["Edit", "Copy"])
            .key("Cmd+C")
            .params("{handles?}")
            .noundo()
            .interactive(|_| Ok(Box::new(SelectRun::new("copyclip", "COPYCLIP")))),
        CommandSpec::new("copybase", "Copy with Base Point", run_copybase)
            .menu(&["Edit", "Copy with Base Point"])
            .key("Cmd+Shift+C")
            .params("{handles?, base: [x,y]}")
            .noundo()
            .interactive(|_| Ok(Box::new(CopyBaseM::default()))),
        CommandSpec::new("pasteclip", "Paste", run_paste)
            .menu(&["Edit", "Paste"])
            .key("Cmd+V")
            .params("{at?: [x,y]} (where the base point lands; default: the original coordinates)")
            .enabled(clipboard::has_paste)
            .interactive(|_| Ok(Box::new(PasteM { mode: PasteMode::Clip }))),
        CommandSpec::new("pastespec", "Paste Special", run_pastespec)
            .menu(&["Edit", "Paste Special..."])
            .alias(&["pa"])
            .params(
                "{as?: \"objects\" (CADaddy objects, the default when there are some) | \"image\" (a raster image: the picture copied \
                 in another program, or the copied objects drawn as one; saved as a PNG beside the drawing) | \"text\" (text copied \
                 in another program, as multiline text), at?: [x,y] (objects: where the base point lands; image: lower-left corner; \
                 text: insertion point), width?: the image's width in drawing units (default: half the view tall)}",
            )
            .enabled(clipboard::has_paste)
            .interactive(|_| Ok(Box::new(PasteSpecM::default()))),
        CommandSpec::new("pasteblock", "Paste as Block", run_pasteblock)
            .menu(&["Edit", "Paste as Block"])
            .key("Cmd+Shift+V")
            .params("{at?: [x,y]} (insertion point; default: the original coordinates)")
            .enabled(clipboard::has_clip)
            .interactive(|_| Ok(Box::new(PasteM { mode: PasteMode::Block }))),
        CommandSpec::new("pasteorig", "Paste to Original Coordinates", run_pasteorig)
            .menu(&["Edit", "Paste to Original Coordinates"])
            .params("{} (only into a drawing other than the one copied from)")
            .enabled(clipboard::can_paste_orig)
            .interactive(|_| Ok(Box::new(PasteM { mode: PasteMode::Orig }))),
        CommandSpec::new("erase.selection", "Clear", run_clear).menu(&["Edit", "Clear"]).key("Delete").enabled(has_selection).params("{}"),
        CommandSpec::new("selectall", "Select All", run_selectall).menu(&["Edit", "Select All"]).key("Cmd+A").alias(&["ai_selall"]).noundo(),
        CommandSpec::new("ai_deselect", "Deselect All", run_deselect).params("{}").noundo(),
        CommandSpec::new("select", "Select", run_select)
            .params(
                "{handles: [hex]} | {window: [[x,y],[x,y]], crossing?: bool} | {lasso: [[x,y],...], mode?: \"window\"|\"crossing\"|\"fence\" (default: rightward window, leftward crossing)} | {at: [x,y], cycle?: n (the nth of the overlapping objects there, nearest first; the reply lists them as candidates)} | {fence: [[x,y],...]} | {class: \"LINE,AcDbCircle,*Text\" (classes, DXF types or type names, wildcards)} | {all: true} | {clear: true} | {add?: bool}",
            )
            .noundo()
            .interactive(|_| Ok(Box::new(SelectM::default()))),
    ]
}

/// A programmatic undo/redo (`engine.execute`, MCP, control channel, scripts) first ends a running
/// command the way Esc does, exactly as `Session::start` does for Cmd+Z, the menu or the command
/// line. Otherwise it would swap the drawing under the command and pop the previous command's step.
fn end_running_command(s: &mut Session) {
    if s.running.is_some() {
        s.cancel();
    }
}

/// The UNDO options of the JSON form (`mode`) and the command line (keywords).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UndoMode {
    Count,
    Mark,
    Back,
    Begin,
    End,
    Auto,
    Control,
}

fn undo_mode(m: &str) -> Option<UndoMode> {
    Some(match m.trim().to_ascii_lowercase().as_str() {
        "" | "count" => UndoMode::Count,
        "mark" => UndoMode::Mark,
        "back" => UndoMode::Back,
        "begin" | "group" => UndoMode::Begin,
        "end" => UndoMode::End,
        "auto" => UndoMode::Auto,
        "control" => UndoMode::Control,
        _ => return None,
    })
}

fn on_param(p: &Value, key: &str) -> Option<bool> {
    let v = p.get(key)?;
    v.as_bool().or_else(|| v.as_i64().map(|i| i != 0)).or_else(|| match v.as_str()?.trim().to_ascii_lowercase().as_str() {
        "on" | "yes" | "y" | "true" | "1" => Some(true),
        "off" | "no" | "n" | "false" | "0" => Some(false),
        _ => None,
    })
}

/// The JSON result of an UNDO option: `message` plus the new UNDOCTL bits and mark count.
fn undo_state(s: &Session, message: impl Into<String>) -> Result<Value> {
    let st = s.state()?;
    Ok(json!({ "message": message.into(), "undoctl": st.undo_ctl.bits(), "marks": st.undo_marks() }))
}

fn undo_off() -> EngineError {
    EngineError::Other("Undo is turned off (UNDO Control None); UNDO Control All turns it on.".into())
}

fn run_undo(s: &mut Session, p: &Value) -> Result<Value> {
    end_running_command(s);
    let mode = match p.get("mode") {
        None | Some(Value::Null) => UndoMode::Count,
        Some(v) => v.as_str().and_then(undo_mode).ok_or_else(|| bad("undo", "mode: count, mark, back, begin, end, auto or control"))?,
    };
    let control = s.state()?.undo_ctl.control;
    if control != Control::All && !matches!(mode, UndoMode::Count | UndoMode::Control | UndoMode::Auto) {
        return Err(EngineError::Other(format!("UNDO {mode:?} needs UNDO Control All.")));
    }
    match mode {
        UndoMode::Count => {
            if control == Control::None {
                return Err(undo_off());
            }
            let n = p.get("count").and_then(Value::as_u64).unwrap_or(1).clamp(1, 1000);
            // Undoing inside a BEgin group ends it: the whole group is undone as one step.
            s.state_mut()?.end_undo_group();
            let mut labels = Vec::new();
            for _ in 0..n {
                match s.undo()? {
                    Some(l) => labels.push(l),
                    None => break,
                }
            }
            if labels.is_empty() {
                return Ok(json!({ "message": "Everything has been undone" }));
            }
            Ok(json!({ "undone": labels, "message": labels.join(", ").to_string() }))
        }
        UndoMode::Mark => {
            s.state_mut()?.undo_mark();
            undo_state(s, "Mark set")
        }
        UndoMode::Back => undo_back(s, on_param(p, "confirm").unwrap_or(true)),
        UndoMode::Begin => {
            let ended = s.state_mut()?.begin_undo_group();
            undo_state(s, if ended { "Group ended; new group begun" } else { "Group begun" })
        }
        UndoMode::End => {
            let ended = s.state_mut()?.end_undo_group();
            undo_state(s, if ended { "Group ended" } else { "No group to end" })
        }
        UndoMode::Auto => {
            let on = on_param(p, "on").ok_or_else(|| bad("undo", "`on` (true/false) is required for mode auto"))?;
            s.state_mut()?.undo_ctl.auto = on;
            undo_state(s, format!("UNDO Auto {}", if on { "on" } else { "off" }))
        }
        UndoMode::Control => {
            let c = str_param(p, "control").unwrap_or("all").trim().to_ascii_lowercase();
            let st = s.state_mut()?;
            let msg = match c.as_str() {
                "all" => {
                    st.set_undo_control(Control::All);
                    "UNDO Control All"
                }
                "none" => {
                    st.set_undo_control(Control::None);
                    "UNDO Control None: undo is off"
                }
                "one" => {
                    st.set_undo_control(Control::One);
                    "UNDO Control One: only the last step can be undone"
                }
                "combine" => {
                    st.undo_ctl.combine_view = on_param(p, "on").unwrap_or(true);
                    "UNDO Control Combine set"
                }
                "layer" => {
                    st.undo_ctl.combine_layer = on_param(p, "on").unwrap_or(true);
                    "UNDO Control Layer set"
                }
                _ => return Err(bad("undo", "control: all, none, one, combine or layer")),
            };
            undo_state(s, msg)
        }
    }
}

/// UNDO Back: undo to the last mark (which goes with it), or with no mark everything when
/// `confirm` is true.
fn undo_back(s: &mut Session, confirm: bool) -> Result<Value> {
    let st = s.state_mut()?;
    st.end_undo_group();
    let depth = st.undo.len();
    st.undo_ctl.marks.retain(|m| *m <= depth);
    let target = match st.undo_ctl.marks.pop() {
        Some(m) => m,
        None if confirm => 0,
        None => return undo_state(s, "Nothing undone"),
    };
    let mut labels = Vec::new();
    // Each step pops one entry, so this ends within the stack's depth.
    while s.state()?.undo.len() > target {
        match s.undo()? {
            Some(l) => labels.push(l),
            None => break,
        }
    }
    let msg = match (labels.is_empty(), target) {
        (true, 0) => "Everything has been undone".to_string(),
        (true, _) => "Back at the mark".to_string(),
        (false, _) => labels.join(", "),
    };
    let mut r = undo_state(s, msg)?;
    r["undone"] = json!(labels);
    Ok(r)
}

fn run_u(s: &mut Session, _p: &Value) -> Result<Value> {
    run_undo(s, &json!({}))
}

/// What the UNDO prompt machine asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum UndoAsk {
    /// "Enter the number of operations to undo or [Auto/Control/BEgin/End/Mark/Back]".
    #[default]
    Main,
    Auto,
    Control,
    Combine,
    Layer,
    /// Back with no mark: undo everything?
    BackAll,
}

/// UNDO at the command line: a number of operations (default 1) or Auto, Control (All, None, One,
/// Combine, Layer), BEgin, End, Mark or Back. With Control None only Control is offered, with One
/// a count or Control.
#[derive(Default)]
struct UndoM {
    ask: UndoAsk,
}

impl UndoM {
    fn finish(s: &mut Session, r: Result<Value>) -> Result<Step> {
        match r {
            Ok(v) => {
                if let Some(m) = v.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
            }
            Err(e) => s.echo(e.to_string()),
        }
        Ok(Step::Done)
    }
}

impl Interactive for UndoM {
    fn name(&self) -> &'static str {
        "UNDO"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        let control = s.state().map(|st| st.undo_ctl.control).unwrap_or_default();
        let ask = if control == Control::None && self.ask == UndoAsk::Main { UndoAsk::Control } else { self.ask };
        let yes_no = |b: bool| if b { "Yes" } else { "No" };
        let ctl = s.state().map(|st| st.undo_ctl.clone()).unwrap_or_default();
        match ask {
            UndoAsk::Auto => Prompt::new("Enter UNDO Auto mode", Accept::TEXT).kw(&["ON", "OFF"]).default(if ctl.auto { "ON" } else { "OFF" }),
            UndoAsk::Control => {
                Prompt::new("Enter an UNDO control option", Accept::TEXT).kw(&["All", "None", "One", "Combine", "Layer"]).default("All")
            }
            UndoAsk::Combine => {
                Prompt::new("Combine zooms and pans into one undo step?", Accept::TEXT).kw(&["Yes", "No"]).default(yes_no(ctl.combine_view))
            }
            UndoAsk::Layer => {
                Prompt::new("Combine layer property changes into one undo step?", Accept::TEXT).kw(&["Yes", "No"]).default(yes_no(ctl.combine_layer))
            }
            UndoAsk::BackAll => Prompt::new("No mark: undo everything?", Accept::TEXT).kw(&["Yes", "No"]).default("Yes"),
            UndoAsk::Main if control == Control::One => {
                Prompt::new("Enter the number of operations to undo", Accept::NUMBER).kw(&["Control"]).default("1")
            }
            UndoAsk::Main => Prompt::new("Enter the number of operations to undo", Accept::NUMBER)
                .kw(&["Auto", "Control", "BEgin", "End", "Mark", "Back"])
                .default("1"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let prompt = self.prompt(s);
        if s.state()?.undo_ctl.control == Control::None && self.ask == UndoAsk::Main {
            self.ask = UndoAsk::Control;
        }
        let kw = match &i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Enter => prompt.default.clone(),
            Input::Keyword(k) | Input::Text(k) => prompt.match_keyword(k),
            _ => return Ok(Step::Continue),
        };
        let yes = kw.as_deref() == Some("Yes");
        match self.ask {
            UndoAsk::Main => {
                // Enter takes the default count, not a keyword.
                if let Some(k) = kw.as_deref().filter(|_| !matches!(i, Input::Enter)) {
                    match k {
                        "Auto" => self.ask = UndoAsk::Auto,
                        "Control" => self.ask = UndoAsk::Control,
                        "Back" if s.state()?.undo_marks() == 0 => self.ask = UndoAsk::BackAll,
                        _ => {
                            let r = run_undo(s, &json!({ "mode": k.to_ascii_lowercase() }));
                            return Self::finish(s, r);
                        }
                    }
                    return Ok(Step::Continue);
                }
                let n = match &i {
                    Input::Text(t) => {
                        t.trim().parse::<u64>().ok().filter(|n| *n >= 1).ok_or_else(|| EngineError::Other("Requires a positive integer.".into()))?
                    }
                    _ => 1,
                };
                let r = run_undo(s, &json!({ "count": n }));
                Self::finish(s, r)
            }
            UndoAsk::Auto => {
                let on = match kw.as_deref() {
                    Some("ON") => true,
                    Some("OFF") => false,
                    _ => return Err(EngineError::Other("Enter ON or OFF.".into())),
                };
                let r = run_undo(s, &json!({ "mode": "auto", "on": on }));
                Self::finish(s, r)
            }
            UndoAsk::Control => match kw.as_deref() {
                Some("Combine") => {
                    self.ask = UndoAsk::Combine;
                    Ok(Step::Continue)
                }
                Some("Layer") => {
                    self.ask = UndoAsk::Layer;
                    Ok(Step::Continue)
                }
                Some(c) => {
                    let r = run_undo(s, &json!({ "mode": "control", "control": c.to_ascii_lowercase() }));
                    Self::finish(s, r)
                }
                None => Err(EngineError::Other("Enter All, None, One, Combine or Layer.".into())),
            },
            UndoAsk::Combine | UndoAsk::Layer => {
                if kw.is_none() {
                    return Err(EngineError::Other("Answer Yes or No.".into()));
                }
                let control = if self.ask == UndoAsk::Combine { "combine" } else { "layer" };
                let r = run_undo(s, &json!({ "mode": "control", "control": control, "on": yes }));
                Self::finish(s, r)
            }
            UndoAsk::BackAll => {
                if kw.is_none() {
                    return Err(EngineError::Other("Answer Yes or No.".into()));
                }
                let r = run_undo(s, &json!({ "mode": "back", "confirm": yes }));
                Self::finish(s, r)
            }
        }
    }
}

/// Redo up to `n` undone steps.
fn redo_steps(s: &mut Session, n: u64) -> Result<Value> {
    end_running_command(s);
    let mut labels = Vec::new();
    for _ in 0..n {
        match s.redo()? {
            Some(l) => labels.push(l),
            None => break,
        }
    }
    Ok(json!({ "redone": labels, "message": if labels.is_empty() { "Nothing to redo".to_string() } else { labels.join(", ") } }))
}

fn run_redo(s: &mut Session, p: &Value) -> Result<Value> {
    redo_steps(s, p.get("count").and_then(Value::as_u64).unwrap_or(1).clamp(1, 1000))
}

/// MREDO: `{count}` steps, or `{all: true}` every undone step.
fn run_mredo(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("all").and_then(Value::as_bool).unwrap_or(false) {
        let n = s.state()?.redo.len() as u64;
        return redo_steps(s, n.max(1));
    }
    run_redo(s, p)
}

/// MREDO at the command line: a number of actions, All or Last.
struct MredoM;

impl Interactive for MredoM {
    fn name(&self) -> &'static str {
        "MREDO"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Enter number of actions", Accept::NUMBER).kw(&["All", "Last"])
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let p = match &i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Enter => return Ok(Step::Done),
            Input::Keyword(k) | Input::Text(k) => match self.prompt(s).match_keyword(k).as_deref() {
                Some("All") => json!({ "all": true }),
                Some(_) => json!({ "count": 1 }),
                None => {
                    let n =
                        k.trim().parse::<u64>().ok().filter(|n| *n >= 1).ok_or_else(|| EngineError::Other("Requires a positive integer.".into()))?;
                    json!({ "count": n })
                }
            },
            _ => return Ok(Step::Continue),
        };
        let r = run_mredo(s, &p)?;
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            s.echo(m.to_string());
        }
        Ok(Step::Done)
    }
}

fn copy_to_clip(s: &mut Session, hs: &[Handle], base: Option<Vec2>) -> Result<usize> {
    let d = s.doc()?;
    let ents: Vec<Entity> = hs.iter().filter_map(|h| d.entity(*h).map(|e| (**e).clone())).collect();
    let b = ents.iter().fold(Bounds2::EMPTY, |acc, e| acc.union(&entity_bounds(d, e, 0)));
    let base = base.unwrap_or(if b.min.is_finite() { b.min } else { Vec2::ZERO });
    let n = ents.len();
    clipboard::copy(s, ents, base)?;
    Ok(n)
}

fn run_copyclip(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let n = copy_to_clip(s, &hs, None)?;
    Ok(json!({ "copied": n }))
}

fn run_copybase(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let base = point_param(p, "base");
    let n = copy_to_clip(s, &hs, base)?;
    Ok(json!({ "copied": n }))
}

fn run_cut(s: &mut Session, p: &Value) -> Result<Value> {
    let mut hs = targets(s, p)?;
    hs.retain(|h| !super::curves::is_locked(s, *h));
    let n = copy_to_clip(s, &hs, None)?;
    let d = s.doc_mut()?;
    for h in &hs {
        d.remove_entity(*h);
    }
    s.set_selection(Vec::new());
    Ok(json!({ "cut": n }))
}

/// Paste the clipboard moved by `offset` (the clipboard brings the layers, styles and blocks its
/// objects need).
fn paste(s: &mut Session, offset: Vec2) -> Result<Vec<Handle>> {
    clipboard::paste(s, &Mat3::translate(offset))
}

fn run_paste(s: &mut Session, p: &Value) -> Result<Value> {
    if s.clipboard.is_empty() {
        // Copied in another program: an image becomes a raster image, text multiline text.
        return match PasteKind::fallback(s) {
            Some(kind) => paste_as(s, kind, point_param(p, "at"), p.get("width").and_then(Value::as_f64)),
            None => Err(bad("pasteclip", "the clipboard is empty")),
        };
    }
    let off = match point_param(p, "at") {
        Some(at) => at - s.clipboard_base,
        None => Vec2::ZERO,
    };
    let r = paste(s, off)?;
    Ok(json!({ "handles": r.iter().map(|h| h.hex()).collect::<Vec<_>>() }))
}

/// What PASTESPEC pastes the clipboard as.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PasteKind {
    /// CADaddy objects (PASTECLIP).
    Objects,
    /// A raster image.
    Image,
    /// Multiline text.
    Text,
}

impl PasteKind {
    const KEYWORDS: [&'static str; 3] = ["Objects", "Image", "Text"];

    fn from_name(n: &str) -> Option<PasteKind> {
        match n.trim().to_ascii_lowercase().as_str() {
            "objects" | "object" | "o" | "cadaddy" => Some(PasteKind::Objects),
            "image" | "picture" | "i" | "png" => Some(PasteKind::Image),
            "text" | "mtext" | "t" => Some(PasteKind::Text),
            _ => None,
        }
    }

    /// Whether the clipboard can paste as this.
    fn available(self, s: &Session) -> bool {
        match self {
            PasteKind::Objects => !s.clipboard.is_empty(),
            PasteKind::Image => s.system_clipboard.image.is_some() || !s.clipboard.is_empty(),
            PasteKind::Text => s.system_clipboard.text.as_deref().is_some_and(|t| !t.trim().is_empty()),
        }
    }

    /// With no CADaddy objects on the clipboard: what another program's copy pastes as.
    fn fallback(s: &Session) -> Option<PasteKind> {
        [PasteKind::Image, PasteKind::Text].into_iter().find(|k| k.available(s))
    }

    /// The first way the clipboard can paste: objects, then an image, then text.
    fn first(s: &Session) -> Option<PasteKind> {
        [PasteKind::Objects, PasteKind::Image, PasteKind::Text].into_iter().find(|k| k.available(s))
    }
}

/// Paste the clipboard as `kind` at `at` (objects: where the base point lands, default their
/// original place; image: lower-left corner; text: insertion point).
fn paste_as(s: &mut Session, kind: PasteKind, at: Option<Vec2>, width: Option<f64>) -> Result<Value> {
    if !kind.available(s) {
        let what = match kind {
            PasteKind::Objects => "the clipboard holds no CADaddy objects",
            PasteKind::Image => "the clipboard holds no image or objects to paste as an image",
            PasteKind::Text => "the clipboard holds no text",
        };
        return Err(bad("pastespec", what));
    }
    match kind {
        PasteKind::Objects => {
            let off = at.map_or(Vec2::ZERO, |a| a - s.clipboard_base);
            let r = paste(s, off)?;
            Ok(json!({ "as": "objects", "handles": r.iter().map(|h| h.hex()).collect::<Vec<_>>() }))
        }
        PasteKind::Image => {
            // A picture from another program; else the copied objects drawn as one.
            let image = match s.system_clipboard.image.clone() {
                Some(i) => i,
                None => std::sync::Arc::new(
                    clipboard::system_image(s, clipboard::IMAGE_SIDE).ok_or_else(|| bad("pastespec", "the objects couldn't be drawn as an image"))?,
                ),
            };
            let mut r = clipboard::paste_image(s, &image, at, width)?;
            r["as"] = json!("image");
            Ok(r)
        }
        PasteKind::Text => {
            let text = s.system_clipboard.text.clone().unwrap_or_default();
            let mut r = clipboard::paste_text(s, &text, at)?;
            r["as"] = json!("text");
            Ok(r)
        }
    }
}

fn run_pastespec(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = match str_param(p, "as") {
        Some(n) => PasteKind::from_name(n).ok_or_else(|| bad("pastespec", "`as` is \"objects\", \"image\" or \"text\""))?,
        None => PasteKind::first(s).ok_or_else(|| bad("pastespec", "the clipboard is empty"))?,
    };
    paste_as(s, kind, point_param(p, "at"), p.get("width").and_then(Value::as_f64))
}

/// PASTESPEC at the command line: how to paste (when the clipboard can paste more than one way),
/// then where.
#[derive(Default)]
struct PasteSpecM {
    kind: Option<PasteKind>,
}

impl Interactive for PasteSpecM {
    fn name(&self) -> &'static str {
        "PASTESPEC"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        let ways: Vec<PasteKind> = [PasteKind::Objects, PasteKind::Image, PasteKind::Text].into_iter().filter(|k| k.available(s)).collect();
        match ways.as_slice() {
            [] => {
                s.echo("Clipboard is empty.");
                Ok(Step::Done)
            }
            [one] => {
                self.kind = Some(*one);
                Ok(Step::Continue)
            }
            _ => Ok(Step::Continue),
        }
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.kind {
            None => {
                let kws: Vec<&str> =
                    PasteKind::KEYWORDS.into_iter().filter(|k| PasteKind::from_name(k).is_some_and(|kind| kind.available(s))).collect();
                let first = kws.first().copied().unwrap_or("Objects");
                Prompt::new("Paste as", Accept { enter: true, ..Accept::TEXT }).kw(&kws).default(first)
            }
            Some(_) => Prompt::new("Specify insertion point", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.kind, i) {
            (_, Input::Cancel) => Ok(Step::Cancel),
            (None, Input::Enter) => {
                self.kind = PasteKind::first(s);
                Ok(if self.kind.is_some() { Step::Continue } else { Step::Done })
            }
            (None, Input::Keyword(k) | Input::Text(k)) => {
                let k = self.prompt(s).match_keyword(&k);
                match k.as_deref().and_then(PasteKind::from_name) {
                    Some(kind) => self.kind = Some(kind),
                    None => s.echo("Invalid option keyword."),
                }
                Ok(Step::Continue)
            }
            (Some(kind), Input::Point(p)) => {
                paste_as(s, kind, Some(p), None)?;
                Ok(Step::Done)
            }
            (Some(kind), Input::Enter) => {
                paste_as(s, kind, None, None)?;
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
    fn preview(&self, s: &Session, c: Vec2) -> Vec<cadaddy_doc::EntityKind> {
        if self.kind != Some(PasteKind::Objects) {
            return Vec::new();
        }
        let m = Mat3::translate(c - s.clipboard_base);
        s.clipboard
            .iter()
            .take(500)
            .map(|e| {
                let mut k = e.kind.clone();
                k.transform(&m);
                k
            })
            .collect()
    }
}

fn run_pasteblock(s: &mut Session, p: &Value) -> Result<Value> {
    let at = point_param(p, "at").unwrap_or(s.clipboard_base);
    let h = clipboard::paste_block(s, at)?;
    let block = match s.doc()?.entity(h).map(|e| &e.kind) {
        Some(cadaddy_doc::EntityKind::Insert(i)) => i.block.clone(),
        _ => String::new(),
    };
    Ok(json!({ "handle": h.hex(), "block": block }))
}

fn run_pasteorig(s: &mut Session, _p: &Value) -> Result<Value> {
    let r = paste(s, Vec2::ZERO)?;
    Ok(json!({ "handles": r.iter().map(|h| h.hex()).collect::<Vec<_>>() }))
}

fn run_clear(s: &mut Session, _p: &Value) -> Result<Value> {
    let mut hs = s.selection();
    hs.retain(|h| !super::curves::is_locked(s, *h));
    let d = s.doc_mut()?;
    let mut n = 0;
    for h in hs {
        if d.remove_entity(h).is_some() {
            n += 1;
        }
    }
    s.set_selection(Vec::new());
    Ok(json!({ "erased": n }))
}

fn run_selectall(s: &mut Session, _p: &Value) -> Result<Value> {
    let space = s.space();
    let d = s.doc()?;
    let hs: Vec<Handle> = d
        .space(&space)
        .map(|st| st.iter().filter(|e| d.is_visible(e) && d.layer(&e.common.layer).is_none_or(|l| !l.locked)).map(|e| e.handle).collect())
        .unwrap_or_default();
    let n = hs.len();
    s.set_selection(hs);
    Ok(json!({ "selected": n }))
}

fn run_deselect(s: &mut Session, _p: &Value) -> Result<Value> {
    s.set_selection(Vec::new());
    Ok(json!({ "selected": 0 }))
}

fn run_select(s: &mut Session, p: &Value) -> Result<Value> {
    if bool_or(p, "clear", false) {
        s.set_selection(Vec::new());
        return Ok(json!({ "selected": 0 }));
    }
    // Everything selectable in the current space (SELECTALL; ALL at "Select objects").
    if bool_or(p, "all", false) {
        return run_selectall(s, p);
    }
    let space = s.space();
    let add = bool_or(p, "add", false);
    let mut candidates: Option<Vec<Handle>> = None;
    let mut named = false;
    let picked: Vec<Handle> = if let Some(w) = points_param(p, "window") {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { return Err(bad("select", "window needs 2 corners")) };
        crate::select::select_window(s.doc()?, &space, Bounds2::new(*a, *b), bool_or(p, "crossing", false))
    } else if let Some(c) = str_param(p, "class") {
        crate::select::select_class(s.doc()?, &space, c)
    } else if let Some(path) = points_param(p, "lasso") {
        let mode = match str_param(p, "mode") {
            Some(m) => crate::select::LassoMode::from_name(m).ok_or_else(|| bad("select", "`mode` is \"window\", \"crossing\" or \"fence\""))?,
            None => crate::select::LassoMode::from_direction(&path),
        };
        let need = if mode == crate::select::LassoMode::Fence { 2 } else { 3 };
        if path.len() < need {
            return Err(bad("select", format!("a {} lasso needs at least {need} points", mode.name())));
        }
        crate::select::select_lasso(s.doc()?, &space, &path, mode)
    } else if let Some(at) = point_param(p, "at") {
        let ap = f64_or(p, "aperture", s.pixel_size() * s.settings.pickbox * 1.5);
        match p.get("cycle") {
            None | Some(Value::Null) => crate::select::pick(s.doc()?, &space, at, ap).into_iter().collect(),
            Some(v) => {
                let n = v.as_u64().ok_or_else(|| bad("select", "`cycle` is a whole number (0: the nearest object)"))?;
                let all = crate::select::pick_all(s.doc()?, &space, at, ap);
                let chosen = usize::try_from(n).ok().and_then(|n| n.checked_rem(all.len())).and_then(|i| all.get(i).copied());
                candidates = Some(all);
                chosen.into_iter().collect()
            }
        }
    } else if let Some(f) = points_param(p, "fence") {
        crate::select::select_fence(s.doc()?, &space, &f)
    } else {
        named = true;
        targets(s, p)?
    };
    // A pick takes whole groups (PICKSTYLE), as typed picks and clicks do (battle test 0.7.7c);
    // handles named one by one are taken as named.
    let picked = if named { picked } else { super::groups::expand(s, picked) };
    let mut sel = if add { s.selection() } else { Vec::new() };
    sel.extend(picked);
    s.set_selection(sel);
    let sel = s.selection();
    let mut reply = json!({ "selected": sel.len(), "handles": sel.iter().map(|h| h.hex()).collect::<Vec<_>>() });
    if let (Some(c), Some(m)) = (candidates, reply.as_object_mut()) {
        m.insert("candidates".into(), json!(c.iter().map(|h| h.hex()).collect::<Vec<_>>()));
    }
    Ok(reply)
}

#[derive(Clone, Copy, PartialEq)]
enum PasteMode {
    /// PASTECLIP: at an insertion point.
    Clip,
    /// PASTEBLOCK: as a block, at an insertion point.
    Block,
    /// PASTEORIG: at the original coordinates, without asking.
    Orig,
}

struct PasteM {
    mode: PasteMode,
}

impl Interactive for PasteM {
    fn name(&self) -> &'static str {
        match self.mode {
            PasteMode::Clip => "PASTECLIP",
            PasteMode::Block => "PASTEBLOCK",
            PasteMode::Orig => "PASTEORIG",
        }
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        // PASTECLIP of an image or text copied in another program: placed at a point too.
        if s.clipboard.is_empty() && self.mode == PasteMode::Clip && PasteKind::fallback(s).is_some() {
            return Ok(Step::Continue);
        }
        if s.clipboard.is_empty() {
            s.echo("Clipboard is empty.");
            return Ok(Step::Done);
        }
        if self.mode == PasteMode::Orig {
            paste(s, Vec2::ZERO)?;
            return Ok(Step::Done);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Specify insertion point", Accept::POINT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Point(p) if self.mode == PasteMode::Block => {
                clipboard::paste_block(s, p)?;
                Ok(Step::Done)
            }
            // Another program's copy: at the point, or Enter for the view's centre.
            i @ (Input::Point(_) | Input::Enter) if s.clipboard.is_empty() => {
                let at = if let Input::Point(p) = i { Some(p) } else { None };
                if let Some(kind) = PasteKind::fallback(s) {
                    paste_as(s, kind, at, None)?;
                }
                Ok(Step::Done)
            }
            Input::Point(p) => {
                let base = s.clipboard_base;
                paste(s, p - base)?;
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
    fn drag(&self, s: &Session, c: Vec2) -> Option<crate::prompt::Drag> {
        if self.mode == PasteMode::Orig || s.clipboard.is_empty() {
            return None;
        }
        Some(crate::prompt::Drag { handles: Vec::new(), matrix: Mat3::translate(c - s.clipboard_base), extra: Vec::new(), clipboard: true })
    }
    fn preview(&self, s: &Session, c: Vec2) -> Vec<cadaddy_doc::EntityKind> {
        let m = Mat3::translate(c - s.clipboard_base);
        s.clipboard
            .iter()
            .take(500)
            .map(|e| {
                let mut k = e.kind.clone();
                k.transform(&m);
                k
            })
            .collect()
    }
}

#[derive(Default)]
struct SelectM {
    sel: SelectPhase,
}

impl Interactive for SelectM {
    fn name(&self) -> &'static str {
        "SELECT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        self.sel.prompt()
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            _ => Ok(Step::Done),
        }
    }
}

/// COPYBASE: base point first, then "Select objects:" (skipped with a pickfirst selection).
#[derive(Default)]
struct CopyBaseM {
    base: Option<Vec2>,
    sel: SelectPhase,
}

impl CopyBaseM {
    fn copy(&self, s: &mut Session, hs: &[Handle]) -> Result<Step> {
        copy_to_clip(s, hs, self.base)?;
        Ok(Step::Done)
    }
}

impl Interactive for CopyBaseM {
    fn name(&self) -> &'static str {
        "COPYBASE"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.base.is_none() { Prompt::new("Specify base point", Accept::POINT) } else { self.sel.prompt() }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.base.is_none() {
            if let Input::Point(p) = i {
                self.base = Some(p);
                if self.sel.done {
                    let hs = self.sel.picked.clone();
                    return self.copy(s, &hs);
                }
            }
            return Ok(Step::Continue);
        }
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Empty => Ok(Step::Done),
            SelOutcome::Done(hs) => {
                let step = self.copy(s, &hs)?;
                s.set_selection(Vec::new());
                Ok(step)
            }
        }
    }
}
