//! Layer states beyond save and restore: the properties restoring sets, "turn off layers not in
//! the state", editing a state's layer entries, and `.las` files.
//!
//! A `.las` file here is CADaddy's own JSON (the layout of AutoCAD's binary-ish .las files isn't
//! published):
//!
//! ```json
//! { "format": "cadaddy-layerstate", "version": 1, "name": "Plot",
//!   "description": "…", "offOthers": false,
//!   "restore": { "on": true, "frozen": true, "locked": true, "plot": true, "vpFreezeNew": true,
//!                "color": true, "linetype": true, "lineweight": true, "transparency": true,
//!                "plotStyle": true },
//!   "layers": [ { "name": "WALLS", "color": …, "on": true, … } ] }
//! ```
//!
//! `layers` holds each layer as the drawing keeps it. Importing adds the layers the drawing lacks
//! (with linetypes it doesn't have shown as Continuous) and saves the state.

use cadaddy_doc::{Layer, LayerState, LayerStateMask};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::*;
use crate::{EngineError, Result, Session};

const FORMAT: &str = "cadaddy-layerstate";
/// Largest .las file read.
const MAX_FILE: u64 = 16 * 1024 * 1024;
/// Most layers one state may hold.
const MAX_LAYERS: usize = 100_000;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LasFile {
    format: String,
    version: u32,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    off_others: bool,
    #[serde(default)]
    restore: LayerStateMask,
    layers: Vec<Layer>,
}

pub(super) fn state_json(st: &LayerState) -> Value {
    json!({
        "name": st.name,
        "description": st.description,
        "layers": st.layers.len(),
        "restore": st.restore,
        "offOthers": st.off_others,
    })
}

/// The `restore` mask of a call, if given: `{on?, frozen?, …}` (left out = on).
pub(super) fn mask_param(p: &Value) -> Result<Option<LayerStateMask>> {
    match p.get("restore") {
        None | Some(Value::Null) => Ok(None),
        Some(v @ Value::Object(_)) => serde_json::from_value(v.clone()).map(Some).map_err(|e| bad("layerstate", format!("`restore`: {e}"))),
        Some(_) => Err(bad("layerstate", "`restore` is an object of true/false properties")),
    }
}

fn names(v: &Value, what: &str) -> Result<Vec<String>> {
    v.as_array()
        .filter(|a| a.len() <= MAX_LAYERS)
        .ok_or_else(|| bad("layerstate.edit", format!("`{what}` is a list of layer names")))?
        .iter()
        .map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or_else(|| bad("layerstate.edit", format!("`{what}` is a list of layer names"))))
        .collect()
}

pub(super) fn run_edit(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").ok_or_else(|| bad("layerstate.edit", "`name` is required"))?.to_string();
    let d = s.doc()?;
    let mut st = d.layer_states.iter().find(|x| x.name == name).cloned().ok_or_else(|| bad("layerstate.edit", "no such layer state"))?;
    if let Some(desc) = str_param(p, "description") {
        st.description = desc.chars().take(2048).collect();
    }
    if let Some(m) = mask_param(p)? {
        st.restore = m;
    }
    if let Some(o) = p.get("offOthers").and_then(Value::as_bool) {
        st.off_others = o;
    }
    if let Some(v) = p.get("layers") {
        let entries = v.as_array().filter(|a| a.len() <= MAX_LAYERS).ok_or_else(|| bad("layerstate.edit", "`layers` is a list of {name, …}"))?;
        for e in entries {
            let n = e.get("name").and_then(Value::as_str).ok_or_else(|| bad("layerstate.edit", "each entry of `layers` has a `name`"))?;
            let l = st
                .layers
                .iter_mut()
                .find(|l| l.name.eq_ignore_ascii_case(n.trim()))
                .ok_or_else(|| bad("layerstate.edit", format!("layer `{n}` is not in the state")))?;
            let mut changed = l.clone();
            super::apply(&mut changed, e)?;
            if let Some(lt) = e.get("linetype").and_then(Value::as_str) {
                if d.linetype(lt).is_none() {
                    return Err(bad("layerstate.edit", format!("linetype `{lt}` is not loaded")));
                }
                changed.linetype = lt.to_string();
            }
            *l = changed;
        }
    }
    if let Some(v) = p.get("addLayers") {
        for n in names(v, "addLayers")? {
            let l = d.layer(&n).cloned().ok_or_else(|| bad("layerstate.edit", format!("no layer `{n}`")))?;
            if !st.layers.iter().any(|x| x.name.eq_ignore_ascii_case(&l.name)) {
                st.layers.push(l);
            }
        }
    }
    if let Some(v) = p.get("removeLayers") {
        let gone = names(v, "removeLayers")?;
        st.layers.retain(|l| !gone.iter().any(|g| g.eq_ignore_ascii_case(&l.name)));
    }
    let out = state_json(&st);
    let d = s.doc_mut()?;
    if let Some(x) = d.layer_states.iter_mut().find(|x| x.name == name) {
        *x = st;
    }
    Ok(out)
}

pub(super) fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").ok_or_else(|| bad("layerstate.export", "`name` is required"))?;
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad("layerstate.export", "`path` is required"))?.to_string();
    let st = s.doc()?.layer_states.iter().find(|x| x.name == name).cloned().ok_or_else(|| bad("layerstate.export", "no such layer state"))?;
    let file = LasFile {
        format: FORMAT.into(),
        version: 1,
        name: st.name,
        description: st.description,
        off_others: st.off_others,
        restore: st.restore,
        layers: st.layers,
    };
    let text = serde_json::to_string_pretty(&file).map_err(|e| EngineError::Other(e.to_string()))?;
    write(&path, &text)?;
    Ok(json!({ "path": path }))
}

pub(super) fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad("layerstate.import", "`path` is required"))?;
    let text = read(path)?;
    let f: LasFile = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| EngineError::Other(format!("{path}: not a CADaddy layer state file ({e})")))?;
    if f.format != FORMAT || f.version != 1 {
        return Err(EngineError::Other(format!("{path}: not a CADaddy layer state file")));
    }
    if f.layers.len() > MAX_LAYERS {
        return Err(EngineError::Other(format!("{path}: too many layers")));
    }
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| f.name.trim().to_string(), str::to_string);
    if !super::valid_name(&name) || f.layers.iter().any(|l| !super::valid_name(&l.name)) {
        return Err(EngineError::Other(format!("{path}: a layer or state name is not valid")));
    }
    let replace = bool_or(p, "replace", false);
    let d = s.doc_mut()?;
    if d.layer_states.iter().any(|x| x.name == name) && !replace {
        return Err(EngineError::Other(format!("A layer state named \"{name}\" exists; import with replace or another name.")));
    }
    // Layers the drawing lacks are added; linetypes it doesn't have become Continuous.
    let mut layers = f.layers;
    for l in &mut layers {
        if d.linetype(&l.linetype).is_none() {
            l.linetype = "Continuous".into();
        }
        if d.layer(&l.name).is_none() {
            d.layers.push(l.clone());
        }
    }
    let st = LayerState {
        name: name.clone(),
        layers,
        description: f.description.chars().take(2048).collect(),
        restore: f.restore,
        off_others: f.off_others,
    };
    d.layer_states.retain(|x| x.name != name);
    d.layer_states.push(st);
    Ok(json!({ "name": name }))
}

fn write(path: &str, text: &str) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::write(path, text).map_err(|e| EngineError::Other(format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = text;
        Err(EngineError::Other(format!("{path}: files can't be written in the browser")))
    }
}

fn read(path: &str) -> Result<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let meta = std::fs::metadata(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        if meta.len() > MAX_FILE {
            return Err(EngineError::Other(format!("{path}: the file is too large")));
        }
        std::fs::read_to_string(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err(EngineError::Other(format!("{path}: files can't be read in the browser")))
    }
}
