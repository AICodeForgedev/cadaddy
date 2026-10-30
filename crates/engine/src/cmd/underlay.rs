//! PDF underlays: PDFATTACH (and ATTACH of a .pdf) places a page of a PDF as a reference that is
//! drawn, not converted; PDFCLIP clips it, PDFADJUST sets fade, contrast and monochrome, PDFLAYERS
//! turns the page's layers on and off. Snapping to the page's geometry follows PDFOSNAP (a drawing
//! setting, on by default).
//!
//! DWF underlays (DWFATTACH) are the same entity on a sheet of a DWFx or DWF 6 file; DWFOUT and
//! DWFXOUT write those files. DGN underlays (DGNATTACH, `dgn.rs`) are the same entity too.
//!
//! Pages are read through the io hooks' PDF, DWF or drawing reader (at 1 drawing unit per sheet
//! inch) and kept in a cache shared with the renderer ([`load_page`]); after a drawing is opened
//! again (or the cache is emptied) each page is read again by its file's type.

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

/// The underlay commands' own functions, for DGNCLIP, DGNADJUST and DGNLAYERS.
/// The DGN command of the same name (`cmd`: its own, for its messages).
pub(crate) fn clip(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    run_clip(s, p, cmd)
}
/// The DGN command of the same name (`cmd`: its own, for its messages).
pub(crate) fn adjust(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    run_adjust(s, p, cmd)
}
/// The DGN command of the same name (`cmd`: its own, for its messages).
pub(crate) fn layers(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    run_layers(s, p, cmd)
}

pub fn specs() -> Vec<CommandSpec> {
    // The renderer draws underlays with the pages this module reads.
    cadaddy_render::set_underlay_loader(load_page);
    vec![
        CommandSpec::new("pdfattach", "PDF Underlay...", run_attach)
            .menu(&["Insert", "PDF Underlay..."])
            .params("{path, page?: 1, at?: [x,y] (default 0,0), scale?: 1 (drawing units per sheet inch), rotation?: degrees, name?} → {handle, size: [w,h] sheet inches, layers}")
            .interactive(|_| Ok(Box::new(AttachM::default()))),
        CommandSpec::new("pdfclip", "Clip PDF Underlay", |s, p| run_clip(s, p, "pdfclip"))
            .params("{handle?, rect?: [[x1,y1],[x2,y2]] | points?: [[x,y]…] (drawing coordinates), off?: bool, on?: bool, delete?: bool} (default: the selected underlay)"),
        CommandSpec::new("pdfadjust", "Adjust PDF Underlay", |s, p| run_adjust(s, p, "pdfadjust"))
            .params("{handles?, fade?: 0-80, contrast?: 20-100, monochrome?: bool} → {fade, contrast, monochrome}"),
        CommandSpec::new("pdflayers", "PDF Underlay Layers", |s, p| run_layers(s, p, "pdflayers"))
            .params("{handle?, off?: [layer], on?: [layer]} → {layers: [{name, on}]}: the page's layers"),
        CommandSpec::new("dwfattach", "DWF Underlay...", run_dwf_attach)
            .menu(&["Insert", "DWF Underlay..."])
            .params("{path: a .dwfx or .dwf, page?: 1, at?: [x,y] (default 0,0), scale?: 1 (drawing units per sheet inch), rotation?: degrees, name?, trueScale?: bool (a sheet CADaddy wrote: `at` and `scale` default to where it was drawn from, so it lies on the source's coordinates)} → {handle, size: [w,h] sheet inches, layers, pages, sheet?: the sheet's name, origin?, scale?: where the file says the sheet was drawn from}. DWF 6 files are read when they use WHIP!'s readable opcodes (as DWFOUT writes them); others are refused with a message")
            .interactive(|_| Ok(Box::new(DwfAttachM::default()))),
        CommandSpec::new("dwfout", "DWF Out", run_dwfout)
            .menu(&["File", "Export", "DWF..."])
            .alias(&["dwf"])
            .params("{path? (else returns base64 `data`), layouts?: [name | \"Model\"] (default: Model; one ePlot section each), scale?: drawing units per sheet inch for model space (default 1:1 in the drawing's units), fit?: bool (model space fitted to 24 x 18 in instead)} → {format: \"dwf\", path? | data, pages}")
            .noundo(),
        CommandSpec::new("dwfxout", "DWFx Out", run_dwfxout)
            .menu(&["File", "Export", "DWFx..."])
            .alias(&["dwfx"])
            .params("{path? (else returns base64 `data`), layouts?: [name | \"Model\"] (default: Model if it has objects, then every layout in tab order), scale?: drawing units per sheet inch for model space (default 1:1 in the drawing's units; layouts are always at paper size), fit?: bool (model space fitted to 24 x 18 in instead)} → {format: \"dwfx\", path? | data, pages}: one named page per sheet, one Canvas per layer, hyperlinks kept")
            .noundo(),
    ]
}

/// The page `page` (1-based) of the PDF, DWFx or DWF at `path` (a DGN's design) as a drawing in
/// sheet inches, read once.
pub fn load_page(path: &str, page: u32) -> Option<Arc<Drawing>> {
    let key = (path.to_string(), page.max(1));
    if let Ok(g) = PAGES.lock()
        && let Some(hit) = g.as_ref().and_then(|m| m.get(&key))
    {
        return hit.clone();
    }
    let lower = path.trim().to_ascii_lowercase();
    let got = if lower.ends_with(".dgn") {
        read_dgn(path).ok().map(|(d, _, _)| Arc::new(d))
    } else if lower.ends_with(".dwf") || lower.ends_with(".dwfx") {
        read_bytes(path).ok().and_then(|b| read_dwf_page(&b, path, page.max(1)).ok()).map(|(d, _)| Arc::new(d))
    } else {
        read_page(path, page.max(1)).ok().map(|(d, _)| Arc::new(d))
    };
    if let Ok(mut g) = PAGES.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        if m.len() >= MAX_CACHED_PAGES {
            m.clear();
        }
        m.insert(key, got.clone());
    }
    got
}

/// Forget every page read so far: each is read again from its file when next drawn (a file
/// changed on disk, or a drawing opened again).
pub fn forget_pages() {
    if let Ok(mut g) = PAGES.lock() {
        *g = None;
    }
}

/// Keep a page (or a DGN design) just read, for the renderer and snaps.
pub(crate) fn cache_page(path: &str, page: u32, d: Drawing) {
    if let Ok(mut g) = PAGES.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        if m.len() >= MAX_CACHED_PAGES {
            m.clear();
        }
        m.insert((path.to_string(), page.max(1)), Some(Arc::new(d)));
    }
}

/// A DGN design file read through the io hooks, moved so its extents start at the origin (an
/// underlay's "page"): the drawing, where its extents started, and their size.
pub(crate) fn read_dgn(path: &str) -> Result<(Drawing, Vec2, Vec2)> {
    let hooks = super::file::io().ok_or_else(|| bad("dgnattach", "file formats are not available in this build"))?;
    let bytes = read_bytes(path)?;
    let mut d = (hooks.read)(&bytes, path).map_err(|e| bad("dgnattach", format!("{path}: {e}")))?;
    let ext = d.extents(&cadaddy_doc::Space::Model);
    if ext.is_empty() {
        return Err(bad("dgnattach", format!("{path}: the design file has nothing in it")));
    }
    let to_origin = Mat3::translate(-ext.min);
    let hs: Vec<Handle> = d.model.iter().map(|e| e.handle).collect();
    for h in hs {
        let _ = d.modify_entity(h, |e| e.kind.transform(&to_origin));
    }
    Ok((d, ext.min, Vec2::new(ext.width().max(1e-9), ext.height().max(1e-9))))
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
    let h = s.add_entity(EntityKind::Underlay(Box::new(u)))?;
    Ok(json!({ "handle": h.hex(), "size": [size.x, size.y], "layers": layers }))
}

/// The underlays a command acts on: `handle`/`handles`, else the selected ones.
fn underlays(s: &Session, p: &Value, cmd: &str) -> Result<Vec<Handle>> {
    let d = s.doc()?;
    let hs: Vec<Handle> = targets(s, p)?.into_iter().filter(|h| d.entity(*h).is_some_and(|e| matches!(e.kind, EntityKind::Underlay(_)))).collect();
    if hs.is_empty() {
        return Err(bad(cmd, "select an underlay (PDF, DWF or DGN)"));
    }
    Ok(hs)
}

fn underlay_of(s: &Session, h: Handle) -> Result<Underlay> {
    match s.doc()?.entity(h).map(|e| &e.kind) {
        Some(EntityKind::Underlay(u)) => Ok(u.as_ref().clone()),
        _ => Err(bad("pdf", "not an underlay")),
    }
}

fn store(s: &mut Session, h: Handle, u: Underlay) -> Result<()> {
    s.doc_mut()?.modify_entity(h, |e| e.kind = EntityKind::Underlay(Box::new(u)))?;
    Ok(())
}

/// PDFCLIP: a new boundary (rectangle or polygon in drawing coordinates), clipping on or off, or
/// the boundary deleted.
fn run_clip(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    let hs = underlays(s, p, cmd)?;
    let h = *hs.first().ok_or_else(|| bad(cmd, "select an underlay (PDF or DGN)"))?;
    let mut u = underlay_of(s, h)?;
    let to_page = u.transform().inverse().ok_or_else(|| bad(cmd, "the underlay has no area"))?;
    let pts: Option<Vec<Vec2>> = match (p.get("rect"), p.get("points")) {
        (Some(r), _) => {
            let c =
                points_param(&json!({ "p": r }), "p").filter(|c| c.len() == 2).ok_or_else(|| bad(cmd, "`rect` is two corners [[x1,y1],[x2,y2]]"))?;
            Some(c)
        }
        (None, Some(_)) => Some(points_param(p, "points").filter(|c| c.len() >= 3).ok_or_else(|| bad(cmd, "`points` needs three or more points"))?),
        _ => None,
    };
    if let Some(pts) = pts {
        if pts.iter().any(|q| !q.is_finite()) {
            return Err(bad(cmd, "points must be finite"));
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
fn run_adjust(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    let hs = underlays(s, p, cmd)?;
    let pct = |k: &str, lo: f64, hi: f64| -> Result<Option<u8>> {
        match p.get(k) {
            None => Ok(None),
            Some(v) => {
                let x = v.as_f64().filter(|x| x.is_finite() && (lo..=hi).contains(x)).ok_or_else(|| bad(cmd, format!("`{k}` must be {lo}-{hi}")))?;
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
fn run_layers(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    let hs = underlays(s, p, cmd)?;
    let h = *hs.first().ok_or_else(|| bad(cmd, "select an underlay (PDF or DGN)"))?;
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
    // PDFOSNAP for PDF underlays, DGNOSNAP for DGN ones, DWFOSNAP for DWF ones (each on unless 0).
    let (pdf, dgn, dwf) = (d.header.i64("PDFOSNAP", 1) != 0, d.header.i64("DGNOSNAP", 1) != 0, d.header.i64("DWFOSNAP", 1) != 0);
    if !pdf && !dgn && !dwf {
        return Vec::new();
    }
    let Some(store) = d.space(space) else { return Vec::new() };
    let probe = Bounds2::new(cursor - Vec2::new(aperture, aperture), cursor + Vec2::new(aperture, aperture));
    store
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Underlay(u)
                if (if u.is_dgn() {
                    dgn
                } else if u.is_dwf() {
                    dwf
                } else {
                    pdf
                }) && Bounds2::from_points(u.corners()).intersects(&probe) =>
            {
                Some((load_page(&u.path, u.page)?, u.transform()))
            }
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
            // Enter with no file ends the command.
            let t = match i {
                Input::Text(t) => t,
                Input::Enter => return Ok(Step::Cancel),
                _ => return Ok(Step::Continue),
            };
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

// ---------- DWF/DWFx underlay and export ----------

/// DWFATTACH: place a sheet of a DWFx or DWF file as an underlay. With `trueScale`, a sheet
/// CADaddy wrote is placed where it was drawn from (its recorded origin and scale), so its
/// geometry lies on the source drawing's coordinates.
fn run_dwf_attach(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(|t| t.trim().trim_matches('"').to_string()).ok_or_else(|| bad("dwfattach", "`path` is required"))?;
    let page = p.get("page").and_then(Value::as_u64).map(|n| u32::try_from(n.clamp(1, 100_000)).unwrap_or(1)).unwrap_or(1);
    let bytes = read_bytes(&path)?;
    let (d, report) = read_dwf_page(&bytes, &path, page)?;
    let recorded = |k: &str| report.get(k).filter(|_| bool_or(p, "trueScale", false));
    let at = match (point_param(p, "at"), recorded("origin").and_then(|o| Some(Vec2::new(o.get(0)?.as_f64()?, o.get(1)?.as_f64()?)))) {
        (Some(a), _) => a,
        (None, Some(o)) => o,
        (None, None) => Vec2::ZERO,
    };
    let scale = match (p.get("scale").and_then(Value::as_f64), recorded("scale").and_then(Value::as_f64)) {
        (Some(k), _) => k,
        (None, Some(k)) => k,
        (None, None) => 1.0,
    };
    if !(scale.is_finite() && scale > 0.0 && scale < 1e9) {
        return Err(bad("dwfattach", "`scale` must be a positive number"));
    }
    let rot = f64_or(p, "rotation", 0.0).to_radians();
    if !rot.is_finite() || !at.is_finite() {
        return Err(bad("dwfattach", "`at` and `rotation` must be finite"));
    }
    let sheet = report.get("sheet").and_then(Value::as_array);
    let dim = |i: usize| sheet.and_then(|a| a.get(i)).and_then(Value::as_f64).filter(|x| x.is_finite() && *x > 0.0);
    let ext = d.extents(&cadaddy_doc::Space::Model);
    let size = match (dim(0), dim(1)) {
        (Some(w), Some(h)) => Vec2::new(w, h),
        _ if !ext.is_empty() => ext.max,
        _ => Vec2::new(8.5, 11.0),
    };
    let layers: Vec<String> = d.layers.iter().map(|l| l.name.clone()).collect();
    cache_page(&path, page, d);

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
    let h = s.add_entity(EntityKind::Underlay(Box::new(u)))?;
    let mut out = json!({ "handle": h.hex(), "size": [size.x, size.y], "layers": layers, "pages": report.get("pages").cloned().unwrap_or(json!(1)) });
    if let Some(n) = report.get("name") {
        out["sheet"] = n.clone();
    }
    for k in ["origin", "scale"] {
        if let Some(v) = report.get(k) {
            out[k] = v.clone();
        }
    }
    Ok(out)
}

/// A sheet of a DWFx or DWF 6 file, through the io hooks' DWF reader (which tells a DWFx from a
/// DWF by its content): the drawing and the reader's report.
fn read_dwf_page(bytes: &[u8], path: &str, page: u32) -> Result<(Drawing, Value)> {
    let hooks = super::file::io().ok_or_else(|| bad("dwfattach", "file formats are not available in this build"))?;
    let read = hooks.read_dwf.ok_or_else(|| bad("dwfattach", "DWF reading is not available in this build"))?;
    read(bytes, page).map_err(|e| bad("dwfattach", format!("{path}: {e}")))
}

/// The sheets DWFOUT/DWFXOUT write: `layouts`, else `default`.
fn out_spaces(cmd: &str, d: &Drawing, p: &Value, model_only: bool) -> Result<Vec<cadaddy_doc::Space>> {
    match p.get("layouts") {
        Some(v) => {
            let a = v.as_array().ok_or_else(|| bad(cmd, "`layouts` must be a list of layout names"))?;
            let spaces: Vec<cadaddy_doc::Space> = a
                .iter()
                .map(|n| {
                    let n = n.as_str().ok_or_else(|| bad(cmd, "`layouts` must be a list of layout names"))?;
                    if n.eq_ignore_ascii_case("model") {
                        return Ok(cadaddy_doc::Space::Model);
                    }
                    d.layout(n).map(|l| cadaddy_doc::Space::Paper(l.name.clone())).ok_or_else(|| bad(cmd, format!("no layout `{n}`")))
                })
                .collect::<Result<_>>()?;
            if spaces.is_empty() {
                return Err(bad(cmd, "`layouts` names no sheet"));
            }
            Ok(spaces)
        }
        None if model_only => Ok(vec![cadaddy_doc::Space::Model]),
        None => {
            let mut out = Vec::new();
            if !d.model.is_empty() {
                out.push(cadaddy_doc::Space::Model);
            }
            let mut layouts: Vec<_> = d.layouts.iter().collect();
            layouts.sort_by_key(|l| l.tab_order);
            for l in layouts {
                out.push(cadaddy_doc::Space::Paper(l.name.clone()));
            }
            if out.is_empty() {
                out.push(cadaddy_doc::Space::Model);
            }
            Ok(out)
        }
    }
}

/// DWFOUT / DWFXOUT: the sheets as one DWF 6 or DWFx file, written or returned.
fn write_sheets(s: &mut Session, p: &Value, cmd: &str, format: &str, model_only: bool) -> Result<Value> {
    let hooks = super::file::io().ok_or_else(|| bad(cmd, "file formats are not available in this build"))?;
    let write = hooks.write_dwfx.ok_or_else(|| bad(cmd, "DWF writing is not available in this build"))?;
    if let Some(k) = p.get("scale")
        && !k.as_f64().is_some_and(|k| k.is_finite() && k > 0.0)
    {
        return Err(bad(cmd, "`scale` must be a positive number of drawing units per sheet inch"));
    }
    let d = s.doc()?;
    let spaces = out_spaces(cmd, d, p, model_only)?;
    let opts = json!({ "format": format, "scale": p.get("scale"), "fit": p.get("fit").and_then(Value::as_bool).unwrap_or(false) });
    let bytes = write(d, &spaces, &opts).map_err(|e| bad(cmd, e))?;
    let pages = spaces.len();
    match str_param(p, "path") {
        Some(path) => {
            super::exchange::write_file(cmd, path, &bytes)?;
            Ok(json!({ "format": format, "path": path, "pages": pages, "bytes": bytes.len() }))
        }
        None => Ok(json!({ "format": format, "data": super::file::base64_encode(&bytes), "pages": pages, "bytes": bytes.len() })),
    }
}

/// DWFOUT: DWF 6, model space unless `layouts` names the sheets.
fn run_dwfout(s: &mut Session, p: &Value) -> Result<Value> {
    write_sheets(s, p, "dwfout", "dwf", true)
}

/// DWFXOUT: DWFx, one page per sheet.
fn run_dwfxout(s: &mut Session, p: &Value) -> Result<Value> {
    write_sheets(s, p, "dwfxout", "dwfx", false)
}

/// DWFATTACH interactive: file, page, insertion point, scale, rotation.
#[derive(Default)]
struct DwfAttachM {
    path: Option<String>,
    page: Option<u32>,
    at: Option<Vec2>,
    scale: Option<f64>,
}

impl Interactive for DwfAttachM {
    fn name(&self) -> &'static str {
        "DWFATTACH"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (&self.path, self.page, self.at, self.scale) {
            (None, ..) => Prompt::new("Enter path of DWF/DWFx file to attach", Accept::TEXT),
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
            let t = match i {
                Input::Text(t) => t,
                Input::Enter => return Ok(Step::Cancel),
                _ => return Ok(Step::Continue),
            };
            let t = t.trim().trim_matches('"').to_string();
            if t.is_empty() {
                return Ok(Step::Cancel);
            }
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
                Input::Text(_) => number(&i).ok_or_else(|| bad("dwfattach", "Requires a page number of 1 or more."))?,
                _ => return Ok(Step::Continue),
            };
            if !(1.0..=100_000.0).contains(&n) {
                return Err(bad("dwfattach", "Requires a page number of 1 or more."));
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
                    number(&i).filter(|n| n.is_finite() && *n > 0.0).ok_or_else(|| bad("dwfattach", "Requires a positive scale factor."))?
                }
                _ => return Ok(Step::Continue),
            };
            self.scale = Some(k);
            return Ok(Step::Continue);
        }
        let at = self.at.unwrap_or(Vec2::ZERO);
        let rot = match i {
            Input::Enter => 0.0,
            Input::Text(_) => number(&i).filter(|a| a.is_finite()).ok_or_else(|| bad("dwfattach", "Requires an angle."))?,
            Input::Point(q) => (q - at).angle().to_degrees(),
            _ => return Ok(Step::Continue),
        };
        let p = json!({
            "path": self.path, "page": self.page,
            "at": [at.x, at.y], "scale": self.scale, "rotation": rot
        });
        run_dwf_attach(s, &p)?;
        Ok(Step::Done)
    }
}
