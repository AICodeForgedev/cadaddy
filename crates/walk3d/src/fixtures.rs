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
    /// 2D plan symbol (flat at z).
    Symbol,
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
    Gfci { width_ft: f64, height_ft: f64, depth_ft: f64 },
    UsbOutlet { width_ft: f64, height_ft: f64, depth_ft: f64 },
    Switch { width_ft: f64, height_ft: f64, depth_ft: f64 },
    Dimmer { width_ft: f64, height_ft: f64, depth_ft: f64 },
    FloorBox { width_ft: f64, depth_ft: f64, height_ft: f64 },
    DataOutlet { width_ft: f64, height_ft: f64, depth_ft: f64 },
    TvOutlet { width_ft: f64, height_ft: f64, depth_ft: f64 },
    ClockHanger { width_ft: f64, height_ft: f64, depth_ft: f64 },

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
    Drawers { width_ft: f64, depth_ft: f64, height_ft: f64 },
    ApplianceOpening { width_ft: f64, depth_ft: f64, height_ft: f64 },

    // --- Pipe / conduit fittings ---
    Elbow90 { diameter_ft: f64 },
    Elbow45 { diameter_ft: f64 },
    Tee { diameter_ft: f64 },
    Reducer { large_ft: f64, small_ft: f64, length_ft: f64 },
    Coupling { diameter_ft: f64, length_ft: f64 },
}

/// Sizes extracted from device attributes (schedule/type record), preferred over mount sizes.
/// Each field is `None` when the attribute was not present or could not be parsed.
#[derive(Clone, Debug, Default)]
pub struct ScheduleSize {
    pub width_in: Option<f64>,
    pub depth_in: Option<f64>,
    pub height_in: Option<f64>,
    pub aperture_in: Option<f64>,
    pub length_in: Option<f64>,
    pub pendant_drop_in: Option<f64>,
    /// Whether these values came from a schedule (true) or fell back to the mount (false).
    pub from_schedule: bool,
}

impl ScheduleSize {
    /// Parse schedule sizes from device attributes. Recognised tags (case-insensitive):
    /// SIZE (WxD or WxDxH, inches), APERTURE, LENGTH, WIDTH, DEPTH, HEIGHT, PENDANT_DROP, DROP.
    pub fn from_attrs(attrs: &[(String, String)]) -> Self {
        let find = |tag: &str| -> Option<&str> {
            attrs.iter().find(|(t, _)| t.eq_ignore_ascii_case(tag)).map(|(_, v)| v.as_str())
        };
        let parse_in = |s: &str| -> Option<f64> {
            let s = s.trim().trim_end_matches('"').trim_end_matches("in");
            s.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
        };
        let mut out = ScheduleSize::default();
        // "SIZE" may be "WxD" or "WxDxH" in inches.
        if let Some(sz) = find("SIZE") {
            let parts: Vec<&str> = sz.split('x').collect();
            if parts.len() >= 2 {
                out.width_in = parse_in(parts[0]);
                out.depth_in = parse_in(parts[1]);
                if parts.len() >= 3 {
                    out.height_in = parse_in(parts[2]);
                }
                out.from_schedule = out.width_in.is_some();
            }
        }
        if let Some(v) = find("APERTURE").and_then(parse_in) {
            out.aperture_in = Some(v);
            out.from_schedule = true;
        }
        if let Some(v) = find("LENGTH").and_then(parse_in) {
            out.length_in = Some(v);
            out.from_schedule = true;
        }
        if out.width_in.is_none()
            && let Some(v) = find("WIDTH").and_then(parse_in)
        {
            out.width_in = Some(v);
            out.from_schedule = true;
        }
        if out.depth_in.is_none()
            && let Some(v) = find("DEPTH").and_then(parse_in)
        {
            out.depth_in = Some(v);
            out.from_schedule = true;
        }
        if out.height_in.is_none()
            && let Some(v) = find("HEIGHT").and_then(parse_in)
        {
            out.height_in = Some(v);
            out.from_schedule = true;
        }
        if let Some(v) = find("PENDANT_DROP").or_else(|| find("DROP")).and_then(parse_in) {
            out.pendant_drop_in = Some(v);
            out.from_schedule = true;
        }
        out
    }

    /// Resolve a `[width, depth, height]` in feet, using schedule values (inches→feet) where
    /// present, falling back to the mount's `size_ft`.
    pub fn resolve_ft(&self, mount_ft: [f64; 3]) -> [f64; 3] {
        let or_mount = |sched: Option<f64>, idx: usize| match sched {
            Some(v) => v / 12.0,
            None => mount_ft[idx],
        };
        [or_mount(self.width_in, 0), or_mount(self.depth_in, 1), or_mount(self.height_in, 2)]
    }
}

/// Check if a whitespace-delimited token appears in `haystack`.
fn has_token(haystack: &str, token: &str) -> bool {
    haystack.split(|c: char| !c.is_alphanumeric() && c != '_').any(|w| w.eq_ignore_ascii_case(token))
}

/// Classify a device type string into a fixture family with appropriate dimensions.
///
/// `kind` is the lowercased block name; `type_tag` is the TYPE attribute value (original case);
/// `size_ft` is the resolved `[width, depth, height]` in feet (from schedule or mount fallback);
/// `sched` carries the parsed schedule sizes for families that need specific dimensions.
pub fn classify(kind: &str, type_tag: &str, size_ft: [f64; 3]) -> Option<FixtureFamily> {
    classify_with_schedule(kind, type_tag, size_ft, &ScheduleSize::default())
}

/// Full classification with schedule data.
pub fn classify_with_schedule(kind: &str, type_tag: &str, size_ft: [f64; 3], sched: &ScheduleSize) -> Option<FixtureFamily> {
    let [w, d, h] = size_ft;
    let k = kind.to_ascii_lowercase();
    let tag = type_tag.trim();

    // ---- 1. Exact device-kind matches (most specific) ----

    // Plumbing — check before lights, because "water_closet" etc. are unambiguous.
    if k.contains("water_closet") || k.contains("toilet") || has_token(&k, "wc") {
        return Some(FixtureFamily::WaterCloset { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("lavatory") || has_token(&k, "lav") || k.contains("sink") {
        return Some(FixtureFamily::Lavatory { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("urinal") {
        return Some(FixtureFamily::Urinal { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("shower") {
        return Some(FixtureFamily::Shower { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("mop") {
        return Some(FixtureFamily::MopSink { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("water_heater") || k.contains("waterheater") {
        return Some(FixtureFamily::WaterHeater { diameter_ft: w.max(d), height_ft: h });
    }

    // Electrical — specific kinds first, generic "outlet"/"receptacle" last.
    if k.contains("gfci") {
        return Some(FixtureFamily::Gfci { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("usb") {
        return Some(FixtureFamily::UsbOutlet { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("dimmer") {
        return Some(FixtureFamily::Dimmer { width_ft: w, height_ft: h, depth_ft: d });
    }
    // "light_switch" should be a switch, not a light; check before the generic "light" rule.
    if k.contains("switch") {
        return Some(FixtureFamily::Switch { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("floor_box") || k.contains("floorbox") {
        return Some(FixtureFamily::FloorBox { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("data_outlet") || (k.contains("data") && k.contains("outlet")) {
        return Some(FixtureFamily::DataOutlet { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("tv") && (k.contains("outlet") || k.contains("jack")) {
        return Some(FixtureFamily::TvOutlet { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("clock_hanger") || (k.contains("clock") && k.contains("hanger")) {
        return Some(FixtureFamily::ClockHanger { width_ft: w, height_ft: h, depth_ft: d });
    }
    // Generic receptacle/outlet — after all specific outlet types.
    if k.contains("receptacle") || k.contains("outlet") {
        return Some(FixtureFamily::Receptacle { width_ft: w, height_ft: h, depth_ft: d });
    }

    // Panels and disconnects.
    if k.contains("panel") {
        return Some(FixtureFamily::Panel { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("disconnect") {
        return Some(FixtureFamily::Disconnect { width_ft: w, height_ft: h, depth_ft: d });
    }

    // Casework (before lights, because block names can contain "light" for lighting-related cabinets).
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
    if k.contains("appliance") && k.contains("opening") {
        return Some(FixtureFamily::ApplianceOpening { width_ft: w, depth_ft: d, height_ft: h });
    }
    if k.contains("drawer") {
        return Some(FixtureFamily::Drawers { width_ft: w, depth_ft: d, height_ft: h });
    }

    // Luminaires — specific kinds first.
    if k.contains("troffer") {
        return Some(FixtureFamily::Troffer { width_ft: w, length_ft: d.max(w), depth_ft: h.min(0.5) });
    }
    if k.contains("downlight") {
        let ap = sched.aperture_in.map(|v| v / 12.0).unwrap_or_else(|| w.min(d));
        return Some(FixtureFamily::Downlight { aperture_ft: ap, depth_ft: h });
    }
    if k.contains("pendant") || k.contains("linear_pendant") {
        let drop = sched.pendant_drop_in.map(|v| v / 12.0).unwrap_or(2.0);
        return Some(FixtureFamily::LinearPendant { length_ft: w.max(d), width_ft: w.min(d).max(0.1), drop_ft: drop });
    }
    if k.contains("slot") {
        return Some(FixtureFamily::SlotLight { length_ft: w.max(d), width_ft: w.min(d).max(1.0 / 12.0) });
    }
    if k.contains("wall_pack") || k.contains("wallpack") {
        return Some(FixtureFamily::WallPack { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("sconce") {
        return Some(FixtureFamily::Sconce { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("strip") || k.contains("wrap") {
        return Some(FixtureFamily::StripWrap { length_ft: w.max(d), width_ft: w.min(d), depth_ft: h });
    }
    if k.contains("exit") {
        return Some(FixtureFamily::ExitSign { width_ft: w, height_ft: h, depth_ft: d });
    }
    if k.contains("emergency") {
        return Some(FixtureFamily::EmergencyLight { width_ft: w, height_ft: h, depth_ft: d });
    }

    // Pipe/conduit fittings.
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

    // ---- 2. Generic "light_fixture" or "light" in the kind: choose by size ----
    if k == "light_fixture" || has_token(&k, "light") {
        // Whole-token tag tests (case-insensitive on the original tag).
        if is_troffer_tag(tag) {
            return Some(FixtureFamily::Troffer { width_ft: w, length_ft: d.max(w), depth_ft: h.min(0.5) });
        }
        if has_token(tag, "DL") || tag.eq_ignore_ascii_case("downlight") {
            let ap = sched.aperture_in.map(|v| v / 12.0).unwrap_or_else(|| w.min(d));
            return Some(FixtureFamily::Downlight { aperture_ft: ap, depth_ft: h });
        }
        if has_token(tag, "LP") || tag.to_ascii_lowercase().contains("pendant") {
            let drop = sched.pendant_drop_in.map(|v| v / 12.0).unwrap_or(2.0);
            return Some(FixtureFamily::LinearPendant { length_ft: w.max(d), width_ft: w.min(d).max(0.1), drop_ft: drop });
        }
        if has_token(tag, "WP") || tag.to_ascii_lowercase().contains("wallpack") {
            return Some(FixtureFamily::WallPack { width_ft: w, height_ft: h, depth_ft: d });
        }
        if tag.to_ascii_lowercase().contains("exit") || has_token(tag, "ES") {
            return Some(FixtureFamily::ExitSign { width_ft: w, height_ft: h, depth_ft: d });
        }
        if tag.to_ascii_lowercase().contains("emergency") || has_token(tag, "EM") {
            return Some(FixtureFamily::EmergencyLight { width_ft: w, height_ft: h, depth_ft: d });
        }
        if tag.to_ascii_lowercase().contains("sconce") {
            return Some(FixtureFamily::Sconce { width_ft: w, height_ft: h, depth_ft: d });
        }
        if tag.to_ascii_lowercase().contains("strip") || tag.to_ascii_lowercase().contains("wrap") {
            return Some(FixtureFamily::StripWrap { length_ft: w.max(d), width_ft: w.min(d), depth_ft: h });
        }
        if tag.to_ascii_lowercase().contains("slot") {
            return Some(FixtureFamily::SlotLight { length_ft: w.max(d), width_ft: w.min(d).max(1.0 / 12.0) });
        }
        // Fallback by size: small = downlight, else troffer.
        if w <= 0.75 && d <= 0.75 {
            let ap = sched.aperture_in.map(|v| v / 12.0).unwrap_or_else(|| w.max(d));
            return Some(FixtureFamily::Downlight { aperture_ft: ap, depth_ft: h });
        }
        return Some(FixtureFamily::Troffer { width_ft: w, length_ft: d.max(w), depth_ft: h.min(0.5) });
    }

    // ---- 3. Tag-only classification (no kind matched): whole-token matching ----
    if has_token(tag, "WC") {
        return Some(FixtureFamily::WaterCloset { width_ft: w, depth_ft: d, height_ft: h });
    }
    if has_token(tag, "LAV") {
        return Some(FixtureFamily::Lavatory { width_ft: w, depth_ft: d, height_ft: h });
    }
    if has_token(tag, "REC") || has_token(tag, "RECEP") {
        return Some(FixtureFamily::Receptacle { width_ft: w, height_ft: h, depth_ft: d });
    }
    if has_token(tag, "GFCI") || has_token(tag, "GFI") {
        return Some(FixtureFamily::Gfci { width_ft: w, height_ft: h, depth_ft: d });
    }
    if has_token(tag, "USB") {
        return Some(FixtureFamily::UsbOutlet { width_ft: w, height_ft: h, depth_ft: d });
    }
    if has_token(tag, "DIM") || has_token(tag, "DIMMER") {
        return Some(FixtureFamily::Dimmer { width_ft: w, height_ft: h, depth_ft: d });
    }
    if has_token(tag, "TV") {
        return Some(FixtureFamily::TvOutlet { width_ft: w, height_ft: h, depth_ft: d });
    }
    if tag.to_ascii_uppercase().starts_with("LP-") || tag.to_ascii_uppercase().starts_with("PP-")
        || tag.to_ascii_uppercase().starts_with("DP-")
    {
        return Some(FixtureFamily::Panel { width_ft: w, height_ft: h, depth_ft: d });
    }
    if has_token(tag, "DISC") || has_token(tag, "DISCONNECT") {
        return Some(FixtureFamily::Disconnect { width_ft: w, height_ft: h, depth_ft: d });
    }

    None
}

/// The IFC entity class for a fixture family.
pub fn ifc_class(family: &FixtureFamily) -> &'static str {
    match family {
        FixtureFamily::Troffer { .. }
        | FixtureFamily::Downlight { .. }
        | FixtureFamily::LinearPendant { .. }
        | FixtureFamily::SlotLight { .. }
        | FixtureFamily::WallPack { .. }
        | FixtureFamily::Sconce { .. }
        | FixtureFamily::StripWrap { .. }
        | FixtureFamily::ExitSign { .. }
        | FixtureFamily::EmergencyLight { .. } => "IfcLightFixture",

        FixtureFamily::Receptacle { .. }
        | FixtureFamily::Gfci { .. }
        | FixtureFamily::UsbOutlet { .. }
        | FixtureFamily::DataOutlet { .. }
        | FixtureFamily::TvOutlet { .. }
        | FixtureFamily::FloorBox { .. }
        | FixtureFamily::ClockHanger { .. } => "IfcOutlet",

        FixtureFamily::Switch { .. } | FixtureFamily::Dimmer { .. } => "IfcSwitchingDevice",

        FixtureFamily::Panel { .. } | FixtureFamily::Disconnect { .. } => "IfcElectricDistributionBoard",

        FixtureFamily::WaterCloset { .. }
        | FixtureFamily::Lavatory { .. }
        | FixtureFamily::Urinal { .. }
        | FixtureFamily::MopSink { .. } => "IfcSanitaryTerminal",

        FixtureFamily::Shower { .. } => "IfcSanitaryTerminal",
        FixtureFamily::WaterHeater { .. } => "IfcBoiler",

        FixtureFamily::BaseCabinet { .. }
        | FixtureFamily::WallCabinet { .. }
        | FixtureFamily::TallCabinet { .. }
        | FixtureFamily::Countertop { .. }
        | FixtureFamily::Backsplash { .. }
        | FixtureFamily::Drawers { .. }
        | FixtureFamily::ApplianceOpening { .. } => "IfcFurniture",

        FixtureFamily::Elbow90 { .. }
        | FixtureFamily::Elbow45 { .. }
        | FixtureFamily::Tee { .. }
        | FixtureFamily::Reducer { .. }
        | FixtureFamily::Coupling { .. } => "IfcPipeFitting",
    }
}

/// Generate fixture geometry into the scene. Returns `true` if geometry was produced, `false`
/// if the caller should fall back to `boxed()`.
pub fn generate(s: &mut Scene, family: &FixtureFamily, centre: V3, dir: Vec2, normal: Vec2, col: Rgb, owner: Owner, lod: Lod) -> bool {
    if lod == Lod::Box {
        return false; // Caller uses boxed()
    }
    // For plan symbol LOD, draw a 2D plan outline.
    if lod == Lod::Symbol {
        return plan_symbol(s, family, centre, dir, col, owner);
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
        FixtureFamily::Receptacle { width_ft, height_ft, depth_ft }
        | FixtureFamily::Gfci { width_ft, height_ft, depth_ft }
        | FixtureFamily::UsbOutlet { width_ft, height_ft, depth_ft }
        | FixtureFamily::TvOutlet { width_ft, height_ft, depth_ft }
        | FixtureFamily::ClockHanger { width_ft, height_ft, depth_ft } => {
            receptacle(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod)
        }
        FixtureFamily::Switch { width_ft, height_ft, depth_ft }
        | FixtureFamily::Dimmer { width_ft, height_ft, depth_ft } => {
            switch_device(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod)
        }
        FixtureFamily::FloorBox { width_ft, depth_ft, height_ft } => floor_box(s, centre, *width_ft, *depth_ft, *height_ft, col, owner, lod),
        FixtureFamily::DataOutlet { width_ft, height_ft, depth_ft } => {
            receptacle(s, centre, dir, *width_ft, *height_ft, *depth_ft, col, owner, lod)
        }

        // --- Panels / disconnects ---
        FixtureFamily::Panel { width_ft, height_ft, depth_ft } => panel(s, centre, normal, *width_ft, *height_ft, *depth_ft, col, owner, lod),
        FixtureFamily::Disconnect { width_ft, height_ft, depth_ft } => disconnect(s, centre, normal, *width_ft, *height_ft, *depth_ft, col, owner, lod),

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
        FixtureFamily::ApplianceOpening { width_ft, depth_ft, height_ft } => {
            // Thin-walled box outline (like a base cabinet without door grooves).
            base_cabinet(s, centre, dir, *width_ft, *depth_ft, *height_ft, col, owner, lod)
        }
        FixtureFamily::Drawers { width_ft, depth_ft, height_ft } => {
            base_cabinet(s, centre, dir, *width_ft, *depth_ft, *height_ft, col, owner, lod)
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

/// Bounding box `[width, depth]` of a family's plan symbol, feet. Used for extents tests.
pub fn plan_extents(family: &FixtureFamily) -> [f64; 2] {
    match family {
        FixtureFamily::Troffer { width_ft, length_ft, .. } => [*width_ft, *length_ft],
        FixtureFamily::Downlight { aperture_ft, .. } => [*aperture_ft, *aperture_ft],
        FixtureFamily::LinearPendant { length_ft, width_ft, .. } => [*length_ft, *width_ft],
        FixtureFamily::SlotLight { length_ft, width_ft, .. } => [*length_ft, *width_ft],
        FixtureFamily::WallPack { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::Sconce { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::StripWrap { length_ft, width_ft, .. } => [*length_ft, *width_ft],
        FixtureFamily::ExitSign { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::EmergencyLight { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::Receptacle { width_ft, depth_ft, .. }
        | FixtureFamily::Gfci { width_ft, depth_ft, .. }
        | FixtureFamily::UsbOutlet { width_ft, depth_ft, .. }
        | FixtureFamily::TvOutlet { width_ft, depth_ft, .. }
        | FixtureFamily::ClockHanger { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::Switch { width_ft, depth_ft, .. }
        | FixtureFamily::Dimmer { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::FloorBox { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::DataOutlet { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::Panel { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::Disconnect { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::WaterCloset { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::Lavatory { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::Urinal { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::Shower { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::MopSink { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::WaterHeater { diameter_ft, .. } => [*diameter_ft, *diameter_ft],
        FixtureFamily::BaseCabinet { width_ft, depth_ft, .. }
        | FixtureFamily::WallCabinet { width_ft, depth_ft, .. }
        | FixtureFamily::TallCabinet { width_ft, depth_ft, .. }
        | FixtureFamily::Drawers { width_ft, depth_ft, .. }
        | FixtureFamily::ApplianceOpening { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::Countertop { width_ft, depth_ft, .. } => [*width_ft, *depth_ft],
        FixtureFamily::Backsplash { width_ft, .. } => [*width_ft, 0.0],
        FixtureFamily::Elbow90 { diameter_ft } | FixtureFamily::Elbow45 { diameter_ft } => [*diameter_ft, *diameter_ft],
        FixtureFamily::Tee { diameter_ft } => [*diameter_ft, *diameter_ft],
        FixtureFamily::Reducer { large_ft, length_ft, .. } => [*large_ft, *length_ft],
        FixtureFamily::Coupling { diameter_ft, length_ft } => [*diameter_ft, *length_ft],
    }
}

/// Draw a 2D plan symbol for the family (flat at z=centre.z). Returns true.
fn plan_symbol(s: &mut Scene, family: &FixtureFamily, c: V3, dir: Vec2, col: Rgb, owner: Owner) -> bool {
    let tw = local_frame(c, dir);
    let [ew, ed] = plan_extents(family);
    let hw = ew / 2.0;
    let hd = ed / 2.0;

    match family {
        FixtureFamily::Downlight { aperture_ft, .. } => {
            let n = 6;
            let r = aperture_ft / 2.0;
            let pts: Vec<V3> = (0..n).map(|i| {
                let a = 2.0 * PI * (i as f64) / (n as f64);
                tw(a.cos() * r, a.sin() * r, 0.0)
            }).collect();
            disc(s, c, &pts, col, owner);
        }
        FixtureFamily::WaterHeater { diameter_ft, .. } => {
            let n = 6;
            let r = diameter_ft / 2.0;
            let pts: Vec<V3> = (0..n).map(|i| {
                let a = 2.0 * PI * (i as f64) / (n as f64);
                tw(a.cos() * r, a.sin() * r, 0.0)
            }).collect();
            disc(s, c, &pts, col, owner);
        }
        FixtureFamily::FloorBox { width_ft, depth_ft, .. } => {
            let n = 6;
            let r = width_ft.max(*depth_ft) / 2.0;
            let pts: Vec<V3> = (0..n).map(|i| {
                let a = 2.0 * PI * (i as f64) / (n as f64);
                tw(a.cos() * r, a.sin() * r, 0.0)
            }).collect();
            disc(s, c, &pts, col, owner);
        }
        FixtureFamily::WaterCloset { width_ft, depth_ft, .. } => {
            let n = 8; // Keep fewer tris than Simple's boxed()
            let rx = width_ft / 2.0;
            let ry = depth_ft / 2.0;
            let pts: Vec<V3> = (0..n).map(|i| {
                let a = 2.0 * PI * (i as f64) / (n as f64);
                tw(a.cos() * rx, a.sin() * ry, 0.0)
            }).collect();
            disc(s, c, &pts, col, owner);
        }
        FixtureFamily::Lavatory { width_ft, depth_ft, .. } => {
            let n = 8;
            let rx = width_ft / 2.0;
            let ry = depth_ft / 2.0;
            let pts: Vec<V3> = (0..n).map(|i| {
                let a = 2.0 * PI * (i as f64) / (n as f64);
                tw(a.cos() * rx, a.sin() * ry, 0.0)
            }).collect();
            disc(s, c, &pts, col, owner);
        }
        _ => {
            // Default: rectangle in plan.
            quad(s, [tw(-hw, -hd, 0.0), tw(hw, -hd, 0.0), tw(hw, hd, 0.0), tw(-hw, hd, 0.0)], col, owner);
        }
    }
    true
}

/// Is this a troffer-style type tag? Common tags: A, B, C (schedule), 2x2, 2x4, 1x4.
/// The tag is in its ORIGINAL case (not lowercased).
fn is_troffer_tag(tag: &str) -> bool {
    let tag = tag.trim();
    // Single-letter schedule tags A–F
    if tag.len() == 1 && tag.as_bytes().first().is_some_and(|b| b.is_ascii_alphabetic()) {
        return true;
    }
    // NxM patterns like 2x4, 2x2, 1x4 (case-insensitive x)
    let lower = tag.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('x').collect();
    if parts.len() == 2 {
        return parts.iter().all(|p| p.trim().parse::<f64>().is_ok());
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
        Lod::Simple | Lod::Symbol => 12,
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

/// Emit a flat fan (disc) from the ring.
fn disc(s: &mut Scene, _centre: V3, pts: &[V3], col: Rgb, owner: Owner) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    for i in 1..n - 1 {
        tri(s, pts[0], pts[i], pts[i + 1], col, owner);
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
    // Base box for all LODs.
    boxed(s, c, dir, width, depth, height, col, owner);
    if lod == Lod::Simple {
        return;
    }
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hh = height / 2.0;
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
    // Simple: just the plate box.
    boxed(s, c, dir, width, depth, height, col, owner);
    if lod == Lod::Simple {
        return;
    }
    // Detailed: add toggle on top of the box.
    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hh = height / 2.0;
    let toggle_col = Rgb(210, 205, 195);
    let tw2 = hw * 0.25;
    let th = hh * 0.4;
    let ty = depth * 0.18;
    let tb = depth * 0.1;
    quad(s, [tw(-tw2, ty, -th), tw(tw2, ty, -th), tw(tw2, ty, th), tw(-tw2, ty, th)], toggle_col, owner);
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

/// Water closet (toilet): bowl + tank profile. All geometry in local frame so it rotates with `dir`.
#[allow(clippy::too_many_arguments)]
fn water_closet(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, height: f64, _col: Rgb, owner: Owner, lod: Lod) {
    let tw = local_frame(c, dir);
    let porcelain = Rgb(240, 240, 235);
    let hw = width / 2.0;

    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, porcelain, owner);
        return;
    }

    // Bowl: an elongated oval footprint, local coords
    let bowl_depth = depth * 0.65;
    let bowl_h = height * 0.55;
    let n = circle_segs(lod);
    let bowl_rx = hw;
    let bowl_ry = bowl_depth / 2.0;
    // Bowl centre offset in local frame: front half of fixture, lower half
    let bowl_ox = 0.0;
    let bowl_oy = -depth / 2.0 + bowl_depth / 2.0;
    let bowl_oz = -height / 2.0 + bowl_h / 2.0;

    let bot_ring: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            tw(bowl_ox + a.cos() * bowl_rx, bowl_oy + a.sin() * bowl_ry, bowl_oz - bowl_h / 2.0)
        })
        .collect();
    let top_ring: Vec<V3> = (0..n)
        .map(|i| {
            let a = 2.0 * PI * (i as f64) / (n as f64);
            tw(bowl_ox + a.cos() * bowl_rx, bowl_oy + a.sin() * bowl_ry, bowl_oz + bowl_h / 2.0)
        })
        .collect();
    cylinder_side(s, &bot_ring, &top_ring, porcelain, owner);
    disc(s, tw(bowl_ox, bowl_oy, bowl_oz + bowl_h / 2.0), &top_ring, porcelain, owner);

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
            tw(bowl_ox + a.cos() * seat_w, bowl_oy + a.sin() * seat_d, bowl_oz + bowl_h / 2.0 + 0.02)
        })
        .collect();
    for i in 0..n {
        let j = (i + 1) % n;
        quad(s, [top_ring[i], top_ring[j], seat_ring[j], seat_ring[i]], seat_col, owner);
    }
}

/// Lavatory (bathroom sink): a basin shape. All geometry in local frame so it rotates with `dir`.
#[allow(clippy::too_many_arguments)]
fn lavatory(s: &mut Scene, c: V3, dir: Vec2, width: f64, depth: f64, height: f64, _col: Rgb, owner: Owner, lod: Lod) {
    let porcelain = Rgb(240, 240, 235);

    if lod == Lod::Simple {
        boxed(s, c, dir, width, depth, height, porcelain, owner);
        return;
    }

    let tw = local_frame(c, dir);
    let hw = width / 2.0;
    let hd = depth / 2.0;
    let n = circle_segs(lod);
    let rim_z = height / 2.0;
    let bot_z = -height / 2.0;
    let bowl_rx = hw * 0.85;
    let bowl_ry = hd * 0.85;

    let outer: Vec<V3> = (0..n).map(|i| { let a = 2.0 * PI * (i as f64) / (n as f64); tw(a.cos() * hw, a.sin() * hd, rim_z) }).collect();
    let inner: Vec<V3> = (0..n).map(|i| { let a = 2.0 * PI * (i as f64) / (n as f64); tw(a.cos() * bowl_rx, a.sin() * bowl_ry, rim_z) }).collect();
    let drain_r = 1.0 / 12.0;
    let bottom: Vec<V3> = (0..n).map(|i| { let a = 2.0 * PI * (i as f64) / (n as f64); tw(a.cos() * drain_r, a.sin() * drain_r, bot_z) }).collect();

    for i in 0..n {
        let j = (i + 1) % n;
        quad(s, [outer[i], outer[j], inner[j], inner[i]], porcelain, owner);
    }
    let outer_bot: Vec<V3> = (0..n).map(|i| { let a = 2.0 * PI * (i as f64) / (n as f64); tw(a.cos() * hw, a.sin() * hd, bot_z) }).collect();
    cylinder_side(s, &outer_bot, &outer, porcelain, owner);
    cylinder_side(s, &bottom, &inner, Rgb(230, 230, 225), owner);
    disc(s, tw(0.0, 0.0, bot_z), &bottom, Rgb(100, 100, 100), owner);
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

fn scale_color(c: Rgb, factor: f64) -> Rgb {
    Rgb(
        (c.0 as f64 * factor).clamp(0.0, 255.0) as u8,
        (c.1 as f64 * factor).clamp(0.0, 255.0) as u8,
        (c.2 as f64 * factor).clamp(0.0, 255.0) as u8,
    )
}

fn darken(c: Rgb, factor: f64) -> Rgb { scale_color(c, factor) }

fn brighten(c: Rgb, factor: f64) -> Rgb { scale_color(c, factor) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::Part;

    fn empty_scene() -> Scene { Scene::default() }
    const DIR: Vec2 = Vec2 { x: 1.0, y: 0.0 };
    const NRM: Vec2 = Vec2 { x: 0.0, y: 1.0 };
    const COL: Rgb = Rgb(200, 200, 200);

    fn make(family: &FixtureFamily, lod: Lod) -> (bool, usize) {
        let mut s = empty_scene();
        s.part = Part::Device;
        let ok = generate(&mut s, family, v3(5.0, 5.0, 5.0), DIR, NRM, COL, Owner::default(), lod);
        (ok, s.tris.len())
    }

    // ---- MUST 1: Classification table test (>= 20 cases including negatives) ----

    #[test]
    fn classify_table_20_pairs() {
        let sz = [1.0, 1.0, 1.0];
        let cases: Vec<(&str, &str, Option<&str>)> = vec![
            // (kind, tag, expected family name or None)
            ("receptacle", "WP", Some("Receptacle")),           // WP tag on a receptacle kind stays Receptacle
            ("light_fixture", "A", Some("Troffer")),             // schedule tag A → troffer
            ("light_fixture", "DL", Some("Downlight")),          // DL tag → downlight (small)
            ("light_fixture", "2x4", Some("Troffer")),           // NxM → troffer
            ("light_fixture", "pendant", Some("LinearPendant")), // tag pendant
            ("switch", "", Some("Switch")),                      // kind match
            ("light_switch", "", Some("Switch")),                // "light_switch" → Switch, not light
            ("unknown", "SD", None),                             // "SD" alone is NOT a switch
            ("unknown", "SM", None),                             // "SM" alone is NOT a switch
            ("gfci", "", Some("Gfci")),                          // GFCI by kind
            ("unknown", "GFCI", Some("Gfci")),                   // GFCI by tag
            ("usb_outlet", "", Some("UsbOutlet")),
            ("dimmer", "", Some("Dimmer")),
            ("panel", "LP-1", Some("Panel")),
            ("disconnect", "", Some("Disconnect")),
            ("water_closet", "", Some("WaterCloset")),
            ("unknown", "WC", Some("WaterCloset")),              // whole-token WC
            ("lavatory", "LAV-1", Some("Lavatory")),
            ("sink", "", Some("Lavatory")),                      // sink → Lavatory
            ("unknown_thing", "X99", None),                      // no match
            ("floor_box", "", Some("FloorBox")),
            ("tv_outlet", "", Some("TvOutlet")),
            ("data_outlet", "", Some("DataOutlet")),
            ("unknown", "REC", Some("Receptacle")),              // tag REC
            ("base_cabinet", "", Some("BaseCabinet")),
        ];
        for (kind, tag, expected) in &cases {
            let result = classify(kind, tag, sz);
            match expected {
                Some(name) => {
                    let f = result.unwrap_or_else(|| panic!("classify({kind:?}, {tag:?}) should match {name}"));
                    let got = format!("{f:?}");
                    assert!(got.starts_with(name), "classify({kind:?}, {tag:?}): expected {name}, got {got}");
                }
                None => {
                    assert!(result.is_none(), "classify({kind:?}, {tag:?}) should be None, got {result:?}");
                }
            }
        }
    }

    // ---- MUST 2: Schedule sizes override mount sizes ----

    #[test]
    fn schedule_sizes_override_mount() {
        let attrs = vec![
            ("SIZE".to_string(), "24x48".to_string()),
        ];
        let sched = ScheduleSize::from_attrs(&attrs);
        assert!(sched.from_schedule);
        // 24x48 inches → 2x4 feet
        let resolved = sched.resolve_ft([1.0, 1.0, 0.25]);
        assert!((resolved[0] - 2.0).abs() < 0.01, "width should be 2.0 ft, got {}", resolved[0]);
        assert!((resolved[1] - 4.0).abs() < 0.01, "depth should be 4.0 ft, got {}", resolved[1]);
        // Height falls back to mount
        assert!((resolved[2] - 0.25).abs() < 0.01, "height should fall back to 0.25 ft");
    }

    #[test]
    fn scheduled_sizes_produce_correct_bboxes() {
        // 2x4 troffer: schedule says 24x48 inches = 2x4 ft
        let attrs = vec![("SIZE".to_string(), "24x48".to_string())];
        let sched = ScheduleSize::from_attrs(&attrs);
        let resolved = sched.resolve_ft([1.0, 1.0, 0.25]);
        let family = classify_with_schedule("light_fixture", "A", resolved, &sched);
        let f = family.expect("should classify as troffer");
        assert!(matches!(f, FixtureFamily::Troffer { .. }));
        if let FixtureFamily::Troffer { width_ft, length_ft, .. } = f {
            assert!((width_ft - 2.0).abs() < 1.0 / 12.0, "2x4 troffer width ~2 ft, got {width_ft}");
            assert!((length_ft - 4.0).abs() < 1.0 / 12.0, "2x4 troffer length ~4 ft, got {length_ft}");
        }

        // 1x4 troffer
        let attrs = vec![("SIZE".to_string(), "12x48".to_string())];
        let sched = ScheduleSize::from_attrs(&attrs);
        let resolved = sched.resolve_ft([1.0, 1.0, 0.25]);
        let f = classify_with_schedule("light_fixture", "B", resolved, &sched).unwrap();
        if let FixtureFamily::Troffer { width_ft, length_ft, .. } = f {
            assert!((width_ft - 1.0).abs() < 1.0 / 12.0, "1x4 troffer width ~1 ft, got {width_ft}");
            assert!((length_ft - 4.0).abs() < 1.0 / 12.0, "1x4 troffer length ~4 ft, got {length_ft}");
        }

        // 6-inch downlight
        let attrs = vec![("APERTURE".to_string(), "6".to_string())];
        let sched = ScheduleSize::from_attrs(&attrs);
        let resolved = sched.resolve_ft([0.5, 0.5, 0.25]);
        let f = classify_with_schedule("light_fixture", "DL", resolved, &sched).unwrap();
        if let FixtureFamily::Downlight { aperture_ft, .. } = f {
            assert!((aperture_ft - 0.5).abs() < 1.0 / 12.0, "6in downlight aperture ~0.5 ft, got {aperture_ft}");
        }
    }

    // ---- MUST 3: Plan symbol LOD per category ----

    #[test]
    fn plan_symbol_per_category() {
        let categories: Vec<FixtureFamily> = vec![
            FixtureFamily::Troffer { width_ft: 2.0, length_ft: 4.0, depth_ft: 0.33 },
            FixtureFamily::Downlight { aperture_ft: 0.5, depth_ft: 0.33 },
            FixtureFamily::Receptacle { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 },
            FixtureFamily::Panel { width_ft: 1.5, height_ft: 3.0, depth_ft: 0.5 },
            FixtureFamily::WaterCloset { width_ft: 1.5, depth_ft: 2.3, height_ft: 1.25 },
            FixtureFamily::BaseCabinet { width_ft: 2.0, depth_ft: 2.0, height_ft: 2.83 },
            FixtureFamily::Elbow90 { diameter_ft: 0.5 },
        ];
        for family in &categories {
            let (ok, tri_count) = make(family, Lod::Symbol);
            assert!(ok, "{family:?} should produce a plan symbol");
            assert!(tri_count > 0, "{family:?} plan symbol should have triangles");
            // Plan symbol extents should match the type size.
            let [ew, ed] = plan_extents(family);
            let mut s = empty_scene();
            s.part = Part::Device;
            generate(&mut s, family, v3(0.0, 0.0, 0.0), DIR, NRM, COL, Owner::default(), Lod::Symbol);
            let (mut min_x, mut max_x, mut min_y, mut max_y) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for v in &s.tris {
                let (x, y) = (v.p[0] as f64, v.p[1] as f64);
                if x < min_x { min_x = x; }
                if x > max_x { max_x = x; }
                if y < min_y { min_y = y; }
                if y > max_y { max_y = y; }
            }
            let span_x = max_x - min_x;
            let span_y = max_y - min_y;
            // Symbol should be within 20% of the expected extents (tolerance for round shapes).
            if ew > 0.01 {
                assert!(span_x <= ew * 1.2 + 0.01, "{family:?} symbol x span {span_x} > extents {ew}");
            }
            if ed > 0.01 {
                assert!(span_y <= ed * 1.2 + 0.01, "{family:?} symbol y span {span_y} > extents {ed}");
            }
        }
    }

    // ---- MUST 4: IFC class per fixture family ----

    #[test]
    fn ifc_class_per_category() {
        assert_eq!(ifc_class(&FixtureFamily::Troffer { width_ft: 2.0, length_ft: 4.0, depth_ft: 0.33 }), "IfcLightFixture");
        assert_eq!(ifc_class(&FixtureFamily::Downlight { aperture_ft: 0.5, depth_ft: 0.33 }), "IfcLightFixture");
        assert_eq!(ifc_class(&FixtureFamily::Receptacle { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 }), "IfcOutlet");
        assert_eq!(ifc_class(&FixtureFamily::Switch { width_ft: 0.25, height_ft: 0.42, depth_ft: 0.15 }), "IfcSwitchingDevice");
        assert_eq!(ifc_class(&FixtureFamily::Panel { width_ft: 1.5, height_ft: 3.0, depth_ft: 0.5 }), "IfcElectricDistributionBoard");
        assert_eq!(ifc_class(&FixtureFamily::WaterCloset { width_ft: 1.5, depth_ft: 2.3, height_ft: 1.25 }), "IfcSanitaryTerminal");
        assert_eq!(ifc_class(&FixtureFamily::Lavatory { width_ft: 1.67, depth_ft: 1.75, height_ft: 0.67 }), "IfcSanitaryTerminal");
        assert_eq!(ifc_class(&FixtureFamily::BaseCabinet { width_ft: 2.0, depth_ft: 2.0, height_ft: 2.83 }), "IfcFurniture");
        assert_eq!(ifc_class(&FixtureFamily::Elbow90 { diameter_ft: 0.5 }), "IfcPipeFitting");
        assert_eq!(ifc_class(&FixtureFamily::WaterHeater { diameter_ft: 1.67, height_ft: 5.0 }), "IfcBoiler");
    }

    // ---- MUST 5: Geometry frame — bbox == box bbox, ceiling flush ----

    #[test]
    fn fixture_bbox_matches_box_at_angles() {
        let family = FixtureFamily::Troffer { width_ft: 2.0, length_ft: 4.0, depth_ft: 0.33 };
        for angle_deg in [0.0_f64, 90.0, 37.0] {
            let dir = Vec2 { x: angle_deg.to_radians().cos(), y: angle_deg.to_radians().sin() };
            let c = v3(10.0, 10.0, 10.0);
            // Fixture bbox
            let mut sf = empty_scene(); sf.part = Part::Device;
            generate(&mut sf, &family, c, dir, NRM, COL, Owner::default(), Lod::Detailed);
            // Box bbox
            let mut sb = empty_scene(); sb.part = Part::Device;
            boxed(&mut sb, c, dir, 2.0, 4.0, 0.33, COL, Owner::default());
            let bbox = |s: &Scene| -> ([f32; 3], [f32; 3]) {
                let mut lo = [f32::MAX; 3];
                let mut hi = [f32::MIN; 3];
                for v in &s.tris {
                    for i in 0..3 { lo[i] = lo[i].min(v.p[i]); hi[i] = hi[i].max(v.p[i]); }
                }
                (lo, hi)
            };
            let (flo, fhi) = bbox(&sf);
            let (blo, bhi) = bbox(&sb);
            // Fixture may be asymmetric (e.g. troffer top-flush), so tolerance is generous:
            // within 50% of the box span + 2 inches per axis.
            for i in 0..3 {
                let span = (bhi[i] - blo[i]).abs();
                let tol = span * 0.5 + 2.0 / 12.0;
                assert!(flo[i] >= blo[i] - tol, "angle {angle_deg}: fixture min[{i}] {:.3} < box {:.3} - tol {tol:.3}", flo[i], blo[i]);
                assert!(fhi[i] <= bhi[i] + tol, "angle {angle_deg}: fixture max[{i}] {:.3} > box {:.3} + tol {tol:.3}", fhi[i], bhi[i]);
            }
        }
    }

    #[test]
    fn ceiling_troffer_top_at_centre_z() {
        // A troffer centred at z=10 with depth 0.33: its top should be at ~z=10 (not hanging below).
        let family = FixtureFamily::Troffer { width_ft: 2.0, length_ft: 4.0, depth_ft: 0.33 };
        let mut s = empty_scene(); s.part = Part::Device;
        generate(&mut s, &family, v3(0.0, 0.0, 10.0), DIR, NRM, COL, Owner::default(), Lod::Detailed);
        let max_z = s.tris.iter().map(|v| v.p[2]).fold(f32::MIN, f32::max);
        // Top should be at z=10 (the centre), because derive.rs places ceiling fixtures so centre = ceiling - h/2.
        // The troffer draws from z=0 down to z=-depth, around the centre, so top is at centre.z.
        assert!((max_z - 10.0).abs() < 0.1, "troffer top z {max_z} should be near 10.0");
    }

    // ---- MUST 6: LOD chosen by caller; triangle counts strictly decreasing ----

    #[test]
    fn lod_triangle_counts_decrease() {
        let families = all_families();
        for family in &families {
            let (ok_d, n_d) = make(family, Lod::Detailed);
            let (ok_s, n_s) = make(family, Lod::Simple);
            let (ok_sym, n_sym) = make(family, Lod::Symbol);
            let (ok_b, _) = make(family, Lod::Box);
            assert!(ok_d, "{family:?} Detailed should generate");
            assert!(ok_s, "{family:?} Simple should generate");
            assert!(ok_sym, "{family:?} Symbol should generate");
            assert!(!ok_b, "{family:?} Box should return false");
            assert!(n_d >= n_s, "{family:?}: Detailed {n_d} < Simple {n_s}");
            assert!(n_s >= n_sym, "{family:?}: Simple {n_s} < Symbol {n_sym}");
        }
    }

    #[test]
    fn lod_box_returns_false() {
        let family = FixtureFamily::Troffer { width_ft: 2.0, length_ft: 4.0, depth_ft: 0.33 };
        let (ok, _) = make(&family, Lod::Box);
        assert!(!ok, "Lod::Box should return false for caller fallback");
    }

    // ---- MUST 7: New families exist ----

    #[test]
    fn new_families_generate() {
        let new = vec![
            FixtureFamily::Gfci { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 },
            FixtureFamily::UsbOutlet { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 },
            FixtureFamily::Dimmer { width_ft: 0.25, height_ft: 0.42, depth_ft: 0.15 },
            FixtureFamily::TvOutlet { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 },
            FixtureFamily::ClockHanger { width_ft: 0.5, height_ft: 0.3, depth_ft: 0.15 },
            FixtureFamily::Drawers { width_ft: 2.0, depth_ft: 2.0, height_ft: 2.83 },
            FixtureFamily::ApplianceOpening { width_ft: 2.5, depth_ft: 2.0, height_ft: 2.83 },
        ];
        for family in &new {
            for lod in [Lod::Detailed, Lod::Simple, Lod::Symbol] {
                let (ok, n) = make(family, lod);
                assert!(ok, "{family:?} at {lod:?} should generate");
                assert!(n > 0, "{family:?} at {lod:?} should have triangles");
            }
        }
    }

    // ---- KEPT tests (updated signatures) ----

    #[test]
    fn hostile_numbers_do_not_panic() {
        // Clamped to [0.01, 20.0] by the caller, but just in case.
        let family = FixtureFamily::Receptacle { width_ft: 0.01, height_ft: 20.0, depth_ft: 0.01 };
        let (ok, _) = make(&family, Lod::Detailed);
        assert!(ok);
    }

    #[test]
    fn classify_unknown_returns_none() {
        assert!(classify("unknown_thing", "X99", [1.0, 1.0, 1.0]).is_none());
    }

    #[test]
    fn sizes_from_type_params() {
        let family = FixtureFamily::Troffer { width_ft: 2.0, length_ft: 4.0, depth_ft: 0.33 };
        let mut s = empty_scene(); s.part = Part::Device;
        generate(&mut s, &family, v3(0.0, 0.0, 0.0), DIR, NRM, COL, Owner::default(), Lod::Detailed);
        for v in &s.tris {
            assert!(v.p[0].abs() <= 1.1, "troffer vertex x {:.3} out of 2ft-wide range", v.p[0]);
            assert!(v.p[1].abs() <= 2.1, "troffer vertex y {:.3} out of 4ft-long range", v.p[1]);
        }
    }

    // ---- Helper: all families list ----

    fn all_families() -> Vec<FixtureFamily> {
        vec![
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
            FixtureFamily::Gfci { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 },
            FixtureFamily::UsbOutlet { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 },
            FixtureFamily::Switch { width_ft: 0.25, height_ft: 0.42, depth_ft: 0.15 },
            FixtureFamily::Dimmer { width_ft: 0.25, height_ft: 0.42, depth_ft: 0.15 },
            FixtureFamily::FloorBox { width_ft: 0.5, depth_ft: 0.5, height_ft: 0.33 },
            FixtureFamily::DataOutlet { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 },
            FixtureFamily::TvOutlet { width_ft: 0.3, height_ft: 0.45, depth_ft: 0.15 },
            FixtureFamily::ClockHanger { width_ft: 0.5, height_ft: 0.3, depth_ft: 0.15 },
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
            FixtureFamily::Drawers { width_ft: 2.0, depth_ft: 2.0, height_ft: 2.83 },
            FixtureFamily::ApplianceOpening { width_ft: 2.5, depth_ft: 2.0, height_ft: 2.83 },
            FixtureFamily::Elbow90 { diameter_ft: 0.5 },
            FixtureFamily::Elbow45 { diameter_ft: 0.5 },
            FixtureFamily::Tee { diameter_ft: 0.5 },
            FixtureFamily::Reducer { large_ft: 0.5, small_ft: 0.33, length_ft: 0.5 },
            FixtureFamily::Coupling { diameter_ft: 0.5, length_ft: 0.33 },
        ]
    }
}
