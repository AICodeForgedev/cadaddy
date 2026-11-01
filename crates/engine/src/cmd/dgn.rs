//! MicroStation DGN (V7): DGNIMPORT brings a design file's objects into the drawing, DGNEXPORT
//! writes the drawing's model space as one, DGNATTACH shows one as an underlay (drawn, not
//! converted) that DGNCLIP clips, DGNADJUST fades and DGNLAYERS shows by level; snapping to it
//! follows DGNOSNAP. Files go through the host's io hooks (`cadaddy_io::dgn`).

use cadaddy_doc::{EntityKind, Underlay};
use cadaddy_geom::Vec3;
use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("dgnimport", "DGN Import...", run_import)
            .menu(&["Insert", "DGN Import..."])
            .params("{path: a DGN V7 file} → {added, layers, notes}: its objects, levels as layers, cells as blocks")
            .interactive(|_| Ok(Box::new(PathM { name: "DGNIMPORT", ask: "Enter path of DGN file to import", run: run_import }))),
        CommandSpec::new("dgnexport", "DGN Export...", run_export)
            .menu(&["File", "Export", "DGN..."])
            .enabled(always)
            .params("{path: .dgn} → {path, bytes, notes}: model space as a 2D DGN V7 file, layers as levels")
            .interactive(|_| Ok(Box::new(PathM { name: "DGNEXPORT", ask: "Enter path of DGN file to write", run: run_export })))
            .noundo(),
        CommandSpec::new("dgnattach", "DGN Underlay...", run_attach)
            .menu(&["Insert", "DGN Underlay..."])
            .params("{path, at?: [x,y] (default: where the design's own coordinates put it), scale?: 1, rotation?: degrees, name?} → {handle, size, levels}")
            .interactive(|_| Ok(Box::new(PathM { name: "DGNATTACH", ask: "Enter path of DGN file to attach", run: run_attach }))),
        CommandSpec::new("dgnclip", "Clip DGN Underlay", |s, p| super::underlay::clip(s, p, "dgnclip"))
            .params("{handle?, rect?: [[x1,y1],[x2,y2]] | points?: [[x,y]…], off?, on?, delete?} (default: the selected underlay)"),
        CommandSpec::new("dgnadjust", "Adjust DGN Underlay", |s, p| super::underlay::adjust(s, p, "dgnadjust")).params("{handles?, fade?: 0-80, contrast?: 20-100, monochrome?: bool}"),
        CommandSpec::new("dgnlayers", "DGN Underlay Levels", |s, p| super::underlay::layers(s, p, "dgnlayers")).params("{handle?, off?: [level], on?: [level]} → {layers: [{name, on}]}: the design's levels"),
    ]
}

fn path_param(cmd: &str, p: &Value) -> Result<String> {
    str_param(p, "path").map(|t| t.trim().trim_matches('"').to_string()).filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "`path` is required"))
}

fn read(cmd: &str, path: &str) -> Result<cadaddy_doc::Drawing> {
    let hooks = super::file::io().ok_or_else(|| bad(cmd, "file formats are not available in this build"))?;
    #[cfg(not(target_arch = "wasm32"))]
    let bytes = crate::readfile::read(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    #[cfg(target_arch = "wasm32")]
    let bytes: Vec<u8> = return Err(bad(cmd, format!("{path}: paths are not available on the web")));
    (hooks.read)(&bytes, path).map_err(|e| bad(cmd, format!("{path}: {e}")))
}

/// DGNIMPORT: the design file's objects added to the drawing as they are (its cells become
/// blocks, its levels layers).
fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let path = path_param("dgnimport", p)?;
    if !path.to_ascii_lowercase().ends_with(".dgn") {
        return Err(bad("dgnimport", "give a .dgn file"));
    }
    let src = read("dgnimport", &path)?;
    let layers: Vec<String> = src.layers.iter().filter(|l| l.name.starts_with("Level ")).map(|l| l.name.clone()).collect();
    let notes = src.read_notes.clone();
    let stem = path.rsplit(['/', '\\']).next().unwrap_or("dgn").rsplit_once('.').map_or("dgn", |(a, _)| a).to_string();
    let added = super::blocks::import_drawing(s, &src, &format!("DGN-{stem}"))?;
    Ok(json!({ "added": added.len(), "layers": layers, "notes": notes }))
}

/// What DGNEXPORT can't write as such, counted by kind (the writer's own list).
fn not_written(d: &cadaddy_doc::Drawing) -> Vec<String> {
    let mut counts: std::collections::BTreeMap<&'static str, usize> = Default::default();
    for e in d.model.iter() {
        let as_such = matches!(
            e.kind,
            EntityKind::Line(_)
                | EntityKind::Point(_)
                | EntityKind::LwPolyline(_)
                | EntityKind::Circle(_)
                | EntityKind::Arc(_)
                | EntityKind::Ellipse(_)
                | EntityKind::Text(_)
                | EntityKind::Insert(_)
        );
        if !as_such {
            *counts.entry(e.kind.type_name()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .map(|(k, n)| match k {
            "Spline" => format!("{n} splines written as line strings through the curve"),
            "Hatch" => format!("{n} hatches written as their boundaries"),
            _ => format!("{n} {k} objects not written (DGN V7 has no equivalent here)"),
        })
        .collect()
}

/// DGNEXPORT: model space as a 2D DGN V7 file.
fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = path_param("dgnexport", p)?;
    if !path.to_ascii_lowercase().ends_with(".dgn") {
        return Err(bad("dgnexport", "the file must end in .dgn"));
    }
    let hooks = super::file::io().ok_or_else(|| bad("dgnexport", "file formats are not available in this build"))?;
    let d = s.doc()?;
    let bytes = (hooks.write)(d, &path).map_err(|e| bad("dgnexport", e))?;
    let notes = not_written(d);
    #[cfg(not(target_arch = "wasm32"))]
    crate::writefile::write(&path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
    Ok(json!({ "path": path, "bytes": bytes.len(), "notes": notes }))
}

/// DGNATTACH: a design file as an underlay, at its own coordinates unless `at` says where.
fn run_attach(s: &mut Session, p: &Value) -> Result<Value> {
    let path = path_param("dgnattach", p)?;
    if !path.to_ascii_lowercase().ends_with(".dgn") {
        return Err(bad("dgnattach", "give a .dgn file"));
    }
    let scale = p.get("scale").and_then(Value::as_f64).unwrap_or(1.0);
    if !(scale.is_finite() && scale > 0.0 && scale < 1e9) {
        return Err(bad("dgnattach", "`scale` must be a positive number"));
    }
    let rot = p.get("rotation").and_then(Value::as_f64).unwrap_or(0.0).to_radians();
    let (d, min, size) = super::underlay::read_dgn(&path)?;
    let at = point_param(p, "at").unwrap_or(min);
    if !at.is_finite() || !rot.is_finite() {
        return Err(bad("dgnattach", "`at` and `rotation` must be finite"));
    }
    let warnings = d.read_notes.clone();
    for w in &warnings {
        s.echo(w.clone());
    }
    let levels: Vec<String> = d.layers.iter().filter(|l| l.name.starts_with("Level ")).map(|l| l.name.clone()).collect();
    super::underlay::cache_page(&path, 1, d);
    let (sn, cs) = rot.sin_cos();
    let u = Underlay {
        insert: at.to3(0.0),
        u: Vec3::new(cs * scale, sn * scale, 0.0),
        v: Vec3::new(-sn * scale, cs * scale, 0.0),
        size,
        path: path.clone(),
        page: 1,
        name: str_param(p, "name").unwrap_or("").to_string(),
        clip: Vec::new(),
        clipping: false,
        fade: 25,
        contrast: 20,
        monochrome: false,
        layers_off: Vec::new(),
    };
    let h = s.add_entity(EntityKind::Underlay(Box::new(u)))?;
    Ok(json!({ "handle": h.hex(), "size": [size.x, size.y], "levels": levels, "warnings": warnings }))
}

/// The DGN commands at the command line: the file's path, then the command runs as given.
struct PathM {
    name: &'static str,
    ask: &'static str,
    run: fn(&mut Session, &Value) -> Result<Value>,
}

impl Interactive for PathM {
    fn name(&self) -> &'static str {
        self.name
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new(self.ask, Accept::TEXT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Cancel | Input::Enter => Ok(Step::Cancel),
            Input::Text(t) | Input::Keyword(t) => {
                let r = (self.run)(s, &json!({ "path": t.trim().trim_matches('"') }))?;
                if let Some(notes) = r.get("notes").and_then(Value::as_array) {
                    for n in notes.iter().filter_map(Value::as_str) {
                        s.echo(n);
                    }
                }
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
}
