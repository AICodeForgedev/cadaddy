//! Hyperlinks: HYPERLINK (insert, edit or remove an object's link: a URL or file, a named location
//! (layout or view), a description), -HYPERLINK at the command line, and following a link
//! (`hyperlink.open`). Links are kept as AutoCAD keeps them, `PE_URL` extended data
//! ([`cadaddy_doc::hyperlink`]), so DXF and DWG files carry them and PDF plots link them.
//! HYPERLINKBASE (a drawing variable) is the base relative links resolve against.

use cadaddy_doc::Handle;
use cadaddy_doc::hyperlink::{self, Hyperlink};
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase};
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("hyperlink", "Hyperlink...", run_hyperlink)
            .menu(&["Insert", "Hyperlink..."])
            .key("Cmd+K")
            .alias(&["-hyperlink"])
            .params(
                "{handles?: [hex] (default: the selection), url?: URL or file (\"\" or absent with target: this drawing), target?: named location (a layout or view), description?, remove?: bool} → {changed} | {handles?} alone → {links: [{handle, url, target, description}]} (typed HYPERLINK opens the dialog; -HYPERLINK prompts)",
            )
            .interactive(|_| Ok(Box::new(HyperlinkM::default()))),
        CommandSpec::new("hyperlink.pick", "Select Objects for a Hyperlink", run_pick)
            .params("{handles} → selects them (typed: \"Select objects:\"; the selection stays for the Insert Hyperlink dialog)")
            .noundo()
            .interactive(|_| Ok(Box::new(PickM::default()))),
        CommandSpec::new("hyperlink.open", "Open Hyperlink", run_open)
            .params("{handle} → {url?: what to open (resolved against HYPERLINKBASE, else the drawing's folder), target?, label} — a named location in this drawing (a layout or view) is gone to here; a URL is the host's to open")
            .noundo(),
    ]
}

/// An object's hyperlink as JSON.
fn link_json(h: Handle, l: &Hyperlink) -> Value {
    json!({ "handle": h.hex(), "url": l.url, "target": l.target, "description": l.description })
}

/// The hyperlink of object `h` in the current drawing.
pub fn link_of(s: &Session, h: Handle) -> Option<Hyperlink> {
    hyperlink::read(&s.doc().ok()?.entity(h)?.common.xdata)
}

/// Set (or with `None` remove) the hyperlink of each object; the number changed.
fn set_links(s: &mut Session, hs: &[Handle], link: Option<&Hyperlink>) -> Result<usize> {
    let d = s.doc_mut()?;
    let mut n = 0usize;
    for h in hs {
        let had = d.entity(*h).map(|e| hyperlink::read(&e.common.xdata));
        let Some(had) = had else { continue };
        if had.as_ref() == link.filter(|l| !l.is_empty()) {
            continue;
        }
        if link.is_none() && had.is_none() {
            continue;
        }
        if d.modify_entity(*h, |e| hyperlink::write(&mut e.common.xdata, link)).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

fn text(p: &Value, key: &str) -> Result<Option<String>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) => Ok(Some(t.chars().take(hyperlink::MAX_LEN).collect())),
        Some(v) => Err(bad("hyperlink", format!("`{key}` must be text, got {v}"))),
    }
}

fn run_hyperlink(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let (url, target, description) = (text(p, "url")?, text(p, "target")?, text(p, "description")?);
    let remove = match p.get("remove") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("hyperlink", "`remove` must be true or false")),
    };
    if !remove && url.is_none() && target.is_none() && description.is_none() {
        // A query: the objects' links.
        let d = s.doc()?;
        let links: Vec<Value> =
            hs.iter().filter_map(|h| Some((*h, hyperlink::read(&d.entity(*h)?.common.xdata)?))).map(|(h, l)| link_json(h, &l)).collect();
        return Ok(json!({ "links": links }));
    }
    if hs.is_empty() {
        return Err(bad("hyperlink", "no objects: pass `handles` or select objects first"));
    }
    if remove {
        let n = set_links(s, &hs, None)?;
        return Ok(json!({ "changed": n }));
    }
    let link = Hyperlink { url: url.unwrap_or_default(), target: target.unwrap_or_default(), description: description.unwrap_or_default() };
    if link.is_empty() {
        return Err(bad("hyperlink", "a hyperlink needs a `url` or a `target` (use remove: true to remove one)"));
    }
    let n = set_links(s, &hs, Some(&link))?;
    Ok(json!({ "changed": n }))
}

/// The folder of the current drawing's file, for relative links.
fn folder(s: &Session) -> Option<String> {
    let p = s.state().ok()?.path.clone()?;
    let full = std::path::absolute(&p).unwrap_or_else(|_| std::path::PathBuf::from(&p));
    Some(full.parent()?.to_string_lossy().into_owned())
}

/// Go to a named location of this drawing: a layout (or Model), else a named view. False when
/// it names neither.
fn go_to(s: &mut Session, target: &str) -> bool {
    let Ok(d) = s.doc() else { return false };
    let layout = if target.eq_ignore_ascii_case("model") {
        Some("Model".to_string())
    } else {
        d.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(target)).map(|l| l.name.clone())
    };
    if let Some(name) = layout {
        return s.execute("layout", &json!({ "option": "set", "name": name })).is_ok();
    }
    let view = d.views.iter().find(|v| v.name.eq_ignore_ascii_case(target)).map(|v| v.name.clone());
    view.is_some_and(|name| s.execute("view", &json!({ "option": "restore", "name": name })).is_ok())
}

fn run_open(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let h = *hs.first().ok_or_else(|| bad("hyperlink.open", "`handle` is required"))?;
    let link = link_of(s, h).ok_or_else(|| bad("hyperlink.open", format!("object {} has no hyperlink", h.hex())))?;
    let base = s.doc()?.header.str("HYPERLINKBASE", "");
    let url = hyperlink::resolve(&link, &base, folder(s).as_deref());
    let mut r = json!({ "label": link.label() });
    match url {
        Some(u) => r["url"] = json!(u),
        None => {
            if !go_to(s, &link.target) {
                return Err(bad("hyperlink.open", format!("this drawing has no layout or view named \"{}\"", link.target)));
            }
            r["target"] = json!(link.target);
        }
    }
    Ok(r)
}

fn run_pick(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let d = s.doc()?;
    let hs: Vec<Handle> = hs.into_iter().filter(|h| d.entity(*h).is_some()).collect();
    s.set_selection(hs.clone());
    Ok(json!({ "selected": hs.len() }))
}

/// The objects a hyperlink goes on, picked and left selected (what the dialog edits next).
#[derive(Default)]
struct PickM {
    sel: SelectPhase,
}

impl Interactive for PickM {
    fn name(&self) -> &'static str {
        "HYPERLINK"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        self.sel.prompt()
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Empty => Ok(Step::Done),
            SelOutcome::Done(hs) => {
                s.set_selection(hs);
                Ok(Step::Done)
            }
        }
    }
}

/// -HYPERLINK: Remove or Insert, select objects, then (Insert) the link, named location and
/// description, as AutoCAD asks at the command line.
#[derive(Default)]
struct HyperlinkM {
    stage: Stage,
    remove: bool,
    sel: SelectPhase,
    objs: Vec<Handle>,
    url: String,
    target: String,
}

#[derive(Default, PartialEq)]
enum Stage {
    #[default]
    Option,
    Select,
    Url,
    Target,
    Description,
}

impl HyperlinkM {
    fn selected(&mut self, s: &mut Session, hs: Vec<Handle>) -> Result<Step> {
        s.set_selection(Vec::new());
        if self.remove {
            let n = set_links(s, &hs, None)?;
            s.echo(format!("{n} hyperlink{} deleted.", if n == 1 { "" } else { "s" }));
            return Ok(Step::Done);
        }
        self.objs = hs;
        self.stage = Stage::Url;
        Ok(Step::Continue)
    }
}

impl Interactive for HyperlinkM {
    fn name(&self) -> &'static str {
        "HYPERLINK"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::Option => Prompt::new("Enter an option", Accept::TEXT).kw(&["Remove", "Insert"]).default("Insert"),
            Stage::Select => self.sel.prompt(),
            Stage::Url => Prompt::new("Enter hyperlink <current drawing>", Accept::TEXT),
            Stage::Target => Prompt::new("Enter named location <none>", Accept::TEXT),
            Stage::Description => Prompt::new("Enter description <none>", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        let typed = |i: &Input| match i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            _ => String::new(),
        };
        match self.stage {
            Stage::Option => {
                let t = typed(&i).to_ascii_lowercase();
                self.remove = match t.as_str() {
                    "" | "i" | "insert" => false,
                    "r" | "remove" => true,
                    _ => {
                        s.echo("Invalid option keyword.");
                        return Ok(Step::Continue);
                    }
                };
                self.stage = Stage::Select;
                self.sel = SelectPhase::begin(s);
                if self.sel.done {
                    let hs = self.sel.picked.clone();
                    return self.selected(s, hs);
                }
                Ok(Step::Continue)
            }
            Stage::Select => match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => self.selected(s, hs),
            },
            Stage::Url => {
                self.url = typed(&i);
                self.stage = Stage::Target;
                Ok(Step::Continue)
            }
            Stage::Target => {
                self.target = typed(&i);
                if self.url.is_empty() && self.target.is_empty() {
                    s.echo("A hyperlink to the current drawing needs a named location.");
                    return Ok(Step::Done);
                }
                self.stage = Stage::Description;
                Ok(Step::Continue)
            }
            Stage::Description => {
                let link = Hyperlink { url: std::mem::take(&mut self.url), target: std::mem::take(&mut self.target), description: typed(&i) };
                let objs = std::mem::take(&mut self.objs);
                let n = set_links(s, &objs, Some(&link))?;
                s.echo(format!("{n} hyperlink{} attached.", if n == 1 { "" } else { "s" }));
                Ok(Step::Done)
            }
        }
    }
}
