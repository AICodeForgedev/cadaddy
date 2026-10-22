//! Shapes: LOAD (make a compiled shape file's shapes available), SHAPE (place one), COMPILE
//! (a `.shp` source to a `.shx` file) and MKSHAPE (Express: a shape from drawn objects, added
//! to a `.shp` file, compiled and loaded).
//!
//! The drawing keeps the loaded files in `Drawing::shape_files` (saved as unnamed shape-file
//! STYLE records); a SHAPE names its shape and is drawn from the first loaded file that has it.

use std::sync::Arc;

use cadaddy_doc::{EntityKind, MAX_SHAPE_FILES, MAX_SHAPE_NAME, Shape};
use cadaddy_fonts::shp::{self, ShapeDef};
use cadaddy_fonts::shx::{self, ShxFont};
use cadaddy_geom::{Bounds2, Vec2};
use serde_json::{Value, json};

use super::super::helpers::{lwpoly, support_text};
use super::super::machines::{SelOutcome, SelectPhase};
use super::super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// Most objects MKSHAPE turns into one shape, and the finest resolution it takes.
const MAX_MKSHAPE_OBJECTS: usize = 10_000;
const MAX_RESOLUTION: f64 = 2048.0;
/// The largest number a shape in a plain shape file can have (subshape calls use one byte).
const MAX_SHAPE_NUMBER: u16 = 255;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("load", "Load Shape File", run_load)
            .files()
            .params("{file: path of a compiled shape file (.shx; added when the name has no extension)} → {file, shapes: [names]}")
            .interactive(|_| Ok(Box::new(PathM { name: "LOAD", message: "Enter name of shape file to load", run: run_load }))),
        CommandSpec::new("shape", "Shape", run_shape)
            .params(
                "{name, at: [x,y], size?: height (default 1), rotation?: degrees, widthFactor?, oblique?: degrees} → {handle} — \
                 a shape from a loaded shape file (LOAD); {list: true} → {shapes: [{name, file}]}",
            )
            .interactive(|_| Ok(Box::new(ShapeM::default()))),
        CommandSpec::new("compile", "Compile Shape File", run_compile)
            .params(
                "{file: path of a .shp source | text: its contents, output?: path of the .shx (default: the source's, as .shx)} → \
                 {output, shapes, warnings}",
            )
            .noundo()
            .interactive(|_| Ok(Box::new(PathM { name: "COMPILE", message: "Enter name of shape source file to compile", run: run_compile }))),
        CommandSpec::new("mkshape", "Make Shape", run_mkshape)
            .params(
                "{name, file: path of the .shp file (created, or the shape added to it; a shape of the same name is replaced), \
                 handles?, base: [x,y], resolution?: grid steps across the objects (default 128)} → {name, number, file, shx, size} \
                 — the shape is compiled and loaded; SHAPE at `size` draws it as large as the objects",
            )
            .interactive(|_| Ok(Box::new(MkShapeM::default()))),
    ]
}

/// `name` with `.ext` added when it has no extension.
fn with_ext(name: &str, ext: &str) -> String {
    let n = name.trim().trim_matches('"');
    let has = std::path::Path::new(n).extension().is_some();
    if has { n.to_string() } else { format!("{n}.{ext}") }
}

/// The path or file a command names (`file`, or `path` as the typed forms pass it).
fn file_param(p: &Value) -> Option<&str> {
    str_param(p, "file").or_else(|| str_param(p, "path")).map(str::trim).filter(|f| !f.is_empty())
}

/// The shape file `name` names, read again from disk.
fn read_shx(cmd: &str, name: &str) -> Result<Arc<ShxFont>> {
    shx::forget_cached();
    shx::find(name).ok_or_else(|| bad(cmd, format!("{name}: not found, or not a compiled shape file")))
}

/// Add `file` to the drawing's loaded shape files (once).
fn remember(s: &mut Session, file: &str) -> Result<()> {
    let d = s.doc_mut()?;
    if !d.shape_files.iter().any(|f| f.eq_ignore_ascii_case(file)) {
        if d.shape_files.len() >= MAX_SHAPE_FILES {
            return Err(EngineError::Other(format!("at most {MAX_SHAPE_FILES} shape files can be loaded")));
        }
        d.shape_files.push(file.to_string());
    }
    Ok(())
}

fn run_load(s: &mut Session, p: &Value) -> Result<Value> {
    let file = file_param(p).ok_or_else(|| bad("load", "`file` (a .shx shape file) is required"))?;
    let file = with_ext(file, "shx");
    let f = read_shx("load", &file)?;
    if f.is_empty() {
        return Err(bad("load", format!("{file} has no shapes")));
    }
    remember(s, &file)?;
    let names: Vec<String> = f.shape_names().into_iter().map(|(_, n)| n).collect();
    Ok(json!({ "file": file, "shapes": names, "message": format!("{file}: {} shape(s) loaded.", names.len()) }))
}

/// Every shape the drawing's loaded files have: (name, file), in file order.
fn loaded_shapes(s: &Session) -> Result<Vec<(String, String)>> {
    let d = s.doc()?;
    let mut out = Vec::new();
    for f in &d.shape_files {
        if let Some(font) = shx::find(f) {
            out.extend(font.shape_names().into_iter().map(|(_, n)| (n, f.clone())));
        }
    }
    Ok(out)
}

fn run_shape(s: &mut Session, p: &Value) -> Result<Value> {
    if bool_or(p, "list", false) {
        let v = loaded_shapes(s)?;
        return Ok(json!({ "shapes": v.iter().map(|(n, f)| json!({ "name": n, "file": f })).collect::<Vec<_>>() }));
    }
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("shape", "`name` is required"))?;
    if name.chars().count() > MAX_SHAPE_NAME {
        return Err(bad("shape", "the name is too long"));
    }
    let at = point_req("shape", p, "at")?;
    let size = size_param("shape", p, "size", false)?.unwrap_or(1.0);
    let rotation = f64_or(p, "rotation", 0.0).to_radians();
    let width_factor = size_param("shape", p, "widthFactor", false)?.unwrap_or(1.0);
    let oblique = f64_or(p, "oblique", 0.0);
    if oblique.abs() > 85.0 {
        return Err(bad("shape", "the oblique angle must be between -85 and 85 degrees"));
    }
    if cadaddy_render::shapes::find(s.doc()?, name).is_none() {
        return Err(bad("shape", format!("shape {name} is not in a loaded shape file (load one with LOAD)")));
    }
    let elevation = s.doc()?.header.f64("ELEVATION", 0.0);
    let sh = Shape { insert: at.to3(elevation), name: name.to_ascii_uppercase(), size, rotation, width_factor, oblique: oblique.to_radians() };
    let h = s.add_entity(EntityKind::Shape(sh))?;
    Ok(json!({ "handle": h.hex() }))
}

fn run_compile(s: &mut Session, p: &Value) -> Result<Value> {
    let _ = s;
    let mut q = p.clone();
    if let Some(f) = file_param(p)
        && q.get("text").is_none()
    {
        q["file"] = json!(with_ext(f, "shp"));
    }
    let (text, source) =
        support_text("compile", &q, "file", "text")?.ok_or_else(|| bad("compile", "`file` (a .shp source) or `text` is required"))?;
    let output = match str_param(p, "output").map(str::trim).filter(|o| !o.is_empty()) {
        Some(o) => with_ext(o, "shx"),
        None if q.get("text").is_none() => std::path::Path::new(&source).with_extension("shx").to_string_lossy().into_owned(),
        None => return Err(bad("compile", "`output` (where to write the .shx) is required with `text`")),
    };
    let c = shp::compile(&text).map_err(|e| bad("compile", format!("{source}: {e}")))?;
    crate::writefile::write(&output, &c.shx).map_err(|e| bad("compile", format!("{output}: {e}")))?;
    shx::forget_cached();
    let n = c.shapes.iter().filter(|d| d.number != 0).count();
    let mut message = format!("Compilation successful. Output file {output} contains {} bytes, {n} shape(s).", c.shx.len());
    for w in &c.warnings {
        message.push_str(&format!("\nNote: {w}"));
    }
    Ok(json!({ "output": output, "shapes": n, "bytes": c.shx.len(), "warnings": c.warnings, "message": message }))
}

/// Integer grid steps, each at most 127 across, from `d` (split into as many steps as needed).
fn steps(d: (i64, i64), out: &mut Vec<(i8, i8)>) {
    let (mut x, mut y) = d;
    while x != 0 || y != 0 {
        let sx = x.clamp(-127, 127);
        let sy = y.clamp(-127, 127);
        out.push((i8::try_from(sx).unwrap_or(0), i8::try_from(sy).unwrap_or(0)));
        x -= sx;
        y -= sy;
    }
}

/// A shape specification drawing `lines` (grid coordinates from the base point): for each, the
/// pen goes up to its start, then down along it, in displacement runs (code 9).
fn spec_of(lines: &[Vec<(i64, i64)>]) -> Vec<u8> {
    let mut spec = Vec::new();
    let mut pen = (0i64, 0i64);
    let run = |spec: &mut Vec<u8>, moves: &[(i8, i8)]| {
        if moves.is_empty() {
            return;
        }
        spec.push(9);
        for (x, y) in moves {
            spec.push(*x as u8);
            spec.push(*y as u8);
        }
        spec.extend([0, 0]);
    };
    for l in lines {
        let Some(&first) = l.first() else { continue };
        let mut moves = Vec::new();
        steps((first.0 - pen.0, first.1 - pen.1), &mut moves);
        if !moves.is_empty() {
            spec.push(2);
            run(&mut spec, &moves);
        }
        spec.push(1);
        let mut moves = Vec::new();
        let mut at = first;
        for &q in l.iter().skip(1) {
            steps((q.0 - at.0, q.1 - at.1), &mut moves);
            at = q;
        }
        run(&mut spec, &moves);
        pen = at;
    }
    spec.push(2);
    spec.push(0);
    spec
}

fn run_mkshape(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("mkshape", "`name` is required"))?;
    if name.len() > 31 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(bad("mkshape", "a shape name is up to 31 letters, digits, - or _"));
    }
    let name = name.to_ascii_uppercase();
    let file = with_ext(file_param(p).ok_or_else(|| bad("mkshape", "`file` (the .shp file) is required"))?, "shp");
    let base = point_req("mkshape", p, "base")?;
    let res = f64_or(p, "resolution", 128.0);
    if !(1.0..=MAX_RESOLUTION).contains(&res) {
        return Err(bad("mkshape", format!("`resolution` must be 1 to {MAX_RESOLUTION}")));
    }
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad("mkshape", "no objects selected"));
    }
    if hs.len() > MAX_MKSHAPE_OBJECTS {
        return Err(bad("mkshape", format!("at most {MAX_MKSHAPE_OBJECTS} objects")));
    }
    // The objects as drawn (curves as short lines), relative to the base point.
    let d = s.doc()?;
    let ents: Vec<_> = hs.iter().filter_map(|h| d.entity(*h)).collect();
    let mut bounds = Bounds2::EMPTY;
    for e in &ents {
        bounds = bounds.union(&cadaddy_doc::entity_bounds(d, e, 0));
    }
    let extent = bounds.width().max(bounds.height());
    if !(extent.is_finite() && extent > 1e-12) {
        return Err(bad("mkshape", "the objects have no size"));
    }
    let k = res / extent;
    let opts = cadaddy_render::Options { tolerance: extent / res / 2.0, text: true, ..Default::default() };
    let list = cadaddy_render::build_entities(d, ents.iter().map(|e| e.as_ref()), &opts);
    let grid = |q: Vec2| (((q.x - base.x) * k).round() as i64, ((q.y - base.y) * k).round() as i64);
    let mut lines: Vec<Vec<(i64, i64)>> = Vec::new();
    for pr in &list.prims {
        let pts: Vec<Vec<Vec2>> = match pr.kind {
            cadaddy_render::Kind::Polyline => vec![list.points(pr).to_vec()],
            cadaddy_render::Kind::Point => list.points(pr).first().map(|q| vec![vec![*q, *q]]).unwrap_or_default(),
            k if k.is_fill() => list.fill_outlines(pr),
            _ => Vec::new(),
        };
        for l in pts {
            let mut g: Vec<(i64, i64)> = Vec::new();
            for q in l.iter().filter(|q| q.is_finite()) {
                let c = grid(*q);
                if g.last() != Some(&c) {
                    g.push(c);
                }
            }
            if !g.is_empty() {
                lines.push(g);
            }
        }
    }
    if lines.is_empty() {
        return Err(bad("mkshape", "the objects draw nothing a shape can hold"));
    }
    let spec = spec_of(&lines);
    if spec.len() > 2000 {
        return Err(bad("mkshape", format!("the shape needs {} bytes, more than the 2000 one shape holds: use a lower resolution", spec.len())));
    }
    // Add to (or replace in) the source file, then compile it next to it and load it.
    let mut defs = match std::fs::read_to_string(&file) {
        Ok(t) => shp::read(&t).map_err(|e| bad("mkshape", format!("{file}: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(bad("mkshape", format!("{file}: {e}"))),
    };
    let number = match defs.iter().find(|d| d.name == name) {
        Some(d) => d.number,
        None => {
            let n = defs.iter().map(|d| d.number).max().unwrap_or(0).saturating_add(1);
            if n > MAX_SHAPE_NUMBER {
                return Err(bad("mkshape", format!("{file} already has {MAX_SHAPE_NUMBER} shapes")));
            }
            n
        }
    };
    defs.retain(|d| d.name != name);
    defs.push(ShapeDef { number, name: name.clone(), spec });
    defs.sort_by_key(|d| d.number);
    let text = shp::source(&defs);
    let c = shp::compile(&text).map_err(|e| bad("mkshape", e))?;
    let shx_path = std::path::Path::new(&file).with_extension("shx").to_string_lossy().into_owned();
    crate::writefile::write(&file, text.as_bytes()).map_err(|e| bad("mkshape", format!("{file}: {e}")))?;
    crate::writefile::write(&shx_path, &c.shx).map_err(|e| bad("mkshape", format!("{shx_path}: {e}")))?;
    read_shx("mkshape", &shx_path)?;
    remember(s, &shx_path)?;
    let size = 1.0 / k;
    Ok(json!({
        "name": name, "number": number, "file": file, "shx": shx_path, "size": size,
        "message": format!("Shape {name} ({number}) written to {file}, compiled to {shx_path} and loaded. Insert it with SHAPE at size {size:.4} to match the objects.")
    }))
}

/// LOAD and COMPILE typed: a file name.
struct PathM {
    name: &'static str,
    message: &'static str,
    run: fn(&mut Session, &Value) -> Result<Value>,
}

impl Interactive for PathM {
    fn name(&self) -> &'static str {
        self.name
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new(self.message, Accept::TEXT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Text(t) | Input::Keyword(t) if !t.trim().trim_matches('"').is_empty() => {
                let r = (self.run)(s, &json!({ "file": t.trim().trim_matches('"') }))?;
                super::echo_message(s, &r);
                Ok(Step::Done)
            }
            Input::Point(_) | Input::Point3(_) | Input::Pick(_) | Input::Deferred(_) => Ok(Step::Continue),
            _ => Ok(Step::Cancel),
        }
    }
}

/// SHAPE typed: "Enter shape name or [?]:", "Specify insertion point:", "Specify height <1>:",
/// "Specify rotation angle <0>:".
#[derive(Default)]
struct ShapeM {
    name: Option<String>,
    at: Option<Vec2>,
    size: Option<f64>,
}

impl Interactive for ShapeM {
    fn name(&self) -> &'static str {
        "SHAPE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (&self.name, self.at, self.size) {
            (None, ..) => Prompt::new("Enter shape name or [?]", super::NAME),
            (Some(_), None, _) => Prompt::new("Specify insertion point", Accept::POINT),
            (Some(_), Some(at), None) => Prompt::new("Specify height", Accept::POINT_OR_NUMBER).base(at).default("1.0000"),
            (Some(_), Some(at), Some(_)) => Prompt::new("Specify rotation angle", Accept::POINT_OR_NUMBER).base(at).default("0"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let number = |t: &str| t.trim().parse::<f64>().ok().filter(|v| v.is_finite());
        match (&self.name, self.at, self.size, i) {
            (_, _, _, Input::Cancel) => Ok(Step::Cancel),
            (None, _, _, Input::Text(t) | Input::Keyword(t)) if t.trim() == "?" => {
                let v = loaded_shapes(s)?;
                if v.is_empty() {
                    s.echo("No shape files loaded.");
                }
                for (n, f) in v {
                    s.echo(format!("{n}  ({f})"));
                }
                Ok(Step::Continue)
            }
            (None, _, _, Input::Text(t) | Input::Keyword(t)) if !t.trim().is_empty() => {
                let n = t.trim().to_string();
                if cadaddy_render::shapes::find(s.doc()?, &n).is_none() {
                    s.echo(format!("Shape {} is not in a loaded shape file.", n.to_ascii_uppercase()));
                    return Ok(Step::Continue);
                }
                self.name = Some(n);
                Ok(Step::Continue)
            }
            (None, _, _, _) => Ok(Step::Cancel),
            (Some(_), None, _, Input::Point(q)) => {
                self.at = Some(q);
                Ok(Step::Continue)
            }
            (Some(_), Some(at), None, inp) => {
                let v = match inp {
                    Input::Enter => Some(1.0),
                    Input::Point(q) => Some(q.dist(at)),
                    Input::Text(t) => number(&t),
                    _ => None,
                };
                if let Some(v) = v.filter(|v| *v > 0.0) {
                    self.size = Some(v);
                }
                Ok(Step::Continue)
            }
            (Some(n), Some(at), Some(size), inp) => {
                let deg = match inp {
                    Input::Enter => Some(0.0),
                    Input::Point(q) => Some(at.angle_to(q).to_degrees()),
                    Input::Text(t) => number(&t),
                    _ => None,
                };
                let Some(deg) = deg else { return Ok(Step::Continue) };
                let n = n.clone();
                run_shape(s, &json!({ "name": n, "at": [at.x, at.y], "size": size, "rotation": deg }))?;
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
    fn preview(&self, s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        let (Some(n), Some(at)) = (&self.name, self.at) else { return Vec::new() };
        let (size, rot) = match self.size {
            None => (cursor.dist(at).max(1e-9), 0.0),
            Some(z) => (z, at.angle_to(cursor)),
        };
        let Ok(d) = s.doc() else { return Vec::new() };
        let sh = Shape { insert: at.to3(0.0), name: n.clone(), size, rotation: rot, width_factor: 1.0, oblique: 0.0 };
        cadaddy_render::shapes::strokes(d, &sh)
            .into_iter()
            .map(|l| lwpoly(l.into_iter().map(cadaddy_geom::PolyVertex::new).collect(), false))
            .collect()
    }
}

/// MKSHAPE typed: "Enter the name of the shape:", "Enter the name of the shape file:",
/// "Enter resolution <128>:", "Specify insertion base point:", then the objects.
#[derive(Default)]
struct MkShapeM {
    name: Option<String>,
    file: Option<String>,
    res: Option<f64>,
    base: Option<Vec2>,
    sel: SelectPhase,
}

impl Interactive for MkShapeM {
    fn name(&self) -> &'static str {
        "MKSHAPE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (&self.name, &self.file, self.res, self.base) {
            (None, ..) => Prompt::new("Enter the name of the shape", super::NAME),
            (Some(_), None, ..) => Prompt::new("Enter the name of the shape file", Accept::TEXT),
            (Some(_), Some(_), None, _) => Prompt::new("Enter resolution", Accept::NUMBER).default("128"),
            (Some(_), Some(_), Some(_), None) => Prompt::new("Specify insertion base point", Accept::POINT),
            _ => self.sel.prompt(),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.name.is_none() {
            return Ok(match i {
                Input::Text(t) | Input::Keyword(t) if !t.trim().is_empty() => {
                    self.name = Some(t.trim().to_string());
                    Step::Continue
                }
                Input::Enter => Step::Cancel,
                _ => Step::Continue,
            });
        }
        if self.file.is_none() {
            return Ok(match i {
                Input::Text(t) | Input::Keyword(t) if !t.trim().trim_matches('"').is_empty() => {
                    self.file = Some(t.trim().trim_matches('"').to_string());
                    Step::Continue
                }
                Input::Enter => Step::Cancel,
                _ => Step::Continue,
            });
        }
        if self.res.is_none() {
            match i {
                Input::Enter => self.res = Some(128.0),
                Input::Text(t) => match t.trim().parse::<f64>() {
                    Ok(v) if (1.0..=MAX_RESOLUTION).contains(&v) => self.res = Some(v),
                    _ => s.echo(format!("Enter a resolution from 1 to {MAX_RESOLUTION}.")),
                },
                _ => {}
            }
            return Ok(Step::Continue);
        }
        if self.base.is_none() {
            if let Input::Point(q) = i {
                self.base = Some(q);
                self.sel = SelectPhase::begin(s);
                if self.sel.done {
                    let hs = self.sel.picked.clone();
                    return self.finish(s, &hs);
                }
            }
            return Ok(Step::Continue);
        }
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Empty => Ok(Step::Cancel),
            SelOutcome::Done(hs) => self.finish(s, &hs),
        }
    }
}

impl MkShapeM {
    fn finish(&self, s: &mut Session, hs: &[cadaddy_doc::Handle]) -> Result<Step> {
        let (Some(name), Some(file), Some(res), Some(base)) = (&self.name, &self.file, self.res, self.base) else { return Ok(Step::Cancel) };
        let r = run_mkshape(
            s,
            &json!({ "name": name, "file": file, "resolution": res, "base": [base.x, base.y], "handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>() }),
        )?;
        super::echo_message(s, &r);
        Ok(Step::Done)
    }
}
