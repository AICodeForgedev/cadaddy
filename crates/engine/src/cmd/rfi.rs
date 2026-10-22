//! What the prints don't say: the RFI list (gaps and conflicts in the prints) for the building
//! spec, from the print readers' items in the spec (`rfis`, with the sheets involved) and the
//! spec's own gaps ([`cadaddy_walk3d::rfi::check`]).
//!
//! - `rfi.list {level?, kind?, severity?, status?}`: the items, most severe first;
//! - `rfi.zoom {id}`: the plan view on an item;
//! - `rfi.set {id, status?, answer?, severity?}`: track an item (kept in the spec);
//! - `rfi.export {path, format?, pictures?}`: the list as CSV, XLSX or PDF; XLSX and PDF carry a
//!   picture of the built model at each item (the model only, never the prints).

use cadaddy_doc::{Common, Drawing, EntityKind, Image, Line, Space, Text};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, Rfi, RfiKind, RfiStatus, Severity};
use serde_json::{Value, json};

use super::bspec::current;
use super::*;
use crate::{Result, Session};

/// Items pictured in one export at most (each is a render).
#[cfg(not(target_arch = "wasm32"))]
const MAX_PICTURES: usize = 200;
/// The longest answer kept.
const MAX_ANSWER: usize = 4_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("rfi.list", "RFI List", run_list)
            .params(
                "{level?: name, kind?: untagged_wall|unknown_tag|sheet_conflict|height_vs_ceiling|rated_to_ceiling|double_ceiling_short_wall|head_unspecified|ceiling_unspecified|soffit_no_detail|detail_missing|detail_unused|sleeve_no_route|sleeve_no_fit, severity?: high|medium|low, status?: open|sent|answered|closed} → {items: [{id, kind, title, severity, level, at, extent?, sheets, about, issue, assumed, proposed?, status, answer?}], total, bySeverity, byKind, csv}: where the prints are silent or disagree and what was assumed meanwhile — the readers' items in the spec (with their sheets) and the spec's own gaps (guessed wall and ceiling types, rated walls stopping at a ceiling, heads of wall not given…)",
            )
            .noundo(),
        CommandSpec::new("rfi.zoom", "Zoom to RFI", run_zoom).params("{id} → {id, level, window}: the plan view on the item (its extent, or 20' round it)").noundo(),
        CommandSpec::new("rfi.set", "Update RFI", run_set)
            .params("{id, status?: open|sent|answered|closed, answer?: text, severity?: high|medium|low} → {item}: tracks an item in the spec (items from the spec's own gaps are copied into it)"),
        CommandSpec::new("rfi.export", "Export RFI List", run_export)
            .params(
                "{path: .csv|.xlsx|.pdf, format?: csv|xlsx|pdf (default from the extension), pictures?: bool (default true; XLSX/PDF: a picture of the built model at each item, saved beside the file in <name>-pictures/), level?, kind?, severity?, status?} → {path, format, items, pictures}",
            )
            .noundo(),
    ]
}

/// The list, filtered by the usual parameters.
fn items(s: &Session, cmd: &str, p: &Value) -> Result<(BuildingSpec, Vec<Rfi>)> {
    let (sp, _) = current(s, cmd)?;
    let kind = match str_param(p, "kind") {
        Some(k) => Some(RfiKind::parse(k).ok_or_else(|| bad(cmd, format!("unknown kind {k}")))?),
        None => None,
    };
    let severity = match str_param(p, "severity") {
        Some(k) => Some(Severity::parse(k).ok_or_else(|| bad(cmd, "`severity` is high, medium or low"))?),
        None => None,
    };
    let status = match str_param(p, "status") {
        Some(k) => Some(RfiStatus::parse(k).ok_or_else(|| bad(cmd, "`status` is open, sent, answered or closed"))?),
        None => None,
    };
    let level = str_param(p, "level");
    let mut list = cadaddy_walk3d::rfi::list(&sp);
    list.retain(|r| {
        kind.is_none_or(|k| r.kind == Some(k))
            && severity.is_none_or(|k| r.severity == k)
            && status.is_none_or(|k| r.status == k)
            && level.is_none_or(|l| r.level.eq_ignore_ascii_case(l))
    });
    list.sort_by(|a, b| a.severity.cmp(&b.severity).then_with(|| a.id.cmp(&b.id)));
    Ok((sp, list))
}

fn item_json(r: &Rfi) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
    v["kind"] = json!(r.kind_name());
    v["title"] = json!(r.kind.map_or("Other", RfiKind::title));
    v
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, list) = items(s, "rfi.list", p)?;
    let mut by_kind = serde_json::Map::new();
    for r in &list {
        let n = by_kind.get(r.kind_name()).and_then(Value::as_u64).unwrap_or(0) + 1;
        by_kind.insert(r.kind_name().into(), json!(n));
    }
    let count = |k: Severity| list.iter().filter(|r| r.severity == k).count();
    Ok(json!({
        "items": list.iter().map(item_json).collect::<Vec<_>>(),
        "total": list.len(),
        "bySeverity": {"high": count(Severity::High), "medium": count(Severity::Medium), "low": count(Severity::Low)},
        "byKind": by_kind,
        "csv": cadaddy_walk3d::rfi::csv(&list),
    }))
}

fn find(s: &Session, cmd: &str, p: &Value) -> Result<(BuildingSpec, Rfi)> {
    let id = str_param(p, "id").ok_or_else(|| bad(cmd, "`id` is required"))?;
    let (sp, _) = current(s, cmd)?;
    let item = cadaddy_walk3d::rfi::list(&sp)
        .into_iter()
        .find(|r| r.id.eq_ignore_ascii_case(id.trim()))
        .ok_or_else(|| bad(cmd, format!("there is no {id}")))?;
    Ok((sp, item))
}

/// The plan window that shows an item: its extent with a margin, else 20' round it.
fn window(sp: &BuildingSpec, r: &Rfi) -> Option<Bounds2> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let fin = |p: [f64; 2]| p.iter().all(|v| v.is_finite());
    let b = match r.extent {
        Some([a, b]) if fin(a) && fin(b) => Bounds2::from_points([Vec2::new(a[0], a[1]), Vec2::new(b[0], b[1])]).expand(6.0 * upf),
        _ if fin(r.at) && !r.level.is_empty() => Bounds2::from_points([Vec2::new(r.at[0], r.at[1])]).expand(10.0 * upf),
        _ => return None,
    };
    (b.width().is_finite() && b.height().is_finite() && b.width() < 1e9).then_some(b)
}

fn run_zoom(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, r) = find(s, "rfi.zoom", p)?;
    let w = window(&sp, &r).ok_or_else(|| bad("rfi.zoom", format!("{} is about the whole set (no place on a plan)", r.id)))?;
    s.execute("zoom", &json!({"mode": "window", "p1": [w.min.x, w.min.y], "p2": [w.max.x, w.max.y]}))?;
    Ok(json!({ "id": r.id, "level": r.level, "window": [w.min.x, w.min.y, w.max.x, w.max.y] }))
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, mut r) = find(s, "rfi.set", p)?;
    if let Some(st) = str_param(p, "status") {
        r.status = RfiStatus::parse(st).ok_or_else(|| bad("rfi.set", "`status` is open, sent, answered or closed"))?;
    }
    if let Some(sv) = str_param(p, "severity") {
        r.severity = Severity::parse(sv).ok_or_else(|| bad("rfi.set", "`severity` is high, medium or low"))?;
    }
    if let Some(a) = p.get("answer") {
        r.answer = a.as_str().map(|t| t.chars().take(MAX_ANSWER).collect::<String>()).filter(|t| !t.trim().is_empty());
    }
    let n = sp.rfis.len();
    match sp.rfis.iter_mut().find(|x| x.id == r.id) {
        Some(x) => *x = r.clone(),
        None if n < cadaddy_walk3d::rfi::MAX_RFIS => sp.rfis.push(r.clone()),
        None => return Err(bad("rfi.set", "the spec holds as many items as it can")),
    }
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(json!({ "item": item_json(&r) }))
}

// ---- Export ------------------------------------------------------------------------------------

/// A picture of the built model at an item: round its place, from above and to the south-west,
/// its level alone without ceilings, a red post where it is (PNG). None when nothing is built or
/// the item has no place.
#[cfg(not(target_arch = "wasm32"))]
fn picture(s: &mut Session, sp: &BuildingSpec, r: &Rfi, path: &str) -> Option<()> {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let level = sp.level(&r.level)?;
    if !r.at.iter().all(|v| v.is_finite()) {
        return None;
    }
    let (x, y) = (r.at[0], r.at[1]);
    // Its place and 15' round it (or its extent and 6' round that), from the south-west above.
    let w = window(sp, r).map(|b| b.expand(4.0 * upf)).unwrap_or_else(|| Bounds2::from_points([Vec2::new(x, y)]).expand(15.0 * upf));
    let around = [w.min.x, w.min.y, w.max.x, w.max.y];
    let req = json!({"level": level.name, "dir": [-0.6, -1.0, 0.9], "around": around, "mark": [x, y], "width": 640, "height": 480, "path": path});
    s.execute("model.render", &req).ok().map(|_| ())
}

fn text(d: &mut Drawing, at: Vec2, h: f64, value: &str) {
    let kind = EntityKind::Text(Text {
        insert: Vec3::new(at.x, at.y, 0.0),
        align_pt: None,
        height: h,
        value: value.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: Default::default(),
        valign: Default::default(),
    });
    let _ = d.add(&Space::Model, Common::default(), kind);
}

fn line(d: &mut Drawing, a: Vec2, b: Vec2) {
    let _ = d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(a.x, a.y, 0.0), b: Vec3::new(b.x, b.y, 0.0) }));
}

/// Words wrapped to lines of at most `n` characters.
fn wrap(s: &str, n: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in s.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > n {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The list as a PDF: two items a letter-size page (landscape), each with its picture.
fn pdf(list: &[Rfi], pictures: &[Option<(String, u32, u32)>]) -> std::result::Result<Vec<u8>, String> {
    const PER_PAGE: usize = 2;
    let mut d = Drawing::new_imperial();
    let pages = list.len().div_ceil(PER_PAGE).max(1);
    let mut items = Vec::new();
    for page in 0..pages {
        let ox = page as f64 * 12.0;
        text(&mut d, Vec2::new(ox + 0.5, 7.9), 0.2, "RFI LIST: WHERE THE PRINTS ARE SILENT OR DISAGREE");
        text(&mut d, Vec2::new(ox + 0.5, 7.6), 0.1, &format!("Page {} of {pages}. Pictures show the CADaddy model built meanwhile.", page + 1));
        line(&mut d, Vec2::new(ox + 0.5, 7.5), Vec2::new(ox + 10.5, 7.5));
        for (j, r) in list.iter().skip(page * PER_PAGE).take(PER_PAGE).enumerate() {
            let top = 7.3 - j as f64 * 3.6;
            let x = ox + 0.5;
            text(
                &mut d,
                Vec2::new(x, top - 0.2),
                0.16,
                &format!("{}  {}  {}", r.id, r.severity.name().to_uppercase(), r.kind.map_or("Other", RfiKind::title)),
            );
            let mut lines: Vec<String> = Vec::new();
            let field = |lines: &mut Vec<String>, label: &str, v: &str| {
                if !v.trim().is_empty() {
                    for (k, l) in wrap(&format!("{label}: {v}"), 78).into_iter().enumerate() {
                        lines.push(if k == 0 { l } else { format!("    {l}") });
                    }
                }
            };
            field(&mut lines, "Level", &r.level);
            field(&mut lines, "Sheets", &r.sheets.join(", "));
            field(&mut lines, "About", &r.about);
            field(&mut lines, "Issue", &r.issue);
            field(&mut lines, "Assumed meanwhile", &r.assumed);
            field(&mut lines, "Proposed", r.proposed.as_deref().unwrap_or_default());
            field(&mut lines, "Status", r.status.name());
            field(&mut lines, "Answer", r.answer.as_deref().unwrap_or_default());
            for (k, l) in lines.iter().take(17).enumerate() {
                text(&mut d, Vec2::new(x, top - 0.5 - k as f64 * 0.17), 0.1, l);
            }
            if let Some(Some((path, w, h))) = pictures.get(page * PER_PAGE + j) {
                let (bw, bh) = (3.6, 2.7);
                let px = (bw / f64::from((*w).max(1))).min(bh / f64::from((*h).max(1)));
                let image = Image {
                    insert: Vec3::new(ox + 7.0, top - 0.3 - bh, 0.0),
                    u: Vec3::new(px, 0.0, 0.0),
                    v: Vec3::new(0.0, px, 0.0),
                    size: Vec2::new(f64::from(*w), f64::from(*h)),
                    path: path.clone(),
                    name: r.id.clone(),
                    ..Default::default()
                };
                let _ = d.add(&Space::Model, Common::default(), EntityKind::Image(Box::new(image)));
            }
            line(&mut d, Vec2::new(x, top - 3.45), Vec2::new(ox + 10.5, top - 3.45));
        }
        items.push(json!({"window": [[ox, 0.0], [ox + 11.0, 8.5]], "paper": "Letter", "landscape": true}));
    }
    // Plotted by the app's plotter (the engine writes no PDF itself).
    let plot = super::file::io().and_then(|h| h.plot).ok_or("PDF export isn't available in this build")?;
    plot(&d, &Space::Model, &json!({"pages": items, "margins": [0.0, 0.0, 0.0, 0.0], "lineweights": false, "stamp": false}))
}

/// Pictures of the built model at each item, beside the export: `<name>-pictures/<id>.png`
/// (none when nothing is built: never pictures of the prints).
#[cfg(not(target_arch = "wasm32"))]
fn pictures_for(s: &mut Session, sp: &BuildingSpec, list: &[Rfi], path: &str) -> Vec<Option<(String, u32, u32)>> {
    let mut out = vec![None; list.len()];
    let built = s.doc().is_ok_and(|d| d.model.iter().any(|e| super::wallgen::made_as(e).is_some()));
    if !built {
        return out;
    }
    let pp = std::path::Path::new(path);
    let stem = pp.file_stem().and_then(|x| x.to_str()).unwrap_or("rfi");
    let dir = pp.with_file_name(format!("{stem}-pictures"));
    if std::fs::create_dir_all(&dir).is_err() {
        return out;
    }
    for (i, r) in list.iter().enumerate().take(MAX_PICTURES) {
        let f = dir.join(format!("{}.png", r.id)).to_string_lossy().to_string();
        if picture(s, sp, r, &f).is_some()
            && let Some(slot) = out.get_mut(i)
        {
            *slot = Some((f, 640, 480));
        }
    }
    out
}

#[cfg(target_arch = "wasm32")]
fn pictures_for(_: &mut Session, _: &BuildingSpec, list: &[Rfi], _: &str) -> Vec<Option<(String, u32, u32)>> {
    vec![None; list.len()]
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad("rfi.export", "`path` is required"))?.to_string();
    let format = match str_param(p, "format") {
        Some(f) => f.to_ascii_lowercase(),
        None => std::path::Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("csv").to_ascii_lowercase(),
    };
    if !["csv", "xlsx", "pdf"].contains(&format.as_str()) {
        return Err(bad("rfi.export", "`format` is csv, xlsx or pdf"));
    }
    let (sp, list) = items(s, "rfi.export", p)?;
    let want = bool_or(p, "pictures", true) && format != "csv";
    let pictures = if want { pictures_for(s, &sp, &list, &path) } else { vec![None; list.len()] };
    let bytes: Vec<u8> = match format.as_str() {
        "csv" => cadaddy_walk3d::rfi::csv(&list).into_bytes(),
        "xlsx" => {
            let mut rows = cadaddy_walk3d::rfi::rows(&list);
            if pictures.iter().any(Option::is_some) {
                for (i, row) in rows.iter_mut().enumerate() {
                    let cell = if i == 0 {
                        "Picture".to_string()
                    } else {
                        pictures
                            .get(i - 1)
                            .and_then(|x| x.as_ref())
                            .and_then(|(f, _, _)| std::path::Path::new(f).file_name())
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                    };
                    row.push(cell);
                }
            }
            super::table::xlsx::write(&rows, "RFIs").map_err(|e| bad("rfi.export", e))?
        }
        _ => pdf(&list, &pictures).map_err(|e| bad("rfi.export", e))?,
    };
    let mut out = json!({ "format": format, "items": list.len(), "pictures": pictures.iter().filter(|x| x.is_some()).count() });
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::writefile::write(&path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
        out["path"] = json!(path);
    }
    #[cfg(target_arch = "wasm32")]
    {
        out["data"] = json!(super::file::base64_encode(&bytes));
    }
    Ok(out)
}
