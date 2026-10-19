//! LIGHTBAKE: bake the light that bounces round the model (sky through openings, sun and lights
//! off walls and floors) onto its surfaces, so the Realistic style shows it at full frame rate
//! (`cadaddy_render::lightmap`). A rebake redoes only the objects that changed and those near
//! them; new light (the sun's date and time, the lights) bakes everything again.
//!
//! The lightmap is kept with the open drawing and, for a saved drawing, beside its file:
//! `<drawing>.lightmap.png` (RGBM) and `<drawing>.lightmap.json` (each object's cells and hash).
//! Opening the drawing again finds it there.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use cadaddy_doc::{Handle, Space};
use cadaddy_render::lightmap::{self, BakeOptions, Baked, Cell, Lightmap, ObjectCells};
use cadaddy_render::shade::Scene;
use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("lightbake", "Bake Lighting", run)
            .menu(&["View", "Render", "Bake Lighting"])
            .params("{quality?: draft|medium|high (32/128/512 paths a texel side, 2/3/4 bounces; default medium), samples?, bounces?, texel? (drawing units; default 15 cm), full?: bool (bake everything again), clear?: true (drop the lightmap)} → {objects, baked, kept, texels, seconds, width, height, saved?}")
            .noundo()
            .interactive(|_| Ok(Box::new(BakeM))),
    ]
}

/// Each model-space object's [`lightmap::object_hash`].
pub fn hashes(d: &cadaddy_doc::Drawing) -> HashMap<Handle, u64> {
    d.model.iter().map(|e| (e.handle, lightmap::object_hash(d, e))).collect()
}

/// `<drawing>.lightmap.png` and `.json` beside the drawing's file.
fn sidecar(path: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let p = std::path::Path::new(path);
    (p.with_extension("lightmap.png"), p.with_extension("lightmap.json"))
}

/// The index (cells and hashes) as JSON.
pub fn index_json(lm: &Lightmap) -> Value {
    let objects: serde_json::Map<String, Value> = lm
        .objects
        .iter()
        .map(|(h, oc)| {
            (h.hex(), json!({ "hash": format!("{:016x}", oc.hash), "cells": oc.cells.iter().map(|c| [c.x, c.y, c.n]).collect::<Vec<_>>() }))
        })
        .collect();
    json!({ "version": 1, "width": lm.width, "height": lm.height, "range": lightmap::RANGE, "lighting": format!("{:016x}", lm.lighting), "objects": objects })
}

/// A lightmap from its PNG and index; `None` when they don't make one.
pub fn read_lightmap(png: &[u8], index: &str) -> Option<Lightmap> {
    let v: Value = serde_json::from_str(index).ok()?;
    let (width, height, texels) = lightmap::from_png(png)?;
    if v.get("width")?.as_u64()? != u64::from(width) || v.get("height")?.as_u64()? != u64::from(height) {
        return None;
    }
    let hex = |s: &Value| u64::from_str_radix(s.as_str()?, 16).ok();
    let mut objects = BTreeMap::new();
    for (h, o) in v.get("objects")?.as_object()?.iter().take(10_000_000) {
        let cells = o
            .get("cells")?
            .as_array()?
            .iter()
            .map(|c| {
                let a = c.as_array()?;
                let g = |k: usize| a.get(k)?.as_u64().and_then(|x| u32::try_from(x).ok());
                let cell = Cell { x: g(0)?, y: g(1)?, n: g(2)? };
                // Inside the atlas with its border.
                (cell.x >= 1 && cell.y >= 1 && cell.n <= lightmap::MAX_CELL && cell.x + cell.n < width && cell.y + cell.n < height).then_some(cell)
            })
            .collect::<Option<Vec<Cell>>>()?;
        objects.insert(Handle::parse_hex(h)?, ObjectCells { hash: hex(o.get("hash")?)?, cells });
    }
    Some(Lightmap { width, height, texels, objects, lighting: hex(v.get("lighting")?)? })
}

/// The names the baked lightmap is kept under in the drawing ([`cadaddy_doc::Drawing::embedded`]).
pub const EMBEDDED: (&str, &str) = ("lightmap.png", "lightmap.json");

/// The drawing's lightmap: baked this session, carried in the drawing (a .cdy file), or found
/// beside its file (looked for once).
pub fn current(s: &mut Session) -> Option<Arc<Lightmap>> {
    let st = s.state_mut().ok()?;
    if st.lightmap.is_none() && !st.lightmap_tried {
        st.lightmap_tried = true;
        let d = &st.doc;
        if let (Some(p), Some(i)) = (d.embedded.get(EMBEDDED.0), d.embedded.get(EMBEDDED.1))
            && let Some(lm) = std::str::from_utf8(i).ok().and_then(|i| read_lightmap(p, i))
        {
            st.lightmap = Some(Arc::new(lm));
            st.lightmap_rev += 1;
            return st.lightmap.clone();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = st.path.clone() {
            let (png, idx) = sidecar(&path);
            if let (Ok(p), Ok(i)) = (crate::readfile::read(&png), std::fs::read_to_string(&idx))
                && let Some(lm) = read_lightmap(&p, &i)
            {
                st.lightmap = Some(Arc::new(lm));
                st.lightmap_rev += 1;
            }
        }
    }
    st.lightmap.clone()
}

/// The baked light for a scene of the drawing's model space (objects changed since the bake show
/// without it).
pub fn baked_for(s: &mut Session, scene: &Scene) -> Option<Baked> {
    let lm = current(s)?;
    let h = hashes(s.doc().ok()?);
    Some(Baked::of(scene, lm, &h))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "lightbake";
    if p.get("clear").and_then(Value::as_bool) == Some(true) {
        let st = s.state_mut()?;
        let had = st.lightmap.take().is_some();
        st.lightmap_tried = true;
        st.lightmap_rev += 1;
        if s.doc()?.embedded.contains_key(EMBEDDED.0) || s.doc()?.embedded.contains_key(EMBEDDED.1) {
            let d = s.doc_mut()?;
            d.embedded.remove(EMBEDDED.0);
            d.embedded.remove(EMBEDDED.1);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = s.state()?.path.clone() {
            let (png, idx) = sidecar(&path);
            let _ = std::fs::remove_file(png);
            let _ = std::fs::remove_file(idx);
        }
        return Ok(json!({ "cleared": had }));
    }
    let (samples, bounces) = match str_param(p, "quality").map(str::to_ascii_lowercase).as_deref() {
        None | Some("medium" | "m") => (128, 3),
        Some("draft" | "d") => (32, 2),
        Some("high" | "h") => (512, 4),
        Some(_) => return Err(bad(cmd, "`quality` must be draft, medium or high")),
    };
    let int = |k: &str, lo: u64, hi: u64| -> Result<Option<u32>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .filter(|n| (lo..=hi).contains(n))
                .and_then(|n| u32::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| bad(cmd, format!("`{k}` must be from {lo} to {hi}"))),
        }
    };
    let samples = int("samples", 1, 4096)?.unwrap_or(samples);
    let bounces = int("bounces", 1, 16)?.unwrap_or(bounces);
    let d = s.doc()?;
    let lit = cadaddy_render::light::Lighting::of(d, &Space::Model);
    // Curves tessellated to 5 mm.
    let scene = cadaddy_render::shade::scene(d, &Space::Model, 0.005 / lit.unit_m.max(1e-12), cadaddy_doc::color::Rgb(0, 0, 0));
    let texel = match p.get("texel") {
        None | Some(Value::Null) => 0.15 / lit.unit_m.max(1e-12),
        Some(v) => v.as_f64().filter(|t| t.is_finite() && *t > 0.0).ok_or_else(|| bad(cmd, "`texel` must be a positive size in drawing units"))?,
    };
    if scene.tris.is_empty() {
        return Err(bad(cmd, "model space has no surfaces to bake (3D faces, meshes, solids)"));
    }
    let h = hashes(d);
    let full = p.get("full").and_then(Value::as_bool) == Some(true);
    let previous = if full { None } else { current(s) };
    let (lm, r) = lightmap::bake(&scene, &lit, &h, previous.as_deref(), &BakeOptions { samples, bounces, texel, seed: 1 });
    let mut out = json!({
        "objects": r.objects, "baked": r.baked, "kept": r.kept, "texels": r.texels, "seconds": r.seconds,
        "width": lm.width, "height": lm.height,
    });
    let bytes = lightmap::to_png(&lm).ok_or_else(|| bad(cmd, "the lightmap could not be encoded"))?;
    let index = index_json(&lm).to_string();
    // Carried in the drawing (a .cdy file opens lit, anywhere)…
    let d = s.doc_mut()?;
    d.embedded.insert(EMBEDDED.0.into(), Arc::new(bytes.clone()));
    d.embedded.insert(EMBEDDED.1.into(), Arc::new(index.clone().into_bytes()));
    // …and beside its file, for a drawing saved as DXF or DWG.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = s.state()?.path.clone() {
        let (png, idx) = sidecar(&path);
        std::fs::write(&png, bytes).map_err(|e| bad(cmd, format!("{}: {e}", png.display())))?;
        std::fs::write(&idx, index).map_err(|e| bad(cmd, format!("{}: {e}", idx.display())))?;
        if let Some(o) = out.as_object_mut() {
            o.insert("saved".into(), json!(png.display().to_string()));
        }
    }
    s.echo(format!("Baked {} of {} objects ({} kept), {} texels in {:.1} s.", r.baked, r.objects, r.kept, r.texels, r.seconds));
    let st = s.state_mut()?;
    st.lightmap = Some(Arc::new(lm));
    st.lightmap_tried = true;
    st.lightmap_rev += 1;
    s.touch();
    Ok(out)
}

struct BakeM;

impl Interactive for BakeM {
    fn name(&self) -> &'static str {
        "LIGHTBAKE"
    }
    fn prompt(&self, _: &Session) -> Prompt {
        Prompt::new("Enter bake quality", Accept::TEXT).kw(&["Draft", "Medium", "High", "Clear"]).default("Medium")
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let t = match i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Text(t) | Input::Keyword(t) => t.trim().to_ascii_lowercase(),
            _ => String::new(),
        };
        let p = match t.chars().next() {
            None | Some('m') => json!({ "quality": "medium" }),
            Some('d') => json!({ "quality": "draft" }),
            Some('h') => json!({ "quality": "high" }),
            Some('c') => json!({ "clear": true }),
            _ => {
                s.echo("Invalid option keyword.");
                return Ok(Step::Continue);
            }
        };
        if let Err(e) = run(s, &p) {
            s.echo(e.to_string());
        }
        Ok(Step::Done)
    }
}
