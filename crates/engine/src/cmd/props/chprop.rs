//! CHPROP and CHANGE at the command line: change the selected objects' properties one by one
//! (Color, LAyer, LType, ltScale, LWeight, TRansparency) until Enter, as AutoCAD scripts do.
//! CHANGE also takes a change point: line ends nearest it move to it, circles pass through it.

use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase, number};
use super::run_set;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};
use cadaddy_doc::{EntityKind, Handle};
use cadaddy_geom::Vec2;

/// A name or value typed as one word: in a script a space ends it, as in AutoCAD.
const WORD: Accept = Accept { text: true, number: true, ..Accept::TEXT };

const PROPS: [&str; 6] = ["Color", "LAyer", "LType", "ltScale", "LWeight", "TRansparency"];

pub(super) struct ChpropM {
    change: bool,
    sel: SelectPhase,
    objs: Vec<Handle>,
    /// CHANGE: the Properties option was chosen (then it is CHPROP).
    props: bool,
    /// The property whose new value is asked for.
    asking: Option<&'static str>,
}

impl ChpropM {
    pub(super) fn new(change: bool) -> Self {
        ChpropM { change, sel: SelectPhase::default(), objs: Vec::new(), props: !change, asking: None }
    }
    fn set(&self, s: &mut Session, key: &str, v: Value) -> Result<()> {
        let mut p = serde_json::Map::new();
        p.insert("handles".into(), json!(self.objs.iter().map(|h| h.hex()).collect::<Vec<_>>()));
        p.insert(key.into(), v);
        run_set(s, &Value::Object(p)).map(|_| ())
    }
    /// CHANGE's change point: line ends nearest it move to it; circles take it on their edge.
    fn change_point(&self, s: &mut Session, at: Vec2) -> Result<()> {
        let d = s.doc_mut()?;
        for h in &self.objs {
            let locked = d.entity(*h).and_then(|e| d.layer(&e.common.layer)).is_some_and(|l| l.locked);
            if locked {
                continue;
            }
            d.modify_entity(*h, |e| match &mut e.kind {
                EntityKind::Line(l) => {
                    if l.a.xy().dist(at) <= l.b.xy().dist(at) {
                        l.a = at.to3(l.a.z);
                    } else {
                        l.b = at.to3(l.b.z);
                    }
                }
                EntityKind::Circle(c) => {
                    let r = c.center.xy().dist(at);
                    if r > 1e-12 {
                        c.radius = r;
                    }
                }
                _ => {}
            })?;
        }
        Ok(())
    }
}

impl Interactive for ChpropM {
    fn name(&self) -> &'static str {
        if self.change { "CHANGE" } else { "CHPROP" }
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.objs = self.sel.picked.clone();
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if !self.sel.done {
            return self.sel.prompt();
        }
        match self.asking {
            Some("Color") => Prompt::new("Enter new color", WORD).default("BYLAYER"),
            Some("LAyer") => Prompt::new("Enter new layer name", WORD),
            Some("LType") => Prompt::new("Enter new linetype name", WORD).default("ByLayer"),
            Some("ltScale") => Prompt::new("Specify new linetype scale", Accept::NUMBER).default("1.0000"),
            Some("LWeight") => Prompt::new("Enter new lineweight", WORD).default("ByLayer"),
            Some(_) => Prompt::new("Enter transparency value (0-90)", WORD).kw(&["ByLayer", "ByBlock"]).default("ByLayer"),
            None if self.props => Prompt::new("Enter property to change", WORD).kw(&PROPS),
            None => Prompt::new("Specify change point", Accept::POINT).kw(&["Properties"]),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if !self.sel.done {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.objs = hs;
                    Ok(Step::Continue)
                }
            };
        }
        let word = match &i {
            Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_string()),
            _ => None,
        };
        if let Some(prop) = self.asking.take() {
            let Some(v) = word.filter(|w| !w.is_empty()) else {
                // Enter keeps the objects as they are.
                return Ok(Step::Continue);
            };
            let num = number(&v).filter(|n| n.is_finite());
            // Whole numbers go on as integers (colour indexes, lineweights in 1/100 mm).
            let as_json = |n: f64| if n.fract() == 0.0 && n.abs() < 1e15 { json!(n as i64) } else { json!(n) };
            match prop {
                "Color" => self.set(s, "color", num.map_or_else(|| json!(v), as_json))?,
                "LAyer" => {
                    if s.doc()?.layer(&v).is_none() {
                        return Err(EngineError::Other(format!("Cannot find layer \"{v}\".")));
                    }
                    self.set(s, "layer", json!(v))?;
                }
                "LType" => self.set(s, "linetype", json!(v))?,
                "ltScale" => {
                    self.set(s, "ltscale", json!(num.filter(|n| *n > 0.0).ok_or_else(|| EngineError::Other("Requires a positive number.".into()))?))?
                }
                "LWeight" => self.set(s, "lineweight", num.map_or_else(|| json!(v), as_json))?,
                _ => self.set(s, "transparency", num.map_or_else(|| json!(v), as_json))?,
            }
            return Ok(Step::Continue);
        }
        match (i, word) {
            (Input::Enter, _) | (Input::Cancel, _) => {
                s.set_selection(Vec::new());
                Ok(Step::Done)
            }
            (Input::Point(p), _) if !self.props => {
                self.change_point(s, p)?;
                Ok(Step::Continue)
            }
            (_, Some(w)) => {
                let lw = w.to_ascii_lowercase();
                if !self.props {
                    if "properties".starts_with(&lw) && !lw.is_empty() {
                        self.props = true;
                        return Ok(Step::Continue);
                    }
                    return Err(EngineError::Other("Invalid option keyword.".into()));
                }
                // The keyword's capitals are what may be typed (LA = LAyer, LT = LType...).
                let hit = PROPS.iter().find(|k| {
                    let short: String = k.chars().filter(char::is_ascii_uppercase).collect::<String>().to_ascii_lowercase();
                    lw == k.to_ascii_lowercase() || lw == short || (lw.len() >= short.len() && k.to_ascii_lowercase().starts_with(&lw))
                });
                match hit {
                    Some(k) => self.asking = Some(k),
                    None => return Err(EngineError::Other("Invalid option keyword.".into())),
                }
                Ok(Step::Continue)
            }
            _ => Ok(Step::Continue),
        }
    }
}

#[cfg(test)]
mod tests {
    use cadaddy_doc::EntityKind;

    use crate::Session;

    #[test]
    fn chprop_and_change() {
        let mut s = Session::new();
        s.execute("layer.new", &serde_json::json!({ "name": "WALLS" })).unwrap();
        s.cmdline("LINE 0,0 10,0").unwrap();
        s.cmdline("").unwrap();
        s.script("CHPROP 5,0\n\nLA WALLS C 3\n\n").unwrap();
        assert!(s.running.is_none(), "{}", s.prompt_text());
        let e = s.doc().unwrap().model.iter().next().unwrap().clone();
        assert_eq!(e.common.layer, "WALLS");
        assert_eq!(e.common.color, cadaddy_color::Color::Index(3));
        // A missing layer is refused.
        assert!(s.script("CHPROP 5,0\n\nLA NOPE\n").is_err() || s.doc().unwrap().layer("NOPE").is_none());
        s.cancel();
        s.set_selection(Vec::new());
        // CHANGE: the line end nearest the change point moves to it.
        s.script("CHANGE 5,0\n\n12,3\n\n").unwrap();
        assert!(s.running.is_none(), "{}", s.prompt_text());
        let EntityKind::Line(l) = &s.doc().unwrap().model.iter().next().unwrap().kind else { panic!() };
        assert_eq!((l.b.x, l.b.y), (12.0, 3.0));
    }
}
