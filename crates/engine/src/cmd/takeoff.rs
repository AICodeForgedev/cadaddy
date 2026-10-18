//! TAKEOFF: quantities by layer for estimating. Length totals the lines, arcs, circles,
//! polylines, ellipses and splines on each layer (conduit, cable tray, pipe); Area totals the
//! closed shapes and hatches (rooms, slabs). Each layer's row has the object count, the total in
//! drawing units and in feet or metres (square feet or square metres for area, from INSUNITS),
//! and the total with a waste factor added. The rows can go to a CSV file and/or a table.

use std::collections::BTreeMap;

use cadaddy_doc::{Entity, EntityKind, MAX_BLOCK_DEPTH, Space};
use cadaddy_geom::Mat3;
use serde_json::{Value, json};

use super::blocks::extract::{Cell, csv, insert_table, write_file};
use super::curves::Chain;
use super::qselect::wildcard;
use super::{CommandSpec, bad, f64_or, point_param, str_param, targets};
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Header variable: the waste percentage last used (the next TAKEOFF's default).
const WASTE_VAR: &str = "CADADDY_TAKEOFF_WASTE";
/// Rows a table can hold (the TABLE command's limit, less header and title).
const MAX_TABLE_ROWS: usize = 1998;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("takeoff", "Quantity Takeoff", run)
            .menu(&["Tools", "Inquiry", "Quantity Takeoff"])
            .alias(&["qto"])
            .params(
                "{mode?: \"length\" (default) | \"area\", handles? | layers?: \"E-*,P-*\" (wildcards; default: every object in the current space), waste?: percent added (default 0; the waste column is left out at 0), nested?: bool (default true: shapes inside block references, arrays and xrefs count, on the layer they are drawn on), file?: path (.csv; .txt tab-separated), at?: [x,y] (insert a table), title?} → {mode, unit, rows: [{layer, objects, drawing, real, withWaste}], total: {objects, drawing, real, withWaste}, columns, file?, table?}",
            )
            .noundo()
            .interactive(|_| Ok(Box::new(TakeoffM::default()))),
    ]
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Mode {
    #[default]
    Length,
    Area,
}

/// The real-world unit for a drawing's INSUNITS: its name and how many of it one drawing unit is
/// (squared for areas). `None`: unitless, totals stay in drawing units.
fn real_unit(d: &cadaddy_doc::Drawing, mode: Mode) -> Option<(&'static str, f64)> {
    let (name, f) = match d.header.i64("INSUNITS", 0) {
        1 => ("ft", 1.0 / 12.0),
        2 => ("ft", 1.0),
        4 => ("m", 0.001),
        5 => ("m", 0.01),
        6 => ("m", 1.0),
        _ => return None,
    };
    Some(match mode {
        Mode::Length => (name, f),
        Mode::Area => (if name == "ft" { "sq ft" } else { "sq m" }, f * f),
    })
}

/// How much a shape adds: its length, or the area it encloses (closed shapes only).
fn measure(k: &EntityKind, mode: Mode) -> Option<f64> {
    match mode {
        Mode::Length => match k {
            EntityKind::Line(_)
            | EntityKind::Arc(_)
            | EntityKind::Circle(_)
            | EntityKind::LwPolyline(_)
            | EntityKind::Polyline3d(_)
            | EntityKind::Ellipse(_)
            | EntityKind::Spline(_) => Chain::of(k).map(|c| c.len()),
            _ => None,
        },
        Mode::Area => {
            let closed = match k {
                EntityKind::LwPolyline(p) => p.closed,
                EntityKind::Circle(_) | EntityKind::Hatch(_) | EntityKind::Region(_) | EntityKind::Ellipse(_) => true,
                _ => false,
            };
            if !closed {
                return None;
            }
            super::inquiry::kind_area(k).ok().map(|(a, _)| a)
        }
    }
    .filter(|v| v.is_finite() && *v > 0.0)
}

/// Shapes measured at most (MINSERT grids and arrays of nested blocks multiply quickly).
const MAX_SHAPES: usize = 500_000;

/// What was measured: (layer, amount) per shape.
struct Tally<'a> {
    d: &'a cadaddy_doc::Drawing,
    mode: Mode,
    nested: bool,
    layers: &'a str,
    out: Vec<(String, f64)>,
}

impl Tally<'_> {
    fn on(&self, layer: &str) -> bool {
        self.layers.split(',').any(|w| wildcard(w.trim(), layer))
    }
    /// Add `k` (on `layer`, placed by `m`), or the shapes inside it when it is a block reference.
    fn add(&mut self, k: &EntityKind, layer: &str, m: Option<Mat3>, depth: usize) {
        if self.out.len() >= MAX_SHAPES || depth > MAX_BLOCK_DEPTH {
            return;
        }
        if let EntityKind::Insert(ins) = k {
            if !self.nested {
                return;
            }
            let Some(b) = self.d.block(&ins.block) else { return };
            let base = b.base.xy();
            for r in 0..ins.rows.clamp(1, 10_000) {
                for c in 0..ins.cols.clamp(1, 10_000) {
                    let off = cadaddy_geom::Vec2::new(ins.col_spacing * f64::from(c), ins.row_spacing * f64::from(r)).rotate(ins.rotation);
                    let local = Mat3::translate(off).then_before(ins.transform(base));
                    let mm = m.map_or(local, |o| o.then_before(local));
                    for be in b.entities.iter() {
                        // Layer 0 inside a block takes the reference's layer (as drawn).
                        let l = if be.common.layer == "0" { layer } else { be.common.layer.as_str() };
                        self.add(&be.kind, l, Some(mm), depth + 1);
                    }
                    if self.out.len() >= MAX_SHAPES {
                        return;
                    }
                }
            }
            return;
        }
        if !self.on(layer) {
            return;
        }
        let v = match m {
            None => measure(k, self.mode),
            Some(m) => {
                let mut t = k.clone();
                t.transform(&m);
                measure(&t, self.mode)
            }
        };
        if let Some(v) = v {
            self.out.push((layer.to_string(), v));
        }
    }
}

/// The shapes to measure: those of `handles`, else of the current space, on `layers` (all), with
/// the contents of block references unless `nested` is false.
fn shapes(s: &Session, p: &Value, mode: Mode) -> Result<Vec<(String, f64)>> {
    let d = s.doc()?;
    let given = p.get("handles").is_some() || p.get("handle").is_some();
    let ents: Vec<std::sync::Arc<Entity>> = if given {
        targets(s, p)?.iter().filter_map(|h| d.entity(*h).cloned()).collect()
    } else {
        let space = s.layout_space();
        d.space(&space).or_else(|| d.space(&Space::Model)).ok_or_else(|| bad("takeoff", "no such space"))?.iter().cloned().collect()
    };
    let layers = str_param(p, "layers").filter(|l| !l.trim().is_empty() && !given).unwrap_or("*");
    let mut t = Tally { d, mode, nested: super::bool_or(p, "nested", true), layers, out: Vec::new() };
    for e in &ents {
        t.add(&e.kind, &e.common.layer, None, 0);
    }
    Ok(t.out)
}

fn round(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let mode = match str_param(p, "mode").map(str::to_ascii_lowercase).as_deref() {
        None | Some("length" | "l") => Mode::Length,
        Some("area" | "a" | "ar") => Mode::Area,
        Some(_) => return Err(bad("takeoff", "mode must be length or area")),
    };
    let waste = f64_or(p, "waste", 0.0);
    if !(0.0..=1000.0).contains(&waste) {
        return Err(bad("takeoff", "waste must be a percentage from 0 to 1000"));
    }
    // Per layer (any case, first spelling kept): objects and total.
    let mut by_layer: BTreeMap<String, (String, u64, f64)> = BTreeMap::new();
    for (layer, v) in shapes(s, p, mode)? {
        let row = by_layer.entry(layer.to_ascii_uppercase()).or_insert_with(|| (layer.clone(), 0, 0.0));
        row.1 = row.1.saturating_add(1);
        row.2 += v;
    }
    let d = s.doc()?;
    let unit = real_unit(d, mode);
    let factor = unit.map_or(1.0, |(_, f)| f);
    let unit_name = unit.map_or("drawing units", |(n, _)| n);
    let grow = 1.0 + waste / 100.0;
    let mut columns: Vec<String> = vec!["Layer".into(), "Objects".into()];
    let what = if mode == Mode::Length { "Length" } else { "Area" };
    columns.push(format!("{what} (drawing units)"));
    columns.push(format!("{what} ({unit_name})"));
    let with_waste = waste > 0.0;
    if with_waste {
        columns.push(format!("With {waste}% waste ({unit_name})"));
    }
    let mut rows = Vec::new();
    let mut cells = Vec::new();
    let (mut n, mut total) = (0u64, 0.0);
    for (layer, count, sum) in by_layer.into_values() {
        n = n.saturating_add(count);
        total += sum;
        let real = sum * factor;
        rows.push(json!({ "layer": layer, "objects": count, "drawing": round(sum), "real": round(real), "withWaste": round(real * grow) }));
        let mut row = vec![Cell::Text(layer), Cell::Count(count), Cell::Num(sum), Cell::Num(real)];
        if with_waste {
            row.push(Cell::Num(real * grow));
        }
        cells.push(row);
    }
    let real = total * factor;
    let mut row = vec![Cell::Text("Total".into()), Cell::Count(n), Cell::Num(total), Cell::Num(real)];
    if with_waste {
        row.push(Cell::Num(real * grow));
    }
    cells.push(row);
    let mut message = format!("{what} takeoff, {n} object(s) on {} layer(s):", rows.len());
    for r in &cells {
        let t = |i: usize| r.get(i).map(Cell::text).unwrap_or_default();
        let extra = if with_waste { format!("  ({} with waste)", t(4)) } else { String::new() };
        message.push_str(&format!("\n  {:<24} {:>5}  {} {unit_name}{extra}", t(0), t(1), t(3)));
    }
    let mut out = json!({
        "mode": what.to_ascii_lowercase(),
        "unit": unit_name,
        "waste": waste,
        "rows": rows,
        "total": { "objects": n, "drawing": round(total), "real": round(real), "withWaste": round(real * grow) },
        "columns": columns,
        "message": message,
    });
    if let Some(path) = str_param(p, "file").filter(|f| !f.trim().is_empty()) {
        let tab = std::path::Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("txt"));
        write_file(path, &csv(&columns, &cells, if tab { '\t' } else { ',' }))?;
        out["file"] = json!(path);
    }
    if p.get("at").is_some_and(|v| !v.is_null()) {
        let at = point_param(p, "at").ok_or_else(|| bad("takeoff", "`at` must be a point [x, y]"))?;
        if cells.len() > MAX_TABLE_ROWS {
            return Err(bad("takeoff", format!("Too many layers for a table ({}).", cells.len())));
        }
        let title = str_param(p, "title").map_or_else(|| format!("{what} Takeoff"), str::to_string);
        out["table"] = json!(insert_table(s, at, Some(&title), &columns, &cells)?);
    }
    s.doc_mut()?.header.set_f64(WASTE_VAR, waste);
    Ok(out)
}

// ---------------- command line ----------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Stage {
    #[default]
    Mode,
    Select,
    Layers,
    Waste,
    Output,
    Point,
    File,
}

#[derive(Default)]
struct TakeoffM {
    stage: Stage,
    mode: Mode,
    handles: Vec<String>,
    layers: Option<String>,
    waste: f64,
}

impl TakeoffM {
    fn params(&self) -> Value {
        let mut p = json!({ "mode": if self.mode == Mode::Area { "area" } else { "length" }, "waste": self.waste });
        if !self.handles.is_empty() {
            p["handles"] = json!(self.handles);
        } else if let Some(l) = &self.layers {
            p["layers"] = json!(l);
        }
        p
    }
    fn finish(&mut self, s: &mut Session, extra: Value) -> Result<Step> {
        let mut p = self.params();
        if let (Value::Object(m), Value::Object(e)) = (&mut p, extra) {
            m.extend(e);
        }
        let r = run(s, &p)?;
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            m.lines().for_each(|l| s.echo(l.to_string()));
        }
        if let Some(f) = r.get("file").and_then(Value::as_str) {
            s.echo(format!("Wrote {f}"));
        }
        Ok(Step::Done)
    }
}

impl Interactive for TakeoffM {
    fn name(&self) -> &'static str {
        "TAKEOFF"
    }

    fn prompt(&self, s: &Session) -> Prompt {
        match self.stage {
            Stage::Mode => Prompt::new("Enter takeoff type", Accept::TEXT).kw(&["Length", "Area"]).default("Length"),
            Stage::Select => Prompt::new("Select objects or press Enter for all", Accept::SELECT).kw(&["Layers"]),
            Stage::Layers => Prompt::new("Enter layer names (wildcards, comma-separated)", Accept::TEXT).default("*"),
            Stage::Waste => {
                let w = s.doc().map(|d| d.header.f64(WASTE_VAR, 0.0)).unwrap_or(0.0);
                Prompt::new("Enter waste factor percent", Accept::NUMBER).default(format!("{w}"))
            }
            Stage::Output => Prompt::new("Output", Accept::TEXT).kw(&["Table", "File", "None"]).default("None"),
            Stage::Point => Prompt::new("Specify table insertion point", Accept::POINT),
            Stage::File => Prompt::new("Enter CSV file name", Accept::TEXT),
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        let word = |i: &Input| match i {
            Input::Keyword(k) | Input::Text(k) => Some(k.trim().to_ascii_lowercase()),
            _ => None,
        };
        match self.stage {
            Stage::Mode => {
                self.mode = match word(&i).as_deref() {
                    None | Some("" | "l" | "length") => Mode::Length,
                    Some("a" | "area") => Mode::Area,
                    Some(_) => {
                        s.echo("Enter Length or Area.");
                        return Ok(Step::Continue);
                    }
                };
                self.stage = Stage::Select;
            }
            Stage::Select => match i {
                Input::Pick(hs) => {
                    let mut have: std::collections::HashSet<String> = self.handles.iter().cloned().collect();
                    for h in hs {
                        let h = h.hex();
                        if have.insert(h.clone()) {
                            self.handles.push(h);
                        }
                    }
                }
                Input::Enter => self.stage = Stage::Waste,
                Input::Keyword(_) | Input::Text(_) if word(&i).is_some_and(|w| w == "layers" || w == "la") => self.stage = Stage::Layers,
                _ => s.echo("*Invalid selection*"),
            },
            Stage::Layers => {
                let l = word(&i).filter(|w| !w.is_empty()).map_or_else(
                    || "*".to_string(),
                    |_| match &i {
                        Input::Keyword(k) | Input::Text(k) => k.trim().to_string(),
                        _ => "*".into(),
                    },
                );
                self.layers = Some(l);
                self.handles.clear();
                self.stage = Stage::Waste;
            }
            Stage::Waste => {
                self.waste = match &i {
                    Input::Enter => s.doc()?.header.f64(WASTE_VAR, 0.0),
                    Input::Text(t) => match t.trim().trim_end_matches('%').parse::<f64>() {
                        Ok(w) if (0.0..=1000.0).contains(&w) => w,
                        _ => {
                            s.echo("Enter a percentage from 0 to 1000.");
                            return Ok(Step::Continue);
                        }
                    },
                    _ => return Ok(Step::Continue),
                };
                self.stage = Stage::Output;
            }
            Stage::Output => match word(&i).as_deref() {
                None | Some("" | "n" | "none") => return self.finish(s, json!({})),
                Some("t" | "table") => self.stage = Stage::Point,
                Some("f" | "file") => self.stage = Stage::File,
                Some(_) => s.echo("Enter Table, File or None."),
            },
            Stage::Point => {
                if let Input::Point(p) = i {
                    return self.finish(s, json!({ "at": [p.x, p.y] }));
                }
            }
            Stage::File => match &i {
                Input::Text(t) if !t.trim().is_empty() => {
                    let mut f = t.trim().trim_matches('"').to_string();
                    if std::path::Path::new(&f).extension().is_none() {
                        f.push_str(".csv");
                    }
                    return self.finish(s, json!({ "file": f }));
                }
                Input::Enter => return Ok(Step::Cancel),
                _ => {}
            },
        }
        Ok(Step::Continue)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn length_and_area_by_layer_with_waste_and_feet() {
        let mut s = Session::new();
        s.execute("setvar", &json!({"name": "INSUNITS", "value": 1})).unwrap();
        s.execute("layer.new", &json!({"name": "E-CONDUIT"})).unwrap();
        s.execute("layer.new", &json!({"name": "E-TRAY"})).unwrap();
        s.execute("layer.new", &json!({"name": "A-ROOM"})).unwrap();
        s.execute("line", &json!({"points": [[0, 0], [120, 0]]})).unwrap();
        s.execute("pline", &json!({"points": [[0, 10], [120, 10], [120, 130]]})).unwrap();
        let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
        s.execute("properties.set", &json!({"handles": hs, "layer": "E-CONDUIT"})).unwrap();
        let t = s.execute("line", &json!({"points": [[0, 200], [240, 200]]})).unwrap();
        let th = t["handles"][0].clone();
        s.execute("properties.set", &json!({"handles": [th], "layer": "E-TRAY"})).unwrap();
        let r = s.execute("takeoff", &json!({"layers": "E-*", "waste": 10})).unwrap();
        assert_eq!(r["unit"], "ft");
        assert_eq!(r["rows"][0], json!({"layer": "E-CONDUIT", "objects": 2, "drawing": 360.0, "real": 30.0, "withWaste": 33.0}));
        assert_eq!(r["rows"][1]["real"], 20.0);
        assert_eq!(r["total"]["withWaste"], 55.0);

        s.execute("rectang", &json!({"p1": [0, 300], "p2": [120, 396]})).unwrap();
        let rh = s.doc().unwrap().model.iter().last().unwrap().handle.hex();
        s.execute("properties.set", &json!({"handles": [rh], "layer": "A-ROOM"})).unwrap();
        let r = s.execute("takeoff", &json!({"mode": "area", "layers": "A-*"})).unwrap();
        assert_eq!((r["unit"].as_str(), r["total"]["real"].as_f64()), (Some("sq ft"), Some(80.0)));
    }

    /// Shapes inside block references count, scaled as placed and on the layer drawn (battle
    /// test Q1: typical-room blocks and arrays hold conduit).
    #[test]
    fn block_contents_count_with_their_scale() {
        let mut s = Session::new();
        s.execute("setvar", &json!({"name": "INSUNITS", "value": 1})).unwrap();
        s.execute("layer.new", &json!({"name": "E-CNDT"})).unwrap();
        s.execute("pline", &json!({"points": [[0, 0], [120, 0]]})).unwrap();
        let h = s.doc().unwrap().model.iter().last().unwrap().handle.hex();
        s.execute("properties.set", &json!({"handles": [h.clone()], "layer": "E-CNDT"})).unwrap();
        s.execute("block", &json!({"name": "CB", "base": [0, 0], "handles": [h], "keep": "delete"})).unwrap();
        s.execute("insert", &json!({"name": "CB", "at": [0, 0]})).unwrap();
        s.execute("insert", &json!({"name": "CB", "at": [0, 100], "scale": 2})).unwrap();
        let r = s.execute("takeoff", &json!({"layers": "E-*"})).unwrap();
        assert_eq!((r["total"]["objects"].as_u64(), r["total"]["real"].as_f64()), (Some(2), Some(30.0)), "{r}");
        assert_eq!(r["rows"][0]["layer"], "E-CNDT");
        let r = s.execute("takeoff", &json!({"nested": false})).unwrap();
        assert_eq!(r["total"]["objects"], 0);
        // No waste: no waste column.
        assert!(!r["columns"].as_array().unwrap().iter().any(|c| c.as_str().is_some_and(|c| c.contains("waste"))));
    }

    #[test]
    fn typed_takeoff_writes_a_csv() {
        let mut s = Session::new();
        s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
        let f = std::env::temp_dir().join(format!("cadaddy-takeoff-{}", std::process::id()));
        for i in ["takeoff", "", "", "5", "f", &f.to_string_lossy()] {
            s.cmdline(i).unwrap();
        }
        assert!(s.running.is_none(), "{}", s.prompt_text());
        let csv_path = f.with_extension("csv");
        let text = std::fs::read_to_string(&csv_path).unwrap();
        assert!(text.starts_with("Layer,Objects,") && text.contains("Total,1,10,0.8333,0.875"), "{text}");
        let _ = std::fs::remove_file(csv_path);
    }
}
