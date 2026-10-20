//! Express Tools IV: BCOUNT, GATTE, GETSEL, SSX, XDLIST, XLIST, SHOWURLS, REDIR, REDIRMODE,
//! MPEDIT, LSP, LSPSURF, REVERT, TFRAMES, BSCALE, BROTATE, VPSYNC and DIMREASSOC.
//!
//! BCOUNT counts block references by name, those nested in other blocks included (a block's own
//! count is worked out once, so deep nesting stays linear). GATTE sets one attribute's value in
//! every reference of a block. GETSEL selects by layer and object type; SSX selects what is like
//! an object (type, layer, colour and linetype). XDLIST lists an object's extended data, XLIST
//! the object nested in a block reference at a point. SHOWURLS lists every hyperlink. REDIR
//! changes a folder in the paths of xrefs, images and underlays, text style fonts and RTEXT files
//! (REDIRMODE picks which). LSP lists what LISP code defined; LSPSURF the functions a `.lsp` file
//! defines. REVERT goes back to the drawing as last saved. TFRAMES toggles image and wipeout
//! frames. BSCALE and BROTATE set block references' scale and rotation about their insertion
//! points. VPSYNC lines viewports up with a master viewport (same scale, model space continuing
//! across). DIMREASSOC brings back dimensions' measured text.

use std::collections::BTreeMap;

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Insert};
use cadaddy_geom::{Mat3, Vec2, Vec3};
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase, SelectRun};
use super::*;
use crate::lisp::value::Value as LispValue;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most objects one call looks at.
const MAX_SCAN: usize = 2_000_000;
/// Most block definitions BCOUNT works through.
const MAX_BLOCKS: usize = 100_000;

fn other(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}

fn hexes(hs: &[Handle]) -> Vec<String> {
    hs.iter().map(|h| h.hex()).collect()
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bcount", "Count Blocks", run_bcount)
            .menu(&["Tools", "Express Tools", "Count Blocks"])
            .params("{handles? (default the selection, else every object of the current space), nested?: bool (default true: references inside blocks count too)} → {counts: {block: n}, total}")
            .noundo()
            .interactive(|_| Ok(Box::new(BcountM::default()))),
        CommandSpec::new("gatte", "Global Attribute Edit", run_gatte)
            .menu(&["Tools", "Express Tools", "Global Attribute Edit"])
            .params("{block, tag, value, handles? (only these references; default every reference of the block in the drawing)} → {changed}")
            .interactive(|_| Ok(Box::new(GatteM::default()))),
        CommandSpec::new("getsel", "Get Selection Set", run_getsel)
            .menu(&["Tools", "Express Tools", "Get Selection Set"])
            .params("{layer? (wildcard; default *), type? (object type, wildcard: LINE, *TEXT…; default *)} → selects the matching objects of the current space: {selected, handles}")
            .noundo()
            .interactive(|_| Ok(Box::new(GetselM::default()))),
        CommandSpec::new("ssx", "Select Similar by Properties", run_ssx)
            .menu(&["Tools", "Express Tools", "Select Similar by Properties"])
            .params("{handle (the object to match), match?: [\"type\", \"layer\", \"color\", \"linetype\"] (default all four)} → selects the objects of the current space like it: {selected, handles}")
            .noundo()
            .interactive(|_| Ok(Box::new(SelectRun::single("ssx", "SSX", "Select object")))),
        CommandSpec::new("xdlist", "List Object Xdata", run_xdlist)
            .menu(&["Tools", "Express Tools", "List Object Xdata"])
            .params("{handle, app? (an application name; default all)} → {apps: [{app, tags: [{code, value}]}]}")
            .noundo()
            .interactive(|_| Ok(Box::new(SelectRun::single("xdlist", "XDLIST", "Select object")))),
        CommandSpec::new("xlist", "List Nested Object", run_xlist)
            .menu(&["Tools", "Express Tools", "List Nested Object"])
            .params("{handle (a block or xref reference), at?: [x, y] (the nested object nearest this point; default the first)} → {type, block, layer, color, linetype}")
            .noundo()
            .interactive(|_| Ok(Box::new(XlistM))),
        CommandSpec::new("showurls", "Show URLs", run_showurls)
            .menu(&["Tools", "Express Tools", "Show URLs"])
            .params("{} → {links: [{handle, url, target, description}]}: every hyperlink in the drawing")
            .noundo(),
        CommandSpec::new("redir", "Redefine Path", run_redir)
            .menu(&["Tools", "Express Tools", "Redefine Path"])
            .params("{from (the folder to replace; \"*\" any folder), to (the new folder), kinds?: [\"styles\", \"xrefs\", \"images\", \"rtext\"] (default REDIRMODE)} → {changed: [{kind, name, from, to}]}")
            .interactive(|_| Ok(Box::new(RedirM::default()))),
        CommandSpec::new("redirmode", "Redefine Path Mode", run_redirmode)
            .menu(&["Tools", "Express Tools", "Redefine Path Mode"])
            .params("{kinds?: [\"styles\", \"xrefs\", \"images\", \"rtext\"] (absent: read)} → {kinds}: what REDIR changes")
            .noundo()
            .interactive(|_| Ok(Box::new(RedirmodeM))),
        CommandSpec::new("mpedit", "Multiple Polyline Edit", super::modify2::run_pedit)
            .menu(&["Tools", "Express Tools", "Multiple Polyline Edit"])
            .params("{handles, option: close|open|join|width|fit|spline|decurve|reverse|ltypegen, width?, fuzz?, convert?: bool} → as PEDIT, for each polyline")
            .interactive(|_| Ok(super::modify2::pedit_multiple())),
        CommandSpec::new("lsp", "List LISP Symbols", run_lsp)
            .menu(&["Tools", "Express Tools", "List LISP Symbols"])
            .params("{kind?: \"all\" (default) | \"commands\" | \"functions\" | \"variables\", pattern? (wildcard; default *)} → {commands, functions, variables: [{name, value}]}: what LISP code defined in this drawing")
            .noundo()
            .interactive(|_| Ok(Box::new(LspM))),
        CommandSpec::new("lspsurf", "LISP Function Surfer", run_lspsurf)
            .menu(&["Tools", "Express Tools", "LISP Function Surfer"])
            .params("{path | text (LISP source), pattern? (wildcard; default *)} → {functions: [{name, params, line}]}: the functions the file defines")
            .noundo()
            .interactive(|_| Ok(Box::new(LspsurfM))),
        CommandSpec::new("revert", "Revert", run_revert)
            .menu(&["Tools", "Express Tools", "Revert"])
            .params("{} → the drawing as last saved (Undo brings the changes back): {reverted}"),
        CommandSpec::new("tframes", "Toggle Frames", run_tframes)
            .menu(&["Tools", "Express Tools", "Toggle Frames"])
            .params("{on?: bool (default: the other way from now)} → IMAGEFRAME and WIPEOUTFRAME: {imageframe, wipeoutframe}"),
        CommandSpec::new("bscale", "Scale Blocks", run_bscale)
            .menu(&["Tools", "Express Tools", "Scale Blocks"])
            .params("{handles? (block references; default the selection), factor, relative?: bool (default false: the scale becomes factor; true: multiplied by it)} → {handles}: each scaled about its insertion point")
            .interactive(|_| Ok(Box::new(BlockXformM::new(Xform::Scale)))),
        CommandSpec::new("brotate", "Rotate Blocks", run_brotate)
            .menu(&["Tools", "Express Tools", "Rotate Blocks"])
            .params("{handles? (block references; default the selection), angle (degrees), relative?: bool (default false: the rotation becomes angle; true: turned by it)} → {handles}: each about its insertion point")
            .interactive(|_| Ok(Box::new(BlockXformM::new(Xform::Rotate)))),
        CommandSpec::new("vpsync", "Synchronize Viewports", run_vpsync)
            .menu(&["Tools", "Express Tools", "Synchronize Viewports"])
            .params("{master (viewport handle), handles (viewports to line up)} → {handles}: each at the master's scale and twist, its view continuing the master's across the sheet")
            .interactive(|_| Ok(Box::new(VpsyncM::default()))),
        CommandSpec::new("dimreassoc", "Restore Dimension Text", run_dimreassoc)
            .menu(&["Tools", "Express Tools", "Restore Dimension Text"])
            .params("{handles? (dimensions; default the selection)} → {handles}: those whose text override was removed (they show the measurement again)")
            .interactive(|_| Ok(Box::new(SelectRun::new("dimreassoc", "DIMREASSOC")))),
    ]
}

// ---------- BCOUNT ----------

/// Add `n` times each count of `from` to `into`.
fn add_counts(into: &mut BTreeMap<String, u64>, from: &BTreeMap<String, u64>, n: u64) {
    for (k, v) in from {
        let e = into.entry(k.clone()).or_insert(0);
        *e = e.saturating_add(v.saturating_mul(n));
    }
}

/// The references nested in block `name` (its own references and theirs), by block name.
fn nested_counts(d: &Drawing, name: &str, depth: usize, memo: &mut BTreeMap<String, BTreeMap<String, u64>>) -> BTreeMap<String, u64> {
    let key = name.to_ascii_uppercase();
    if let Some(c) = memo.get(&key) {
        return c.clone();
    }
    let mut out = BTreeMap::new();
    if depth >= cadaddy_doc::MAX_BLOCK_DEPTH || memo.len() >= MAX_BLOCKS {
        return out;
    }
    // A cycle gets nothing more on its way back in.
    memo.insert(key.clone(), BTreeMap::new());
    if let Some(b) = d.block(name) {
        for e in b.entities.iter().take(MAX_SCAN) {
            if let EntityKind::Insert(i) = &e.kind {
                count_one(&mut out, &i.block, 1);
                let inner = nested_counts(d, &i.block, depth + 1, memo);
                add_counts(&mut out, &inner, 1);
            }
        }
    }
    memo.insert(key, out.clone());
    out
}

fn count_one(into: &mut BTreeMap<String, u64>, name: &str, n: u64) {
    // Anonymous blocks (dimensions, hatches, dynamic blocks' variants) aren't the user's blocks.
    if !name.starts_with('*') {
        let e = into.entry(name.to_string()).or_insert(0);
        *e = e.saturating_add(n);
    }
}

fn run_bcount(s: &mut Session, p: &Value) -> Result<Value> {
    let nested = bool_or(p, "nested", true);
    let mut hs = targets(s, p)?;
    let d = s.doc()?;
    if hs.is_empty() {
        hs = d.space(&s.space()).map(|st| st.iter().take(MAX_SCAN).map(|e| e.handle).collect()).unwrap_or_default();
    }
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut memo = BTreeMap::new();
    for h in hs.iter().take(MAX_SCAN) {
        let Some(e) = d.entity(*h) else { continue };
        let EntityKind::Insert(i) = &e.kind else { continue };
        count_one(&mut counts, &i.block, 1);
        if nested {
            let inner = nested_counts(d, &i.block, 1, &mut memo);
            add_counts(&mut counts, &inner, 1);
        }
    }
    let total = counts.values().fold(0u64, |a, b| a.saturating_add(*b));
    let width = counts.keys().map(|k| k.chars().count()).max().unwrap_or(5).max(5);
    let mut message = format!("{:<width$}  Count\n{}", "Block", "-".repeat(width + 7));
    for (k, v) in &counts {
        message.push_str(&format!("\n{k:<width$}  {v}"));
    }
    if counts.is_empty() {
        message = "No block references selected.".into();
    }
    Ok(json!({ "counts": counts, "total": total, "message": message }))
}

/// BCOUNT typed: "Select objects:", Enter alone counts the whole space.
#[derive(Default)]
struct BcountM {
    sel: SelectPhase,
}

impl BcountM {
    fn finish(s: &mut Session, hs: &[Handle]) -> Result<Step> {
        let r = run_bcount(s, &json!({ "handles": hexes(hs) }))?;
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            s.echo(m.to_string());
        }
        Ok(Step::Done)
    }
}

impl Interactive for BcountM {
    fn name(&self) -> &'static str {
        "BCOUNT"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            let hs = self.sel.picked.clone();
            return Self::finish(s, &hs);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Select objects <All>", Accept::SELECT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Done(hs) => Self::finish(s, &hs),
            SelOutcome::Empty => Self::finish(s, &[]),
        }
    }
}

// ---------- GATTE ----------

/// Every block reference of the drawing (model space and layouts).
fn all_entities(d: &Drawing) -> impl Iterator<Item = &std::sync::Arc<Entity>> {
    d.model.iter().chain(d.layouts.iter().flat_map(|l| l.entities.iter())).take(MAX_SCAN)
}

fn run_gatte(s: &mut Session, p: &Value) -> Result<Value> {
    let block = str_param(p, "block").filter(|b| !b.trim().is_empty()).ok_or_else(|| bad("gatte", "`block` is required"))?.trim().to_string();
    let tag = str_param(p, "tag").filter(|t| !t.trim().is_empty()).ok_or_else(|| bad("gatte", "`tag` is required"))?.trim().to_string();
    let value = str_param(p, "value").ok_or_else(|| bad("gatte", "`value` (text) is required"))?.to_string();
    let only: Option<Vec<Handle>> = match (p.get("handles"), p.get("handle")) {
        (None | Some(Value::Null), None | Some(Value::Null)) => None,
        _ => Some(targets(s, p)?),
    };
    let d = s.doc()?;
    if d.block(&block).is_none() {
        return Err(other(format!("No block named \"{block}\".")));
    }
    let refs: Vec<Handle> = match &only {
        Some(hs) => hs.clone(),
        None => all_entities(d).map(|e| e.handle).collect(),
    };
    let mut hits = Vec::new();
    let mut has_tag = false;
    for h in refs {
        let Some(e) = d.entity(h) else { continue };
        let EntityKind::Insert(i) = &e.kind else { continue };
        if !i.block.eq_ignore_ascii_case(&block) {
            continue;
        }
        if let Some(a) = i.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case(&tag)) {
            has_tag = true;
            if a.text.value != value && !super::curves::is_locked(s, h) {
                hits.push(h);
            }
        }
    }
    if !has_tag {
        return Err(other(format!("No reference of \"{block}\" has an attribute tagged \"{tag}\".")));
    }
    let d = s.doc_mut()?;
    for h in &hits {
        let _ = d.modify_entity(*h, |e| {
            if let EntityKind::Insert(i) = &mut e.kind {
                for a in i.attribs.iter_mut().filter(|a| a.tag.eq_ignore_ascii_case(&tag)) {
                    a.text.value = value.clone();
                }
            }
        });
    }
    Ok(json!({ "changed": hits.len(), "handles": hexes(&hits), "message": format!("Number of inserts changed: {}", hits.len()) }))
}

#[derive(Default)]
struct GatteM {
    block: Option<String>,
    tags: Vec<String>,
    tag: Option<String>,
}

impl Interactive for GatteM {
    fn name(&self) -> &'static str {
        "GATTE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (&self.block, &self.tag) {
            (None, _) => Prompt::new("Select block or attribute", Accept::SELECT).kw(&["Block name"]),
            (Some(_), None) => Prompt::new(format!("Known tag names for block: {}\nEnter attribute tag name", self.tags.join(" ")), Accept::TEXT),
            (Some(_), Some(_)) => Prompt::new("Enter new text", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (&self.block, &self.tag, i) {
            (None, _, Input::Pick(hs)) => {
                let d = s.doc()?;
                let ins = hs.iter().find_map(|h| match &d.entity(*h)?.kind {
                    EntityKind::Insert(i) => Some(i.clone()),
                    _ => None,
                });
                let ins = ins.ok_or_else(|| other("Select a block reference."))?;
                self.take(s, &ins.block)
            }
            (None, _, Input::Point(p)) => {
                let h = super::curves::pick_at(s, p).ok_or_else(|| other("*Invalid selection*"))?;
                match &super::curves::entity(s, h)?.kind {
                    EntityKind::Insert(i) => {
                        let b = i.block.clone();
                        self.take(s, &b)
                    }
                    _ => Err(other("Select a block reference.")),
                }
            }
            (None, _, Input::Text(t)) | (None, _, Input::Keyword(t)) if !t.eq_ignore_ascii_case("block name") && !t.eq_ignore_ascii_case("b") => {
                let t = t.trim().to_string();
                self.take(s, &t)
            }
            (None, _, Input::Keyword(_)) | (None, _, Input::Text(_)) => {
                s.echo("Type the block's name.");
                Ok(Step::Continue)
            }
            (Some(_), None, Input::Text(t)) => {
                let t = t.trim().to_string();
                if !self.tags.iter().any(|k| k.eq_ignore_ascii_case(&t)) {
                    return Err(other(format!("Tag \"{t}\" not found.")));
                }
                self.tag = Some(t);
                Ok(Step::Continue)
            }
            (Some(b), Some(t), Input::Text(v)) => {
                let r = run_gatte(s, &json!({ "block": b, "tag": t, "value": v }))?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
                Ok(Step::Done)
            }
            (_, _, Input::Enter) => Ok(Step::Cancel),
            _ => Ok(Step::Continue),
        }
    }
}

impl GatteM {
    fn take(&mut self, s: &mut Session, name: &str) -> Result<Step> {
        let d = s.doc()?;
        let b = d.block(name).ok_or_else(|| other(format!("No block named \"{name}\".")))?;
        let tags: Vec<String> = b
            .entities
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::AttDef(a) if !a.constant => Some(a.tag.clone()),
                _ => None,
            })
            .collect();
        if tags.is_empty() {
            return Err(other("The block has no attributes."));
        }
        let n = all_entities(d).filter(|e| matches!(&e.kind, EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(name))).count();
        self.block = Some(b.name.clone());
        self.tags = tags;
        s.echo(format!("Block {name}: number of inserts in drawing = {n}"));
        Ok(Step::Continue)
    }
}

// ---------- GETSEL / SSX ----------

fn select_found(s: &mut Session, found: Vec<Handle>) -> Result<Value> {
    let n = found.len();
    let out = hexes(&found);
    s.set_selection(found);
    Ok(json!({ "selected": n, "handles": out, "message": format!("{n} object(s) selected.") }))
}

fn run_getsel(s: &mut Session, p: &Value) -> Result<Value> {
    let layer = str_param(p, "layer").map(str::trim).filter(|l| !l.is_empty()).unwrap_or("*").to_string();
    let kind = str_param(p, "type").map(str::trim).filter(|l| !l.is_empty()).unwrap_or("*").to_string();
    let d = s.doc()?;
    let found: Vec<Handle> = d
        .space(&s.space())
        .map(|st| {
            st.iter()
                .take(MAX_SCAN)
                .filter(|e| d.is_visible(e))
                .filter(|e| super::qselect::wildcard(&layer, &e.common.layer))
                .filter(|e| super::qselect::wildcard(&kind, e.kind.dxf_name()) || super::qselect::wildcard(&kind, e.kind.type_name()))
                .map(|e| e.handle)
                .collect()
        })
        .unwrap_or_default();
    select_found(s, found)
}

#[derive(Default)]
struct GetselM {
    layer: Option<String>,
}

impl Interactive for GetselM {
    fn name(&self) -> &'static str {
        "GETSEL"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.layer {
            None => Prompt::new("Select an object on the layer to select, or type its name", Accept::SELECT).default("*"),
            Some(_) => Prompt::new("Select an object of the type to select, or type it", Accept::SELECT).default("*"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let answer = match i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            Input::Enter => "*".into(),
            Input::Pick(hs) => {
                let d = s.doc()?;
                let Some(e) = hs.first().and_then(|h| d.entity(*h)) else { return Ok(Step::Continue) };
                if self.layer.is_none() { e.common.layer.clone() } else { e.kind.dxf_name().to_string() }
            }
            Input::Point(p) => {
                let Some(h) = super::curves::pick_at(s, p) else { return Ok(Step::Continue) };
                let e = super::curves::entity(s, h)?;
                if self.layer.is_none() { e.common.layer } else { e.kind.dxf_name().to_string() }
            }
            _ => return Ok(Step::Continue),
        };
        match self.layer.take() {
            None => {
                self.layer = Some(answer);
                Ok(Step::Continue)
            }
            Some(layer) => {
                let r = run_getsel(s, &json!({ "layer": layer, "type": answer }))?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
                Ok(Step::Done)
            }
        }
    }
}

fn run_ssx(s: &mut Session, p: &Value) -> Result<Value> {
    let r = targets(s, p)?.first().copied().ok_or_else(|| bad("ssx", "`handle` (the object to match) is required"))?;
    let keys: Vec<String> = match p.get("match") {
        None | Some(Value::Null) => ["type", "layer", "color", "linetype"].map(String::from).to_vec(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|k| k.trim().to_ascii_lowercase()).collect(),
        Some(_) => return Err(bad("ssx", "`match` is a list of type, layer, color, linetype")),
    };
    if let Some(k) = keys.iter().find(|k| !matches!(k.as_str(), "type" | "layer" | "color" | "linetype")) {
        return Err(bad("ssx", format!("`match`: \"{k}\" is not type, layer, color or linetype")));
    }
    let has = |k: &str| keys.iter().any(|x| x == k);
    let d = s.doc()?;
    let reference = d.entity(r).ok_or_else(|| other("No such object."))?.clone();
    let (rc, rk) = (&reference.common, &reference.kind);
    let found: Vec<Handle> = d
        .space(&s.space())
        .map(|st| {
            st.iter()
                .take(MAX_SCAN)
                .filter(|e| d.is_visible(e))
                .filter(|e| !has("type") || e.kind.dxf_name().eq_ignore_ascii_case(rk.dxf_name()))
                .filter(|e| !has("layer") || e.common.layer.eq_ignore_ascii_case(&rc.layer))
                .filter(|e| !has("color") || e.common.color == rc.color)
                .filter(|e| !has("linetype") || e.common.linetype.eq_ignore_ascii_case(&rc.linetype))
                .map(|e| e.handle)
                .collect()
        })
        .unwrap_or_default();
    select_found(s, found)
}

// ---------- XDLIST / XLIST / SHOWURLS ----------

fn run_xdlist(s: &mut Session, p: &Value) -> Result<Value> {
    let h = targets(s, p)?.first().copied().ok_or_else(|| bad("xdlist", "`handle` is required"))?;
    let want = str_param(p, "app").map(str::trim).filter(|a| !a.is_empty() && *a != "*");
    let e = super::curves::entity(s, h)?;
    let mut apps: Vec<(String, Vec<Value>)> = Vec::new();
    for t in &e.common.xdata {
        if t.code == 1001 {
            apps.push((t.value.clone(), Vec::new()));
        } else if let Some((_, tags)) = apps.last_mut() {
            tags.push(json!({ "code": t.code, "value": t.value }));
        }
    }
    if let Some(w) = want {
        apps.retain(|(a, _)| super::qselect::wildcard(w, a));
    }
    let mut message = String::new();
    for (a, tags) in &apps {
        message.push_str(&format!("\n* Registered Application Name: {a}"));
        for t in tags {
            message.push_str(&format!("\n* Code {}: {}", t["code"], t["value"].as_str().unwrap_or("")));
        }
    }
    if apps.is_empty() {
        message = "Object has no extended data.".into();
    }
    let apps: Vec<Value> = apps.into_iter().map(|(app, tags)| json!({ "app": app, "tags": tags })).collect();
    Ok(json!({ "apps": apps, "message": message.trim_start() }))
}

/// The objects nested in a block reference in world coordinates, with the name of the block
/// each sits in.
fn nested_objects(d: &Drawing, ins: &Insert, m: &Mat3, depth: usize, out: &mut Vec<(String, Common, EntityKind)>) {
    if depth >= cadaddy_doc::MAX_BLOCK_DEPTH || out.len() >= MAX_SCAN {
        return;
    }
    let Some(b) = d.block(&ins.block) else { return };
    let mm = m.then_before(ins.transform(b.base.xy()));
    for e in b.entities.iter() {
        match &e.kind {
            EntityKind::Insert(inner) => nested_objects(d, inner, &mm, depth + 1, out),
            EntityKind::AttDef(_) => {}
            k => {
                let mut k = k.clone();
                k.transform(&mm);
                out.push((b.name.clone(), e.common.clone(), k));
            }
        }
    }
}

fn xlist_json(s: &Session, h: Handle, at: Option<Vec2>) -> Result<Value> {
    let d = s.doc()?;
    let e = d.entity(h).ok_or_else(|| other("No such object."))?;
    let EntityKind::Insert(ins) = &e.kind else { return Err(other("Select a block or xref reference.")) };
    let mut all = Vec::new();
    nested_objects(d, ins, &Mat3::IDENTITY, 0, &mut all);
    let pick = match at {
        Some(p) => all.into_iter().min_by(|a, b| super::curves::kind_distance(&a.2, p).total_cmp(&super::curves::kind_distance(&b.2, p))),
        None => all.into_iter().next(),
    };
    let (block, c, k) = pick.ok_or_else(|| other("The reference has no nested objects."))?;
    let color = c.color.name();
    let message = format!("Object: {}\nBlock name: {block}\nLayer: {}\nColor: {color}\nLinetype: {}", k.type_name(), c.layer, c.linetype);
    Ok(json!({ "type": k.dxf_name(), "block": block, "layer": c.layer, "color": color, "linetype": c.linetype, "message": message }))
}

fn run_xlist(s: &mut Session, p: &Value) -> Result<Value> {
    let h = targets(s, p)?.first().copied().ok_or_else(|| bad("xlist", "`handle` (a block reference) is required"))?;
    xlist_json(s, h, point_param(p, "at"))
}

struct XlistM;

impl Interactive for XlistM {
    fn name(&self) -> &'static str {
        "XLIST"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Select nested xref or block object to list", Accept::POINT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Point(p) => {
                let h = super::curves::pick_at(s, p).ok_or_else(|| other("*Invalid selection*"))?;
                let r = xlist_json(s, h, Some(p))?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Cancel),
            _ => Ok(Step::Continue),
        }
    }
}

fn run_showurls(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let links: Vec<Value> = all_entities(d)
        .filter_map(|e| {
            let l = cadaddy_doc::hyperlink::read(&e.common.xdata)?;
            Some(json!({ "handle": e.handle.hex(), "url": l.url, "target": l.target, "description": l.description }))
        })
        .collect();
    let mut message = if links.is_empty() { "No hyperlinks in the drawing.".to_string() } else { format!("{} hyperlink(s):", links.len()) };
    for l in &links {
        message.push_str(&format!(
            "\n{}  {}{}",
            l["handle"].as_str().unwrap_or(""),
            l["url"].as_str().unwrap_or(""),
            l["target"].as_str().filter(|t| !t.is_empty()).map(|t| format!("#{t}")).unwrap_or_default()
        ));
    }
    Ok(json!({ "links": links, "message": message }))
}

// ---------- REDIR / REDIRMODE ----------

const REDIR_KINDS: [&str; 4] = ["styles", "xrefs", "images", "rtext"];

/// `path` with its folder `from` (or one inside it; any folder for `*`) replaced by `to`. Paths
/// without a folder (a font found on the support path) are left alone.
fn redirect(path: &str, from: &str, to: &str) -> Option<String> {
    let norm = |t: &str| t.replace('\\', "/");
    let p = norm(path);
    let slash = p.rfind('/')?;
    let (dir, file) = (&p[..slash], &p[slash + 1..]);
    let to = norm(to.trim()).trim_end_matches('/').to_string();
    let from = norm(from.trim()).trim_end_matches('/').to_string();
    let rest = if from == "*" || dir.eq_ignore_ascii_case(&from) {
        file.to_string()
    } else {
        // A folder inside `from` keeps the part below it.
        let head = dir.get(..from.len()).filter(|h| h.eq_ignore_ascii_case(&from))?;
        let below = dir.get(head.len()..).filter(|b| b.starts_with('/'))?;
        format!("{}/{file}", below.trim_start_matches('/'))
    };
    let out = if to.is_empty() { rest } else { format!("{to}/{rest}") };
    // Keep the separator the path used.
    let out = if path.contains('\\') && !path.contains('/') { out.replace('/', "\\") } else { out };
    (out != path).then_some(out)
}

fn kinds_param(cmd: &str, p: &Value) -> Result<Option<Vec<String>>> {
    match p.get("kinds") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => {
            let mut out = Vec::new();
            for v in a {
                let k = v.as_str().map(|k| k.trim().to_ascii_lowercase()).unwrap_or_default();
                let k = REDIR_KINDS
                    .iter()
                    .find(|x| **x == k || x.trim_end_matches('s') == k)
                    .ok_or_else(|| bad(cmd, format!("`kinds`: {v} is not styles, xrefs, images or rtext")))?;
                if !out.iter().any(|o| o == k) {
                    out.push(k.to_string());
                }
            }
            Ok(Some(out))
        }
        Some(Value::String(t)) => kinds_param(cmd, &json!({ "kinds": t.split([',', ' ']).filter(|x| !x.is_empty()).collect::<Vec<_>>() })),
        Some(_) => Err(bad(cmd, "`kinds` is a list of styles, xrefs, images, rtext")),
    }
}

fn run_redir(s: &mut Session, p: &Value) -> Result<Value> {
    let from = str_param(p, "from").filter(|f| !f.trim().is_empty()).ok_or_else(|| bad("redir", "`from` (a folder, or *) is required"))?.to_string();
    let to = str_param(p, "to").ok_or_else(|| bad("redir", "`to` (the new folder) is required"))?.to_string();
    let kinds = kinds_param("redir", p)?.unwrap_or_else(|| s.settings.redirmode.clone());
    let on = |k: &str| kinds.iter().any(|x| x == k);
    let mut changed: Vec<Value> = Vec::new();
    let mut note = |kind: &str, name: &str, old: &str, new: &str| changed.push(json!({ "kind": kind, "name": name, "from": old, "to": new }));
    let d = s.doc_mut()?;
    if on("styles") {
        for st in d.text_styles.iter_mut() {
            for f in [&mut st.font, &mut st.big_font] {
                if let Some(n) = redirect(f, &from, &to) {
                    note("styles", &st.name.clone(), f, &n);
                    *f = n;
                }
            }
        }
    }
    if on("xrefs") {
        for b in d.blocks.values_mut() {
            let Some(old) = b.xref_path.clone() else { continue };
            if let Some(n) = redirect(&old, &from, &to) {
                note("xrefs", &b.name, &old, &n);
                std::sync::Arc::make_mut(b).xref_path = Some(n);
            }
        }
    }
    let handles: Vec<Handle> = all_entities(d).map(|e| e.handle).collect();
    for h in handles {
        let Some(e) = d.entity(h) else { continue };
        let update: Option<(String, String, String)> = match &e.kind {
            EntityKind::Image(i) if on("images") => redirect(&i.path, &from, &to).map(|n| ("images".into(), i.path.clone(), n)),
            EntityKind::Underlay(u) if on("images") => redirect(&u.path, &from, &to).map(|n| ("images".into(), u.path.clone(), n)),
            EntityKind::MText(_) if on("rtext") => match express::rtext::source_of(&e.common) {
                Some(express::rtext::Source::File(f)) => redirect(&f, &from, &to).map(|n| ("rtext".into(), f, n)),
                _ => None,
            },
            _ => None,
        };
        let Some((kind, old, new)) = update else { continue };
        note(&kind, &h.hex(), &old, &new);
        let _ = d.modify_entity(h, |e| match &mut e.kind {
            EntityKind::Image(i) => i.path = new.clone(),
            EntityKind::Underlay(u) => u.path = new.clone(),
            _ => express::rtext::set_source(&mut e.common, &express::rtext::Source::File(new.clone())),
        });
    }
    let message = format!("{} path(s) redirected.", changed.len());
    Ok(json!({ "changed": changed, "message": message }))
}

fn run_redirmode(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(k) = kinds_param("redirmode", p)? {
        s.settings.redirmode = k;
    }
    let k = s.settings.redirmode.clone();
    Ok(json!({ "kinds": k, "message": format!("REDIR changes paths in: {}", if k.is_empty() { "nothing".into() } else { k.join(", ") }) }))
}

#[derive(Default)]
struct RedirM {
    from: Option<String>,
}

impl Interactive for RedirM {
    fn name(&self) -> &'static str {
        "REDIR"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        s.echo(format!("Current REDIRMODE: {}", s.settings.redirmode.join(",")));
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.from {
            None => Prompt::new("Find directory (use * for all)", Accept::TEXT),
            Some(_) => Prompt::new("Replace with", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let t = match i {
            Input::Text(t) | Input::Keyword(t) => t,
            Input::Enter if self.from.is_some() => String::new(),
            Input::Enter => return Ok(Step::Cancel),
            _ => return Ok(Step::Continue),
        };
        match self.from.take() {
            None => {
                self.from = Some(t.trim().to_string());
                Ok(Step::Continue)
            }
            Some(from) => {
                let r = run_redir(s, &json!({ "from": from, "to": t.trim() }))?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
                Ok(Step::Done)
            }
        }
    }
}

struct RedirmodeM;

impl Interactive for RedirmodeM {
    fn name(&self) -> &'static str {
        "REDIRMODE"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        Prompt::new("Replace directories in Styles, Xrefs, Images, Rtext", Accept::TEXT).default(s.settings.redirmode.join(","))
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Text(t) | Input::Keyword(t) => {
                let r = run_redirmode(s, &json!({ "kinds": t }))?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}

// ---------- LSP / LSPSURF ----------

fn run_lsp(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = str_param(p, "kind").map(|k| k.trim().to_ascii_lowercase()).unwrap_or_else(|| "all".into());
    if !matches!(kind.as_str(), "all" | "commands" | "functions" | "variables") {
        return Err(bad("lsp", "`kind` is all, commands, functions or variables"));
    }
    let pattern = str_param(p, "pattern").map(str::trim).filter(|t| !t.is_empty()).unwrap_or("*").to_string();
    let (mut commands, mut functions, mut variables) = (Vec::new(), Vec::new(), Vec::new());
    for (name, v) in s.lisp_user_symbols() {
        match &v {
            LispValue::Func(_) if name.starts_with("C:") => {
                if super::qselect::wildcard(&pattern, &name[2..]) || super::qselect::wildcard(&pattern, &name) {
                    commands.push(json!(name[2..].to_string()));
                }
            }
            LispValue::Func(_) => {
                if super::qselect::wildcard(&pattern, &name) {
                    functions.push(json!(name));
                }
            }
            _ => {
                if super::qselect::wildcard(&pattern, &name) {
                    let text: String = crate::lisp::value::to_text(&v, true).chars().take(200).collect();
                    variables.push(json!({ "name": name, "value": text }));
                }
            }
        }
    }
    let all = kind == "all";
    let mut message = String::new();
    if all || kind == "commands" {
        message.push_str(&format!("Commands: {}\n", commands.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")));
    }
    if all || kind == "functions" {
        message.push_str(&format!("Functions: {}\n", functions.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")));
    }
    if all || kind == "variables" {
        message.push_str("Variables:");
        for v in &variables {
            message.push_str(&format!("\n  {} = {}", v["name"].as_str().unwrap_or(""), v["value"].as_str().unwrap_or("")));
        }
    }
    let mut out = serde_json::Map::new();
    if all || kind == "commands" {
        out.insert("commands".into(), json!(commands));
    }
    if all || kind == "functions" {
        out.insert("functions".into(), json!(functions));
    }
    if all || kind == "variables" {
        out.insert("variables".into(), json!(variables));
    }
    out.insert("message".into(), json!(message.trim_end()));
    Ok(Value::Object(out))
}

struct LspM;

impl Interactive for LspM {
    fn name(&self) -> &'static str {
        "LSP"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("LISP symbols to list", Accept::TEXT).kw(&["Commands", "Functions", "Variables", "All"]).default("All")
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let kind = match i {
            Input::Keyword(k) | Input::Text(k) => match k.trim().to_ascii_lowercase().chars().next() {
                Some('c') => "commands",
                Some('f') => "functions",
                Some('v') => "variables",
                _ => "all",
            },
            Input::Enter => "all",
            _ => return Ok(Step::Continue),
        };
        let r = run_lsp(s, &json!({ "kind": kind }))?;
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            s.echo(m.to_string());
        }
        Ok(Step::Done)
    }
}

/// Most bytes of a file LSPSURF reads.
const MAX_LSP_BYTES: u64 = 16 << 20;

#[cfg(not(target_arch = "wasm32"))]
fn lisp_file(path: &str) -> Result<String> {
    use std::io::Read;
    let f = crate::readfile::open(path).map_err(|e| other(format!("{path}: {e}")))?;
    let mut bytes = Vec::new();
    f.take(MAX_LSP_BYTES).read_to_end(&mut bytes).map_err(|e| other(format!("{path}: {e}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(target_arch = "wasm32")]
fn lisp_file(path: &str) -> Result<String> {
    let _ = MAX_LSP_BYTES;
    Err(other(format!("Files are not available in the web version: {path}")))
}

/// The list's elements (up to a cap: a hostile file can nest deep or long).
fn list_items(v: &LispValue) -> Vec<LispValue> {
    let mut out = Vec::new();
    let mut cur = v.clone();
    while let LispValue::Cons(c) = cur {
        if out.len() >= 100_000 {
            break;
        }
        out.push(c.car.clone());
        cur = c.cdr.clone();
    }
    out
}

/// `defun` forms anywhere in `v` (a `defun` inside a `progn` or another function counts).
fn find_defuns(v: &LispValue, depth: usize, out: &mut Vec<(String, Vec<String>)>) {
    if depth > 64 || out.len() >= 100_000 {
        return;
    }
    let items = list_items(v);
    if let (Some(LispValue::Sym(head)), Some(LispValue::Sym(name))) = (items.first(), items.get(1))
        && matches!(head.name(), "DEFUN" | "DEFUN-Q")
    {
        let params = items
            .get(2)
            .map(list_items)
            .unwrap_or_default()
            .iter()
            .filter_map(|p| match p {
                LispValue::Sym(s) => Some(s.name().to_string()),
                _ => None,
            })
            .take_while(|p| p != "/")
            .collect();
        out.push((name.name().to_string(), params));
    }
    for it in items {
        if matches!(it, LispValue::Cons(_)) {
            find_defuns(&it, depth + 1, out);
        }
    }
}

/// The line each function's `(defun NAME` starts on, from the source text.
fn defun_line(src: &str, name: &str) -> Option<usize> {
    let lower = name.to_ascii_lowercase();
    src.lines().position(|l| {
        let l = l.to_ascii_lowercase();
        l.find("(defun")
            .is_some_and(|i| l[i..].trim_start_matches("(defun-q").trim_start_matches("(defun").split_whitespace().next() == Some(lower.as_str()))
    })
}

fn run_lspsurf(_s: &mut Session, p: &Value) -> Result<Value> {
    let src = match (str_param(p, "text"), str_param(p, "path")) {
        (Some(t), _) => t.to_string(),
        (None, Some(path)) => lisp_file(path)?,
        (None, None) => return Err(bad("lspsurf", "`path` (a .lsp file) or `text` is required")),
    };
    let pattern = str_param(p, "pattern").map(str::trim).filter(|t| !t.is_empty()).unwrap_or("*").to_string();
    let forms = crate::lisp::read::read_all(&src).map_err(|e| other(format!("The file could not be read as LISP: {}", e.msg)))?;
    let mut found = Vec::new();
    for f in &forms {
        find_defuns(f, 0, &mut found);
    }
    let functions: Vec<Value> = found
        .into_iter()
        .filter(|(n, _)| super::qselect::wildcard(&pattern, n))
        .map(|(n, params)| json!({ "name": n, "params": params, "line": defun_line(&src, &n).map(|l| l + 1) }))
        .collect();
    let mut message = format!("{} function(s):", functions.len());
    for f in &functions {
        let params: Vec<&str> = f["params"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        message.push_str(&format!("\n  ({} {})", f["name"].as_str().unwrap_or(""), params.join(" ")));
    }
    Ok(json!({ "functions": functions, "message": message }))
}

struct LspsurfM;

impl Interactive for LspsurfM {
    fn name(&self) -> &'static str {
        "LSPSURF"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("LISP file to look through", Accept::TEXT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Text(t) | Input::Keyword(t) => {
                let r = run_lspsurf(s, &json!({ "path": t.trim().trim_matches('"') }))?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Cancel),
            _ => Ok(Step::Continue),
        }
    }
}

// ---------- REVERT / TFRAMES ----------

fn run_revert(s: &mut Session, _p: &Value) -> Result<Value> {
    let st = s.state()?;
    if st.path.is_none() {
        return Err(other("The drawing has never been saved: there is nothing to revert to."));
    }
    if !st.is_dirty() {
        return Ok(json!({ "reverted": false, "message": "No changes to discard." }));
    }
    let saved = st.saved.clone();
    s.state_mut()?.doc = saved;
    s.set_selection(Vec::new());
    Ok(json!({ "reverted": true, "message": "Changes since the last save discarded." }))
}

fn frame_on(s: &Session, name: &str) -> bool {
    crate::sysvars::get(s, name).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))).is_some_and(|v| v != 0)
}

fn run_tframes(s: &mut Session, p: &Value) -> Result<Value> {
    let on = match p.get("on") {
        None | Some(Value::Null) => !(frame_on(s, "IMAGEFRAME") || frame_on(s, "WIPEOUTFRAME")),
        Some(v) => v.as_bool().ok_or_else(|| bad("tframes", "`on` is true or false"))?,
    };
    let v = json!(i32::from(on));
    crate::sysvars::set(s, "IMAGEFRAME", &v)?;
    crate::sysvars::set(s, "WIPEOUTFRAME", &v)?;
    Ok(
        json!({ "imageframe": i32::from(on), "wipeoutframe": i32::from(on), "message": format!("Image and wipeout frames {}.", if on { "on" } else { "off" }) }),
    )
}

// ---------- BSCALE / BROTATE ----------

#[derive(Clone, Copy, PartialEq)]
enum Xform {
    Scale,
    Rotate,
}

impl Xform {
    fn id(self) -> &'static str {
        match self {
            Xform::Scale => "bscale",
            Xform::Rotate => "brotate",
        }
    }
}

/// Set block references' scale or rotation about their insertion points: their attributes move
/// with them.
fn block_xform(s: &mut Session, p: &Value, x: Xform) -> Result<Value> {
    let cmd = x.id();
    let amount = match x {
        Xform::Scale => size_param(cmd, p, "factor", false)?.ok_or_else(|| bad(cmd, "`factor` (a positive number) is required"))?,
        Xform::Rotate => f64_req(cmd, p, "angle")?.to_radians(),
    };
    let relative = bool_or(p, "relative", false);
    let hs = targets(s, p)?;
    let mut changed = Vec::new();
    for h in hs.into_iter().take(MAX_SCAN) {
        if super::curves::is_locked(s, h) {
            continue;
        }
        let d = s.doc()?;
        let Some(e) = d.entity(h) else { continue };
        let EntityKind::Insert(old) = &e.kind else { continue };
        let base = d.block(&old.block).map(|b| b.base.xy()).unwrap_or(Vec2::ZERO);
        let mut new = old.clone();
        match x {
            Xform::Scale => {
                let f = |c: f64| {
                    let sign = if c < 0.0 { -1.0 } else { 1.0 };
                    if relative { c * amount } else { sign * amount }
                };
                new.scale = Vec3::new(f(old.scale.x), f(old.scale.y), f(old.scale.z));
                if !new.scale.is_finite() || new.scale.x == 0.0 || new.scale.y == 0.0 {
                    continue;
                }
            }
            Xform::Rotate => new.rotation = if relative { old.rotation + amount } else { amount },
        }
        let Some(inv) = old.transform(base).inverse() else { continue };
        let delta = new.transform(base).then_before(inv);
        for a in new.attribs.iter_mut() {
            let mut k = EntityKind::Text(a.text.clone());
            k.transform(&delta);
            if let EntityKind::Text(t) = k {
                a.text = t;
            }
        }
        let _ = s.doc_mut()?.modify_entity(h, |e| e.kind = EntityKind::Insert(new));
        changed.push(h);
    }
    if changed.is_empty() {
        return Err(other("No block references selected."));
    }
    Ok(json!({ "handles": hexes(&changed), "message": format!("{} block reference(s) changed.", changed.len()) }))
}

fn run_bscale(s: &mut Session, p: &Value) -> Result<Value> {
    block_xform(s, p, Xform::Scale)
}

fn run_brotate(s: &mut Session, p: &Value) -> Result<Value> {
    block_xform(s, p, Xform::Rotate)
}

struct BlockXformM {
    x: Xform,
    sel: SelectPhase,
    objs: Vec<Handle>,
    relative: bool,
}

impl BlockXformM {
    fn new(x: Xform) -> Self {
        BlockXformM { x, sel: SelectPhase::default(), objs: Vec::new(), relative: false }
    }
}

impl Interactive for BlockXformM {
    fn name(&self) -> &'static str {
        match self.x {
            Xform::Scale => "BSCALE",
            Xform::Rotate => "BROTATE",
        }
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.objs = self.sel.picked.clone();
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.objs.is_empty() {
            return self.sel.prompt();
        }
        let msg = match (self.x, self.relative) {
            (Xform::Scale, false) => "Specify scale factor or",
            (Xform::Scale, true) => "Specify relative scale factor or",
            (Xform::Rotate, false) => "Specify rotation angle or",
            (Xform::Rotate, true) => "Specify relative rotation angle or",
        };
        Prompt::new(msg, Accept::NUMBER).kw(&[if self.relative { "Absolute" } else { "Relative" }])
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.objs.is_empty() {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Done(hs) => {
                    self.objs = hs;
                    Ok(Step::Continue)
                }
                SelOutcome::Empty => Ok(Step::Cancel),
            };
        }
        match i {
            Input::Keyword(k) | Input::Text(k) if k.trim().eq_ignore_ascii_case("relative") || k.trim().eq_ignore_ascii_case("r") => {
                self.relative = true;
                Ok(Step::Continue)
            }
            Input::Keyword(k) | Input::Text(k) if k.trim().eq_ignore_ascii_case("absolute") || k.trim().eq_ignore_ascii_case("a") => {
                self.relative = false;
                Ok(Step::Continue)
            }
            Input::Text(t) => {
                let v: f64 = t.trim().parse().map_err(|_| other("Requires a number."))?;
                let key = if self.x == Xform::Scale { "factor" } else { "angle" };
                let r = block_xform(s, &json!({ "handles": hexes(&self.objs), key: v, "relative": self.relative }), self.x)?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Cancel),
            _ => Ok(Step::Continue),
        }
    }
}

// ---------- VPSYNC ----------

fn run_vpsync(s: &mut Session, p: &Value) -> Result<Value> {
    let master = super::curves::handle_param(p, "master").ok_or_else(|| bad("vpsync", "`master` (a viewport) is required"))?;
    let hs = targets(s, p)?;
    let d = s.doc()?;
    let viewport = |h: Handle| match d.entity(h).map(|e| &e.kind) {
        Some(EntityKind::Viewport(v)) => Some(v.clone()),
        _ => None,
    };
    let m = viewport(master).ok_or_else(|| other("The master must be a layout viewport."))?;
    if !(m.view_height > 0.0 && m.view_height.is_finite() && m.height > 0.0) {
        return Err(other("The master viewport has no view."));
    }
    let space = d.space_of(master);
    // Paper units per model unit, and the master's twist.
    let scale = m.height / m.view_height;
    let turn = Mat3::rotate(m.twist);
    let mut changed = Vec::new();
    for h in hs.into_iter().filter(|h| *h != master) {
        let Some(v) = viewport(h) else { continue };
        if d.space_of(h) != space || v.locked {
            continue;
        }
        // The model point under the slave's centre, seen through the master.
        let along = (v.center.xy() - m.center.xy()) * (1.0 / scale);
        let view_center = m.view_center + turn.apply_vec(along);
        let view_height = v.height / scale;
        if view_center.is_finite() && view_height.is_finite() && view_height > 0.0 {
            changed.push((h, view_center, view_height));
        }
    }
    if changed.is_empty() {
        return Err(other("No other viewports of the master's layout selected (locked ones are left alone)."));
    }
    let twist = m.twist;
    let d = s.doc_mut()?;
    for (h, c, vh) in &changed {
        let _ = d.modify_entity(*h, |e| {
            if let EntityKind::Viewport(v) = &mut e.kind {
                v.view_center = *c;
                v.view_height = *vh;
                v.twist = twist;
            }
        });
    }
    let hs: Vec<Handle> = changed.iter().map(|c| c.0).collect();
    Ok(json!({ "handles": hexes(&hs), "message": format!("{} viewport(s) synchronized.", hs.len()) }))
}

#[derive(Default)]
struct VpsyncM {
    master: Option<Handle>,
    sel: SelectPhase,
}

impl Interactive for VpsyncM {
    fn name(&self) -> &'static str {
        "VPSYNC"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.master {
            None => Prompt::new("Select master viewport", Accept::SELECT),
            Some(_) => Prompt::new("Select viewports to be aligned to master viewport", Accept::SELECT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let Some(master) = self.master else {
            let picked = match i {
                Input::Pick(hs) => hs.first().copied(),
                Input::Point(p) => super::curves::pick_at(s, p),
                Input::Enter => return Ok(Step::Cancel),
                _ => None,
            };
            if let Some(h) = picked {
                if !matches!(s.doc()?.entity(h).map(|e| &e.kind), Some(EntityKind::Viewport(_))) {
                    return Err(other("Select a layout viewport."));
                }
                self.master = Some(h);
            }
            return Ok(Step::Continue);
        };
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Done(hs) => {
                let r = run_vpsync(s, &json!({ "master": master.hex(), "handles": hexes(&hs) }))?;
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    s.echo(m.to_string());
                }
                Ok(Step::Done)
            }
            SelOutcome::Empty => Ok(Step::Cancel),
        }
    }
}

// ---------- DIMREASSOC ----------

fn run_dimreassoc(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let d = s.doc()?;
    let hits: Vec<Handle> = hs
        .into_iter()
        .take(MAX_SCAN)
        .filter(|h| matches!(d.entity(*h).map(|e| &e.kind), Some(EntityKind::Dimension(dm)) if !dm.text.is_empty() && dm.text != "<>"))
        .filter(|h| !super::curves::is_locked(s, *h))
        .collect();
    let d = s.doc_mut()?;
    for h in &hits {
        let _ = d.modify_entity(*h, |e| {
            if let EntityKind::Dimension(dm) = &mut e.kind {
                dm.text.clear();
            }
        });
    }
    Ok(json!({ "handles": hexes(&hits), "message": format!("{} dimension(s) restored.", hits.len()) }))
}
