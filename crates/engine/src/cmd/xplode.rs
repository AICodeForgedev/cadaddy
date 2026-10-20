//! XPLODE: EXPLODE that sets the pieces' colour, layer, linetype and lineweight while it explodes
//! (all of them, or "Inherit from parent block": pieces on layer 0 drawn ByBlock take the exploded
//! object's), for every selected object at once or one by one.

use serde_json::{Map, Value, json};

use super::machines::{SelOutcome, SelectPhase};
use super::{CommandSpec, bad, targets};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};
use cadaddy_color::Color;
use cadaddy_doc::{Common, Handle, Lineweight};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("xplode", "Explode and Set Properties", run)
            .params(
                "{handles? (default: the selection), color?: \"ByLayer\" | \"ByBlock\" | \"red\" | 1..255 | \"r,g,b\", layer?, linetype?, lineweight?: \"ByLayer\" | \"ByBlock\" | mm, inherit?: bool (pieces on layer 0 drawn ByBlock take the object's layer, colour, linetype and lineweight)} → {handles}",
            )
            .interactive(|_| Ok(Box::new(XplodeM::default()))),
    ]
}

/// What happens to the pieces.
#[derive(Clone, Debug, Default)]
struct Options {
    color: Option<Color>,
    layer: Option<String>,
    linetype: Option<String>,
    lineweight: Option<Lineweight>,
    inherit: bool,
}

impl Options {
    fn from(s: &Session, p: &Value) -> Result<Options> {
        let d = s.doc()?;
        let color = p.get("color").map(|c| super::props::parse_color(c).ok_or_else(|| bad("xplode", "bad colour"))).transpose()?;
        let layer = match super::str_param(p, "layer").map(str::trim).filter(|l| !l.is_empty()) {
            Some(l) => Some(d.layer(l).map(|x| x.name.clone()).ok_or_else(|| EngineError::Other(format!("Cannot find layer \"{l}\".")))?),
            None => None,
        };
        let linetype = match super::str_param(p, "linetype").map(str::trim).filter(|l| !l.is_empty()) {
            Some(l) if l.eq_ignore_ascii_case("ByLayer") => Some("ByLayer".to_string()),
            Some(l) if l.eq_ignore_ascii_case("ByBlock") => Some("ByBlock".to_string()),
            Some(l) => Some(d.linetype(l).map(|x| x.name.clone()).ok_or_else(|| EngineError::Other(format!("Linetype {l} not found.")))?),
            None => None,
        };
        let lineweight = p.get("lineweight").map(|v| super::layer::parse_lw(v).ok_or_else(|| bad("xplode", "bad lineweight"))).transpose()?;
        let inherit = p.get("inherit").and_then(Value::as_bool).unwrap_or(false);
        Ok(Options { color, layer, linetype, lineweight, inherit })
    }

    /// A piece of the exploded object `from`.
    fn apply(&self, c: &mut Common, from: &Common) {
        if self.inherit && c.layer == "0" {
            c.layer = from.layer.clone();
            if c.color == Color::ByBlock {
                c.color = from.color;
            }
            if c.linetype.eq_ignore_ascii_case("ByBlock") {
                c.linetype = from.linetype.clone();
            }
            if c.lineweight == Lineweight::ByBlock {
                c.lineweight = from.lineweight;
            }
        }
        if let Some(v) = self.color {
            c.color = v;
        }
        if let Some(v) = &self.layer {
            c.layer = v.clone();
        }
        if let Some(v) = &self.linetype {
            c.linetype = v.clone();
        }
        if let Some(v) = self.lineweight {
            c.lineweight = v;
        }
    }
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad("xplode", "nothing selected"));
    }
    let opts = Options::from(s, p)?;
    let mut out: Vec<Handle> = Vec::new();
    let mut not = 0usize;
    for h in hs {
        let Some(from) = s.doc()?.entity(h).map(|e| e.common.clone()) else { continue };
        let (pieces, n) = super::modify::explode(s, &[h])?;
        not += n;
        let d = s.doc_mut()?;
        for piece in &pieces {
            d.modify_entity(*piece, |e| opts.apply(&mut e.common, &from))?;
        }
        out.extend(pieces);
    }
    // Nothing exploded is an error, as with EXPLODE (no undo step that changed nothing).
    if out.is_empty() && not > 0 {
        return Err(EngineError::Other(if not == 1 {
            "1 was not able to be exploded.".into()
        } else {
            format!("{not} were not able to be exploded.")
        }));
    }
    let mut r = json!({ "handles": out.iter().map(|h| h.hex()).collect::<Vec<_>>() });
    if not > 0 {
        r["message"] = json!(format!("{not} could not be exploded."));
    }
    Ok(r)
}

/// What XPLODE asks next.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Ask {
    #[default]
    Select,
    /// Individually or globally (more than one object).
    How,
    Option,
    Color,
    Layer,
    Linetype,
    Lineweight,
}

/// The option keywords and what each starts with.
const OPTIONS: [&str; 7] = ["All", "Color", "LAyer", "LType", "LWeight", "Inherit from parent block", "Explode"];

/// The option `t` names (AutoCAD's capitals: A, C, LA, LT, LW, I, E).
fn option(t: &str) -> Option<&'static str> {
    let t = t.trim().to_ascii_lowercase();
    match () {
        _ if t.starts_with("la") => Some("LAyer"),
        _ if t.starts_with("lt") => Some("LType"),
        _ if t.starts_with("lw") => Some("LWeight"),
        _ if t.starts_with('a') => Some("All"),
        _ if t.starts_with('c') => Some("Color"),
        _ if t.starts_with('i') => Some("Inherit from parent block"),
        _ if t.starts_with('e') => Some("Explode"),
        _ => None,
    }
}

/// XPLODE at the command line.
#[derive(Default)]
struct XplodeM {
    sel: SelectPhase,
    ask: Ask,
    objs: Vec<Handle>,
    /// One by one: the object whose options are asked for.
    individually: bool,
    at: usize,
    /// The options being collected for the current object (or all of them).
    params: Map<String, Value>,
    /// The values still to ask for (All asks each in turn).
    queue: Vec<Ask>,
}

fn word(i: &Input) -> Option<String> {
    match i {
        Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_string()).filter(|t| !t.is_empty()),
        _ => None,
    }
}

impl XplodeM {
    fn selected(&mut self, hs: Vec<Handle>) {
        self.ask = if hs.len() > 1 { Ask::How } else { Ask::Option };
        self.objs = hs;
    }

    /// The objects the current options are for.
    fn current(&self) -> Vec<Handle> {
        if self.individually { self.objs.get(self.at).copied().into_iter().collect() } else { self.objs.clone() }
    }

    /// The next value to ask for, or run with what was collected.
    fn next(&mut self, s: &mut Session) -> Result<Step> {
        if let Some(a) = self.queue.pop() {
            self.ask = a;
            return Ok(Step::Continue);
        }
        let mut p = std::mem::take(&mut self.params);
        p.insert("handles".into(), json!(self.current().iter().map(|h| h.hex()).collect::<Vec<_>>()));
        let r = run(s, &Value::Object(p))?;
        if let Some(m) = r["message"].as_str() {
            s.echo(m.to_string());
        }
        self.at += 1;
        if self.individually && self.at < self.objs.len() {
            self.ask = Ask::Option;
            return Ok(Step::Continue);
        }
        Ok(Step::Done)
    }

    fn set(&mut self, s: &mut Session, key: &str, v: Value) -> Result<Step> {
        // Checked now, so a wrong name asks again instead of failing at the end.
        let mut probe = Map::new();
        probe.insert(key.into(), v.clone());
        Options::from(s, &Value::Object(probe))?;
        self.params.insert(key.into(), v);
        self.next(s)
    }
}

impl Interactive for XplodeM {
    fn name(&self) -> &'static str {
        "XPLODE"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            let hs = self.sel.picked.clone();
            self.selected(hs);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, s: &Session) -> Prompt {
        let clayer = s.doc().map(|d| d.header.str("CLAYER", "0")).unwrap_or_else(|_| "0".into());
        match self.ask {
            Ask::Select => self.sel.prompt(),
            Ask::How => Prompt::new("Xplode", Accept::TEXT).kw(&["Individually", "Globally"]).default("Globally"),
            Ask::Option => Prompt::new("Enter an option", Accept::TEXT).kw(&OPTIONS).default("Explode"),
            Ask::Color => Prompt::new("Enter new color for exploded objects", Accept::TEXT).default("BYLAYER"),
            Ask::Layer => Prompt::new("Enter new layer name for exploded objects", Accept::TEXT).default(clayer),
            Ask::Linetype => Prompt::new("Enter new linetype name for exploded objects", Accept::TEXT).default("BYLAYER"),
            Ask::Lineweight => Prompt::new("Enter new lineweight for exploded objects", Accept::TEXT).default("BYLAYER"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.ask == Ask::Select {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.selected(hs);
                    Ok(Step::Continue)
                }
            };
        }
        let w = word(&i);
        match self.ask {
            Ask::Select => Ok(Step::Continue),
            Ask::How => {
                self.individually = w.is_some_and(|w| w.to_ascii_lowercase().starts_with('i'));
                self.ask = Ask::Option;
                Ok(Step::Continue)
            }
            Ask::Option => {
                let opt = match &w {
                    None => "Explode",
                    Some(t) => {
                        option(t).ok_or_else(|| bad("xplode", "enter All, Color, LAyer, LType, LWeight, Inherit from parent block or Explode"))?
                    }
                };
                self.params.clear();
                self.queue = match opt {
                    // Asked in this order: Color, LAyer, LType, LWeight.
                    "All" => vec![Ask::Lineweight, Ask::Linetype, Ask::Layer, Ask::Color],
                    "Color" => vec![Ask::Color],
                    "LAyer" => vec![Ask::Layer],
                    "LType" => vec![Ask::Linetype],
                    "LWeight" => vec![Ask::Lineweight],
                    "Inherit from parent block" => {
                        self.params.insert("inherit".into(), json!(true));
                        Vec::new()
                    }
                    _ => Vec::new(),
                };
                self.next(s)
            }
            Ask::Color => self.set(s, "color", json!(w.unwrap_or_else(|| "BYLAYER".into()))),
            Ask::Layer => {
                let clayer = s.doc()?.header.str("CLAYER", "0");
                self.set(s, "layer", json!(w.unwrap_or(clayer)))
            }
            Ask::Linetype => self.set(s, "linetype", json!(w.unwrap_or_else(|| "BYLAYER".into()))),
            Ask::Lineweight => {
                let w = w.unwrap_or_else(|| "BYLAYER".into());
                let v = w.parse::<f64>().map_or_else(|_| json!(w), |n| json!(n));
                self.set(s, "lineweight", v)
            }
        }
    }
}
