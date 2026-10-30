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
use cadaddy_geom::{Line, Mat3, Vec2, Vec3};
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase, SelectRun};
use super::modify::Edges;
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
                if self.layer.is_none() { e.common.layer.to_string() } else { e.kind.dxf_name().to_string() }
            }
            Input::Point(p) => {
                let Some(h) = super::curves::pick_at(s, p) else { return Ok(Step::Continue) };
                let e = super::curves::entity(s, h)?;
                if self.layer.is_none() { e.common.layer.to_string() } else { e.kind.dxf_name().to_string() }
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

// VPSYNC lives in vpsync.rs (one command, registered once).

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

// ---------- EXTRIM, TREX, BTRIM, BEXTEND, SUPERHATCH ----------

/// Most picks one TREX call takes.
const MAX_PICKS: usize = 100_000;
/// Most tiles one SUPERHATCH makes.
const MAX_TILES: usize = 100_000;
/// Most nested edge segments one BTRIM or BEXTEND works with.
const MAX_NESTED_SEGMENTS: usize = 1_000_000;

pub fn trim_specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("extrim", "Extended Trim", run_extrim)
            .menu(&["Tools", "Express Tools", "Extended Trim"])
            .params("{boundary (handle of a closed polyline, circle, ellipse or spline), side: [x, y] (a point on the side to trim away)} → {trimmed}: every curve crossing the boundary loses its parts on that side")
            .interactive(|_| Ok(Box::new(ExtrimM::default()))),
        CommandSpec::new("trex", "Trim and Extend", run_trex)
            .menu(&["Tools", "Express Tools", "Trim and Extend"])
            .params("{edges? (handles; default every object), picks: [{handle, at: [x, y], extend?: bool}]} → {trimmed, extended}: trims each pick, or extends it to the edges when `extend` is true")
            .interactive(|_| Ok(Box::new(TrexM::default()))),
        CommandSpec::new("btrim", "Trim to Block Objects", |s, p| run_block_edges(s, p, false))
            .menu(&["Tools", "Express Tools", "Trim to Block Objects"])
            .params("{edges: [handles of block references (their nested objects cut) or plain objects], handle (the object to trim), pick: [x, y]} → {handles}: the objects replacing it")
            .interactive(|_| Ok(Box::new(BlockEdgesM::new(false)))),
        CommandSpec::new("bextend", "Extend to Block Objects", |s, p| run_block_edges(s, p, true))
            .menu(&["Tools", "Express Tools", "Extend to Block Objects"])
            .params("{edges: [handles of block references (their nested objects bound) or plain objects], handle (the object to extend), pick: [x, y] (nearest the end to extend)} → {extended}")
            .interactive(|_| Ok(Box::new(BlockEdgesM::new(true)))),
        CommandSpec::new("superhatch", "Super Hatch", run_superhatch)
            .menu(&["Tools", "Express Tools", "Super Hatch"])
            .params("{boundary (handle of a closed polyline, circle, ellipse or spline), block (name), spacing?: [dx, dy] (default the block's extents), mode?: block (the only mode available)} → {count, whole, clipped, group, handles}: the block tiled on a grid from the boundary's lower left corner; a tile is placed where its cell is inside or crosses the boundary, and tiles crossing it are clipped to it (XCLIP); grouped. Image, wipeout and xref modes are not supported")
            .interactive(|_| Ok(Box::new(SuperhatchM::default()))),
    ]
}

/// The objects a pick or a point at a prompt names (`None` for other input).
fn picked(s: &Session, i: &Input) -> Option<Result<Vec<Handle>>> {
    match i {
        Input::Pick(hs) => Some(Ok(hs.clone())),
        Input::Point(p) => Some(super::curves::pick_at(s, *p).map(|h| vec![h]).ok_or_else(|| other("*Invalid selection*"))),
        _ => None,
    }
}

fn handle_param(cmd: &str, p: &Value, key: &str) -> Result<Handle> {
    p.get(key).and_then(Value::as_str).and_then(Handle::parse_hex).ok_or_else(|| bad(cmd, format!("`{key}` (an object handle) is required")))
}

fn handle_list(cmd: &str, p: &Value, key: &str) -> Result<Option<Vec<Handle>>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) if a.len() <= MAX_PICKS => a
            .iter()
            .map(|v| v.as_str().and_then(Handle::parse_hex).ok_or_else(|| bad(cmd, format!("`{key}` must be a list of object handles"))))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        Some(_) => Err(bad(cmd, format!("`{key}` must be a list of object handles"))),
    }
}

/// The outline of a closed polyline, circle, ellipse or spline as a polygon, and its bounds.
fn closed_outline(s: &Session, cmd: &str, h: Handle) -> Result<(Vec<Vec2>, cadaddy_geom::Bounds2)> {
    let d = s.doc()?;
    let e = d.entity(h).ok_or_else(|| other("No such object."))?;
    let closed = match &e.kind {
        EntityKind::LwPolyline(p) => p.closed,
        EntityKind::Circle(_) => true,
        EntityKind::Ellipse(el) => cadaddy_geom::ccw_sweep(el.start, el.end) > std::f64::consts::TAU - 1e-9,
        EntityKind::Spline(sp) => sp.closed,
        _ => false,
    };
    if !closed {
        return Err(bad(cmd, "the boundary must be a closed polyline, circle, ellipse or spline"));
    }
    let b = cadaddy_doc::entity_bounds(d, e, 0);
    let tol = crate::select::window_tol(&b) / 10.0;
    // One polyline per segment of the outline: chain them into the polygon.
    let mut poly: Vec<Vec2> = Vec::new();
    for pl in crate::select::hit_polylines(d, e, tol) {
        for q in pl {
            if poly.last().is_none_or(|l| !l.near(q, 1e-12)) {
                poly.push(q);
            }
        }
    }
    if poly.len() > 1 && poly.first().zip(poly.last()).is_some_and(|(a, z)| a.near(*z, 1e-12)) {
        poly.pop();
    }
    if poly.len() < 3 || b.is_empty() {
        return Err(bad(cmd, "the boundary has no area"));
    }
    Ok((poly, b))
}

// ----- EXTRIM -----

/// The curves of the current space EXTRIM looks at: the kinds TRIM cuts, visible and unlocked.
fn extrim_candidates(s: &Session, boundary: Handle) -> Result<Vec<Handle>> {
    let d = s.doc()?;
    let store = d.space(&s.space()).ok_or(EngineError::NoDocument)?;
    Ok(store
        .iter()
        .take(MAX_SCAN)
        .filter(|e| e.handle != boundary && d.is_visible(e))
        .filter(|e| {
            matches!(
                e.kind,
                EntityKind::Line(_)
                    | EntityKind::Arc(_)
                    | EntityKind::Circle(_)
                    | EntityKind::LwPolyline(_)
                    | EntityKind::Ellipse(_)
                    | EntityKind::Spline(_)
            )
        })
        .map(|e| e.handle)
        .filter(|h| !super::curves::is_locked(s, *h))
        .collect())
}

/// One point on every stretch of `h` between boundary crossings that lies on the trim side.
fn extrim_picks(s: &Session, h: Handle, poly: &[Vec2], inside_wanted: bool, tol: f64) -> Result<Vec<Vec2>> {
    let d = s.doc()?;
    let e = d.entity(h).ok_or_else(|| other("No such object."))?;
    let edges: Vec<Line> = (0..poly.len()).filter_map(|i| Some(Line::new(*poly.get(i)?, *poly.get((i + 1) % poly.len())?))).collect();
    let mut picks = Vec::new();
    let mut crossings = 0usize;
    for pl in crate::select::hit_polylines(d, e, tol) {
        for w in pl.windows(2) {
            let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
            let seg = Line::new(*a, *b);
            let mut ts = vec![0.0, 1.0];
            for be in &edges {
                if let Some(x) = cadaddy_geom::line_line(&seg, be) {
                    let t = seg.param_of(x).clamp(0.0, 1.0);
                    if t > 1e-9 && t < 1.0 - 1e-9 {
                        crossings += 1;
                    }
                    ts.push(t);
                }
            }
            ts.sort_by(f64::total_cmp);
            for pair in ts.windows(2) {
                let (Some(t0), Some(t1)) = (pair.first(), pair.get(1)) else { continue };
                if t1 - t0 < 1e-9 {
                    continue;
                }
                let mid = seg.at((t0 + t1) / 2.0);
                if cadaddy_geom::point_in_polygon(poly, mid) == inside_wanted {
                    picks.push(mid);
                }
            }
        }
    }
    // A curve that never crosses the boundary (inside, outside or only touching it) stays.
    if crossings == 0 { Ok(Vec::new()) } else { Ok(picks) }
}

fn extrim(s: &mut Session, boundary: Handle, side: Vec2) -> Result<usize> {
    let (poly, b) = closed_outline(s, "extrim", boundary)?;
    let inside_wanted = cadaddy_geom::point_in_polygon(&poly, side);
    let tol = crate::select::window_tol(&b);
    let only = [boundary];
    let edges = Edges { only: Some(&only), ..Edges::default() };
    let mut work: Vec<(Handle, Vec<Vec2>)> = Vec::new();
    for h in extrim_candidates(s, boundary)? {
        let picks = extrim_picks(s, h, &poly, inside_wanted, tol / 10.0)?;
        if !picks.is_empty() {
            work.push((h, picks));
        }
    }
    if work.is_empty() {
        return Ok(0);
    }
    super::trimextend::run_picks(s, work, false, edges, false, tol)
}

fn run_extrim(s: &mut Session, p: &Value) -> Result<Value> {
    let boundary = handle_param("extrim", p, "boundary")?;
    let side = point_param(p, "side").ok_or_else(|| bad("extrim", "`side` ([x, y], a point on the side to trim) is required"))?;
    let n = extrim(s, boundary, side)?;
    Ok(json!({ "trimmed": n, "message": format!("{n} object(s) trimmed.") }))
}

#[derive(Default)]
struct ExtrimM {
    boundary: Option<Handle>,
}

impl Interactive for ExtrimM {
    fn name(&self) -> &'static str {
        "EXTRIM"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.boundary {
            None => Prompt::new("Select a closed polyline, circle, ellipse or spline as the cutting edge", Accept::SELECT),
            Some(_) => Prompt::new("Specify a point on the side to trim", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.boundary, &i) {
            (_, Input::Enter) => Ok(Step::Cancel),
            (None, _) => {
                if let Some(r) = picked(s, &i) {
                    let h = r?.first().copied().ok_or_else(|| other("*Invalid selection*"))?;
                    closed_outline(s, "extrim", h)?;
                    self.boundary = Some(h);
                }
                Ok(Step::Continue)
            }
            (Some(b), Input::Point(p)) => {
                let n = extrim(s, b, *p)?;
                s.echo(format!("{n} object(s) trimmed."));
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
}

// ----- TREX -----

fn run_trex(s: &mut Session, p: &Value) -> Result<Value> {
    let only = handle_list("trex", p, "edges")?;
    let picks = p.get("picks").and_then(Value::as_array).filter(|a| !a.is_empty() && a.len() <= MAX_PICKS);
    let picks = picks.ok_or_else(|| bad("trex", "`picks` needs a list of {handle, at: [x, y], extend?: bool}"))?;
    // Check every pick before changing anything.
    let mut work = Vec::new();
    for pk in picks {
        let h = handle_param("trex", pk, "handle")?;
        let at = point_param(pk, "at").ok_or_else(|| bad("trex", "each pick needs `at` ([x, y])"))?;
        if s.doc()?.entity(h).is_none() {
            return Err(bad("trex", format!("no object {}", h.hex())));
        }
        work.push((h, at, pk.get("extend").and_then(Value::as_bool).unwrap_or(false)));
    }
    let edges = Edges { only: only.as_deref(), ..Edges::default() };
    let (mut trimmed, mut extended) = (0, 0);
    for (h, at, extend) in work {
        super::trimextend::apply(s, h, at, extend, edges, false)?;
        if extend {
            extended += 1;
        } else {
            trimmed += 1;
        }
    }
    Ok(json!({ "trimmed": trimmed, "extended": extended, "message": format!("{trimmed} trimmed, {extended} extended.") }))
}

#[derive(Default)]
struct TrexM {
    edges: Vec<Handle>,
    picking: bool,
    extend: bool,
}

impl Interactive for TrexM {
    fn name(&self) -> &'static str {
        "TREX"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if !self.picking {
            return Prompt::new("Select cutting and boundary edges or <select all>", Accept::SELECT);
        }
        let msg = if self.extend { "Select object to extend or [Trim]" } else { "Select object to trim or [Extend]" };
        Prompt::new(msg, Accept::POINT).kw(&["Trim", "Extend"])
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if !self.picking {
            return match picked(s, &i) {
                Some(r) => {
                    for h in r? {
                        if !self.edges.contains(&h) {
                            self.edges.push(h);
                        }
                    }
                    Ok(Step::Continue)
                }
                None if matches!(i, Input::Enter) => {
                    self.picking = true;
                    Ok(Step::Continue)
                }
                None => Ok(Step::Continue),
            };
        }
        match i {
            Input::Keyword(k) | Input::Text(k) => {
                match k.trim().to_ascii_lowercase().as_str() {
                    "e" | "extend" => self.extend = true,
                    "t" | "trim" => self.extend = false,
                    _ => {}
                }
                Ok(Step::Continue)
            }
            Input::Point(p) => {
                let h = super::curves::pick_at(s, p).ok_or_else(|| other("*Invalid selection*"))?;
                let only = (!self.edges.is_empty()).then_some(self.edges.as_slice());
                super::trimextend::apply(s, h, p, self.extend, Edges { only, ..Edges::default() }, false)?;
                Ok(Step::Continue)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}

// ----- BTRIM, BEXTEND -----

/// The cutting or boundary edges of BTRIM and BEXTEND: a block reference gives the objects nested
/// in it (in world coordinates), any other object itself.
fn block_edges(s: &Session, hs: &[Handle]) -> Result<(Vec<Handle>, Vec<cadaddy_geom::Segment>)> {
    let d = s.doc()?;
    let mut plain = Vec::new();
    let mut segs = Vec::new();
    for h in hs {
        let e = d.entity(*h).ok_or_else(|| other("No such object."))?;
        match &e.kind {
            EntityKind::Insert(ins) => {
                let mut all = Vec::new();
                nested_objects(d, ins, &Mat3::IDENTITY, 0, &mut all);
                for (_, _, k) in &all {
                    for pr in k.prims() {
                        segs.extend(super::modify::prim_to_segments(&pr));
                    }
                }
                if segs.len() > MAX_NESTED_SEGMENTS {
                    return Err(other("The block references hold too much geometry."));
                }
            }
            _ => plain.push(*h),
        }
    }
    Ok((plain, segs))
}

fn run_block_edges(s: &mut Session, p: &Value, extending: bool) -> Result<Value> {
    let cmd = if extending { "bextend" } else { "btrim" };
    let hs =
        handle_list(cmd, p, "edges")?.filter(|v| !v.is_empty()).ok_or_else(|| bad(cmd, "`edges` (block reference or object handles) is required"))?;
    let h = targets(s, p)?.first().copied().ok_or_else(|| bad(cmd, "`handle` is required"))?;
    let pick = point_param(p, "pick").ok_or_else(|| bad(cmd, "`pick` ([x, y]) is required"))?;
    let (plain, segs) = block_edges(s, &hs)?;
    if segs.is_empty() && plain.is_empty() {
        return Err(bad(cmd, "the block references hold no objects"));
    }
    let edges = Edges { only: Some(&plain), implied: false, extra: &segs };
    if extending {
        super::modify::extend(s, h, pick, edges)?;
        Ok(json!({ "extended": h.hex() }))
    } else {
        let r = super::modify::trim(s, h, pick, edges)?;
        Ok(json!({ "handles": hexes(&r) }))
    }
}

struct BlockEdgesM {
    extend: bool,
    edges: Vec<Handle>,
    picking: bool,
}

impl BlockEdgesM {
    fn new(extend: bool) -> Self {
        BlockEdgesM { extend, edges: Vec::new(), picking: false }
    }
}

impl Interactive for BlockEdgesM {
    fn name(&self) -> &'static str {
        if self.extend { "BEXTEND" } else { "BTRIM" }
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (self.picking, self.extend) {
            (false, false) => Prompt::new("Select block references or objects as cutting edges", Accept::SELECT),
            (false, true) => Prompt::new("Select block references or objects as boundary edges", Accept::SELECT),
            (true, false) => Prompt::new("Select object to trim", Accept::POINT),
            (true, true) => Prompt::new("Select object to extend", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if !self.picking {
            return match picked(s, &i) {
                Some(r) => {
                    for h in r? {
                        if !self.edges.contains(&h) {
                            self.edges.push(h);
                        }
                    }
                    Ok(Step::Continue)
                }
                None if matches!(i, Input::Enter) => {
                    if self.edges.is_empty() {
                        return Ok(Step::Cancel);
                    }
                    self.picking = true;
                    Ok(Step::Continue)
                }
                None => Ok(Step::Continue),
            };
        }
        match i {
            Input::Point(p) => {
                let h = super::curves::pick_at(s, p).ok_or_else(|| other("*Invalid selection*"))?;
                let params = json!({ "edges": hexes(&self.edges), "handle": h.hex(), "pick": [p.x, p.y] });
                run_block_edges(s, &params, self.extend)?;
                Ok(Step::Continue)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}

// ----- SUPERHATCH -----

#[derive(Clone, Copy, PartialEq, Debug)]
enum Cover {
    Outside,
    Partial,
    Whole,
}

/// Whether the cell `[lo, hi]` lies outside, across or inside the polygon `poly`.
fn cell_cover(poly: &[Vec2], lo: Vec2, hi: Vec2) -> Cover {
    // Shrunk a hair, so a cell whose sides run along the outline counts as inside it.
    let eps = (hi.x - lo.x).min(hi.y - lo.y).abs() * 1e-6;
    let (a, c) = (Vec2::new(lo.x + eps, lo.y + eps), Vec2::new(hi.x - eps, hi.y - eps));
    let (b, d) = (Vec2::new(c.x, a.y), Vec2::new(a.x, c.y));
    let corners = [a, b, c, d];
    let sides: Vec<Line> = (0..4).filter_map(|i| Some(Line::new(*corners.get(i)?, *corners.get((i + 1) % 4)?))).collect();
    let n = poly.len();
    let crossed = (0..n).any(|i| {
        let (Some(p), Some(q)) = (poly.get(i), poly.get((i + 1) % n)) else { return false };
        let within = |v: &Vec2| v.x > a.x && v.x < c.x && v.y > a.y && v.y < c.y;
        within(p) || sides.iter().any(|sd| cadaddy_geom::line_line(sd, &Line::new(*p, *q)).is_some())
    });
    let inside = corners.iter().filter(|c| cadaddy_geom::point_in_polygon(poly, **c)).count();
    match (crossed, inside) {
        (true, _) => Cover::Partial,
        (false, 4) => Cover::Whole,
        (false, 0) => Cover::Outside,
        _ => Cover::Partial,
    }
}

fn spacing_param(p: &Value) -> Result<Option<Vec2>> {
    match p.get("spacing") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let sp = cadaddy_engine_point(v).filter(|sp| sp.x.is_finite() && sp.y.is_finite() && sp.x > 0.0 && sp.y > 0.0);
            sp.map(Some).ok_or_else(|| bad("superhatch", "`spacing` must be [dx, dy], both greater than 0"))
        }
    }
}

fn cadaddy_engine_point(v: &Value) -> Option<Vec2> {
    super::point_value(v)
}

fn run_superhatch(s: &mut Session, p: &Value) -> Result<Value> {
    for key in ["image", "wipeout", "xref"] {
        if p.get(key).is_some_and(|v| !v.is_null()) {
            return Err(bad("superhatch", format!("the {key} mode is not supported; SUPERHATCH tiles a block (`block`)")));
        }
    }
    if let Some(m) = p.get("mode").and_then(Value::as_str)
        && !m.trim().eq_ignore_ascii_case("block")
    {
        return Err(bad("superhatch", format!("the {} mode is not supported; SUPERHATCH tiles a block (`block`)", m.trim().to_ascii_lowercase())));
    }
    let boundary = handle_param("superhatch", p, "boundary")?;
    let name = str_param(p, "block").ok_or_else(|| bad("superhatch", "`block` (a block name) is required"))?.to_string();
    let spacing = spacing_param(p)?;
    let (poly, bb) = closed_outline(s, "superhatch", boundary)?;
    let blk = s.doc()?.block(&name).cloned().ok_or_else(|| bad("superhatch", format!("no block named \"{name}\"")))?;
    let d = s.doc()?;
    let ext = blk.entities.iter().fold(cadaddy_geom::Bounds2::EMPTY, |acc, e| acc.union(&cadaddy_doc::entity_bounds(d, e, 1)));
    if ext.is_empty() || !(ext.width() > 0.0 || ext.height() > 0.0) {
        return Err(bad("superhatch", format!("block \"{name}\" has no extents to tile")));
    }
    let size = Vec2::new(ext.width(), ext.height());
    let step = spacing.unwrap_or(size);
    if !(step.x > 0.0 && step.y > 0.0) {
        return Err(bad("superhatch", "give `spacing` for a block with no width or height"));
    }
    let nx = ((bb.width() / step.x).ceil() as usize).max(1);
    let ny = ((bb.height() / step.y).ceil() as usize).max(1);
    if nx.saturating_mul(ny) > MAX_TILES {
        return Err(bad("superhatch", format!("that is more than {MAX_TILES} tiles; use a larger `spacing`")));
    }
    let eps = 1e-9 * (bb.width() + bb.height());
    let mut cells = Vec::new();
    for iy in 0..ny {
        for ix in 0..nx {
            let lo = Vec2::new(bb.min.x + ix as f64 * step.x, bb.min.y + iy as f64 * step.y);
            if lo.x >= bb.max.x - eps || lo.y >= bb.max.y - eps {
                continue;
            }
            match cell_cover(&poly, lo, lo + size) {
                Cover::Outside => {}
                c => cells.push((lo, c)),
            }
        }
    }
    if cells.is_empty() {
        return Err(bad("superhatch", "no tile of the block falls inside the boundary"));
    }
    let (mut hs, mut clipped) = (Vec::new(), 0);
    let none = serde_json::Map::new();
    for (lo, cover) in cells {
        let h = super::blocks::insert_with(s, &name, lo, Vec2::new(1.0, 1.0), 0.0, Some(ext.min), &none)?;
        if cover == Cover::Partial {
            let boundary_in_block = {
                let d = s.doc()?;
                let Some(EntityKind::Insert(ins)) = d.entity(h).map(|e| &e.kind) else { continue };
                let inv = ins.transform(blk.base.xy()).inverse().ok_or_else(|| other("The tile cannot be clipped."))?;
                poly.iter().map(|q| inv.apply(*q)).collect::<Vec<_>>()
            };
            s.doc_mut()?.modify_entity(h, |e| {
                if let EntityKind::Insert(i) = &mut e.kind {
                    i.clip =
                        Some(Box::new(cadaddy_doc::XClip { boundary: boundary_in_block, enabled: true, inverted: false, front: None, back: None }));
                }
            })?;
            clipped += 1;
        }
        hs.push(h);
    }
    let group = super::groups::create(s, None, "SUPERHATCH", &hs, true)?;
    Ok(json!({
        "count": hs.len(), "whole": hs.len() - clipped, "clipped": clipped, "group": group, "handles": hexes(&hs),
        "message": format!("{} tile(s) of \"{name}\" ({clipped} clipped to the boundary).", hs.len()),
    }))
}

#[derive(Default)]
struct SuperhatchM {
    boundary: Option<Handle>,
    block: Option<String>,
}

impl Interactive for SuperhatchM {
    fn name(&self) -> &'static str {
        "SUPERHATCH"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (&self.boundary, &self.block) {
            (None, _) => Prompt::new("Select a closed polyline, circle, ellipse or spline as the boundary", Accept::SELECT),
            (Some(_), None) => Prompt::new("Enter block name", Accept::TEXT),
            (Some(_), Some(_)) => Prompt::new("Specify spacing as dx,dy or <block size>", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (&self.boundary, &self.block, i) {
            (_, _, Input::Enter) if self.block.is_none() => Ok(Step::Cancel),
            (None, _, i) => {
                if let Some(r) = picked(s, &i) {
                    let h = r?.first().copied().ok_or_else(|| other("*Invalid selection*"))?;
                    closed_outline(s, "superhatch", h)?;
                    self.boundary = Some(h);
                }
                Ok(Step::Continue)
            }
            (Some(_), None, Input::Text(t)) => {
                let t = t.trim().to_string();
                if s.doc()?.block(&t).is_none() {
                    return Err(other(format!("No block named \"{t}\".")));
                }
                self.block = Some(t);
                Ok(Step::Continue)
            }
            (Some(b), Some(name), Input::Enter) => {
                let r = run_superhatch(s, &json!({ "boundary": b.hex(), "block": name }))?;
                self.finish(s, &r)
            }
            (Some(b), Some(name), Input::Text(t)) => {
                let nums: Vec<f64> = t.split([',', ' ']).filter(|v| !v.is_empty()).filter_map(|v| v.parse().ok()).collect();
                let sp = match nums.as_slice() {
                    [d] => json!([d, d]),
                    [dx, dy] => json!([dx, dy]),
                    _ => return Err(other("Enter the spacing as dx,dy.")),
                };
                let r = run_superhatch(s, &json!({ "boundary": b.hex(), "block": name, "spacing": sp }))?;
                self.finish(s, &r)
            }
            _ => Ok(Step::Continue),
        }
    }
}

impl SuperhatchM {
    fn finish(&self, s: &mut Session, r: &Value) -> Result<Step> {
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            s.echo(m.to_string());
        }
        Ok(Step::Done)
    }
}
