//! SETBYLAYER: the selected objects' colour, linetype, lineweight, material, plot style and
//! transparency become ByLayer (which ones: SETBYLAYERMODE). ByBlock values change too when
//! asked, and with blocks included so do the objects inside the selected references' block
//! definitions, nested ones too.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase};
use super::{CommandSpec, bad, targets};
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};
use cadaddy_color::Color;
use cadaddy_doc::{Common, EntityKind, Handle, Lineweight, Transparency};

/// SETBYLAYERMODE bits (AutoCAD's).
const LINETYPE: i64 = 1;
const LINEWEIGHT: i64 = 2;
const MATERIAL: i64 = 4;
const COLOR: i64 = 8;
const PLOT_STYLE: i64 = 16;
const BY_BLOCK: i64 = 32;
const BLOCKS: i64 = 64;
const TRANSPARENCY: i64 = 128;

/// Most block definitions one SETBYLAYER walks into (nested references included).
const MAX_BLOCKS: usize = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("setbylayer", "Set to ByLayer", run)
            .params(
                "{handles? (default: the selection), color?, linetype?, lineweight?, material?, plotStyle?, transparency?: bool (which properties; default SETBYLAYERMODE), byBlock?: bool (ByBlock values too), blocks?: bool (the objects inside selected block references' definitions)} → {changed, blocks}",
            )
            .interactive(|_| Ok(Box::new(SetByLayerM::default()))),
    ]
}

fn mode(s: &Session) -> i64 {
    s.doc().map(|d| d.header.i64("SETBYLAYERMODE", 255)).unwrap_or(255)
}

/// Which properties change and whether ByBlock ones do.
#[derive(Clone, Copy, Debug)]
struct Which {
    color: bool,
    linetype: bool,
    lineweight: bool,
    material: bool,
    plot_style: bool,
    transparency: bool,
    by_block: bool,
}

impl Which {
    fn from(mode: i64, p: &Value) -> Which {
        let on = |key: &str, bit: i64| p.get(key).and_then(Value::as_bool).unwrap_or(mode & bit != 0);
        Which {
            color: on("color", COLOR),
            linetype: on("linetype", LINETYPE),
            lineweight: on("lineweight", LINEWEIGHT),
            material: on("material", MATERIAL),
            plot_style: on("plotStyle", PLOT_STYLE),
            transparency: on("transparency", TRANSPARENCY),
            by_block: on("byBlock", BY_BLOCK),
        }
    }

    /// The names AutoCAD lists as the current settings.
    fn names(&self) -> Vec<&'static str> {
        [
            (self.color, "Color"),
            (self.linetype, "Linetype"),
            (self.lineweight, "Lineweight"),
            (self.material, "Material"),
            (self.plot_style, "Plotstyle"),
            (self.transparency, "Transparency"),
        ]
        .into_iter()
        .filter_map(|(on, n)| on.then_some(n))
        .collect()
    }

    /// Whether a value that is ByBlock (`by_block`) or ByLayer (`by_layer`) changes.
    fn takes(&self, by_layer: bool, by_block: bool) -> bool {
        !by_layer && (self.by_block || !by_block)
    }

    /// Set `c`'s properties to ByLayer; whether anything changed.
    fn apply(&self, c: &mut Common) -> bool {
        let mut changed = false;
        if self.color && self.takes(c.color == Color::ByLayer, c.color == Color::ByBlock) {
            c.color = Color::ByLayer;
            changed = true;
        }
        if self.linetype && self.takes(c.linetype.eq_ignore_ascii_case("ByLayer"), c.linetype.eq_ignore_ascii_case("ByBlock")) {
            c.linetype = "ByLayer".into();
            changed = true;
        }
        if self.lineweight && self.takes(c.lineweight == Lineweight::ByLayer, c.lineweight == Lineweight::ByBlock) {
            c.lineweight = Lineweight::ByLayer;
            changed = true;
        }
        let material_by_block = c.material.as_deref().is_some_and(|m| m.eq_ignore_ascii_case("ByBlock"));
        if self.material && self.takes(c.material.is_none(), material_by_block) {
            c.material = None;
            changed = true;
        }
        if self.plot_style && self.takes(c.plot_style.eq_ignore_ascii_case("ByLayer"), c.plot_style.eq_ignore_ascii_case("ByBlock")) {
            c.plot_style = "ByLayer".into();
            changed = true;
        }
        if self.transparency && self.takes(c.transparency == Transparency::ByLayer, c.transparency == Transparency::ByBlock) {
            c.transparency = Transparency::ByLayer;
            changed = true;
        }
        changed
    }

    /// A block reference's attributes: their own colour, linetype and lineweight.
    fn apply_attribs(&self, kind: &mut EntityKind) -> bool {
        let EntityKind::Insert(i) = kind else { return false };
        let mut changed = false;
        for a in &mut i.attribs {
            let p = &mut a.props;
            if self.color && p.color.is_some_and(|c| self.takes(c == Color::ByLayer, c == Color::ByBlock)) {
                p.color = Some(Color::ByLayer);
                changed = true;
            }
            if self.linetype
                && p.linetype.as_deref().is_some_and(|l| self.takes(l.eq_ignore_ascii_case("ByLayer"), l.eq_ignore_ascii_case("ByBlock")))
            {
                p.linetype = Some("ByLayer".into());
                changed = true;
            }
            if self.lineweight && p.lineweight.is_some_and(|l| self.takes(l == Lineweight::ByLayer, l == Lineweight::ByBlock)) {
                p.lineweight = Some(Lineweight::ByLayer);
                changed = true;
            }
        }
        changed
    }
}

/// The block a reference shows (none for other objects).
fn block_of(kind: &EntityKind) -> Option<String> {
    match kind {
        EntityKind::Insert(i) => Some(i.block.clone()),
        _ => None,
    }
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad("setbylayer", "nothing selected"));
    }
    let mode = mode(s);
    let which = Which::from(mode, p);
    let blocks = p.get("blocks").and_then(Value::as_bool).unwrap_or(mode & BLOCKS != 0);
    let d = s.doc_mut()?;
    // Objects on locked layers stay as they are, as with every other change.
    let locked: HashSet<String> = d.layers.iter().filter(|l| l.locked).map(|l| l.name.to_ascii_lowercase()).collect();
    let mut changed = 0usize;
    let mut skipped = 0usize;
    let mut queue: Vec<String> = Vec::new();
    for h in &hs {
        let Some(e) = d.entity(*h) else { continue };
        if locked.contains(&e.common.layer.to_ascii_lowercase()) {
            skipped += 1;
            continue;
        }
        queue.extend(block_of(&e.kind));
        let mut did = false;
        d.modify_entity(*h, |e| {
            did = which.apply(&mut e.common) | which.apply_attribs(&mut e.kind);
        })?;
        changed += usize::from(did);
    }
    // The objects inside the referenced blocks, nested ones too (each definition once).
    let mut seen: HashSet<String> = HashSet::new();
    let mut blocks_changed = 0usize;
    if !blocks {
        queue.clear();
    }
    while let Some(name) = queue.pop() {
        if seen.len() >= MAX_BLOCKS || !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let Some(block) = d.blocks.get_mut(&name) else { continue };
        // Another drawing's blocks (xrefs) are that drawing's to change.
        if block.xref_path.is_some() || block.name.contains('|') {
            continue;
        }
        let inner: Vec<(Handle, Option<String>)> = block.entities.iter().map(|e| (e.handle, block_of(&e.kind))).collect();
        let b = Arc::make_mut(block);
        let mut any = false;
        for (h, nested) in inner {
            queue.extend(nested);
            b.entities.modify(h, |e| {
                any |= which.apply(&mut e.common) | which.apply_attribs(&mut e.kind);
            });
        }
        blocks_changed += usize::from(any);
    }
    let mut message = format!("{changed} object(s) changed to ByLayer.");
    if blocks_changed > 0 {
        message.push_str(&format!(" {blocks_changed} block definition(s) changed."));
    }
    if skipped > 0 {
        message.push_str(&format!(" {skipped} on a locked layer."));
    }
    Ok(json!({ "changed": changed, "blocks": blocks_changed, "message": message }))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Ask {
    #[default]
    Select,
    Settings,
    ByBlock,
    Blocks,
}

/// SETBYLAYER at the command line: the objects (or Settings), then "Change ByBlock to ByLayer?"
/// and "Include blocks?".
#[derive(Default)]
struct SetByLayerM {
    sel: SelectPhase,
    ask: Ask,
    objs: Vec<Handle>,
    by_block: bool,
}

/// Yes or No, from a keyword or the typed word; `None` for anything else.
fn yes_no(i: &Input) -> Option<bool> {
    let (Input::Keyword(t) | Input::Text(t)) = i else { return None };
    match t.trim().chars().next().map(|c| c.to_ascii_lowercase()) {
        Some('y') => Some(true),
        Some('n') => Some(false),
        _ => None,
    }
}

fn yes_no_text(b: bool) -> &'static str {
    if b { "Yes" } else { "No" }
}

impl SetByLayerM {
    fn echo_settings(s: &mut Session) {
        let names = Which::from(mode(s), &Value::Null).names();
        s.echo(format!("Current active settings: {}", names.join(" ")));
    }

    fn selected(&mut self, hs: Vec<Handle>) {
        self.objs = hs;
        self.ask = Ask::ByBlock;
    }

    fn finish(&self, s: &mut Session, blocks: bool) -> Result<Step> {
        let hs: Vec<String> = self.objs.iter().map(|h| h.hex()).collect();
        let r = run(s, &json!({ "handles": hs, "byBlock": self.by_block, "blocks": blocks }))?;
        if let Some(m) = r["message"].as_str() {
            s.echo(m.to_string());
        }
        Ok(Step::Done)
    }
}

impl Interactive for SetByLayerM {
    fn name(&self) -> &'static str {
        "SETBYLAYER"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        Self::echo_settings(s);
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            let hs = self.sel.picked.clone();
            self.selected(hs);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, s: &Session) -> Prompt {
        let m = mode(s);
        match self.ask {
            Ask::Select if self.sel.removing => self.sel.prompt(),
            Ask::Select => self.sel.prompt().kw(&["Settings"]),
            Ask::Settings => Prompt::new("Enter new value for SETBYLAYERMODE", Accept::NUMBER).default(m.to_string()),
            Ask::ByBlock => Prompt::new("Change ByBlock to ByLayer?", Accept::TEXT).kw(&["Yes", "No"]).default(yes_no_text(m & BY_BLOCK != 0)),
            Ask::Blocks => Prompt::new("Include blocks?", Accept::TEXT).kw(&["Yes", "No"]).default(yes_no_text(m & BLOCKS != 0)),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.ask {
            Ask::Select => {
                if let Input::Keyword(k) | Input::Text(k) = &i
                    && "settings".starts_with(&k.trim().to_ascii_lowercase())
                    && !k.trim().is_empty()
                {
                    self.ask = Ask::Settings;
                    return Ok(Step::Continue);
                }
                match self.sel.feed(s, &i)? {
                    SelOutcome::More => Ok(Step::Continue),
                    SelOutcome::Empty => Ok(Step::Done),
                    SelOutcome::Done(hs) => {
                        self.selected(hs);
                        Ok(Step::Continue)
                    }
                }
            }
            Ask::Settings => {
                if let Input::Text(t) = &i {
                    let v = t
                        .trim()
                        .parse::<i64>()
                        .ok()
                        .filter(|v| (0..=255).contains(v))
                        .ok_or_else(|| bad("setbylayer", "SETBYLAYERMODE is 0 to 255"))?;
                    s.doc_mut()?.header.set_i64("SETBYLAYERMODE", v);
                }
                Self::echo_settings(s);
                self.ask = Ask::Select;
                Ok(Step::Continue)
            }
            Ask::ByBlock => {
                self.by_block = match i {
                    Input::Enter => mode(s) & BY_BLOCK != 0,
                    _ => yes_no(&i).ok_or_else(|| bad("setbylayer", "answer Yes or No"))?,
                };
                self.ask = Ask::Blocks;
                Ok(Step::Continue)
            }
            Ask::Blocks => {
                let blocks = match i {
                    Input::Enter => mode(s) & BLOCKS != 0,
                    _ => yes_no(&i).ok_or_else(|| bad("setbylayer", "answer Yes or No"))?,
                };
                self.finish(s, blocks)
            }
        }
    }
}
