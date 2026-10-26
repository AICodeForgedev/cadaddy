//! IFC4 exchange, written from the public IFC schema and ISO 10303-21 (the STEP physical file);
//! no IFC library is used.
//!
//! Out: the building WALLGEN made — walls (IfcWall), floor slabs (IfcSlab), ceilings
//! (IfcCovering), doors (IfcDoor), windows (IfcWindow), room volumes (IfcSpace) and other meshes
//! (IfcBuildingElementProxy) — in a project, site, building and one storey per level, each body a
//! tessellated IfcPolygonalFaceSet in metres. GlobalIds come from the objects' handles, so the
//! same drawing gives the same ids. A wall whose building spec type has an assembly (from a wall
//! type legend) carries it: an IfcMaterialLayerSetUsage with one IfcMaterialLayer per board,
//! stud, insulation… layer, and Pset_WallCommon (Reference, FireRating, AcousticRating,
//! IsExternal), and its head of wall detail (CADaddy_HeadOfWall: track, what it meets, deflection,
//! bottom track). A ceiling or soffit whose spec type has layers carries an IfcMaterialLayerSet
//! from the room's face up (IfcMaterialLayerSetUsage on the third axis) and Pset_CoveringCommon.
//!
//! In: every product with a body (walls, slabs, spaces, doors… whatever the file has) as a MESH on
//! a layer per IFC class (`IFC-Wall`, `IFC-Space`…), tagged with its class, GlobalId and name. The
//! common body kinds are read: polygonal and triangulated face sets, faceted BReps and shell
//! models, extruded rectangle, circle and polyline profiles, mapped items and the first operand of
//! booleans (openings aren't cut), placed by their local placement chains, in the file's length unit.
//! Each object's tag also carries what the file says about it: the storey it is in (`level:`, and
//! `elevation:` in metres), the space, building and site above it (`space:`, `building:`, `site:`),
//! its type (`type:`), the systems it is grouped into (`system:`) and its property and quantity sets (`prop:Pset_WallCommon.FireRating=1HR`; quantity lengths in metres).

use std::collections::HashMap;
use std::fmt::Write as _;

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Layer, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_geom::Vec3;

use crate::{IoError, Result};

/// The extended data application tagging objects read from IFC (class, GlobalId, name).
pub const APP: &str = "CADADDY_IFC";
/// Most instances one file holds.
const MAX_INSTANCES: usize = 5_000_000;
/// Deepest placement chain, mapped item or boolean followed.
const MAX_DEPTH: usize = 64;
/// Most points one imported object has.
const MAX_POINTS: usize = 2_000_000;
/// Most faces all imported objects have together.
const MAX_FACES: usize = 10_000_000;

// ---------------- what goes out ----------------

/// What an object is in IFC, from what WALLGEN, model.build or ROOMS tagged it as (or the class
/// it was read as from an IFC file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Wall,
    Slab,
    Ceiling,
    Door,
    Window,
    Stair,
    Space,
    Proxy,
    Column,
    CurtainWall,
    Railing,
    Ramp,
    Roof,
    Beam,
    Terrain,
    Duct,
    Pipe,
    Conduit,
    /// Cable tray: IfcCableCarrierSegment, CABLETRAYSEGMENT.
    CableTray,
    /// Cable outside conduit (a homerun): IfcCableSegment, CABLESEGMENT.
    Cable,
    CableFitting,
    LightFixture,
    Outlet,
    SwitchingDevice,
    JunctionBox,
    ElectricDistributionBoard,
    DuctFitting,
    PipeFitting,
    ConduitFitting,
    AirTerminal,
    /// Sprinkler heads.
    FireSuppressionTerminal,
    // Scheduled equipment.
    UnitaryEquipment,
    AirTerminalBox,
    Fan,
    Pump,
    Boiler,
    Condenser,
    FlowMeter,
    Transformer,
    // Concrete.
    Footing,
    Pile,
    ReinforcingBar,
    MechanicalFastener,
}

impl Class {
    /// The IFC4 entity and the predefined type this class is written with.
    fn ifc(self) -> (&'static str, &'static str) {
        match self {
            Class::Wall => ("IfcWall", "STANDARD"),
            Class::Slab => ("IfcSlab", "FLOOR"),
            Class::Ceiling => ("IfcCovering", "CEILING"),
            Class::Door => ("IfcDoor", "DOOR"),
            Class::Window => ("IfcWindow", "WINDOW"),
            Class::Stair => ("IfcStair", "NOTDEFINED"),
            Class::Space => ("IfcSpace", "INTERNAL"),
            Class::Proxy => ("IfcBuildingElementProxy", "NOTDEFINED"),
            Class::Column => ("IfcColumn", "COLUMN"),
            Class::CurtainWall => ("IfcCurtainWall", "NOTDEFINED"),
            Class::Railing => ("IfcRailing", "HANDRAIL"),
            Class::Ramp => ("IfcRamp", "NOTDEFINED"),
            Class::Roof => ("IfcRoof", "NOTDEFINED"),
            Class::Beam => ("IfcBeam", "BEAM"),
            Class::Terrain => ("IfcGeographicElement", "TERRAIN"),
            Class::Duct => ("IfcDuctSegment", "RIGIDSEGMENT"),
            Class::Pipe => ("IfcPipeSegment", "RIGIDSEGMENT"),
            Class::Conduit => ("IfcCableCarrierSegment", "CONDUITSEGMENT"),
            Class::CableTray => ("IfcCableCarrierSegment", "CABLETRAYSEGMENT"),
            Class::Cable => ("IfcCableSegment", "CABLESEGMENT"),
            Class::CableFitting => ("IfcCableFitting", "NOTDEFINED"),
            Class::LightFixture => ("IfcLightFixture", "NOTDEFINED"),
            Class::Outlet => ("IfcOutlet", "POWEROUTLET"),
            Class::SwitchingDevice => ("IfcSwitchingDevice", "TOGGLESWITCH"),
            Class::JunctionBox => ("IfcJunctionBox", "NOTDEFINED"),
            Class::ElectricDistributionBoard => ("IfcElectricDistributionBoard", "DISTRIBUTIONBOARD"),
            Class::DuctFitting => ("IfcDuctFitting", "NOTDEFINED"),
            Class::PipeFitting => ("IfcPipeFitting", "NOTDEFINED"),
            Class::ConduitFitting => ("IfcCableCarrierFitting", "NOTDEFINED"),
            Class::AirTerminal => ("IfcAirTerminal", "NOTDEFINED"),
            Class::FireSuppressionTerminal => ("IfcFireSuppressionTerminal", "SPRINKLER"),
            Class::UnitaryEquipment => ("IfcUnitaryEquipment", "NOTDEFINED"),
            Class::AirTerminalBox => ("IfcAirTerminalBox", "NOTDEFINED"),
            Class::Fan => ("IfcFan", "NOTDEFINED"),
            Class::Pump => ("IfcPump", "NOTDEFINED"),
            Class::Boiler => ("IfcBoiler", "NOTDEFINED"),
            Class::Condenser => ("IfcCondenser", "NOTDEFINED"),
            Class::FlowMeter => ("IfcFlowMeter", "NOTDEFINED"),
            Class::Transformer => ("IfcTransformer", "VOLTAGE"),
            Class::Footing => ("IfcFooting", "NOTDEFINED"),
            Class::Pile => ("IfcPile", "BORED"),
            Class::ReinforcingBar => ("IfcReinforcingBar", "MAIN"),
            Class::MechanicalFastener => ("IfcMechanicalFastener", "ANCHORBOLT"),
        }
    }

    /// The class an object read from IFC as `name` (the reader's class name) is written back as.
    fn of_read(name: &str) -> Option<Class> {
        Some(match name {
            "Wall" | "WallStandardCase" => Class::Wall,
            "Slab" => Class::Slab,
            "Covering" => Class::Ceiling,
            "Door" => Class::Door,
            "Window" => Class::Window,
            "Stair" => Class::Stair,
            "Space" => Class::Space,
            "BuildingElementProxy" => Class::Proxy,
            "Column" => Class::Column,
            "CurtainWall" => Class::CurtainWall,
            "Railing" => Class::Railing,
            "Ramp" => Class::Ramp,
            "Roof" => Class::Roof,
            "Beam" => Class::Beam,
            "GeographicElement" => Class::Terrain,
            "DuctSegment" => Class::Duct,
            "PipeSegment" => Class::Pipe,
            "CableCarrierSegment" => Class::Conduit,
            "CableSegment" => Class::Cable,
            "CableFitting" => Class::CableFitting,
            "LightFixture" => Class::LightFixture,
            "Outlet" => Class::Outlet,
            "SwitchingDevice" => Class::SwitchingDevice,
            "JunctionBox" => Class::JunctionBox,
            "ElectricDistributionBoard" => Class::ElectricDistributionBoard,
            "DuctFitting" => Class::DuctFitting,
            "PipeFitting" => Class::PipeFitting,
            "CableCarrierFitting" => Class::ConduitFitting,
            "AirTerminal" => Class::AirTerminal,
            "FireSuppressionTerminal" => Class::FireSuppressionTerminal,
            "UnitaryEquipment" => Class::UnitaryEquipment,
            "AirTerminalBox" => Class::AirTerminalBox,
            "Fan" => Class::Fan,
            "Pump" => Class::Pump,
            "Boiler" => Class::Boiler,
            "Condenser" => Class::Condenser,
            "FlowMeter" => Class::FlowMeter,
            "Transformer" => Class::Transformer,
            "Footing" => Class::Footing,
            "Pile" => Class::Pile,
            "ReinforcingBar" => Class::ReinforcingBar,
            "MechanicalFastener" => Class::MechanicalFastener,
            _ => return None,
        })
    }
}

/// What an object is written as: its class, predefined type, storey (WALLGEN's level), name,
/// long name, and the GlobalId it was read with from an IFC file.
struct Kind {
    class: Class,
    predefined: &'static str,
    level: Option<String>,
    name: String,
    long: String,
    read_id: Option<String>,
    /// For MEP objects: the system name for IfcDistributionSystem grouping.
    system_name: Option<String>,
    /// Scheduled equipment: every system it is joined to, its schedule's title and row.
    systems: Vec<String>,
    description: String,
    properties: Vec<(String, String)>,
    /// MEP segments, fittings and terminals: their size tags (`size:24x12`, `size:10` round;
    /// `size2:` a tee's branch or a transition's other end), inches.
    sizes: Vec<[f64; 2]>,
    /// For a `USERDEFINED` predefined type: what it is (IFC's ObjectType).
    object_type: &'static str,
}

/// The IFC4 class and predefined type a scheduled equipment kind is written as.
fn equipment_class(kind: &str) -> (Class, &'static str) {
    match kind {
        "ahu" => (Class::UnitaryEquipment, "AIRHANDLER"),
        "rtu" => (Class::UnitaryEquipment, "ROOFTOPUNIT"),
        "vav" => (Class::AirTerminalBox, "NOTDEFINED"),
        "exhaust_fan" | "fan" => (Class::Fan, "NOTDEFINED"),
        "pump" => (Class::Pump, "NOTDEFINED"),
        "water_heater" => (Class::Boiler, "WATER"),
        "condensing_unit" => (Class::Condenser, "AIRCOOLED"),
        "gas_meter" => (Class::FlowMeter, "GASMETER"),
        // Electrical gear.
        "panel" | "panelboard" => (Class::ElectricDistributionBoard, "DISTRIBUTIONBOARD"),
        "switchboard" | "switchgear" => (Class::ElectricDistributionBoard, "SWITCHBOARD"),
        "transformer" => (Class::Transformer, "VOLTAGE"),
        "disconnect" => (Class::SwitchingDevice, "SWITCHDISCONNECTOR"),
        _ => (Class::Proxy, "NOTDEFINED"),
    }
}

/// The tag `app`'s first strings on `e` (WALLGEN: part, level; ROOMS: name, number).
fn tag_strings(e: &Entity, app: &str) -> Option<Vec<String>> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(app))?;
    Some(x.iter().skip(at + 1).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.clone()).collect())
}

/// How far model.build moved a WALLGEN object to stack its level (its `stacked:<dx>,<dy>` tag),
/// drawing units; zero when it didn't.
fn stacked_by(e: &Entity) -> cadaddy_geom::Vec2 {
    let moved = tag_strings(e, "CADADDY_WALLGEN").and_then(|t| {
        let (x, y) = t.iter().find_map(|v| v.strip_prefix("stacked:"))?.split_once(',')?;
        Some(cadaddy_geom::Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
    });
    moved.filter(|v| v.is_finite()).unwrap_or(cadaddy_geom::Vec2::ZERO)
}

/// An MEP size tag, inches: `24x12` → [24, 12]; `10` (round) → [10, 0].
fn parse_size_tag(v: &str) -> Option<[f64; 2]> {
    let ok = |x: f64| x.is_finite() && x > 0.0 && x < 10_000.0;
    match v.split_once('x') {
        Some((w, h)) => Some([w.trim().parse().ok().filter(|x| ok(*x))?, h.trim().parse().ok().filter(|x| ok(*x))?]),
        None => Some([v.trim().parse().ok().filter(|x| ok(*x))?, 0.0]),
    }
}

/// A size as printed: `24"x12"`, `10"Ø`.
fn size_label_in(s: [f64; 2]) -> String {
    let n = |v: f64| {
        let t = format!("{v:.3}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    if s[1] > 0.0 { format!("{}\"x{}\"", n(s[0]), n(s[1])) } else { format!("{}\"Ø", n(s[0])) }
}

fn class_of(e: &Entity) -> Kind {
    let kind = |class: Class, level: Option<String>, name: String, long: String| Kind {
        class,
        predefined: class.ifc().1,
        level,
        name,
        long,
        read_id: None,
        system_name: None,
        systems: Vec::new(),
        description: String::new(),
        properties: Vec::new(),
        sizes: Vec::new(),
        object_type: "",
    };
    if let Some(t) = tag_strings(e, "CADADDY_WALLGEN") {
        let part = t.first().map(String::as_str).unwrap_or_default();
        // model.build's MEP runs: `system:<name>` and `kind:duct|pipe|conduit` after the source.
        // Part tags: `part:segment`, `part:fitting:elbow`, `part:terminal:diffuser`.
        let extra = |key: &str| t.iter().skip(3).find_map(|v| v.strip_prefix(key)).map(str::to_string);
        let mep_part = extra("part:");
        let class = match part {
            "wall" | "parapet" => Class::Wall,
            "floor" => Class::Slab,
            "ceiling" | "soffit" => Class::Ceiling,
            "door" => Class::Door,
            "window" => Class::Window,
            "stair" => Class::Stair,
            "column" => Class::Column,
            "curtainwall" => Class::CurtainWall,
            "railing" => Class::Railing,
            "ramp" => Class::Ramp,
            "roof" => Class::Roof,
            "ground" => Class::Terrain,
            "concrete" => {
                // An element by its kind; its rebar and what is cast into it by their part.
                let ek = extra("kind:").unwrap_or_default();
                let (class, pre) = match mep_part.as_deref().unwrap_or("element") {
                    "rebar" => (Class::ReinforcingBar, "MAIN"),
                    "embed:anchor_bolt" => (Class::MechanicalFastener, "ANCHORBOLT"),
                    "embed:stub_up" | "embed:conduit" | "embed:sleeve" => (Class::Conduit, "CONDUITSEGMENT"),
                    "embed:ufer" | "embed:bond" => (Class::ReinforcingBar, "USERDEFINED"),
                    p if p.starts_with("embed:") => (Class::Proxy, "NOTDEFINED"),
                    _ => match ek.as_str() {
                        "slab" => (Class::Slab, "BASESLAB"),
                        "footing" => (Class::Footing, if t.iter().any(|v| v == "prop:shape=strip") { "STRIP_FOOTING" } else { "PAD_FOOTING" }),
                        "grade_beam" => (Class::Footing, "FOOTING_BEAM"),
                        "pier" | "pole_base" => (Class::Pile, "BORED"),
                        "pad" | "handhole_base" => (Class::Slab, "USERDEFINED"),
                        _ => (Class::Proxy, "NOTDEFINED"),
                    },
                };
                let mut k = kind(class, t.get(1).cloned(), extra("tag:").unwrap_or_else(|| ek.clone()), String::new());
                k.predefined = pre;
                k.description = ek.replace('_', " ");
                k.properties = t
                    .iter()
                    .skip(3)
                    .filter_map(|v| v.strip_prefix("prop:"))
                    .filter_map(|v| v.split_once('='))
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect();
                return k;
            }
            "mep" if extra("kind:").as_deref() == Some("equipment") => {
                let ek = mep_part.as_deref().and_then(|p| p.strip_prefix("equipment:")).unwrap_or_default();
                let (class, pre) = equipment_class(ek);
                let mut k = kind(class, t.get(1).cloned(), extra("tag:").unwrap_or_else(|| ek.to_string()), String::new());
                k.predefined = pre;
                k.systems = t.iter().skip(3).filter_map(|v| v.strip_prefix("system:")).map(str::to_string).collect();
                k.description = extra("title:").unwrap_or_default();
                k.properties = t
                    .iter()
                    .skip(3)
                    .filter_map(|v| v.strip_prefix("prop:"))
                    .filter_map(|v| v.split_once('='))
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect();
                return k;
            }
            "mep" => {
                let is_fitting = mep_part.as_deref().is_some_and(|p| p.starts_with("fitting:"));
                let is_terminal = mep_part.as_deref().is_some_and(|p| p.starts_with("terminal:"));
                if is_terminal && mep_part.as_deref().is_some_and(|p| p.starts_with("terminal:sprinkler")) {
                    Class::FireSuppressionTerminal
                } else if is_terminal {
                    Class::AirTerminal
                } else if is_fitting {
                    match extra("kind:").as_deref() {
                        Some("duct") => Class::DuctFitting,
                        Some("pipe") => Class::PipeFitting,
                        Some("conduit") | Some("tray") => Class::ConduitFitting,
                        Some("cable") => Class::CableFitting,
                        _ => Class::DuctFitting,
                    }
                } else {
                    match extra("kind:").as_deref() {
                        Some("duct") => Class::Duct,
                        Some("pipe") => Class::Pipe,
                        Some("conduit") => Class::Conduit,
                        Some("tray") => Class::CableTray,
                        Some("cable") => Class::Cable,
                        Some("light_fixture") => Class::LightFixture,
                        Some("outlet") | Some("receptacle") | Some("data_outlet") | Some("floor_box") => Class::Outlet,
                        Some("switch") | Some("disconnect") => Class::SwitchingDevice,
                        Some("junction_box") => Class::JunctionBox,
                        Some("panel") | Some("transformer") => Class::ElectricDistributionBoard,
                        _ => Class::Proxy,
                    }
                }
            }
            _ => Class::Proxy,
        };
        let sys_name = extra("system:");
        let mut k = kind(class, t.get(1).cloned(), sys_name.clone().unwrap_or_else(|| part.to_string()), String::new());
        k.system_name = sys_name;
        match part {
            "parapet" => k.predefined = "PARAPET",
            "soffit" => k.predefined = "CLADDING",
            _ => {}
        }
        // Fitting predefined types from the part tag.
        if let Some(p) = mep_part.as_deref()
            && let Some(sub) = p.strip_prefix("fitting:")
        {
            k.predefined = match sub {
                "elbow" => "BEND",
                "tee" | "cross" => "JUNCTION",
                "transition" | "reducer" => "TRANSITION",
                // A riser between two heights: none of IFC's own kinds.
                "drop" => "USERDEFINED",
                _ => "NOTDEFINED",
            };
            if sub == "drop" {
                k.object_type = "DROP";
            }
        }
        if part == "mep" {
            k.sizes = ["size:", "size2:"].iter().filter_map(|key| extra(key).and_then(|v| parse_size_tag(&v))).collect();
        }
        return k;
    }
    if let Some(t) = tag_strings(e, "CADADDY_ROOM") {
        return kind(Class::Space, None, t.first().cloned().unwrap_or_default(), t.get(1).cloned().unwrap_or_default());
    }
    // Read from an IFC file: written back as its class, with its GlobalId and name.
    if let Some(t) = tag_strings(e, APP)
        && let Some(class) = t.first().and_then(|c| Class::of_read(c))
    {
        let name = t.get(2).cloned().filter(|n| !n.is_empty()).unwrap_or_else(|| e.common.layer.to_string());
        let mut k = kind(class, None, name, String::new());
        // Its predefined type wasn't read: not defined, rather than a guess.
        k.predefined = if class == Class::Space { "SPACE" } else { "NOTDEFINED" };
        k.read_id = t.get(1).filter(|g| is_guid(g)).cloned();
        return k;
    }
    kind(Class::Proxy, None, e.common.layer.to_string(), String::new())
}

/// Whether `g` is an IFC GlobalId: 22 characters of IFC's base-64 alphabet, the first 0–3.
fn is_guid(g: &str) -> bool {
    g.len() == 22
        && g.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
        && g.as_bytes().first().is_some_and(|b| (b'0'..=b'3').contains(b))
}

/// A number standing for `text` (FNV-1a), for GlobalIds made from names.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3))
}

/// What makes this drawing's GlobalIds its own: its FINGERPRINTGUID (every AutoCAD drawing has one,
/// kept when it is saved), so two drawings with the same handles don't give the same ids; 0 for a
/// drawing without one (its ids are then the handles' alone, as before).
fn drawing_seed(d: &Drawing) -> u64 {
    match d.header.get("FINGERPRINTGUID").and_then(cadaddy_doc::HVal::as_str).map(str::trim) {
        Some(g) if !g.is_empty() => hash(&g.to_ascii_uppercase()),
        _ => 0,
    }
}

/// An IFC GlobalId: 128 bits from `seed`, in IFC's 22-character base-64 form.
fn guid(seed: u64, salt: u64) -> String {
    const CHARS: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";
    let mix = |mut z: u64| {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let hi = mix(seed ^ salt.rotate_left(17));
    let lo = mix(hi ^ seed.rotate_left(29) ^ salt);
    let v = (u128::from(hi) << 64) | u128::from(lo);
    let mut s = String::with_capacity(22);
    s.push(CHARS[(v >> 126) as usize & 3] as char);
    for i in (0..21).rev() {
        s.push(CHARS[(v >> (i * 6)) as usize & 63] as char);
    }
    s
}

/// A STEP string: `'` doubled, `\` doubled, anything outside printable ASCII as `\X2\…\X0\`.
fn step_str(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            // ISO 10303-21: \X2\ holds 16-bit characters, \X4\ the ones above U+FFFF.
            c if u32::from(c) > 0xFFFF => {
                let _ = write!(out, "\\X4\\{:08X}\\X0\\", u32::from(c));
            }
            c => {
                let _ = write!(out, "\\X2\\{:04X}\\X0\\", u32::from(c));
            }
        }
    }
    out.push('\'');
    out
}

/// A STEP real: always with a decimal point, no exponent for ordinary values.
fn real(v: f64) -> String {
    if !v.is_finite() {
        return "0.".into();
    }
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0');
    if s == "-0." { "0.".into() } else { s.to_string() }
}

/// Instances written one per line, numbered from 1.
struct Out {
    lines: Vec<String>,
}

impl Out {
    fn add(&mut self, body: String) -> usize {
        self.lines.push(body);
        self.lines.len()
    }
}

fn refs(ids: &[usize]) -> String {
    let v: Vec<String> = ids.iter().map(|i| format!("#{i}")).collect();
    format!("({})", v.join(","))
}

/// A wall, door or window as written: its class, product and placement instances, storey, and
/// its extents in the storey's coordinates.
struct Placed {
    class: Class,
    product: usize,
    place: usize,
    storey: usize,
    lo: [f64; 3],
    hi: [f64; 3],
    handle: u64,
}

/// A mesh's measures for the base quantities, in drawing units: its volume (when it is closed),
/// its footprint (the area of its downward faces seen from above) and its height.
struct Measures {
    volume: Option<f64>,
    footprint: f64,
    height: f64,
}

fn measures(m: &Mesh) -> Measures {
    let p = |i: &u32| m.vertices.get(*i as usize).copied();
    let mut volume = 0.0;
    let mut footprint = 0.0;
    let mut edges: HashMap<(u32, u32), i64> = HashMap::new();
    let mut closed = true;
    for f in &m.faces {
        let pts: Vec<Vec3> = f.v.iter().filter_map(p).collect();
        if pts.len() < 3 || pts.len() != f.v.len() {
            closed = false;
            continue;
        }
        // Fan triangles: signed volume against the origin, and the area seen from above.
        let mut normal_z = 0.0;
        let mut area_xy = 0.0;
        if let Some(&a) = pts.first() {
            for w in pts.get(1..).unwrap_or_default().windows(2) {
                if let [b, c] = w {
                    volume += a.dot(b.cross(*c)) / 6.0;
                    normal_z += (*b - a).cross(*c - a).z;
                    area_xy += ((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)) / 2.0;
                }
            }
        }
        if normal_z < 0.0 {
            footprint += area_xy.abs();
        }
        // Each edge one way in one face and the other way in another: a closed, oriented surface.
        for (i, &a) in f.v.iter().enumerate() {
            let b = f.v.get((i + 1) % f.v.len()).copied().unwrap_or(a);
            *edges.entry((a.min(b), a.max(b))).or_default() += if a < b { 1 } else { -1 };
        }
    }
    closed &= !edges.is_empty() && edges.values().all(|n| *n == 0);
    let (lo, hi) = m.vertices.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)));
    Measures { volume: closed.then_some(volume.abs()), footprint, height: (hi - lo).max(0.0) }
}

/// The drawing's model-space meshes as an IFC4 file (`name` goes into the header).
pub fn write_ifc(d: &Drawing, name: &str) -> Result<Vec<u8>> {
    let insunits = d.header.i64("INSUNITS", 0);
    // Metres per drawing unit: a drawing without units is in millimetres if metric, else inches.
    let unit_scale = match insunits {
        0 if d.header.i64("MEASUREMENT", 1) == 1 => 0.001,
        0 => 0.0254,
        u => cadaddy_doc::geo::metres_per_unit(u).max(1e-9),
    };
    let mm = (unit_scale - 0.001).abs() < 1e-12;
    struct Item<'a> {
        e: &'a Entity,
        mesh: &'a Mesh,
        kind: Kind,
        zmin: f64,
    }
    let mut items: Vec<Item> = Vec::new();
    for e in d.model.iter() {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        if m.faces.is_empty() || m.vertices.is_empty() {
            continue;
        }
        let zmin = m.vertices.iter().map(|v| v.z).fold(f64::MAX, f64::min);
        items.push(Item { e, mesh: m, kind: class_of(e), zmin });
    }
    if items.is_empty() {
        return Err(IoError::Format("nothing to write to IFC: no meshes in model space (make the building with WALLGEN or ROOMS first)".into()));
    }
    // Storeys: WALLGEN's levels, lowest first, at their lowest wall or slab top; one when none.
    let mut levels: Vec<(String, f64)> = Vec::new();
    for it in &items {
        if let Some(l) = &it.kind.level {
            let z = if it.kind.class == Class::Slab { it.mesh.vertices.iter().map(|v| v.z).fold(f64::MIN, f64::max) } else { it.zmin };
            match levels.iter_mut().find(|(n, _)| n == l) {
                Some(x) => x.1 = x.1.min(z),
                None => levels.push((l.clone(), z)),
            }
        }
    }
    // The spec's levels and storey datums (the roof, a penthouse drawn elsewhere) are storeys
    // too, at their elevations, even with nothing built on them.
    let spec = d.building_spec.as_deref().and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok());
    if let Some(sp) = &spec {
        let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        for (name, e) in sp.storeys() {
            if !levels.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                levels.push((name, e * upf));
            }
        }
    }
    if levels.is_empty() {
        levels.push(("Level 1".into(), items.iter().map(|i| i.zmin).fold(f64::MAX, f64::min).min(0.0)));
    }
    levels.sort_by(|a, b| a.1.total_cmp(&b.1));
    // Where each storey's objects are on the sheet: plans drawn side by side (L2's beside L1's)
    // are built over their own plans, and their building spec's base points stack them.
    let bases: Vec<(String, cadaddy_geom::Vec2)> = spec.as_ref().map(|sp| cadaddy_walk3d::level_bases(d, &sp.to_rules())).unwrap_or_default();
    let base_of = |level: &str| bases.iter().find(|(n, _)| n == level).map_or(cadaddy_geom::Vec2::ZERO, |(_, b)| *b);
    let storey_of = |it: &Item| -> usize {
        if let Some(l) = &it.kind.level
            && let Some(i) = levels.iter().position(|(n, _)| n == l)
        {
            return i;
        }
        // The highest storey at or below the object's foot.
        levels.iter().rposition(|(_, z)| *z <= it.zmin + 1e-6).unwrap_or(0)
    };
    // GlobalIds: from the drawing's own seed, so they stay the same each time it is exported (and
    // differ from another drawing's); an object read from IFC keeps the id it was read with
    // (the first one, when copies of it carry the same id).
    let seed = drawing_seed(d);
    let id = |n: u64, salt: u64| guid(n, salt ^ seed);
    let mut read_ids: HashMap<&str, u64> = HashMap::new();
    for it in &items {
        if let Some(g) = &it.kind.read_id {
            let h = read_ids.entry(g.as_str()).or_insert(it.e.handle.0);
            *h = (*h).min(it.e.handle.0);
        }
    }
    let element_id = |it: &Item| -> String {
        match &it.kind.read_id {
            Some(g) if read_ids.get(g.as_str()) == Some(&it.e.handle.0) => g.clone(),
            _ => id(it.e.handle.0, 7),
        }
    };

    let mut o = Out { lines: Vec::new() };
    let unit = if mm { o.add("IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.)".into()) } else { o.add("IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)".into()) };
    // Coordinates go out in the unit declared: millimetres for metric drawings, else metres.
    let k = if mm { 1.0 } else { unit_scale };
    let area = o.add("IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.)".into());
    let volume = o.add("IFCSIUNIT(*,.VOLUMEUNIT.,$,.CUBIC_METRE.)".into());
    let units = o.add(format!("IFCUNITASSIGNMENT({})", refs(&[unit, area, volume])));
    let origin = o.add("IFCCARTESIANPOINT((0.,0.,0.))".into());
    let zdir = o.add("IFCDIRECTION((0.,0.,1.))".into());
    let xdir = o.add("IFCDIRECTION((1.,0.,0.))".into());
    let world = o.add(format!("IFCAXIS2PLACEMENT3D(#{origin},#{zdir},#{xdir})"));
    let context = o.add(format!("IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#{world},$)"));
    let body_ctx = o.add(format!("IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Body','Model',*,*,*,*,#{context},$,.MODEL_VIEW.,$)"));
    let title = std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "CADaddy".into());
    let project = o.add(format!("IFCPROJECT('{}',$,{},$,$,$,$,(#{context}),#{units})", id(1, 1), step_str(&title)));
    let site_place = o.add(format!("IFCLOCALPLACEMENT($,#{world})"));
    let site = o.add(format!("IFCSITE('{}',$,'Site',$,$,#{site_place},$,$,.ELEMENT.,$,$,$,$,$)", id(1, 2)));
    let bldg_place = o.add(format!("IFCLOCALPLACEMENT(#{site_place},#{world})"));
    let building = o.add(format!("IFCBUILDING('{}',$,'Building',$,$,#{bldg_place},$,$,.ELEMENT.,$,$,$)", id(1, 3)));
    o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{project},(#{site}))", id(1, 4)));
    o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{site},(#{building}))", id(1, 5)));
    // A storey's ids come from its name, so adding a level doesn't change the others'.
    let mut storeys = Vec::new();
    for (lname, z) in &levels {
        let p = o.add(format!("IFCCARTESIANPOINT((0.,0.,{}))", real(z * k)));
        let a = o.add(format!("IFCAXIS2PLACEMENT3D(#{p},#{zdir},#{xdir})"));
        let place = o.add(format!("IFCLOCALPLACEMENT(#{bldg_place},#{a})"));
        let st = o.add(format!("IFCBUILDINGSTOREY('{}',$,{},$,$,#{place},$,$,.ELEMENT.,{})", id(hash(lname), 2), step_str(lname), real(z * k)));
        storeys.push((st, place, *z, hash(lname), base_of(lname)));
    }
    o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{building},{})", id(1, 6), refs(&storeys.iter().map(|s| s.0).collect::<Vec<_>>())));
    let mut contained: Vec<Vec<usize>> = vec![Vec::new(); storeys.len()];
    let mut spaces: Vec<Vec<usize>> = vec![Vec::new(); storeys.len()];
    let mut quantities: Vec<(usize, String, Vec<String>, u64)> = Vec::new();
    // MEP products grouped by system name for IfcDistributionSystem.
    let mut system_products: HashMap<String, Vec<usize>> = HashMap::new();
    let mut schedules: Vec<(usize, Vec<(String, String)>, u64)> = Vec::new();
    let mut mep_sizes: Vec<(usize, Vec<[f64; 2]>, u64)> = Vec::new();
    // Walls, doors and windows as written: for the openings the doors and windows fill.
    let mut placed: Vec<Placed> = Vec::new();
    // All products that have a BIM spec: (product instance number, entity handle).
    let mut bim_products: Vec<(usize, u64)> = Vec::new();
    // Products with a property store: (product, entity handle, IFC class written).
    let mut with_props: Vec<(usize, u64, &'static str)> = Vec::new();
    // Walls by their spec wall type (its index), for their materials and properties.
    let mut typed: Vec<(usize, usize)> = Vec::new();
    // Ceilings and soffits by their spec type: (soffit, type index, covering product).
    let mut covered: Vec<(bool, usize, usize)> = Vec::new();
    for it in &items {
        let si = storey_of(it);
        let Some(&(_, place_parent, sz, _, base)) = storeys.get(si) else { continue };
        // An object model.build moved to stack its level is that much further from its plan.
        let base = base + stacked_by(it.e);
        // The body in the storey's coordinates (its placement is at the storey's elevation; its
        // plan's base point at the building origin).
        let pts: Vec<String> = it
            .mesh
            .vertices
            .iter()
            .map(|v| format!("({},{},{})", real((v.x - base.x) * k), real((v.y - base.y) * k), real((v.z - sz) * k)))
            .collect();
        let list = o.add(format!("IFCCARTESIANPOINTLIST3D(({}))", pts.join(",")));
        let nv = it.mesh.vertices.len();
        let mut faces = Vec::new();
        for f in &it.mesh.faces {
            if f.v.len() < 3 || f.v.iter().any(|i| *i as usize >= nv) {
                continue;
            }
            let idx: Vec<String> = f.v.iter().map(|i| (i + 1).to_string()).collect();
            faces.push(o.add(format!("IFCINDEXEDPOLYGONALFACE(({}))", idx.join(","))));
        }
        if faces.is_empty() {
            continue;
        }
        let set = o.add(format!("IFCPOLYGONALFACESET(#{list},$,{},$)", refs(&faces)));
        let shape = o.add(format!("IFCSHAPEREPRESENTATION(#{body_ctx},'Body','Tessellation',(#{set}))"));
        let pds = o.add(format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{shape}))"));
        let place = o.add(format!("IFCLOCALPLACEMENT(#{place_parent},#{world})"));
        let g = element_id(it);
        let kd = &it.kind;
        let nm = step_str(if kd.name.is_empty() { "Element" } else { &kd.name });
        let tag = step_str(&it.e.handle.hex());
        let (entity, _) = kd.class.ifc();
        let pre = kd.predefined;
        let upper = entity.to_ascii_uppercase();
        let line = match kd.class {
            Class::Door | Class::Window => format!("{upper}('{g}',$,{nm},$,$,#{place},#{pds},{tag},$,$,.{pre}.,$,$)"),
            Class::Space => format!(
                "IFCSPACE('{g}',$,{},$,$,#{place},#{pds},{nm},.ELEMENT.,.{pre}.,$)",
                step_str(if kd.long.is_empty() { &kd.name } else { &kd.long })
            ),
            _ => {
                let desc = if kd.description.is_empty() { "$".to_string() } else { step_str(&kd.description) };
                let object_type = if kd.object_type.is_empty() { "$".to_string() } else { step_str(kd.object_type) };
                format!("{upper}('{g}',$,{nm},{desc},{object_type},#{place},#{pds},{tag},.{pre}.)")
            }
        };
        let product = o.add(line);
        // Elements with properties of their own: Pset_CADaddy and the standard sets.
        if cadaddy_doc::props::has_store(&it.e.common.xdata) {
            with_props.push((product, it.e.handle.0, entity));
        }
        // Track products with BIM spec marks for pset writing.
        if cadaddy_doc::bimspec::entity_type_mark(&it.e.common.xdata).is_some() {
            bim_products.push((product, it.e.handle.0));
        }
        if kd.class == Class::Wall
            && let Some(sp) = &spec
            && let Some(i) = wall_type_of(it.e).and_then(|n| sp.wall_types.iter().position(|t| t.name.eq_ignore_ascii_case(&n)))
        {
            typed.push((i, product));
        }
        if kd.class == Class::Ceiling
            && let Some(sp) = &spec
            && let Some(n) = wall_type_of(it.e)
        {
            let soffit = kd.predefined == "CLADDING";
            let i = if soffit {
                sp.soffit_types.iter().position(|t| t.name.eq_ignore_ascii_case(&n))
            } else {
                sp.ceiling_types.iter().position(|t| t.name.eq_ignore_ascii_case(&n))
            };
            if let Some(i) = i {
                covered.push((soffit, i, product));
            }
        }
        if matches!(kd.class, Class::Wall | Class::Door | Class::Window) {
            let (lo, hi) = it.mesh.vertices.iter().fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), v| {
                let p = [(v.x - base.x) * k, (v.y - base.y) * k, (v.z - sz) * k];
                (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
            });
            placed.push(Placed { class: kd.class, product, place, storey: si, lo, hi, handle: it.e.handle.0 });
        }
        if let Some(list) = if kd.class == Class::Space { spaces.get_mut(si) } else { contained.get_mut(si) } {
            list.push(product);
        }
        // Track MEP products by system name for IfcDistributionSystem grouping.
        if let Some(sys) = &kd.system_name {
            system_products.entry(sys.clone()).or_default().push(product);
        }
        for sys in &kd.systems {
            let list = system_products.entry(sys.clone()).or_default();
            if !list.contains(&product) {
                list.push(product);
            }
        }
        // Scheduled equipment: its schedule row as a property set (our own set, so not `Pset_`).
        if !kd.properties.is_empty() {
            schedules.push((product, kd.properties.clone(), it.e.handle.0));
        }
        if !kd.sizes.is_empty() {
            mep_sizes.push((product, kd.sizes.clone(), it.e.handle.0));
        }
        // Base quantities (IFC4's Qto_…BaseQuantities): lengths in the length unit, areas in m², volumes in m³.
        let m = measures(it.mesh);
        let (len, a, v) = (|x: f64| real(x * k), |x: f64| real(x * unit_scale * unit_scale), |x: f64| real(x * unit_scale.powi(3)));
        let q_len = |n: &str, x: f64| format!("IFCQUANTITYLENGTH('{n}',$,$,{},$)", len(x));
        let q_area = |n: &str, x: f64| format!("IFCQUANTITYAREA('{n}',$,$,{},$)", a(x));
        let q_vol = |n: &str, x: Option<f64>| x.map(|x| format!("IFCQUANTITYVOLUME('{n}',$,$,{},$)", v(x)));
        let (set_name, qs): (&str, Vec<String>) = match kd.class {
            Class::Wall => {
                ("Qto_WallBaseQuantities", [Some(q_len("Height", m.height)), q_vol("GrossVolume", m.volume)].into_iter().flatten().collect())
            }
            Class::Slab => {
                ("Qto_SlabBaseQuantities", [Some(q_area("GrossArea", m.footprint)), q_vol("GrossVolume", m.volume)].into_iter().flatten().collect())
            }
            Class::Ceiling => ("Qto_CoveringBaseQuantities", vec![q_area("GrossArea", m.footprint)]),
            Class::Column => {
                ("Qto_ColumnBaseQuantities", [Some(q_len("Length", m.height)), q_vol("GrossVolume", m.volume)].into_iter().flatten().collect())
            }
            Class::Door => ("Qto_DoorBaseQuantities", vec![q_len("Height", m.height)]),
            Class::Window => ("Qto_WindowBaseQuantities", vec![q_len("Height", m.height)]),
            Class::Space => (
                "Qto_SpaceBaseQuantities",
                [
                    Some(q_len("Height", m.height)),
                    Some(q_area("GrossFloorArea", m.footprint)),
                    Some(q_area("NetFloorArea", m.footprint)),
                    q_vol("GrossVolume", m.volume),
                ]
                .into_iter()
                .flatten()
                .collect(),
            ),
            _ => ("", Vec::new()),
        };
        if !qs.is_empty() {
            quantities.push((product, set_name.to_string(), qs, it.e.handle.0));
        }
    }
    // Each door and window fills an opening in its wall (IfcOpeningElement, voiding the wall): a
    // box through the wall's thickness over the door's extents, placed relative to the wall. The
    // wall's body already has the hole.
    let near = 0.01 / unit_scale * k;
    for fill in placed.iter().filter(|p| matches!(p.class, Class::Door | Class::Window)) {
        let overlap = |w: &Placed| -> f64 { (0..3).map(|i| (fill.hi[i].min(w.hi[i] + near) - fill.lo[i].max(w.lo[i] - near)).max(0.0)).product() };
        let host = placed
            .iter()
            .filter(|w| w.class == Class::Wall && w.storey == fill.storey)
            .map(|w| (w, overlap(w)))
            .filter(|(_, v)| *v > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(w, _)| w);
        let Some(wall) = host else { continue };
        // The wall's thin side: the opening goes through all of it.
        let thin = if wall.hi[0] - wall.lo[0] <= wall.hi[1] - wall.lo[1] { 0 } else { 1 };
        let mut lo = fill.lo;
        let mut hi = fill.hi;
        lo[thin] = wall.lo[thin].min(fill.lo[thin]);
        hi[thin] = wall.hi[thin].max(fill.hi[thin]);
        let corner = |i: usize| -> String {
            let pick = |axis: usize, bit: usize| if i >> bit & 1 == 1 { hi[axis] } else { lo[axis] };
            format!("({},{},{})", real(pick(0, 0)), real(pick(1, 1)), real(pick(2, 2)))
        };
        let list = o.add(format!("IFCCARTESIANPOINTLIST3D(({}))", (0..8).map(corner).collect::<Vec<_>>().join(",")));
        // The box's faces, facing out (corners numbered by their x, y, z bits).
        let faces: Vec<usize> = [[1, 3, 4, 2], [5, 6, 8, 7], [1, 2, 6, 5], [3, 7, 8, 4], [1, 5, 7, 3], [2, 4, 8, 6]]
            .iter()
            .map(|f| o.add(format!("IFCINDEXEDPOLYGONALFACE(({},{},{},{}))", f[0], f[1], f[2], f[3])))
            .collect();
        let set = o.add(format!("IFCPOLYGONALFACESET(#{list},.T.,{},$)", refs(&faces)));
        let shape = o.add(format!("IFCSHAPEREPRESENTATION(#{body_ctx},'Body','Tessellation',(#{set}))"));
        let pds = o.add(format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{shape}))"));
        let place = o.add(format!("IFCLOCALPLACEMENT(#{},#{world})", wall.place));
        let opening = o.add(format!("IFCOPENINGELEMENT('{}',$,'Opening',$,$,#{place},#{pds},$,.OPENING.)", id(fill.handle, 10)));
        o.add(format!("IFCRELVOIDSELEMENT('{}',$,$,$,#{},#{opening})", id(fill.handle, 11), wall.product));
        o.add(format!("IFCRELFILLSELEMENT('{}',$,$,$,#{opening},#{})", id(fill.handle, 12), fill.product));
    }
    if let Some(sp) = &spec {
        // Inches → the file's length unit.
        let inch = sp.units_per_foot / 12.0 * k;
        wall_materials(&mut o, sp, &typed, inch, &|n, salt| id(n, salt));
        covering_materials(&mut o, sp, &covered, inch, &|n, salt| id(n, salt));
    }
    for (product, set_name, qs, handle) in quantities {
        let ids: Vec<usize> = qs.into_iter().map(|q| o.add(q)).collect();
        let set = o.add(format!("IFCELEMENTQUANTITY('{}',$,'{set_name}',$,'BaseQuantities',{})", id(handle, 8), refs(&ids)));
        o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,(#{product}),#{set})", id(handle, 9)));
    }
    for (product, props, handle) in schedules {
        let ids: Vec<usize> =
            props.iter().map(|(k, v)| o.add(format!("IFCPROPERTYSINGLEVALUE({},$,IFCLABEL({}),$)", step_str(k), step_str(v)))).collect();
        let set = o.add(format!("IFCPROPERTYSET('{}',$,'CADaddy_EquipmentSchedule',$,{})", id(handle, 13), refs(&ids)));
        o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,(#{product}),#{set})", id(handle, 14)));
    }
    // MEP sizes (our own set): the nominal width and height of a rectangular section, or the
    // diameter of a round one, in the length unit; the size as printed; for a tee or a transition
    // the other size too.
    let per_inch = if mm { 25.4 } else { 0.0254 };
    for (product, sizes, handle) in mep_sizes {
        let mut ids = Vec::new();
        for (k, sz) in sizes.iter().enumerate().take(2) {
            let pre = if k == 0 { "" } else { "Other" };
            let mut len = |name: &str, v: f64| {
                ids.push(o.add(format!("IFCPROPERTYSINGLEVALUE('{pre}{name}',$,IFCPOSITIVELENGTHMEASURE({}),$)", real(v * per_inch))))
            };
            if sz[1] > 0.0 {
                len("NominalWidth", sz[0]);
                len("NominalHeight", sz[1]);
            } else {
                len("NominalDiameter", sz[0]);
            }
            ids.push(o.add(format!("IFCPROPERTYSINGLEVALUE('{pre}Size',$,IFCLABEL({}),$)", step_str(&size_label_in(*sz)))));
        }
        let set = o.add(format!("IFCPROPERTYSET('{}',$,'CADaddy_MepSize',$,{})", id(handle, 22), refs(&ids)));
        o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,(#{product}),#{set})", id(handle, 23)));
    }
    // BIM spec property sets (CaDaddy_Spec): entities with a type mark → IfcPropertySet with the
    // type's fields as IfcPropertySingleValue (all text). Attached via IfcRelDefinesByProperties.
    // Also: Pset_ManufacturerTypeInformation (standard IFC pset) and CaDaddy_SpecSource (provenance).
    for &(product, handle) in &bim_products {
        let ent = d.entity(cadaddy_doc::Handle(handle));
        let mark = ent.and_then(|e| cadaddy_doc::bimspec::entity_type_mark(&e.common.xdata));
        let ty = mark.and_then(|m| d.bim_types.get(m));
        if let (Some(mark), Some(ty)) = (mark, ty) {
            let mut prop_ids = Vec::new();
            // Type mark itself.
            let mk_val = o.add(format!("IFCPROPERTYSINGLEVALUE('TypeMark',$,IFCTEXT({}),$)", step_str(mark)));
            prop_ids.push(mk_val);
            if !ty.category.is_empty() {
                let cat = o.add(format!("IFCPROPERTYSINGLEVALUE('Category',$,IFCTEXT({}),$)", step_str(&ty.category)));
                prop_ids.push(cat);
            }
            for (key, field) in ty.fields.iter().take(200) {
                let prop = o.add(format!("IFCPROPERTYSINGLEVALUE({},$,IFCTEXT({}),$)", step_str(key), step_str(&field.value)));
                prop_ids.push(prop);
            }
            let pset = o.add(format!("IFCPROPERTYSET('{}',$,'CaDaddy_Spec',$,{})", id(handle, 20), refs(&prop_ids)));
            o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,(#{product}),#{pset})", id(handle, 21)));

            // Pset_ManufacturerTypeInformation (IFC4 standard pset): Manufacturer + ModelReference.
            let mfr = ty.get_field(cadaddy_doc::bimspec::keys::MANUFACTURER);
            let model_ref = ty.get_field(cadaddy_doc::bimspec::keys::CATALOG_NUMBER);
            if mfr.is_some() || model_ref.is_some() {
                let mut mfr_ids = Vec::new();
                if let Some(f) = mfr {
                    mfr_ids.push(o.add(format!("IFCPROPERTYSINGLEVALUE('Manufacturer',$,IFCLABEL({}),$)", step_str(&f.value))));
                }
                if let Some(f) = model_ref {
                    mfr_ids.push(o.add(format!("IFCPROPERTYSINGLEVALUE('ModelReference',$,IFCLABEL({}),$)", step_str(&f.value))));
                }
                let mfr_pset = o.add(format!("IFCPROPERTYSET('{}',$,'Pset_ManufacturerTypeInformation',$,{})", id(handle, 24), refs(&mfr_ids)));
                o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,(#{product}),#{mfr_pset})", id(handle, 25)));
            }

            // CaDaddy_SpecSource: source and confidence per field for provenance.
            let source_fields: Vec<_> = ty
                .fields
                .iter()
                .filter(|(_, f)| !f.source.is_empty() || f.confidence != cadaddy_doc::bimspec::Confidence::Stated)
                .take(200)
                .collect();
            if !source_fields.is_empty() {
                let mut src_ids = Vec::new();
                for (key, field) in &source_fields {
                    if !field.source.is_empty() {
                        src_ids.push(o.add(format!(
                            "IFCPROPERTYSINGLEVALUE({},$,IFCTEXT({}),$)",
                            step_str(&format!("{key}.source")),
                            step_str(&field.source)
                        )));
                    }
                    if field.confidence != cadaddy_doc::bimspec::Confidence::Stated {
                        src_ids.push(o.add(format!(
                            "IFCPROPERTYSINGLEVALUE({},$,IFCTEXT({}),$)",
                            step_str(&format!("{key}.confidence")),
                            step_str(field.confidence.as_str())
                        )));
                    }
                }
                let src_pset = o.add(format!("IFCPROPERTYSET('{}',$,'CaDaddy_SpecSource',$,{})", id(handle, 26), refs(&src_ids)));
                o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,(#{product}),#{src_pset})", id(handle, 27)));
            }
        }
    }
    for (product, handle, class) in with_props {
        let Some(e) = d.entity(cadaddy_doc::Handle(handle)) else { continue };
        element_props(&mut o, product, handle, class, &cadaddy_doc::props::read(&e.common.xdata), &|n, salt| id(n, salt));
    }
    for ((st, _, _, h, _), (inside, rooms)) in storeys.iter().zip(contained.iter().zip(&spaces)) {
        if !inside.is_empty() {
            o.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('{}',$,$,$,{},#{st})", id(*h, 3), refs(inside)));
        }
        if !rooms.is_empty() {
            o.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{st},{})", id(*h, 4), refs(rooms)));
        }
    }
    // IfcDistributionSystem for each MEP system, grouping its products.
    for (sys_name, products) in &system_products {
        if products.is_empty() {
            continue;
        }
        let sys_id = hash(sys_name);
        // IfcDistributionSystem: GlobalId, OwnerHistory, Name, Description, ObjectType, LongName, PredefinedType.
        let ds = o.add(format!("IFCDISTRIBUTIONSYSTEM('{}',$,{},$,$,$,.NOTDEFINED.)", id(sys_id, 20), step_str(sys_name),));
        o.add(format!("IFCRELASSIGNSTOGROUP('{}',$,$,$,{},$,#{ds})", id(sys_id, 21), refs(products),));
    }

    let mut s = String::with_capacity(o.lines.iter().map(|l| l.len() + 10).sum::<usize>() + 512);
    s.push_str("ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [ReferenceView]'),'2;1');\n");
    let _ = writeln!(s, "FILE_NAME({},'',(''),(''),'CADaddy','CADaddy','');", step_str(&format!("{title}.ifc")));
    s.push_str("FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n");
    for (i, l) in o.lines.iter().enumerate() {
        let _ = writeln!(s, "#{}={l};", i + 1);
    }
    s.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    Ok(s.into_bytes())
}

/// The wall type WALLGEN gave a wall (`type:<name>` after its source).
/// An IFC value of a property: a number as a real, a boolean as one, anything else as a label.
fn prop_value(p: &cadaddy_doc::props::Prop) -> String {
    match &p.value {
        serde_json::Value::Number(n) => {
            n.as_f64().filter(|v| v.is_finite()).map_or_else(|| format!("IFCLABEL({})", step_str(&p.text())), |v| format!("IFCREAL({})", real(v)))
        }
        serde_json::Value::Bool(b) => format!("IFCBOOLEAN(.{}.)", if *b { "T" } else { "F" }),
        _ => format!("IFCLABEL({})", step_str(&p.text())),
    }
}

/// An element's own properties ([`cadaddy_doc::props`]) as property sets: every one in
/// `Pset_CADaddy` (its unit, source, confidence and note in the property's description, so a
/// read gives them back), and the ones a standard set has a place for in that set too
/// (`Pset_WallCommon.FireRating`, `Pset_SpaceCommon.Reference`…).
fn element_props(o: &mut Out, product: usize, handle: u64, class: &str, store: &cadaddy_doc::props::Props, id: &dyn Fn(u64, u64) -> String) {
    use cadaddy_doc::props;
    if store.is_empty() {
        return;
    }
    let kind = props::kind_of_ifc_class(class);
    let mut own = Vec::new();
    let mut standard: Vec<(&'static str, Vec<usize>)> = Vec::new();
    for (key, p) in store.iter().take(props::MAX_PROPS) {
        let meta = serde_json::json!({ "unit": p.unit, "source": p.source, "confidence": p.confidence, "note": p.note });
        let meta: serde_json::Map<String, serde_json::Value> =
            meta.as_object().into_iter().flatten().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), v.clone())).collect();
        let desc = step_str(&serde_json::Value::Object(meta).to_string());
        let value = if p.has_value() { prop_value(p) } else { "$".to_string() };
        own.push(o.add(format!("IFCPROPERTYSINGLEVALUE({},{desc},{value},$)", step_str(key))));
        if let Some((set, name)) = props::standard_ifc(kind, key).filter(|_| p.has_value()) {
            let v = match name {
                "Reference" => format!("IFCIDENTIFIER({})", step_str(&p.text())),
                _ => format!("IFCLABEL({})", step_str(&p.text())),
            };
            let item = o.add(format!("IFCPROPERTYSINGLEVALUE('{name}',$,{v},$)"));
            match standard.iter_mut().find(|(s, _)| *s == set) {
                Some((_, items)) => items.push(item),
                None => standard.push((set, vec![item])),
            }
        }
    }
    let set = o.add(format!("IFCPROPERTYSET('{}',$,'{}',$,{})", id(handle, 28), props::IFC_PSET, refs(&own)));
    o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,(#{product}),#{set})", id(handle, 29)));
    for (k, (name, items)) in standard.into_iter().enumerate() {
        let salt = 30 + 2 * k as u64;
        let set = o.add(format!("IFCPROPERTYSET('{}',$,'{name}',$,{})", id(handle, salt), refs(&items)));
        o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,(#{product}),#{set})", id(handle, salt + 1)));
    }
}

fn wall_type_of(e: &Entity) -> Option<String> {
    tag_strings(e, "CADADDY_WALLGEN")?.iter().skip(3).find_map(|v| v.strip_prefix("type:")).map(str::to_string)
}

/// Each wall type with an assembly that walls were written with: its materials, its layer set
/// (one IfcMaterialLayer per layer, cavity insulation inside the studs' thickness), the usage
/// (centred on the wall's axis) its walls are associated with, and Pset_WallCommon. `typed`: (wall
/// type index, wall product); `inch`: the file's length unit per inch.
fn wall_materials(o: &mut Out, sp: &cadaddy_walk3d::BuildingSpec, typed: &[(usize, usize)], inch: f64, id: &dyn Fn(u64, u64) -> String) {
    let mut materials: HashMap<String, usize> = HashMap::new();
    let mut types: Vec<usize> = typed.iter().map(|(t, _)| *t).collect();
    types.sort_unstable();
    types.dedup();
    for ti in types {
        let Some(t) = sp.wall_types.get(ti) else { continue };
        let Some(a) = &t.assembly else { continue };
        let walls: Vec<usize> = typed.iter().filter(|(i, _)| *i == ti).map(|(_, p)| *p).collect();
        let key = hash(&format!("walltype:{}", t.name));
        let layers = a.layers.iter().take(cadaddy_walk3d::MAX_ASSEMBLY_LAYERS).filter(|l| l.thickness_in.is_finite() && l.thickness_in > 0.0);
        // Cavity insulation shares the studs' thickness: the studs keep what it leaves.
        let mut stack: Vec<(&cadaddy_walk3d::AssemblyLayer, f64)> = Vec::new();
        for l in layers {
            if l.in_cavity {
                if let Some(studs) = stack.iter_mut().rev().find(|(s, _)| s.side == cadaddy_walk3d::LayerSide::Core && !s.in_cavity) {
                    let take = l.thickness_in.min(studs.1);
                    studs.1 -= take;
                    stack.push((l, take));
                }
                continue;
            }
            stack.push((l, l.thickness_in));
        }
        let mut ids = Vec::new();
        // Every layer is written, even studs a batt fills (IFC allows a layer no thickness).
        for (l, th) in stack {
            let m = match materials.get(&l.material) {
                Some(m) => *m,
                None => {
                    let cat = if l.category.is_empty() { "$".to_string() } else { step_str(&l.category) };
                    let m = o.add(format!("IFCMATERIAL({},$,{cat})", step_str(&l.material)));
                    materials.insert(l.material.clone(), m);
                    m
                }
            };
            let side = match l.side {
                cadaddy_walk3d::LayerSide::Side1 => "Side 1",
                cadaddy_walk3d::LayerSide::Core => "Core",
                cadaddy_walk3d::LayerSide::Side2 => "Side 2",
            };
            let cat = if l.category.is_empty() { "$".to_string() } else { step_str(&l.category) };
            ids.push(o.add(format!("IFCMATERIALLAYER(#{m},{},.F.,{},'{side}',{cat},$)", real(th * inch), step_str(&l.material))));
        }
        if ids.is_empty() || walls.is_empty() {
            continue;
        }
        let total: f64 = a.layers.iter().filter(|l| !l.in_cavity && l.thickness_in.is_finite()).map(|l| l.thickness_in).sum();
        let label = if a.tag.is_empty() { t.name.clone() } else { a.tag.clone() };
        let set = o.add(format!(
            "IFCMATERIALLAYERSET({},{},{})",
            refs(&ids),
            step_str(&format!("Wall type {label}")),
            if a.description.is_empty() { "$".to_string() } else { step_str(&a.description) }
        ));
        let usage = o.add(format!("IFCMATERIALLAYERSETUSAGE(#{set},.AXIS2.,.POSITIVE.,{},$)", real(-total * inch / 2.0)));
        o.add(format!("IFCRELASSOCIATESMATERIAL('{}',$,$,$,{},#{usage})", id(key, 13), refs(&walls)));
        // Pset_WallCommon: what the legend says about the type.
        let mut props = vec![o.add(format!("IFCPROPERTYSINGLEVALUE('Reference',$,IFCIDENTIFIER({}),$)", step_str(&label)))];
        if let Some(r) = &a.fire_rating {
            props.push(o.add(format!("IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL({}),$)", step_str(r))));
        }
        if let Some(r) = &a.acoustic_rating {
            props.push(o.add(format!("IFCPROPERTYSINGLEVALUE('AcousticRating',$,IFCLABEL({}),$)", step_str(r))));
        }
        let ext = if a.external { ".T." } else { ".F." };
        props.push(o.add(format!("IFCPROPERTYSINGLEVALUE('IsExternal',$,IFCBOOLEAN({ext}),$)")));
        let pset = o.add(format!("IFCPROPERTYSET('{}',$,'Pset_WallCommon',$,{})", id(key, 14), refs(&props)));
        o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,{},#{pset})", id(key, 15), refs(&walls)));
        // The head of wall detail: how it meets what is above, and its bottom track.
        if let Some(h) = &a.head {
            let word = |v: &str| step_str(v);
            let track = match h.track {
                cadaddy_walk3d::HeadTrack::Single => "top track",
                cadaddy_walk3d::HeadTrack::Deflection => "slotted deflection track",
                cadaddy_walk3d::HeadTrack::Double => "nested double track",
                cadaddy_walk3d::HeadTrack::Angle => "steel angles",
            };
            let to = if h.to == cadaddy_walk3d::HeadTo::Deck { "deck" } else { "ceiling" };
            let mut hp = vec![
                o.add(format!("IFCPROPERTYSINGLEVALUE('HeadTrack',$,IFCLABEL({}),$)", word(track))),
                o.add(format!("IFCPROPERTYSINGLEVALUE('HeadTo',$,IFCLABEL({}),$)", word(to))),
                o.add(format!("IFCPROPERTYSINGLEVALUE('BottomTrack',$,IFCBOOLEAN({}),$)", if h.bottom_track { ".T." } else { ".F." })),
            ];
            if let Some(d) = h.deflection_in.filter(|d| d.is_finite()) {
                hp.push(o.add(format!("IFCPROPERTYSINGLEVALUE('Deflection',$,IFCLENGTHMEASURE({}),$)", real(d * inch))));
            }
            if let Some(det) = &h.detail {
                hp.push(o.add(format!("IFCPROPERTYSINGLEVALUE('Detail',$,IFCLABEL({}),$)", word(det))));
            }
            let pset = o.add(format!("IFCPROPERTYSET('{}',$,'CADaddy_HeadOfWall',$,{})", id(key, 16), refs(&hp)));
            o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,{},#{pset})", id(key, 17), refs(&walls)));
        }
    }
}

/// Each ceiling and soffit type coverings were written with: a layer set from the room's face up
/// (boards or tile, then framing; a soffit's bottom boards, then its framing), the usage its
/// coverings are associated with, and Pset_CoveringCommon. `covered`: (soffit, type index,
/// covering product).
fn covering_materials(o: &mut Out, sp: &cadaddy_walk3d::BuildingSpec, covered: &[(bool, usize, usize)], inch: f64, id: &dyn Fn(u64, u64) -> String) {
    let mut materials: HashMap<String, usize> = HashMap::new();
    let mut keys: Vec<(bool, usize)> = covered.iter().map(|(s, i, _)| (*s, *i)).collect();
    keys.sort_unstable();
    keys.dedup();
    for (soffit, ti) in keys {
        let products: Vec<usize> = covered.iter().filter(|(s, i, _)| *s == soffit && *i == ti).map(|(_, _, p)| *p).collect();
        let (name, description, layers, fire): (&str, &str, Vec<&cadaddy_walk3d::AssemblyLayer>, Option<&String>) = if soffit {
            let Some(t) = sp.soffit_types.get(ti) else { continue };
            (&t.name, &t.description, t.bottom.iter().chain(&t.framing).collect(), None)
        } else {
            let Some(t) = sp.ceiling_types.get(ti) else { continue };
            (&t.name, &t.description, t.layers.iter().collect(), t.fire_rating.as_ref())
        };
        let key = hash(&format!("{}:{name}", if soffit { "soffittype" } else { "ceilingtype" }));
        let mut ids = Vec::new();
        for l in layers.into_iter().take(cadaddy_walk3d::MAX_ASSEMBLY_LAYERS).filter(|l| l.thickness_in.is_finite() && l.thickness_in > 0.0) {
            let cat = if l.category.is_empty() { "$".to_string() } else { step_str(&l.category) };
            let m = match materials.get(&l.material) {
                Some(m) => *m,
                None => {
                    let m = o.add(format!("IFCMATERIAL({},$,{cat})", step_str(&l.material)));
                    materials.insert(l.material.clone(), m);
                    m
                }
            };
            ids.push(o.add(format!("IFCMATERIALLAYER(#{m},{},.F.,{},$,{cat},$)", real(l.thickness_in * inch), step_str(&l.material))));
        }
        if ids.is_empty() || products.is_empty() {
            continue;
        }
        let label = format!("{} type {name}", if soffit { "Soffit" } else { "Ceiling" });
        let desc = if description.is_empty() { "$".to_string() } else { step_str(description) };
        let set = o.add(format!("IFCMATERIALLAYERSET({},{},{desc})", refs(&ids), step_str(&label)));
        // Layers stack up from the covering's underside (the room's face).
        let usage = o.add(format!("IFCMATERIALLAYERSETUSAGE(#{set},.AXIS3.,.POSITIVE.,0.,$)"));
        o.add(format!("IFCRELASSOCIATESMATERIAL('{}',$,$,$,{},#{usage})", id(key, 13), refs(&products)));
        let mut props = vec![o.add(format!("IFCPROPERTYSINGLEVALUE('Reference',$,IFCIDENTIFIER({}),$)", step_str(name)))];
        if let Some(r) = fire {
            props.push(o.add(format!("IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL({}),$)", step_str(r))));
        }
        let pset = o.add(format!("IFCPROPERTYSET('{}',$,'Pset_CoveringCommon',$,{})", id(key, 14), refs(&props)));
        o.add(format!("IFCRELDEFINESBYPROPERTIES('{}',$,$,$,{},#{pset})", id(key, 15), refs(&products)));
    }
}

// ---------------- what comes in ----------------

/// One parameter of an instance.
#[derive(Clone, Debug, PartialEq)]
enum P {
    Ref(u64),
    Num(f64),
    Str(String),
    Enum(String),
    List(Vec<P>),
    Typed(String, Vec<P>),
    Null,
}

impl P {
    fn num(&self) -> Option<f64> {
        match self {
            P::Num(v) => Some(*v),
            P::Typed(_, a) => a.first().and_then(P::num),
            _ => None,
        }
    }
    fn list(&self) -> &[P] {
        match self {
            P::List(v) => v,
            _ => &[],
        }
    }
    fn reference(&self) -> Option<u64> {
        match self {
            P::Ref(r) => Some(*r),
            _ => None,
        }
    }
    fn text(&self) -> Option<&str> {
        match self {
            P::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// The file's instances: number → (TYPE in upper case, parameters).
type Model = HashMap<u64, (String, Vec<P>)>;

struct Lexer<'a> {
    b: &'a [u8],
    at: usize,
}

impl Lexer<'_> {
    fn skip(&mut self) {
        loop {
            while self.b.get(self.at).is_some_and(u8::is_ascii_whitespace) {
                self.at += 1;
            }
            if self.b.get(self.at..self.at + 2) == Some(b"/*") {
                match self.b.get(self.at + 2..).and_then(|r| r.windows(2).position(|w| w == b"*/")) {
                    Some(e) => self.at += e + 4,
                    None => self.at = self.b.len(),
                }
                continue;
            }
            break;
        }
    }

    /// A STEP string after its opening `'`: `''` is a quote, `\X2\hhhh…\X0\` UTF-16, `\X\hh` a byte.
    fn string(&mut self) -> Option<String> {
        let mut raw = Vec::new();
        loop {
            let c = *self.b.get(self.at)?;
            self.at += 1;
            if c == b'\'' {
                if self.b.get(self.at) == Some(&b'\'') {
                    raw.push(b'\'');
                    self.at += 1;
                    continue;
                }
                break;
            }
            raw.push(c);
            if raw.len() > 1 << 20 {
                return None;
            }
        }
        Some(decode_step_string(&raw))
    }

    fn word(&mut self) -> String {
        let start = self.at;
        while self.b.get(self.at).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
            self.at += 1;
        }
        String::from_utf8_lossy(&self.b[start..self.at]).to_ascii_uppercase()
    }

    fn param(&mut self, depth: usize) -> Option<P> {
        if depth > 64 {
            return None;
        }
        self.skip();
        let c = *self.b.get(self.at)?;
        match c {
            b'#' => {
                self.at += 1;
                let start = self.at;
                while self.b.get(self.at).is_some_and(u8::is_ascii_digit) {
                    self.at += 1;
                }
                std::str::from_utf8(&self.b[start..self.at]).ok()?.parse().ok().map(P::Ref)
            }
            b'\'' => {
                self.at += 1;
                self.string().map(P::Str)
            }
            b'.' if self.b.get(self.at + 1).is_some_and(u8::is_ascii_alphabetic) => {
                self.at += 1;
                let w = self.word();
                (self.b.get(self.at) == Some(&b'.')).then(|| {
                    self.at += 1;
                    P::Enum(w)
                })
            }
            b'$' | b'*' => {
                self.at += 1;
                Some(P::Null)
            }
            b'(' => {
                self.at += 1;
                self.list(depth).map(P::List)
            }
            b'"' => {
                // Binary: hex digits.
                self.at += 1;
                while self.b.get(self.at).is_some_and(|c| *c != b'"') {
                    self.at += 1;
                }
                self.at += 1;
                Some(P::Null)
            }
            c if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' => {
                let start = self.at;
                self.at += 1;
                while self.b.get(self.at).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b'E' | b'e' | b'-' | b'+')) {
                    self.at += 1;
                }
                let t = std::str::from_utf8(&self.b[start..self.at]).ok()?;
                // STEP allows `1.E-05`; Rust parses it as written.
                t.parse::<f64>().ok().filter(|v| v.is_finite()).map(P::Num).or(Some(P::Num(0.0)))
            }
            c if c.is_ascii_alphabetic() => {
                let w = self.word();
                self.skip();
                if self.b.get(self.at) == Some(&b'(') {
                    self.at += 1;
                    self.list(depth).map(|a| P::Typed(w, a))
                } else {
                    Some(P::Enum(w))
                }
            }
            _ => None,
        }
    }

    /// Parameters after `(` up to the matching `)`.
    fn list(&mut self, depth: usize) -> Option<Vec<P>> {
        let mut out = Vec::new();
        loop {
            self.skip();
            match self.b.get(self.at)? {
                b')' => {
                    self.at += 1;
                    return Some(out);
                }
                b',' => self.at += 1,
                _ => {
                    out.push(self.param(depth + 1)?);
                    if out.len() > 50_000_000 {
                        return None;
                    }
                }
            }
        }
    }
}

fn decode_step_string(raw: &[u8]) -> String {
    let t = String::from_utf8_lossy(raw);
    let mut out = String::new();
    let mut rest: &str = &t;
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(body) = tail.strip_prefix("\\X2\\") {
            let end = body.find("\\X0\\").unwrap_or(body.len());
            let units: Vec<u16> = (0..end / 4).filter_map(|k| u16::from_str_radix(body.get(k * 4..k * 4 + 4)?, 16).ok()).collect();
            out.push_str(&String::from_utf16_lossy(&units));
            rest = body.get(end + 4..).unwrap_or("");
        } else if let Some(body) = tail.strip_prefix("\\X4\\") {
            let end = body.find("\\X0\\").unwrap_or(body.len());
            let chars = (0..end / 8).filter_map(|k| u32::from_str_radix(body.get(k * 8..k * 8 + 8)?, 16).ok()).filter_map(char::from_u32);
            out.extend(chars);
            rest = body.get(end + 4..).unwrap_or("");
        } else if let Some(body) = tail.strip_prefix("\\S\\") {
            // ISO 8859's upper half: the next character + 128.
            let mut cs = body.chars();
            if let Some(c) = cs.next().and_then(|c| char::from_u32(u32::from(c) + 128)) {
                out.push(c);
            }
            rest = cs.as_str();
        } else if tail.starts_with("\\P") && tail.get(3..4) == Some("\\") {
            // A code page directive (\PA\): Latin-1 is read as Latin-1.
            rest = tail.get(4..).unwrap_or("");
        } else if let Some(body) = tail.strip_prefix("\\X\\") {
            if let Some(b) = body.get(..2).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(char::from(b));
            }
            rest = body.get(2..).unwrap_or("");
        } else if let Some(body) = tail.strip_prefix("\\\\") {
            out.push('\\');
            rest = body;
        } else {
            out.push('\\');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The DATA section's instances.
fn parse(bytes: &[u8]) -> Result<Model> {
    let start = find(bytes, b"DATA;").ok_or_else(|| IoError::Format("not an IFC (STEP) file: no DATA section".into()))? + 5;
    if !bytes.starts_with(b"ISO-10303-21") && find(bytes.get(..bytes.len().min(256)).unwrap_or_default(), b"ISO-10303-21").is_none() {
        return Err(IoError::Format("not an IFC (STEP) file".into()));
    }
    let mut lx = Lexer { b: bytes, at: start };
    let mut m: Model = HashMap::new();
    loop {
        lx.skip();
        match lx.b.get(lx.at) {
            Some(b'#') => {}
            _ => break,
        }
        lx.at += 1;
        let s = lx.at;
        while lx.b.get(lx.at).is_some_and(u8::is_ascii_digit) {
            lx.at += 1;
        }
        let id: Option<u64> = std::str::from_utf8(&lx.b[s..lx.at]).ok().and_then(|t| t.parse().ok());
        lx.skip();
        let ok = lx.b.get(lx.at) == Some(&b'=');
        lx.at += 1;
        lx.skip();
        let name = lx.word();
        lx.skip();
        let params = if ok && lx.b.get(lx.at) == Some(&b'(') {
            lx.at += 1;
            lx.list(0)
        } else {
            None
        };
        // A damaged instance is skipped to the next `;`.
        match (id, params) {
            (Some(id), Some(p)) if !name.is_empty() => {
                m.insert(id, (name, p));
            }
            _ => {}
        }
        match lx.b.get(lx.at..).and_then(|r| r.iter().position(|c| *c == b';')) {
            Some(e) => lx.at += e + 1,
            None => break,
        }
        if m.len() > MAX_INSTANCES {
            return Err(IoError::Format(format!("more than {MAX_INSTANCES} instances")));
        }
    }
    if m.is_empty() {
        return Err(IoError::Format("the IFC file has no instances".into()));
    }
    Ok(m)
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// An affine transform: three rows of [x y z t].
type M = [[f64; 4]; 3];
const ID: M = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

fn mul(a: &M, b: &M) -> M {
    let mut r = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..4 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j] + if j == 3 { a[i][3] } else { 0.0 };
        }
    }
    r
}

fn apply(m: &M, p: Vec3) -> Vec3 {
    Vec3::new(
        m[0][0] * p.x + m[0][1] * p.y + m[0][2] * p.z + m[0][3],
        m[1][0] * p.x + m[1][1] * p.y + m[1][2] * p.z + m[1][3],
        m[2][0] * p.x + m[2][1] * p.y + m[2][2] * p.z + m[2][3],
    )
}

struct Reader<'a> {
    m: &'a Model,
}

impl Reader<'_> {
    fn get(&self, p: &P) -> Option<&(String, Vec<P>)> {
        self.m.get(&p.reference()?)
    }

    fn coords(&self, p: &P) -> Vec<f64> {
        match self.get(p) {
            Some((_, a)) => a.first().map(|l| l.list().iter().filter_map(P::num).collect()).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    fn point(&self, p: &P) -> Option<Vec3> {
        let c = self.coords(p);
        Some(Vec3::new(*c.first()?, *c.get(1)?, c.get(2).copied().unwrap_or(0.0)))
    }

    fn direction(&self, p: &P) -> Option<Vec3> {
        let v = self.point(p)?;
        let l = v.len();
        (l > 1e-12 && l.is_finite()).then(|| v * (1.0 / l))
    }

    /// An IfcAxis2Placement3D (or 2D) as a transform.
    fn axis(&self, p: &P) -> M {
        let Some((name, a)) = self.get(p) else { return ID };
        let o = a.first().and_then(|x| self.point(x)).unwrap_or_default();
        let (z, xr) = if name == "IFCAXIS2PLACEMENT2D" {
            (Vec3::new(0.0, 0.0, 1.0), a.get(1).and_then(|x| self.direction(x)).unwrap_or(Vec3::new(1.0, 0.0, 0.0)))
        } else {
            (
                a.get(1).and_then(|x| self.direction(x)).unwrap_or(Vec3::new(0.0, 0.0, 1.0)),
                a.get(2).and_then(|x| self.direction(x)).unwrap_or(Vec3::new(1.0, 0.0, 0.0)),
            )
        };
        let mut x = xr - z * xr.dot(z);
        if x.len() < 1e-9 {
            x = if z.x.abs() < 0.9 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
            x = x - z * x.dot(z);
        }
        let x = x * (1.0 / x.len());
        let y = z.cross(x);
        [[x.x, y.x, z.x, o.x], [x.y, y.y, z.y, o.y], [x.z, y.z, z.z, o.z]]
    }

    /// An IfcLocalPlacement chain as a transform to world.
    fn placement(&self, p: &P, depth: usize) -> M {
        if depth > MAX_DEPTH {
            return ID;
        }
        let Some((name, a)) = self.get(p) else { return ID };
        if name != "IFCLOCALPLACEMENT" {
            return ID;
        }
        let parent = a.first().filter(|x| **x != P::Null).map_or(ID, |x| self.placement(x, depth + 1));
        let local = a.get(1).map_or(ID, |x| self.axis(x));
        mul(&parent, &local)
    }

    /// The faces (outer loops, local coordinates) of geometry item `p`, transformed by `t`.
    fn item(&self, p: &P, t: &M, depth: usize, out: &mut Vec<Vec<Vec3>>) {
        if depth > MAX_DEPTH || out.len() > MAX_FACES {
            return;
        }
        let Some((name, a)) = self.get(p) else { return };
        match name.as_str() {
            "IFCPOLYGONALFACESET" => {
                let pts = self.point_list(a.first());
                let pn: Vec<usize> = a.get(3).map(|l| l.list().iter().filter_map(|x| x.num().map(|v| v as usize)).collect()).unwrap_or_default();
                let at = |i: usize| -> Option<Vec3> {
                    let j = if pn.is_empty() { i } else { *pn.get(i.checked_sub(1)?)? };
                    pts.get(j.checked_sub(1)?).map(|v| apply(t, *v))
                };
                for f in a.get(2).map(P::list).unwrap_or_default() {
                    let Some((_, fa)) = self.get(f) else { continue };
                    let face: Option<Vec<Vec3>> = fa.first().map(P::list).unwrap_or_default().iter().map(|i| at(i.num()? as usize)).collect();
                    if let Some(face) = face.filter(|f| f.len() >= 3) {
                        out.push(face);
                    }
                }
            }
            "IFCTRIANGULATEDFACESET" => {
                let pts = self.point_list(a.first());
                let pn: Vec<usize> = a.get(4).map(|l| l.list().iter().filter_map(|x| x.num().map(|v| v as usize)).collect()).unwrap_or_default();
                let at = |i: usize| -> Option<Vec3> {
                    let j = if pn.is_empty() { i } else { *pn.get(i.checked_sub(1)?)? };
                    pts.get(j.checked_sub(1)?).map(|v| apply(t, *v))
                };
                for tri in a.get(3).map(P::list).unwrap_or_default() {
                    let face: Option<Vec<Vec3>> = tri.list().iter().map(|i| at(i.num()? as usize)).collect();
                    if let Some(face) = face.filter(|f| f.len() == 3) {
                        out.push(face);
                    }
                }
            }
            "IFCFACETEDBREP"
            | "IFCFACEBASEDSURFACEMODEL"
            | "IFCSHELLBASEDSURFACEMODEL"
            | "IFCCLOSEDSHELL"
            | "IFCOPENSHELL"
            | "IFCCONNECTEDFACESET" => {
                let children: Vec<&P> = match name.as_str() {
                    "IFCFACETEDBREP" => a.first().into_iter().collect(),
                    "IFCCLOSEDSHELL" | "IFCOPENSHELL" | "IFCCONNECTEDFACESET" => {
                        // Faces directly.
                        for f in a.first().map(P::list).unwrap_or_default() {
                            if let Some(face) = self.face(f, t) {
                                out.push(face);
                            }
                            if out.len() > MAX_FACES {
                                return;
                            }
                        }
                        Vec::new()
                    }
                    _ => a.first().map(P::list).unwrap_or_default().iter().collect(),
                };
                for c in children {
                    self.item(c, t, depth + 1, out);
                }
            }
            "IFCEXTRUDEDAREASOLID" => {
                let profile = a.first().map(|x| self.profile(x)).unwrap_or_default();
                if profile.len() < 3 {
                    return;
                }
                let pos = a.get(1).map_or(ID, |x| self.axis(x));
                let dir = a.get(2).and_then(|x| self.direction(x)).unwrap_or(Vec3::new(0.0, 0.0, 1.0));
                let depth_v = a.get(3).and_then(P::num).unwrap_or(0.0);
                if !depth_v.is_finite() || depth_v.abs() < 1e-12 {
                    return;
                }
                let m = mul(t, &pos);
                let off = dir * depth_v;
                let bottom: Vec<Vec3> = profile.iter().map(|q| apply(&m, *q)).collect();
                let top: Vec<Vec3> = profile.iter().map(|q| apply(&m, *q + off)).collect();
                out.push(bottom.iter().rev().copied().collect());
                out.push(top.clone());
                let n = profile.len();
                for i in 0..n {
                    let j = (i + 1) % n;
                    out.push(vec![bottom[i], bottom[j], top[j], top[i]]);
                }
            }
            "IFCMAPPEDITEM" => {
                let Some((_, src)) = a.first().and_then(|x| self.get(x)) else { return };
                let origin = src.first().map_or(ID, |x| self.axis(x));
                let target = a.get(1).map_or(ID, |x| self.operator(x));
                let m = mul(&mul(t, &target), &origin);
                if let Some((_, rep)) = src.get(1).and_then(|x| self.get(x)) {
                    for it in rep.get(3).map(P::list).unwrap_or_default() {
                        self.item(it, &m, depth + 1, out);
                    }
                }
            }
            "IFCBOOLEANRESULT" | "IFCBOOLEANCLIPPINGRESULT" => {
                // The body before openings and clips are taken away.
                if let Some(first) = a.get(1) {
                    self.item(first, t, depth + 1, out);
                }
            }
            _ => {}
        }
    }

    fn point_list(&self, p: Option<&P>) -> Vec<Vec3> {
        let Some((_, a)) = p.and_then(|x| self.get(x)) else { return Vec::new() };
        a.first()
            .map(P::list)
            .unwrap_or_default()
            .iter()
            .take(MAX_POINTS)
            .filter_map(|c| {
                let v: Vec<f64> = c.list().iter().filter_map(P::num).collect();
                Some(Vec3::new(*v.first()?, *v.get(1)?, v.get(2).copied().unwrap_or(0.0)))
            })
            .collect()
    }

    /// An IfcFace's outer loop.
    fn face(&self, f: &P, t: &M) -> Option<Vec<Vec3>> {
        let (_, fa) = self.get(f)?;
        let bounds = fa.first()?.list();
        let outer = bounds.iter().find(|b| self.get(b).is_some_and(|(n, _)| n == "IFCFACEOUTERBOUND")).or_else(|| bounds.first())?;
        let (_, ba) = self.get(outer)?;
        let (lname, la) = self.get(ba.first()?)?;
        if lname != "IFCPOLYLOOP" {
            return None;
        }
        let pts: Vec<Vec3> = la.first()?.list().iter().filter_map(|p| self.point(p)).map(|v| apply(t, v)).collect();
        (pts.len() >= 3).then_some(pts)
    }

    /// A profile's outline in its own plane (z = 0).
    fn profile(&self, p: &P) -> Vec<Vec3> {
        let Some((name, a)) = self.get(p) else { return Vec::new() };
        match name.as_str() {
            "IFCRECTANGLEPROFILEDEF" => {
                let pos = a.get(2).map_or(ID, |x| self.axis(x));
                let (w, h) = (a.get(3).and_then(P::num).unwrap_or(0.0) / 2.0, a.get(4).and_then(P::num).unwrap_or(0.0) / 2.0);
                [(-w, -h), (w, -h), (w, h), (-w, h)].iter().map(|(x, y)| apply(&pos, Vec3::new(*x, *y, 0.0))).collect()
            }
            "IFCCIRCLEPROFILEDEF" => {
                let pos = a.get(2).map_or(ID, |x| self.axis(x));
                let r = a.get(3).and_then(P::num).unwrap_or(0.0);
                (0..24)
                    .map(|i| {
                        let t = std::f64::consts::TAU * f64::from(i) / 24.0;
                        apply(&pos, Vec3::new(r * t.cos(), r * t.sin(), 0.0))
                    })
                    .collect()
            }
            "IFCARBITRARYCLOSEDPROFILEDEF" | "IFCARBITRARYPROFILEDEFWITHVOIDS" => {
                let mut pts = a.get(2).map(|c| self.curve(c)).unwrap_or_default();
                if pts.len() > 1 && (pts[0] - pts[pts.len() - 1]).len() < 1e-9 {
                    pts.pop();
                }
                pts
            }
            _ => Vec::new(),
        }
    }

    fn curve(&self, p: &P) -> Vec<Vec3> {
        let Some((name, a)) = self.get(p) else { return Vec::new() };
        match name.as_str() {
            "IFCPOLYLINE" => a.first().map(P::list).unwrap_or_default().iter().take(MAX_POINTS).filter_map(|x| self.point(x)).collect(),
            "IFCINDEXEDPOLYCURVE" => self.point_list(a.first()),
            _ => Vec::new(),
        }
    }

    /// An IfcCartesianTransformationOperator3D (axes, origin, scale) as a transform.
    fn operator(&self, p: &P) -> M {
        let Some((_, a)) = self.get(p) else { return ID };
        let x = a.first().and_then(|v| self.direction(v)).unwrap_or(Vec3::new(1.0, 0.0, 0.0));
        let y = a.get(1).and_then(|v| self.direction(v)).unwrap_or(Vec3::new(0.0, 1.0, 0.0));
        let o = a.get(2).and_then(|v| self.point(v)).unwrap_or_default();
        let s = a.get(3).and_then(P::num).filter(|s| s.is_finite() && *s > 0.0).unwrap_or(1.0);
        let z = a.get(4).and_then(|v| self.direction(v)).unwrap_or_else(|| x.cross(y));
        [[x.x * s, y.x * s, z.x * s, o.x], [x.y * s, y.y * s, z.y * s, o.y], [x.z * s, y.z * s, z.z * s, o.z]]
    }

    /// Metres per length unit of the project (1 when the file says none).
    fn unit(&self) -> f64 {
        let Some((_, project)) = self.m.values().find(|(n, _)| n == "IFCPROJECT") else { return 1.0 };
        let Some((_, ua)) = project.get(8).and_then(|x| self.get(x)) else { return 1.0 };
        for u in ua.first().map(P::list).unwrap_or_default() {
            let Some((name, a)) = self.get(u) else { continue };
            if a.get(1) != Some(&P::Enum("LENGTHUNIT".into())) {
                continue;
            }
            match name.as_str() {
                "IFCSIUNIT" => {
                    let prefix = match a.get(2) {
                        Some(P::Enum(p)) => match p.as_str() {
                            "MILLI" => 0.001,
                            "CENTI" => 0.01,
                            "DECI" => 0.1,
                            "KILO" => 1000.0,
                            "MICRO" => 1e-6,
                            _ => 1.0,
                        },
                        _ => 1.0,
                    };
                    return prefix;
                }
                "IFCCONVERSIONBASEDUNIT" => {
                    let factor = a.get(3).and_then(|x| self.get(x)).and_then(|(_, m)| m.first()?.num());
                    if let Some(f) = factor.filter(|f| f.is_finite() && *f > 0.0) {
                        // The factor is in the conversion's own unit, usually metres or millimetres.
                        return if f > 1.0 { f * 0.001 } else { f };
                    }
                    let name = a.get(2).and_then(P::text).unwrap_or_default().to_ascii_uppercase();
                    return if name.contains("INCH") {
                        0.0254
                    } else if name.contains("FOOT") || name.contains("FEET") {
                        0.3048
                    } else {
                        1.0
                    };
                }
                _ => {}
            }
        }
        1.0
    }
}

/// Most properties and quantities kept per object read from IFC (hostile files).
const MAX_PROPS: usize = 96;
/// Most characters of one tag string (DXF extended data strings hold 255).
const MAX_TAG: usize = 250;

/// What an IFC file says about its objects beyond their bodies: the spatial structure each sits
/// in (IfcRelContainedInSpatialStructure, IfcRelAggregates), its property and quantity sets
/// (IfcRelDefinesByProperties), its type (IfcRelDefinesByType) and the systems it is grouped
/// into (IfcRelAssignsToGroup).
#[derive(Default)]
struct Relations {
    /// Object → the spatial element or object it is part of.
    parent: HashMap<u64, u64>,
    /// Object → its property and quantity sets.
    sets: HashMap<u64, Vec<u64>>,
    /// Object → its type object.
    types: HashMap<u64, u64>,
    /// Object → the names of the systems it belongs to.
    systems: HashMap<u64, Vec<String>>,
}

/// The ids in list parameter `i` of `a`.
fn ref_list(a: &[P], i: usize) -> Vec<u64> {
    a.get(i).map(P::list).unwrap_or_default().iter().filter_map(P::reference).collect()
}

/// A property's value as text: `IFCLABEL('x')` → `x`, `.T.` → `TRUE`, numbers trimmed.
fn value_text(p: &P) -> Option<String> {
    match p {
        P::Typed(_, a) => a.first().and_then(value_text),
        P::Str(s) => Some(s.clone()),
        P::Num(v) => Some(num_text(*v)),
        P::Enum(e) => Some(match e.as_str() {
            "T" => "TRUE".into(),
            "F" => "FALSE".into(),
            "U" => "UNKNOWN".into(),
            _ => e.clone(),
        }),
        P::List(l) => {
            let parts: Vec<String> = l.iter().take(16).filter_map(value_text).collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        P::Ref(_) | P::Null => None,
    }
}

/// A number with at most six decimals and no trailing zeros.
fn num_text(v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let t = format!("{v:.6}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    if t == "-0" { "0".into() } else { t.to_string() }
}

/// The first `n` characters of `s`.
fn cap(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

impl Reader<'_> {
    fn relations(&self) -> Relations {
        let mut r = Relations::default();
        for (name, a) in self.m.values() {
            match name.as_str() {
                // GlobalId, OwnerHistory, Name, Description, RelatingObject, RelatedObjects.
                "IFCRELAGGREGATES" | "IFCRELNESTS" => {
                    if let Some(whole) = a.get(4).and_then(P::reference) {
                        for part in ref_list(a, 5) {
                            r.parent.entry(part).or_insert(whole);
                        }
                    }
                }
                // …, RelatedElements, RelatingStructure.
                "IFCRELCONTAINEDINSPATIALSTRUCTURE" => {
                    if let Some(st) = a.get(5).and_then(P::reference) {
                        for e in ref_list(a, 4) {
                            r.parent.insert(e, st);
                        }
                    }
                }
                // …, RelatedObjects, RelatingPropertyDefinition.
                "IFCRELDEFINESBYPROPERTIES" => {
                    let sets: Vec<u64> = match a.get(5) {
                        Some(P::List(l)) => l.iter().filter_map(P::reference).collect(),
                        Some(p) => p.reference().into_iter().collect(),
                        None => Vec::new(),
                    };
                    for o in ref_list(a, 4) {
                        let list = r.sets.entry(o).or_default();
                        if list.len() < MAX_PROPS {
                            list.extend(sets.iter().copied());
                        }
                    }
                }
                // …, RelatedObjects, RelatingType.
                "IFCRELDEFINESBYTYPE" => {
                    if let Some(t) = a.get(5).and_then(P::reference) {
                        for o in ref_list(a, 4) {
                            r.types.insert(o, t);
                        }
                    }
                }
                // …, RelatedObjects, RelatedObjectsType, RelatingGroup.
                "IFCRELASSIGNSTOGROUP" => {
                    let Some((group, ga)) = a.get(6).and_then(|g| self.get(g)) else { continue };
                    if !group.contains("SYSTEM") {
                        continue;
                    }
                    let Some(sys) = ga.get(2).and_then(P::text).filter(|t| !t.is_empty()) else { continue };
                    for o in ref_list(a, 4) {
                        let list = r.systems.entry(o).or_default();
                        if list.len() < 16 && !list.iter().any(|s| s == sys) {
                            list.push(cap(sys, 120));
                        }
                    }
                }
                _ => {}
            }
        }
        // The file's records come in hash order: sets by their record number, systems by name,
        // so the same file always reads the same.
        for v in r.sets.values_mut() {
            v.sort_unstable();
            v.dedup();
        }
        for v in r.systems.values_mut() {
            v.sort();
        }
        r
    }

    /// The storey object `id` is in (through spaces and assemblies): its name and elevation in
    /// metres (its placement's height; else its Elevation attribute).
    fn storey(&self, rel: &Relations, id: u64, unit: f64) -> Option<(String, f64)> {
        let mut at = id;
        for _ in 0..16 {
            at = *rel.parent.get(&at)?;
            let (name, a) = self.m.get(&at)?;
            if name == "IFCBUILDINGSTOREY" {
                let label = a.get(2).and_then(P::text).filter(|t| !t.is_empty()).or_else(|| a.get(7).and_then(P::text)).unwrap_or("Storey");
                let placed = a.get(5).filter(|p| **p != P::Null).map(|p| self.placement(p, 0)[2][3]);
                let z = placed.filter(|z| z.is_finite() && *z != 0.0).or_else(|| a.get(9).and_then(P::num)).or(placed).unwrap_or(0.0) * unit;
                return Some((cap(label, 120), if z.is_finite() { z } else { 0.0 }));
            }
            if name == "IFCBUILDING" || name == "IFCSITE" || name == "IFCPROJECT" {
                return None;
            }
        }
        None
    }

    /// The spatial structure object `id` is in, above its storey: its space, building and site
    /// (each `(tag, name)`; spaces by their name, else their long name).
    fn spatial(&self, rel: &Relations, id: u64) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        let mut at = id;
        for _ in 0..16 {
            let Some(up) = rel.parent.get(&at) else { break };
            at = *up;
            let Some((name, a)) = self.m.get(&at) else { break };
            let tag = match name.as_str() {
                "IFCSPACE" => "space",
                "IFCBUILDING" => "building",
                "IFCSITE" => "site",
                "IFCPROJECT" => break,
                _ => continue,
            };
            if out.iter().any(|(t, _)| *t == tag) {
                continue;
            }
            let label = a.get(2).and_then(P::text).filter(|t| !t.is_empty()).or_else(|| a.get(7).and_then(P::text)).unwrap_or_default();
            if !label.is_empty() {
                out.push((tag, cap(label, 120)));
            }
        }
        out
    }

    /// `prop:Set.Name=value` strings of the property and quantity sets `sets` (lengths in metres).
    fn properties(&self, sets: &[u64], unit: f64, out: &mut Vec<String>) {
        for id in sets {
            let Some((name, a)) = self.m.get(id) else { continue };
            let set = a.get(2).and_then(P::text).unwrap_or_default();
            let items = match name.as_str() {
                "IFCPROPERTYSET" => ref_list(a, 4),
                "IFCELEMENTQUANTITY" => ref_list(a, 5),
                _ => continue,
            };
            for item in items {
                if out.len() >= MAX_PROPS {
                    return;
                }
                let Some((kind, pa)) = self.m.get(&item) else { continue };
                let pname = pa.first().and_then(P::text).unwrap_or_default();
                let value = match kind.as_str() {
                    "IFCPROPERTYSINGLEVALUE" | "IFCPROPERTYENUMERATEDVALUE" | "IFCPROPERTYLISTVALUE" => pa.get(2).and_then(value_text),
                    "IFCQUANTITYLENGTH" => pa.get(3).and_then(P::num).map(|v| num_text(v * unit)),
                    "IFCQUANTITYAREA" | "IFCQUANTITYVOLUME" | "IFCQUANTITYCOUNT" | "IFCQUANTITYWEIGHT" | "IFCQUANTITYTIME" => {
                        pa.get(3).and_then(P::num).map(num_text)
                    }
                    _ => None,
                };
                if let Some(v) = value.filter(|_| !pname.is_empty()) {
                    out.push(cap(&format!("prop:{set}.{pname}={v}"), MAX_TAG));
                }
            }
        }
    }

    /// Object `id`'s properties as an element's own store ([`cadaddy_doc::props`]): what
    /// `Pset_CADaddy` holds as it was written (source, unit, confidence and note from each
    /// property's description), then the standard sets' properties our keys map to, then every
    /// other set's as `Set.Name` (source `spec`, the document naming the set) — one store,
    /// whatever program wrote the file.
    fn own_props(&self, rel: &Relations, id: u64, class: &str) -> cadaddy_doc::props::Props {
        use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
        let kind = props::kind_of_ifc_class(class);
        let mut out = Props::default();
        let mut later: Vec<(String, String, Prop)> = Vec::new();
        let mut sets: Vec<u64> = rel.sets.get(&id).cloned().unwrap_or_default();
        if let Some((_, ta)) = rel.types.get(&id).and_then(|t| self.m.get(t)) {
            sets.extend(ref_list(ta, 5));
        }
        for sid in sets.iter().take(1000) {
            let Some((name, a)) = self.m.get(sid) else { continue };
            if name != "IFCPROPERTYSET" {
                continue;
            }
            let set = a.get(2).and_then(P::text).unwrap_or_default().to_string();
            for item in ref_list(a, 4).into_iter().take(props::MAX_PROPS) {
                let Some((k, pa)) = self.m.get(&item) else { continue };
                if k != "IFCPROPERTYSINGLEVALUE" && k != "IFCPROPERTYENUMERATEDVALUE" {
                    continue;
                }
                let pname = pa.first().and_then(P::text).unwrap_or_default();
                if pname.is_empty() {
                    continue;
                }
                let raw = pa.get(2);
                let value = match raw {
                    Some(P::Typed(t, v))
                        if matches!(t.as_str(), "IFCREAL" | "IFCINTEGER" | "IFCLENGTHMEASURE" | "IFCPOSITIVELENGTHMEASURE" | "IFCAREAMEASURE") =>
                    {
                        v.first().and_then(P::num).and_then(serde_json::Number::from_f64).map(serde_json::Value::Number)
                    }
                    Some(P::Typed(t, v)) if t == "IFCBOOLEAN" => match v.first() {
                        Some(P::Enum(e)) if e == "T" => Some(serde_json::Value::Bool(true)),
                        Some(P::Enum(e)) if e == "F" => Some(serde_json::Value::Bool(false)),
                        _ => None,
                    },
                    Some(P::Null) | None => Some(serde_json::Value::Null),
                    Some(p) => value_text(p).map(serde_json::Value::String),
                }
                .unwrap_or(serde_json::Value::Null);
                if set.eq_ignore_ascii_case(props::IFC_PSET) {
                    let meta: serde_json::Value = pa.get(1).and_then(P::text).and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default();
                    let mut prop = Prop::new(value, Source::from_doc(SourceKind::Spec, &format!("IFC {}", props::IFC_PSET)));
                    if let Some(src) = meta.get("source").and_then(|v| serde_json::from_value::<Source>(v.clone()).ok()) {
                        prop.source = src;
                    }
                    prop.unit = meta.get("unit").and_then(serde_json::Value::as_str).map(str::to_string);
                    prop.confidence = meta.get("confidence").and_then(serde_json::Value::as_f64);
                    prop.note = meta.get("note").and_then(serde_json::Value::as_str).map(str::to_string);
                    out.set(pname, prop);
                } else {
                    let prop = Prop::new(value, Source::from_doc(SourceKind::Spec, &format!("IFC {set}")));
                    let key = props::key_of_standard_ifc(kind, &set, pname).map_or_else(|| format!("{set}.{pname}"), str::to_string);
                    later.push((key, set.clone(), prop));
                }
            }
        }
        for (key, _, prop) in later {
            if prop.has_value() {
                out.set_if_missing(&key, prop);
            }
        }
        out
    }

    /// What object `id` carries beyond its class, GlobalId and name, as tag strings: `level:`,
    /// `elevation:` (metres), `space:`, `building:`, `site:`, `type:`, `system:` and `prop:` ones.
    fn extras(&self, rel: &Relations, id: u64, unit: f64) -> Vec<String> {
        let mut out = Vec::new();
        if let Some((level, z)) = self.storey(rel, id, unit) {
            out.push(format!("level:{level}"));
            out.push(format!("elevation:{}", num_text(z)));
        }
        for (tag, name) in self.spatial(rel, id) {
            out.push(format!("{tag}:{name}"));
        }
        let ty = rel.types.get(&id).and_then(|t| self.m.get(t));
        if let Some(n) = ty.and_then(|(_, a)| a.get(2)).and_then(P::text).filter(|t| !t.is_empty()) {
            out.push(cap(&format!("type:{n}"), MAX_TAG));
        }
        for s in rel.systems.get(&id).into_iter().flatten() {
            out.push(format!("system:{s}"));
        }
        let mut props = Vec::new();
        self.properties(rel.sets.get(&id).map(Vec::as_slice).unwrap_or_default(), unit, &mut props);
        // The type's own sets (IfcTypeObject's HasPropertySets), where the object has none of that name.
        if let Some((_, ta)) = ty {
            let mut from_type = Vec::new();
            self.properties(&ref_list(ta, 5), unit, &mut from_type);
            for p in from_type {
                let key = p.split_once('=').map_or(p.as_str(), |(k, _)| k);
                if props.len() < MAX_PROPS && !props.iter().any(|q| q.split_once('=').is_some_and(|(k, _)| k == key)) {
                    props.push(p);
                }
            }
        }
        out.extend(props);
        out
    }
}

/// Spatial structure and openings: not objects of their own.
const NOT_OBJECTS: [&str; 6] = ["IFCPROJECT", "IFCSITE", "IFCBUILDING", "IFCBUILDINGSTOREY", "IFCOPENINGELEMENT", "IFCANNOTATION"];

/// `IFCWALLSTANDARDCASE` → `WallStandardCase`, as the IFC schema writes it.
fn class_name(upper: &str) -> String {
    const KNOWN: &[&str] = &[
        "WallStandardCase",
        "Wall",
        "Slab",
        "Space",
        "Door",
        "Window",
        "Covering",
        "Roof",
        "Stair",
        "StairFlight",
        "Railing",
        "Column",
        "Beam",
        "Member",
        "Plate",
        "CurtainWall",
        "Footing",
        "Pile",
        "Ramp",
        "BuildingElementProxy",
        "FurnishingElement",
        "FlowTerminal",
        "FlowSegment",
        "DistributionElement",
        "DuctSegment",
        "PipeSegment",
        "CableCarrierSegment",
        "CableSegment",
        "GeographicElement",
        "DuctFitting",
        "PipeFitting",
        "CableCarrierFitting",
        "CableFitting",
        "AirTerminal",
        "FireSuppressionTerminal",
        "UnitaryEquipment",
        "AirTerminalBox",
        "Fan",
        "Pump",
        "Boiler",
        "Condenser",
        "FlowMeter",
        // IFC4's other products, as the schema spells them (COBie types and schedules show them).
        "SanitaryTerminal",
        "WasteTerminal",
        "StackTerminal",
        "LightFixture",
        "Lamp",
        "Furniture",
        "SystemFurnitureElement",
        "ElectricAppliance",
        "ElectricDistributionBoard",
        "ElectricMotor",
        "ElectricGenerator",
        "ElectricFlowStorageDevice",
        "ElectricTimeControl",
        "Outlet",
        "SwitchingDevice",
        "ProtectiveDevice",
        "ProtectiveDeviceTrippingUnit",
        "Transformer",
        "CableSegment",
        "CableFitting",
        "JunctionBox",
        "AudioVisualAppliance",
        "CommunicationsAppliance",
        "Alarm",
        "Sensor",
        "Actuator",
        "Controller",
        "UnitaryControlElement",
        "FlowInstrument",
        "Valve",
        "Damper",
        "Tank",
        "Chiller",
        "CoolingTower",
        "Coil",
        "Compressor",
        "HeatExchanger",
        "Humidifier",
        "EvaporativeCooler",
        "Evaporator",
        "Filter",
        "Interceptor",
        "SpaceHeater",
        "Burner",
        "Engine",
        "MedicalDevice",
        "AirToAirHeatRecovery",
        "DuctSilencer",
        "FlowController",
        "FlowFitting",
        "FlowMovingDevice",
        "FlowStorageDevice",
        "FlowTreatmentDevice",
        "EnergyConversionDevice",
        "DistributionChamberElement",
        "ShadingDevice",
        "Chimney",
        "BuildingElementPart",
        "TransportElement",
        "VirtualElement",
    ];
    let bare = upper.strip_prefix("IFC").unwrap_or(upper);
    KNOWN.iter().find(|k| k.eq_ignore_ascii_case(bare)).map_or_else(
        || {
            let mut s = bare.to_ascii_lowercase();
            if let Some(f) = s.get_mut(..1) {
                f.make_ascii_uppercase();
            }
            s
        },
        |k| (*k).to_string(),
    )
}

/// A mesh of `faces` with equal points welded.
fn mesh_of(faces: &[Vec<Vec3>]) -> Option<Mesh> {
    let mut index: HashMap<[u64; 3], u32> = HashMap::new();
    let mut m = Mesh { kind: MeshKind::Polyface, ..Mesh::default() };
    for f in faces {
        let mut v = Vec::with_capacity(f.len());
        for p in f {
            if !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite()) {
                continue;
            }
            let key = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
            let next = u32::try_from(m.vertices.len()).ok()?;
            let i = *index.entry(key).or_insert_with(|| {
                m.vertices.push(*p);
                next
            });
            if v.last() != Some(&i) {
                v.push(i);
            }
        }
        if v.len() > 2 && v.first() == v.last() {
            v.pop();
        }
        if v.len() >= 3 {
            m.faces.push(MeshFace { v, hidden: 0 });
        }
    }
    (!m.faces.is_empty()).then_some(m)
}

/// An IFC file as a drawing: a mesh per product with a body, on a layer per class, in metres.
pub fn read_ifc(bytes: &[u8]) -> Result<Drawing> {
    let model = parse(bytes)?;
    let r = Reader { m: &model };
    let unit = r.unit();
    let scale: M = [[unit, 0.0, 0.0, 0.0], [0.0, unit, 0.0, 0.0], [0.0, 0.0, unit, 0.0]];
    let mut d = Drawing::new_metric();
    d.header.set_i64("INSUNITS", 6);
    let rel = r.relations();
    let mut ids: Vec<&u64> = model.keys().collect();
    ids.sort_unstable();
    let mut total_faces = 0usize;
    let mut n = 0usize;
    for id in ids {
        let Some((name, a)) = model.get(id) else { continue };
        if NOT_OBJECTS.contains(&name.as_str()) || a.len() < 7 {
            continue;
        }
        // An IfcProduct: GlobalId, OwnerHistory, Name, Description, ObjectType, ObjectPlacement, Representation…
        let Some((pds_name, pds)) = a.get(6).and_then(|x| r.get(x)) else { continue };
        if pds_name != "IFCPRODUCTDEFINITIONSHAPE" {
            continue;
        }
        let place = mul(&scale, &a.get(5).map_or(ID, |x| r.placement(x, 0)));
        let mut faces = Vec::new();
        for rep in pds.get(2).map(P::list).unwrap_or_default() {
            let Some((_, ra)) = r.get(rep) else { continue };
            // The body; an axis, box or footprint representation would draw a second outline.
            let id = ra.get(1).and_then(P::text).unwrap_or_default();
            if !id.is_empty() && !id.eq_ignore_ascii_case("Body") && !id.eq_ignore_ascii_case("Facetation") {
                continue;
            }
            for it in ra.get(3).map(P::list).unwrap_or_default() {
                r.item(it, &place, 0, &mut faces);
            }
        }
        total_faces = total_faces.saturating_add(faces.len());
        if total_faces > MAX_FACES {
            return Err(IoError::Format(format!("more than {MAX_FACES} faces")));
        }
        let Some(mesh) = mesh_of(&faces) else { continue };
        if mesh.vertices.len() > MAX_POINTS {
            continue;
        }
        let class = class_name(name);
        let layer = format!("IFC-{class}");
        if d.layer(&layer).is_none() {
            d.layers.push(Layer { name: layer.clone(), ..Layer::default() });
        }
        let guid = a.first().and_then(P::text).unwrap_or_default();
        let label = a.get(2).and_then(P::text).unwrap_or_default();
        let mut xdata = vec![
            RawTag { code: 1001, value: APP.into() },
            RawTag { code: 1000, value: class.clone() },
            RawTag { code: 1000, value: guid.chars().take(64).collect() },
            RawTag { code: 1000, value: cap(label, MAX_TAG) },
        ];
        xdata.extend(r.extras(&rel, *id, unit).into_iter().map(|value| RawTag { code: 1000, value }));
        let own = r.own_props(&rel, *id, &class);
        if !own.is_empty() {
            xdata = cadaddy_doc::props::write(&xdata, &own);
        }
        let mut c = Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() };
        if class == "Space" {
            c.transparency = cadaddy_doc::Transparency::Percent(80);
        }
        d.add(&Space::Model, c, EntityKind::Mesh(mesh)).map_err(|e| IoError::Format(e.to_string()))?;
        n += 1;
    }
    if n == 0 {
        return Err(IoError::Format("no building elements with a body in the IFC file".into()));
    }
    Ok(d)
}

/// What an object is as a building element: the IFC class it is written as (`IfcDuctSegment`,
/// `IfcBuildingElementProxy` for plain linework), its storey, its name (a run's system, a piece of
/// equipment's tag, a room's name, else its layer) and the systems it belongs to (4D schedules
/// link tasks to elements by these).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElementInfo {
    pub class: String,
    pub level: Option<String>,
    pub name: String,
    pub systems: Vec<String>,
}

pub fn element_info(e: &Entity) -> ElementInfo {
    let k = class_of(e);
    let mut systems = k.systems;
    if let Some(s) = k.system_name
        && !systems.contains(&s)
    {
        systems.insert(0, s);
    }
    ElementInfo { class: k.class.ifc().0.to_string(), level: k.level, name: k.name, systems }
}

/// The IFC class and GlobalId of an object read from IFC.
pub fn ifc_tag(e: &Entity) -> Option<(String, String)> {
    let t = tag_strings(e, APP)?;
    Some((t.first()?.clone(), t.get(1).cloned().unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_and_reals_are_valid_step() {
        assert_eq!(step_str("It's a \\ test \u{e9}"), "'It''s a \\\\ test \\X2\\00E9\\X0\\'");
        assert_eq!(decode_step_string(b"It's a \\\\ test \\X2\\00E9\\X0\\"), "It's a \\ test \u{e9}");
        assert_eq!(real(1.0), "1.");
        assert_eq!(real(-0.0), "0.");
        assert_eq!(real(2.5), "2.5");
        // Above U+FFFF: \X4\ (ISO 10303-21), not UTF-16 surrogates in \X2\.
        assert_eq!(step_str("Room \u{1F3E0}"), "'Room \\X4\\0001F3E0\\X0\\'");
        assert_eq!(decode_step_string(b"Room \\X4\\0001F3E0\\X0\\"), "Room \u{1F3E0}");
        // \S\ shifts the next character into ISO 8859's upper half; \PA\ picks a code page.
        assert_eq!(decode_step_string(b"Caf\\S\\i \\PA\\ok"), "Caf\u{e9} ok");
        assert!(is_guid(&guid(7, 9)) && !is_guid("4zzzzzzzzzzzzzzzzzzzzz") && !is_guid("short"));
        assert_eq!(guid(1, 2).len(), 22);
        assert_eq!(guid(1, 2), guid(1, 2));
        assert_ne!(guid(1, 2), guid(1, 3));
    }

    /// A hand-written IFC: an extruded rectangle wall in millimetres, placed through a chain, and
    /// a triangulated slab.
    #[test]
    fn extrusions_face_sets_placements_and_units_are_read() {
        let ifc = b"ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n\
            #1=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n#2=IFCUNITASSIGNMENT((#1));\n\
            #3=IFCPROJECT('p',$,'P',$,$,$,$,$,#2);\n\
            #10=IFCCARTESIANPOINT((1000.,2000.,0.));\n#11=IFCAXIS2PLACEMENT3D(#10,$,$);\n#12=IFCLOCALPLACEMENT($,#11);\n\
            #13=IFCCARTESIANPOINT((0.,0.,3000.));\n#14=IFCAXIS2PLACEMENT3D(#13,$,$);\n#15=IFCLOCALPLACEMENT(#12,#14);\n\
            #20=IFCCARTESIANPOINT((0.,0.));\n#21=IFCAXIS2PLACEMENT2D(#20,$);\n\
            #22=IFCRECTANGLEPROFILEDEF(.AREA.,$,#21,4000.,200.);\n#23=IFCDIRECTION((0.,0.,1.));\n\
            #24=IFCEXTRUDEDAREASOLID(#22,$,#23,2500.);\n#25=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#24));\n\
            #26=IFCPRODUCTDEFINITIONSHAPE($,$,(#25));\n#27=IFCWALL('w1',$,'Wall /* not a comment */',$,$,#15,#26,$,$);\n\
            /* a comment */\n\
            #30=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1000.,0.,0.),(0.,1000.,0.)));\n\
            #31=IFCTRIANGULATEDFACESET(#30,$,.T.,((1,2,3)),$);\n#32=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#31));\n\
            #33=IFCPRODUCTDEFINITIONSHAPE($,$,(#32));\n#34=IFCSLAB('s1',$,'Slab',$,$,$,#33,$,.FLOOR.);\n\
            ENDSEC;\nEND-ISO-10303-21;\n";
        let d = read_ifc(ifc).unwrap();
        let walls: Vec<&Entity> = d.model.iter().filter(|e| e.common.layer == "IFC-Wall").map(|e| e.as_ref()).collect();
        assert_eq!(walls.len(), 1);
        let EntityKind::Mesh(m) = &walls[0].kind else { panic!("a mesh") };
        assert_eq!(m.vertices.len(), 8);
        assert_eq!(m.faces.len(), 6);
        let (lo, hi) = m.vertices.iter().fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), v| {
            (Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z)), Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z)))
        });
        // Metres: x 1±2, y 2±0.1, z 3..5.5.
        assert!((lo.x + 1.0).abs() < 1e-9 && (hi.x - 3.0).abs() < 1e-9, "{lo:?} {hi:?}");
        assert!((lo.y - 1.9).abs() < 1e-9 && (hi.z - 5.5).abs() < 1e-9, "{lo:?} {hi:?}");
        assert_eq!(ifc_tag(walls[0]), Some(("Wall".into(), "w1".into())));
        assert_eq!(d.model.iter().filter(|e| e.common.layer == "IFC-Slab").count(), 1);
    }

    /// Storeys (through a space and directly), property and quantity sets, a type with its own
    /// set, and a system, carried on each object's tag; written back with the same GlobalIds.
    #[test]
    fn storeys_properties_types_and_systems_are_read_and_ids_round_trip() {
        let ifc = b"ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC2X3'));\nENDSEC;\nDATA;\n\
            #1=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n#2=IFCUNITASSIGNMENT((#1));\n\
            #3=IFCPROJECT('0Prj000000000000000001',$,'P',$,$,$,$,$,#2);\n\
            #4=IFCBUILDING('0Bld000000000000000001',$,'B',$,$,$,$,$,.ELEMENT.,$,$,$);\n\
            #5=IFCCARTESIANPOINT((0.,0.,3500.));\n#6=IFCAXIS2PLACEMENT3D(#5,$,$);\n#7=IFCLOCALPLACEMENT($,#6);\n\
            #8=IFCBUILDINGSTOREY('0Sty000000000000000002',$,'Level 2',$,$,#7,$,$,.ELEMENT.,3500.);\n\
            #9=IFCBUILDINGSTOREY('0Sty000000000000000001',$,'Level 1',$,$,$,$,$,.ELEMENT.,0.);\n\
            #10=IFCRELAGGREGATES('0Rel000000000000000001',$,$,$,#4,(#9,#8));\n\
            #20=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1000.,0.,0.),(0.,1000.,0.)));\n\
            #21=IFCTRIANGULATEDFACESET(#20,$,.T.,((1,2,3)),$);\n#22=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#21));\n\
            #23=IFCPRODUCTDEFINITIONSHAPE($,$,(#22));\n\
            #24=IFCWALLSTANDARDCASE('1Wal000000000000000001',$,'Wall A',$,$,#7,#23,$);\n\
            #25=IFCSPACE('1Spc000000000000000001',$,'101',$,$,$,#23,'Office',.ELEMENT.,.INTERNAL.,$);\n\
            #26=IFCDUCTSEGMENT('1Dct000000000000000001',$,'Duct',$,$,$,#23,$,.RIGIDSEGMENT.);\n\
            #27=IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000002',$,$,$,(#24),#8);\n\
            #28=IFCRELAGGREGATES('0Rel000000000000000003',$,$,$,#9,(#25));\n\
            #29=IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000004',$,$,$,(#26),#25);\n\
            #30=IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('2HR'),$);\n\
            #31=IFCPROPERTYSINGLEVALUE('IsExternal',$,IFCBOOLEAN(.F.),$);\n\
            #32=IFCPROPERTYSET('0Pst000000000000000001',$,'Pset_WallCommon',$,(#30,#31));\n\
            #33=IFCQUANTITYLENGTH('Height',$,$,3000.,$);\n\
            #34=IFCELEMENTQUANTITY('0Qto000000000000000001',$,'Qto_WallBaseQuantities',$,$,(#33));\n\
            #35=IFCRELDEFINESBYPROPERTIES('0Rel000000000000000005',$,$,$,(#24),#32);\n\
            #36=IFCRELDEFINESBYPROPERTIES('0Rel000000000000000006',$,$,$,(#24),#34);\n\
            #37=IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('1HR'),$);\n\
            #38=IFCPROPERTYSINGLEVALUE('AcousticRating',$,IFCLABEL('STC 50'),$);\n\
            #39=IFCPROPERTYSET('0Pst000000000000000002',$,'Pset_WallCommon',$,(#37,#38));\n\
            #40=IFCWALLTYPE('0Typ000000000000000001',$,'Partition 200',$,$,(#39),$,$,$,.STANDARD.);\n\
            #41=IFCRELDEFINESBYTYPE('0Rel000000000000000007',$,$,$,(#24),#40);\n\
            #42=IFCDISTRIBUTIONSYSTEM('0Sys000000000000000001',$,'Supply Air',$,$,$,.SUPPLYAIR.);\n\
            #43=IFCRELASSIGNSTOGROUP('0Rel000000000000000008',$,$,$,(#26),$,#42);\n\
            ENDSEC;\nEND-ISO-10303-21;\n";
        let d = read_ifc(ifc).unwrap();
        let tags = |class: &str| -> Vec<String> {
            let e = d.model.iter().find(|e| ifc_tag(e).is_some_and(|(c, _)| c == class)).unwrap();
            tag_strings(e, APP).unwrap()
        };
        let wall = tags("WallStandardCase");
        assert_eq!(wall.get(..3), Some(&["WallStandardCase".to_string(), "1Wal000000000000000001".into(), "Wall A".into()][..]));
        for t in
            ["level:Level 2", "elevation:3.5", "type:Partition 200", "prop:Pset_WallCommon.FireRating=2HR", "prop:Pset_WallCommon.IsExternal=FALSE"]
        {
            assert!(wall.iter().any(|x| x == t), "{t} in {wall:?}");
        }
        // Quantity lengths in metres; the type's set fills in only what the object lacks.
        assert!(wall.iter().any(|x| x == "prop:Qto_WallBaseQuantities.Height=3"), "{wall:?}");
        assert!(wall.iter().any(|x| x == "prop:Pset_WallCommon.AcousticRating=STC 50"), "{wall:?}");
        assert!(!wall.iter().any(|x| x == "prop:Pset_WallCommon.FireRating=1HR"), "{wall:?}");
        // The space sits on Level 1 (aggregated), the duct in the space: Level 1 too, in its system.
        assert!(tags("Space").iter().any(|x| x == "level:Level 1"));
        let duct = tags("DuctSegment");
        assert!(duct.iter().any(|x| x == "level:Level 1") && duct.iter().any(|x| x == "system:Supply Air"), "{duct:?}");
        // The spatial chain above the storey: the duct's space and building, the wall's building.
        assert!(duct.iter().any(|x| x == "space:101") && duct.iter().any(|x| x == "building:B"), "{duct:?}");
        assert!(wall.iter().any(|x| x == "building:B") && !wall.iter().any(|x| x.starts_with("space:")), "{wall:?}");
        // Written back, the objects keep the GlobalIds they were read with.
        let out = String::from_utf8(write_ifc(&d, "Round Trip.ifc").unwrap()).unwrap();
        for g in ["1Wal000000000000000001", "1Spc000000000000000001", "1Dct000000000000000001"] {
            assert!(out.contains(&format!("('{g}'")), "{g}");
        }
        assert_eq!(read_ifc(out.as_bytes()).unwrap().model.len(), 3);
    }

    #[test]
    fn garbage_and_hostile_files_are_refused() {
        for bad in [&b""[..], b"hello", b"ISO-10303-21;\nDATA;\nENDSEC;\n", b"ISO-10303-21;\nDATA;\n#1=IFCWALL((((((((("] {
            assert!(read_ifc(bad).is_err());
        }
        // A placement that refers to itself ends.
        let ifc = b"ISO-10303-21;\nDATA;\n#1=IFCLOCALPLACEMENT(#1,$);\n#2=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.,0.),(0.,1.,0.)));\n\
            #3=IFCTRIANGULATEDFACESET(#2,$,.T.,((1,2,3)),$);\n#4=IFCSHAPEREPRESENTATION($,'Body','T',(#3));\n\
            #5=IFCPRODUCTDEFINITIONSHAPE($,$,(#4));\n#6=IFCWALL('w',$,$,$,$,#1,#5,$,$);\nENDSEC;\n";
        assert_eq!(read_ifc(ifc).unwrap().model.len(), 1);
        // A mapped item that maps itself ends too.
        let ifc = b"ISO-10303-21;\nDATA;\n#1=IFCREPRESENTATIONMAP($,#2);\n#2=IFCSHAPEREPRESENTATION($,'Body','M',(#3));\n\
            #3=IFCMAPPEDITEM(#1,$);\n#5=IFCPRODUCTDEFINITIONSHAPE($,$,(#2));\n#6=IFCWALL('w',$,$,$,$,$,#5,$,$);\nENDSEC;\n";
        assert!(read_ifc(ifc).is_err(), "no faces");
    }

    #[test]
    fn four_d_elements_are_classed_as_ifc_export_writes_them() {
        // 4D (`cadaddy-walk3d`'s `fourd::element_of`) keeps its own copy of this table; they agree.
        let wg = |strings: &[&str]| {
            let mut x = vec![RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }];
            x.extend(strings.iter().map(|v| RawTag { code: 1000, value: (*v).into() }));
            x
        };
        let mut cases: Vec<Vec<RawTag>> = Vec::new();
        for part in [
            "wall",
            "parapet",
            "floor",
            "ceiling",
            "soffit",
            "door",
            "window",
            "stair",
            "column",
            "curtainwall",
            "railing",
            "ramp",
            "roof",
            "ground",
            "beam?",
        ] {
            cases.push(wg(&[part, "Level 1", "2A"]));
        }
        for kind in [
            "duct",
            "pipe",
            "conduit",
            "tray",
            "light_fixture",
            "outlet",
            "receptacle",
            "data_outlet",
            "floor_box",
            "switch",
            "disconnect",
            "junction_box",
            "panel",
            "transformer",
            "other",
        ] {
            for part in ["part:segment", "part:fitting:elbow", "part:terminal:diffuser", "part:terminal:sprinkler_pendent"] {
                cases.push(wg(&["mep", "Level 2", "0", "system:Supply A", &format!("kind:{kind}"), part]));
            }
        }
        for ek in ["ahu", "rtu", "vav", "exhaust_fan", "fan", "pump", "water_heater", "condensing_unit", "gas_meter", "mystery"] {
            cases.push(wg(&[
                "mep",
                "Roof",
                "0",
                "kind:equipment",
                &format!("part:equipment:{ek}"),
                "tag:EQ-1",
                "system:Supply A",
                "system:Return A",
            ]));
        }
        cases.push(vec![
            RawTag { code: 1001, value: "CADADDY_ROOM".into() },
            RawTag { code: 1000, value: "Office".into() },
            RawTag { code: 1000, value: "101".into() },
        ]);
        cases.push(Vec::new());
        for xdata in cases {
            let e = Entity {
                handle: cadaddy_doc::Handle(0x2a),
                common: Common { layer: "M-DUCT".into(), xdata: xdata.into(), ..Common::default() },
                kind: EntityKind::Mesh(Mesh::default()),
            };
            let (a, b) = (element_info(&e), cadaddy_walk3d::fourd::element_of(&e));
            assert_eq!((a.class.as_str(), a.name.as_str(), &a.systems), (b.class.as_str(), b.name.as_str(), &b.systems), "{:?}", e.common.xdata);
            assert_eq!(a.level.unwrap_or_default(), b.level, "{:?}", e.common.xdata);
        }
    }
}
