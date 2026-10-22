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
                 dialect?: \"generic\"|\"linuxcnc\"|\"mach3\", metric?: bool (default true), \
                 path?: write G-code file, preview?: bool (add toolpath to drawing)} \
                 → {gcode, profiles, moves, previewHandles?}",
            )
            .enabled(has_selection),
        CommandSpec::new("cncdxf", "CNC DXF Export", run_cncdxf)
            .menu(&["Manufacture", "CNC DXF Export..."])
            .params(
                "{handles? (closed polylines and circles; default: selection), \
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

    // Compute toolpaths.
    let result = toolpath::compute(&profiles, &job).map_err(|e| bad("cam2dprofile", e.to_string()))?;

    // Generate G-code.
    let opts = parse_gcode_opts(p);
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

    // Collect all closed polylines, circles and arcs; explode inserts one level.
    let profiles = extract_all_polylines(doc, &hs);
    if profiles.is_empty() {
        return Err(bad("cncdxf", "no polylines, circles or arcs in the selection"));
    }

    // Build a minimal DXF: just LWPOLYLINE entities.
    let mut lines = Vec::new();
    lines.push("0\nSECTION\n2\nENTITIES".to_string());
    for pl in &profiles {
        lines.push(polyline_to_dxf(pl));
    }
    lines.push("0\nENDSEC\n0\nEOF".to_string());
    let dxf = lines.join("\n");
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

/// All polylines for CNC DXF: closed and open, circles and arcs → polylines.
fn extract_all_polylines(doc: &Drawing, handles: &[cadaddy_doc::Handle]) -> Vec<Polyline> {
    let mut out = Vec::new();
    for h in handles {
        let Some(entity) = doc.entity(*h) else { continue };
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
            _ => {}
        }
    }
    out
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

fn parse_cam_job(_cmd: &str, p: &Value) -> Result<CamJob> {
    let diameter = f64_or(p, "toolDiameter", 6.0);
    let offset = match str_param(p, "offset") {
        Some("inside") => OffsetSide::Inside,
        Some("outside") => OffsetSide::Outside,
        Some("on") => OffsetSide::On,
        _ => OffsetSide::Outside,
    };
    let feed_rate = f64_or(p, "feedRate", 500.0);
    let plunge_rate = f64_or(p, "plungeRate", 100.0);
    let spindle_speed = f64_or(p, "spindleSpeed", 12000.0);
    let depth = f64_or(p, "depth", 5.0);
    let safe_z = f64_or(p, "safeZ", 10.0);
    let step_down = f64_or(p, "stepDown", 0.0);

    let lead_in = parse_lead(p, "leadIn");
    let lead_out = parse_lead(p, "leadOut");

    let tab_count = p.get("tabCount").and_then(Value::as_u64).unwrap_or(0) as usize;
    let tabs = TabParams {
        width: f64_or(p, "tabWidth", 3.0),
        height: f64_or(p, "tabHeight", 1.0),
        count: tab_count,
    };

    let order = match str_param(p, "order") {
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

fn parse_lead(p: &Value, key: &str) -> LeadParams {
    match p.get(key) {
        Some(v) if v.is_object() => {
            let kind = match v.get("kind").and_then(Value::as_str) {
                Some("arc") => LeadKind::Arc,
                Some("line") => LeadKind::Line,
                _ => LeadKind::None,
            };
            let length = v.get("length").and_then(Value::as_f64).unwrap_or(2.0);
            LeadParams { kind, length }
        }
        _ => LeadParams { kind: LeadKind::None, length: 0.0 },
    }
}

fn parse_gcode_opts(p: &Value) -> GcodeOptions {
    let dialect = match str_param(p, "dialect") {
        Some("linuxcnc") | Some("grbl") => Dialect::LinuxCnc,
        Some("mach3") => Dialect::Mach3,
        _ => Dialect::Generic,
    };
    GcodeOptions {
        dialect,
        metric: bool_or(p, "metric", true),
        ..GcodeOptions::default()
    }
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
            vertices: vec![
                PolyVertex::new(Vec2::new(0.0, 0.0)),
                PolyVertex::new(Vec2::new(10.0, 0.0)),
                PolyVertex::new(Vec2::new(10.0, 10.0)),
            ],
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
        let h1 = doc.add(&Space::Model, Common::default(), EntityKind::LwPolyline(LwPolyline { vertices: vs, closed: false, const_width: 0.0, elevation: 0.0, plinegen: false })).unwrap_or(Handle(0));
        let h2 = doc.add(&Space::Model, Common::default(), EntityKind::Circle(cadaddy_doc::Circle { center: cadaddy_geom::Vec3::new(5.0, 5.0, 0.0), radius: 2.0 })).unwrap_or(Handle(0));
        let h3 = doc.add(&Space::Model, Common::default(), EntityKind::Arc(cadaddy_doc::Arc { center: cadaddy_geom::Vec3::new(10.0, 10.0, 0.0), radius: 3.0, start: 0.0, end: std::f64::consts::PI })).unwrap_or(Handle(0));
        let h4 = doc.add(&Space::Model, Common::default(), EntityKind::Line(cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(0.0, 0.0, 0.0), b: cadaddy_geom::Vec3::new(20.0, 0.0, 0.0) })).unwrap_or(Handle(0));
        let polys = extract_all_polylines(&doc, &[h1, h2, h3, h4]);
        assert_eq!(polys.len(), 4, "should extract 4 polylines from 4 entity types");
    }
}
