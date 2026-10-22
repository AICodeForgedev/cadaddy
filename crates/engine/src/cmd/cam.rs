//! CAM commands: 2-D profile toolpaths (`cam2dprofile`) and CNC-clean DXF export (`cncdxf`).
//!
//! `cam2dprofile` takes selected closed polylines and circles, applies tool-radius offset,
//! computes lead-in/out, tabs, depth passes and cut ordering, then returns the G-code and
//! optionally adds the toolpath preview to the drawing.
//!
//! `cncdxf` writes a DXF containing only polylines (arcs preserved as bulge vertices, no blocks,
//! no text, no dimensions), suitable for CNC controllers that read DXF directly.

use cadaddy_cam::gcode::{Dialect, GcodeOptions};
use cadaddy_cam::toolpath::{self, Move};
use cadaddy_cam::{CamJob, CutOrder, LeadKind, LeadParams, OffsetSide, TabParams, ToolParams};
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::{PolyVertex, Polyline, Vec2};
use serde_json::{Value, json};

use super::exchange::write_file;
use super::file::base64_encode;
use super::helpers::lwpoly;
use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("cam2dprofile", "2D Profile Toolpath", run_cam2dprofile)
            .menu(&["Manufacture", "2D Profile"])
            .params(
                "{handles?, toolDiameter: number, offset: \"inside\"|\"outside\"|\"on\", \
                 feedRate: number, plungeRate: number, spindleSpeed: number, depth: number, \
                 safeZ: number, stepDown?: number, tabWidth?: number, tabHeight?: number, \
                 tabCount?: number (0=none), leadIn?: {kind: \"arc\"|\"line\"|\"none\", length}, \
                 leadOut?: {kind, length}, order?: \"nearest\"|\"insideout\" (default insideout), \
                 dialect?: \"generic\"|\"linuxcnc\"|\"grbl\"|\"mach3\" (grbl: no G64, no % lines), \
                 metric?: bool (default true), \
                 path?: write G-code file, preview?: bool (add toolpath to drawing)} \
                 → {gcode, profiles, moves, previewHandles?}",
            )
            .enabled(has_selection),
        CommandSpec::new("cncdxf", "CNC DXF Export", run_cncdxf)
            .menu(&["Manufacture", "CNC DXF Export..."])
            .params(
                "{handles? (polylines, circles, arcs, lines and block references, exploded; \
                 pieces whose ends meet are joined; default: selection), \
                 path?: file path (else returns base64 `data`)} \
                 → {path | data, entities: count}",
            )
            .noundo()
            .enabled(has_selection),
    ]
}

// ── cam2dprofile ─────────────────────────────────────────────────────────────

fn run_cam2dprofile(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let doc = s.doc()?;

    // Extract closed profiles from the selected entities.
    let profiles = extract_profiles(doc, &hs)?;
    if profiles.is_empty() {
        return Err(bad("cam2dprofile", "no closed polylines or circles in the selection"));
    }

    // Build the CamJob from parameters.
    let job = parse_cam_job("cam2dprofile", p)?;
    let opts = parse_gcode_opts("cam2dprofile", p)?;

    // Compute toolpaths.
    let result = toolpath::compute(&profiles, &job).map_err(|e| bad("cam2dprofile", e.to_string()))?;

    // Generate G-code.
    let gcode = cadaddy_cam::gcode::emit(&result, &job, &opts).map_err(|e| bad("cam2dprofile", e.to_string()))?;

    // Write G-code to file if a path was given.
    if let Some(path) = str_param(p, "path") {
        write_file("cam2dprofile", path, gcode.as_bytes())?;
    }

    // Preview: add the offset polylines and toolpath lines to the drawing.
    let mut preview_handles = Vec::new();
    if bool_or(p, "preview", false) {
        for pl in &result.offset_profiles {
            let h = s.add_entity(lwpoly(pl.vertices.clone(), pl.closed))?;
            preview_handles.push(h.hex());
        }
        // Add toolpath moves as lightweight polylines on a "CAM-Toolpath" layer.
        for path_item in &result.paths {
            let tp_poly = moves_to_polyline(&path_item.moves);
            if !tp_poly.vertices.is_empty() {
                let h = s.add_entity(lwpoly(tp_poly.vertices, false))?;
                preview_handles.push(h.hex());
            }
        }
    }

    let move_count: usize = result.paths.iter().map(|p| p.moves.len()).sum();
    let mut reply = json!({
        "profiles": profiles.len(),
        "moves": move_count,
        "gcode": gcode,
        "dialect": format!("{:?}", opts.dialect).to_ascii_lowercase(),
        "lines": gcode.lines().count(),
    });
    if !preview_handles.is_empty() {
        reply["previewHandles"] = json!(preview_handles);
    }
    if let Some(path) = str_param(p, "path") {
        reply["path"] = json!(path);
        reply["message"] = json!(format!("G-code written to {path}"));
    }
    Ok(reply)
}

// ── cncdxf ───────────────────────────────────────────────────────────────────

fn run_cncdxf(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let doc = s.doc()?;

    // Collect polylines, circles, arcs and lines (block references exploded), then join pieces
    // whose ends meet.
    let pieces = extract_all_polylines(doc, &hs);
    let profiles = toolpath::clean_cnc_polylines(&pieces, JOIN_TOL);
    if profiles.is_empty() {
        return Err(bad("cncdxf", "no polylines, circles or arcs in the selection"));
    }

    // Build a minimal DXF: just LWPOLYLINE entities.
    // Every group code and value on its own line, no blank lines between entities (a blank line
    // shifts the code/value pairs for any DXF reader).
    let mut dxf = String::from("0\nSECTION\n2\nENTITIES\n");
    for pl in &profiles {
        dxf.push_str(&polyline_to_dxf(pl));
    }
    dxf.push_str("0\nENDSEC\n0\nEOF\n");
    let bytes = dxf.as_bytes();

    match str_param(p, "path") {
        Some(path) => {
            write_file("cncdxf", path, bytes)?;
            Ok(json!({ "path": path, "entities": profiles.len(), "bytes": bytes.len(), "message": format!("CNC DXF written to {path}") }))
        }
        None => Ok(json!({ "data": base64_encode(bytes), "entities": profiles.len() })),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Extract closed Polylines from entities (LwPolyline if closed, Circle → polyline).
fn extract_profiles(doc: &Drawing, handles: &[cadaddy_doc::Handle]) -> Result<Vec<Polyline>> {
    let mut profiles = Vec::new();
    for h in handles {
        let Some(entity) = doc.entity(*h) else { continue };
        match &entity.kind {
            EntityKind::LwPolyline(pl) if pl.closed && pl.vertices.len() >= 3 => {
                profiles.push(Polyline { vertices: pl.vertices.clone(), closed: true });
            }
            EntityKind::Circle(c) => {
                // Approximate circle as a closed polyline with bulge arcs (4 quarter-arcs).
                profiles.push(circle_to_polyline(c));
            }
            _ => {} // skip non-profile entities silently
        }
    }
    Ok(profiles)
}

/// End points closer than this are joined in a CNC DXF.
const JOIN_TOL: f64 = 1e-6;
/// Block references nested deeper than this are not exploded further.
const MAX_NEST: usize = 16;
/// Pieces collected for one CNC DXF at most (hostile block nesting).
const MAX_PIECES: usize = 1_000_000;

/// All polylines for CNC DXF: closed and open, circles and arcs → polylines, block references
/// exploded (with their insertion transform, nested ones too).
fn extract_all_polylines(doc: &Drawing, handles: &[cadaddy_doc::Handle]) -> Vec<Polyline> {
    let mut out = Vec::new();
    for h in handles {
        let Some(entity) = doc.entity(*h) else { continue };
        collect_polylines(doc, entity, 0, &mut out);
    }
    out
}

fn collect_polylines(doc: &Drawing, entity: &cadaddy_doc::Entity, depth: usize, out: &mut Vec<Polyline>) {
    if out.len() >= MAX_PIECES {
        return;
    }
    match &entity.kind {
        EntityKind::LwPolyline(pl) if pl.vertices.len() >= 2 => {
            out.push(Polyline { vertices: pl.vertices.clone(), closed: pl.closed });
        }
        EntityKind::Circle(c) => {
            out.push(circle_to_polyline(c));
        }
        EntityKind::Arc(a) => {
            out.push(arc_to_polyline(a));
        }
        EntityKind::Line(l) => {
            let p1 = Vec2::new(l.a.x, l.a.y);
            let p2 = Vec2::new(l.b.x, l.b.y);
            out.push(Polyline { vertices: vec![PolyVertex::new(p1), PolyVertex::new(p2)], closed: false });
        }
        EntityKind::Insert(_) if depth < MAX_NEST => {
            for e in super::modify::explode_kind(doc, entity).unwrap_or_default() {
                collect_polylines(doc, &e, depth + 1, out);
            }
        }
        _ => {}
    }
}

fn circle_to_polyline(c: &cadaddy_doc::Circle) -> Polyline {
    let cx = c.center.x;
    let cy = c.center.y;
    let r = c.radius;
    // Four quarter-arcs with bulge = tan(pi/8) ≈ 0.4142.
    let b = cadaddy_geom::arc_to_bulge(std::f64::consts::FRAC_PI_2);
    Polyline {
        vertices: vec![
            PolyVertex::with_bulge(Vec2::new(cx + r, cy), b),
            PolyVertex::with_bulge(Vec2::new(cx, cy + r), b),
            PolyVertex::with_bulge(Vec2::new(cx - r, cy), b),
            PolyVertex::with_bulge(Vec2::new(cx, cy - r), b),
        ],
        closed: true,
    }
}

fn arc_to_polyline(a: &cadaddy_doc::Arc) -> Polyline {
    let geom_arc = cadaddy_geom::Arc::new(Vec2::new(a.center.x, a.center.y), a.radius, a.start, a.end);
    let start = geom_arc.start_point();
    let end = geom_arc.end_point();
    let bulge = cadaddy_geom::arc_to_bulge(geom_arc.sweep());
    Polyline { vertices: vec![PolyVertex::with_bulge(start, bulge), PolyVertex::new(end)], closed: false }
}

/// Convert toolpath moves to a 2D polyline (for preview; Z is ignored).
fn moves_to_polyline(moves: &[Move]) -> Polyline {
    let mut vertices = Vec::new();
    for m in moves {
        match m {
            Move::Rapid { x, y, .. } | Move::Feed { x, y, .. } => {
                vertices.push(PolyVertex::new(Vec2::new(*x, *y)));
            }
            Move::ArcCw { x, y, .. } | Move::ArcCcw { x, y, .. } => {
                // For preview, just add the endpoint (full arc rendering would need bulge computation).
                vertices.push(PolyVertex::new(Vec2::new(*x, *y)));
            }
        }
    }
    Polyline { vertices, closed: false }
}

/// Tabs per profile at most.
const MAX_TABS: u64 = 1000;

/// String param `key` as one of `allowed` (`None` when absent); anything else is refused with
/// the allowed values, never turned into a default.
fn enum_param<'a>(cmd: &str, p: &'a Value, key: &str, allowed: &[&str]) -> Result<Option<&'a str>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(v)) if allowed.contains(&v.as_str()) => Ok(Some(v.as_str())),
        Some(v) => Err(bad(cmd, format!("`{key}`: {v} is not one of {}", allowed.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(", ")))),
    }
}

/// Number param `key` (default when absent); a non-number is refused.
fn num_param(cmd: &str, p: &Value, key: &str, default: f64) -> Result<f64> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad(cmd, format!("`{key}` must be a number, got {v}"))),
    }
}

fn parse_cam_job(cmd: &str, p: &Value) -> Result<CamJob> {
    let diameter = num_param(cmd, p, "toolDiameter", 6.0)?;
    let offset = match enum_param(cmd, p, "offset", &["inside", "outside", "on"])? {
        Some("inside") => OffsetSide::Inside,
        Some("on") => OffsetSide::On,
        _ => OffsetSide::Outside,
    };
    let feed_rate = num_param(cmd, p, "feedRate", 500.0)?;
    let plunge_rate = num_param(cmd, p, "plungeRate", 100.0)?;
    let spindle_speed = num_param(cmd, p, "spindleSpeed", 12000.0)?;
    let depth = num_param(cmd, p, "depth", 5.0)?;
    let safe_z = num_param(cmd, p, "safeZ", 10.0)?;
    let step_down = num_param(cmd, p, "stepDown", 0.0)?;
    if depth <= 0.0 {
        return Err(bad(cmd, format!("`depth` must be above 0 (the cut depth below the surface, positive down), got {depth}")));
    }
    if step_down < 0.0 {
        return Err(bad(cmd, format!("`stepDown` must be 0 (full depth in one pass) or positive, got {step_down}")));
    }
    if feed_rate <= 0.0 {
        return Err(bad(cmd, format!("`feedRate` must be above 0, got {feed_rate}")));
    }
    if plunge_rate <= 0.0 {
        return Err(bad(cmd, format!("`plungeRate` must be above 0, got {plunge_rate}")));
    }
    if safe_z <= 0.0 {
        return Err(bad(cmd, format!("`safeZ` must be above 0 (rapid moves clear the work surface at Z 0), got {safe_z}")));
    }

    let lead_in = parse_lead(cmd, p, "leadIn")?;
    let lead_out = parse_lead(cmd, p, "leadOut")?;

    let tab_count = match p.get("tabCount") {
        None | Some(Value::Null) => 0,
        Some(v) => match v.as_u64() {
            Some(n) if n <= MAX_TABS => n as usize,
            _ => return Err(bad(cmd, format!("`tabCount` must be a whole number from 0 to {MAX_TABS}, got {v}"))),
        },
    };
    let tabs = TabParams { width: num_param(cmd, p, "tabWidth", 3.0)?, height: num_param(cmd, p, "tabHeight", 1.0)?, count: tab_count };

    let order = match enum_param(cmd, p, "order", &["nearest", "insideout"])? {
        Some("nearest") => CutOrder::Nearest,
        _ => CutOrder::InsideOut,
    };

    Ok(CamJob {
        tool: ToolParams { diameter, feed_rate, plunge_rate, spindle_speed, depth, safe_z, step_down },
        offset,
        lead_in,
        lead_out,
        tabs,
        order,
    })
}

fn parse_lead(cmd: &str, p: &Value, key: &str) -> Result<LeadParams> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(LeadParams { kind: LeadKind::None, length: 0.0 }),
        Some(v) if v.is_object() => {
            let kind = match enum_param(cmd, v, "kind", &["arc", "line", "none"]).map_err(|_| {
                bad(cmd, format!("`{key}.kind`: {} is not one of \"arc\", \"line\", \"none\"", v.get("kind").cloned().unwrap_or(Value::Null)))
            })? {
                Some("arc") => LeadKind::Arc,
                Some("line") => LeadKind::Line,
                _ => LeadKind::None,
            };
            let length = num_param(cmd, v, "length", 2.0)?;
            if kind != LeadKind::None && length <= 0.0 {
                return Err(bad(cmd, format!("`{key}.length` must be above 0, got {length}")));
            }
            Ok(LeadParams { kind, length })
        }
        Some(v) => Err(bad(cmd, format!("`{key}` must be an object {{kind, length}}, got {v}"))),
    }
}

fn parse_gcode_opts(cmd: &str, p: &Value) -> Result<GcodeOptions> {
    let dialect = match enum_param(cmd, p, "dialect", &["generic", "linuxcnc", "grbl", "mach3"])? {
        Some("linuxcnc") => Dialect::LinuxCnc,
        Some("grbl") => Dialect::Grbl,
        Some("mach3") => Dialect::Mach3,
        _ => Dialect::Generic,
    };
    Ok(GcodeOptions { dialect, metric: bool_or(p, "metric", true), ..GcodeOptions::default() })
}

/// Write an LWPOLYLINE entity as DXF ASCII text.
fn polyline_to_dxf(pl: &Polyline) -> String {
    let mut s = String::new();
    s.push_str("0\nLWPOLYLINE\n");
    s.push_str("100\nAcDbEntity\n");
    s.push_str("8\n0\n"); // layer 0
    s.push_str("100\nAcDbPolyline\n");
    s.push_str(&format!("90\n{}\n", pl.vertices.len()));
    s.push_str(if pl.closed { "70\n1\n" } else { "70\n0\n" });
    for v in &pl.vertices {
        s.push_str(&format!("10\n{:.6}\n20\n{:.6}\n", v.p.x, v.p.y));
        if v.bulge.abs() > 1e-12 {
            s.push_str(&format!("42\n{:.6}\n", v.bulge));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, Drawing, Handle, LwPolyline, Space};

    fn drawing_with_square() -> (Drawing, Vec<Handle>) {
        let mut doc = Drawing::new_metric();
        let vs = vec![
            PolyVertex::new(Vec2::new(0.0, 0.0)),
            PolyVertex::new(Vec2::new(10.0, 0.0)),
            PolyVertex::new(Vec2::new(10.0, 10.0)),
            PolyVertex::new(Vec2::new(0.0, 10.0)),
        ];
        let kind = EntityKind::LwPolyline(LwPolyline { vertices: vs, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false });
        let h = doc.add(&Space::Model, Common::default(), kind).unwrap_or(Handle(0));
        (doc, vec![h])
    }

    #[test]
    fn extract_profiles_from_lwpolyline() {
        let (doc, hs) = drawing_with_square();
        let profiles = extract_profiles(&doc, &hs);
        assert!(profiles.is_ok());
        assert_eq!(profiles.as_ref().ok().map(|p| p.len()), Some(1));
        let pl = &profiles.ok().unwrap_or_default()[0];
        assert!(pl.closed);
        assert_eq!(pl.vertices.len(), 4);
    }

    #[test]
    fn extract_profiles_from_circle() {
        let mut doc = Drawing::new_metric();
        let kind = EntityKind::Circle(cadaddy_doc::Circle { center: cadaddy_geom::Vec3::new(5.0, 5.0, 0.0), radius: 3.0 });
        let h = doc.add(&Space::Model, Common::default(), kind).unwrap_or(Handle(0));
        let profiles = extract_profiles(&doc, &[h]);
        assert!(profiles.is_ok());
        assert_eq!(profiles.as_ref().ok().map(|p| p.len()), Some(1));
        assert!(profiles.ok().unwrap_or_default()[0].closed);
    }

    #[test]
    fn parse_cam_job_defaults() {
        let p = json!({});
        let job = parse_cam_job("test", &p);
        assert!(job.is_ok());
        let j = job.ok().unwrap_or_else(|| std::process::abort());
        assert!((j.tool.diameter - 6.0).abs() < 1e-9);
        assert!(matches!(j.offset, OffsetSide::Outside));
    }

    #[test]
    fn parse_cam_job_with_params() {
        let p = json!({
            "toolDiameter": 3.0,
            "offset": "inside",
            "feedRate": 800.0,
            "plungeRate": 200.0,
            "spindleSpeed": 18000.0,
            "depth": 6.0,
            "safeZ": 15.0,
            "stepDown": 2.0,
            "tabCount": 4,
            "tabWidth": 2.0,
            "order": "nearest",
            "leadIn": { "kind": "arc", "length": 3.0 },
            "dialect": "linuxcnc"
        });
        let job = parse_cam_job("test", &p);
        assert!(job.is_ok());
        let j = job.ok().unwrap_or_else(|| std::process::abort());
        assert!((j.tool.diameter - 3.0).abs() < 1e-9);
        assert!(matches!(j.offset, OffsetSide::Inside));
        assert!((j.tool.feed_rate - 800.0).abs() < 1e-9);
        assert_eq!(j.tabs.count, 4);
        assert!(matches!(j.lead_in.kind, LeadKind::Arc));
        assert!(matches!(j.order, CutOrder::Nearest));
    }

    #[test]
    fn polyline_to_dxf_round_trips() {
        let pl = Polyline {
            vertices: vec![PolyVertex::new(Vec2::new(0.0, 0.0)), PolyVertex::new(Vec2::new(10.0, 0.0)), PolyVertex::new(Vec2::new(10.0, 10.0))],
            closed: true,
        };
        let dxf = polyline_to_dxf(&pl);
        assert!(dxf.contains("LWPOLYLINE"));
        assert!(dxf.contains("90\n3")); // 3 vertices
        assert!(dxf.contains("70\n1")); // closed
    }

    #[test]
    fn cncdxf_includes_all_geometry_types() {
        let mut doc = Drawing::new_metric();
        let vs = vec![PolyVertex::new(Vec2::new(0.0, 0.0)), PolyVertex::new(Vec2::new(5.0, 0.0))];
        let h1 = doc
            .add(
                &Space::Model,
                Common::default(),
                EntityKind::LwPolyline(LwPolyline { vertices: vs, closed: false, const_width: 0.0, elevation: 0.0, plinegen: false }),
            )
            .unwrap_or(Handle(0));
        let h2 = doc
            .add(
                &Space::Model,
                Common::default(),
                EntityKind::Circle(cadaddy_doc::Circle { center: cadaddy_geom::Vec3::new(5.0, 5.0, 0.0), radius: 2.0 }),
            )
            .unwrap_or(Handle(0));
        let h3 = doc
            .add(
                &Space::Model,
                Common::default(),
                EntityKind::Arc(cadaddy_doc::Arc {
                    center: cadaddy_geom::Vec3::new(10.0, 10.0, 0.0),
                    radius: 3.0,
                    start: 0.0,
                    end: std::f64::consts::PI,
                }),
            )
            .unwrap_or(Handle(0));
        let h4 = doc
            .add(
                &Space::Model,
                Common::default(),
                EntityKind::Line(cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(0.0, 0.0, 0.0), b: cadaddy_geom::Vec3::new(20.0, 0.0, 0.0) }),
            )
            .unwrap_or(Handle(0));
        let polys = extract_all_polylines(&doc, &[h1, h2, h3, h4]);
        assert_eq!(polys.len(), 4, "should extract 4 polylines from 4 entity types");
    }

    // ── Rework regression tests ──────────────────────────────────────────────

    fn add(s: &mut Session, kind: EntityKind) -> Handle {
        s.add_entity(kind).unwrap()
    }

    fn line_kind(a: (f64, f64), b: (f64, f64)) -> EntityKind {
        EntityKind::Line(cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(a.0, a.1, 0.0), b: cadaddy_geom::Vec3::new(b.0, b.1, 0.0) })
    }

    /// LWPOLYLINEs of a DXF's ENTITIES as (closed, [(x, y, bulge)]); every other entity type
    /// found is returned in the second list.
    fn read_dxf(text: &str) -> (Vec<(bool, Vec<(f64, f64, f64)>)>, Vec<String>) {
        let pairs: Vec<(&str, &str)> = text.lines().collect::<Vec<_>>().chunks(2).filter_map(|c| Some((c.first()?.trim(), *c.get(1)?))).collect();
        let (mut polys, mut others) = (Vec::new(), Vec::new());
        let mut cur: Option<(bool, Vec<(f64, f64, f64)>)> = None;
        for (code, value) in pairs {
            let num = || value.trim().parse::<f64>().unwrap();
            match code {
                "0" => {
                    polys.extend(cur.take());
                    match value.trim() {
                        "LWPOLYLINE" => cur = Some((false, Vec::new())),
                        "SECTION" | "ENDSEC" | "EOF" => {}
                        other => others.push(other.to_string()),
                    }
                }
                "2" => others.extend((value.trim() != "ENTITIES").then(|| value.trim().to_string())),
                "70" => {
                    if let Some(c) = cur.as_mut() {
                        c.0 = num() as i64 & 1 == 1;
                    }
                }
                "10" => cur.as_mut().unwrap().1.push((num(), 0.0, 0.0)),
                "20" => cur.as_mut().unwrap().1.last_mut().unwrap().1 = num(),
                "42" => cur.as_mut().unwrap().1.last_mut().unwrap().2 = num(),
                _ => {}
            }
        }
        polys.extend(cur);
        (polys, others)
    }

    // MUST 7: four LINEs meeting end to end + a block reference of a circle → one closed
    // 4-vertex polyline and one circle polyline where the block puts it; no INSERT, no BLOCK.
    #[test]
    fn cncdxf_joins_lines_and_explodes_block_references() {
        let mut s = Session::new();
        // Drawn in a mixed order and direction, as picked.
        let mut hs = vec![
            add(&mut s, line_kind((0.0, 0.0), (20.0, 0.0))),
            add(&mut s, line_kind((0.0, 10.0), (20.0, 10.0))),
            add(&mut s, line_kind((20.0, 0.0), (20.0, 10.0))),
            add(&mut s, line_kind((0.0, 10.0), (0.0, 0.0))),
        ];
        {
            let d = s.doc_mut().unwrap();
            let mut blk = cadaddy_doc::Block::new("PEG HOLE");
            let c = EntityKind::Circle(cadaddy_doc::Circle { center: cadaddy_geom::Vec3::new(1.0, 0.0, 0.0), radius: 2.0 });
            blk.entities.push(cadaddy_doc::Entity::new(Handle(0x400), c));
            d.blocks.insert("PEG HOLE".into(), std::sync::Arc::new(blk));
        }
        hs.push(add(
            &mut s,
            EntityKind::Insert(cadaddy_doc::Insert {
                block: "PEG HOLE".into(),
                insert: cadaddy_geom::Vec3::new(30.0, 5.0, 0.0),
                scale: cadaddy_geom::Vec3::new(1.5, 1.5, 1.5),
                rotation: std::f64::consts::FRAC_PI_2,
                attribs: Vec::new(),
                cols: 1,
                rows: 1,
                col_spacing: 0.0,
                row_spacing: 0.0,
                clip: None,
            }),
        ));
        let hex: Vec<String> = hs.iter().map(|h| h.hex()).collect();
        s.set_selection(hs.clone());
        let r = s.execute("cncdxf", &json!({ "handles": hex })).unwrap();
        let bytes = super::super::file::base64_decode(r["data"].as_str().unwrap()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.lines().any(|l| l.trim().is_empty()), "no blank lines in the DXF");
        let (polys, others) = read_dxf(&text);
        assert!(others.is_empty(), "only LWPOLYLINEs: {others:?}");
        assert!(!text.contains("INSERT") && !text.contains("BLOCK"));
        assert_eq!(polys.len(), 2, "{polys:?}");
        assert_eq!(r["entities"], json!(2));

        let rect = polys.iter().find(|p| p.1.iter().all(|v| v.2 == 0.0)).expect("the rectangle");
        assert!(rect.0, "the rectangle is closed");
        assert_eq!(rect.1.len(), 4);
        for corner in [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)] {
            assert!(rect.1.iter().any(|v| (v.0 - corner.0).abs() < 1e-6 && (v.1 - corner.1).abs() < 1e-6), "corner {corner:?}");
        }

        // The circle: (1, 0) in the block, scaled 1.5 and turned 90° about (30, 5) → (30, 6.5), r 3.
        let circle = polys.iter().find(|p| p.1.iter().any(|v| v.2 != 0.0)).expect("the circle");
        assert!(circle.0);
        let pl = Polyline { vertices: circle.1.iter().map(|v| PolyVertex::with_bulge(Vec2::new(v.0, v.1), v.2)).collect(), closed: true };
        let segs = pl.segments();
        assert!(!segs.is_empty());
        for seg in &segs {
            let cadaddy_geom::Segment::Arc { arc, .. } = seg else { panic!("circle piece is not an arc: {seg:?}") };
            assert!(arc.center.near(Vec2::new(30.0, 6.5), 1e-5), "centre {:?}", arc.center);
            assert!((arc.radius - 3.0).abs() < 1e-5, "radius {}", arc.radius);
        }
        let len: f64 = segs.iter().map(cadaddy_geom::Segment::len).sum();
        assert!((len - std::f64::consts::TAU * 3.0).abs() < 1e-4, "a full circle: {len}");
    }

    // MUST 8: bad values are refused with a guiding error, before any G-code is made.
    #[test]
    fn cam2dprofile_refuses_bad_params() {
        let mut s = Session::new();
        let sq = add(
            &mut s,
            EntityKind::LwPolyline(LwPolyline {
                vertices: vec![
                    PolyVertex::new(Vec2::new(0.0, 0.0)),
                    PolyVertex::new(Vec2::new(10.0, 0.0)),
                    PolyVertex::new(Vec2::new(10.0, 10.0)),
                    PolyVertex::new(Vec2::new(0.0, 10.0)),
                ],
                closed: true,
                const_width: 0.0,
                elevation: 0.0,
                plinegen: false,
            }),
        );
        s.set_selection(vec![sq]);
        let entities_before = s.doc().unwrap().entity_count();
        let cases: Vec<(Value, &str)> = vec![
            (json!({ "offset": "outsde" }), r#""inside", "outside", "on""#),
            (json!({ "order": "insidout" }), r#""nearest", "insideout""#),
            (json!({ "dialect": "grlb" }), r#""generic", "linuxcnc", "grbl", "mach3""#),
            (json!({ "leadIn": { "kind": "arcs", "length": 2 } }), r#""arc", "line", "none""#),
            (json!({ "leadOut": { "kind": "tangent" } }), "leadOut.kind"),
            (json!({ "tabCount": 1001 }), "0 to 1000"),
            (json!({ "tabCount": 1e12 }), "0 to 1000"),
            (json!({ "tabCount": -1 }), "0 to 1000"),
            (json!({ "depth": 0 }), "`depth` must be above 0"),
            (json!({ "depth": -3 }), "`depth` must be above 0"),
            (json!({ "stepDown": -1 }), "`stepDown`"),
            (json!({ "feedRate": 0 }), "`feedRate` must be above 0"),
            (json!({ "plungeRate": -5 }), "`plungeRate` must be above 0"),
            (json!({ "safeZ": 0 }), "`safeZ` must be above 0"),
            (json!({ "safeZ": -2 }), "`safeZ` must be above 0"),
        ];
        for (mut p, want) in cases {
            p["handles"] = json!([sq.hex()]);
            p["preview"] = json!(true);
            let r = s.execute("cam2dprofile", &p);
            let msg = match &r {
                Err(e) => e.to_string(),
                Ok(v) => panic!("{p} was accepted: {v}"),
            };
            assert!(msg.contains(want), "{p}: error `{msg}` does not say {want}");
            assert_eq!(s.doc().unwrap().entity_count(), entities_before, "{p}: nothing added");
        }
        // The same job with good values runs, and "grbl" is Grbl, without G64.
        let ok = s.execute("cam2dprofile", &json!({ "handles": [sq.hex()], "dialect": "grbl", "tabCount": 1000, "stepDown": 0 })).unwrap();
        assert_eq!(ok["dialect"], json!("grbl"));
        let g = ok["gcode"].as_str().unwrap();
        assert!(g.contains("G02") || g.contains("G01"));
        assert!(!g.contains("G64"));
    }
}
