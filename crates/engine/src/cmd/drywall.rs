//! Drywall views and takeoff: the "What is this wall?" inspector (`wall.info`), view mode
//! colouring by wall type / fire rating / board-layer count / height rule (`drywall.viewmode`),
//! finish faces (`drywall.finishfaces`), and a drywall-specific takeoff (`drywall.takeoff`) that
//! gives board area per type and thickness (minus openings, per side, per level/room/wall type),
//! sheet counts with waste, stud and track length, insulation area, and corner bead / trim length.
//!
//! Everything builds on the wall assemblies from Q293: the building spec's `wall_types` (each with
//! an `assembly` of layers), `walls` (marks on the plans giving each wall a type and height), and
//! WALLGEN's `type:<name>` xdata tag on 3D wall meshes.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Mesh};
use cadaddy_geom::Vec3;
use cadaddy_walk3d::{Assembly, AssemblyLayer, BuildingSpec, Head, HeadTo, HeadTrack, HeightRule, LayerSide};
use serde_json::{Value, json};

use super::wallgen;
use super::*;
use crate::{Result, Session};

/// Maximum wall entities to process in one takeoff (hostile drawings).
const MAX_WALLS: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("wall.info", "Wall Info", run_info)
            .params("{handle? | handles? (default: selection)} → {walls: [{handle, level, wallType, assembly?: {tag, description, layers: [{material, thickness, side, category, inCavity}], height?, fireRating?, acousticRating?, external, thickness, head?: {track, trackKind, to (deck|ceiling), deflectionIn?, bottomTrack, detail?, text}}, grossAreaSqFt?, finishFaces?: [{side, room, board, finish}]}, ...], count}")
            .noundo(),
        CommandSpec::new("drywall.takeoff", "Drywall Takeoff", run_takeoff)
            .params("{handles? | levels?: [name, ...] | wallTypes?: [name, ...] | rooms?: [name, ...], waste?: percent (default 10), sheetSize?: [w,h] feet (default [4,8]), file?: path (.csv), groupBy?: \"level\" | \"room\" | \"wallType\" (default \"wallType\"), ceilings?: bool (default true; off when handles, wallTypes or rooms are given)} → {rows: [{group, boards: [{material, thicknessIn, side, areaSqFt, sheets, sheetSize}], studs: {lengthFt, count, material, spacingIn}, track: {lengthFt} (plain top and bottom track), headTrack: [{part: top track|bottom track, wallType, material, lengthFt}] (walls marked on the plans whose type has a head of wall, by kind; a double track or angles count both runs), insulation: {areaSqFt, material}, cornerBead: {lengthFt}, trim: {lengthFt}}], ceilings: [{level, part: ceiling|ceiling framing|soffit bottom|soffit sides|soffit framing, assembly, material, layer, quantity, unit: sf|lf, sheets?}] (the spec's ceiling and soffit types, as rcp.takeoff), totals (boardAreaSqFt and sheets: walls; ceilingBoardAreaSqFt, soffitBoardAreaSqFt, ceilingSheets, ceilingFramingLengthFt; trackLengthFt = plain + head + bottom track, each wall once), waste}")
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
    let k = insunits_per_ft(d);
    let room_map = sp.as_ref().map(|sp| build_room_map(d, sp));
    let mut walls = Vec::new();
    for h in hs.iter().take(MAX_WALLS) {
        let Some(e) = d.entity(*h) else { continue };
        if wallgen::made_as(e).as_deref() != Some("wall") {
            continue;
        }
        walls.push(wall_info_json(e, &sp, k, room_map.as_ref()));
    }
    let count = walls.len();
    Ok(json!({ "walls": walls, "count": count }))
}

/// The wall info for one WALLGEN wall entity.
fn wall_info_json(e: &Entity, sp: &Option<BuildingSpec>, k: f64, room_map: Option<&Vec<RoomPoint>>) -> Value {
    let tags = wallgen_tags(e);
    let level = tags.get(1).cloned().unwrap_or_default();
    let type_name = tags.iter().skip(3).find_map(|v| v.strip_prefix("type:")).map(str::to_string);

    let mut info = json!({
        "handle": e.handle.hex(),
        "level": level,
    });

    let mut asm_ref: Option<&Assembly> = None;
    if let Some(tn) = &type_name {
        info["wallType"] = json!(tn);
        // Look up the assembly from the building spec.
        if let Some(sp) = sp
            && let Some(wt) = sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(tn))
            && let Some(asm) = &wt.assembly
        {
            info["assembly"] = assembly_json(asm);
            asm_ref = Some(asm);
        }
    }

    // Compute gross area per side from the mesh bounding box.
    if let EntityKind::Mesh(ref mesh) = e.kind {
        let (height_ft, length_ft) = mesh_wall_dims(mesh, k);
        let area = height_ft * length_ft;
        if area > 0.0 {
            info["grossAreaSqFt"] = json!((area * 100.0).round() / 100.0);
        }
    }

    // Finish faces (included in wall.info per the params text).
    if let Some(asm) = asm_ref
        && let EntityKind::Mesh(ref mesh) = e.kind
    {
        let faces = finish_faces_for_wall(e.handle, mesh, asm, k, room_map);
        if !faces.is_empty() {
            info["finishFaces"] = json!(faces);
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
    if let Some(h) = &asm.head {
        v["head"] = head_json(h);
    }
    v
}

/// A wall type's head of wall: its track, what it goes up to, the deflection it takes, the bottom
/// track and the detail.
fn head_json(h: &Head) -> Value {
    let (track, kind) = head_track_names(h.track);
    let mut v = json!({
        "track": track,
        "trackKind": kind,
        "to": match h.to { HeadTo::Deck => "deck", HeadTo::Ceiling => "ceiling" },
        "bottomTrack": h.bottom_track,
        "text": h.text(),
    });
    if let Some(d) = h.deflection_in.filter(|d| d.is_finite()) {
        v["deflectionIn"] = json!(d);
    }
    if let Some(d) = &h.detail {
        v["detail"] = json!(d);
    }
    v
}

/// A head track's name as the wall inspector shows it, and its kind as the spec writes it.
fn head_track_names(t: HeadTrack) -> (&'static str, &'static str) {
    match t {
        HeadTrack::Single => ("top track", "single"),
        HeadTrack::Deflection => ("slotted deflection track", "deflection"),
        HeadTrack::Double => ("double (nested) track", "double"),
        HeadTrack::Angle => ("steel angles each side", "angle"),
    }
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

    // Validate sheetSize.
    let (sheet_w, sheet_h) = if let Some(ss) = p.get("sheetSize") {
        let w = ss.get(0).and_then(Value::as_f64).unwrap_or(0.0);
        let h = ss.get(1).and_then(Value::as_f64).unwrap_or(0.0);
        if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
            return Err(bad(cmd, "sheetSize must be [width, height] in feet, both > 0 (e.g. [4, 8] for 4'x8' sheets)"));
        }
        (w, h)
    } else {
        (4.0, 8.0)
    };

    let group_by = str_param(p, "groupBy").unwrap_or("wallType");
    match group_by {
        "level" | "room" | "wallType" => {}
        _ => return Err(bad(cmd, "groupBy must be \"level\", \"room\" or \"wallType\"")),
    }

    // Validate file extension.
    if let Some(path) = str_param(p, "file")
        && (path.ends_with(".xlsx") || path.ends_with(".XLSX"))
    {
        return Err(bad(cmd, "XLSX export is not supported; use .csv"));
    }

    // Collect all WALLGEN wall meshes.
    let wall_entities = collect_walls(d, &sp);
    if wall_entities.is_empty() && sp.rcp.ceilings.is_empty() && sp.rcp.soffits.is_empty() {
        return Ok(json!({ "rows": [], "totals": {}, "waste": waste_pct }));
    }

    // Apply filters.
    let handle_filter: Option<Vec<Handle>> = str_array_param(p, "handles").map(|v| v.iter().filter_map(|s| Handle::parse_hex(s)).collect());
    let level_filter = str_array_param(p, "levels");
    let type_filter = str_array_param(p, "wallTypes");
    let room_filter = str_array_param(p, "rooms");

    let room_map = if group_by == "room" || room_filter.is_some() { Some(build_room_map(d, &sp)) } else { None };
    // Ceilings and soffits have no wall handle, type or room side: only the level filter applies.
    let with_ceilings =
        p.get("ceilings").and_then(Value::as_bool).unwrap_or(true) && handle_filter.is_none() && type_filter.is_none() && room_filter.is_none();

    let filtered: Vec<&WallData> = wall_entities
        .iter()
        .filter(|wd| {
            if let Some(ref handles) = handle_filter
                && !handles.contains(&wd.handle)
            {
                return false;
            }
            if let Some(ref levels) = level_filter
                && !levels.iter().any(|l| l.eq_ignore_ascii_case(&wd.level))
            {
                return false;
            }
            if let Some(ref types) = type_filter {
                let tn = wd.type_name.as_deref().unwrap_or("");
                if !types.iter().any(|t| t.eq_ignore_ascii_case(tn)) {
                    return false;
                }
            }
            if let Some(ref rooms) = room_filter {
                if let Some(ref rm) = room_map {
                    let (s1, s2) = wall_rooms(&wd.centreline, rm);
                    let s1 = s1.unwrap_or_default();
                    let s2 = s2.unwrap_or_default();
                    if !rooms.iter().any(|r| r.eq_ignore_ascii_case(&s1) || r.eq_ignore_ascii_case(&s2)) {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            true
        })
        .collect();

    // Group walls by the requested key.
    let mut groups: BTreeMap<String, Vec<&WallData>> = BTreeMap::new();
    for wd in &filtered {
        let key = match group_by {
            "level" => wd.level.clone(),
            "room" => {
                if let Some(ref rm) = room_map {
                    let (s1, s2) = wall_rooms(&wd.centreline, rm);
                    format!("{} / {}", s1.unwrap_or_default(), s2.unwrap_or_default())
                } else {
                    "(unknown)".into()
                }
            }
            _ => wd.type_name.clone().unwrap_or_else(|| "(untyped)".into()),
        };
        groups.entry(key).or_default().push(wd);
    }

    let mut rows = Vec::new();
    let mut total_board_area = 0.0_f64;
    let mut total_sheets = 0_u64;
    let mut total_stud_ft = 0.0_f64;
    let mut total_track_ft = 0.0_f64;
    let mut total_insulation_area = 0.0_f64;
    let mut total_trim_ft = 0.0_f64;
    let mut total_corner_bead_ft = 0.0_f64;
    let mut total_head_ft = 0.0_f64;
    let mut total_bottom_ft = 0.0_f64;

    for (group_name, walls) in &groups {
        let mut board_rows: Vec<Value> = Vec::new();
        let mut stud_length = 0.0_f64;
        let mut stud_count_total = 0_u64;
        let mut stud_material = String::new();
        let mut stud_spacing = 0.0_f64;
        let mut track_length = 0.0_f64;
        let mut insul_area = 0.0_f64;
        let mut insul_material = String::new();
        let mut trim_length = 0.0_f64;
        let mut corner_bead_length = 0.0_f64;

        // Accumulate per board type (material + thickness + side).
        let mut board_totals: BTreeMap<(String, String, String), f64> = BTreeMap::new();
        // The plan's wall marks whose head of wall these walls take their track from.
        let mut head_marks: BTreeSet<usize> = BTreeSet::new();

        for wd in walls {
            let net = wd.net_area_sqft();
            let head_mark = head_mark(&sp, wd);
            if let Some(i) = head_mark {
                head_marks.insert(i);
            }
            let wall_height_ft = wd.height_ft;
            let wall_length_ft = wd.length_ft;

            if let Some(ref asm) = wd.assembly {
                for layer in &asm.layers {
                    match layer.category.as_str() {
                        "board" | "sheathing" | "" if !layer.in_cavity => {
                            let key = (layer.material.clone(), format!("{:.3}", layer.thickness_in), side_str(layer.side).to_string());
                            *board_totals.entry(key).or_default() += net;
                        }
                        "framing" => {
                            // Use spacing_in from the layer struct first, then parse from text.
                            let spacing = layer.spacing_in.unwrap_or_else(|| layer_spacing_in(layer).unwrap_or(16.0));
                            let count = ((wall_length_ft * 12.0 / spacing).ceil() as u64).saturating_add(1);
                            stud_length += count as f64 * wall_height_ft;
                            stud_count_total = stud_count_total.saturating_add(count);
                            if stud_material.is_empty() {
                                stud_material = layer.material.clone();
                                stud_spacing = spacing;
                            }
                            // Top and bottom track; a wall with a head of wall has its track by kind below.
                            if head_mark.is_none() {
                                track_length += wall_length_ft * 2.0;
                            }
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
                // Corner bead: at outside corners and wall ends. Approximate from height
                // at the two ends of each wall.
                corner_bead_length += wall_height_ft * 2.0;
                // Trim: board edges at floor and ceiling on each side with boards.
                let has_s1 = asm
                    .layers
                    .iter()
                    .any(|l| l.side == LayerSide::Side1 && !l.in_cavity && matches!(l.category.as_str(), "board" | "sheathing" | ""));
                let has_s2 = asm
                    .layers
                    .iter()
                    .any(|l| l.side == LayerSide::Side2 && !l.in_cavity && matches!(l.category.as_str(), "board" | "sheathing" | ""));
                let side_count = has_s1 as usize + has_s2 as usize;
                trim_length += wall_length_ft * 2.0 * side_count as f64;
            } else {
                // No assembly: count the whole wall as one board layer per side.
                let key = ("gypsum board".into(), "0.625".into(), "side1".into());
                *board_totals.entry(key).or_default() += net;
                let key = ("gypsum board".into(), "0.625".into(), "side2".into());
                *board_totals.entry(key).or_default() += net;
            }
        }

        let sheet_area = sheet_w * sheet_h;
        for ((material, thick, side), area) in &board_totals {
            let area_with_waste = area * (1.0 + waste_pct / 100.0);
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
        total_corner_bead_ft += corner_bead_length;

        let head_rows = head_track(&sp, &head_marks);
        for r in &head_rows {
            if r.part == "bottom track" {
                total_bottom_ft += r.quantity;
            } else {
                total_head_ft += r.quantity;
            }
        }
        let head_json: Vec<Value> = head_rows
            .iter()
            .map(|r| json!({"part": r.part, "wallType": r.assembly, "material": r.material, "lengthFt": (r.quantity * 100.0).round() / 100.0}))
            .collect();

        let mut studs_json = json!({ "lengthFt": (stud_length * 100.0).round() / 100.0, "material": stud_material });
        if stud_count_total > 0 {
            studs_json["count"] = json!(stud_count_total);
            studs_json["spacingIn"] = json!(stud_spacing);
        }

        rows.push(json!({
            "group": group_name,
            "boards": board_rows,
            "studs": studs_json,
            "track": { "lengthFt": (track_length * 100.0).round() / 100.0 },
            "headTrack": head_json,
            "insulation": { "areaSqFt": (insul_area * 100.0).round() / 100.0, "material": insul_material },
            "cornerBead": { "lengthFt": (corner_bead_length * 100.0).round() / 100.0 },
            "trim": { "lengthFt": (trim_length * 100.0).round() / 100.0 },
        }));
    }

    // Ceilings and soffits: board by type and layer, framing (the wall track is in the rows).
    let mut ceilings = Vec::new();
    let (mut ceiling_sf, mut soffit_sf, mut ceiling_framing_ft) = (0.0_f64, 0.0_f64, 0.0_f64);
    let mut ceiling_sheets = 0_u64;
    if with_ceilings {
        let sheet_area = sheet_w * sheet_h;
        for r in cadaddy_walk3d::takeoff(&sp).rows {
            if matches!(r.part.as_str(), "top track" | "bottom track")
                || level_filter.as_ref().is_some_and(|ls| !ls.iter().any(|l| l.eq_ignore_ascii_case(&r.level)))
            {
                continue;
            }
            match r.part.as_str() {
                "ceiling" => ceiling_sf += r.quantity,
                "soffit bottom" | "soffit sides" => soffit_sf += r.quantity,
                _ => ceiling_framing_ft += r.quantity,
            }
            let mut v = json!({"level": r.level, "part": r.part, "assembly": r.assembly, "material": r.material, "layer": r.layer,
                "quantity": (r.quantity * 100.0).round() / 100.0, "unit": r.unit});
            if r.unit == "sf" && !r.material.to_ascii_lowercase().contains("tile") {
                let sheets = (r.quantity * (1.0 + waste_pct / 100.0) / sheet_area).ceil() as u64;
                ceiling_sheets += sheets;
                v["sheets"] = json!(sheets);
            }
            ceilings.push(v);
        }
    }

    // Write CSV if requested.
    if let Some(path) = str_param(p, "file") {
        write_takeoff_csv(path, &rows, &ceilings, waste_pct)?;
    }

    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    Ok(json!({
        "rows": rows,
        "ceilings": ceilings,
        "totals": {
            "boardAreaSqFt": (total_board_area * 100.0).round() / 100.0,
            "sheets": total_sheets,
            "studLengthFt": (total_stud_ft * 100.0).round() / 100.0,
            "trackLengthFt": r2(total_track_ft + total_head_ft + total_bottom_ft),
            "headTrackLengthFt": r2(total_head_ft),
            "bottomTrackLengthFt": r2(total_bottom_ft),
            "ceilingBoardAreaSqFt": r2(ceiling_sf),
            "soffitBoardAreaSqFt": r2(soffit_sf),
            "ceilingFramingLengthFt": r2(ceiling_framing_ft),
            "ceilingSheets": ceiling_sheets,
            "insulationAreaSqFt": (total_insulation_area * 100.0).round() / 100.0,
            "cornerBeadLengthFt": (total_corner_bead_ft * 100.0).round() / 100.0,
            "trimLengthFt": (total_trim_ft * 100.0).round() / 100.0,
        },
        "waste": waste_pct,
    }))
}

/// The plan wall mark a WALLGEN wall stands on, when its type has a head of wall: same level and
/// type, the wall's middle within half its thickness (and 6") of the mark, running along it.
fn head_mark(sp: &BuildingSpec, wd: &WallData) -> Option<usize> {
    let asm = wd.assembly.as_ref()?;
    asm.head.as_ref()?;
    let tn = wd.type_name.as_deref()?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let t = asm.thickness_in();
    let tol = if t.is_finite() { (t / 24.0).clamp(0.0, 10.0) + 0.5 } else { 0.5 };
    let m = wd.centreline.mid;
    sp.walls.iter().enumerate().take(MAX_WALLS).find_map(|(i, w)| {
        if !w.level.eq_ignore_ascii_case(&wd.level) || !w.wall_type.eq_ignore_ascii_case(tn) {
            return None;
        }
        let (a, b) = ([w.a[0] / upf, w.a[1] / upf], [w.b[0] / upf, w.b[1] / upf]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy);
        if !(len.is_finite() && len > 1e-9) {
            return None;
        }
        // A wall across the mark (a corner) isn't on it.
        let n = wd.centreline.normal;
        if ((dx * n[0] + dy * n[1]) / len).abs() > 0.75 {
            return None;
        }
        let u = (((m[0] - a[0]) * dx + (m[1] - a[1]) * dy) / (len * len)).clamp(0.0, 1.0);
        let dist = (m[0] - a[0] - u * dx).hypot(m[1] - a[1] - u * dy);
        (dist <= tol).then_some(i)
    })
}

/// The top and bottom track of the plan's wall marks `marks` by kind, by the same rules as
/// `rcp.takeoff` (a double track and head angles count both runs).
fn head_track(sp: &BuildingSpec, marks: &BTreeSet<usize>) -> Vec<cadaddy_walk3d::TakeoffRow> {
    if marks.is_empty() {
        return Vec::new();
    }
    let only = BuildingSpec {
        units_per_foot: sp.units_per_foot,
        wall_types: sp.wall_types.clone(),
        walls: marks.iter().filter_map(|i| sp.walls.get(*i).cloned()).collect(),
        ..BuildingSpec::default()
    };
    cadaddy_walk3d::takeoff(&only).rows
}

/// Extract spacing in inches from a layer's material description or its `spacing_in` field.
fn layer_spacing_in(layer: &AssemblyLayer) -> Option<f64> {
    if let Some(s) = layer.spacing_in
        && s > 0.0
        && s.is_finite()
    {
        return Some(s);
    }
    let m = &layer.material;
    if let Some(at) = m.find('@') {
        let after = &m[at + 1..];
        let num: String = after.chars().skip_while(|c| c.is_whitespace()).take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        return num.parse::<f64>().ok().filter(|v| *v > 0.0 && v.is_finite());
    }
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
            s.drywall_color_overrides.clear();
            s.drywall_layer_lines.clear();
            Ok(json!({ "mode": "off", "legend": [] }))
        }
        "type" | "rating" | "boardLayers" | "heightRule" => {
            let d = s.doc()?;
            let sp = sp.ok_or_else(|| bad(cmd, "no building spec"))?;
            let legend = build_legend(mode, &sp);
            let vm = ViewMode { mode: mode.into(), legend: legend.clone() };
            // Build the handle→colour map for the renderer.
            let mut overrides = BTreeMap::new();
            for e in d.model.iter().take(MAX_WALLS) {
                if let Some(c) = viewmode_color(e, &Some(sp.clone()), &vm) {
                    overrides.insert(e.handle, c);
                }
            }
            // Compute layer boundary lines for the renderer.
            let layer_lines = compute_layer_lines(d, &sp);
            s.drywall_viewmode = Some(vm);
            s.drywall_color_overrides = overrides;
            s.drywall_layer_lines = layer_lines;
            let legend_json: Vec<Value> = legend.iter().map(|(label, [r, g, b])| json!({ "label": label, "color": [r, g, b] })).collect();
            Ok(json!({ "mode": mode, "legend": legend_json }))
        }
        _ => Err(bad(cmd, "mode must be type, rating, boardLayers, heightRule or off")),
    }
}

/// A shared label function for view modes. Given a wall type and mode, returns the legend label.
fn viewmode_label(mode: &str, wt: &cadaddy_walk3d::WallType) -> Option<String> {
    Some(match mode {
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
    })
}

/// Build a legend: each unique value gets a colour.
fn build_legend(mode: &str, sp: &BuildingSpec) -> Vec<(String, [u8; 3])> {
    let mut labels: Vec<String> = Vec::new();
    for wt in &sp.wall_types {
        if let Some(label) = viewmode_label(mode, wt)
            && !labels.contains(&label)
        {
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

/// How many board layers (non-cavity, category "board" or "sheathing" or empty) a wall assembly has.
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
    let label = viewmode_label(&vm.mode, wt)?;
    vm.legend.iter().find(|(l, _)| l == &label).map(|(_, c)| *c)
}

// ---------------------------------------------------------------------------
// drywall.finishfaces — outermost board + finish per room face
// ---------------------------------------------------------------------------

fn run_finishfaces(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let sp = spec_if_any(s).ok_or_else(|| bad("drywall.finishfaces", "no building spec"))?;
    let k = insunits_per_ft(d);
    let room_map = build_room_map(d, &sp);
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
        let EntityKind::Mesh(ref mesh) = e.kind else { continue };

        let ff = finish_faces_for_wall(e.handle, mesh, asm, k, Some(&room_map));
        faces.extend(ff);
    }

    let count = faces.len();
    Ok(json!({ "faces": faces, "count": count }))
}

/// Compute finish faces for one wall, returning JSON values.
fn finish_faces_for_wall(handle: Handle, mesh: &Mesh, asm: &Assembly, k: f64, room_map: Option<&Vec<RoomPoint>>) -> Vec<Value> {
    let mut faces = Vec::new();
    let centreline = mesh_centreline(mesh, k);

    // Find the outermost board on each side:
    // Side 1: the FIRST non-cavity board/sheathing layer on side 1 (the outermost face).
    // Layers run from side 1's face to side 2's face, so the first side1 layer is the face layer.
    let side1_board =
        asm.layers.iter().find(|l| l.side == LayerSide::Side1 && !l.in_cavity && matches!(l.category.as_str(), "board" | "sheathing" | ""));
    // Side 2: the LAST non-cavity board/sheathing layer on side 2.
    let side2_board =
        asm.layers.iter().rfind(|l| l.side == LayerSide::Side2 && !l.in_cavity && matches!(l.category.as_str(), "board" | "sheathing" | ""));

    let (room1, room2) = room_map.map(|rm| wall_rooms(&centreline, rm)).unwrap_or((None, None));

    if let Some(board) = side1_board {
        let room = room1.clone().unwrap_or_default();
        let finish = room_map
            .and_then(|rm| {
                let rn = room1.as_deref()?;
                rm.iter().find(|r| r.name.eq_ignore_ascii_case(rn))?.finish.clone()
            })
            .unwrap_or_default();
        faces.push(json!({
            "wallHandle": handle.hex(),
            "side": "side1",
            "room": room,
            "board": board.material,
            "finish": finish,
        }));
    }
    if let Some(board) = side2_board {
        let room = room2.clone().unwrap_or_default();
        let finish = room_map
            .and_then(|rm| {
                let rn = room2.as_deref()?;
                rm.iter().find(|r| r.name.eq_ignore_ascii_case(rn))?.finish.clone()
            })
            .unwrap_or_default();
        faces.push(json!({
            "wallHandle": handle.hex(),
            "side": "side2",
            "room": room,
            "board": board.material,
            "finish": finish,
        }));
    }

    faces
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

/// Units per foot for the drawing: INSUNITS=1 (inches) → 12, INSUNITS=2 (feet) → 1, etc.
fn insunits_per_ft(d: &Drawing) -> f64 {
    match d.header.i64("INSUNITS", 0) {
        1 => 12.0,
        2 => 1.0,
        4 => 304.8,  // mm
        5 => 30.48,  // cm
        6 => 0.3048, // m
        _ => 12.0,   // default inches
    }
}

/// Area of a 3D polygon (triangle or quad) by summing cross products.
#[cfg(test)]
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
    handle: Handle,
    level: String,
    type_name: Option<String>,
    assembly: Option<Assembly>,
    /// One face's area (length × height) in sq ft.
    gross_area_sqft: f64,
    /// Area of openings (doors/windows) deducted from gross.
    opening_area_sqft: f64,
    height_ft: f64,
    length_ft: f64,
    /// The wall centreline as (midpoint, direction) in drawing units.
    centreline: WallCentreline,
}

/// The centreline of a wall for room-side detection.
#[derive(Clone, Debug)]
struct WallCentreline {
    mid: [f64; 2],
    /// Unit normal pointing to side 1 (left when facing from a to b).
    normal: [f64; 2],
}

impl WallData {
    fn net_area_sqft(&self) -> f64 {
        (self.gross_area_sqft - self.opening_area_sqft).max(0.0)
    }
}

/// Gather all WALLGEN wall meshes with their types and assemblies.
fn collect_walls(d: &Drawing, sp: &BuildingSpec) -> Vec<WallData> {
    let mut out = Vec::new();
    let k = insunits_per_ft(d);

    // Collect door/window entities for opening deduction.
    let openings = collect_openings(d, k);

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
        let (height_ft, length_ft) = mesh_wall_dims(mesh, k);
        // Gross area is one face: length × height.
        let gross = length_ft * height_ft;
        let centreline = mesh_centreline(mesh, k);

        // Opening deduction: sum area of door/window entities whose centreline overlaps this wall.
        let opening_area = opening_deduction(&centreline, height_ft, &openings);

        out.push(WallData {
            handle: e.handle,
            level,
            type_name,
            assembly,
            gross_area_sqft: gross,
            opening_area_sqft: opening_area,
            height_ft,
            length_ft,
            centreline,
        });
    }
    out
}

/// An opening (door or window) on a wall.
#[derive(Clone, Debug)]
struct Opening {
    /// Midpoint of the opening in plan, drawing units.
    mid: [f64; 2],
    width_ft: f64,
    height_ft: f64,
}

/// Collect door and window entities for opening deduction.
fn collect_openings(d: &Drawing, k: f64) -> Vec<Opening> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_WALLS) {
        let part = wallgen::made_as(e);
        let is_opening = matches!(part.as_deref(), Some("door") | Some("window"));
        if !is_opening {
            continue;
        }
        let EntityKind::Mesh(ref mesh) = e.kind else { continue };
        if mesh.vertices.is_empty() {
            continue;
        }
        let (h, w) = mesh_wall_dims(mesh, k);
        let mid = mesh_mid_xy(mesh);
        out.push(Opening { mid, width_ft: w, height_ft: h });
    }
    out
}

/// Midpoint of a mesh's bounding box in plan (XY), in drawing units.
fn mesh_mid_xy(mesh: &Mesh) -> [f64; 2] {
    if mesh.vertices.is_empty() {
        return [0.0, 0.0];
    }
    let first = mesh.vertices.first().copied().unwrap_or(Vec3::ZERO);
    let (mut minx, mut miny) = (first.x, first.y);
    let (mut maxx, mut maxy) = (first.x, first.y);
    for v in &mesh.vertices {
        if v.x < minx {
            minx = v.x;
        }
        if v.y < miny {
            miny = v.y;
        }
        if v.x > maxx {
            maxx = v.x;
        }
        if v.y > maxy {
            maxy = v.y;
        }
    }
    [(minx + maxx) * 0.5, (miny + maxy) * 0.5]
}

/// The centreline of a wall mesh: the midpoint and the normal direction (side1 is left).
fn mesh_centreline(mesh: &Mesh, k: f64) -> WallCentreline {
    if mesh.vertices.is_empty() {
        return WallCentreline { mid: [0.0, 0.0], normal: [0.0, 1.0] };
    }
    let first = mesh.vertices.first().copied().unwrap_or(Vec3::ZERO);
    let (mut minx, mut miny) = (first.x, first.y);
    let (mut maxx, mut maxy) = (first.x, first.y);
    for v in &mesh.vertices {
        if v.x < minx {
            minx = v.x;
        }
        if v.y < miny {
            miny = v.y;
        }
        if v.x > maxx {
            maxx = v.x;
        }
        if v.y > maxy {
            maxy = v.y;
        }
    }
    let dx = maxx - minx;
    let dy = maxy - miny;
    let mid = [(minx + maxx) * 0.5 / k, (miny + maxy) * 0.5 / k];
    // The wall runs along the longer dimension; the normal is perpendicular.
    let (nx, ny) = if dx > dy { (0.0, 1.0) } else { (-1.0, 0.0) };
    WallCentreline { mid, normal: [nx, ny] }
}

/// Sum opening area that overlaps a wall, in sq ft.
fn opening_deduction(wall: &WallCentreline, _wall_height_ft: f64, openings: &[Opening]) -> f64 {
    let mut total = 0.0;
    for op in openings {
        // Check if the opening is near the wall centreline (within a few feet).
        let dx = op.mid[0] - wall.mid[0];
        let dy = op.mid[1] - wall.mid[1];
        let dist_along_normal = (dx * wall.normal[0] + dy * wall.normal[1]).abs();
        // If the opening is within ~2 ft of the wall centreline, it's on this wall.
        if dist_along_normal < 2.0 {
            total += op.width_ft * op.height_ft;
        }
    }
    total
}

/// Estimate a wall mesh's height and length in feet from its bounding box.
fn mesh_wall_dims(mesh: &Mesh, units_per_ft: f64) -> (f64, f64) {
    if mesh.vertices.is_empty() {
        return (0.0, 0.0);
    }
    let first = mesh.vertices.first().copied().unwrap_or(Vec3::ZERO);
    let mut min = first;
    let mut max = first;
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
    // The wall runs along its longer plan dimension; the shorter is its thickness.
    let length = dx.max(dy);
    (height, length)
}

/// Write takeoff data to a CSV file with properly escaped fields.
fn write_takeoff_csv(path: &str, rows: &[Value], ceilings: &[Value], waste: f64) -> Result<()> {
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
                writeln!(f, "{},{},{thick:.3},{side},{area:.2},{sheets},{ss},,,,,{waste:.1}", csv_escape(group), csv_escape(mat))
                    .map_err(|e| bad("drywall.takeoff", e.to_string()))?;
            }
        }
        let stud_ft = row["studs"]["lengthFt"].as_f64().unwrap_or(0.0);
        let track_ft = row["track"]["lengthFt"].as_f64().unwrap_or(0.0);
        let insul = row["insulation"]["areaSqFt"].as_f64().unwrap_or(0.0);
        let trim = row["trim"]["lengthFt"].as_f64().unwrap_or(0.0);
        if stud_ft > 0.0 || track_ft > 0.0 || insul > 0.0 || trim > 0.0 {
            writeln!(f, "{},,,,,,,{stud_ft:.2},{track_ft:.2},{insul:.2},{trim:.2},", csv_escape(group))
                .map_err(|e| bad("drywall.takeoff", e.to_string()))?;
        }
        for h in row["headTrack"].as_array().into_iter().flatten() {
            let mat = h["material"].as_str().unwrap_or("");
            let len = h["lengthFt"].as_f64().unwrap_or(0.0);
            writeln!(f, "{},{},,,,,,,{len:.2},,,", csv_escape(group), csv_escape(mat)).map_err(|e| bad("drywall.takeoff", e.to_string()))?;
        }
    }
    // Ceilings and soffits: board in the area column, framing in the stud length column.
    for c in ceilings {
        let group = format!("{} {} ({})", c["part"].as_str().unwrap_or(""), c["assembly"].as_str().unwrap_or(""), c["level"].as_str().unwrap_or(""));
        let mat = c["material"].as_str().unwrap_or("");
        let q = c["quantity"].as_f64().unwrap_or(0.0);
        let line = if c["unit"] == "sf" {
            let sheets = c["sheets"].as_u64().map(|n| n.to_string()).unwrap_or_default();
            format!("{},{},,,{q:.2},{sheets},,,,,,{waste:.1}", csv_escape(&group), csv_escape(mat))
        } else {
            format!("{},{},,,,,,{q:.2},,,,", csv_escape(&group), csv_escape(mat))
        };
        writeln!(f, "{line}").map_err(|e| bad("drywall.takeoff", e.to_string()))?;
    }
    Ok(())
}

/// Properly escape a CSV field: if it contains commas, quotes or newlines, wrap in quotes
/// and double any embedded quotes.
fn csv_escape(s: &str) -> String {
    if s.contains('"') || s.contains(',') || s.contains('\n') || s.contains('\r') { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// Parse an optional JSON array of strings.
fn str_array_param(p: &Value, key: &str) -> Option<Vec<String>> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
}

// ---------------------------------------------------------------------------
// Room geometry matching
// ---------------------------------------------------------------------------

/// A room's position for geometric matching: found from text entities in the drawing
/// that match room names in the spec.
#[derive(Clone, Debug)]
struct RoomPoint {
    name: String,
    /// Position of the room name label in feet.
    pos: [f64; 2],
    /// Wall finish material from the spec.
    finish: Option<String>,
}

/// Build a map of room positions from text entities in the drawing that match spec room names.
fn build_room_map(d: &Drawing, sp: &BuildingSpec) -> Vec<RoomPoint> {
    let k = insunits_per_ft(d);
    let mut out = Vec::new();
    let room_names: Vec<(&str, Option<&str>)> = sp.rooms.iter().map(|r| (r.name.as_str(), r.materials.wall.as_deref())).collect();

    for e in d.model.iter() {
        let (text, insert) = match &e.kind {
            EntityKind::Text(t) => (t.value.as_str(), t.insert),
            EntityKind::MText(t) => (t.contents.as_str(), t.insert),
            _ => continue,
        };
        let text = text.trim();
        for &(name, finish) in &room_names {
            if text.eq_ignore_ascii_case(name) {
                out.push(RoomPoint { name: name.to_string(), pos: [insert.x / k, insert.y / k], finish: finish.map(str::to_string) });
            }
        }
    }
    out
}

/// Determine which rooms are on side 1 and side 2 of a wall.
fn wall_rooms(centreline: &WallCentreline, rooms: &[RoomPoint]) -> (Option<String>, Option<String>) {
    let mid = centreline.mid;
    let n = centreline.normal;

    let mut best_s1: Option<(&RoomPoint, f64)> = None;
    let mut best_s2: Option<(&RoomPoint, f64)> = None;

    for rp in rooms {
        let dx = rp.pos[0] - mid[0];
        let dy = rp.pos[1] - mid[1];
        // Positive dot = side 1 (same direction as normal), negative = side 2.
        let dot = dx * n[0] + dy * n[1];
        let dist = (dx * dx + dy * dy).sqrt();
        if dot > 0.0 && best_s1.as_ref().is_none_or(|b| dist < b.1) {
            best_s1 = Some((rp, dist));
        } else if dot < 0.0 && best_s2.as_ref().is_none_or(|b| dist < b.1) {
            best_s2 = Some((rp, dist));
        }
    }

    (best_s1.map(|(r, _)| r.name.clone()), best_s2.map(|(r, _)| r.name.clone()))
}

// ---------------------------------------------------------------------------
// Layer drawing data
// ---------------------------------------------------------------------------

/// Compute layer boundary line segments for all walls in the drawing.
/// Each line runs the full length of the wall at a layer boundary offset from the side-1 face.
fn compute_layer_lines(d: &Drawing, sp: &BuildingSpec) -> Vec<([f64; 3], [f64; 3])> {
    let k = insunits_per_ft(d);
    let mut lines = Vec::new();
    for e in d.model.iter().take(MAX_WALLS) {
        if wallgen::made_as(e).as_deref() != Some("wall") {
            continue;
        }
        let EntityKind::Mesh(ref mesh) = e.kind else { continue };
        if mesh.vertices.is_empty() {
            continue;
        }
        let tags = wallgen_tags(e);
        let type_name = tags.iter().skip(3).find_map(|v| v.strip_prefix("type:").map(str::to_string));
        let Some(ref tn) = type_name else { continue };
        let wt = sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(tn));
        let Some(wt) = wt else { continue };
        let Some(ref asm) = wt.assembly else { continue };

        // Get bounding box in drawing units.
        let first = mesh.vertices.first().copied().unwrap_or(Vec3::ZERO);
        let (mut minx, mut miny, mut minz) = (first.x, first.y, first.z);
        let (mut maxx, mut maxy, mut maxz) = (first.x, first.y, first.z);
        for v in &mesh.vertices {
            if v.x < minx {
                minx = v.x;
            }
            if v.y < miny {
                miny = v.y;
            }
            if v.z < minz {
                minz = v.z;
            }
            if v.x > maxx {
                maxx = v.x;
            }
            if v.y > maxy {
                maxy = v.y;
            }
            if v.z > maxz {
                maxz = v.z;
            }
        }
        let dx = maxx - minx;
        let dy = maxy - miny;
        // The wall runs along the longer plan dimension.
        let (ax, ay, bx, by, nx, ny) = if dx >= dy {
            // Runs along X, normal along Y.
            (minx, (miny + maxy) * 0.5, maxx, (miny + maxy) * 0.5, 0.0, 1.0)
        } else {
            // Runs along Y, normal along X.
            ((minx + maxx) * 0.5, miny, (minx + maxx) * 0.5, maxy, 1.0, 0.0)
        };
        let total = asm.thickness_in();
        if total <= 0.0 {
            continue;
        }
        let offsets = layer_offsets_in(asm);
        for offset in &offsets {
            if *offset <= 0.0 || *offset >= total {
                continue;
            }
            // Offset from centre: (offset / total - 0.5) * thickness_drawing_units
            let d_from_centre = (offset / total - 0.5) * total * k / 12.0;
            let ox = ax + nx * d_from_centre;
            let oy = ay + ny * d_from_centre;
            let ox2 = bx + nx * d_from_centre;
            let oy2 = by + ny * d_from_centre;
            // Draw at z=0 (plan view) and at the wall top (for sections).
            lines.push(([ox, oy, minz], [ox2, oy2, minz]));
            lines.push(([ox, oy, maxz], [ox2, oy2, maxz]));
        }
    }
    lines
}

/// Compute the layer boundaries for a wall's assembly, in drawing units, for the renderer.
/// Returns offsets from the side-1 face for each layer boundary.
pub fn layer_offsets_in(asm: &Assembly) -> Vec<f64> {
    let mut offsets = vec![0.0];
    let mut pos = 0.0;
    for layer in &asm.layers {
        if !layer.in_cavity {
            pos += layer.thickness_in;
        }
        offsets.push(pos);
    }
    offsets
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

    #[test]
    fn csv_escape_handles_quotes() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("5/8\" Type X"), "\"5/8\"\" Type X\"");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
    }

    #[test]
    fn layer_offsets_correct() {
        let asm = Assembly {
            layers: vec![
                AssemblyLayer {
                    material: "5/8\" gypsum".into(),
                    thickness_in: 0.625,
                    side: LayerSide::Side1,
                    category: "board".into(),
                    in_cavity: false,
                    spacing_in: None,
                },
                AssemblyLayer {
                    material: "3 5/8\" studs".into(),
                    thickness_in: 3.625,
                    side: LayerSide::Core,
                    category: "framing".into(),
                    in_cavity: false,
                    spacing_in: Some(16.0),
                },
                AssemblyLayer {
                    material: "insulation".into(),
                    thickness_in: 3.5,
                    side: LayerSide::Core,
                    category: "insulation".into(),
                    in_cavity: true,
                    spacing_in: None,
                },
                AssemblyLayer {
                    material: "5/8\" gypsum".into(),
                    thickness_in: 0.625,
                    side: LayerSide::Side2,
                    category: "board".into(),
                    in_cavity: false,
                    spacing_in: None,
                },
            ],
            ..Assembly::default()
        };
        let offsets = layer_offsets_in(&asm);
        // 0, 0.625, 4.25, 4.25 (cavity doesn't add), 4.875
        assert_eq!(offsets.len(), 5);
        assert!((offsets[0] - 0.0).abs() < 0.001);
        assert!((offsets[1] - 0.625).abs() < 0.001);
        assert!((offsets[2] - 4.25).abs() < 0.001);
        // cavity: no thickness added
        assert!((offsets[3] - 4.25).abs() < 0.001);
        assert!((offsets[4] - 4.875).abs() < 0.001);
    }
}
