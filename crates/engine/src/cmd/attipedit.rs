//! ATTIPEDIT: change one attribute's value of a block reference in place, picked on its text.

use serde_json::{Value, json};

use super::curves::{handle_param, pick_at};
use super::{CommandSpec, bad, point_param, str_param};
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};
use cadaddy_doc::{EntityKind, Handle};
use cadaddy_geom::Vec2;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("attipedit", "Edit Attribute in Place", run)
            .params("{handle (a block reference), tag | at: [x,y] (a point on the attribute's text), value} → {tag, value}")
            .interactive(|_| Ok(Box::new(AttipeditM::default()))),
    ]
}

/// The attribute of reference `h` with `tag` (any case), or whose text holds `at`.
fn find(s: &Session, h: Handle, tag: Option<&str>, at: Option<Vec2>) -> Result<usize> {
    let d = s.doc()?;
    let Some(EntityKind::Insert(ins)) = d.entity(h).map(|e| &e.kind) else {
        return Err(bad("attipedit", "not a block reference"));
    };
    let i = match (tag, at) {
        (Some(t), _) => ins.attribs.iter().position(|a| a.tag.eq_ignore_ascii_case(t)),
        (None, Some(p)) => crate::select::attrib_at(d, ins, p),
        (None, None) => return Err(bad("attipedit", "give `tag` or `at`")),
    };
    let i = i.ok_or_else(|| EngineError::Other("Not an attribute.".into()))?;
    if ins.attribs.get(i).is_some_and(|a| a.constant) {
        return Err(EngineError::Other("A constant attribute can't be edited.".into()));
    }
    Ok(i)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let h = handle_param(p, "handle").ok_or_else(|| bad("attipedit", "`handle` is required"))?;
    let value = match p.get("value") {
        Some(Value::String(v)) => v.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err(bad("attipedit", "`value` is required")),
    };
    if super::curves::is_locked(s, h) {
        return Err(EngineError::Other("The object is on a locked layer.".into()));
    }
    let i = find(s, h, str_param(p, "tag"), point_param(p, "at"))?;
    let mut tag = String::new();
    s.doc_mut()?.modify_entity(h, |e| {
        if let EntityKind::Insert(ins) = &mut e.kind
            && let Some(a) = ins.attribs.get_mut(i)
        {
            a.text.value = value.clone();
            tag = a.tag.clone();
        }
    })?;
    Ok(json!({ "tag": tag, "value": value }))
}

/// ATTIPEDIT at the command line: pick the attribute's text, then type its new value.
#[derive(Default)]
struct AttipeditM {
    /// The reference and the attribute's tag, once picked.
    picked: Option<(Handle, String, String)>,
}

impl Interactive for AttipeditM {
    fn name(&self) -> &'static str {
        "ATTIPEDIT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match &self.picked {
            None => Prompt::new("Select attribute to edit", Accept::POINT),
            Some((_, _, value)) => Prompt::new("Enter attribute value", Accept::TEXT).default(value.clone()),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (&self.picked, i) {
            (None, Input::Point(p)) => {
                let h = pick_at(s, p).ok_or_else(|| EngineError::Other("Not an attribute.".into()))?;
                let at = find(s, h, None, Some(p))?;
                let (tag, value) = match s.doc()?.entity(h).map(|e| &e.kind) {
                    Some(EntityKind::Insert(ins)) => ins.attribs.get(at).map(|a| (a.tag.clone(), a.text.value.clone())).unwrap_or_default(),
                    _ => return Err(EngineError::Other("Not an attribute.".into())),
                };
                self.picked = Some((h, tag, value));
                Ok(Step::Continue)
            }
            (None, Input::Enter) => Ok(Step::Done),
            (None, _) => Err(EngineError::Other("Select an attribute.".into())),
            // Enter keeps the value.
            (Some(_), Input::Enter) => Ok(Step::Done),
            (Some((h, tag, _)), Input::Text(v) | Input::Keyword(v)) => {
                run(s, &json!({ "handle": h.hex(), "tag": tag, "value": v }))?;
                Ok(Step::Done)
            }
            (Some(_), _) => Err(EngineError::Other("Enter the attribute's value.".into())),
        }
    }
}
