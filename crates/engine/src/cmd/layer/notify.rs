//! Layer notification (LAYEREVAL, LAYERNOTIFY, LAYER rEconcile): layers that arrive with xrefs
//! (or by any means, LAYEREVAL 2) after the drawing's layer list was last reviewed are
//! "unreconciled" until reconciled.
//!
//! The reviewed list lives in the header as JSON under [`KNOWN_KEY`] (kept in DXF as an XRECORD).
//! No list means nothing has been evaluated yet: every layer counts as reconciled.

use cadaddy_doc::Drawing;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Header key of the reconciled layer names (a JSON array).
pub const KNOWN_KEY: &str = "CADADDY_KNOWNLAYERS";

/// LAYERNOTIFY bits: when the unreconciled layers are reported (1 plot, 2 open, 4 xref load or
/// reload, 8 layer state restore, 16 save, 32 insert; CADaddy reports on open and xref reload).
pub const ON_OPEN: i64 = 2;
pub const ON_XREF: i64 = 4;

fn known(d: &Drawing) -> Option<Vec<String>> {
    let text = d.header.str(KNOWN_KEY, "");
    if text.is_empty() {
        return None;
    }
    serde_json::from_str(&text).ok()
}

fn set_known(d: &mut Drawing, names: &[String]) {
    d.header.set_str(KNOWN_KEY, &serde_json::to_string(names).unwrap_or_default());
}

/// LAYEREVAL: 0 off, 1 new xref layers, 2 new xref and drawing layers.
pub fn layereval(d: &Drawing) -> i64 {
    d.header.i64("LAYEREVAL", 1).clamp(0, 2)
}

/// Start the reviewed list (every layer now in the drawing) when evaluation is on and none exists.
pub fn ensure_baseline(d: &mut Drawing) {
    if layereval(d) != 0 && known(d).is_none() {
        let all: Vec<String> = d.layers.iter().map(|l| l.name.clone()).collect();
        set_known(d, &all);
    }
}

/// Mark layers as reviewed (a layer the user creates is reconciled from the start).
pub fn reconcile_names(d: &mut Drawing, names: &[String]) {
    let Some(mut k) = known(d) else { return };
    for n in names {
        if !k.iter().any(|x| x.eq_ignore_ascii_case(n)) {
            k.push(n.clone());
        }
    }
    set_known(d, &k);
}

/// The unreconciled layers, in layer-table order.
pub fn unreconciled(d: &Drawing) -> Vec<String> {
    let eval = layereval(d);
    let Some(k) = known(d).filter(|_| eval != 0) else { return Vec::new() };
    d.layers
        .iter()
        .filter(|l| eval == 2 || l.name.contains('|'))
        .filter(|l| !k.iter().any(|x| x.eq_ignore_ascii_case(&l.name)))
        .map(|l| l.name.clone())
        .collect()
}

/// After an event of kind `bit` (the list was started before it): echo the unreconciled layers
/// when LAYERNOTIFY asks for that event. Returns them.
pub fn after_event(s: &mut Session, bit: i64) -> Vec<String> {
    let Ok(d) = s.doc() else { return Vec::new() };
    let list = unreconciled(d);
    let notify = d.header.i64("LAYERNOTIFY", 15);
    if !list.is_empty() && notify & bit != 0 {
        let shown: Vec<&str> = list.iter().take(20).map(String::as_str).collect();
        let more = if list.len() > shown.len() { format!(" and {} more", list.len() - shown.len()) } else { String::new() };
        s.echo(format!("Unreconciled new layers: {}{more}. Review them in the Layer Properties Manager (LAYER rEconcile).", shown.join(", ")));
    }
    list
}

/// `layer.unreconciled`: the layers waiting for review.
pub fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    Ok(json!({ "layers": unreconciled(d), "layereval": layereval(d), "evaluated": known(d).is_some() }))
}

/// `layer.reconcile`: mark `names` (default every unreconciled layer) as reviewed.
pub fn run_reconcile(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc_mut()?;
    let pending = unreconciled(d);
    let names: Vec<String> = match p.get("names") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).take(10_000).map(str::to_string).collect(),
        Some(Value::String(t)) => t.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect(),
        None | Some(Value::Null) => pending.clone(),
        Some(_) => return Err(bad("layer.reconcile", "`names` is a list of layer names")),
    };
    for n in &names {
        if d.layer(n).is_none() {
            return Err(bad("layer.reconcile", format!("no layer named \"{n}\"")));
        }
    }
    ensure_baseline(d);
    reconcile_names(d, &names);
    Ok(json!({ "reconciled": names.iter().filter(|n| pending.iter().any(|x| x.eq_ignore_ascii_case(n))).collect::<Vec<_>>() }))
}
