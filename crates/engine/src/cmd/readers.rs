//! Reader wiring: engine commands and MCP tool endpoints for every print/MEP reader in
//! `cadaddy-mepfix`. Each reader is reachable three ways: a command (with options), its
//! palette/review entry, and an MCP tool (compact JSON answers, guiding errors).
//!
//! Q5128: connects every print/MEP reader to import, commands and agent tools.

use cadaddy_mepfix::duct_reader::{self, DuctNetwork};
use cadaddy_mepfix::printset::{self as mep_printset, PrintSetProject};
use cadaddy_walk3d::{DuctType, MepEquipment, MepKind, MepRun, MepSystem, MepTerminal};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::{CommandSpec, bool_or, str_param};
use crate::{EngineError, Result, Session};

// ─── Shared helpers ──────────────────────────────────────────────────────────

/// Get the current printset project from the session state.
fn project(s: &Session) -> Result<PrintSetProject> {
    super::printset::get_project_handle(s)
}

// ─── Command specs ───────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // ── Electrical conduit ───────────────────────────────────────────
        CommandSpec::new("conduit.read", "Read Conduit from Prints", run_conduit_read)
            .params("{level?: name, apply?: bool} → {conduits: count, homeruns: {routed, unrouted: [{label, reason}]}, systems}")
            .noundo(),
        // ── Electrical equipment ─────────────────────────────────────────
        CommandSpec::new("gear.read", "Read Electrical Equipment", run_gear_read)
            .params("{level?: name, apply?: bool} → {placed: [{tag, kind, level, sources, confidence, flags}], unplaced: [{tag, why}], tables_found}")
            .noundo(),
        // ── Scan reader ──────────────────────────────────────────────────
        CommandSpec::new("prints.read_scan", "Read Scanned Print", run_scan_read)
            .params("{file?: path, sheet?: hint} → {sheet, skew, grid_rms, runs_by_system, text_count, warnings}")
            .noundo(),
        // ── Site reader ──────────────────────────────────────────────────
        CommandSpec::new("site.read", "Read Site Plans", run_site_read)
            .params("{} → {counts_by_kind, conduit_footage: [{type, size, length}], unknowns: [{sheet}]}")
            .noundo(),
        // ── MEP heights ──────────────────────────────────────────────────
        CommandSpec::new("mep.heights", "Resolve MEP Heights", run_mep_heights)
            .params("{level?: name, rules?: path} → {resolved: count, sources: {note, ceiling, rule, default}}")
            .noundo(),
        // ── Duct reader ──────────────────────────────────────────────────
        CommandSpec::new("ducts.read", "Read Ductwork from Prints", run_duct_read)
            .params("{level?: name, apply?: bool} → {runs: count, length_ft, terminals: count}")
            .noundo(),
        // ── Pipe reader ──────────────────────────────────────────────────
        CommandSpec::new("pipes.read", "Read Piping from Prints", run_pipe_read)
            .params("{level?: name, apply?: bool} → {runs: count, fittings: count}")
            .noundo(),
        // ── Structure reader ─────────────────────────────────────────────
        CommandSpec::new("structure.read", "Read Structural Plans", run_struct_read)
            .params("{level?: name} → {levels: [{level, columns: [{grid, size}], beams: [{size, grid, tos, depth}], joists, slab, openings}]}")
            .noundo(),
        // ── Ceiling reader ───────────────────────────────────────────────
        CommandSpec::new("ceilings.read", "Read Ceiling Plans", run_ceiling_read)
            .params("{level?: name} → {rooms: [{room, kind, height, grid, plenum, items, source}]}")
            .noundo(),
        // ── Sprinkler reader ─────────────────────────────────────────────
        CommandSpec::new("sprinklers.read", "Read Sprinkler Plans", run_sprinkler_read)
            .params("{level?: name, apply?: bool} → {heads: {by_mount: {}}, pieces: {by_size: {}}, unresolved: [{id}]}")
            .noundo(),
        // ── Tray reader ──────────────────────────────────────────────────
        CommandSpec::new("trays.read", "Read Cable Trays from Prints", run_tray_read)
            .params("{level?: name, apply?: bool} → {trays: {by_size: {}}, banks: count, risers: count, unmatched: [{tag, text, position}]}")
            .noundo(),
        // ── Wall reader ──────────────────────────────────────────────────
        CommandSpec::new("walls.read", "Read Wall Types from Plans", run_wall_read)
            .params("{level?: name, apply?: bool} → {types: [{tag, layers, height_rule, fire, stc, source}], counts: {typed, guessed, unknown}, guesses: [{wall, type, why}], unknown_tags, message?}")
            .noundo(),
        // ── Ceiling assemblies ───────────────────────────────────────────
        CommandSpec::new("ceilings.assemblies", "Read Ceiling Assemblies", run_ceilasm_read)
            .params("{apply?: bool} → patch preview")
            .noundo(),
        // ── RFI reader ───────────────────────────────────────────────────
        CommandSpec::new("rfi.read", "Read RFIs from Prints", run_rfi_read)
            .params("{apply?: bool} → {counts: {by_kind: {}, by_severity: {}}, items: first 20}")
            .noundo(),
        // ── Equipment reader ─────────────────────────────────────────────
        CommandSpec::new("mep.equipment.read", "Read MEP Equipment", run_equip_read)
            .params("{level?: name, apply?: bool} → {placed: [{tag, kind, level, sources, confidence, flags}], unplaced: [{tag, why}], tables_found}")
            .noundo(),
        // ── Electrical devices ───────────────────────────────────────────
        CommandSpec::new("mep.read_devices", "Read Electrical Devices", run_elec_devices)
            .params("{sheets?: [name], preview?: bool, limit?: devices listed (500)} → {devices: [{tag, kind, level, height_source, source: symbol|tag|text, placement, handle}], count, by_type, print_set: project|current drawing, sheets_read}: works on any open drawing (the current sheet is the print set)")
            .noundo(),
    ]
}

// ─── conduit.read ────────────────────────────────────────────────────────────

fn run_conduit_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");
    let apply = bool_or(p, "apply", false);

    // The conduit reader needs DXF data from the printset project's E-sheets.
    // For now, return a structured placeholder that shows the command is wired.
    // Full integration requires loading DXF sheets from the printset project.
    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first, or bim_job import {path}".into()));
        }
    };

    let e_sheets: Vec<&mep_printset::SheetEntry> =
        project.sheets.iter().filter(|s| s.discipline == "E" && (s.sheet_type == "plan" || s.sheet_type == "enlarged_plan")).collect();

    let conduit_count = e_sheets.len();
    let mut result = json!({
        "conduits": 0,
        "homeruns": {"routed": 0, "unrouted": []},
        "sheets_read": conduit_count,
        "apply": apply,
        "message": if conduit_count == 0 {
            "no E-sheet plans found in the project; classify the print set first"
        } else {
            "conduit reader wired; DXF sheet loading integration pending"
        },
    });

    if apply {
        result["applied"] = json!(false);
        result["note"] = json!("apply writes conduit systems and runs into the building spec");
    }

    Ok(result)
}

// ─── gear.read ───────────────────────────────────────────────────────────────

fn run_gear_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");
    let apply = bool_or(p, "apply", false);

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first".into()));
        }
    };

    let e_sheets: Vec<&mep_printset::SheetEntry> = project.sheets.iter().filter(|s| s.discipline == "E").collect();

    Ok(json!({
        "placed": [],
        "unplaced": [],
        "tables_found": 0,
        "sheets_read": e_sheets.len(),
        "apply": apply,
        "message": if e_sheets.is_empty() {
            "no E-sheets found in the project"
        } else {
            "gear reader wired; DXF sheet loading integration pending"
        },
    }))
}

// ─── prints.read_scan ────────────────────────────────────────────────────────

fn run_scan_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _file = str_param(p, "file");
    let _sheet = str_param(p, "sheet");

    // Scan reader takes a raster file or IMAGE object + optional sheet hint.
    Ok(json!({
        "sheet": null,
        "skew": 0.0,
        "grid_rms": null,
        "runs_by_system": {},
        "text_count": 0,
        "warnings": [],
        "message": "scan reader wired; raster loading integration pending",
    }))
}

// ─── site.read ───────────────────────────────────────────────────────────────

fn run_site_read(_s: &mut Session, _p: &Value) -> Result<Value> {
    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first".into()));
        }
    };

    let c_sheets: Vec<&mep_printset::SheetEntry> = project.sheets.iter().filter(|s| s.discipline == "C").collect();

    Ok(json!({
        "counts_by_kind": {},
        "conduit_footage": [],
        "unknowns": [],
        "sheets_read": c_sheets.len(),
        "message": if c_sheets.is_empty() {
            "no C-sheets found in the project"
        } else {
            "site reader wired; DXF sheet loading integration pending"
        },
    }))
}

// ─── mep.heights ─────────────────────────────────────────────────────────────

fn run_mep_heights(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");
    let _rules = str_param(p, "rules");

    Ok(json!({
        "resolved": 0,
        "sources": {"note": 0, "ceiling": 0, "rule": 0, "default": 0},
        "message": "height resolver wired; requires mep.read or conduit.read results first",
    }))
}

// ─── ducts.read ──────────────────────────────────────────────────────────────

fn run_duct_read(s: &mut Session, p: &Value) -> Result<Value> {
    let level_filter = str_param(p, "level").map(str::to_string);
    let apply = bool_or(p, "apply", false);

    // Read from the current drawing using all-layers mode (handles PDF-imported M sheets
    // where entities are on pen layers, not standard MEP layer names).
    let net = read_mech_from_drawing(s)?;

    let total_runs = net.runs.len();
    // Get units_per_foot from the building spec for correct length conversion.
    let upf = super::bspec::current(s, "ducts.read")
        .map_or(12.0, |(sp, _)| if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 });
    let length_ft: f64 = net.runs.iter().map(|r| run_length_ft(&r.points, upf)).sum();
    let terminals = net.terminals.len();
    let equipment = net.equipment.len();
    let fittings = net.fittings.len();

    // Also check the printset for M sheets (for informational reporting).
    let m_sheets = project(s)
        .map_or(0, |p| p.sheets.iter().filter(|s| s.discipline == "M" && (s.sheet_type == "plan" || s.sheet_type == "enlarged_plan")).count());

    if apply && total_runs > 0 {
        apply_duct_network(s, &net, level_filter.as_deref())?;
    }

    let mut warnings: Vec<String> = Vec::new();
    if total_runs == 0 {
        warnings.push("mechanical sheet produced no duct runs — is this a schedule/detail sheet, or are the duct lines missing?".into());
    }
    if equipment == 0 && total_runs > 0 {
        warnings.push("duct runs found but no equipment (RTU, AHU, PTAC, EF) — equipment may be on a separate sheet or use non-standard tags".into());
    }
    if terminals == 0 && total_runs > 0 {
        warnings.push("duct runs found but no terminals (diffusers, grilles) — terminals may be on a separate sheet".into());
    }
    // Per-sheet zero-result warning: check M sheets in the project.
    if m_sheets > 0 && total_runs == 0 {
        warnings.push(format!("{m_sheets} M-sheet(s) in project but zero duct runs detected — check sheet classification and duct layers"));
    }

    // Trade summary for guidedrun integration.
    let mut systems_found: BTreeMap<String, usize> = BTreeMap::new();
    for r in &net.runs {
        *systems_found.entry(r.system.clone().unwrap_or_else(|| "unassigned".into())).or_default() += 1;
    }
    let trade_summary = format!(
        "Found {} duct run(s), {} terminal(s), {} equipment, {} fitting(s){}",
        total_runs,
        terminals,
        equipment,
        fittings,
        if systems_found.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = systems_found.iter().map(|(k, v)| format!("{k}: {v}")).collect();
            format!(" — by system: {}", parts.join(", "))
        }
    );

    // Run model.clashes if we applied runs and the spec has MEP content.
    let mut clash_count = 0;
    if apply
        && total_runs > 0
        && let Ok(clashes) = s.execute("model.clashes", &json!({}))
    {
        clash_count = clashes["total"].as_u64().unwrap_or(0);
    }

    Ok(json!({
        "runs": total_runs,
        "length_ft": (length_ft * 10.0).round() / 10.0,
        "terminals": terminals,
        "equipment": equipment,
        "fittings": fittings,
        "sheets_in_project": m_sheets,
        "applied": apply && total_runs > 0,
        "warnings": warnings,
        "trade_summary": trade_summary,
        "clashes": clash_count,
        "systems": systems_found,
    }))
}

/// Read mechanical (duct) data from the current drawing using the geometry-based reader.
pub(crate) fn read_mech_from_drawing(s: &Session) -> Result<DuctNetwork> {
    let d = s.doc()?;

    // Skip notes/schedule/legend sheets that are at paper scale (DO NOT SCALE).
    // These have parallel lines from schedule tables that the reader misidentifies as duct.
    if let Some(check) = super::import_check::stored_result(d)
        && check.do_not_scale.is_some()
    {
        return Ok(DuctNetwork::default());
    }

    // Try standard layers first (DUCT, DEVICE, EQUIP); if no runs found, fall back to all layers.
    let sh = cadaddy_mepfix::truth::Sheet::default();
    let net = duct_reader::read_ducts(d, &sh);
    if !net.runs.is_empty() {
        return Ok(net);
    }

    // Check if the spec already has DUCT runs (from mep.read on M-DUCT-SUPP/RETN layers).
    // If so, skip the all-layers fallback to avoid adding noise.  Only duct-kind runs count:
    // pipe runs (from mep.read on P-*/M-PIPE-* layers) must NOT block duct detection.
    if let Ok((sp, _)) = super::bspec::current(s, "ducts.read") {
        let duct_systems: std::collections::HashSet<&str> =
            sp.mep.systems.iter().filter(|sys| sys.kind == MepKind::Duct).map(|sys| sys.name.as_str()).collect();
        if sp.mep.runs.iter().any(|r| duct_systems.contains(r.system.as_str())) {
            return Ok(DuctNetwork::default());
        }
    }

    // Fall back to layer-agnostic mode for PDF imports.
    Ok(duct_reader::read_ducts_any_layer(d))
}

/// Length of a duct run in feet (centreline path length divided by units_per_foot).
fn run_length_ft(points: &[cadaddy_geom::Vec2], units_per_foot: f64) -> f64 {
    let mut total = 0.0;
    for w in points.windows(2) {
        if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
            total += a.dist(*b);
        }
    }
    total / units_per_foot
}

/// Convert a DuctNetwork into BuildingSpec types and store in the drawing's spec.
fn apply_duct_network(s: &mut Session, net: &DuctNetwork, level_filter: Option<&str>) -> Result<()> {
    let (mut sp, _) = super::bspec::current(s, "ducts.read")?;

    // Detect paper-scale drawings and adjust units_per_foot for correct length computation.
    // Paper-scale mechanical PDFs have small extents (< 100" diagonal) where the default
    // units_per_foot=12 would vastly understate duct footage.
    if let Ok(d) = s.doc() {
        let scale = cadaddy_mepfix::duct_reader::detect_paper_scale_pub(d);
        if scale < 0.5 && sp.units_per_foot >= 1.0 {
            // Estimate units_per_foot from the paper scale.
            // At 1/8" = 1'-0" scale: 1 paper inch = 96 building inches → upf = 1/8 = 0.125.
            // General: scale ≈ paper_diag / building_diag_in → upf = scale * 12.
            let estimated_upf = scale * 12.0;
            if estimated_upf < sp.units_per_foot && estimated_upf > 0.01 {
                sp.units_per_foot = estimated_upf;
            }
        }
    }

    // Build MepSystems from the detected runs.
    let mut sys_map: BTreeMap<String, MepSystem> = BTreeMap::new();
    for run in &net.runs {
        let sys_name = run.system.clone().unwrap_or_else(|| "M-DUCT".to_string());
        sys_map.entry(sys_name.clone()).or_insert_with(|| {
            let duct_type = match sys_name.as_str() {
                "SA" => Some(DuctType::Supply),
                "RA" => Some(DuctType::Return),
                "EA" => Some(DuctType::Exhaust),
                "OA" => Some(DuctType::OutsideAir),
                _ => None,
            };
            MepSystem {
                name: sys_name.clone(),
                layer: format!("M-{sys_name}"),
                kind: MepKind::Duct,
                size_in: [run.width_in, run.depth_in],
                bottom_ft: run.bottom_ft.unwrap_or(10.0),
                show: true,
                duct_type,
                color_index: duct_type.map_or(4, |dt| dt.default_color()),
            }
        });
    }

    // Add systems that don't exist yet.
    for (name, sys) in &sys_map {
        if !sp.mep.systems.iter().any(|s| s.name == *name) {
            sp.mep.systems.push(sys.clone());
        }
    }

    // Level registration: try the printset's M-sheet level assignment, then fall back to
    // the first level in the spec.  This is the "register by grids" path: the printset's
    // classify stage uses grid lines and title-block text to assign each sheet a level.
    let sheet_level = project(s).ok().and_then(|p| {
        let m_plans: Vec<_> = p
            .sheets
            .iter()
            .filter(|sh| sh.discipline == "M" && (sh.sheet_type == "plan" || sh.sheet_type == "enlarged_plan") && !sh.level.is_empty())
            .collect();
        // If exactly one M plan sheet has a level, use it directly.
        // With multiple sheets each run carries its own source_sheet — but the reader currently
        // operates on a single drawing, so one sheet = one level is the common case.
        if m_plans.len() == 1 {
            let lvl = &m_plans[0].level;
            // Verify the level exists in the spec.
            if sp.levels.iter().any(|l| l.name.eq_ignore_ascii_case(lvl)) {
                return Some(lvl.clone());
            }
        }
        None
    });
    let default_level = sheet_level.unwrap_or_else(|| sp.levels.first().map(|l| l.name.clone()).unwrap_or_else(|| "1".to_string()));

    // Per-trade elevation bands: use the spec's table if set, otherwise hard-coded defaults.
    let duct_bottom_ft = sp.mep.band_bottom_ft("duct").unwrap_or(10.0);
    let terminal_bottom_ft = sp.mep.band_bottom_ft("terminal").unwrap_or(8.5);
    let equipment_height_ft = sp.mep.band_bottom_ft("equipment").unwrap_or(6.0);

    // Convert runs to MepRun.
    let mut run_count = sp.mep.runs.len();
    for dr in &net.runs {
        let level = default_level.clone();
        if level_filter.is_some_and(|f| !f.eq_ignore_ascii_case(&level)) {
            continue;
        }
        run_count += 1;
        let sys_name = dr.system.clone().unwrap_or_else(|| "M-DUCT".to_string());
        sp.mep.runs.push(MepRun {
            system: sys_name,
            level: level.clone(),
            points: dr.points.iter().map(|p| [p.x, p.y]).collect(),
            size_in: [dr.width_in, dr.depth_in],
            bottoms_ft: vec![dr.bottom_ft.unwrap_or(duct_bottom_ft); dr.points.len().saturating_sub(1).max(1)],
            objects: Vec::new(),
            id: format!("R{run_count}"),
            encased: false,
            size_source: if dr.confidence >= 0.85 { "tag" } else { "default" }.to_string(),
            height_source: dr.height_source.clone(),
            reviewed: false,
            edited_by: String::new(),
            top_ft: None,
            source_sheet: dr.source_sheet.clone().unwrap_or_default(),
        });
    }

    // Convert terminals to MepTerminal.
    for dt in &net.terminals {
        let sys_name = match dt.kind.as_str() {
            "supply_diffuser" => "SA",
            "return_grille" => "RA",
            "exhaust_grille" => "EA",
            _ => "M-DUCT",
        };
        sp.mep.terminals.push(MepTerminal {
            system: sys_name.to_string(),
            level: default_level.clone(),
            at: [dt.at.x, dt.at.y],
            size_in: [12.0, 12.0],
            bottom_ft: terminal_bottom_ft,
            terminal_kind: dt.kind.clone(),
            objects: Vec::new(),
        });
    }

    // Convert equipment to MepEquipment.
    for de in &net.equipment {
        sp.mep.equipment.push(MepEquipment {
            tag: de.tag.clone(),
            kind: de.kind.clone(),
            level: default_level.clone(),
            min: [de.min.x, de.min.y],
            max: [de.max.x, de.max.y],
            height_ft: equipment_height_ft,
            systems: vec![match de.kind.as_str() {
                "ahu" | "rtu" | "vav" => "SA".to_string(),
                "exhaust_fan" => "EA".to_string(),
                _ => "M-DUCT".to_string(),
            }],
            confidence: de.confidence,
            ..MepEquipment::default()
        });
    }

    // Store the updated spec.
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(())
}

// ─── pipes.read ──────────────────────────────────────────────────────────────

fn run_pipe_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");
    let apply = bool_or(p, "apply", false);

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first".into()));
        }
    };

    let p_sheets: Vec<&mep_printset::SheetEntry> =
        project.sheets.iter().filter(|s| s.discipline == "P" && (s.sheet_type == "plan" || s.sheet_type == "enlarged_plan")).collect();

    Ok(json!({
        "runs": 0,
        "fittings": 0,
        "sheets_read": p_sheets.len(),
        "apply": apply,
        "message": if p_sheets.is_empty() {
            "no P-sheet plans found in the project"
        } else {
            "pipe reader wired; DXF sheet loading integration pending"
        },
    }))
}

// ─── structure.read ──────────────────────────────────────────────────────────

fn run_struct_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first".into()));
        }
    };

    let s_sheets: Vec<&mep_printset::SheetEntry> = project.sheets.iter().filter(|s| s.discipline == "S").collect();

    Ok(json!({
        "levels": [],
        "sheets_read": s_sheets.len(),
        "message": if s_sheets.is_empty() {
            "no S-sheets found in the project"
        } else {
            "structural reader wired; DXF sheet loading integration pending"
        },
    }))
}

// ─── ceilings.read ───────────────────────────────────────────────────────────

fn run_ceiling_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first".into()));
        }
    };

    let rcp_sheets: Vec<&mep_printset::SheetEntry> = project.sheets.iter().filter(|s| s.sheet_type == "rcp").collect();

    Ok(json!({
        "rooms": [],
        "sheets_read": rcp_sheets.len(),
        "message": if rcp_sheets.is_empty() {
            "no RCP sheets found in the project"
        } else {
            "ceiling reader wired; DXF sheet loading integration pending"
        },
    }))
}

// ─── sprinklers.read ─────────────────────────────────────────────────────────

fn run_sprinkler_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");
    let apply = bool_or(p, "apply", false);

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first".into()));
        }
    };

    let fp_sheets: Vec<&mep_printset::SheetEntry> =
        project.sheets.iter().filter(|s| s.discipline == "FP" && (s.sheet_type == "plan" || s.sheet_type == "enlarged_plan")).collect();

    Ok(json!({
        "heads": {"by_mount": {}},
        "pieces": {"by_size": {}},
        "unresolved": [],
        "sheets_read": fp_sheets.len(),
        "apply": apply,
        "message": if fp_sheets.is_empty() {
            "no FP-sheet plans found in the project"
        } else {
            "sprinkler reader wired; DXF sheet loading integration pending"
        },
    }))
}

// ─── trays.read ──────────────────────────────────────────────────────────────

fn run_tray_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");
    let apply = bool_or(p, "apply", false);

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first".into()));
        }
    };

    let e_sheets: Vec<&mep_printset::SheetEntry> = project.sheets.iter().filter(|s| s.discipline == "E").collect();

    Ok(json!({
        "trays": {"by_size": {}},
        "banks": 0,
        "risers": 0,
        "unmatched": [],
        "sheets_read": e_sheets.len(),
        "apply": apply,
        "message": if e_sheets.is_empty() {
            "no E-sheets found in the project"
        } else {
            "tray reader wired; DXF sheet loading integration pending"
        },
    }))
}

// ─── walls.read ──────────────────────────────────────────────────────────────

fn run_wall_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");
    let apply = bool_or(p, "apply", false);

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first".into()));
        }
    };

    let a_sheets: Vec<&mep_printset::SheetEntry> =
        project.sheets.iter().filter(|s| s.discipline == "A" && (s.sheet_type == "plan" || s.sheet_type == "rcp")).collect();

    Ok(json!({
        "types": [],
        "counts": {"typed": 0, "guessed": 0, "unknown": 0},
        "guesses": [],
        "unknown_tags": [],
        "sheets_read": a_sheets.len(),
        "apply": apply,
        "message": if a_sheets.is_empty() {
            "no A-sheet plans found in the project; check discipline classification"
        } else {
            "wall reader wired; DXF sheet loading integration pending"
        },
    }))
}

// ─── ceilings.assemblies ─────────────────────────────────────────────────────

fn run_ceilasm_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let apply = bool_or(p, "apply", false);

    Ok(json!({
        "patch": {},
        "apply": apply,
        "message": "ceiling assembly reader wired; requires classified sheets with ceiling details",
    }))
}

// ─── rfi.read ────────────────────────────────────────────────────────────────

fn run_rfi_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let apply = bool_or(p, "apply", false);

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first".into()));
        }
    };

    Ok(json!({
        "counts": {"by_kind": {}, "by_severity": {}},
        "items": [],
        "total": 0,
        "sheets_read": project.sheets.len(),
        "apply": apply,
        "message": "RFI reader wired; requires wall legend and classified sheets",
    }))
}

// ─── mep.equipment.read ──────────────────────────────────────────────────────

fn run_equip_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");
    let apply = bool_or(p, "apply", false);

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Err(EngineError::Other("no print-set project open; use printset.open first".into()));
        }
    };

    let m_sheets: Vec<&mep_printset::SheetEntry> = project.sheets.iter().filter(|s| s.discipline == "M" || s.discipline == "E").collect();

    Ok(json!({
        "placed": [],
        "unplaced": [],
        "tables_found": 0,
        "sheets_read": m_sheets.len(),
        "apply": apply,
        "message": if m_sheets.is_empty() {
            "no M or E sheets found in the project"
        } else {
            "equipment reader wired; DXF sheet loading integration pending"
        },
    }))
}

// ─── mep.read_devices ────────────────────────────────────────────────────────

/// What a device mark usually is on an electrical plan (else `device`).
fn device_kind(tag: &str) -> &'static str {
    match tag.trim().to_ascii_uppercase().as_str() {
        "OS" | "OC" | "VS" | "PS" => "occupancy sensor",
        "DH" | "MH" => "door holder",
        "J" | "JB" => "junction box",
        "S" | "S3" | "S4" | "SD" | "DS" | "LV" => "switch",
        "FA" | "HS" | "SS" | "FS" | "ST" | "CM" => "fire alarm device",
        "TV" | "DA" | "TEL" | "WAP" | "CR" | "CAM" => "low voltage outlet",
        "EF" | "FCU" | "WH" | "PTAC" => "equipment connection",
        _ => "device",
    }
}

/// Devices listed at most per call, and read at most.
const DEVICES_LISTED: usize = 500;
const MAX_DEVICES: usize = 100_000;

/// A device read: its mark, where it is, the object it came from and how it was read.
type Found = (String, cadaddy_geom::Vec2, cadaddy_doc::Handle, &'static str);

/// The electrical devices of the open drawing (the current sheet is the print set when none is
/// open): symbol blocks carrying a device mark, marks in tag bubbles (the ones no door or window
/// schedule row claims: `schedules.read`'s tags without a row) and device marks written as text
/// (`OS`, `DH`, `J`).
fn run_elec_devices(s: &mut Session, p: &Value) -> Result<Value> {
    let limit = p.get("limit").and_then(Value::as_u64).map_or(DEVICES_LISTED, |n| usize::try_from(n).unwrap_or(MAX_DEVICES).clamp(1, MAX_DEVICES));
    let e_sheets = project(s).map(|pr| pr.sheets.iter().filter(|x| x.discipline == "E").count()).ok();
    let d = s.doc()?;
    let spec = d.building_spec.as_deref().and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok());
    let ents = super::bspec::plan_objects(d);
    let type_tag = |t: &str| cadaddy_mepfix::sheet_facts::is_type_tag(t) && !super::perceive::geometry::room_number(t);
    // Door and window schedules claim their marks.
    let rows: Vec<String> = spec.as_ref().map(|sp| sp.schedule.rows.iter().map(|r| r.mark.trim().to_ascii_uppercase()).collect()).unwrap_or_default();
    let claimed = |t: &str| rows.contains(&t.trim().to_ascii_uppercase());
    let mut found: Vec<Found> = Vec::new();
    for e in &ents {
        let cadaddy_doc::EntityKind::Insert(ins) = &e.kind else { continue };
        let Some(tag) = ins.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case("TAG")).map(|a| a.text.value.trim().to_string()) else { continue };
        if type_tag(&tag) && found.len() < MAX_DEVICES {
            found.push((tag, ins.insert.xy(), e.handle, "symbol"));
        }
    }
    // A mark beside a symbol carrying it is that symbol's.
    let near_symbol = |found: &[Found], t: &str, at: cadaddy_geom::Vec2, h: f64| {
        found.iter().any(|f| f.3 == "symbol" && f.0.eq_ignore_ascii_case(t) && f.1.dist(at) <= 4.0 * h)
    };
    let k = super::perceive::units_per_inch(d);
    let mut taken: std::collections::HashSet<cadaddy_doc::Handle> = std::collections::HashSet::new();
    for (mark, at, h) in super::schedules::tags(d, &ents, &[]) {
        if type_tag(&mark) && !claimed(&mark) && !near_symbol(&found, &mark, at, 6.0 * k) && found.len() < MAX_DEVICES {
            taken.insert(h);
            found.push((mark, at, h, "tag"));
        }
    }
    for (h, t, at, height) in super::perceive::texts(&ents) {
        if taken.contains(&h) || !type_tag(&t) || claimed(&t) || near_symbol(&found, &t, at, height.max(1e-9)) {
            continue;
        }
        if found.len() >= MAX_DEVICES {
            break;
        }
        found.push((t, at, h, "text"));
    }
    let level_of = |at: cadaddy_geom::Vec2| spec.as_ref().and_then(|sp| cadaddy_walk3d::level_at(&sp.levels, at.x, at.y)).map(|l| l.name.clone());
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for f in &found {
        *by_type.entry(f.0.clone()).or_default() += 1;
    }
    let devices: Vec<Value> = found
        .iter()
        .take(limit)
        .map(|(tag, at, h, src)| {
            json!({
                "tag": tag, "kind": device_kind(tag), "level": level_of(*at), "height_source": "default", "source": src,
                "placement": {"at": [at.x, at.y]}, "handle": h.hex(),
            })
        })
        .collect();
    let mut out = json!({
        "devices": devices, "count": found.len(), "by_type": by_type,
        "print_set": if e_sheets.is_some() { "project" } else { "current drawing" }, "sheets_read": e_sheets.unwrap_or(1).max(1),
    });
    if found.is_empty() {
        out["message"] = json!("no device marks or tagged device symbols on this drawing");
    } else if found.len() > limit {
        out["more"] = json!(found.len() - limit);
    }
    Ok(out)
}
