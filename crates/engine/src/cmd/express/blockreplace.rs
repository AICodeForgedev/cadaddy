//! BLOCKREPLACE: every reference to one block becomes a reference to another, keeping its
//! insertion point, scale, rotation, layer and other properties; attributes of the new block
//! take the values of the old reference's attributes with the same tag (else their defaults).
//! References inside other block definitions are replaced too. Optionally the old block is
//! purged afterwards.

use std::sync::Arc;

use cadaddy_doc::{Drawing, EntityKind, Insert};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most block definitions followed when checking nesting.
const MAX_WALK: usize = 100_000;

/// Whether block `outer` references block `inner`, directly or nested.
fn contains(d: &Drawing, outer: &str, inner: &str) -> bool {
    let mut frontier = vec![outer.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(n) = frontier.pop() {
        if seen.len() > MAX_WALK || !seen.insert(n.to_ascii_uppercase()) {
            continue;
        }
        let Some(b) = d.block(&n) else { continue };
        for e in b.entities.iter() {
            if let EntityKind::Insert(i) = &e.kind {
                if i.block.eq_ignore_ascii_case(inner) {
                    return true;
                }
                frontier.push(i.block.clone());
            }
        }
    }
    false
}

/// `ins` (a reference to the old block) as a reference to `to`.
fn replaced(ins: &Insert, to: &cadaddy_doc::Block) -> Insert {
    let mut out = ins.clone();
    out.block = to.name.clone();
    let m = out.transform(to.base.xy());
    out.attribs = super::super::blocks::reference_attribs(to, &m, |tag| {
        ins.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case(tag)).map(|a| a.text.value.clone())
    });
    out
}

/// Replace the references in `d`; returns how many.
fn replace_all(d: &mut Drawing, from: &str, to: &Arc<cadaddy_doc::Block>) -> usize {
    let is_old = |k: &EntityKind| matches!(k, EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(from));
    let mut n = 0;
    let mut stores: Vec<&mut cadaddy_doc::EntityStore> = vec![&mut d.model];
    stores.extend(d.layouts.iter_mut().map(|l| &mut l.entities));
    for (name, b) in d.blocks.iter_mut() {
        // The new block's own definition keeps its references (it can't contain the old one:
        // that is refused before), and the old one's too.
        if name.eq_ignore_ascii_case(&to.name) || name.eq_ignore_ascii_case(from) || !b.entities.iter().any(|e| is_old(&e.kind)) {
            continue;
        }
        stores.push(&mut Arc::make_mut(b).entities);
    }
    for st in stores {
        let hs: Vec<_> = st.iter().filter(|e| is_old(&e.kind)).map(|e| e.handle).collect();
        for h in hs {
            st.modify(h, |e| {
                if let EntityKind::Insert(i) = &e.kind {
                    e.kind = EntityKind::Insert(replaced(i, to));
                }
            });
            n += 1;
        }
    }
    n
}

/// `{from, to, purge?: bool (default true: purge the old block when nothing refers to it)}` →
/// `{replaced, purged}`.
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "blockreplace";
    let from = str_param(p, "from").ok_or_else(|| bad(cmd, "`from` (the block to replace) is required"))?.to_string();
    let to_name = str_param(p, "to").ok_or_else(|| bad(cmd, "`to` (the replacement block) is required"))?.to_string();
    let d = s.doc()?;
    let old = d.block(&from).ok_or_else(|| EngineError::Other(format!("Block \"{from}\" not found.")))?;
    let to = d.block(&to_name).cloned().ok_or_else(|| EngineError::Other(format!("Block \"{to_name}\" not found.")))?;
    let from = old.name.clone();
    if from.eq_ignore_ascii_case(&to.name) {
        return Err(bad(cmd, "the two blocks are the same"));
    }
    if to.name.starts_with('*') || to.anonymous || to.xref_path.is_some() {
        return Err(bad(cmd, format!("\"{}\" can't replace a block", to.name)));
    }
    if contains(d, &to.name, &from) {
        return Err(EngineError::Other(format!("Block \"{}\" contains \"{from}\": it can't replace it.", to.name)));
    }
    let n = replace_all(s.doc_mut()?, &from, &to);
    let mut purged = false;
    if bool_or(p, "purge", true) {
        let spec = super::super::find_command("purge").ok_or_else(|| EngineError::UnknownCommand("purge".into()))?;
        // Only the old block; it stays when something still refers to it.
        let _ = (spec.run)(s, &json!({ "type": "blocks", "names": from }));
        purged = s.doc()?.block(&from).is_none();
    }
    s.echo(format!("{n} block reference(s) replaced."));
    Ok(json!({ "replaced": n, "purged": purged }))
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    From,
    To,
    Purge,
}

/// Typed BLOCKREPLACE: the block to replace and its replacement (a name, or pick a reference),
/// then "Purge unreferenced items when finished?".
#[derive(Clone)]
pub(super) struct BlockReplaceM {
    stage: Stage,
    from: String,
    to: String,
}

impl BlockReplaceM {
    pub(super) fn new() -> Self {
        BlockReplaceM { stage: Stage::From, from: String::new(), to: String::new() }
    }
}

const NAME_OR_PICK: Accept = Accept { point: true, number: false, text: true, select: false, enter: true };
const YES_NO: Accept = Accept { point: false, number: false, text: false, select: false, enter: true };

/// The block named by `i`: a typed name of a block in the drawing, or the block of a reference
/// picked at a point.
fn block_of(s: &Session, i: &Input) -> Result<Option<String>> {
    let d = s.doc()?;
    match i {
        Input::Enter => Ok(None),
        Input::Point(p) => {
            let ap = s.pixel_size() * s.settings.pickbox.max(1.0) * 1.5;
            let h = crate::select::pick(d, &s.space(), *p, ap).ok_or_else(|| EngineError::Other("No block reference there.".into()))?;
            match d.entity(h).map(|e| &e.kind) {
                Some(EntityKind::Insert(ins)) => Ok(Some(ins.block.clone())),
                _ => Err(EngineError::Other("That is not a block reference.".into())),
            }
        }
        Input::Text(t) | Input::Keyword(t) => {
            let name = t.trim();
            d.block(name).map(|b| Some(b.name.clone())).ok_or_else(|| EngineError::Other(format!("Block \"{name}\" not found.")))
        }
        _ => Err(EngineError::Other("Enter a block name or pick a block reference.".into())),
    }
}

impl Interactive for BlockReplaceM {
    fn name(&self) -> &'static str {
        "BLOCKREPLACE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::From => Prompt::new("Enter the name of the block to be replaced, or pick a reference", NAME_OR_PICK),
            Stage::To => Prompt::new(format!("Enter the name of the block to replace \"{}\" with, or pick a reference", self.from), NAME_OR_PICK),
            Stage::Purge => Prompt::new("Purge unreferenced items when finished?", YES_NO).kw(&["Yes", "No"]).default("Yes"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        match self.stage {
            Stage::From => match block_of(s, &i)? {
                None => Ok(Step::Done),
                Some(b) => {
                    self.from = b;
                    self.stage = Stage::To;
                    Ok(Step::Continue)
                }
            },
            Stage::To => match block_of(s, &i)? {
                None => Ok(Step::Done),
                Some(b) if b.eq_ignore_ascii_case(&self.from) => Err(EngineError::Other("Pick a different block.".into())),
                Some(b) => {
                    self.to = b;
                    self.stage = Stage::Purge;
                    Ok(Step::Continue)
                }
            },
            Stage::Purge => {
                let purge = match &i {
                    Input::Enter => true,
                    Input::Text(t) | Input::Keyword(t) => match t.trim().to_ascii_lowercase().chars().next() {
                        Some('y') => true,
                        Some('n') => false,
                        _ => return Err(EngineError::Other("Enter Yes or No.".into())),
                    },
                    _ => return Err(EngineError::Other("Enter Yes or No.".into())),
                };
                run(s, &json!({ "from": self.from, "to": self.to, "purge": purge }))?;
                Ok(Step::Done)
            }
        }
    }
    fn preview(&self, _s: &Session, _c: Vec2) -> Vec<EntityKind> {
        Vec::new()
    }
}
