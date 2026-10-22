//! MVSETUP: setting up a sheet. In a layout: `title` inserts a border and title block for a
//! standard sheet size, `create` lays out a grid of viewports, `scale` sets viewports' scale, and
//! `align` pans a viewport so a point in it lines up with a point seen in another. On the Model tab
//! (TILEMODE 1) `model` draws a border the size of the sheet at the drawing scale and sets the
//! limits to it.

use std::sync::Arc;

use cadaddy_doc::{Block, Entity, EntityKind, HAlign, Handle, LwPolyline, Space, Text, VAlign, Viewport};
use cadaddy_geom::{Bounds2, PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

/// The most viewports `create` lays out (rows × columns).
const MAX_GRID: usize = 8;

/// Standard sheets: name, width × height (landscape), and whether the size is in millimetres
/// (else inches).
const SHEETS: [(&str, f64, f64, bool); 14] = [
    ("ANSI-A", 11.0, 8.5, false),
    ("ANSI-B", 17.0, 11.0, false),
    ("ANSI-C", 22.0, 17.0, false),
    ("ANSI-D", 34.0, 22.0, false),
    ("ANSI-E", 44.0, 34.0, false),
    ("ARCH-C", 24.0, 18.0, false),
    ("ARCH-D", 36.0, 24.0, false),
    ("ARCH-E1", 42.0, 30.0, false),
    ("ARCH-E", 48.0, 36.0, false),
    ("ISO-A4", 297.0, 210.0, true),
    ("ISO-A3", 420.0, 297.0, true),
    ("ISO-A2", 594.0, 420.0, true),
    ("ISO-A1", 841.0, 594.0, true),
    ("ISO-A0", 1189.0, 841.0, true),
];

pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    match str_param(p, "option").map(|o| o.trim().to_ascii_lowercase()).as_deref() {
        Some("title" | "t") => title(s, p),
        Some("create" | "c") => create(s, p),
        Some("scale" | "s") => scale(s, p),
        Some("align" | "a") => align(s, p),
        Some("model" | "m") => model(s, p),
        None | Some("list" | "?") => Ok(
            json!({ "sheets": SHEETS.iter().map(|(n, w, h, mm)| json!({ "name": n, "width": w, "height": h, "units": if *mm { "mm" } else { "in" } })).collect::<Vec<_>>() }),
        ),
        Some(o) => Err(bad("mvsetup", format!("unknown option `{o}`: title, create, scale, align, model or list"))),
    }
}

/// The standard sheet closest to a layout's paper (`size` mm, either way round).
fn nearest_sheet(size_mm: Vec2) -> &'static str {
    let (a, b) = (size_mm.x.max(size_mm.y), size_mm.x.min(size_mm.y));
    SHEETS
        .iter()
        .min_by(|x, y| {
            let off = |s: &(&str, f64, f64, bool)| {
                let k = if s.3 { 1.0 } else { 25.4 };
                (s.1 * k - a).abs() + (s.2 * k - b).abs()
            };
            off(x).total_cmp(&off(y))
        })
        .map_or("ANSI-B", |s| s.0)
}

fn sheet_param(p: &Value) -> Result<(&'static str, f64, f64, bool)> {
    let want = str_param(p, "size").unwrap_or("ANSI-B").trim().to_ascii_uppercase().replace([' ', '_'], "-");
    SHEETS
        .iter()
        .copied()
        .find(|(n, ..)| *n == want || n.split_once('-').is_some_and(|(_, short)| short == want))
        .ok_or_else(|| bad("mvsetup", format!("unknown sheet size `{want}` (ANSI-A…E, ARCH-C…E, ISO-A4…A0)")))
}

fn rect(a: Vec2, b: Vec2) -> EntityKind {
    let v = [a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)].map(PolyVertex::new).to_vec();
    EntityKind::LwPolyline(LwPolyline { vertices: v, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false })
}

fn line(a: Vec2, b: Vec2) -> EntityKind {
    EntityKind::Line(cadaddy_doc::Line { a: a.to3(0.0), b: b.to3(0.0) })
}

fn label(at: Vec2, height: f64, value: &str) -> EntityKind {
    EntityKind::Text(Text {
        insert: at.to3(0.0),
        align_pt: Some(at.to3(0.0)),
        height,
        value: value.to_string(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::Left,
        valign: VAlign::Top,
    })
}

/// The border and title block of a `w` × `h` sheet, in the sheet's own units: the border inside
/// a margin, and a title block in the lower right corner (a title row over Drawn, Date, Scale and
/// Sheet). Returns its objects and the drawing area left above the title block.
fn title_block(w: f64, h: f64, mm: bool) -> (Vec<EntityKind>, Bounds2) {
    let unit = if mm { 25.4 } else { 1.0 };
    let m = 0.5 * unit;
    let (x0, y0, x1, y1) = (m, m, w - m, h - m);
    let tw = (7.0 * unit).min((x1 - x0) * 0.5);
    let th = (1.5 * unit).min((y1 - y0) * 0.25);
    let tx = x1 - tw;
    let mut out = vec![rect(Vec2::new(x0, y0), Vec2::new(x1, y1)), rect(Vec2::new(tx, y0), Vec2::new(x1, y0 + th))];
    let mid = y0 + th / 2.0;
    out.push(line(Vec2::new(tx, mid), Vec2::new(x1, mid)));
    let cell = tw / 4.0;
    for i in 1..4 {
        let x = tx + cell * f64::from(i);
        out.push(line(Vec2::new(x, y0), Vec2::new(x, mid)));
    }
    let t = th * 0.12;
    let pad = t * 0.6;
    out.push(label(Vec2::new(tx + pad, y0 + th - pad), t, "TITLE"));
    for (i, name) in ["DRAWN", "DATE", "SCALE", "SHEET"].iter().enumerate() {
        out.push(label(Vec2::new(tx + cell * i as f64 + pad, mid - pad), t, name));
    }
    let area = Bounds2::new(Vec2::new(x0, y0 + th), Vec2::new(x1, y1));
    (out, area)
}

fn layout_of(s: &Session, cmd: &str, p: &Value) -> Result<String> {
    match target_layout(s, cmd, p)? {
        Space::Paper(name) => Ok(name),
        Space::Model => Err(bad(cmd, "MVSETUP's title, create, scale and align options work in a layout (on the Model tab: `model`)")),
    }
}

/// Paper units per inch of the layout `name`.
fn per_inch(s: &Session, name: &str) -> Result<f64> {
    let sheet = cadaddy_render::sheet(s.doc()?, name).ok_or_else(|| bad("mvsetup", format!("no layout `{name}`")))?;
    Ok(25.4 / sheet.unit_mm.max(1e-9))
}

/// `title`: a border and title block block, inserted at the sheet's corner.
fn title(s: &mut Session, p: &Value) -> Result<Value> {
    let layout = layout_of(s, "mvsetup", p)?;
    // Without a size: the standard sheet closest to the layout's paper.
    let (name, w, h, mm) = match str_param(p, "size") {
        Some(_) => sheet_param(p)?,
        None => {
            let sheet = cadaddy_render::sheet(s.doc()?, &layout).ok_or_else(|| bad("mvsetup", format!("no layout `{layout}`")))?;
            sheet_param(&json!({ "size": nearest_sheet(sheet.size * sheet.unit_mm) }))?
        }
    };
    let (objs, area) = title_block(w, h, mm);
    // The block is drawn in the sheet's own units; the layout's paper units may differ.
    let k = per_inch(s, &layout)? / if mm { 25.4 } else { 1.0 };
    let origin = cadaddy_render::sheet(s.doc()?, &layout).map_or(Vec2::ZERO, |sh| sh.origin);
    let block = format!("MVSETUP-{name}");
    let d = s.doc_mut()?;
    if d.block(&block).is_none() {
        let mut b = Block::new(&block);
        b.description = format!("{name} border and title block (MVSETUP)");
        for kind in objs {
            let h = d.new_handle();
            b.entities.push(Entity::new(h, kind));
        }
        d.blocks.insert(block.clone(), Arc::new(b));
    }
    let h = crate::cmd::blocks::insert(s, &block, origin, k, 0.0, &serde_json::Map::new())?;
    let area = Bounds2::new(origin + area.min * k, origin + area.max * k);
    Ok(json!({ "block": block, "handle": h.hex(), "area": [[area.min.x, area.min.y], [area.max.x, area.max.y]] }))
}

fn int(cmd: &str, p: &Value, key: &str, max: usize) -> Result<usize> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(1),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| bad(cmd, format!("`{key}` must be 1-{max}"))),
    }
}

/// `create`: `rows` × `cols` viewports filling the area (default: the printable area), `gap`
/// paper units apart, each showing the model's extents (or at `scale`).
fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "mvsetup";
    let layout = layout_of(s, cmd, p)?;
    let (rows, cols) = (int(cmd, p, "rows", MAX_GRID)?, int(cmd, p, "cols", MAX_GRID)?);
    let gap = p.get("gap").and_then(Value::as_f64).unwrap_or(0.0);
    let area = match (point_param(p, "p1"), point_param(p, "p2")) {
        (Some(a), Some(b)) => Bounds2::new(a, b),
        _ => printable(s, &layout)?,
    };
    let (cw, ch) = ((area.width() - gap * (cols - 1) as f64) / cols as f64, (area.height() - gap * (rows - 1) as f64) / rows as f64);
    if !(gap.is_finite() && gap >= 0.0 && cw > 1e-6 && ch > 1e-6 && area.width() < 1e7 && area.height() < 1e7) {
        return Err(bad(cmd, "the viewports have no room: a smaller `gap` or a larger area"));
    }
    let scale = p
        .get("scale")
        .map(|v| parse_scale(v).ok_or_else(|| bad(cmd, "`scale` must be a positive number or a ratio such as \"1:50\"")))
        .transpose()?;
    let common = s.current_common();
    let space = Space::Paper(layout);
    let d = s.doc_mut()?;
    let mut out = Vec::new();
    // Row by row from the top, left to right.
    for r in 0..rows {
        for c in 0..cols {
            let x = area.min.x + (cw + gap) * c as f64;
            let y = area.max.y - (ch + gap) * r as f64 - ch;
            let id = crate::cmd::clip_partners::next_viewport_id(d);
            let mut vp = viewports::new_viewport(d, Bounds2::new(Vec2::new(x, y), Vec2::new(x + cw, y + ch)), id);
            if let Some(k) = scale {
                vp.view_height = vp.height / k;
            }
            out.push(d.add(&space, common.clone(), EntityKind::Viewport(vp))?);
        }
    }
    Ok(json!({ "viewports": out.iter().map(|h| h.hex()).collect::<Vec<_>>() }))
}

/// The viewports `handles` names (or `handle`), else every viewport of the layout.
fn chosen(s: &Session, layout: &str, p: &Value) -> Result<Vec<(Handle, Viewport)>> {
    let d = s.doc()?;
    let all: Vec<(Handle, Viewport)> = d
        .space(&Space::Paper(layout.to_string()))
        .map(|st| {
            st.iter().filter_map(|e| if let EntityKind::Viewport(v) = &e.kind { (v.id != 1).then(|| (e.handle, v.clone())) } else { None }).collect()
        })
        .unwrap_or_default();
    if p.get("handles").is_none() && p.get("handle").is_none() {
        return Ok(all);
    }
    let want = targets(s, p)?;
    let picked: Vec<_> = all.into_iter().filter(|(h, _)| want.contains(h)).collect();
    if picked.is_empty() {
        return Err(bad("mvsetup", "no viewport of the layout among `handles`"));
    }
    Ok(picked)
}

/// `scale`: the viewports' scale (paper units per model unit, or "1:50"), about their centres.
fn scale(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "mvsetup";
    let layout = layout_of(s, cmd, p)?;
    let k = p.get("scale").and_then(parse_scale).ok_or_else(|| bad(cmd, "`scale` (a positive number or \"1:50\") is required"))?;
    let vps = chosen(s, &layout, p)?;
    if vps.is_empty() {
        return Err(bad(cmd, "the layout has no viewports"));
    }
    let d = s.doc_mut()?;
    let mut done = Vec::new();
    for (h, _) in vps {
        let ok = d.modify_entity(h, |e| {
            if let EntityKind::Viewport(v) = &mut e.kind
                && !v.locked
            {
                v.view_height = v.height / k;
            }
        });
        if ok.is_ok() {
            done.push(h.hex());
        }
    }
    Ok(json!({ "viewports": done, "scale": k }))
}

/// `align`: pan viewport `handle` so model point `point` (seen in it) lines up with model point
/// `base` seen in viewport `base_viewport`: `mode` horizontal (same height on the sheet),
/// vertical (one above the other) or both (the same place).
fn align(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "mvsetup";
    let layout = layout_of(s, cmd, p)?;
    let vps = chosen(s, &layout, &json!({}))?;
    let pick = |key: &str| -> Result<(Handle, Viewport)> {
        let h = crate::cmd::curves::handle_param(p, key).ok_or_else(|| bad(cmd, format!("`{key}` (a viewport handle) is required")))?;
        vps.iter().find(|(x, _)| *x == h).cloned().ok_or_else(|| bad(cmd, format!("`{key}` is not a viewport of the layout")))
    };
    let (bh, bv) = pick("base_viewport")?;
    let (h, v) = pick("handle")?;
    if bh == h {
        return Err(bad(cmd, "align one viewport to another"));
    }
    if v.locked {
        return Err(bad(cmd, "the viewport to pan is locked"));
    }
    let base = point_req(cmd, p, "base")?;
    let point = point_req(cmd, p, "point")?;
    let (Some(from), Some(to)) = (v.to_paper(point), bv.to_paper(base)) else { return Err(bad(cmd, "a viewport has no area")) };
    let mode = str_param(p, "mode").unwrap_or("horizontal").trim().to_ascii_lowercase();
    let shift = match mode.as_str() {
        "horizontal" | "h" => Vec2::new(0.0, to.y - from.y),
        "vertical" | "v" | "vert" => Vec2::new(to.x - from.x, 0.0),
        "both" | "point" | "b" => to - from,
        _ => return Err(bad(cmd, "`mode` must be horizontal, vertical or both")),
    };
    // Moving the model on the sheet by `shift` moves the view centre the other way.
    let k = v.view_height / v.height;
    let new_center = v.view_center - shift.rotate(-v.twist) * k;
    s.doc_mut()?.modify_entity(h, |e| {
        if let EntityKind::Viewport(x) = &mut e.kind {
            x.view_center = new_center;
        }
    })?;
    Ok(json!({ "viewport": h.hex(), "moved": [shift.x, shift.y] }))
}

/// `model`: on the Model tab, a border the size of sheet `size` at drawing scale `scale` (model
/// units per sheet unit, e.g. 48 for 1/4" = 1'-0"), with the limits set to it.
fn model(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "mvsetup";
    if s.layout_space() != Space::Model {
        return Err(bad(cmd, "`model` works on the Model tab (TILEMODE 1)"));
    }
    let (name, w, h, _) = sheet_param(p)?;
    let k = p.get("scale").and_then(Value::as_f64).filter(|k| k.is_finite() && *k > 0.0 && *k < 1e6).unwrap_or(1.0);
    let max = Vec2::new(w * k, h * k);
    let hb = s.add_entity(rect(Vec2::ZERO, max))?;
    let d = s.doc_mut()?;
    d.header.set("LIMMIN", cadaddy_doc::HVal::Point(Vec3::ZERO));
    d.header.set("LIMMAX", cadaddy_doc::HVal::Point(max.to3(0.0)));
    Ok(json!({ "border": hb.hex(), "sheet": name, "limits": [[0.0, 0.0], [max.x, max.y]] }))
}

/// MVSETUP typed: an option, then its values; back to the options until Enter.
#[derive(Default)]
pub(super) struct MvsetupM {
    stage: Stage,
    rows: usize,
    mode: &'static str,
    base: Option<(Handle, Vec2)>,
    sheet: String,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Stage {
    #[default]
    Main,
    Title,
    Rows,
    Cols,
    Scale,
    AlignMode,
    AlignBase,
    AlignPoint,
    ModelScale,
}

impl MvsetupM {
    fn word(i: &Input) -> Option<String> {
        match i {
            Input::Text(t) | Input::Keyword(t) => Some(t.trim().to_ascii_lowercase()),
            _ => None,
        }
    }
    /// A layout viewport under paper point `at`, and the model point it shows there.
    fn seen(s: &Session, at: Vec2) -> Result<(Handle, Vec2)> {
        let h = viewports::viewport_at(s, at).ok_or_else(|| EngineError::Other("Point is not in a viewport.".into()))?;
        let d = s.doc()?;
        match d.entity(h).map(|e| &e.kind) {
            Some(EntityKind::Viewport(v)) => v.to_model(at).map(|m| (h, m)).ok_or_else(|| EngineError::Other("The viewport has no area.".into())),
            _ => Err(EngineError::Other("Point is not in a viewport.".into())),
        }
    }
}

impl Interactive for MvsetupM {
    fn name(&self) -> &'static str {
        "MVSETUP"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        let sizes = "ANSI-A…E, ARCH-C…E, ISO-A4…A0";
        match self.stage {
            Stage::Main if s.layout_space() == Space::Model => {
                Prompt::new(format!("Enter sheet size for a model space border ({sizes})"), Accept::TEXT).default("ANSI-B")
            }
            Stage::Main => Prompt::new("Enter an option", Accept::TEXT).kw(&["Align", "Create", "Scale viewports", "Title block"]),
            Stage::Title => Prompt::new(format!("Enter sheet size ({sizes}; Enter: the layout's)"), Accept::TEXT),
            Stage::Rows => Prompt::new("Enter number of rows", Accept::NUMBER).default("1"),
            Stage::Cols => Prompt::new("Enter number of columns", Accept::NUMBER).default("1"),
            Stage::Scale => Prompt::new("Enter the viewports' scale (paper:model, e.g. 1:50)", Accept::TEXT).default("1:1"),
            Stage::AlignMode => Prompt::new("Enter an option", Accept::TEXT).kw(&["Horizontal", "Vertical alignment"]).default("Horizontal"),
            Stage::AlignBase => Prompt::new("Specify base point", Accept::POINT),
            Stage::AlignPoint => Prompt::new("Specify point in viewport to pan", Accept::POINT),
            Stage::ModelScale => Prompt::new("Enter the scale factor (model units per sheet unit)", Accept::NUMBER).default("1"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        let enter = i == Input::Enter;
        let num = |i: &Input, default: f64| -> Option<f64> {
            match i {
                Input::Enter => Some(default),
                Input::Text(t) => t.trim().parse().ok(),
                _ => None,
            }
        };
        match self.stage {
            Stage::Main if s.layout_space() == Space::Model => {
                self.sheet = if enter { "ANSI-B".into() } else { Self::word(&i).unwrap_or_default() };
                sheet_param(&json!({ "size": self.sheet }))?;
                self.stage = Stage::ModelScale;
            }
            Stage::Main => match Self::word(&i).as_deref() {
                None if enter => return Ok(Step::Done),
                Some(w) if w.starts_with('a') => self.stage = Stage::AlignMode,
                Some(w) if w.starts_with('c') => self.stage = Stage::Rows,
                Some(w) if w.starts_with('s') => self.stage = Stage::Scale,
                Some(w) if w.starts_with('t') => self.stage = Stage::Title,
                _ => return Err(EngineError::Other("Invalid option keyword.".into())),
            },
            Stage::Title => {
                let p = if enter { json!({ "option": "title" }) } else { json!({ "option": "title", "size": Self::word(&i).unwrap_or_default() }) };
                title(s, &p)?;
                self.stage = Stage::Main;
            }
            Stage::Rows => {
                let n = num(&i, 1.0).filter(|n| n.fract() == 0.0 && (1.0..=MAX_GRID as f64).contains(n));
                self.rows = n.ok_or_else(|| EngineError::Other(format!("Requires a whole number from 1 to {MAX_GRID}.")))? as usize;
                self.stage = Stage::Cols;
            }
            Stage::Cols => {
                let n = num(&i, 1.0).filter(|n| n.fract() == 0.0 && (1.0..=MAX_GRID as f64).contains(n));
                let cols = n.ok_or_else(|| EngineError::Other(format!("Requires a whole number from 1 to {MAX_GRID}.")))? as usize;
                create(s, &json!({ "option": "create", "rows": self.rows, "cols": cols }))?;
                self.stage = Stage::Main;
            }
            Stage::Scale => {
                let t = match &i {
                    Input::Enter => "1:1".to_string(),
                    Input::Text(t) | Input::Keyword(t) => t.clone(),
                    _ => return Err(EngineError::Other("Requires a scale such as 1:50.".into())),
                };
                scale(s, &json!({ "option": "scale", "scale": t.trim() }))?;
                self.stage = Stage::Main;
            }
            Stage::AlignMode => {
                self.mode = match Self::word(&i).as_deref() {
                    None if enter => "horizontal",
                    Some(w) if w.starts_with('h') => "horizontal",
                    Some(w) if w.starts_with('v') => "vertical",
                    _ => return Err(EngineError::Other("Invalid option keyword.".into())),
                };
                self.stage = Stage::AlignBase;
            }
            Stage::AlignBase => {
                let Input::Point(at) = i else { return Err(EngineError::Other("Requires a point.".into())) };
                self.base = Some(Self::seen(s, at)?);
                self.stage = Stage::AlignPoint;
            }
            Stage::AlignPoint => {
                let Input::Point(at) = i else { return Err(EngineError::Other("Requires a point.".into())) };
                let (h, m) = Self::seen(s, at)?;
                let Some((bh, bm)) = self.base else { return Ok(Step::Cancel) };
                let p = json!({ "option": "align", "mode": self.mode, "base_viewport": bh.hex(), "base": [bm.x, bm.y], "handle": h.hex(), "point": [m.x, m.y] });
                align(s, &p)?;
                self.stage = Stage::Main;
            }
            Stage::ModelScale => {
                let k = num(&i, 1.0).filter(|k| k.is_finite() && *k > 0.0).ok_or_else(|| EngineError::Other("Requires a positive number.".into()))?;
                model(s, &json!({ "option": "model", "size": self.sheet, "scale": k }))?;
                return Ok(Step::Done);
            }
        }
        Ok(Step::Continue)
    }
}
