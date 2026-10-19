//! Review markups from PDFs: MARKUPIMPORT reads a page's annotations (as any PDF markup program
//! leaves them) and places them on a markup layer over the sheet: shapes (clouds, rectangles,
//! ellipses, polygons, stamps, highlights) as revision clouds, arrows and callouts as
//! multileaders with their comment, notes as multiline text, lines and ink as polylines.
//! Placement: onto the current layout's sheet (the page is the sheet: its title block lands on
//! the layout's), through a picked pair of points (two page points and where they go), or at a
//! scale and insertion point. Each markup's objects carry its id, status, kind, author and
//! comment (`CADADDY_MARKUP` extended data); MARKUPS lists them and MARKUPSTATUS marks them open
//! or done.

use cadaddy_doc::markup::{Markup, MarkupKind, MarkupPage, MarkupReader};
use cadaddy_doc::{EntityKind, Handle, RawTag, Space};
use cadaddy_geom::{PolyVertex, Vec2};
use serde_json::{Value, json};

use super::{CommandSpec, bad, has_doc, str_param};
use crate::{Result, Session};

/// The extended data application holding a markup's record.
pub const APP: &str = "CADADDY_MARKUP";
const CMD: &str = "markupimport";
/// Most markups placed from one page.
const MAX_MARKUPS: usize = 5000;

static READER: std::sync::OnceLock<MarkupReader> = std::sync::OnceLock::new();

/// Install the PDF markup reader (once).
pub fn set_reader(f: MarkupReader) {
    let _ = READER.set(f);
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("markupimport", "Import Markups", run_import)
            .menu(&["Insert", "Markup Import..."])
            .params(
                "{path: PDF, page?: 1, layer?: \"MARKUP\", placement: sheet (default in a layout: the page onto the layout's sheet) | \
                 from: [[px,py],[px,py]] (page points, from the bottom left) + to: [[x,y],[x,y]] (where they go) | \
                 scale?: drawing units per inch (default 1) + at?: [x,y] (the page's corner)} → {placed: [{id, kind, handles}], skipped}",
            )
            .enabled(has_doc),
        CommandSpec::new("markups", "Markups", run_list)
            .params("{} → {markups: [{id, status, kind, author, text, handles}]}")
            .noundo()
            .enabled(has_doc),
        CommandSpec::new("markupstatus", "Markup Status", run_status).params("{id, status: open|done}").enabled(has_doc),
    ]
}

/// Where page points go: a similarity (rotation, uniform scale, move).
#[derive(Clone, Copy, Debug)]
struct Place {
    /// `p · (a, b)` rotated and scaled: x' = a·x − b·y + tx, y' = b·x + a·y + ty.
    a: f64,
    b: f64,
    t: Vec2,
}

impl Place {
    fn apply(&self, p: [f64; 2]) -> Vec2 {
        Vec2::new(self.a * p[0] - self.b * p[1] + self.t.x, self.b * p[0] + self.a * p[1] + self.t.y)
    }
    fn scale(&self) -> f64 {
        self.a.hypot(self.b)
    }
    /// The similarity taking page points `p0, p1` to `q0, q1`.
    fn from_pairs(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> Option<Place> {
        let dp = p1 - p0;
        let dq = q1 - q0;
        let n = dp.x * dp.x + dp.y * dp.y;
        if n < 1e-18 {
            return None;
        }
        let a = (dq.x * dp.x + dq.y * dp.y) / n;
        let b = (dq.y * dp.x - dq.x * dp.y) / n;
        let t = Vec2::new(q0.x - (a * p0.x - b * p0.y), q0.y - (b * p0.x + a * p0.y));
        Some(Place { a, b, t })
    }
}

fn pairs(v: Option<&Value>) -> Option<(Vec2, Vec2)> {
    let a = v?.as_array()?;
    let p = |i: usize| super::point_value(a.get(i)?);
    Some((p(0)?, p(1)?))
}

/// The placement asked for, or the default for the current space.
fn placement(s: &Session, p: &Value, page: &MarkupPage) -> Result<Place> {
    if let (Some((p0, p1)), Some((q0, q1))) = (pairs(p.get("from")), pairs(p.get("to"))) {
        return Place::from_pairs(p0, p1, q0, q1).ok_or_else(|| bad(CMD, "the two page points are the same"));
    }
    let space = s.space();
    let d = s.doc()?;
    if let (Space::Paper(name), None) = (&space, p.get("scale")) {
        // The page is the layout's sheet: 1/72 inch per point, in the layout's paper units.
        let l = d.layouts.iter().find(|l| &l.name == name).ok_or_else(|| bad(CMD, "no current layout"))?;
        let mm = l.page.mm_units.unwrap_or(d.header.i64("MEASUREMENT", 0) == 1);
        let k = if mm { 25.4 / 72.0 } else { 1.0 / 72.0 };
        let _ = page;
        return Ok(Place { a: k, b: 0.0, t: l.origin });
    }
    let scale = p.get("scale").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(1.0);
    let at = super::point_param(p, "at").unwrap_or(Vec2::ZERO);
    Ok(Place { a: scale / 72.0, b: 0.0, t: at })
}

/// Text of a markup's record, cut to what one extended data string holds.
fn short(s: &str) -> String {
    s.chars().take(250).collect()
}

fn record(id: &str, status: &str, m: &Markup) -> Vec<RawTag> {
    let kind = serde_json::to_value(m.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let t = |code: i32, v: &str| RawTag { code, value: v.to_string() };
    vec![t(1001, APP), t(1000, id), t(1000, status), t(1000, &kind), t(1000, &short(&m.author)), t(1000, &short(&m.text))]
}

/// A markup's record in an object's extended data: (id, status, kind, author, text).
fn read_record(xdata: &[RawTag]) -> Option<(String, String, String, String, String)> {
    let at = xdata.iter().position(|t| t.code == 1001 && t.value == APP)?;
    let v = |k: usize| xdata.get(at + k).filter(|t| t.code == 1000).map(|t| t.value.clone()).unwrap_or_default();
    Some((v(1), v(2), v(3), v(4), v(5)))
}

/// Every object of the drawing that belongs to a markup.
fn marked(d: &cadaddy_doc::Drawing) -> Vec<(Handle, (String, String, String, String, String))> {
    std::iter::once(&d.model)
        .chain(d.layouts.iter().map(|l| &l.entities))
        .flat_map(|st| st.iter())
        .filter_map(|e| Some((e.handle, read_record(&e.common.xdata)?)))
        .collect()
}

/// A closed revision cloud around `pts` (drawing coordinates).
fn cloud(s: &mut Session, pts: &[Vec2]) -> Option<Handle> {
    let len = s.doc().ok().map(|d| {
        let metric = d.header.i64("MEASUREMENT", 0) == 1;
        d.header.f64("REVCLOUDARCLEN", if metric { 15.0 } else { 0.5 })
    })?;
    // Arcs about a tenth of the shape's size, never fewer than about six.
    let b = cadaddy_geom::Bounds2::from_points(pts.iter().copied());
    let size = b.width().max(b.height());
    let arc = len.min(size / 6.0).max(size / 60.0).max(1e-6);
    let vs = super::draw2::cloud_vertices(pts, arc, false, false)?;
    s.add_entity(super::helpers::lwpoly(vs, true)).ok()
}

fn polyline(s: &mut Session, pts: &[Vec2], closed: bool) -> Option<Handle> {
    (pts.len() >= 2).then_some(())?;
    s.add_entity(super::helpers::lwpoly(pts.iter().map(|p| PolyVertex::new(*p)).collect(), closed)).ok()
}

/// The objects one markup becomes.
fn place_one(s: &mut Session, m: &Markup, at: &Place) -> Vec<Handle> {
    let pts: Vec<Vec2> = m.points.iter().map(|p| at.apply(*p)).collect();
    let rect: Vec<Vec2> = {
        let [x0, y0, x1, y1] = m.rect;
        [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].into_iter().map(|p| at.apply(p)).collect()
    };
    let height = (10.0 * at.scale()).max(1e-6);
    let mut out = Vec::new();
    match m.kind {
        MarkupKind::Cloud | MarkupKind::Rectangle | MarkupKind::Ellipse | MarkupKind::Polygon | MarkupKind::Stamp => {
            out.extend(cloud(s, if pts.len() >= 3 { &pts } else { &rect }));
        }
        MarkupKind::Highlight => {
            for q in &m.strokes {
                let q: Vec<Vec2> = q.iter().map(|p| at.apply(*p)).collect();
                out.extend(cloud(s, &q));
            }
        }
        MarkupKind::Polyline | MarkupKind::Line => out.extend(polyline(s, &pts, false)),
        MarkupKind::Ink => {
            for st in &m.strokes {
                let st: Vec<Vec2> = st.iter().map(|p| at.apply(*p)).collect();
                out.extend(polyline(s, &st, false));
            }
        }
        MarkupKind::Arrow | MarkupKind::Callout => {
            // Arrowhead first, landing last; a callout without a leader points at its box.
            let mut leader = pts.clone();
            if leader.len() < 2 {
                leader = vec![rect.first().copied().unwrap_or_default(), rect.get(2).copied().unwrap_or_default()];
            }
            let points: Vec<[f64; 2]> = leader.iter().map(|p| [p.x, p.y]).collect();
            let text = if m.text.trim().is_empty() { m.subject.clone() } else { m.text.clone() };
            if let Ok(r) = super::annotate::run_mleader(s, &json!({ "points": points, "text": text }))
                && let Some(h) = r["handle"].as_str().and_then(Handle::parse_hex)
            {
                out.push(h);
            }
        }
        MarkupKind::Note => {
            let at_pt = rect.get(3).copied().unwrap_or_default();
            let style = s.doc().map(|d| d.header.str("TEXTSTYLE", "Standard")).unwrap_or_else(|_| "Standard".into());
            let text = if m.text.is_empty() { m.subject.clone() } else { m.text.clone() };
            let mt = cadaddy_doc::MText {
                insert: at_pt.to3(0.0),
                height,
                width: (m.rect[2] - m.rect[0]).abs() * at.scale(),
                attach: 1,
                rotation: at.b.atan2(at.a),
                style,
                contents: text,
                line_spacing: 1.0,
                line_spacing_exact: false,
                columns: None,
                background: None,
            };
            out.extend(s.add_entity(EntityKind::MText(mt)).ok());
        }
    }
    out
}

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(CMD, "`path` of a PDF is required"))?.to_string();
    let reader = READER.get().ok_or_else(|| bad(CMD, "PDF markups can't be read in this build"))?;
    #[cfg(not(target_arch = "wasm32"))]
    let bytes = crate::readfile::read(&path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    #[cfg(target_arch = "wasm32")]
    let bytes: Vec<u8> = return Err(bad(CMD, "files can't be read on the web"));
    let page_no = p.get("page").and_then(Value::as_u64).map_or(1, |n| n.clamp(1, 100_000) as u32);
    let page = reader(&bytes, page_no).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    let at = placement(s, p, &page)?;
    let layer = str_param(p, "layer").map(str::trim).filter(|x| !x.is_empty()).unwrap_or("MARKUP").to_string();
    {
        let d = s.doc_mut()?;
        if d.layer(&layer).is_none() {
            let mut l = cadaddy_doc::Layer::new(&layer);
            l.color = cadaddy_color::Color::Index(1);
            l.plot = false;
            d.layers.push(l);
        }
    }
    // Ids continue after the drawing's markups.
    let mut next = marked(s.doc()?).iter().filter_map(|(_, r)| r.0.strip_prefix('M')?.parse::<u32>().ok()).max().unwrap_or(0) + 1;
    let mut placed = Vec::new();
    let mut skipped = 0;
    for m in page.markups.iter().take(MAX_MARKUPS) {
        let hs = place_one(s, m, &at);
        if hs.is_empty() {
            skipped += 1;
            continue;
        }
        let id = format!("M{next}");
        next += 1;
        let xd = record(&id, "open", m);
        let color = m.color.map(|[r, g, b]| cadaddy_color::Color::True(cadaddy_color::Rgb(r, g, b)));
        let d = s.doc_mut()?;
        for h in &hs {
            d.modify_entity(*h, |e| {
                e.common.layer = layer.clone();
                if let Some(c) = color {
                    e.common.color = c;
                }
                e.common.xdata.extend(xd.iter().cloned());
            })?;
        }
        let kind = serde_json::to_value(m.kind).unwrap_or(Value::Null);
        placed.push(json!({ "id": id, "kind": kind, "handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>() }));
    }
    skipped += page.markups.len().saturating_sub(MAX_MARKUPS);
    let msg =
        format!("{} markup(s) placed on layer {layer}{}.", placed.len(), if skipped > 0 { format!(", {skipped} skipped") } else { String::new() });
    Ok(json!({ "placed": placed, "skipped": skipped, "message": msg }))
}

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let mut list: Vec<(String, Value)> = Vec::new();
    for (h, (id, status, kind, author, text)) in marked(s.doc()?) {
        match list.iter_mut().find(|(i, _)| *i == id) {
            Some((_, v)) => {
                if let Some(a) = v["handles"].as_array_mut() {
                    a.push(json!(h.hex()));
                }
            }
            None => {
                list.push((id.clone(), json!({ "id": id, "status": status, "kind": kind, "author": author, "text": text, "handles": [h.hex()] })))
            }
        }
    }
    list.sort_by_key(|(id, _)| id.trim_start_matches('M').parse::<u64>().unwrap_or(u64::MAX));
    Ok(json!({ "markups": list.into_iter().map(|(_, v)| v).collect::<Vec<_>>() }))
}

fn run_status(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_param(p, "id").ok_or_else(|| bad("markupstatus", "`id` is required"))?.to_string();
    let status = match str_param(p, "status").unwrap_or_default().to_ascii_lowercase().as_str() {
        "open" => "open",
        "done" | "closed" => "done",
        other => return Err(bad("markupstatus", format!("status is open or done, not `{other}`"))),
    };
    let hs: Vec<Handle> = marked(s.doc()?).into_iter().filter(|(_, r)| r.0 == id).map(|(h, _)| h).collect();
    if hs.is_empty() {
        return Err(bad("markupstatus", format!("no markup {id}")));
    }
    let d = s.doc_mut()?;
    for h in &hs {
        d.modify_entity(*h, |e| {
            if let Some(at) = e.common.xdata.iter().position(|t| t.code == 1001 && t.value == APP)
                && let Some(t) = e.common.xdata.get_mut(at + 2)
            {
                t.value = status.to_string();
            }
        })?;
    }
    Ok(json!({ "id": id, "status": status, "objects": hs.len() }))
}
