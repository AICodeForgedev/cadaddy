//! -DIMSTYLE and -STYLE: the styles at the command line, as AutoCAD scripts drive them.
//! -DIMSTYLE: Save (the current settings as a named style), Restore (make one current), STatus
//! (the current DIM* values), Variables (a style's values), Apply (the current style to chosen
//! dimensions) and ? (list). -STYLE: name, font, height, width factor, obliquing angle,
//! backwards and upside-down, then the style is current.

use serde_json::{Map, Value, json};

use super::{CommandSpec, find_command};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("-dimstyle", "Dimension Style", |s, p| run(s, "dimstyle", p))
            .params("as `dimstyle` (typed: Save/Restore/STatus/Variables/Apply/? prompts)")
            .interactive(|_| Ok(Box::new(DashDimStyle::default()))),
        CommandSpec::new("-style", "Text Style", |s, p| run(s, "style", p))
            .params("as `style` (typed: name, font, height, width, obliquing, backwards, upside-down prompts)")
            .interactive(|_| Ok(Box::new(DashStyle::default()))),
    ]
}

/// The JSON form: the same as the dialog command's.
fn run(s: &mut Session, id: &str, p: &Value) -> Result<Value> {
    let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?;
    (spec.run)(s, p)
}

/// Run command `id`'s body (the running command records the undo step) and echo its message.
fn call(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?;
    let r = (spec.run)(s, &p)?;
    if let Some(m) = r.get("message").and_then(Value::as_str) {
        m.lines().for_each(|l| s.echo(l.to_string()));
    }
    Ok(r)
}

fn current_dimstyle(s: &Session) -> String {
    s.doc().map(|d| d.header.str("DIMSTYLE", "Standard")).unwrap_or_else(|_| "Standard".into())
}

/// The current dimension settings: every DIM* variable as the drawing reads it now.
fn current_vars(s: &Session) -> Map<String, Value> {
    cadaddy_doc::DIMVARS.iter().filter_map(|(v, _)| crate::sysvars::get(s, v).map(|x| (v.to_string(), x))).collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum DimStage {
    #[default]
    Option,
    Save,
    Restore,
    Variables,
    Apply,
}

#[derive(Default)]
struct DashDimStyle {
    stage: DimStage,
    /// Apply's "Select objects" (found counts, Remove, Undo, as everywhere).
    sel: super::machines::SelectPhase,
}

impl Interactive for DashDimStyle {
    fn name(&self) -> &'static str {
        "-DIMSTYLE"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        s.echo(format!("Current dimension style:  {}", current_dimstyle(s)));
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            DimStage::Option => Prompt::new("Enter a dimension style option", Accept::TEXT)
                .kw(&["Save", "Restore", "STatus", "Variables", "Apply", "?"])
                .default("Restore"),
            DimStage::Save => Prompt::new("Enter name for new dimension style or [?]", Accept::TEXT),
            DimStage::Restore => Prompt::new("Enter a dimension style name, [?] or <select dimension>", Accept::TEXT),
            DimStage::Variables => Prompt::new("Enter a dimension style name, [?] or <select dimension>", Accept::TEXT),
            DimStage::Apply => self.sel.prompt(),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        let text = match &i {
            Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_string()),
            _ => None,
        };
        let list = |s: &mut Session| -> Result<Step> {
            let names: Vec<String> = s.doc()?.dim_styles.iter().map(|d| d.name.clone()).collect();
            s.echo(format!("Named dimension styles: {}", names.join(", ")));
            Ok(Step::Continue)
        };
        match self.stage {
            DimStage::Option => {
                let k = text.unwrap_or_default().to_ascii_lowercase();
                self.stage = match k.as_str() {
                    "" | "r" | "restore" => DimStage::Restore,
                    "s" | "save" => DimStage::Save,
                    "v" | "variables" => DimStage::Variables,
                    "a" | "apply" => DimStage::Apply,
                    "st" | "status" => {
                        let vars = current_vars(s);
                        for (k, v) in vars {
                            s.echo(format!("{k:<10} {v}"));
                        }
                        return Ok(Step::Done);
                    }
                    "?" => return list(s).map(|_| Step::Done),
                    _ => {
                        s.echo("Invalid option keyword.");
                        DimStage::Option
                    }
                };
                Ok(Step::Continue)
            }
            DimStage::Save => match text.as_deref() {
                Some("?") => list(s),
                Some(name) if !name.is_empty() => {
                    let mut p = current_vars(s);
                    p.insert("name".into(), json!(name));
                    p.insert("current".into(), json!(true));
                    call(s, "dimstyle", Value::Object(p))?;
                    s.echo(format!("Dimension style {name} saved and made current."));
                    Ok(Step::Done)
                }
                _ => Ok(Step::Cancel),
            },
            DimStage::Restore | DimStage::Variables => match text.as_deref() {
                Some("?") => list(s),
                Some(name) if !name.is_empty() => {
                    if self.stage == DimStage::Restore {
                        call(s, "dimstyle.current", json!({ "name": name }))?;
                        s.echo(format!("Current dimension style:  {name}"));
                    } else {
                        let r = call(s, "dimstyle.list", json!({ "name": name }))?;
                        let vars = r.get("styles").and_then(Value::as_array).and_then(|a| a.first()).cloned().unwrap_or(r);
                        s.echo(format!("{vars}"));
                    }
                    Ok(Step::Done)
                }
                _ => Ok(Step::Done),
            },
            DimStage::Apply => match self.sel.feed(s, &i)? {
                super::machines::SelOutcome::More => Ok(Step::Continue),
                super::machines::SelOutcome::Done(hs) => {
                    let handles: Vec<String> = hs.iter().map(|h| h.hex()).collect();
                    call(s, "dimstyle.update", json!({ "handles": handles }))?;
                    Ok(Step::Done)
                }
                super::machines::SelOutcome::Empty => Ok(Step::Done),
            },
        }
    }
}

/// A one-word answer: a space ends it (a script's next word goes to the next prompt).
const WORD: Accept = Accept { text: true, number: true, ..Accept::TEXT };

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum StyleStage {
    #[default]
    Name,
    Font,
    Height,
    Width,
    Oblique,
    Backwards,
    UpsideDown,
}

#[derive(Default)]
struct DashStyle {
    stage: StyleStage,
    p: Map<String, Value>,
}

impl DashStyle {
    fn existing(&self, s: &Session) -> Option<cadaddy_doc::TextStyle> {
        let name = self.p.get("name")?.as_str()?;
        s.doc().ok()?.text_style(name).cloned()
    }
}

impl Interactive for DashStyle {
    fn name(&self) -> &'static str {
        "-STYLE"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        let cur = self.existing(s).unwrap_or_default();
        let yn = |m: &str, v: bool| Prompt::new(m, Accept::TEXT).kw(&["Yes", "No"]).default(if v { "Yes" } else { "No" });
        match self.stage {
            StyleStage::Name => {
                let current = s.doc().map(|d| d.header.str("TEXTSTYLE", "Standard")).unwrap_or_default();
                // A space ends the name, as in AutoCAD scripts (number + text tokenizes).
                Prompt::new("Enter name of text style or [?]", WORD).default(current)
            }
            StyleStage::Font => Prompt::new("Specify full font name or font filename", WORD).default(cur.font),
            StyleStage::Height => Prompt::new("Specify height of text", Accept::NUMBER).default(crate::units::dist(s, cur.height)),
            StyleStage::Width => Prompt::new("Specify width factor", Accept::NUMBER).default(format!("{:.4}", cur.width_factor.max(0.01))),
            StyleStage::Oblique => Prompt::new("Specify obliquing angle", Accept::NUMBER).default(format!("{}", cur.oblique)),
            StyleStage::Backwards => yn("Display text backwards?", cur.backwards),
            StyleStage::UpsideDown => yn("Display text upside-down?", cur.upside_down),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        let text = match &i {
            Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_string()),
            _ => None,
        }
        .filter(|t| !t.is_empty());
        let num = |t: &Option<String>| t.as_deref().and_then(|t| t.parse::<f64>().ok()).filter(|v| v.is_finite());
        match self.stage {
            StyleStage::Name => {
                let name = match text {
                    Some(t) if t == "?" => {
                        let names: Vec<String> = s.doc()?.text_styles.iter().map(|t| t.name.clone()).collect();
                        s.echo(format!("Text styles: {}", names.join(", ")));
                        return Ok(Step::Continue);
                    }
                    // As STYLE checks it (at most 255 characters), asked again here: a bad name
                    // used to be refused only after the last prompt, which then looped (battle
                    // test 0.7.7c).
                    Some(t) if t.len() > 255 => {
                        s.echo("Bad style name.");
                        return Ok(Step::Continue);
                    }
                    Some(t) => t,
                    None => s.doc()?.header.str("TEXTSTYLE", "Standard"),
                };
                self.p.insert("name".into(), json!(name));
                self.stage = StyleStage::Font;
            }
            StyleStage::Font => {
                if let Some(f) = text {
                    self.p.insert("font".into(), json!(f));
                }
                self.stage = StyleStage::Height;
            }
            StyleStage::Height => {
                if let Some(h) = num(&text).filter(|h| *h >= 0.0) {
                    self.p.insert("height".into(), json!(h));
                } else if text.is_some() {
                    s.echo("Requires a number of 0 or more.");
                    return Ok(Step::Continue);
                }
                self.stage = StyleStage::Width;
            }
            StyleStage::Width => {
                if let Some(w) = num(&text).filter(|w| *w > 0.0) {
                    self.p.insert("widthFactor".into(), json!(w));
                } else if text.is_some() {
                    s.echo("Requires a positive number.");
                    return Ok(Step::Continue);
                }
                self.stage = StyleStage::Oblique;
            }
            StyleStage::Oblique => {
                if let Some(o) = num(&text).filter(|o| o.abs() <= 85.0) {
                    self.p.insert("oblique".into(), json!(o));
                } else if text.is_some() {
                    s.echo("Requires an angle from -85 to 85.");
                    return Ok(Step::Continue);
                }
                self.stage = StyleStage::Backwards;
            }
            StyleStage::Backwards | StyleStage::UpsideDown => {
                let key = if self.stage == StyleStage::Backwards { "backwards" } else { "upsideDown" };
                match text.as_deref().map(str::to_ascii_lowercase).as_deref() {
                    Some("y" | "yes") => {
                        self.p.insert(key.into(), json!(true));
                    }
                    Some("n" | "no") => {
                        self.p.insert(key.into(), json!(false));
                    }
                    None => {}
                    Some(_) => {
                        s.echo("Enter Yes or No.");
                        return Ok(Step::Continue);
                    }
                }
                if self.stage == StyleStage::Backwards {
                    self.stage = StyleStage::UpsideDown;
                    return Ok(Step::Continue);
                }
                self.p.insert("current".into(), json!(true));
                let name = self.p.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                // Refused (whatever the reason): the command ends with the message instead of
                // asking its last question again without its answers.
                if let Err(e) = call(s, "style", Value::Object(std::mem::take(&mut self.p))) {
                    s.echo(e.user_message());
                    return Ok(Step::Cancel);
                }
                s.echo(format!("\"{name}\" is now the current text style."));
                return Ok(Step::Done);
            }
        }
        Ok(Step::Continue)
    }
}
