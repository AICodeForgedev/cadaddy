//! DIMREGEN, and the single DIM command's modes: what to dimension is picked from what it is
//! given (two points: linear; a line: aligned; a circle: diameter; an arc: radius; two lines:
//! angular) or named by `mode`, which also covers Baseline, Continue, Ordinate, Align (onto a
//! base dimension's line), Distribute (equally between the outermost, or at an offset) and Layer
//! (DIMLAYER: the layer new dimensions go on).

use cadaddy_doc::{DimKind, EntityKind, Handle};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("dimregen", "Regenerate Dimensions", run_dimregen).params(
            "{} → {updated, regenerated}: associative dimensions put back on their objects as they are now, every dimension's graphics rebuilt",
        ),
    ]
}

/// The DIM command's parameters (its JSON form).
pub const DIM_PARAMS: &str = "{p1, p2, at} (linear) | {handle, at?} (a line: aligned; a circle: diameter; an arc: radius) | {lines: [hex, hex], at} (angular) | {mode: linear|aligned|angular|radius|diameter|ordinate|baseline|continue (those commands' parameters) | align {base, handles} | distribute {handles, type?: equal|offset, offset?} | layer {layer (\".\": the current layer)}}";

fn run_dimregen(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?.clone();
    let mut updated = 0usize;
    let mut regenerated = 0usize;
    let mut changes: Vec<(Handle, cadaddy_doc::Dimension)> = Vec::new();
    for store in std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities)) {
        for e in store.iter() {
            let EntityKind::Dimension(dm) = &e.kind else { continue };
            let nd = if dm.assoc.is_empty() {
                let mut x = (**dm).clone();
                x.block = None;
                x
            } else {
                let x = crate::assoc::regen(dm, &d);
                let mut was = (**dm).clone();
                was.block = None;
                if x != was {
                    updated += 1;
                }
                x
            };
            regenerated += 1;
            changes.push((e.handle, nd));
        }
    }
    let dm = s.doc_mut()?;
    for (h, nd) in changes {
        dm.modify_entity(h, |e| e.kind = EntityKind::Dimension(Box::new(nd)))?;
    }
    s.echo(format!("{regenerated} dimensions regenerated, {updated} put back on their objects."));
    Ok(json!({ "updated": updated, "regenerated": regenerated }))
}

/// Run another command with the same parameters (less `mode`).
fn pass(s: &mut Session, id: &str, p: &Value) -> Result<Value> {
    let mut q = p.clone();
    if let Some(o) = q.as_object_mut() {
        o.remove("mode");
    }
    let spec = find_command(id).ok_or_else(|| bad("dim", format!("{id} is not available")))?;
    (spec.run)(s, &q)
}

fn hex(v: &Value) -> Option<Handle> {
    v.as_str().and_then(Handle::parse_hex)
}

/// The DIM command's JSON form.
pub fn run_dim(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "dim";
    let mode = str_param(p, "mode").map(str::to_ascii_lowercase);
    match mode.as_deref() {
        Some(m @ ("linear" | "aligned" | "angular" | "radius" | "diameter" | "ordinate" | "baseline" | "continue")) => {
            let id = format!("dim{m}");
            return pass(s, &id, p);
        }
        Some("align") => return align(s, p, 0.0),
        Some("distribute") => return distribute(s, p),
        Some("layer") => {
            let l = str_param(p, "layer")
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .ok_or_else(|| bad(cmd, "`layer` is the layer for new dimensions (\".\" the current one)"))?;
            if l != "." {
                s.doc_mut()?.ensure_layer(l);
            }
            s.doc_mut()?.header.set_str("DIMLAYER", l);
            return Ok(json!({ "dimlayer": l }));
        }
        Some(other) => return Err(bad(cmd, format!("unknown mode `{other}`"))),
        None => {}
    }
    // What to dimension, from what was given.
    if p.get("lines").and_then(Value::as_array).is_some_and(|a| a.len() == 2) || p.get("vertex").is_some() {
        return pass(s, "dimangular", p);
    }
    if let Some(h) = p.get("handle").and_then(hex) {
        let kind = s.doc()?.entity(h).map(|e| e.kind.clone()).ok_or_else(|| bad(cmd, "no such object"))?;
        return match kind {
            EntityKind::Circle(_) => pass(s, "dimdiameter", p),
            EntityKind::Arc(_) => pass(s, "dimradius", p),
            EntityKind::Line(l) => {
                let at = p.get("at").cloned().unwrap_or_else(|| {
                    // Beside the line, a little off its middle.
                    let (a, b) = (l.a.xy(), l.b.xy());
                    let n = (b - a).normalized().perp();
                    let m = (a + b) * 0.5 + n * ((b - a).len() * 0.1).max(1.0);
                    json!([m.x, m.y])
                });
                pass(s, "dimaligned", &json!({ "p1": [l.a.x, l.a.y], "p2": [l.b.x, l.b.y], "at": at }))
            }
            _ => Err(bad(cmd, "pick a line, circle or arc (or give p1, p2 and at)")),
        };
    }
    pass(s, "dimlinear", p)
}

/// Align: the dimensions' lines onto the base dimension's (DIMSPACE at `spacing`).
fn align(s: &mut Session, p: &Value, spacing: f64) -> Result<Value> {
    let base = p.get("base").cloned().ok_or_else(|| bad("dim", "`base` is the dimension to align to"))?;
    let handles = p.get("handles").cloned().ok_or_else(|| bad("dim", "`handles` are the dimensions to align"))?;
    let r = pass(s, "dimspace", &json!({ "base": base, "handles": handles, "spacing": spacing }))?;
    Ok(json!({ "aligned": r["spaced"] }))
}

/// Where a linear or aligned dimension's line is across its direction, and that direction's normal.
fn line_offset(dm: &cadaddy_doc::Dimension) -> Option<(Vec2, f64)> {
    let dir = match dm.kind {
        DimKind::Linear { rotation } => Vec2::from_angle(rotation),
        DimKind::Aligned => (dm.p14.xy() - dm.p13.xy()).normalized(),
        _ => return None,
    };
    let n = dir.perp();
    Some((n, dm.defpt.xy().dot(n)))
}

/// Distribute: parallel dimensions equally between the outermost two (`equal`), or at `offset`
/// from the first one given (`offset`).
fn distribute(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "dim";
    let hs: Vec<Handle> = p.get("handles").and_then(Value::as_array).map(|a| a.iter().filter_map(hex).collect()).unwrap_or_default();
    if hs.len() < 2 {
        return Err(bad(cmd, "`handles`: two or more parallel linear or aligned dimensions"));
    }
    let kind = str_param(p, "type").unwrap_or("equal").to_ascii_lowercase();
    if kind == "offset" {
        let offset = p
            .get("offset")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or_else(|| bad(cmd, "`offset`: the distance between dimension lines"))?;
        let (base, rest) = hs.split_first().ok_or_else(|| bad(cmd, "no dimensions"))?;
        let r = pass(s, "dimspace", &json!({ "base": base.hex(), "handles": rest.iter().map(|h| h.hex()).collect::<Vec<_>>(), "spacing": offset }))?;
        return Ok(json!({ "distributed": r["spaced"] }));
    }
    if kind != "equal" {
        return Err(bad(cmd, "`type` is equal or offset"));
    }
    let d = s.doc()?;
    let mut lines: Vec<(Handle, Vec2, f64)> = Vec::new();
    let mut normal: Option<Vec2> = None;
    for h in &hs {
        let Some(EntityKind::Dimension(dm)) = d.entity(*h).map(|e| &e.kind) else { continue };
        let Some((n, _)) = line_offset(dm) else { continue };
        let n0 = *normal.get_or_insert(n);
        // Parallel only (either way round), measured along the first one's normal.
        if n.cross(n0).abs() > 1e-6 {
            continue;
        }
        lines.push((*h, dm.defpt.xy(), dm.defpt.xy().dot(n0)));
    }
    let Some(n) = normal.filter(|_| lines.len() >= 2) else { return Err(bad(cmd, "two or more parallel linear or aligned dimensions are needed")) };
    lines.sort_by(|a, b| a.2.total_cmp(&b.2));
    let (lo, hi) = (lines.first().map_or(0.0, |l| l.2), lines.last().map_or(0.0, |l| l.2));
    let step = (hi - lo) / (lines.len() - 1) as f64;
    let dm = s.doc_mut()?;
    for (i, (h, at, off)) in lines.iter().enumerate() {
        let target = lo + step * i as f64;
        let moved = *at + n * (target - off);
        dm.modify_entity(*h, |e| {
            if let EntityKind::Dimension(o) = &mut e.kind {
                o.defpt.x = moved.x;
                o.defpt.y = moved.y;
                o.user_text_pos = false;
                o.block = None;
            }
        })?;
    }
    Ok(json!({ "distributed": lines.len(), "spacing": step }))
}

/// Typed DIM: the linear dimension's prompts, and at the first one the other modes as keywords —
/// Baseline, Continue and Ordinate (their own prompts from there), aLign (DIMSPACE's prompts;
/// spacing 0 aligns) and LAyer (the layer new dimensions go on). Distribute is the JSON form's.
pub struct DimM {
    inner: Box<dyn crate::prompt::Interactive>,
    started: bool,
    asking_layer: bool,
}

const DIM_KEYWORDS: [&str; 5] = ["Baseline", "Continue", "Ordinate", "aLign", "LAyer"];

fn machine(s: &Session, id: &str) -> Result<Box<dyn crate::prompt::Interactive>> {
    let f = find_command(id).and_then(|c| c.interactive).ok_or_else(|| bad("dim", format!("{id} has no prompts")))?;
    f(s)
}

impl DimM {
    pub fn start(s: &Session) -> Result<Box<dyn crate::prompt::Interactive>> {
        Ok(Box::new(DimM { inner: machine(s, "dimlinear")?, started: false, asking_layer: false }))
    }
}

impl crate::prompt::Interactive for DimM {
    fn name(&self) -> &'static str {
        "dim"
    }
    fn prompt(&self, s: &Session) -> crate::prompt::Prompt {
        if self.asking_layer {
            let cur = s.doc().map(|d| d.header.str("DIMLAYER", ".")).unwrap_or_else(|_| ".".into());
            return crate::prompt::Prompt::new("Enter layer name for new dimensions (. for the current layer)", crate::prompt::Accept::TEXT)
                .default(cur);
        }
        let mut p = self.inner.prompt(s);
        if !self.started {
            p.keywords.extend(DIM_KEYWORDS.iter().map(|k| (*k).to_string()));
        }
        p
    }
    fn begin(&mut self, s: &mut Session) -> Result<crate::prompt::Step> {
        self.inner.begin(s)
    }
    fn input(&mut self, s: &mut Session, i: crate::Input) -> Result<crate::prompt::Step> {
        use crate::prompt::Step;
        if self.asking_layer {
            let name = match &i {
                crate::Input::Text(t) if !t.trim().is_empty() => t.trim().to_string(),
                crate::Input::Enter | crate::Input::Text(_) => ".".to_string(),
                crate::Input::Cancel => return Ok(Step::Cancel),
                _ => return Ok(Step::Continue),
            };
            if name != "." {
                s.doc_mut()?.ensure_layer(&name);
            }
            s.doc_mut()?.header.set_str("DIMLAYER", &name);
            s.echo(format!("New dimensions go on {}.", if name == "." { "the current layer".to_string() } else { name }));
            self.asking_layer = false;
            return Ok(Step::Continue);
        }
        if !self.started
            && let crate::Input::Keyword(k) = &i
        {
            let to = match k.to_ascii_lowercase().as_str() {
                "baseline" => Some("dimbaseline"),
                "continue" => Some("dimcontinue"),
                "ordinate" => Some("dimordinate"),
                "align" => Some("dimspace"),
                "layer" => {
                    self.asking_layer = true;
                    return Ok(Step::Continue);
                }
                _ => None,
            };
            if let Some(id) = to {
                self.inner = machine(s, id)?;
                self.started = true;
                return self.inner.begin(s);
            }
        }
        let r = self.inner.input(s, i)?;
        self.started = true;
        Ok(r)
    }
    fn preview(&self, s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        if self.asking_layer { Vec::new() } else { self.inner.preview(s, cursor) }
    }
    fn drag(&self, s: &Session, cursor: Vec2) -> Option<crate::prompt::Drag> {
        self.inner.drag(s, cursor)
    }
    fn editing_mtext(&self) -> Option<cadaddy_doc::MText> {
        self.inner.editing_mtext()
    }
}
