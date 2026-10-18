//! PDF underlays: PDFATTACH (and ATTACH of a .pdf) places a page of a PDF as a reference that is
//! drawn, not converted; PDFCLIP clips it, PDFADJUST sets fade, contrast and monochrome, PDFLAYERS
//! turns the page's layers on and off. Snapping to the page's geometry follows PDFOSNAP (a drawing
//! setting, on by default).
//!
//! Pages are read through the io hooks' PDF reader (at 1 drawing unit per sheet inch) and kept in
//! a cache shared with the renderer ([`load_page`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cadaddy_doc::{Drawing, EntityKind, Handle, Underlay};
use cadaddy_geom::{Bounds2, Mat3, Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Pages kept in memory; the cache is emptied when it grows past this.
const MAX_CACHED_PAGES: usize = 32;
/// Most clip boundary vertices and layer names a command takes.
const MAX_POINTS: usize = 100_000;

type PageKey = (String, u32);

/// Pages read so far (and pages that couldn't be read, as `None`), by file and page.
static PAGES: Mutex<Option<HashMap<PageKey, Option<Arc<Drawing>>>>> = Mutex::new(None);

pub fn specs() -> Vec<CommandSpec> {
    // The renderer draws underlays with the pages this module reads.
    cadaddy_render::set_underlay_loader(load_page);
    vec![
        CommandSpec::new("pdfattach", "PDF Underlay...", run_attach)
            .menu(&["Insert", "PDF Underlay..."])
            .params("{path, page?: 1, at?: [x,y] (default 0,0), scale?: 1 (drawing units per sheet inch), rotation?: degrees, name?} → {handle, size: [w,h] sheet inches, layers}")
            .interactive(|_| Ok(Box::new(AttachM::default()))),
        CommandSpec::new("pdfclip", "Clip PDF Underlay", run_clip)
            .params("{handle?, rect?: [[x1,y1],[x2,y2]] | points?: [[x,y]…] (drawing coordinates), off?: bool, on?: bool, delete?: bool} (default: the selected underlay)"),
        CommandSpec::new("pdfadjust", "Adjust PDF Underlay", run_adjust)
            .params("{handles?, fade?: 0-80, contrast?: 20-100, monochrome?: bool} → {fade, contrast, monochrome}"),
        CommandSpec::new("pdflayers", "PDF Underlay Layers", run_layers)
            .params("{handle?, off?: [layer], on?: [layer]} → {layers: [{name, on}]}: the page's layers"),
    ]
}

/// The page `page` (1-based) of the PDF at `path` as a drawing in sheet inches, read once.
pub fn load_page(path: &str, page: u32) -> Option<Arc<Drawing>> {
    let key = (path.to_string(), page.max(1));
    if let Ok(g) = PAGES.lock()
        && let Some(hit) = g.as_ref().and_then(|m| m.get(&key))
    {
        return hit.clone();
    }
    let got = read_page(path, page.max(1)).ok().map(|(d, _)| Arc::new(d));
    if let Ok(mut g) = PAGES.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        if m.len() >= MAX_CACHED_PAGES {
            m.clear();
        }
        m.insert(key, got.clone());
    }
    got
}

/// Read a page through the io hooks' PDF reader: the drawing and the reader's report.
fn read_page(path: &str, page: u32) -> Result<(Drawing, Value)> {
    let hooks = super::file::io().ok_or_else(|| bad("pdfattach", "file formats are not available in this build"))?;
    let read = hooks.read_pdf.ok_or_else(|| bad("pdfattach", "PDF reading is not available in this build"))?;
    let bytes = read_bytes(path)?;
    let opts = json!({ "page": page, "scale": 1.0, "layout": false, "join": false, "markups": true, "images": true });
    read(&bytes, &opts).map_err(|e| bad("pdfattach", format!("{path}: {e}")))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_bytes(path: &str) -> Result<Vec<u8>> {
    crate::readfile::read(path).map_err(|e| bad("pdfattach", format!("{path}: {e}")))
}

#[cfg(target_arch = "wasm32")]
fn read_bytes(path: &str) -> Result<Vec<u8>> {
    Err(bad("pdfattach", format!("{path}: paths are not available on the web")))
}

/// PDFATTACH: place a page.
pub(crate) fn run_attach(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(|t| t.trim().trim_matches('"').to_string()).ok_or_else(|| bad("pdfattach", "`path` is required"))?;
    let page = p.get("page").and_then(Value::as_u64).map(|n| u32::try_from(n.clamp(1, 100_000)).unwrap_or(1)).unwrap_or(1);
    let at = point_param(p, "at").unwrap_or(Vec2::ZERO);
    let scale = f64_or(p, "scale", 1.0);
    if !(scale.is_finite() && scale > 0.0 && scale < 1e9) {
        return Err(bad("pdfattach", "`scale` must be a positive number"));
    }
    let rot = f64_or(p, "rotation", 0.0).to_radians();
    if !rot.is_finite() || !at.is_finite() {
        return Err(bad("pdfattach", "`at` and `rotation` must be finite"));
    }
    let (d, report) = read_page(&path, page)?;
    let sheet = report.get("sheet").and_then(Value::as_array);
    let dim = |i: usize| sheet.and_then(|a| a.get(i)).and_then(Value::as_f64).filter(|x| x.is_finite() && *x > 0.0);
    let ext = d.extents(&cadaddy_doc::Space::Model);
    let size = match (dim(0), dim(1)) {
        (Some(w), Some(h)) => Vec2::new(w, h),
        _ if !ext.is_empty() => ext.max,
        _ => Vec2::new(8.5, 11.0),
    };
    let layers: Vec<String> = d.layers.iter().map(|l| l.name.clone()).collect();
    if let Ok(mut g) = PAGES.lock() {
        g.get_or_insert_with(HashMap::new).insert((path.clone(), page), Some(Arc::new(d)));
    }
    let (sn, cs) = rot.sin_cos();
    let u = Underlay {
        insert: at.to3(0.0),
        u: Vec3::new(cs * scale, sn * scale, 0.0),
        v: Vec3::new(-sn * scale, cs * scale, 0.0),
        size,
        path: path.clone(),
        page,
        name: str_param(p, "name").unwrap_or("").to_string(),
        clip: Vec::new(),
        clipping: false,
        fade: 25,
        contrast: 20,
        monochrome: false,
        layers_off: Vec::new(),
    };
    let h = s.add_entity(EntityKind::Underlay(u))?;
    Ok(json!({ "handle": h.hex(), "size": [size.x, size.y], "layers": layers }))
}

/// The underlays a command acts on: `handle`/`handles`, else the selected ones.
fn underlays(s: &Session, p: &Value, cmd: &str) -> Result<Vec<Handle>> {
    let d = s.doc()?;
    let hs: Vec<Handle> = targets(s, p)?.into_iter().filter(|h| d.entity(*h).is_some_and(|e| matches!(e.kind, EntityKind::Underlay(_)))).collect();
    if hs.is_empty() {
        return Err(bad(cmd, "select a PDF underlay"));
    }
    Ok(hs)
}

fn underlay_of(s: &Session, h: Handle) -> Result<Underlay> {
    match s.doc()?.entity(h).map(|e| &e.kind) {
        Some(EntityKind::Underlay(u)) => Ok(u.clone()),
        _ => Err(bad("pdf", "not a PDF underlay")),
    }
}

fn store(s: &mut Session, h: Handle, u: Underlay) -> Result<()> {
    s.doc_mut()?.modify_entity(h, |e| e.kind = EntityKind::Underlay(u))?;
    Ok(())
}

/// PDFCLIP: a new boundary (rectangle or polygon in drawing coordinates), clipping on or off, or
/// the boundary deleted.
fn run_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = underlays(s, p, "pdfclip")?;
    let h = *hs.first().ok_or_else(|| bad("pdfclip", "select a PDF underlay"))?;
    let mut u = underlay_of(s, h)?;
    let to_page = u.transform().inverse().ok_or_else(|| bad("pdfclip", "the underlay has no area"))?;
    let pts: Option<Vec<Vec2>> = match (p.get("rect"), p.get("points")) {
        (Some(r), _) => {
            let c = points_param(&json!({ "p": r }), "p")
                .filter(|c| c.len() == 2)
                .ok_or_else(|| bad("pdfclip", "`rect` is two corners [[x1,y1],[x2,y2]]"))?;
            Some(c)
        }
        (None, Some(_)) => {
            Some(points_param(p, "points").filter(|c| c.len() >= 3).ok_or_else(|| bad("pdfclip", "`points` needs three or more points"))?)
        }
        _ => None,
    };
    if let Some(pts) = pts {
        if pts.iter().any(|q| !q.is_finite()) {
            return Err(bad("pdfclip", "points must be finite"));
        }
        u.clip = pts.into_iter().take(MAX_POINTS).map(|q| to_page.apply(q)).collect();
        u.clipping = true;
    }
    if bool_or(p, "delete", false) {
        u.clip.clear();
        u.clipping = false;
    }
    if let Some(on) = p.get("on").and_then(Value::as_bool).or_else(|| p.get("off").and_then(Value::as_bool).map(|o| !o)) {
        u.clipping = on && u.clip.len() >= 2;
    }
    let out = json!({ "handle": h.hex(), "clipping": u.clipping, "boundary": u.clip.iter().map(|q| [q.x, q.y]).collect::<Vec<_>>() });
    store(s, h, u)?;
    Ok(out)
}

/// PDFADJUST: fade, contrast and monochrome of the underlays.
fn run_adjust(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = underlays(s, p, "pdfadjust")?;
    let pct = |k: &str, lo: f64, hi: f64| -> Result<Option<u8>> {
        match p.get(k) {
            None => Ok(None),
            Some(v) => {
                let x = v
                    .as_f64()
                    .filter(|x| x.is_finite() && (lo..=hi).contains(x))
                    .ok_or_else(|| bad("pdfadjust", format!("`{k}` must be {lo}-{hi}")))?;
                Ok(Some(x.round() as u8))
            }
        }
    };
    let (fade, contrast) = (pct("fade", 0.0, 80.0)?, pct("contrast", 20.0, 100.0)?);
    let mono = p.get("monochrome").and_then(Value::as_bool);
    let mut last = Value::Null;
    for h in hs {
        let mut u = underlay_of(s, h)?;
        if let Some(f) = fade {
            u.fade = f;
        }
        if let Some(c) = contrast {
            u.contrast = c;
        }
        if let Some(m) = mono {
            u.monochrome = m;
        }
        last = json!({ "fade": u.fade, "contrast": u.contrast, "monochrome": u.monochrome });
        store(s, h, u)?;
    }
    Ok(last)
}

fn names(p: &Value, key: &str) -> Vec<String> {
    match p.get(key) {
        Some(Value::String(t)) => t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).take(MAX_POINTS).collect(),
        Some(Value::Array(a)) => {
            a.iter().filter_map(Value::as_str).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).take(MAX_POINTS).collect()
        }
        _ => Vec::new(),
    }
}

/// PDFLAYERS: the page's layers, with those named turned off or on.
fn run_layers(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = underlays(s, p, "pdflayers")?;
    let h = *hs.first().ok_or_else(|| bad("pdflayers", "select a PDF underlay"))?;
    let mut u = underlay_of(s, h)?;
    let page = load_page(&u.path, u.page);
    let all: Vec<String> = page.as_ref().map(|d| d.layers.iter().map(|l| l.name.clone()).collect()).unwrap_or_default();
    for n in names(p, "off") {
        if !u.layers_off.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
            u.layers_off.push(n);
        }
    }
    let on = names(p, "on");
    u.layers_off.retain(|x| !on.iter().any(|n| n.eq_ignore_ascii_case(x)));
    let list: Vec<Value> = all.iter().map(|n| json!({ "name": n, "on": !u.layers_off.iter().any(|x| x.eq_ignore_ascii_case(n)) })).collect();
    store(s, h, u)?;
    Ok(json!({ "layers": list }))
}

/// Snapping to underlay geometry (PDFOSNAP, a drawing setting, on unless 0): the underlays of
/// `space` whose frame is near `cursor`, with the page drawing and its placement.
pub(crate) fn near(d: &Drawing, space: &cadaddy_doc::Space, cursor: Vec2, aperture: f64) -> Vec<(Arc<Drawing>, Mat3)> {
    if d.header.i64("PDFOSNAP", 1) == 0 {
        return Vec::new();
    }
    let Some(store) = d.space(space) else { return Vec::new() };
    let probe = Bounds2::new(cursor - Vec2::new(aperture, aperture), cursor + Vec2::new(aperture, aperture));
    store
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Underlay(u) if Bounds2::from_points(u.corners()).intersects(&probe) => Some((load_page(&u.path, u.page)?, u.transform())),
            _ => None,
        })
        .take(16)
        .collect()
}

/// PDFATTACH typed: the file, the page, the insertion point, the scale and the rotation.
#[derive(Default)]
struct AttachM {
    path: Option<String>,
    page: Option<u32>,
    at: Option<Vec2>,
    scale: Option<f64>,
}

impl Interactive for AttachM {
    fn name(&self) -> &'static str {
        "PDFATTACH"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (&self.path, self.page, self.at, self.scale) {
            (None, ..) => Prompt::new("Enter path of PDF file to attach", Accept::TEXT),
            (Some(_), None, ..) => Prompt::new("Enter page number", Accept::NUMBER).default("1"),
            (Some(_), Some(_), None, _) => Prompt::new("Specify insertion point", Accept::POINT),
            (_, _, Some(_), None) => Prompt::new("Specify scale factor", Accept::NUMBER).default("1"),
            (_, _, Some(a), Some(_)) => Prompt::new("Specify rotation", Accept::POINT_OR_NUMBER).default("0").base(a),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        if self.path.is_none() {
            let Input::Text(t) = i else { return Ok(Step::Continue) };
            let t = t.trim().trim_matches('"').to_string();
            if t.is_empty() {
                return Ok(Step::Cancel);
            }
            // A missing file is reported and asked for again.
            read_page(&t, 1)?;
            self.path = Some(t);
            return Ok(Step::Continue);
        }
        let number = |i: &Input| match i {
            Input::Text(t) => t.trim().parse::<f64>().ok(),
            _ => None,
        };
        if self.page.is_none() {
            let n = match i {
                Input::Enter => 1.0,
                Input::Text(_) => number(&i).ok_or_else(|| bad("pdfattach", "Requires a page number of 1 or more."))?,
                _ => return Ok(Step::Continue),
            };
            if !(1.0..=100_000.0).contains(&n) {
                return Err(bad("pdfattach", "Requires a page number of 1 or more."));
            }
            self.page = Some(n.round() as u32);
            return Ok(Step::Continue);
        }
        if self.at.is_none() {
            if let Input::Point(p) = i {
                self.at = Some(p);
            }
            return Ok(Step::Continue);
        }
        if self.scale.is_none() {
            let k = match i {
                Input::Enter => 1.0,
                Input::Text(_) => {
                    number(&i).filter(|n| n.is_finite() && *n > 0.0).ok_or_else(|| bad("pdfattach", "Requires a positive scale factor."))?
                }
                _ => return Ok(Step::Continue),
            };
            self.scale = Some(k);
            return Ok(Step::Continue);
        }
        let at = self.at.unwrap_or(Vec2::ZERO);
        let rot = match i {
            Input::Enter => 0.0,
            Input::Text(_) => number(&i).filter(|a| a.is_finite()).ok_or_else(|| bad("pdfattach", "Requires an angle."))?,
            Input::Point(q) => (q - at).angle().to_degrees(),
            _ => return Ok(Step::Continue),
        };
        let p = json!({ "path": self.path, "page": self.page, "at": [at.x, at.y], "scale": self.scale, "rotation": rot });
        run_attach(s, &p)?;
        Ok(Step::Done)
    }
}
