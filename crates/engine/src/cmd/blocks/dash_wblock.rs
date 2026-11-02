//! -WBLOCK at the command line, as AutoCAD 2024 asks it: the output file (replace? a DXF's
//! precision), then an existing block, `=` (the block named as the file), `*` (the whole drawing)
//! or Enter for objects (base point, mOde, objects), which are erased once written. The file is
//! written by WBLOCK's JSON form, with its checks (not over an open drawing, not in a dry run).

use cadaddy_doc::Handle;
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::super::curves::KW;
use super::super::machines::{SelOutcome, SelectPhase};
use super::super::{CommandSpec, bad, point_param, str_param, targets};
use super::wblock::stem;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// -WBLOCK erases the objects it writes, whatever its mOde option says: AutoCAD 2024 does, and
/// keeps "Object conversion=Delete" as the setting. OOPS brings them back, as after ERASE.
pub(super) fn dash_wblock_spec() -> CommandSpec {
    CommandSpec::new("-wblock", "Write Block", run_dash_wblock)
        .params("{path, name? | handles?, base?} (the objects written are erased)")
        .interactive(|_| Ok(Box::new(DashWblockM::default())))
}

/// The web build has no file system: -WBLOCK would erase objects it can't save.
const NO_FILES: &str = "files can't be written in the web build";

/// What -WBLOCK writes.
enum Source<'a> {
    /// These objects, `base` becoming the written drawing's origin.
    Objects(&'a [Handle], Vec2),
    /// A block's objects, from its base point.
    Block(&'a str),
    /// The whole drawing.
    Whole,
}

fn run_dash_wblock(s: &mut Session, p: &Value) -> Result<Value> {
    if cfg!(target_arch = "wasm32") {
        return Err(bad("-wblock", NO_FILES));
    }
    if str_param(p, "name").is_some() {
        return super::run_wblock(s, p);
    }
    let hs = targets(s, p)?;
    // As at the command line, nothing selected writes no file.
    if !hs.iter().any(|h| s.doc().is_ok_and(|d| d.entity(*h).is_some())) {
        return Err(bad("-wblock", "no objects selected"));
    }
    let base = point_param(p, "base").unwrap_or(Vec2::ZERO);
    let path = str_param(p, "path").ok_or_else(|| bad("-wblock", "`path` is required"))?.to_string();
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let r = write(s, &path, Source::Objects(&hs, base), force)?;
    erase(s, &hs)?;
    Ok(r)
}

/// Write `what` to `path` through WBLOCK's JSON form (its checks and the written drawing's
/// settings, styles and fresh fingerprint); `force` replaces an existing file.
fn write(s: &mut Session, path: &str, what: Source, force: bool) -> Result<Value> {
    let hex = |hs: &[Handle]| hs.iter().map(|h| h.hex()).collect::<Vec<_>>();
    let p = match what {
        Source::Whole => {
            let all: Vec<Handle> = s.doc()?.model.iter().map(|e| e.handle).collect();
            json!({ "path": path, "handles": hex(&all), "whole": true, "force": force })
        }
        Source::Block(name) => json!({ "path": path, "name": name, "force": force }),
        Source::Objects(hs, base) => json!({ "path": path, "handles": hex(hs), "base": [base.x, base.y], "force": force }),
    };
    super::run_wblock(s, &p)
}

/// Erase what -WBLOCK wrote, as ERASE does: OOPS brings it back.
fn erase(s: &mut Session, hs: &[Handle]) -> Result<()> {
    let doc = s.doc_mut()?;
    let mut gone = Vec::new();
    for h in hs {
        if let Some(space) = doc.space_of(*h)
            && let Some(e) = doc.entity(*h).cloned()
        {
            gone.push((space, e));
        }
        doc.remove_entity(*h);
    }
    crate::cmd::classic::remember_erased(s, gone);
    s.set_selection(Vec::new());
    Ok(())
}

/// The file -WBLOCK writes for a typed name: as named when it ends in .dwg or .dxf, else with
/// .dwg added, as AutoCAD names a drawing.
fn output_path(typed: &str) -> String {
    let t = typed.trim().trim_matches('"');
    let lower = t.to_ascii_lowercase();
    if lower.ends_with(".dwg") || lower.ends_with(".dxf") { t.to_string() } else { format!("{t}.dwg") }
}

/// Whether a file is there to replace; the web has no file system.
fn exists(path: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::path::Path::new(path).exists()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        false
    }
}

/// Where -WBLOCK is in its prompts, as AutoCAD 2024 asks them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Ask {
    #[default]
    File,
    /// The file exists: "… already exists, do you want to replace it? [Yes/No] <N>".
    Replace,
    /// A DXF file's decimal places of accuracy.
    Precision,
    /// An existing block, `=` (the block named as the file), `*` (the whole drawing), or Enter
    /// for objects.
    Block,
    Base,
    /// mOde at the base point prompt: asked, but the objects are erased whatever it says.
    Mode,
    Objects,
}

const SAME_NAME: &str = "= (block=output file)";
const WHOLE: &str = "* (whole drawing)";

#[derive(Default)]
struct DashWblockM {
    ask: Ask,
    path: String,
    base: Vec2,
    sel: SelectPhase,
}

impl Interactive for DashWblockM {
    fn name(&self) -> &'static str {
        "WBLOCK"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        if cfg!(target_arch = "wasm32") {
            s.echo(format!("-WBLOCK: {NO_FILES}."));
            return Ok(Step::Cancel);
        }
        s.echo("Current settings: Object conversion=Delete");
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.ask {
            Ask::File => Prompt::new("Enter name of output file", Accept::TEXT),
            Ask::Replace => Prompt::new(format!("{} already exists, do you want to replace it?", self.path), KW).kw(&["Yes", "No"]).default("N"),
            Ask::Precision => Prompt::new("Enter decimal places of accuracy (0 to 16)", Accept::NUMBER).kw(&["Binary", "Preview"]).default("16"),
            Ask::Block => Prompt::new("Enter name of existing block", Accept::TEXT).kw(&[SAME_NAME, WHOLE]).default("define new drawing"),
            Ask::Base => Prompt::new("Specify insertion base point", Accept::POINT).kw(&["mOde"]),
            Ask::Mode => {
                Prompt::new("Choose mode for selected object conversion", KW).kw(&["Convert to block", "Retain", "Delete"]).default("Delete")
            }
            Ask::Objects => self.sel.prompt(),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.ask {
            Ask::File => {
                if let Input::Text(t) = &i
                    && !t.trim().trim_matches('"').is_empty()
                {
                    self.path = output_path(t);
                    self.ask = if exists(&self.path) { Ask::Replace } else { self.after_file(s) };
                }
                Ok(Step::Continue)
            }
            Ask::Replace => match &i {
                // Yes goes on; No, the default, ends the command; anything else is asked again.
                Input::Keyword(k) if k == "Yes" => {
                    self.ask = self.after_file(s);
                    Ok(Step::Continue)
                }
                Input::Keyword(_) | Input::Enter => Ok(Step::Done),
                _ => {
                    s.echo("Invalid option keyword.");
                    Ok(Step::Continue)
                }
            },
            Ask::Precision => {
                match &i {
                    Input::Enter => self.ask = Ask::Block,
                    // The file is written at full precision whatever is asked.
                    Input::Text(t) if t.trim().parse::<u8>().is_ok_and(|n| n <= 16) => self.ask = Ask::Block,
                    Input::Keyword(k) if k == "Binary" => s.echo("Binary DXF output is not available."),
                    Input::Keyword(_) => {}
                    _ => s.echo("Requires an integer between 0 and 16."),
                }
                Ok(Step::Continue)
            }
            Ask::Block => {
                let name = match &i {
                    Input::Enter => {
                        self.ask = Ask::Base;
                        return Ok(Step::Continue);
                    }
                    Input::Keyword(k) if k == WHOLE => return self.write(s, Source::Whole),
                    Input::Keyword(k) if k == SAME_NAME => stem(&self.path),
                    Input::Text(t) => t.trim().to_string(),
                    // A point (from a LISP list) is a name too: AutoCAD answers Block "0,0" not found.
                    Input::Point(p) => format!("{},{}", p.x, p.y),
                    _ => return Ok(Step::Continue),
                };
                if s.doc()?.block(&name).is_none() {
                    s.echo(format!("Block \"{name}\" not found."));
                    return Ok(Step::Continue);
                }
                self.write(s, Source::Block(&name))
            }
            Ask::Base => {
                match i {
                    Input::Keyword(_) => self.ask = Ask::Mode,
                    Input::Enter | Input::Text(_) => s.echo("Point or option keyword required."),
                    Input::Point(p) => {
                        self.base = p;
                        self.sel = SelectPhase::begin(s);
                        if self.sel.done {
                            let picked = self.sel.picked.clone();
                            return self.write_objects(s, &picked);
                        }
                        self.ask = Ask::Objects;
                    }
                    _ => {}
                }
                Ok(Step::Continue)
            }
            Ask::Mode => {
                // Any answer, Enter included, goes back to the base point.
                self.ask = Ask::Base;
                Ok(Step::Continue)
            }
            Ask::Objects => match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                // Nothing selected: nothing is written.
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => self.write_objects(s, &hs),
            },
        }
    }
}

impl DashWblockM {
    /// The prompt after the file name: a DXF file's precision, else the block.
    fn after_file(&self, s: &mut Session) -> Ask {
        if self.path.to_ascii_lowercase().ends_with(".dxf") {
            s.echo("Current DXF settings: Precision= 16 Format= ASCII Preview= No");
            Ask::Precision
        } else {
            Ask::Block
        }
    }

    fn write(&self, s: &mut Session, what: Source) -> Result<Step> {
        // The file was asked about already (replaced only on Yes).
        if let Err(e) = write(s, &self.path, what, true) {
            s.echo(e.to_string());
        }
        Ok(Step::Done)
    }

    fn write_objects(&self, s: &mut Session, hs: &[Handle]) -> Result<Step> {
        if hs.is_empty() {
            return Ok(Step::Done);
        }
        match write(s, &self.path, Source::Objects(hs, self.base), true) {
            Ok(_) => erase(s, hs)?,
            Err(e) => s.echo(e.to_string()),
        }
        Ok(Step::Done)
    }
}
