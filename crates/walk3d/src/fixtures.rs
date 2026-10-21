//! Parametric fixture geometry: real shapes instead of boxes.
//!
//! Each fixture family generates triangulated meshes from type parameters (width, depth,
//! height, aperture, etc.) and a level of detail. The geometry is drawn by our own code
//! from generic dimensions — no manufacturer models, logos or downloaded content.
//!
//! LOD levels:
//! - `Lod::Detailed` — the full parametric shape
//! - `Lod::Simple` — a reduced-triangle shape (plan symbol + extruded outline)
//! - `Lod::Box` — falls back to the existing box (handled by the caller)

use crate::derive::{Owner, Scene, boxed, quad};
use crate::math::{V3, v3};
use cadaddy_color::Rgb;
use cadaddy_doc::geom::Vec2;
use std::f64::consts::PI;

/// Level of detail for fixture geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lod {
    /// Full parametric geometry.
    Detailed,
    /// Reduced triangles (outline extrusion).
    Simple,
    /// A box (delegate to `boxed()`).
    Box,
}

/// Which fixture family a device type belongs to and its parametric dimensions.
#[derive(Clone, Debug)]
pub enum FixtureFamily {
    // --- Luminaires ---
    Troffer { width_ft: f64, length_ft: f64, depth_ft: f64 },
    Downlight { aperture_ft: f64, depth_ft: f64 },
    LinearPendant { length_ft: f64, width_ft: f64, drop_ft: f64 },
    SlotLight { length_ft: f64, width_ft: f64 },
    WallPack { width_ft: f64, height_ft: f64, depth_ft: f64 },
    Sconce { width_ft: f64, height_ft: f64, depth_ft: f64 },
    StripWrap { length_ft: f64, width_ft: f64, depth_ft: f64 },
    ExitSign { width_ft: f64, height_ft: f64, depth_ft: f64 },
    EmergencyLight { width_ft: f64, height_ft: f64, depth_ft: f64 },

    // --- Electrical devices ---
    Receptacle { width_ft: f64, height_ft: f64, depth_ft: f64 },
    Switch { width_ft: f64, height_ft: f64, depth_ft: f64 },
    FloorBox { width_ft: f64, depth_ft: f64, height_ft: f64 },
    DataOutlet { width_ft: f64, height_ft: f64, depth_ft: f64 },

    // --- Panels / disconnects ---
    Panel { width_ft: f64, height_ft: f64, depth_ft: f64 },
    Disconnect { width_ft: f64, height_ft: f64, depth_ft: f64 },

    // --- Plumbing ---
    WaterCloset { width_ft: f64, depth_ft: f64, height_ft: f64 },
    Lavatory { width_ft: f64, depth_ft: f64, height_ft: f64 },
    Urinal { width_ft: f64, depth_ft: f64, height_ft: f64 },
    Shower { width_ft: f64, depth_ft: f64, height_ft: f64 },
    MopSink { width_ft: f64, depth_ft: f64, height_ft: f64 },
    WaterHeater { diameter_ft: f64, height_ft: f64 },

    // --- Casework ---
    BaseCabinet { width_ft: f64, depth_ft: f64, height_ft: f64 },
    WallCabinet { width_ft: f64, depth_ft: f64, height_ft: f64 },
    TallCabinet { width_ft: f64, depth_ft: f64, height_ft: f64 },
    Countertop { width_ft: f64, depth_ft: f64, thickness_ft: f64 },
    Backsplash { width_ft: f64, height_ft: f64, thickness_ft: f64 },

    // --- Pipe / conduit fittings ---
    Elbow90 { diameter_ft: f64 },
    Elbow45 { diameter_ft: f64 },
    Tee { diameter_ft: f64 },
    Reducer { large_ft: f64, small_ft: f64, length_ft: f64 },
    Coupling { diameter_ft: f64, length_ft: f64 },
}

/// Classify a device type string into a fixture family with appropriate dimensions.
///
/// `size_ft` is the `[width, depth, height]` from the Mount rules.
pub fn classify(kind: &str, type_tag: &str, size_ft: [f64; 3]) -> Option<FixtureFamily> {
    let [w, d, h] = size_ft;
    // Try kind first, then type_tag for more specific matches.
    let k = kind.to_ascii_lowercase();
    let t = type_tag.to_ascii_lowercase();

    // Luminaires
    if k.contains("troffer") || t.contains("troffer") || is_troffer_tag(&t) {
        return Some(FixtureFamily::Troffer { width_ft: w, length_ft: d.max(w), depth_ft: h.min(0.5) });
    }
    if k.contains("downlight") || t.contains("dl") || t.contains("downlight") {
        return Some(FixtureFamily::Downlight { aperture_ft: w.min(d), depth_ft: h });
    }
    if k.contains("pendant") || k.contains("linear_pendant") || t.contains("pendant") {
        return Some(FixtureFamily::LinearPendant { length_ft: w.max(d), width_ft: w.min(d).max(0.1), drop_ft: 2.0 });
    }
    if k.contains("slot") || t.contains("slot") {
        return Some(FixtureFamily::SlotLight { length_ft: w.max(d), width_ft: w.min(d).max(1.0 / 12.0) });
    }
    if k.contains("wall_pack") || k.contains("wallpack") || t.contains("wp") {
        return Some(FixtureFamily::WallPack { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("sconce") || t.contains("sconce") {
        return Some(FixtureFamily::Sconce { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("strip") || k.contains("wrap") || t.contains("strip") || t.contains("wrap") {
        return Some(FixtureFamily::StripWrap { length_ft: w.max(d), width_ft: w.min(d), depth_ft: h });
    }
    if k.contains("exit") || t.contains("exit") || t.starts_with("es") {
        return Some(FixtureFamily::ExitSign { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("emergency") || t.starts_with("em") {
        return Some(FixtureFamily::EmergencyLight { width_ft: w, height_ft: h, depth_ft: d });
    }

    // Ceiling-mounted light fixtures (generic — troffer is the default for ceiling lights)
    if k == "light_fixture" || k.contains("light") {
        // Determine shape from aspect ratio: square/rectangular = troffer, small/round = downlight
        if w <= 0.75 && d <= 0.75 {
            return Some(FixtureFamily::Downlight { aperture_ft: w.max(d), depth_ft: h });
        }
        return Some(FixtureFamily::Troffer { width_ft: w, length_ft: d.max(w), depth_ft: h.min(0.5) });
    }

    // Electrical devices
    if k.contains("receptacle") || k.contains("outlet") || t.contains("rec") {
        return Some(FixtureFamily::Receptacle { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("switch") || k.contains("dimmer") || t.starts_with("s") && t.len() <= 2 {
        return Some(FixtureFamily::Switch { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("floor_box") || k.contains("floorbox") {
        return Some(FixtureFamily::FloorBox { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("data") || t.contains("data") || t.contains("tv") {
        return Some(FixtureFamily::DataOutlet { width_ft: w, height_ft: h, depth_ft: d });
    }

    // Panels and disconnects
    if k.contains("panel") || t.starts_with("lp") || t.starts_with("pp") || t.starts_with("dp") {
        return Some(FixtureFamily::Panel { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("disconnect") || t.contains("disc") {
        return Some(FixtureFamily::Disconnect { width_ft: w, height_ft: h, depth_ft: d });
    }

    // Plumbing fixtures
    if k.contains("water_closet") || k.contains("toilet") || t.contains("wc") {
        return Some(FixtureFamily::WaterCloset { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("lavatory") || k.contains("lav") || t.contains("lav") {
        return Some(FixtureFamily::Lavatory { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("urinal") || t.contains("ur") {
        return Some(FixtureFamily::Urinal { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("shower") || t.contains("sh") && k.contains("plumb") {
        return Some(FixtureFamily::Shower { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("mop") || t.contains("mop") {
        return Some(FixtureFamily::MopSink { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("water_heater") || k.contains("waterheater") {
        return Some(FixtureFamily::WaterHeater { diameter_ft: w.max(d), height_ft: h });
    }

    // Casework
    if k.contains("base_cabinet") || k.contains("basecab") {
        return Some(FixtureFamily::BaseCabinet { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("wall_cabinet") || k.contains("wallcab") {
        return Some(FixtureFamily::WallCabinet { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("tall_cabinet") || k.contains("tallcab") {
        return Some(FixtureFamily::TallCabinet { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("countertop") || k.contains("counter") {
        return Some(FixtureFamily::Countertop { width_ft: w, depth_ft: d, thickness_ft: h.min(2.0 / 12.0) });
    }
    if k.contains("backsplash") {
        return Some(FixtureFamily::Backsplash { width_ft: w, height_ft: h, thickness_ft: d.min(1.0 / 12.0) });
    }

    // Pipe/conduit fittings
    if k.contains("elbow_90") || k.contains("elbow90") {
        return Some(FixtureFamily::Elbow90 { diameter_ft: w.max(d) });
    }
    if k.contains("elbow_45") || k.contains("elbow45") {
        return Some(FixtureFamily::Elbow45 { diameter_ft: w.max(d) });
    }
    if k.contains("tee") && (k.contains("pipe") || k.contains("conduit") || k.contains("fitting")) {
        return Some(FixtureFamily::Tee { diameter_ft: w.max(d) });
    }
    if k.contains("reducer") {
        return Some(FixtureFamily::Reducer { large_ft: w.max(d), small_ft: w.min(d), length_ft: h });
    }
    if k.contains("coupling") {
        return Some(FixtureFamily::Coupling { diameter_ft: w.max(d), length_ft: h });
    }

    None
}

/// Generate fixture geometry into the scene. Returns `true` if geometry was produced, `false`
/// if the caller should fall back to `boxed()`.
pub fn generate(s: &mut Scene, family: &FixtureFamily, centre: V3, dir: Vec2, col: Rgb, owner: Owner, lod: Lod) -> bool {
    if lod == Lod::Box {
        return false; // Caller uses boxed()
    }
    match family {
        // --- Luminaires ---
        FixtureFamily::Troffer { width_ft, length_ft, depth_ft } => troffer(s, centre, dir, *width_ft, *length_ft, *depth_ft, col, owner, lod),
        FixtureFamily::Downlight { aperture_ft, depth_ft } => downlight(s, centre, *aperture_ft, *depth_ft, col, owner, lod),
        FixtureFamily::LinearPendant { length_ft, width_ft, drop_ft } => {
            linear_pendant(s, centre, dir, *length_ft, *width_ft, *drop_ft, col, owner, lod)
        }
        FixtureFamily::SlotLight { length_ft, width_ft } => slot_light(s, centre, dir, *length_ft, *width_ft, col, owner, lod),
        FixtureFamily::WallPack { width_ft, height_ft, depth_ft } => wall_pack(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod),
        FixtureFamily::Sconce { width_ft, height_ft, depth_ft } => sconce(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod),
        FixtureFamily::StripWrap { length_ft, width_ft, depth_ft } => strip_wrap(s, centre, dir, *length_ft, *width_ft, *depth_ft, col, owner, lod),
        FixtureFamily::ExitSign { width_ft, height_ft, depth_ft } => exit_sign(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod),
        FixtureFamily::EmergencyLight { width_ft, height_ft, depth_ft } => {
            emergency_light(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod)
        }

        // --- Electrical devices ---
        FixtureFamily::Receptacle { width_ft, height_ft, depth_ft } => receptacle(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod),
        FixtureFamily::Switch { width_ft, height_ft, depth_ft } => switch_device(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod),
        FixtureFamily::FloorBox { width_ft, depth_ft, height_ft } => floor_box(s, centre, *width_ft, *depth_ft, *height_ft, col, owner, lod),
        FixtureFamily::DataOutlet { width_ft, height_ft, depth_ft } => {
            // Same visual as receptacle
            receptacle(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod)
        }

        // --- Panels / disconnects ---
        FixtureFamily::Panel { width_ft, height_ft, depth_ft } => panel(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod),
        FixtureFamily::Disconnect { width_ft, height_ft, depth_ft } => disconnect(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod),

        // --- Plumbing ---
        FixtureFamily::WaterCloset { width_ft, depth_ft, height_ft } => {
            water_closet(s, centre, dir, *width_ft, *depth_ft, *height_ft, col, owner, lod)
        }
        FixtureFamily::Lavatory { width_ft, depth_ft, height_ft } => lavatory(s, centre, dir, *width_ft, *depth_ft, *height_ft, col, owner, lod),
        FixtureFamily::Urinal { width_ft, depth_ft, height_ft } => urinal(s, centre, dir, *width_ft, *depth_ft, *height_ft, col, owner, lod),
        FixtureFamily::Shower { width_ft, depth_ft, height_ft } => shower(s, centre, dir, *width_ft, *depth_ft, *height_ft, col, owner, lod),
        FixtureFamily::MopSink { width_ft, depth_ft, height_ft } => mop_sink(s, centre, dir, *width_ft, *depth_ft, *height_ft, col, owner, lod),
        FixtureFamily::WaterHeater { diameter_ft, height_ft } => water_heater(s, centre, *diameter_ft, *height_ft, col, owner, lod),

        // --- Casework ---
        FixtureFamily::BaseCabinet { width_ft, depth_ft, height_ft } => {
            base_cabinet(s, centre, dir, *width_ft, *depth_ft, *height_ft, col, owner, lod)
        }
        FixtureFamily::WallCabinet { width_ft, depth_ft, height_ft } => {
            wall_cabinet(s, centre, dir, *width_ft, *depth_ft, *height_ft, col, owner, lod)
        }
        FixtureFamily::TallCabinet { width_ft, depth_ft, height_ft } => {
            tall_cabinet(s, centre, dir, *width_ft, *depth_ft, *height_ft, col, owner, lod)
        }
        FixtureFamily::Countertop { width_ft, depth_ft, thickness_ft } => {
            countertop(s, centre, dir, *width_ft, *depth_ft, *thickness_ft, col, owner, lod)
        }
        FixtureFamily::Backsplash { width_ft, height_ft, thickness_ft } => {
            backsplash(s, centre, dir, *width_ft, *height_ft, *thickness_ft, col, owner, lod)
        }

        // --- Pipe/conduit fittings ---
        FixtureFamily::Elbow90 { diameter_ft } => elbow(s, centre, dir, *diameter_ft, 90.0, col, owner, lod),
        FixtureFamily::Elbow45 { diameter_ft } => elbow(s, centre, dir, *diameter_ft, 45.0, col, owner, lod),
        FixtureFamily::Tee { diameter_ft } => tee_fitting(s, centre, dir, *diameter_ft, col, owner, lod),
        FixtureFamily::Reducer { large_ft, small_ft, length_ft } => reducer(s, centre, dir, *large_ft, *small_ft, *length_ft, col, owner, lod),
        FixtureFamily::Coupling { diameter_ft, length_ft } => coupling(s, centre, dir, *diameter_ft, *length_ft, col, owner, lod),
    }
    true
}

/// Is this a troffer-style type tag? Common tags: A, B, C (schedule), 2x2, 2x4, 1x4.
fn is_troffer_tag(tag: &str) -> bool {
    // Single-letter schedule tags A–F are usually troffers
    if tag.len() == 1 && tag.as_bytes().first().is_some_and(|b| b.is_ascii_uppercase()) {
        return true;
    }
    // NxM patterns like 2x4, 2x2, 1x4
    let parts: Vec<&str> = tag.split('x').collect();
    if parts.len() == 2 {
        return parts.iter().all(|p| p.parse::<f64>().is_ok());
    }
    false
}

// ========================================================================================
// Coordinate helpers
// ========================================================================================

/// Build a local-to-world transform from centre, direction (along X), and up (+Z).
/// Returns a function mapping local (x, y, z) to world V3.
fn local_frame(centre: V3, dir: Vec2) -> impl Fn(f64, f64, f64) -> V3 {
    let dx = v3(dir.x, dir.y, 0.0).norm();
    let dy = v3(-dir.y, dir.x, 0.0).norm();
    let dz = v3(0.0, 0.0, 1.0);
    move |x, y, z| centre + dx * x + dy * y + dz * z
}

/// Number of segments for a circle at a given LOD.
fn circle_segs(lod: Lod) -> usize {
    match lod {
        Lod::Detailed => 24,
        Lod::Simple => 12,
        Lod::Box => 6,
    }
}

/// Generate a ring of points at height z with given radius, in local coordinates.
fn ring(n: usize, radius: f64, z: f64, to_world: &impl Fn(f64, f64, f64) -> V3) -> Vec<V3> {
    (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            to_world(a.cos() * radius, a.sin() * radius, z)
        })
        .collect()
}

/// Emit a cylinder (side faces only) between two rings of the same length.
fn cylinder_side(s: &mut Scene, bottom: &[V3], top: &[V3], col: Rgb, owner: Owner) {
    let n = bottom.len();
    if n < 3 {
        return;
    }
    for i in 0..n {
        let j = (i + 1) % n;
        quad(s, [bottom[i], bottom[j], top[j], top[i]], col, owner);
    }
}

/// Emit a flat fan (disc) from the ring, with given normal direction sign.
fn disc(s: &mut Scene, centre: V3, pts: &[V3], col: Rgb, owner: Owner) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    for i in 1..n - 1 {
        let a = pts[0];
        let b = pts[i];
        let c = pts[i + 1];
        tri(s, a, b, c, col, owner);
    }
    // Close the last triangle
    if n > 2 {
        let _ = centre; // centre used for normal calculation if needed
    }
}

/// One triangle with computed normal.
fn tri(s: &mut Scene, a: V3, b: V3, c: V3, col: Rgb, owner: Owner) {
    let n = (b - a).cross(c - a).norm();
    let v = |p: V3| crate::derive::Vertex { p: p.f32(), n: n.f32(), c: [col.0, col.1, col.2, 255] };
    s.tris.push(v(a));
    s.tris.push(v(b));
    s.tris.push(v(c));
    s.parts.push(s.part);
    s.owners.push(owner);
}

/// A closed cylinder (top and bottom caps) centred on `centre`.
fn closed_cylinder(s: &mut Scene, centre: V3, dir: Vec2, radius: f64, height: f64, col: Rgb, owner: Owner, lod: Lod) {
    let tw = local_frame(centre, dir);
    let n = circle_segs(lod);
    let bot = ring(n, radius, -height / 2.0, &tw);
    let top = ring(n, radius, height / 2.0, &tw);
    cylinder_side(s, &bot, &top, col, owner);
    disc(s, tw(0.0, 0.0, -height / 2.0), &bot, col, owner);
    disc(s, tw(0.0, 0.0, height / 2.0), &top, col, owner);
}

// ========================================================================================
// LUMINAIRES
// ========================================================================================

/// 2×4 / 2×2 / 1×4 recessed troffer: a flat rectangular housing with a recessed lens.
#[allow(clippy::too_many_arguments)]
fn troffer(s: &mut Scene, c: V3, dir: Vec2, width: f64, length: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hl = length / 2.0;

    if lod == Lod::Simple {
        // Simple: just the outer frame box
        boxed(s, c, dir, width, length, depth, col, owner);
        return;
    }

    // Detailed: frame + recessed lens panel
    let frame_w = 1.0 / 12.0; // 1-inch frame lip
    let lens_recess = depth * 0.3; // Lens sits 30% up from bottom

    // Outer frame (top face)
    let top_col = darken(col, 0.85);
    quad(s, [tw(-hw, -hl, 0.0), tw(hw, -hl, 0.0), tw(hw, hl, 0.0), tw(-hw, hl, 0.0)], top_col, owner);

    // Bottom frame border (4 strips around the lens opening)
    let iw = hw - frame_w;
    let il = hl - frame_w;
    let bz = -depth;
    quad(s, [tw(-hw, -hl, bz), tw(hw, -hl, bz), tw(iw, -il, bz), tw(-iw, -il, bz)], col, owner);
    quad(s, [tw(hw, -hl, bz), tw(hw, hl, bz), tw(iw, il, bz), tw(iw, -il, bz)], col, owner);
    quad(s, [tw(hw, hl, bz), tw(-hw, hl, bz), tw(-iw, il, bz), tw(iw, il, bz)], col, owner);
    quad(s, [tw(-hw, hl, bz), tw(-hw, -hl, bz), tw(-iw, -il, bz), tw(-iw, il, bz)], col, owner);

    // Lens panel (slightly recessed, brighter — the light surface)
    let lens_col = brighten(col, 1.3);
    let lz = bz + lens_recess;
    quad(s, [tw(-iw, -il, lz), tw(iw, -il, lz), tw(iw, il, lz), tw(-iw, il, lz)], lens_col, owner);

    // Side faces of the housing
    let sides = [
        [tw(-hw, -hl, 0.0), tw(hw, -hl, 0.0), tw(hw, -hl, bz), tw(-hw, -hl, bz)],
        [tw(hw, -hl, 0.0), tw(hw, hl, 0.0), tw(hw, hl, bz), tw(hw, -hl, bz)],
        [tw(hw, hl, 0.0), tw(-hw, hl, 0.0), tw(-hw, hl, bz), tw(hw, hl, bz)],
        [tw(-hw, hl, 0.0), tw(-hw, -hl, 0.0), tw(-hw, -hl, bz), tw(-hw, hl, bz)],
    ];
    for f in sides {
        quad(s, f, darken(col, 0.9), owner);
    }
}

/// Round recessed downlight.
#[allow(clippy::too_many_arguments)]
fn downlight(s: &mut Scene, c: V3, aperture: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    let n = circle_segs(lod);
    let r = aperture / 2.0;
    let trim_w = 0.5 / 12.0; // Trim ring half-inch

    // Trim ring (flat annulus at ceiling level)
    let trim_col = Rgb(240, 240, 240); // White trim
    let outer: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            c + v3(a.cos() * (r + trim_w), a.sin() * (r + trim_w), 0.0)
        })
        .collect();
    let inner: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            c + v3(a.cos() * r, a.sin() * r, 0.0)
        })
        .collect();
    for i in 0..n {
        let j = (i + 1) % n;
        quad(s, [outer[i], outer[j], inner[j], inner[i]], trim_col, owner);
    }

    // Inner cone (baffle) — tapers slightly
    let inner_r = r * 0.7;
    let bottom: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            c + v3(a.cos() * inner_r, a.sin() * inner_r, -depth)
        })
        .collect();
    cylinder_side(s, &inner, &bottom, darken(col, 0.7), owner);

    // Light disc at bottom
    let lens_col = brighten(col, 1.5);
    disc(s, c + v3(0.0, 0.0, -depth), &bottom, lens_col, owner);
}

/// Linear pendant luminaire: a long narrow housing suspended on cables.
#[allow(clippy::too_many_arguments)]
fn linear_pendant(s: &mut Scene, c: V3, dir: Vec2, length: f64, width: f64, drop: f64, col: Rgb, owner: Owner, lod: Lod) {
    // The housing hangs `drop` feet below c
    let body_c = c + v3(0.0, 0.0, -drop);
    let body_h = width; // Square cross-section roughly
    let tw = local_frame(body_c, dir);
    let hw = width / 2.0;
    let hl = length / 2.0;

    if lod == Lod::Simple {
        boxed(s, body_c, dir, length, width, body_h, col, owner);
        return;
    }

    // Housing body
    let bh = body_h / 2.0;
    let top = [tw(-hl, -hw, bh), tw(hl, -hw, bh), tw(hl, hw, bh), tw(-hl, hw, bh)];
    let bot = [tw(-hl, -hw, -bh), tw(hl, -hw, -bh), tw(hl, hw, -bh), tw(-hl, hw, -bh)];
    quad(s, top, darken(col, 0.85), owner);
    // Bottom is the lens — brighter
    quad(s, [bot[0], bot[3], bot[2], bot[1]], brighten(col, 1.3), owner);
    // Sides
    for i in 0..4 {
        let j = (i + 1) % 4;
        quad(s, [top[i], top[j], bot[j], bot[i]], darken(col, 0.9), owner);
    }

    // Suspension cables (two thin lines)
    let cable_col = Rgb(80, 80, 80);
    let cable_r = 0.02;
    for x_off in [-hl * 0.7, hl * 0.7] {
        let cable_c = tw(x_off, 0.0, bh + drop / 2.0);
        closed_cylinder(s, cable_c, dir, cable_r, drop, cable_col, owner, Lod::Simple);
    }
}

/// Slot light: a narrow recessed slot in the ceiling.
#[allow(clippy::too_many_arguments)]
fn slot_light(s: &mut Scene, c: V3, dir: Vec2, length: f64, width: f64, col: Rgb, owner: Owner, _lod: Lod) {
    let tw = local_frame(c, dir);
    let hl = length / 2.0;
    let hw = width / 2.0;
    let depth = 2.0 / 12.0; // 2 inches deep

    // The slot: a bright narrow rectangle recessed slightly
    let lens_col = brighten(col, 1.5);
    quad(s, [tw(-hl, -hw, -depth), tw(hl, -hw, -depth), tw(hl, hw, -depth), tw(-hl, hw, -depth)], lens_col, owner);

    // Frame edges
    let frame = darken(col, 0.8);
    quad(s, [tw(-hl, -hw, 0.0), tw(hl, -hw, 0.0), tw(hl, -hw, -depth), tw(-hl, -hw, -depth)], frame, owner);
    quad(s, [tw(hl, hw, 0.0), tw(-hl, hw, 0.0), tw(-hl, hw, -depth), tw(hl, hw, -depth)], frame, owner);
    quad(s, [tw(-hl, hw, 0.0), tw(-hl, -hw, 0.0), tw(-hl, -hw, -depth), tw(-hl, hw, -depth)], frame, owner);
    quad(s, [tw(hl, -hw, 0.0), tw(hl, hw, 0.0), tw(hl, hw, -depth), tw(hl, -hw, -depth)], frame, owner);
}

/// Wall pack: a squat, wall-mounted outdoor light.
#[allow(clippy::too_many_arguments)]
fn wall_pack(s: &mut Scene, c: V3, dir: Vec2, width: f64, height: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, col, owner);
        return;
    }
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hh = height / 2.0;

    // Back plate (against wall)
    quad(s, [tw(-hw, 0.0, -hh), tw(hw, 0.0, -hh), tw(hw, 0.0, hh), tw(-hw, 0.0, hh)], darken(col, 0.7), owner);

    // Curved housing — approximate with a trapezoid profile
    let d = depth;
    let top_shrink = 0.7;
    let front_top = [tw(-hw * top_shrink, d, hh * 0.8), tw(hw * top_shrink, d, hh * 0.8)];
    let front_bot = [tw(-hw, d * 0.9, -hh), tw(hw, d * 0.9, -hh)];

    // Front face
    quad(s, [front_bot[0], front_bot[1], front_top[1], front_top[0]], brighten(col, 1.2), owner);
    // Top
    quad(s, [tw(-hw, 0.0, hh), tw(hw, 0.0, hh), front_top[1], front_top[0]], darken(col, 0.9), owner);
    // Bottom
    quad(s, [tw(-hw, 0.0, -hh), front_bot[0], front_bot[1], tw(hw, 0.0, -hh)], darken(col, 0.85), owner);
    // Left side
    quad(s, [tw(-hw, 0.0, -hh), tw(-hw, 0.0, hh), front_top[0], front_bot[0]], darken(col, 0.9), owner);
    // Right side
    quad(s, [tw(hw, 0.0, hh), tw(hw, 0.0, -hh), front_bot[1], front_top[1]], darken(col, 0.9), owner);
}

/// Wall sconce: a decorative wall-mounted light.
#[allow(clippy::too_many_arguments)]
fn sconce(s: &mut Scene, c: V3, dir: Vec2, width: f64, height: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, col, owner);
        return;
    }
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hh = height / 2.0;

    // Back plate
    quad(s, [tw(-hw, 0.0, -hh), tw(hw, 0.0, -hh), tw(hw, 0.0, hh), tw(-hw, 0.0, hh)], darken(col, 0.7), owner);

    // Shade — a half-cylinder bulge outward; approximate with faceted quads
    let n = if lod == Lod::Detailed { 8 } else { 4 };
    for i in 0..n {
        let a0 = PI * (i as f64) / (n as f64);
        let a1 = PI * ((i + 1) as f64) / (n as f64);
        let y0 = a0.sin() * depth;
        let y1 = a1.sin() * depth;
        let x0_off = a0.cos() * hw * 0.3;
        let x1_off = a1.cos() * hw * 0.3;
        let shade_col = if i < n / 2 { brighten(col, 1.2) } else { col };
        quad(s, [tw(-hw + x0_off, y0, -hh), tw(-hw + x1_off, y1, -hh), tw(-hw + x1_off, y1, hh), tw(-hw + x0_off, y0, hh)], shade_col, owner);
        quad(s, [tw(hw - x1_off, y1, -hh), tw(hw - x0_off, y0, -hh), tw(hw - x0_off, y0, hh), tw(hw - x1_off, y1, hh)], shade_col, owner);
    }
}

/// Strip / wrap fluorescent.
#[allow(clippy::too_many_arguments)]
fn strip_wrap(s: &mut Scene, c: V3, dir: Vec2, length: f64, width: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    if lod == Lod::Simple {
        boxed(s, c, dir, length, width, depth, col, owner);
        return;
    }
    let tw = local_frame(c, dir);
    let hl = length / 2.0;
    let hw = width / 2.0;
    let hd = depth / 2.0;

    // Housing top
    quad(s, [tw(-hl, -hw, hd), tw(hl, -hw, hd), tw(hl, hw, hd), tw(-hl, hw, hd)], darken(col, 0.85), owner);
    // Wrap lens (bottom face, brighter)
    quad(s, [tw(-hl, -hw, -hd), tw(-hl, hw, -hd), tw(hl, hw, -hd), tw(hl, -hw, -hd)], brighten(col, 1.4), owner);
    // Sides
    quad(s, [tw(-hl, -hw, hd), tw(-hl, -hw, -hd), tw(hl, -hw, -hd), tw(hl, -hw, hd)], col, owner);
    quad(s, [tw(hl, hw, hd), tw(hl, hw, -hd), tw(-hl, hw, -hd), tw(-hl, hw, hd)], col, owner);
    // Ends
    quad(s, [tw(-hl, -hw, hd), tw(-hl, hw, hd), tw(-hl, hw, -hd), tw(-hl, -hw, -hd)], col, owner);
    quad(s, [tw(hl, -hw, -hd), tw(hl, hw, -hd), tw(hl, hw, hd), tw(hl, -hw, hd)], col, owner);
}

/// Exit sign: rectangular housing with lit EXIT face.
#[allow(clippy::too_many_arguments)]
fn exit_sign(s: &mut Scene, c: V3, dir: Vec2, width: f64, height: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, col, owner);
        return;
    }
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hh = height / 2.0;
    let hd = depth / 2.0;

    // Housing box (dark body)
    let body = Rgb(40, 40, 40);
    quad(s, [tw(-hw, -hd, hh), tw(hw, -hd, hh), tw(hw, hd, hh), tw(-hw, hd, hh)], body, owner);
    quad(s, [tw(-hw, -hd, -hh), tw(-hw, hd, -hh), tw(hw, hd, -hh), tw(hw, -hd, -hh)], body, owner);
    // Sides
    quad(s, [tw(-hw, -hd, -hh), tw(-hw, -hd, hh), tw(-hw, hd, hh), tw(-hw, hd, -hh)], body, owner);
    quad(s, [tw(hw, -hd, hh), tw(hw, -hd, -hh), tw(hw, hd, -hh), tw(hw, hd, hh)], body, owner);

    // EXIT faces (front and back, red/green lit)
    let exit_col = Rgb(220, 30, 30); // Red EXIT
    quad(s, [tw(-hw, -hd, -hh), tw(hw, -hd, -hh), tw(hw, -hd, hh), tw(-hw, -hd, hh)], exit_col, owner);
    quad(s, [tw(hw, hd, -hh), tw(-hw, hd, -hh), tw(-hw, hd, hh), tw(hw, hd, hh)], exit_col, owner);
}

/// Emergency light: two lamp heads on a housing.
#[allow(clippy::too_many_arguments)]
fn emergency_light(s: &mut Scene, c: V3, dir: Vec2, width: f64, height: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, col, owner);
        return;
    }
    let tw = local_frame(c, dir);
    let _hw = width / 2.0;
    let hh = height / 2.0;
    let _hd = depth / 2.0;

    // Central housing (battery box)
    let body = Rgb(230, 230, 230);
    let box_w = width * 0.4;
    let hbw = box_w / 2.0;
    boxed(s, c, dir, box_w, depth, height, body, owner);

    // Two lamp heads — small cylinders pointing outward
    let head_r = height * 0.25;
    let head_len = width * 0.2;
    for side in [-1.0_f64, 1.0] {
        let head_c = tw(side * (hbw + head_len / 2.0), 0.0, hh * 0.3);
        closed_cylinder(s, head_c, dir, head_r, head_len, Rgb(255, 255, 200), owner, lod);
    }
}

// ========================================================================================
// ELECTRICAL DEVICES
// ========================================================================================

/// Receptacle: a wall plate with two outlet slots.
#[allow(clippy::too_many_arguments)]
fn receptacle(s: &mut Scene, c: V3, dir: Vec2, width: f64, height: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, col, owner);
        return;
    }
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hh = height / 2.0;

    // Cover plate (flat, slightly raised from wall)
    let plate_col = Rgb(235, 230, 220); // Off-white plate
    quad(s, [tw(-hw, depth * 0.1, -hh), tw(hw, depth * 0.1, -hh), tw(hw, depth * 0.1, hh), tw(-hw, depth * 0.1, hh)], plate_col, owner);

    // Yoke/body behind the plate
    let _body = Rgb(200, 200, 200);
    let _iw = hw * 0.6;
    let _ih = hh * 0.3;
    // Two receptacle openings (dark slots)
    let slot_col = Rgb(30, 30, 30);
    for z_off in [-hh * 0.35, hh * 0.35] {
        // Two vertical slots + ground
        let sy = depth * 0.12;
        let slot_h = hh * 0.15;
        let slot_w = hw * 0.1;
        let gap = hw * 0.2;
        for x_off in [-gap, gap] {
            quad(
                s,
                [
                    tw(x_off - slot_w, sy, z_off - slot_h),
                    tw(x_off + slot_w, sy, z_off - slot_h),
                    tw(x_off + slot_w, sy, z_off + slot_h),
                    tw(x_off - slot_w, sy, z_off + slot_h),
                ],
                slot_col,
                owner,
            );
        }
        // Ground pin (half-circle → just a small quad below)
        quad(
            s,
            [
                tw(-slot_w, sy, z_off - slot_h * 1.5),
                tw(slot_w, sy, z_off - slot_h * 1.5),
                tw(slot_w, sy, z_off - slot_h * 1.1),
                tw(-slot_w, sy, z_off - slot_h * 1.1),
            ],
            slot_col,
            owner,
        );
    }
}

/// Switch: a wall plate with a toggle.
#[allow(clippy::too_many_arguments)]
fn switch_device(s: &mut Scene, c: V3, dir: Vec2, width: f64, height: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, col, owner);
        return;
    }
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hh = height / 2.0;

    // Cover plate
    let plate_col = Rgb(235, 230, 220);
    quad(s, [tw(-hw, depth * 0.1, -hh), tw(hw, depth * 0.1, -hh), tw(hw, depth * 0.1, hh), tw(-hw, depth * 0.1, hh)], plate_col, owner);

    // Toggle switch (raised rectangular nub)
    let toggle_col = Rgb(210, 205, 195);
    let tw2 = hw * 0.25;
    let th = hh * 0.4;
    let ty = depth * 0.18;
    quad(s, [tw(-tw2, ty, -th), tw(tw2, ty, -th), tw(tw2, ty, th), tw(-tw2, ty, th)], toggle_col, owner);
    // Toggle sides
    let tb = depth * 0.1;
    quad(s, [tw(-tw2, tb, -th), tw(-tw2, ty, -th), tw(-tw2, ty, th), tw(-tw2, tb, th)], darken(toggle_col, 0.9), owner);
    quad(s, [tw(tw2, ty, -th), tw(tw2, tb, -th), tw(tw2, tb, th), tw(tw2, ty, th)], darken(toggle_col, 0.9), owner);
}

/// Floor box: a round or square box flush with the floor.
#[allow(clippy::too_many_arguments)]
fn floor_box(s: &mut Scene, c: V3, width: f64, depth: f64, height: f64, col: Rgb, owner: Owner, lod: Lod) {
    // Floor boxes are cylindrical in practice
    let r = width.max(depth) / 2.0;
    closed_cylinder(s, c, Vec2 { x: 1.0, y: 0.0 }, r, height, darken(col, 0.7), owner, lod);

    // Lid on top
    let tw = local_frame(c, Vec2 { x: 1.0, y: 0.0 });
    let n = circle_segs(lod);
    let top_pts = ring(n, r, height / 2.0, &tw);
    disc(s, c + v3(0.0, 0.0, height / 2.0), &top_pts, Rgb(160, 160, 160), owner);
}

// ========================================================================================
// PANELS / DISCONNECTS
// ========================================================================================

/// Electrical panel: a metal cabinet mounted on a wall.
#[allow(clippy::too_many_arguments)]
fn panel(s: &mut Scene, c: V3, dir: Vec2, width: f64, height: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, col, owner);
        return;
    }
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hh = height / 2.0;
    let panel_col = Rgb(180, 180, 180); // Sheet metal gray

    // Main body box
    boxed(s, c, dir, width, depth, height, panel_col, owner);

    // Door outline (front face detail) — a slightly raised rectangle
    let door_inset = 0.5 / 12.0; // Half-inch inset from edges
    let dy = depth / 2.0 + 0.01; // Just in front of box surface
    let dw = hw - door_inset;
    let dh = hh - door_inset;
    quad(s, [tw(-dw, dy, -dh), tw(dw, dy, -dh), tw(dw, dy, dh), tw(-dw, dy, dh)], Rgb(190, 190, 190), owner);

    // Latch (small rectangle on right side)
    let latch_col = Rgb(100, 100, 100);
    let lw = 0.3 / 12.0;
    let lh = 1.0 / 12.0;
    quad(s, [tw(dw - lw, dy + 0.01, -lh), tw(dw, dy + 0.01, -lh), tw(dw, dy + 0.01, lh), tw(dw - lw, dy + 0.01, lh)], latch_col, owner);
}

/// Disconnect switch: similar to panel but smaller, with a visible handle.
#[allow(clippy::too_many_arguments)]
fn disconnect(s: &mut Scene, c: V3, dir: Vec2, width: f64, height: f64, depth: f64, col: Rgb, owner: Owner, lod: Lod) {
    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, col, owner);
        return;
    }
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hh = height / 2.0;
    let body_col = Rgb(170, 170, 170);

    boxed(s, c, dir, width, depth, height, body_col, owner);

    // Handle (red/orange switch lever on front)
    let handle_col = Rgb(200, 50, 30);
    let dy = depth / 2.0 + 0.01;
    let handle_w = hw * 0.3;
    let handle_h = hh * 0.5;
    quad(s, [tw(-handle_w, dy, -handle_h), tw(handle_w, dy, -handle_h), tw(handle_w, dy, handle_h), tw(-handle_w, dy, handle_h)], handle_col, owner);
}

// ========================================================================================
// PLUMBING FIXTURES
// ========================================================================================

/// Water closet (toilet): bowl + tank profile.
#[allow(clippy::too_many_arguments)]
fn water_closet(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, height: f64, _col: Rgb, owner: Owner, lod: Lod) {
    let tw = local_frame(c, dir);
    let porcelain = Rgb(240, 240, 235);
    let hw = width / 2.0;

    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, porcelain, owner);
        return;
    }

    // Bowl: an elongated oval footprint
    let bowl_depth = depth * 0.65;
    let bowl_h = height * 0.55;
    let n = circle_segs(lod);

    // Bowl — elliptical cylinder
    let bowl_c = tw(0.0, -depth / 2.0 + bowl_depth / 2.0, -height / 2.0 + bowl_h / 2.0);
    let bowl_rx = hw;
    let bowl_ry = bowl_depth / 2.0;
    let bot_ring: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            bowl_c + v3(a.cos() * bowl_rx, a.sin() * bowl_ry, -bowl_h / 2.0)
        })
        .collect();
    let top_ring: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            bowl_c + v3(a.cos() * bowl_rx, a.sin() * bowl_ry, bowl_h / 2.0)
        })
        .collect();
    cylinder_side(s, &bot_ring, &top_ring, porcelain, owner);
    disc(s, bowl_c + v3(0.0, 0.0, bowl_h / 2.0), &top_ring, porcelain, owner);

    // Tank: a rectangular block behind the bowl
    let tank_depth = depth * 0.3;
    let tank_h = height * 0.8;
    let tank_w = width * 0.8;
    let tank_c = tw(0.0, depth / 2.0 - tank_depth / 2.0, -height / 2.0 + tank_h / 2.0);
    boxed(s, tank_c, dir, tank_w, tank_depth, tank_h, porcelain, owner);

    // Seat rim (slightly wider at top)
    let seat_col = Rgb(250, 250, 245);
    let seat_w = hw * 1.05;
    let seat_d = bowl_depth / 2.0 * 1.05;
    let seat_ring: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            bowl_c + v3(a.cos() * seat_w, a.sin() * seat_d, bowl_h / 2.0 + 0.02)
        })
        .collect();
    // Thin annulus for seat
    for i in 0..n {
        let j = (i + 1) % n;
        quad(s, [top_ring[i], top_ring[j], seat_ring[j], seat_ring[i]], seat_col, owner);
    }
}

/// Lavatory (bathroom sink): a basin shape.
#[allow(clippy::too_many_arguments)]
fn lavatory(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, height: f64, _col: Rgb, owner: Owner, lod: Lod) {
    let porcelain = Rgb(240, 240, 235);

    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, porcelain, owner);
        return;
    }

    let _tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hd = depth / 2.0;
    let n = circle_segs(lod);

    // Basin: an oval top rim + tapered bowl inside
    let rim_z = height / 2.0;
    let bot_z = -height / 2.0;
    let bowl_rx = hw * 0.85;
    let bowl_ry = hd * 0.85;

    // Outer rim
    let outer: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            c + v3(a.cos() * hw, a.sin() * hd, rim_z)
        })
        .collect();
    // Inner rim (bowl opening)
    let inner: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            c + v3(a.cos() * bowl_rx, a.sin() * bowl_ry, rim_z)
        })
        .collect();
    // Bowl bottom (smaller)
    let drain_r = 1.0 / 12.0;
    let bottom: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            c + v3(a.cos() * drain_r, a.sin() * drain_r, bot_z)
        })
        .collect();

    // Rim annulus
    for i in 0..n {
        let j = (i + 1) % n;
        quad(s, [outer[i], outer[j], inner[j], inner[i]], porcelain, owner);
    }
    // Outer sides (from rim down)
    let outer_bot: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            c + v3(a.cos() * hw, a.sin() * hd, bot_z)
        })
        .collect();
    cylinder_side(s, &outer_bot, &outer, porcelain, owner);
    // Inner bowl surface
    cylinder_side(s, &bottom, &inner, Rgb(230, 230, 225), owner);
    // Bottom disc
    disc(s, c + v3(0.0, 0.0, bot_z), &bottom, Rgb(100, 100, 100), owner); // Dark drain
}

/// Urinal: a wall-hung porcelain fixture.
#[allow(clippy::too_many_arguments)]
fn urinal(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, height: f64, _col: Rgb, owner: Owner, lod: Lod) {
    let porcelain = Rgb(240, 240, 235);

    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, porcelain, owner);
        return;
    }

    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hh = height / 2.0;

    // Back plate against wall
    quad(s, [tw(-hw, 0.0, -hh), tw(hw, 0.0, -hh), tw(hw, 0.0, hh), tw(-hw, 0.0, hh)], porcelain, owner);

    // Basin: tapered from top to bottom, sticking out from wall
    let top_depth = depth * 0.3;
    let mid_depth = depth;
    let bot_depth = depth * 0.5;
    let top_w = hw * 0.8;
    let mid_w = hw;
    let bot_w = hw * 0.6;

    // Top lip
    quad(s, [tw(-top_w, 0.0, hh), tw(top_w, 0.0, hh), tw(top_w, top_depth, hh), tw(-top_w, top_depth, hh)], porcelain, owner);

    // Front face (3 vertical sections)
    quad(s, [tw(-top_w, top_depth, hh), tw(top_w, top_depth, hh), tw(mid_w, mid_depth, 0.0), tw(-mid_w, mid_depth, 0.0)], porcelain, owner);
    quad(s, [tw(-mid_w, mid_depth, 0.0), tw(mid_w, mid_depth, 0.0), tw(bot_w, bot_depth, -hh), tw(-bot_w, bot_depth, -hh)], porcelain, owner);

    // Bottom
    quad(s, [tw(-bot_w, bot_depth, -hh), tw(bot_w, bot_depth, -hh), tw(hw * 0.3, 0.0, -hh), tw(-hw * 0.3, 0.0, -hh)], porcelain, owner);

    // Sides
    quad(s, [tw(-hw, 0.0, hh), tw(-top_w, top_depth, hh), tw(-mid_w, mid_depth, 0.0), tw(-hw, 0.0, 0.0)], porcelain, owner);
    quad(s, [tw(-hw, 0.0, 0.0), tw(-mid_w, mid_depth, 0.0), tw(-bot_w, bot_depth, -hh), tw(-hw * 0.3, 0.0, -hh)], porcelain, owner);
    quad(s, [tw(top_w, top_depth, hh), tw(hw, 0.0, hh), tw(hw, 0.0, 0.0), tw(mid_w, mid_depth, 0.0)], porcelain, owner);
    quad(s, [tw(mid_w, mid_depth, 0.0), tw(hw, 0.0, 0.0), tw(hw * 0.3, 0.0, -hh), tw(bot_w, bot_depth, -hh)], porcelain, owner);
}

/// Shower: a recessed base pan.
#[allow(clippy::too_many_arguments)]
fn shower(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, height: f64, _col: Rgb, owner: Owner, lod: Lod) {
    let tile_col = Rgb(220, 220, 215);

    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, tile_col, owner);
        return;
    }

    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hd = depth / 2.0;

    // Shower pan: a shallow box with a raised lip
    let lip = 2.0 / 12.0; // 2-inch curb
    let pan_depth = height;

    // Floor of pan
    quad(
        s,
        [tw(-hw, -hd, -pan_depth / 2.0), tw(hw, -hd, -pan_depth / 2.0), tw(hw, hd, -pan_depth / 2.0), tw(-hw, hd, -pan_depth / 2.0)],
        tile_col,
        owner,
    );
    // Top rim
    quad(
        s,
        [
            tw(-hw, -hd, -pan_depth / 2.0 + lip),
            tw(hw, -hd, -pan_depth / 2.0 + lip),
            tw(hw, hd, -pan_depth / 2.0 + lip),
            tw(-hw, hd, -pan_depth / 2.0 + lip),
        ],
        Rgb(200, 200, 195),
        owner,
    );
    // Sides of lip
    let lz = -pan_depth / 2.0 + lip;
    let bz = -pan_depth / 2.0;
    quad(s, [tw(-hw, -hd, bz), tw(hw, -hd, bz), tw(hw, -hd, lz), tw(-hw, -hd, lz)], tile_col, owner);
    quad(s, [tw(hw, hd, bz), tw(-hw, hd, bz), tw(-hw, hd, lz), tw(hw, hd, lz)], tile_col, owner);
    quad(s, [tw(-hw, hd, bz), tw(-hw, -hd, bz), tw(-hw, -hd, lz), tw(-hw, hd, lz)], tile_col, owner);
    quad(s, [tw(hw, -hd, bz), tw(hw, hd, bz), tw(hw, hd, lz), tw(hw, -hd, lz)], tile_col, owner);

    // Drain (small dark circle in centre)
    let n = circle_segs(lod);
    let drain_pts = ring(n, 1.5 / 12.0, bz + 0.01, &tw);
    disc(s, tw(0.0, 0.0, bz + 0.01), &drain_pts, Rgb(80, 80, 80), owner);
}

/// Mop sink: a large floor-mounted basin.
#[allow(clippy::too_many_arguments)]
fn mop_sink(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, height: f64, _col: Rgb, owner: Owner, lod: Lod) {
    let grey = Rgb(180, 180, 175);

    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, grey, owner);
        return;
    }

    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hd = depth / 2.0;
    let hh = height / 2.0;

    // Outer walls
    let wall_t = 1.0 / 12.0; // 1-inch walls
    boxed(s, c, dir, width, depth, height, grey, owner);

    // Inner basin (darker, slightly recessed)
    let iw = hw - wall_t;
    let id = hd - wall_t;
    let basin_z = -hh + wall_t;
    quad(s, [tw(-iw, -id, basin_z), tw(iw, -id, basin_z), tw(iw, id, basin_z), tw(-iw, id, basin_z)], Rgb(160, 160, 155), owner);
}

/// Cylindrical water heater.
#[allow(clippy::too_many_arguments)]
fn water_heater(s: &mut Scene, c: V3, diameter: f64, height: f64, _col: Rgb, owner: Owner, lod: Lod) {
    let body_col = Rgb(220, 220, 215);
    let r = diameter / 2.0;
    closed_cylinder(s, c, Vec2 { x: 1.0, y: 0.0 }, r, height, body_col, owner, lod);
}

// ========================================================================================
// CASEWORK
// ========================================================================================

/// Base cabinet: box with a recessed toe kick.
#[allow(clippy::too_many_arguments)]
fn base_cabinet(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, height: f64, _col: Rgb, owner: Owner, lod: Lod) {
    let wood = Rgb(180, 140, 100);

    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, wood, owner);
        return;
    }

    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hd = depth / 2.0;
    let hh = height / 2.0;
    let toe_h = 3.5 / 12.0; // 3.5-inch toe kick
    let toe_d = 3.0 / 12.0; // 3-inch recess

    // Main cabinet body (above toe kick)
    let body_c = c + v3(0.0, 0.0, toe_h / 2.0);
    let body_h = height - toe_h;
    boxed(s, body_c, dir, width, depth, body_h, wood, owner);

    // Toe kick recessed front face (dark)
    let kick_col = Rgb(40, 35, 30);
    let kick_z_bot = -hh;
    let kick_z_top = -hh + toe_h;
    quad(
        s,
        [tw(-hw, hd - toe_d, kick_z_bot), tw(hw, hd - toe_d, kick_z_bot), tw(hw, hd - toe_d, kick_z_top), tw(-hw, hd - toe_d, kick_z_top)],
        kick_col,
        owner,
    );

    // Bottom of cabinet (horizontal surface at toe kick top)
    quad(
        s,
        [tw(-hw, hd - toe_d, kick_z_top), tw(hw, hd - toe_d, kick_z_top), tw(hw, hd, kick_z_top), tw(-hw, hd, kick_z_top)],
        darken(wood, 0.8),
        owner,
    );

    // Door lines (vertical groove down the centre front)
    let groove = Rgb(150, 115, 80);
    let gy = hd + 0.01;
    let gz_bot = kick_z_top;
    let gz_top = hh;
    let gw = 0.3 / 12.0;
    quad(s, [tw(-gw, gy, gz_bot), tw(gw, gy, gz_bot), tw(gw, gy, gz_top), tw(-gw, gy, gz_top)], groove, owner);
}

/// Wall cabinet: a box (no toe kick, narrower depth).
#[allow(clippy::too_many_arguments)]
fn wall_cabinet(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, height: f64, _col: Rgb, owner: Owner, lod: Lod) {
    let wood = Rgb(180, 140, 100);
    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, wood, owner);
        return;
    }

    let tw = local_frame(c, dir);
    let _hw = width / 2.0;
    let hh = height / 2.0;

    // Main body
    boxed(s, c, dir, width, depth, height, wood, owner);

    // Door line (same as base cabinet)
    let groove = Rgb(150, 115, 80);
    let gy = depth / 2.0 + 0.01;
    let gw = 0.3 / 12.0;
    quad(s, [tw(-gw, gy, -hh), tw(gw, gy, -hh), tw(gw, gy, hh), tw(-gw, gy, hh)], groove, owner);
}

/// Tall cabinet (pantry/utility): full height with toe kick.
#[allow(clippy::too_many_arguments)]
fn tall_cabinet(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, height: f64, col: Rgb, owner: Owner, lod: Lod) {
    // Same construction as base cabinet but taller
    base_cabinet(s, c, dir, width, depth, height, col, owner, lod);
}

/// Countertop slab.
#[allow(clippy::too_many_arguments)]
fn countertop(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, thickness: f64, _col: Rgb, owner: Owner, _lod: Lod) {
    let counter_col = Rgb(200, 195, 185); // Stone/quartz color
    boxed(s, c, dir, width, depth, thickness, counter_col, owner);

    // Slight edge profile: a narrow darker strip at the front
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let ht = thickness / 2.0;
    let edge_h = 0.75 / 12.0; // 3/4-inch edge
    let ey = depth / 2.0 + 0.005;
    quad(s, [tw(-hw, ey, -ht), tw(hw, ey, -ht), tw(hw, ey, -ht - edge_h), tw(-hw, ey, -ht - edge_h)], darken(counter_col, 0.85), owner);
}

/// Backsplash: a thin vertical panel behind the counter.
#[allow(clippy::too_many_arguments)]
fn backsplash(s: &mut Scene, c: V3, dir: Vec2, width: f64, height: f64, thickness: f64, _col: Rgb, owner: Owner, _lod: Lod) {
    let tile = Rgb(220, 215, 205);
    boxed(s, c, dir, width, thickness, height, tile, owner);
}

// ========================================================================================
// PIPE / CONDUIT FITTINGS
// ========================================================================================

/// Elbow fitting (90° or 45°).
#[allow(clippy::too_many_arguments)]
fn elbow(s: &mut Scene, c: V3, dir: Vec2, diameter: f64, angle_deg: f64, col: Rgb, owner: Owner, lod: Lod) {
    let n = circle_segs(lod);
    let r = diameter / 2.0;
    let bend_r = diameter; // Centre-line bend radius = 1 diameter
    let angle = angle_deg * PI / 180.0;
    let steps = n.max(6);
    let tw = local_frame(c, dir);

    // Generate rings along the bend
    let mut prev_ring: Vec<V3> = Vec::new();
    for i in 0..=steps {
        let t = angle * (i as f64) / (steps as f64);
        // Centre of the ring
        let cx = bend_r * t.sin();
        let cz = bend_r * (1.0 - t.cos());
        // Ring normal direction (tangent to bend)
        let nx = t.cos();
        let nz = t.sin();
        let ring_pts: Vec<V3> = (0..n)
            .map(|j| {
                let a = 2.0 * PI * (j as f64) / (n as f64);
                let ly = a.cos() * r;
                let lr = a.sin() * r;
                tw(cx + lr * nx, ly, cz + lr * nz)
            })
            .collect();
        if !prev_ring.is_empty() {
            cylinder_side(s, &prev_ring, &ring_pts, col, owner);
        }
        prev_ring = ring_pts;
    }
}

/// Tee fitting.
#[allow(clippy::too_many_arguments)]
fn tee_fitting(s: &mut Scene, c: V3, dir: Vec2, diameter: f64, col: Rgb, owner: Owner, lod: Lod) {
    let r = diameter / 2.0;
    let run_len = diameter * 2.0;

    // Main run (straight through)
    closed_cylinder(s, c, dir, r, run_len, col, owner, lod);

    // Branch (perpendicular)
    let branch_dir = Vec2 { x: -dir.y, y: dir.x };
    let branch_c = c + v3(branch_dir.x, branch_dir.y, 0.0) * (r + diameter / 2.0);
    closed_cylinder(s, branch_c, branch_dir, r, diameter, col, owner, lod);
}

/// Reducer: a cone between two pipe sizes.
#[allow(clippy::too_many_arguments)]
fn reducer(s: &mut Scene, c: V3, dir: Vec2, large: f64, small: f64, length: f64, col: Rgb, owner: Owner, lod: Lod) {
    let tw = local_frame(c, dir);
    let n = circle_segs(lod);
    let r1 = large / 2.0;
    let r2 = small / 2.0;
    let hl = length / 2.0;

    let bot = ring(n, r1, -hl, &tw);
    let top = ring(n, r2, hl, &tw);
    cylinder_side(s, &bot, &top, col, owner);
    disc(s, tw(0.0, 0.0, -hl), &bot, col, owner);
    disc(s, tw(0.0, 0.0, hl), &top, col, owner);
}

/// Coupling: a short cylinder joining two pipes.
#[allow(clippy::too_many_arguments)]
fn coupling(s: &mut Scene, c: V3, dir: Vec2, diameter: f64, length: f64, col: Rgb, owner: Owner, lod: Lod) {
    closed_cylinder(s, c, dir, diameter / 2.0, length, col, owner, lod);
}

// ========================================================================================
// Colour helpers
// ========================================================================================

fn darken(c: Rgb, factor: f64) -> Rgb {
    Rgb((c.0 as f64 * factor).clamp(0.0, 255.0) as u8, (c.1 as f64 * factor).clamp(0.0, 255.0) as u8, (c.2 as f64 * factor).clamp(0.0, 255.0) as u8)
}

fn brighten(c: Rgb, factor: f64) -> Rgb {
    Rgb((c.0 as f64 * factor).clamp(0.0, 255.0) as u8, (c.1 as f64 * factor).clamp(0.0, 255.0) as u8, (c.2 as f64 * factor).clamp(0.0, 255.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::Part;

    fn empty_scene() -> Scene {
        Scene::default()
    }

    #[test]
    fn classify_troffer_by_kind() {
        let f = classify("light_fixture", "A", [2.0, 4.0, 0.33]);
        assert!(matches!(f, Some(FixtureFamily::Troffer { .. })));
    }

    #[test]
    fn classify_downlight_by_size() {
        let f = classify("light_fixture", "DL", [0.5, 0.5, 0.33]);
        assert!(matches!(f, Some(FixtureFamily::Downlight { .. })));
    }

    #[test]
    fn classify_receptacle() {
        let f = classify("receptacle", "REC", [0.3, 0.15, 0.45]);
        assert!(matches!(f, Some(FixtureFamily::Receptacle { .. })));
    }

    #[test]
    fn classify_switch() {
        let f = classify("switch", "S", [0.25, 0.15, 0.42]);
        assert!(matches!(f, Some(FixtureFamily::Switch { .. })));
    }

    #[test]
    fn classify_panel() {
        let f = classify("panel", "LP-1", [1.5, 0.5, 3.0]);
        assert!(matches!(f, Some(FixtureFamily::Panel { .. })));
    }

    #[test]
    fn classify_water_closet() {
        let f = classify("water_closet", "WC-1", [1.5, 2.3, 1.25]);
        assert!(matches!(f, Some(FixtureFamily::WaterCloset { .. })));
    }

    #[test]
    fn classify_lavatory() {
        let f = classify("lavatory", "LAV-1", [1.67, 1.75, 0.67]);
        assert!(matches!(f, Some(FixtureFamily::Lavatory { .. })));
    }

    #[test]
    fn classify_unknown_returns_none() {
        let f = classify("unknown_thing", "X99", [1.0, 1.0, 1.0]);
        assert!(f.is_none());
    }

    #[test]
    fn troffer_generates_triangles() {
        let mut s = empty_scene();
        s.part = Part::Device;
        let family = FixtureFamily::Troffer { width_ft: 2.0, length_ft: 4.0, depth_ft: 0.33 };
        let ok = generate(&mut s, &family, v3(0.0, 0.0, 10.0), Vec2 { x: 1.0, y: 0.0 }, Rgb(230, 230, 230), Owner::default(), Lod::Detailed);
        assert!(ok);
        assert!(!s.tris.is_empty(), "troffer should generate triangles");
    }

    #[test]
    fn downlight_generates_triangles() {
        let mut s = empty_scene();
        s.part = Part::Device;
        let family = FixtureFamily::Downlight { aperture_ft: 0.5, depth_ft: 0.33 };
        let ok = generate(&mut s, &family, v3(0.0, 0.0, 10.0), Vec2 { x: 1.0, y: 0.0 }, Rgb(230, 230, 230), Owner::default(), Lod::Detailed);
        assert!(ok);
        assert!(!s.tris.is_empty(), "downlight should generate triangles");
    }

    #[test]
    fn water_closet_generates_triangles() {
        let mut s = empty_scene();
        s.part = Part::Device;
        let family = FixtureFamily::WaterCloset { width_ft: 1.5, depth_ft: 2.3, height_ft: 1.25 };
        let ok = generate(&mut s, &family, v3(5.0, 5.0, 0.0), Vec2 { x: 0.0, y: 1.0 }, Rgb(240, 240, 235), Owner::default(), Lod::Detailed);
        assert!(ok);
        assert!(!s.tris.is_empty(), "water closet should generate triangles");
    }

    #[test]
    fn base_cabinet_generates_triangles() {
        let mut s = empty_scene();
        s.part = Part::Device;
        let family = FixtureFamily::BaseCabinet { width_ft: 2.0, depth_ft: 2.0, height_ft: 2.83 };
        let ok = generate(&mut s, &family, v3(3.0, 3.0, 0.0), Vec2 { x: 1.0, y: 0.0 }, Rgb(180, 140, 100), Owner::default(), Lod::Detailed);
        assert!(ok);
        assert!(!s.tris.is_empty(), "base cabinet should generate triangles");
    }

    #[test]
    fn elbow_generates_triangles() {
        let mut s = empty_scene();
        s.part = Part::Device;
        let family = FixtureFamily::Elbow90 { diameter_ft: 0.5 };
        let ok = generate(&mut s, &family, v3(0.0, 0.0, 8.0), Vec2 { x: 1.0, y: 0.0 }, Rgb(150, 150, 150), Owner::default(), Lod::Detailed);
        assert!(ok);
        assert!(!s.tris.is_empty(), "elbow should generate triangles");
    }

    #[test]
    fn lod_box_returns_false() {
        let mut s = empty_scene();
        s.part = Part::Device;
        let family = FixtureFamily::Troffer { width_ft: 2.0, length_ft: 4.0, depth_ft: 0.33 };
        let ok = generate(&mut s, &family, v3(0.0, 0.0, 10.0), Vec2 { x: 1.0, y: 0.0 }, Rgb(230, 230, 230), Owner::default(), Lod::Box);
        assert!(!ok, "Lod::Box should return false for caller fallback");
    }

    #[test]
    fn all_families_build_at_all_lods() {
        let families: Vec<FixtureFamily> = vec![
            FixtureFamily::Troffer { width_ft: 2.0, length_ft: 4.0, depth_ft: 0.33 },
            FixtureFamily::Downlight { aperture_ft: 0.5, depth_ft: 0.33 },
            FixtureFamily::LinearPendant { length_ft: 4.0, width_ft: 0.3, drop_ft: 2.0 },
            FixtureFamily::SlotLight { length_ft: 4.0, width_ft: 0.2 },
            FixtureFamily::WallPack { width_ft: 0.75, height_ft: 0.5, depth_ft: 0.4 },
            FixtureFamily::Sconce { width_ft: 0.5, height_ft: 0.67, depth_ft: 0.33 },
            FixtureFamily::StripWrap { length_ft: 4.0, width_ft: 0.5, depth_ft: 0.25 },
            FixtureFamily::ExitSign { width_ft: 1.0, height_ft: 0.67, depth_ft: 0.17 },
            FixtureFamily::EmergencyLight { width_ft: 1.0, height_ft: 0.5, depth_ft: 0.33 },
            FixtureFamily::Receptacle { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 },
            FixtureFamily::Switch { width_ft: 0.25, height_ft: 0.42, depth_ft: 0.15 },
            FixtureFamily::FloorBox { width_ft: 0.5, depth_ft: 0.5, height_ft: 0.33 },
            FixtureFamily::DataOutlet { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 },
            FixtureFamily::Panel { width_ft: 1.5, height_ft: 3.0, depth_ft: 0.5 },
            FixtureFamily::Disconnect { width_ft: 1.0, height_ft: 1.5, depth_ft: 0.5 },
            FixtureFamily::WaterCloset { width_ft: 1.5, depth_ft: 2.3, height_ft: 1.25 },
            FixtureFamily::Lavatory { width_ft: 1.67, depth_ft: 1.75, height_ft: 0.67 },
            FixtureFamily::Urinal { width_ft: 1.5, depth_ft: 1.17, height_ft: 2.0 },
            FixtureFamily::Shower { width_ft: 3.0, depth_ft: 3.0, height_ft: 0.33 },
            FixtureFamily::MopSink { width_ft: 2.0, depth_ft: 2.0, height_ft: 1.0 },
            FixtureFamily::WaterHeater { diameter_ft: 1.67, height_ft: 5.0 },
            FixtureFamily::BaseCabinet { width_ft: 2.0, depth_ft: 2.0, height_ft: 2.83 },
            FixtureFamily::WallCabinet { width_ft: 2.0, depth_ft: 1.0, height_ft: 2.5 },
            FixtureFamily::TallCabinet { width_ft: 2.0, depth_ft: 2.0, height_ft: 7.0 },
            FixtureFamily::Countertop { width_ft: 3.0, depth_ft: 2.0, thickness_ft: 1.5 / 12.0 },
            FixtureFamily::Backsplash { width_ft: 3.0, height_ft: 0.33, thickness_ft: 0.5 / 12.0 },
            FixtureFamily::Elbow90 { diameter_ft: 0.5 },
            FixtureFamily::Elbow45 { diameter_ft: 0.5 },
            FixtureFamily::Tee { diameter_ft: 0.5 },
            FixtureFamily::Reducer { large_ft: 0.5, small_ft: 0.33, length_ft: 0.5 },
            FixtureFamily::Coupling { diameter_ft: 0.5, length_ft: 0.33 },
        ];
        for lod in [Lod::Detailed, Lod::Simple] {
            for family in &families {
                let mut s = empty_scene();
                s.part = Part::Device;
                let ok = generate(&mut s, family, v3(5.0, 5.0, 5.0), Vec2 { x: 1.0, y: 0.0 }, Rgb(200, 200, 200), Owner::default(), lod);
                assert!(ok, "{family:?} at {lod:?} should generate geometry");
                assert!(!s.tris.is_empty(), "{family:?} at {lod:?} should produce triangles");
                // Triangle count must be a multiple of 3
                assert_eq!(s.tris.len() % 3, 0, "{family:?} at {lod:?} has non-triangle vertex count");
            }
        }
    }

    #[test]
    fn sizes_from_type_params() {
        // Troffer dimensions should come from the type parameters
        let family = FixtureFamily::Troffer { width_ft: 2.0, length_ft: 4.0, depth_ft: 0.33 };
        let mut s = empty_scene();
        s.part = Part::Device;
        generate(&mut s, &family, v3(0.0, 0.0, 0.0), Vec2 { x: 1.0, y: 0.0 }, Rgb(200, 200, 200), Owner::default(), Lod::Detailed);
        // Verify vertices fall within expected bounds
        for v in &s.tris {
            assert!(v.p[0].abs() <= 2.5, "troffer vertex x out of expected range: {}", v.p[0]);
            assert!(v.p[1].abs() <= 4.5, "troffer vertex y out of expected range: {}", v.p[1]);
        }
    }

    #[test]
    fn ifc_export_classification() {
        // Verify that classify returns the right family for IFC-relevant types
        assert!(matches!(classify("light_fixture", "A", [2.0, 4.0, 0.33]), Some(FixtureFamily::Troffer { .. })));
        assert!(matches!(classify("receptacle", "REC", [0.3, 0.15, 0.45]), Some(FixtureFamily::Receptacle { .. })));
        assert!(matches!(classify("water_closet", "WC-1", [1.5, 2.3, 1.25]), Some(FixtureFamily::WaterCloset { .. })));
    }
}
