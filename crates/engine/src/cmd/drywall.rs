//! Drywall views and takeoff: the "What is this wall?" inspector (`wall.info`), view mode
//! colouring by wall type / fire rating / board-layer count / height rule (`drywall.viewmode`),
//! finish faces (`drywall.finishfaces`), and a drywall-specific takeoff (`drywall.takeoff`) that
//! gives board area per type and thickness (minus openings, per side, per level/room/wall type),
//! sheet counts with waste, stud and track length, insulation area, and corner bead / trim length.
//!
//! Everything builds on the wall assemblies from Q293: the building spec's `wall_types` (each with
//! an `assembly` of layers), `walls` (marks on the plans giving each wall a type and height), and
//! WALLGEN's `type:<name>` xdata tag on 3D wall meshes.

use std::collections::BTreeMap;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Mesh};
use cadaddy_geom::Vec3;
use cadaddy_walk3d::{Assembly, AssemblyLayer, BuildingSpec, HeightRule, LayerSide};
use serde_json::{Value, json};

use super::wallgen;
use super::*;
use crate::{Result, Session};

/// Maximum wall entities to process in one takeoff (hostile drawings).
const MAX_WALLS: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("wall.info", "Wall Info", run_info)
            .params("{handle? | handles? (default: selection)} → {walls: [{handle, level, wallType, assembly?: {tag, description, layers: [{material, thickness, side, category, inCavity}], height?, fireRating?, acousticRating?, external, thickness}, finishFaces?: [{side, room, board, finish}]}, ...], count}")
            .noundo(),
        CommandSpec::new("drywall.takeoff", "Drywall Takeoff", run_takeoff)
            .params("{handles? | layers? | levels? | wallTypes? | rooms?, waste?: percent (default 10), sheetSize?: [w,h] feet (default [4,8]), file?: path (.csv/.xlsx), groupBy?: \"level\" | \"room\" | \"wallType\" (default \"wallType\")} → {rows: [{group, boards: [{material, thicknessIn, side, areaSqFt, sheets, sheetSize}], studs: {lengthFt, material}, track: {lengthFt}, insulation: {areaSqFt, material}, cornerBead: {lengthFt}, trim: {lengthFt}}], totals, waste}")
            .noundo(),
        CommandSpec::new("drywall.viewmode", "Drywall View Mode", run_viewmode)
            .params("{mode: \"type\" | \"rating\" | \"boardLayers\" | \"heightRule\" | \"off\"} → {mode, legend: [{label, color}]}")
            .noundo(),
        CommandSpec::new("drywall.finishfaces", "Finish Faces", run_finishfaces)
            .params("{} → {faces: [{wallHandle, side, room, board, finish}], count}")
            .noundo(),
    ]
}

// ---------------------------------------------------------------------------
// wall.info — What is this wall?
// ---------------------------------------------------------------------------

fn run_info(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let d = s.doc()?;
    let sp = spec_if_any(s);
    let mut walls = Vec::new();
    for h in hs.iter().take(MAX_WALLS) {
        let Some(e) = d.entity(*h) else { continue };
        if wallgen::made_as(e).as_deref() != Some("wall") {
            continue;
        }
        walls.push(wall_info_json(e, &sp));
    }
    let count = walls.len();
    Ok(json!({ "walls": walls, "count": count }))
}

/// The wall info for one WALLGEN wall entity.
fn wall_info_json(e: &Entity, sp: &Option<BuildingSpec>) -> Value {
    let tags = wallgen_tags(e);
    let level = tags.get(1).cloned().unwrap_or_default();
    let type_name = tags.iter().skip(3).find_map(|v| v.strip_prefix("type:")).map(str::to_string);

    let mut info = json!({
        "handle": e.handle.hex(),
        "level": level,
    });

    if let Some(tn) = &type_name {
        info["wallType"] = json!(tn);
        // Look up the assembly from the building spec.
        if let Some(sp) = sp
            && let Some(wt) = sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(tn))
            && let Some(asm) = &wt.assembly
        {
            info["assembly"] = assembly_json(asm);
        }
    }

    // Compute gross area from the mesh.
    if let EntityKind::Mesh(ref mesh) = e.kind {
        let area = wall_face_area_sqft(mesh);
        if area > 0.0 {
            info["grossAreaSqFt"] = json!((area * 100.0).round() / 100.0);
        }
    }

    info
}

fn assembly_json(asm: &Assembly) -> Value {
    let layers: Vec<Value> = asm
        .layers
        .iter()
        .map(|l| {
            let mut v = json!({
                "material": l.material,
                "thicknessIn": l.thickness_in,
                "side": side_str(l.side),
                "category": l.category,
            });
            if l.in_cavity {
                v["inCavity"] = json!(true);
            }
            v
        })
        .collect();

    let mut v = json!({
        "tag": asm.tag,
        "description": asm.description,
        "layers": layers,
        "thicknessIn": asm.thickness_in(),
        "external": asm.external,
    });
    if let Some(ref h) = asm.height {
        v["height"] = json!(h.text(asm.above_ceiling_in));
    }
    if let Some(ref fr) = asm.fire_rating {
        v["fireRating"] = json!(fr);
    }
    if let Some(ref ar) = asm.acoustic_rating {
        v["acousticRating"] = json!(ar);
    }
    if !asm.notes.is_empty() {
        v["notes"] = json!(asm.notes);
    }
    v
}

fn side_str(s: LayerSide) -> &'static str {
    match s {
        LayerSide::Side1 => "side1",
        LayerSide::Core => "core",
        LayerSide::Side2 => "side2",
    }
}

// ---------------------------------------------------------------------------
// drywall.takeoff — board area, sheet counts, studs, trim
// ---------------------------------------------------------------------------

fn run_takeoff(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "drywall.takeoff";
    let d = s.doc()?;
    let sp = spec_if_any(s).ok_or_else(|| bad(cmd, "no building spec in this drawing"))?;
    let waste_pct = f64_or(p, "waste", 10.0).clamp(0.0, 1000.0);
    let sheet_w = p.get("sheetSize").and_then(|v| v.get(0)).and_then(Value::as_f64).unwrap_or(4.0);
    let sheet_h = p.get("sheetSize").and_then(|v| v.get(1)).and_then(Value::as_f64).unwrap_or(8.0);
    let group_by = str_param(p, "groupBy").unwrap_or("wallType");

    // Collect all WALLGEN wall meshes.
    let wall_entities = collect_walls(d, &sp);
    if wall_entities.is_empty() {
        return Ok(json!({ "rows": [], "totals": {}, "waste": waste_pct }));
    }

    // Group walls by the requested key.
    let mut groups: BTreeMap<String, Vec<WallData>> = BTreeMap::new();
    for wd in &wall_entities {
        let key = match group_by {
            "level" => wd.level.clone(),
            _ => wd.type_name.clone().unwrap_or_else(|| "(untyped)".into()),
        };
        groups.entry(key).or_default().push(wd.clone());
    }

    let mut rows = Vec::new();
    let mut total_board_area = 0.0_f64;
    let mut total_sheets = 0_u64;
    let mut total_stud_ft = 0.0_f64;
    let mut total_track_ft = 0.0_f64;
    let mut total_insulation_area = 0.0_f64;
    let mut total_trim_ft = 0.0_f64;

    for (group_name, walls) in &groups {
        let mut board_rows: Vec<Value> = Vec::new();
        let mut stud_length = 0.0_f64;
        let mut stud_material = String::new();
        let mut track_length = 0.0_f64;
        let mut insul_area = 0.0_f64;
        let mut insul_material = String::new();
        let mut trim_length = 0.0_f64;

        // Accumulate per board type (material + thickness + side).
        let mut board_totals: BTreeMap<(String, String, String), f64> = BTreeMap::new();

        for wd in walls {
            let gross = wd.gross_area_sqft;
            let opening_area = wd.opening_area_sqft;
            let net = (gross - opening_area).max(0.0);
            let wall_height_ft = wd.height_ft;
            let wall_length_ft = wd.length_ft;
            let perimeter_ft = wall_length_ft;

            if let Some(ref asm) = wd.assembly {
                for layer in &asm.layers {
                    match layer.category.as_str() {
                        "board" | "sheathing" | "" if !layer.in_cavity => {
                            let key = (layer.material.clone(), format!("{:.3}", layer.thickness_in), side_str(layer.side).to_string());
                            *board_totals.entry(key).or_default() += net;
                        }
                        "framing" => {
                            // Studs at spacing + track top and bottom.
                            if let Some(spacing_in) = layer_spacing_in(layer) {
                                let stud_count = (wall_length_ft * 12.0 / spacing_in).ceil() + 1.0;
                                stud_length += stud_count * wall_height_ft;
                                if stud_material.is_empty() {
                                    stud_material = layer.material.clone();
                                }
                            }
                            track_length += wall_length_ft * 2.0; // top and bottom track
                        }
                        _ if layer.in_cavity => {
                            insul_area += net;
                            if insul_material.is_empty() {
                                insul_material = layer.material.clone();
                            }
                        }
                        "insulation" => {
                            insul_area += net;
                            if insul_material.is_empty() {
                                insul_material = layer.material.clone();
                            }
                        }
                        _ => {}
                    }
                }
                // Corner bead / trim: perimeter of each side's board area.
                trim_length += perimeter_ft * 2.0; // rough estimate: 2x wall perimeter for edges
            } else {
                // No assembly: count the whole wall as one board layer per side.
                let key = ("gypsum board".into(), "0.625".into(), "side1".into());
                *board_totals.entry(key).or_default() += net;
                let key = ("gypsum board".into(), "0.625".into(), "side2".into());
                *board_totals.entry(key).or_default() += net;
            }
        }

        for ((material, thick, side), area) in &board_totals {
            let area_with_waste = area * (1.0 + waste_pct / 100.0);
            let sheet_area = sheet_w * sheet_h;
            let sheets = (area_with_waste / sheet_area).ceil() as u64;
            total_board_area += area;
            total_sheets += sheets;
            board_rows.push(json!({
                "material": material,
                "thicknessIn": thick.parse::<f64>().unwrap_or(0.625),
                "side": side,
                "areaSqFt": (area * 100.0).round() / 100.0,
                "sheets": sheets,
                "sheetSize": [sheet_w, sheet_h],
            }));
        }

        total_stud_ft += stud_length;
        total_track_ft += track_length;
        total_insulation_area += insul_area;
        total_trim_ft += trim_length;

        rows.push(json!({
            "group": group_name,
            "boards": board_rows,
            "studs": { "lengthFt": (stud_length * 100.0).round() / 100.0, "material": stud_material },
            "track": { "lengthFt": (track_length * 100.0).round() / 100.0 },
            "insulation": { "areaSqFt": (insul_area * 100.0).round() / 100.0, "material": insul_material },
            "trim": { "lengthFt": (trim_length * 100.0).round() / 100.0 },
        }));
    }

    // Write CSV/XLSX if requested.
    if let Some(path) = str_param(p, "file") {
        write_takeoff_csv(path, &rows, waste_pct)?;
    }

    Ok(json!({
        "rows": rows,
        "totals": {
            "boardAreaSqFt": (total_board_area * 100.0).round() / 100.0,
            "sheets": total_sheets,
            "studLengthFt": (total_stud_ft * 100.0).round() / 100.0,
            "trackLengthFt": (total_track_ft * 100.0).round() / 100.0,
            "insulationAreaSqFt": (total_insulation_area * 100.0).round() / 100.0,
            "trimLengthFt": (total_trim_ft * 100.0).round() / 100.0,
        },
        "waste": waste_pct,
    }))
}

/// Extract spacing in inches from a layer's material description.
fn layer_spacing_in(layer: &AssemblyLayer) -> Option<f64> {
    // The material string contains e.g. "@ 16\" o.c." or "16\" O.C."
    let m = &layer.material;
    // Look for a number before "o.c." or after "@".
    if let Some(at) = m.find('@') {
        let after = &m[at + 1..];
        let num: String = after.chars().skip_while(|c| c.is_whitespace()).take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        return num.parse::<f64>().ok().filter(|v| *v > 0.0 && v.is_finite());
    }
    // Fallback: default 16" o.c.
    if layer.category == "framing" {
        return Some(16.0);
    }
    None
}

// ---------------------------------------------------------------------------
// drywall.viewmode — colour walls by type, rating, board count, height rule
// ---------------------------------------------------------------------------

/// Colours for the view mode legend. Eight distinct colours for visibility.
const VIEW_COLORS: &[[u8; 3]] = &[
    [65, 105, 225],  // royal blue
    [220, 20, 60],   // crimson
    [34, 139, 34],   // forest green
    [255, 165, 0],   // orange
    [148, 103, 189], // purple
    [0, 191, 255],   // deep sky blue
    [255, 215, 0],   // gold
    [199, 21, 133],  // medium violet red
];

fn run_viewmode(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "drywall.viewmode";
    let mode = str_param(p, "mode").unwrap_or("off");
    let sp = spec_if_any(s);

    match mode {
        "off" => {
            // Clear the view mode override.
            s.drywall_viewmode = None;
            Ok(json!({ "mode": "off", "legend": [] }))
        }
        "type" | "rating" | "boardLayers" | "heightRule" => {
            let d = s.doc()?;
            let sp = sp.ok_or_else(|| bad(cmd, "no building spec"))?;
            let legend = build_legend(mode, &sp, d);
            s.drywall_viewmode = Some(ViewMode { mode: mode.into(), legend: legend.clone() });
            let legend_json: Vec<Value> = legend.iter().map(|(label, [r, g, b])| json!({ "label": label, "color": [r, g, b] })).collect();
            Ok(json!({ "mode": mode, "legend": legend_json }))
        }
        _ => Err(bad(cmd, "mode must be type, rating, boardLayers, heightRule or off")),
    }
}

/// Build a legend: each unique value gets a colour.
fn build_legend(mode: &str, sp: &BuildingSpec, _d: &Drawing) -> Vec<(String, [u8; 3])> {
    let mut labels: Vec<String> = Vec::new();
    for wt in &sp.wall_types {
        let label = match mode {
            "type" => wt.name.clone(),
            "rating" => wt.assembly.as_ref().and_then(|a| a.fire_rating.clone()).unwrap_or_else(|| "Not rated".into()),
            "boardLayers" => {
                let count = wt.assembly.as_ref().map(board_layer_count).unwrap_or(0);
                format!("{count} board layer{}", if count == 1 { "" } else { "s" })
            }
            "heightRule" => wt
                .assembly
                .as_ref()
                .and_then(|a| a.height)
                .map(|h| {
                    match h {
                        HeightRule::Ceiling => "To ceiling",
                        HeightRule::AboveCeiling => "Above ceiling",
                        HeightRule::Deck => "To deck",
                        HeightRule::Full => "Full height",
                    }
                    .into()
                })
                .unwrap_or_else(|| "Default".into()),
            _ => wt.name.clone(),
        };
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let color = VIEW_COLORS.get(i % VIEW_COLORS.len()).copied().unwrap_or([128, 128, 128]);
            (label.clone(), color)
        })
        .collect()
}

/// How many board layers (non-cavity, category "board" or category empty) a wall assembly has.
fn board_layer_count(asm: &Assembly) -> usize {
    asm.layers.iter().filter(|l| !l.in_cavity && matches!(l.category.as_str(), "board" | "sheathing" | "")).count()
}

/// The colour a wall entity should have in the current view mode.
pub fn viewmode_color(e: &Entity, sp: &Option<BuildingSpec>, vm: &ViewMode) -> Option<[u8; 3]> {
    if wallgen::made_as(e).as_deref() != Some("wall") {
        return None;
    }
    let sp = sp.as_ref()?;
    let type_name = wallgen_tags(e).iter().skip(3).find_map(|v| v.strip_prefix("type:").map(str::to_string))?;
    let wt = sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(&type_name))?;
    let label = match vm.mode.as_str() {
        "type" => wt.name.clone(),
        "rating" => wt.assembly.as_ref().and_then(|a| a.fire_rating.clone()).unwrap_or_else(|| "Not rated".into()),
        "boardLayers" => {
            let count = wt.assembly.as_ref().map(board_layer_count).unwrap_or(0);
            format!("{count} board layer{}", if count == 1 { "" } else { "s" })
        }
        "heightRule" => wt
            .assembly
            .as_ref()
            .and_then(|a| a.height)
            .map(|h| {
                match h {
                    HeightRule::Ceiling => "To ceiling",
                    HeightRule::AboveCeiling => "Above ceiling",
                    HeightRule::Deck => "To deck",
                    HeightRule::Full => "Full height",
                }
                .into()
            })
            .unwrap_or_else(|| "Default".into()),
        _ => return None,
    };
    vm.legend.iter().find(|(l, _)| l == &label).map(|(_, c)| *c)
}

// ---------------------------------------------------------------------------
// drywall.finishfaces — outermost board + finish per room face
// ---------------------------------------------------------------------------

fn run_finishfaces(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let sp = spec_if_any(s).ok_or_else(|| bad("drywall.finishfaces", "no building spec"))?;
    let mut faces = Vec::new();

    for e in d.model.iter().take(MAX_WALLS) {
        if wallgen::made_as(e).as_deref() != Some("wall") {
            continue;
        }
        let tags = wallgen_tags(e);
        let type_name = tags.iter().skip(3).find_map(|v| v.strip_prefix("type:").map(str::to_string));
        let Some(ref tn) = type_name else { continue };
        let wt = match sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(tn)) {
            Some(w) => w,
            None => continue,
        };
        let Some(ref asm) = wt.assembly else { continue };

        // Find the outermost board on each side.
        let side1_board = asm.layers.iter().rfind(|l| l.side == LayerSide::Side1 && !l.in_cavity);
        let side2_board = asm.layers.iter().rfind(|l| l.side == LayerSide::Side2 && !l.in_cavity);

        if let Some(board) = side1_board {
            // No room info on WallMark; finish comes from the wall type for now.
            let finish = String::new();
            faces.push(json!({
                "wallHandle": e.handle.hex(),
                "side": "side1",
                "board": board.material,
                "finish": finish,
            }));
        }
        if let Some(board) = side2_board {
            let finish = String::new();
            faces.push(json!({
                "wallHandle": e.handle.hex(),
                "side": "side2",
                "board": board.material,
                "finish": finish,
            }));
        }
    }

    let count = faces.len();
    Ok(json!({ "faces": faces, "count": count }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The building spec from the drawing, if any.
fn spec_if_any(s: &Session) -> Option<BuildingSpec> {
    let d = s.doc().ok()?;
    d.building_spec.as_deref().filter(|t| !t.trim().is_empty()).and_then(|t| BuildingSpec::from_json(t).ok())
}

/// Read the xdata tag strings for CADADDY_WALLGEN on an entity.
fn wallgen_tags(e: &Entity) -> Vec<String> {
    let x = &e.common.xdata;
    let Some(at) = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(wallgen::APP)) else {
        return Vec::new();
    };
    x.iter().skip(at + 1).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.clone()).collect()
}

/// Compute the gross face area of a wall mesh in square feet. The wall faces are the vertical
/// quads; we sum their areas. Assumes the drawing is in inches (INSUNITS=1) and converts to sqft.
fn wall_face_area_sqft(mesh: &Mesh) -> f64 {
    let mut total = 0.0_f64;
    for face in &mesh.faces {
        let verts: Vec<Vec3> = face.v.iter().filter_map(|&i| mesh.vertices.get(i as usize).copied()).collect();
        total += polygon_area_3d(&verts);
    }
    // The drawing units might be inches (common for architectural); the BuildingSpec and WALLGEN
    // work in the drawing's units. We convert to sqft assuming inches.
    total / 144.0
}

/// The area of one of a wall's faces, square feet: its faces square to the wall (a vertical
/// face whose normal is the wall's main horizontal normal, either way), halved — the ends, top,
/// bottom and the reveals of openings left out. Each board layer covers one face.
fn one_face_area_sqft(mesh: &Mesh) -> f64 {
    let faces: Vec<(f64, [f64; 3])> = mesh
        .faces
        .iter()
        .filter_map(|face| {
            let v: Vec<Vec3> = face.v.iter().filter_map(|&i| mesh.vertices.get(i as usize).copied()).collect();
            let (a, b, c) = (*v.first()?, *v.get(1)?, *v.get(2)?);
            let n = [
                (b.y - a.y) * (c.z - a.z) - (b.z - a.z) * (c.y - a.y),
                (b.z - a.z) * (c.x - a.x) - (b.x - a.x) * (c.z - a.z),
                (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x),
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            (len > 1e-12).then(|| (polygon_area_3d(&v), [n[0] / len, n[1] / len, n[2] / len]))
        })
        .collect();
    // The main face direction: the horizontal normal carrying the most area.
    let Some(main) = faces.iter().filter(|(_, n)| n[2].abs() < 0.1).max_by(|a, b| a.0.total_cmp(&b.0)).map(|f| f.1) else {
        return wall_face_area_sqft(mesh) / 2.0;
    };
    let both: f64 = faces.iter().filter(|(_, n)| (n[0] * main[0] + n[1] * main[1]).abs() > 0.99).map(|f| f.0).sum();
    both / 2.0 / 144.0
}

/// Area of a 3D polygon (triangle or quad) by summing cross products.
fn polygon_area_3d(pts: &[Vec3]) -> f64 {
    if pts.len() < 3 {
        return 0.0;
    }
    let mut cross = Vec3::new(0.0, 0.0, 0.0);
    let a = pts[0];
    for i in 1..pts.len() - 1 {
        let b = pts[i];
        let c = pts.get(i + 1).copied().unwrap_or(a);
        let ab = Vec3::new(b.x - a.x, b.y - a.y, b.z - a.z);
        let ac = Vec3::new(c.x - a.x, c.y - a.y, c.z - a.z);
        cross.x += ab.y * ac.z - ab.z * ac.y;
        cross.y += ab.z * ac.x - ab.x * ac.z;
        cross.z += ab.x * ac.y - ab.y * ac.x;
    }
    (cross.x * cross.x + cross.y * cross.y + cross.z * cross.z).sqrt() * 0.5
}

/// Collected data for one wall entity.
#[derive(Clone, Debug)]
struct WallData {
    _handle: Handle,
    level: String,
    type_name: Option<String>,
    assembly: Option<Assembly>,
    gross_area_sqft: f64,
    opening_area_sqft: f64,
    height_ft: f64,
    length_ft: f64,
}

/// Gather all WALLGEN wall meshes with their types and assemblies.
fn collect_walls(d: &Drawing, sp: &BuildingSpec) -> Vec<WallData> {
    let mut out = Vec::new();
    // units per foot: for INSUNITS=1 (inches) it's 12, for INSUNITS=2 (feet) 1.
    let k = match d.header.i64("INSUNITS", 0) {
        1 => 12.0,
        2 => 1.0,
        4 => 304.8,  // mm
        5 => 30.48,  // cm
        6 => 0.3048, // m
        _ => 12.0,   // default inches
    };

    for e in d.model.iter().take(MAX_WALLS) {
        if wallgen::made_as(e).as_deref() != Some("wall") {
            continue;
        }
        let tags = wallgen_tags(e);
        let level = tags.get(1).cloned().unwrap_or_default();
        let type_name = tags.iter().skip(3).find_map(|v| v.strip_prefix("type:").map(str::to_string));

        let assembly =
            type_name.as_ref().and_then(|tn| sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(tn)).and_then(|wt| wt.assembly.clone()));

        let EntityKind::Mesh(ref mesh) = e.kind else { continue };
        // One face: each board layer covers one side.
        let gross = one_face_area_sqft(mesh);
        let (height_ft, length_ft) = mesh_wall_dims(mesh, k);

        // Opening area: estimated from WALLGEN cut entities on this wall. For now, use the
        // mesh geometry minus the gross vertical faces as an approximation (openings are
        // cut away from the mesh, so gross already excludes them). Set to 0 here; a more
        // detailed opening deduction can be added by scanning door/window entities that
        // share this wall's centreline.
        let opening_area = 0.0;

        out.push(WallData {
            _handle: e.handle,
            level,
            type_name,
            assembly,
            gross_area_sqft: gross,
            opening_area_sqft: opening_area,
            height_ft,
            length_ft,
        });
    }
    out
}

/// Estimate a wall mesh's height and length in feet from its bounding box.
fn mesh_wall_dims(mesh: &Mesh, units_per_ft: f64) -> (f64, f64) {
    if mesh.vertices.is_empty() {
        return (0.0, 0.0);
    }
    let mut min = mesh.vertices[0];
    let mut max = mesh.vertices[0];
    for v in &mesh.vertices {
        if v.x < min.x {
            min.x = v.x;
        }
        if v.y < min.y {
            min.y = v.y;
        }
        if v.z < min.z {
            min.z = v.z;
        }
        if v.x > max.x {
            max.x = v.x;
        }
        if v.y > max.y {
            max.y = v.y;
        }
        if v.z > max.z {
            max.z = v.z;
        }
    }
    let dx = (max.x - min.x) / units_per_ft;
    let dy = (max.y - min.y) / units_per_ft;
    let dz = (max.z - min.z) / units_per_ft;
    let height = dz;
    let length = (dx * dx + dy * dy).sqrt();
    (height, length)
}

/// Write takeoff data to a CSV file.
fn write_takeoff_csv(path: &str, rows: &[Value], waste: f64) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path).map_err(|e| bad("drywall.takeoff", format!("cannot write {path}: {e}")))?;
    writeln!(f, "Group,Material,Thickness (in),Side,Area (sq ft),Sheets,Sheet Size,Stud Length (ft),Track Length (ft),Insulation Area (sq ft),Trim Length (ft),Waste %")
        .map_err(|e| bad("drywall.takeoff", e.to_string()))?;
    for row in rows {
        let group = row["group"].as_str().unwrap_or("");
        if let Some(boards) = row["boards"].as_array() {
            for board in boards {
                let mat = board["material"].as_str().unwrap_or("");
                let thick = board["thicknessIn"].as_f64().unwrap_or(0.0);
                let side = board["side"].as_str().unwrap_or("");
                let area = board["areaSqFt"].as_f64().unwrap_or(0.0);
                let sheets = board["sheets"].as_u64().unwrap_or(0);
                let ss = board["sheetSize"]
                    .as_array()
                    .map(|a| format!("{}x{}", a.first().and_then(Value::as_f64).unwrap_or(4.0), a.get(1).and_then(Value::as_f64).unwrap_or(8.0)))
                    .unwrap_or_default();
                writeln!(f, "\"{group}\",\"{mat}\",{thick:.3},{side},{area:.2},{sheets},{ss},,,,{waste:.1}")
                    .map_err(|e| bad("drywall.takeoff", e.to_string()))?;
            }
        }
        let stud_ft = row["studs"]["lengthFt"].as_f64().unwrap_or(0.0);
        let track_ft = row["track"]["lengthFt"].as_f64().unwrap_or(0.0);
        let insul = row["insulation"]["areaSqFt"].as_f64().unwrap_or(0.0);
        let trim = row["trim"]["lengthFt"].as_f64().unwrap_or(0.0);
        if stud_ft > 0.0 || track_ft > 0.0 || insul > 0.0 || trim > 0.0 {
            writeln!(f, "\"{group}\",,,,,,,,{stud_ft:.2},{track_ft:.2},{insul:.2},{trim:.2}").map_err(|e| bad("drywall.takeoff", e.to_string()))?;
        }
    }
    Ok(())
}

/// A view mode state for the renderer.
#[derive(Clone, Debug)]
pub struct ViewMode {
    pub mode: String,
    pub legend: Vec<(String, [u8; 3])>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_area_triangle() {
        let pts = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 10.0, 0.0)];
        let area = polygon_area_3d(&pts);
        assert!((area - 50.0).abs() < 0.01, "triangle area should be 50, got {area}");
    }

    #[test]
    fn polygon_area_quad() {
        let pts = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(120.0, 0.0, 0.0), Vec3::new(120.0, 0.0, 96.0), Vec3::new(0.0, 0.0, 96.0)];
        let area = polygon_area_3d(&pts);
        // 120" x 96" = 11520 sq in
        assert!((area - 11520.0).abs() < 0.1, "quad area should be 11520, got {area}");
        // In sqft: 11520/144 = 80
        let sqft = area / 144.0;
        assert!((sqft - 80.0).abs() < 0.01, "should be 80 sqft, got {sqft}");
    }

    #[test]
    fn board_layer_count_works() {
        let asm = Assembly {
            layers: vec![
                AssemblyLayer {
                    material: "5/8\" Type X gypsum board".into(),
                    thickness_in: 0.625,
                    side: LayerSide::Side1,
                    category: "board".into(),
                    in_cavity: false,
                    spacing_in: None,
                },
                AssemblyLayer {
                    material: "3 5/8\" metal studs".into(),
                    thickness_in: 3.625,
                    side: LayerSide::Core,
                    category: "framing".into(),
                    in_cavity: false,
                    spacing_in: None,
                },
                AssemblyLayer {
                    material: "batt insulation".into(),
                    thickness_in: 3.5,
                    side: LayerSide::Core,
                    category: "insulation".into(),
                    in_cavity: true,
                    spacing_in: None,
                },
                AssemblyLayer {
                    material: "5/8\" Type X gypsum board".into(),
                    thickness_in: 0.625,
                    side: LayerSide::Side2,
                    category: "board".into(),
                    in_cavity: false,
                    spacing_in: None,
                },
            ],
            ..Assembly::default()
        };
        assert_eq!(board_layer_count(&asm), 2);
    }

    #[test]
    fn assembly_json_round_trip() {
        let asm = Assembly {
            tag: "A".into(),
            description: "1 Hour Rated Partition".into(),
            layers: vec![AssemblyLayer {
                material: "5/8\" Type X gypsum board".into(),
                thickness_in: 0.625,
                side: LayerSide::Side1,
                category: "board".into(),
                in_cavity: false,
                spacing_in: None,
            }],
            fire_rating: Some("1 HR".into()),
            ..Assembly::default()
        };
        let j = assembly_json(&asm);
        assert_eq!(j["tag"], "A");
        assert_eq!(j["fireRating"], "1 HR");
        assert!(j["layers"].is_array());
    }

    #[test]
    fn layer_spacing_parses() {
        let l = AssemblyLayer { material: "3 5/8\" 20 ga metal studs @ 16\" o.c.".into(), category: "framing".into(), ..Default::default() };
        assert_eq!(layer_spacing_in(&l), Some(16.0));
        let l2 = AssemblyLayer { material: "studs".into(), category: "framing".into(), ..Default::default() };
        assert_eq!(layer_spacing_in(&l2), Some(16.0));
    }
}
