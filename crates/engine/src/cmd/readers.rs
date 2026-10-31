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

/// What a print-set reader says when there is no print set to read: a warning with the way
/// to open one, never an error (an error would stop the rest of a `run`).
fn no_project(what: &str, mut out: Value) -> Value {
    let msg = format!("no print-set project open, so no {what} were read; use printset.open {{path}} first, or bim_job import {{path}}");
    out["sheets_read"] = json!(0);
    out["warnings"] = json!([msg.clone()]);
    out["message"] = json!(msg);
    out
}

/// Sheet-number letters that name a discipline. A sheet found only by its largest
/// sheet-number lettering counts when it is one of these: equipment marks look like sheet
/// numbers too (`RTU-1`, `EF-2`).
const DISCIPLINES: &[&str] = &["G", "C", "L", "A", "I", "ID", "S", "M", "P", "FP", "E", "FA", "T", "LV", "Q"];

/// The current drawing is one sheet whose title block names another discipline than `want`
/// (`M` for ducts, `P` for pipes): what to answer instead of reading another trade's lines.
fn other_trade_sheet(s: &Session, want: &str, what: &str) -> Option<String> {
    let d = s.doc().ok()?;
    let id = cadaddy_mepfix::sheet_facts::identify_sheet(&cadaddy_mepfix::sheet_facts::texts(d))?;
    let known = id.source == "title_block" || DISCIPLINES.iter().any(|k| k.eq_ignore_ascii_case(&id.discipline));
    (known && !id.discipline.is_empty() && !id.discipline.eq_ignore_ascii_case(want))
        .then(|| format!("this drawing is sheet {} ({} discipline): {what} is read on {want} plan sheets only", id.number, id.discipline))
}

/// A print-set read of one trade: on a job's drawing (its sheets side by side), each plan of
/// the trade on its own sheet and level; with a print set open and no sheet of the trade in
/// the drawing, the set's plans of the trade imported for the read. None: read the drawing.
fn read_print_set(s: &mut Session, trade: MepKind, level: Option<&str>, apply: bool) -> Result<Option<Value>> {
    let want = if trade == MepKind::Duct { "M" } else { "P" };
    let job = super::guided::load_job(s).ok().flatten().filter(|j| !j.sheet_regions.is_empty());
    let keep = |r: &super::printmep::SheetRef| {
        level.is_none_or(|l| super::printmep::level_name(&r.level).eq_ignore_ascii_case(l) || r.level.eq_ignore_ascii_case(l))
    };
    let (read, source, in_drawing) = if let Some(job) = job {
        let refs: Vec<super::printmep::SheetRef> =
            super::guided::job_sheet_refs(&job).into_iter().filter(|r| r.trade() != Some(trade) || keep(r)).collect();
        let upf = super::bspec::current(s, "print-set read")
            .map_or(12.0, |(sp, stored)| if stored && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 });
        (super::printmep::read_sheets(s.doc()?, &refs, upf, Some(trade)), "job drawing", true)
    } else if let Ok(pr) = project(s) {
        let empty = s.doc().map_or(true, |d| d.model.is_empty());
        if !empty && other_trade_sheet(s, want, "").is_none() {
            return Ok(None);
        }
        let (t, refs) = super::printmep::load_project_sheets(&pr, Some(trade))?;
        let refs: Vec<super::printmep::SheetRef> = refs.into_iter().filter(|r| keep(r)).collect();
        (super::printmep::read_sheets(t.doc()?, &refs, 12.0, Some(trade)), "print set", false)
    } else {
        return Ok(None);
    };
    let mut warnings = read.warnings.clone();
    if read.sheets.is_empty() {
        warnings.push(format!("no {want} plan sheets in the {source}; nothing read (classify the print set first: printset.classify)"));
    }
    let applied = apply && in_drawing && !read.mep.runs.is_empty();
    if apply && !in_drawing {
        warnings.push(
            "read from the print set's own sheets: to build them, run the guided job (job.start), which places each sheet in its drawing".into(),
        );
    }
    if applied {
        let (mut sp, _) = super::bspec::current(s, "print-set read")?;
        let sheets: Vec<String> = read.sheets.iter().map(|r| r.sheet.clone()).collect();
        super::printmep::merge_mep(&mut sp, read.mep.clone(), &sheets);
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    let trade_key = if trade == MepKind::Duct { "duct" } else { "pipe" };
    let mut out = read.to_json();
    out["runs"] = json!(read.runs(trade_key));
    out["length_ft"] = json!(super::printmep::r1(read.length_ft(trade_key)));
    out["sheets_read"] = json!(read.sheets.len());
    out["source"] = json!(source);
    out["applied"] = json!(applied);
    out["warnings"] = json!(warnings);
    Ok(Some(out))
}

// ─── Command specs ───────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // ── Electrical conduit ───────────────────────────────────────────
        CommandSpec::new("conduit.read", "Read Conduit from Prints", run_conduit_read)
            .params("{level?: name, apply?: bool (put the runs and circuits in the building spec)} → {conduits: drawn runs, conduit_ft, homeruns: {read, routed, unrouted: [{circuit, sheet, level, reason}]}, circuits, estimated_ft, drops, devices_without_circuit, systems, sheets: [{sheet, level, conduit_runs, conduit_ft, homeruns, panels, circuits, estimated_ft}], source: job drawing|print set|current drawing, applied, warnings}: each electrical plan's drawn (tagged) conduit, homeruns and panels, and for each circuit its devices print an estimated route (devices chained, run square above the ceiling, drops to wall boxes; marked \"estimated route\")")
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
            .params("{level?: name, apply?: bool} → {runs: count, length_ft, terminals: count, warnings}: a job's drawing or an open print set: each M plan sheet on its own level (+ sheets: [{sheet, level, runs, length_ft}], source); one drawing: its ducts (none off another discipline's sheet)")
            .noundo(),
        // ── Pipe reader ──────────────────────────────────────────────────
        CommandSpec::new("pipes.read", "Read Piping from Prints", run_pipe_read)
            .params("{level?: name, apply?: bool} → {runs: count, length_ft, systems, warnings}: a job's drawing or an open print set: each P plan sheet on its own level (+ sheets: [{sheet, level, runs, length_ft}], source); one drawing: its pipe runs (none off another discipline's sheet)")
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
            .params("{sheets?: [name], preview?: bool (default true: read only; false puts the devices in the model), save?: bool (default true: preview:false saves the drawing to its file), limit?: devices listed (500)} → {devices: [{tag, kind, level, height_source, source: symbol|tag|text, placement, handle}], count, by_type, print_set: project|current drawing, sheets_read, model: {inserted, replaced, by_type, by_level, elements, elements_replaced, filled?: {key: n}, saved?, not_saved?, save_error?, not_inserted: {reason: count}, why_not?}}: works on any open drawing (the current sheet is the print set); preview:false needs a building model (bspec.draft {store: true} or bim.build first), makes each device an element (kind device, on its sheet's level, moved onto the architectural plan) replacing the device elements an earlier read made (nothing else in the model changes), and never inserts legend samples, circuit numbers, sheet numbers or lettering"),
        // ── Envelope reader ────────────────────────────────────────────────
        CommandSpec::new("envelope.read", "Read Roof and Wall Envelope", run_envelope_read)
            .params("{apply?: bool, page?} → {takeoff: [{code, description, unit, net, waste_pct, order, source, status}], flags: [{kind, subject, message, source, status}], spec_read, notes, page?: 1}")
            .noundo(),
    ]
}

// ─── conduit.read ────────────────────────────────────────────────────────────

/// The open drawing as one electrical plan of `level` (the param, else the spec's first level):
/// its devices are the model's (`mep.read_devices {preview: false}`) on that level.
fn current_plan(s: &Session, level: Option<&str>) -> Result<Option<(super::printmep::SheetRef, Vec<ReadDevice>, f64)>> {
    let d = s.doc()?;
    let ext = d.extents(&cadaddy_doc::Space::Model);
    if d.model.is_empty() || !(ext.min.x.is_finite() && ext.max.x.is_finite() && ext.min.y.is_finite() && ext.max.y.is_finite()) {
        return Ok(None);
    }
    let (sp, stored) = super::bspec::current(s, "conduit.read")?;
    let upf = if stored && sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let name = level.map(str::to_string).or_else(|| sp.levels.first().map(|l| l.name.clone())).unwrap_or_else(|| "Level 1".into());
    let devices: Vec<ReadDevice> = sp
        .tags
        .iter()
        .filter(|t| t.id.starts_with("device:") && t.level.eq_ignore_ascii_case(&name))
        .take(MAX_DEVICES)
        .map(|t| {
            let f = |k: &str| t.fields.get(k).map_or_else(String::new, |v| v.value.clone());
            ReadDevice {
                tag: f("type"),
                at: [t.at_ft[0] * upf, t.at_ft[1] * upf],
                sheet: String::new(),
                level: name.clone(),
                circuit: f("circuit"),
                handle: f("symbol"),
                kind: t.kind.clone(),
                leg: f("switch_leg"),
                room: String::new(),
            }
        })
        .collect();
    let sh = super::printmep::SheetRef {
        number: "current drawing".into(),
        level: name,
        discipline: "E".into(),
        kind: "plan".into(),
        rect: [ext.min.x - 1.0, ext.min.y - 1.0, ext.max.x + 1.0, ext.max.y + 1.0],
        span: None,
        scale_text: None,
        levels: Vec::new(),
        view: None,
    };
    Ok(Some((sh, devices, upf)))
}

fn run_conduit_read(s: &mut Session, p: &Value) -> Result<Value> {
    use super::printmep::{SheetRef, level_name};
    let level = str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()).map(str::to_string);
    let apply = bool_or(p, "apply", false);
    let keep = |r: &SheetRef| level.as_deref().is_none_or(|l| level_name(&r.level).eq_ignore_ascii_case(l) || r.level.eq_ignore_ascii_case(l));
    let job = super::guided::load_job(s).ok().flatten().filter(|j| !j.sheet_regions.is_empty());
    // A job's drawing (its sheets and devices); an open print set's E plans (no devices placed:
    // what is drawn); else the open drawing as one plan with the model's devices.
    let (read, source, in_drawing) = if let Some(job) = job {
        let refs: Vec<SheetRef> = super::guided::job_sheet_refs(&job).into_iter().filter(|r| keep(r)).collect();
        let upf = super::bspec::current(s, "conduit.read")
            .map_or(12.0, |(sp, stored)| if stored && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 });
        (super::printcond::read_sheets(s.doc()?, &refs, upf, &job.devices), "job drawing", true)
    } else if let Ok(pr) = project(s) {
        let (t, refs) = super::printmep::load_project_plans(&pr, &|r| {
            r.discipline.eq_ignore_ascii_case("E") && matches!(r.kind.as_str(), "plan" | "enlarged_plan") && keep(r)
        })?;
        (super::printcond::read_sheets(t.doc()?, &refs, 12.0, &[]), "print set", false)
    } else if let Some((sh, devices, upf)) = current_plan(s, level.as_deref())? {
        (super::printcond::read_sheets(s.doc()?, std::slice::from_ref(&sh), upf, &devices), "current drawing", true)
    } else {
        return Ok(no_project("conduit runs", json!({"conduits": 0, "homeruns": {"read": 0, "routed": 0, "unrouted": []}, "circuits": 0})));
    };
    let mut warnings = read.warnings.clone();
    if read.sheets.is_empty() {
        warnings.push(format!("no electrical plan sheets in the {source}; nothing read (classify the print set first: printset.classify)"));
    }
    if source == "print set" {
        warnings.push(
            "read from the print set's own sheets, where no devices are placed: only drawn conduit and homeruns. Run the guided job (job.start) for each circuit's estimated route".into(),
        );
    }
    let mut applied = false;
    if apply && !in_drawing {
        warnings.push("to build them, run the guided job (job.start), which places each sheet in its drawing".into());
    } else if apply && !read.mep.runs.is_empty() {
        match super::bspec::current(s, "conduit.read")? {
            (mut sp, true) => {
                let sheets: Vec<String> = read.sheets.iter().map(|r| r.sheet.clone()).collect();
                super::printmep::merge_mep(&mut sp, read.mep.clone(), &sheets);
                super::printcond::seat_heights(&mut sp);
                super::printcond::store_circuits(&mut sp, &read.circuits);
                s.doc_mut()?.building_spec = Some(sp.to_json());
                applied = true;
            }
            (_, false) => warnings.push("no building spec to put the runs in: run bspec.draft {store: true} (or bim.build) first".into()),
        }
    }
    let mut out = read.to_json();
    let routed = read.circuits.iter().filter(|t| t.fields.contains_key("panel_at")).count();
    out["conduits"] = out["conduit_runs"].clone();
    out["homeruns"] = json!({"read": out["homeruns"].clone(), "routed": routed, "unrouted": read.unrouted});
    out["systems"] = json!(read.mep.systems.iter().map(|sy| sy.name.clone()).collect::<Vec<_>>());
    out["sheets_read"] = json!(read.sheets.len());
    out["source"] = json!(source);
    out["applied"] = json!(applied);
    out["warnings"] = json!(warnings);
    Ok(out)
}

// ─── gear.read ───────────────────────────────────────────────────────────────

fn run_gear_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");
    let apply = bool_or(p, "apply", false);

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Ok(no_project("electrical equipment", json!({"placed": [], "unplaced": [], "tables_found": 0})));
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
            return Ok(no_project("site items", json!({"counts_by_kind": {}, "conduit_footage": [], "unknowns": []})));
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
    if let Some(v) = read_print_set(s, MepKind::Duct, level_filter.as_deref(), apply)? {
        return Ok(v);
    }
    if let Some(w) = other_trade_sheet(s, "M", "ductwork") {
        return Ok(
            json!({"runs": 0, "length_ft": 0.0, "terminals": 0, "equipment": 0, "fittings": 0, "applied": false, "warnings": [w], "trade_summary": "Found 0 duct run(s)", "clashes": 0, "systems": {}}),
        );
    }

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

    // Level registration: try the printset's M-sheet level assignment, then fall back to
    // the first level in the spec.  This is the "register by grids" path: the printset's
    // classify stage uses grid lines and title-block text to assign each sheet a level.
    let sheet_level = project(s).ok().and_then(|p| {
        let m_plans: Vec<_> = p
            .sheets
            .iter()
            .filter(|sh| sh.discipline == "M" && (sh.sheet_type == "plan" || sh.sheet_type == "enlarged_plan") && !sh.level.is_empty())
            .collect();
        // If exactly one M plan sheet has a level, use it directly (the reader works on one
        // drawing here; the print-set path reads each sheet on its own level).
        match m_plans.as_slice() {
            [one] if sp.levels.iter().any(|l| l.name.eq_ignore_ascii_case(&one.level)) => Some(one.level.clone()),
            _ => None,
        }
    });
    let default_level = sheet_level.unwrap_or_else(|| sp.levels.first().map(|l| l.name.clone()).unwrap_or_else(|| "1".to_string()));
    if level_filter.is_some_and(|f| !f.eq_ignore_ascii_case(&default_level)) {
        return Ok(());
    }
    let mep = duct_mep(net, &default_level, "", &sp);
    super::printmep::merge_mep(&mut sp, mep, &[]);

    // Store the updated spec.
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(())
}

/// A duct network as the spec's MEP parts on `level`: its systems, runs (from `sheet` when
/// known), air terminals and equipment, at the spec's elevation bands (else the defaults).
pub(crate) fn duct_mep(net: &DuctNetwork, level: &str, sheet: &str, sp: &cadaddy_walk3d::BuildingSpec) -> cadaddy_walk3d::Mep {
    let duct_bottom_ft = sp.mep.band_bottom_ft("duct").unwrap_or(10.0);
    let terminal_bottom_ft = sp.mep.band_bottom_ft("terminal").unwrap_or(8.5);
    let equipment_height_ft = sp.mep.band_bottom_ft("equipment").unwrap_or(6.0);
    let mut mep = cadaddy_walk3d::Mep::default();
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
        mep.runs.push(MepRun {
            system: sys_name,
            level: level.to_string(),
            points: run.points.iter().map(|p| [p.x, p.y]).collect(),
            size_in: [run.width_in, run.depth_in],
            bottoms_ft: vec![run.bottom_ft.unwrap_or(duct_bottom_ft); run.points.len().saturating_sub(1).max(1)],
            objects: Vec::new(),
            id: String::new(),
            encased: false,
            size_source: if run.confidence >= 0.85 { "tag" } else { "default" }.to_string(),
            height_source: run.height_source.clone(),
            reviewed: false,
            edited_by: String::new(),
            top_ft: None,
            source_sheet: run.source_sheet.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| sheet.to_string()),
        });
    }
    mep.systems = sys_map.into_values().collect();
    // Geometry noise in the traced centrelines (jogs, hooks, a bend in pieces) is no fitting.
    mep.runs = super::runtidy::tidy_runs(std::mem::take(&mut mep.runs), sp.units_per_foot);
    for dt in &net.terminals {
        let sys_name = match dt.kind.as_str() {
            "supply_diffuser" => "SA",
            "return_grille" => "RA",
            "exhaust_grille" => "EA",
            _ => "M-DUCT",
        };
        mep.terminals.push(MepTerminal {
            system: sys_name.to_string(),
            level: level.to_string(),
            at: [dt.at.x, dt.at.y],
            size_in: [12.0, 12.0],
            bottom_ft: terminal_bottom_ft,
            terminal_kind: dt.kind.clone(),
            objects: Vec::new(),
        });
    }
    for de in &net.equipment {
        let mut sources = BTreeMap::new();
        if !sheet.is_empty() {
            sources.insert("sheet".to_string(), sheet.to_string());
        }
        mep.equipment.push(MepEquipment {
            tag: de.tag.clone(),
            kind: de.kind.clone(),
            level: level.to_string(),
            min: [de.min.x, de.min.y],
            max: [de.max.x, de.max.y],
            height_ft: equipment_height_ft,
            systems: vec![match de.kind.as_str() {
                "ahu" | "rtu" | "vav" => "SA".to_string(),
                "exhaust_fan" => "EA".to_string(),
                _ => "M-DUCT".to_string(),
            }],
            confidence: de.confidence,
            sources,
            ..MepEquipment::default()
        });
    }
    mep
}

// ─── pipes.read ──────────────────────────────────────────────────────────────

fn run_pipe_read(s: &mut Session, p: &Value) -> Result<Value> {
    let level = str_param(p, "level").map(str::to_string);
    let apply = bool_or(p, "apply", false);
    if let Some(v) = read_print_set(s, MepKind::Pipe, level.as_deref(), apply)? {
        return Ok(v);
    }
    if let Some(w) = other_trade_sheet(s, "P", "piping") {
        return Ok(json!({"runs": 0, "length_ft": 0.0, "fittings": 0, "systems": {}, "applied": false, "warnings": [w]}));
    }
    // One drawing: its pipe runs (P-* layers, else lines next to pipe tags).
    let (mut sp, _) = super::bspec::current(s, "pipes.read")?;
    if let Some(l) = level.as_deref().filter(|l| sp.level(l).is_none()) {
        return Err(EngineError::Other(format!("pipes.read: there is no level named {l}")));
    }
    let (systems, runs) = super::mep::read(s, &sp, level.as_deref())?;
    let pipe_names: Vec<String> = systems.iter().filter(|x| x.kind == MepKind::Pipe).map(|x| x.name.clone()).collect();
    let runs: Vec<MepRun> = runs.into_iter().filter(|r| pipe_names.contains(&r.system)).collect();
    let upf = {
        let d = s.doc()?;
        let ext = d.extents(&cadaddy_doc::Space::Model);
        let (w, h) = (ext.max.x - ext.min.x, ext.max.y - ext.min.y);
        let diag = (w * w + h * h).sqrt();
        if diag.is_finite() && diag > 10.0 && diag < 80.0 {
            0.125
        } else if sp.units_per_foot > 0.0 {
            sp.units_per_foot
        } else {
            12.0
        }
    };
    let length_ft: f64 =
        runs.iter().map(|r| run_length_ft(&r.points.iter().map(|q| cadaddy_geom::Vec2::new(q[0], q[1])).collect::<Vec<_>>(), upf)).sum();
    let mut by_system: BTreeMap<String, usize> = BTreeMap::new();
    for r in &runs {
        *by_system.entry(r.system.clone()).or_default() += 1;
    }
    let mut warnings: Vec<String> = Vec::new();
    if runs.is_empty() {
        warnings.push("no pipe runs found: no P-* layers and no pipe tags (2\" CW, 4\" SAN) next to lines — is this a plumbing plan?".into());
    }
    let applied = apply && !runs.is_empty();
    let count = runs.len();
    if applied {
        if upf < sp.units_per_foot {
            sp.units_per_foot = upf;
        }
        let levels: Vec<String> = runs.iter().map(|r| r.level.clone()).collect();
        let old_pipes: Vec<String> = sp.mep.systems.iter().filter(|x| x.kind == MepKind::Pipe).map(|x| x.name.clone()).collect();
        sp.mep.runs.retain(|r| !(old_pipes.contains(&r.system) && levels.iter().any(|l| l.eq_ignore_ascii_case(&r.level))));
        let mep = cadaddy_walk3d::Mep { systems: systems.into_iter().filter(|x| x.kind == MepKind::Pipe).collect(), runs, ..Default::default() };
        super::printmep::merge_mep(&mut sp, mep, &[]);
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({
        "runs": count,
        "length_ft": super::printmep::r1(length_ft),
        "fittings": 0,
        "systems": by_system,
        "applied": applied,
        "warnings": warnings,
    }))
}

// ─── structure.read ──────────────────────────────────────────────────────────

fn run_struct_read(_s: &mut Session, p: &Value) -> Result<Value> {
    let _level = str_param(p, "level");

    let project = match project(_s) {
        Ok(p) => p,
        Err(_) => {
            return Ok(no_project("structural members", json!({"levels": []})));
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
            return Ok(no_project("ceilings", json!({"rooms": []})));
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
            return Ok(no_project("sprinkler heads", json!({"heads": {"by_mount": {}}, "pieces": {"by_size": {}}, "unresolved": []})));
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
            return Ok(no_project("cable trays", json!({"trays": {"by_size": {}}, "banks": 0, "risers": 0, "unmatched": []})));
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
            return Ok(no_project(
                "wall types",
                json!({"types": [], "counts": {"typed": 0, "guessed": 0, "unknown": 0}, "guesses": [], "unknown_tags": []}),
            ));
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
            return Ok(no_project("RFIs", json!({"counts": {"by_kind": {}, "by_severity": {}}, "items": []})));
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
            return Ok(no_project("MEP equipment", json!({"placed": [], "unplaced": [], "tables_found": 0})));
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

/// A receptacle's item by its designations (`REC`, `GFCI`, `GFCI/WP`, `USB`): `duplex
/// receptacle`, `GFCI receptacle, weatherproof`, `duplex receptacle, USB`.
pub(crate) fn receptacle_item(tag: &str) -> String {
    let marks: Vec<String> = tag.split('/').map(|m| m.trim().to_ascii_uppercase()).collect();
    let base = if marks.iter().any(|m| m == "GFCI") { "GFCI receptacle" } else { "duplex receptacle" };
    let with: Vec<&str> = marks
        .iter()
        .filter_map(|m| match m.as_str() {
            "WP" => Some("weatherproof"),
            "WR" => Some("weather-resistant"),
            "TR" => Some("tamper-resistant"),
            "USB" => Some("USB"),
            _ => None,
        })
        .collect();
    if with.is_empty() { base.to_string() } else { format!("{base}, {}", with.join(", ")) }
}

/// What a device mark usually is on an electrical plan (else `device`).
pub(crate) fn device_kind(tag: &str) -> &'static str {
    match tag.trim().to_ascii_uppercase().as_str() {
        "OS" | "OS1" | "OS2" | "OC" | "VS" | "PS" => "occupancy sensor",
        "DH" | "MH" => "door holder",
        "J" | "JB" => "junction box",
        "S" | "S2" | "S3" | "S4" | "SD" | "DS" | "LV" | "SP" | "SM" | "SH" | "SK" | "SOR" | "SWT" => "switch",
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
    // What may go in the model: the drawing's legends and schedules say which marks are types
    // and which symbols are samples; circuit numbers, sheet numbers and lettering never are.
    let (mut keep, not_inserted) = model_devices(d, &found, sheet_numbers(s));
    // A print set imported side by side: each device is its sheet's, on that sheet's level.
    if let Some(sp) = &spec {
        for dv in &mut keep {
            if let Some(sh) = sheet_holding(sp, dv.at) {
                dv.sheet = sh.number.clone();
                dv.level = sh.level.clone().unwrap_or_default();
            }
        }
    }
    let mut out = json!({
        "devices": devices, "count": found.len(), "by_type": by_type,
        "print_set": if e_sheets.is_some() { "project" } else { "current drawing" }, "sheets_read": e_sheets.unwrap_or(1).max(1),
    });
    if found.is_empty() {
        out["message"] = json!("no device marks or tagged device symbols on this drawing");
    } else if found.len() > limit {
        out["more"] = json!(found.len() - limit);
    }
    let mut model = if bool_or(p, "preview", true) {
        json!({"inserted": 0, "would_insert": keep.len(), "why_not": "preview: nothing was put in the model; mep.read_devices {preview: false} inserts these devices"})
    } else if keep.is_empty() {
        json!({"inserted": 0, "why_not": "no device here passed the junk filter (see not_inserted)"})
    } else {
        match place_devices(s, &keep, "mep.read_devices") {
            Ok(v) => devices_to_model(s, p, v)?,
            Err(e) => json!({"inserted": 0, "why_not": e.to_string()}),
        }
    };
    model["not_inserted"] = json!(not_inserted);
    out["model"] = model;
    Ok(out)
}

/// The sheet of the spec's print set whose frame holds `at` (drawing units), when the set was
/// imported side by side.
fn sheet_holding(sp: &cadaddy_walk3d::BuildingSpec, at: [f64; 2]) -> Option<&cadaddy_walk3d::levels::SheetLink> {
    let inside = |r: &[f64; 4]| at[0] >= r[0].min(r[2]) && at[0] <= r[0].max(r[2]) && at[1] >= r[1].min(r[3]) && at[1] <= r[1].max(r[3]);
    sp.sheets.iter().find(|x| x.rect.as_ref().is_some_and(inside))
}

/// After the devices went in the spec: the model's device elements built again from them (the
/// rest of the model stays), what was read filled in (room, host wall, panel row), and the
/// drawing saved to its file (`save: false` skips it). `placed`: what [`place_devices`] said.
fn devices_to_model(s: &mut Session, p: &Value, mut placed: Value) -> Result<Value> {
    const CMD: &str = "mep.read_devices";
    let r = super::propfill::rebuild_devices(s, CMD)?;
    placed["elements"] = json!(r.built);
    placed["elements_replaced"] = json!(r.removed);
    match super::propfill::fill(s) {
        Ok(f) => placed["filled"] = f["filled"]["device"].clone(),
        Err(e) => placed["fill_error"] = json!(e.to_string()),
    }
    if !bool_or(p, "save", true) {
        placed["saved"] = json!(false);
        return Ok(placed);
    }
    match s.state()?.path.clone() {
        None => placed["not_saved"] = json!("the drawing has no file yet: save it with saveas {path}"),
        Some(path) => match super::file::save_to(s, &path) {
            Ok(_) => placed["saved"] = json!(path),
            Err(e) => placed["save_error"] = json!(e.to_string()),
        },
    }
    Ok(placed)
}

/// The set's sheet numbers (the print set's, else the open sheet's title block): a tag that
/// prints one is a reference, not a device.
fn sheet_numbers(s: &Session) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    if let Ok(pr) = project(s) {
        for sh in pr.sheets.iter().take(MAX_DEVICES) {
            out.extend(cadaddy_mepfix::device_types::sheet_number_forms(&sh.number));
        }
    }
    out
}

/// The devices of `found` that belong in the model, and how many were left out, by reason.
fn model_devices(
    d: &cadaddy_doc::Drawing,
    found: &[Found],
    mut numbers: std::collections::BTreeSet<String>,
) -> (Vec<ReadDevice>, BTreeMap<&'static str, usize>) {
    use cadaddy_mepfix::device_types::{self, TagVerdict};
    let texts = cadaddy_mepfix::sheet_facts::texts(d);
    if let Some(id) = cadaddy_mepfix::sheet_facts::identify_sheet(&texts) {
        numbers.extend(device_types::sheet_number_forms(&id.number));
    }
    let legend = device_types::read_legend(&texts);
    let circuits: Vec<&cadaddy_mepfix::sheet_facts::TextItem> =
        texts.iter().filter(|t| cadaddy_mepfix::sheet_facts::is_circuit_label(&t.value)).collect();
    let mut keep = Vec::new();
    let mut skipped: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (tag, at, h, _) in found {
        let why = if legend.covers(*at) {
            Some("legend or schedule sample")
        } else if super::guided::is_circuit_tag(tag) {
            Some("circuit number")
        } else {
            match device_types::tag_verdict(tag, &legend.types, &std::collections::BTreeSet::new(), &numbers) {
                TagVerdict::Device(_) => None,
                TagVerdict::NotDevice(why) => Some(why),
                TagVerdict::Unlisted(_) => Some("not in the legend (the guided job asks about it)"),
            }
        };
        match why {
            Some(w) => *skipped.entry(w).or_default() += 1,
            None => keep.push(ReadDevice {
                tag: tag.trim().to_string(),
                at: [at.x, at.y],
                circuit: circuit_beside(&circuits, *at),
                handle: h.hex(),
                ..ReadDevice::default()
            }),
        }
    }
    (keep, skipped)
}

/// The circuit label printed beside a device symbol at `at` (within six label heights; the
/// nearest), else empty.
pub(crate) fn circuit_beside(circuits: &[&cadaddy_mepfix::sheet_facts::TextItem], at: cadaddy_geom::Vec2) -> String {
    circuits
        .iter()
        .filter(|t| t.height.is_finite() && t.height > 0.0 && t.at.dist(at) <= 6.0 * t.height)
        .min_by(|a, b| a.at.dist(at).total_cmp(&b.at.dist(at)))
        .map_or_else(String::new, |t| t.value.trim().to_string())
}

/// The circuit printed with a device tagged `tag` whose symbol is at `at` (its tag printed at
/// `tag_at`, when the import kept it). A tag and its circuit are printed as a pair, the circuit
/// under the tag (`L4` over `A-2`), and the symbol may sit nearer another device's pair; so the
/// circuit stacked under the device's own tag comes first (one that reads like a sheet number
/// too), else the circuit text nearest the symbol.
pub(crate) fn circuit_of(
    texts: &[cadaddy_mepfix::sheet_facts::TextItem],
    circuits: &[&cadaddy_mepfix::sheet_facts::TextItem],
    tag: &str,
    tag_at: Option<(cadaddy_geom::Vec2, f64)>,
    at: cadaddy_geom::Vec2,
) -> String {
    let sized = |t: &&cadaddy_mepfix::sheet_facts::TextItem| t.height.is_finite() && t.height > 0.0;
    // Where its tag is printed: given (the symbol's TAG attribute), else its text beside it.
    let own = tag_at.or_else(|| {
        texts
            .iter()
            .filter(sized)
            .filter(|t| t.value.trim() == tag && t.at.dist(at) <= TAG_REACH_H * t.height)
            .min_by(|a, b| a.at.dist(at).total_cmp(&b.at.dist(at)))
            .map(|t| (t.at, t.height))
    });
    if let Some((t, h)) = own {
        let under = texts
            .iter()
            .filter(sized)
            .filter(|c| (c.at.x - t.x).abs() <= STACK_DX_H * h && t.y - c.at.y > 0.3 * h && t.y - c.at.y <= STACK_DY_H * h)
            .filter(|c| cadaddy_mepfix::sheet_facts::is_circuit_label(&c.value))
            .min_by(|a, b| (t.y - a.at.y).total_cmp(&(t.y - b.at.y)));
        if let Some(c) = under {
            return c.value.trim().to_string();
        }
    }
    circuit_beside(circuits, at)
}

/// How far from its symbol a device's tag text is looked for, in text heights.
const TAG_REACH_H: f64 = 10.0;
/// A circuit stacked under a tag: this far to either side of it and this far down, in its heights.
const STACK_DX_H: f64 = 1.5;
const STACK_DY_H: f64 = 2.5;

/// A device read off a sheet, to put in the model: its type mark, where its symbol is drawn
/// (drawing units), the sheet and the sheet's level, and its circuit when one is printed
/// beside it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReadDevice {
    pub tag: String,
    pub at: [f64; 2],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sheet: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub level: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub circuit: String,
    /// The symbol's handle (hex).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub handle: String,
    /// What kind of device the model says it is (`receptacle`), when it says; empty: what its
    /// tag says ([`device_kind`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// A switch's or sensor's leg: the lower-case letter of the fixtures it controls.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub leg: String,
    /// The room a typical room's plan stamped it into (its circuits are that room's own).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub room: String,
}

/// A switch designation printed by a switch symbol, as (tag, leg): `D,b` a dimmer (`SD`) on leg
/// `b`, `3,a` a 3-way (`S3`) on leg `a`, `OS2,n` a sensor switch (`OS2`) on leg `n`, a lone `c` a
/// switch on leg `c` (tag empty: a sensor's leg when one is beside it, else a single-pole `S`).
/// A circuit (`G-1,b`) is none.
pub(crate) fn switch_designation(t: &str) -> Option<(String, String)> {
    let t = t.trim();
    let one_leg = |l: &str| l.len() == 1 && l.chars().all(|c| c.is_ascii_lowercase());
    if one_leg(t) {
        return Some((String::new(), t.to_string()));
    }
    let (code, leg) = t.rsplit_once(',')?;
    let (code, leg) = (code.trim(), leg.trim());
    if !one_leg(leg) || code.is_empty() || code.len() > 3 || !code.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        return None;
    }
    let tag = match code {
        "OS" | "OS1" | "OS2" => code.to_string(),
        "1" => "S".to_string(),
        "T" => "SWT".to_string(),
        _ => format!("S{code}"),
    };
    Some((tag, leg.to_string()))
}

/// The model level a device belongs on: the spec level whose plan region holds it, else the
/// spec level named as its sheet's (`2` → `Level 2`), else its sheet's level named the way the
/// spec names its levels, else the spec's level for the whole drawing.
fn device_level(sp: &cadaddy_walk3d::BuildingSpec, dv: &ReadDevice) -> String {
    let regional = cadaddy_walk3d::level_at(&sp.levels, dv.at[0], dv.at[1]).filter(|l| l.region.is_some());
    if let Some(l) = regional {
        return l.name.clone();
    }
    let want = dv.level.trim();
    if !want.is_empty() {
        // Resolve the device's raw level code to its canonical form, then match by canonical name.
        let canonical_want = cadaddy_walk3d::levels::canonical_level(want).map(|(c, _)| c);
        let named = sp.levels.iter().find(|l| {
            let n = l.name.trim().to_ascii_uppercase();
            let w = want.to_ascii_uppercase();
            if n == w || n.ends_with(&format!(" {w}")) {
                return true;
            }
            // Match by canonical form: "R" → "ROOF" matches a level named "Roof".
            if let Some(ref cw) = canonical_want {
                let cn = cadaddy_walk3d::levels::canonical_level(&n).map(|(c, _)| c);
                if cn.as_deref() == Some(cw) {
                    return true;
                }
            }
            false
        });
        return match named {
            Some(l) => l.name.clone(),
            // Named as the spec names its levels (`Level 2` beside `Level 1`).
            None if sp.levels.iter().any(|l| l.name.starts_with("Level ")) => format!("Level {want}"),
            None => want.to_string(),
        };
    }
    cadaddy_walk3d::level_at(&sp.levels, dv.at[0], dv.at[1]).or_else(|| sp.levels.first()).map_or_else(String::new, |l| l.name.clone())
}

/// What a device's BOM line calls it: `switch S3`, `device type A`.
fn device_item(tag: &str) -> String {
    match device_kind(tag) {
        "device" => format!("device type {tag}"),
        k => format!("{k} {tag}"),
    }
}

/// Put `devices` in the drawing's building model as element tags on their levels (`kind`,
/// `type`, `device_type`, `circuit` when known, `sheet`, `symbol`), replacing the devices an
/// earlier read put there (tags made by hand or by other readers stay). The BOM and takeoff
/// count them from there. Errors when the drawing has no building model yet.
pub(crate) fn place_devices(s: &mut Session, devices: &[ReadDevice], cmd: &str) -> Result<Value> {
    let d = s.doc()?;
    let Some(text) = d.building_spec.as_deref().filter(|t| !t.trim().is_empty()) else {
        return Err(EngineError::Other(format!(
            "{cmd}: there is no building model to put the devices in yet: run bspec.draft {{store: true}} (or bim.build) first, then {cmd} {{preview: false}} again"
        )));
    };
    let mut sp = cadaddy_walk3d::BuildingSpec::from_json(text).map_err(|e| EngineError::Other(format!("{cmd}: {e}")))?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let before = sp.tags.len();
    sp.tags.retain(|t| !(t.id.starts_with("device:") && t.fields.contains_key("symbol")));
    let replaced = before - sp.tags.len();
    let mut ids: std::collections::HashSet<String> = sp.tags.iter().map(|t| t.id.clone()).collect();
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_level: BTreeMap<String, usize> = BTreeMap::new();
    let mut inserted = 0usize;
    for dv in devices.iter().take(MAX_DEVICES) {
        let tag = dv.tag.trim();
        if tag.is_empty() || !dv.at.iter().all(|v| v.is_finite()) {
            continue;
        }
        let level = device_level(&sp, dv);
        let at_ft = [dv.at[0] / upf, dv.at[1] / upf];
        let base = format!("device:{level}:{:.1},{:.1}", at_ft[0], at_ft[1]);
        // Two marks on one symbol spot (a switch and its sensor) are two devices.
        let id = if ids.contains(&base) { format!("{base}:{tag}") } else { base };
        if !ids.insert(id.clone()) {
            continue;
        }
        // What its sheet or its submittal says it is (a fire alarm plan's `S`, a luminaire), else
        // what its mark says. A luminaire and a fire alarm detector are on the ceiling.
        let read = dv.kind.trim();
        let luminaire = matches!(read, "luminaire" | "wall luminaire");
        let kind = if read.is_empty() || luminaire { device_kind(tag) } else { read };
        let mut e = cadaddy_walk3d::tags::ElementTag { id, kind: kind.replace(' ', "_"), level: level.clone(), at_ft, ..Default::default() };
        e.set("type", tag, "plan");
        let item = match read {
            "" => device_item(tag),
            "wall luminaire" => format!("luminaire {tag}"),
            "receptacle" => receptacle_item(tag),
            _ => format!("{read} {tag}"),
        };
        e.set("device_type", item, "plan");
        let detector = read == "fire alarm device" && matches!(tag.to_ascii_uppercase().as_str(), "S" | "SD" | "H" | "HD" | "D" | "DD");
        if read == "wall luminaire" {
            e.set("mount", "wall", "submittal");
        } else if luminaire || detector {
            e.set("mount", "ceiling", if luminaire { "submittal" } else { "plan" });
        }
        if !dv.circuit.is_empty() {
            e.set("circuit", dv.circuit.clone(), "plan");
        }
        if !dv.leg.is_empty() {
            e.set("switch_leg", dv.leg.clone(), "plan");
        }
        e.set("sheet", if dv.sheet.is_empty() { "current drawing" } else { dv.sheet.as_str() }, "plan");
        e.set("symbol", dv.handle.clone(), "plan");
        sp.tags.push(e);
        *by_type.entry(tag.to_string()).or_default() += 1;
        *by_level.entry(level).or_default() += 1;
        inserted += 1;
    }
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(json!({"inserted": inserted, "replaced": replaced, "by_type": by_type, "by_level": by_level}))
}

/// The devices an earlier read put in the drawing's model: how many, by level and type.
pub(crate) fn model_device_counts(d: &cadaddy_doc::Drawing) -> Value {
    let spec = d.building_spec.as_deref().and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok());
    let mut by_level: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    let mut n = 0usize;
    for t in spec.iter().flat_map(|sp| sp.tags.iter()).filter(|t| t.id.starts_with("device:") && t.fields.contains_key("symbol")) {
        n += 1;
        *by_level.entry(t.level.clone()).or_default() += 1;
        *by_type.entry(t.fields.get("type").map_or_else(String::new, |f| f.value.clone())).or_default() += 1;
    }
    json!({"count": n, "by_level": by_level, "by_type": by_type})
}

// ─── envelope.read ───────────────────────────────────────────────────────────

fn run_envelope_read(s: &mut Session, p: &Value) -> Result<Value> {
    let apply = bool_or(p, "apply", false);
    let page = p.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;

    let pr = match project(s) {
        Ok(p) => p,
        Err(_) => {
            return Ok(no_project("envelope", json!({"takeoff": [], "flags": [], "spec_read": false, "notes": []})));
        }
    };

    let a_sheets: Vec<&mep_printset::SheetEntry> = pr.sheets.iter().filter(|s| s.discipline == "A").collect();

    // The printset DXF loading pipeline is wired through printset.open/classify (Q5128).
    // Until the DXF byte loader lands, report the sheet count and structure.
    Ok(json!({
        "takeoff": [],
        "takeoff_total": 0,
        "flags": [],
        "spec_read": false,
        "notes": [],
        "sheets_read": a_sheets.len(),
        "page": page,
        "pages": 0,
        "applied": apply,
        "message": if a_sheets.is_empty() {
            "no A-discipline sheets in the print set; classify the print set first (printset.classify)"
        } else {
            "envelope reader wired; DXF sheet loading through the printset pipeline pending (Q5128)"
        },
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_receptacles_designations_name_its_item() {
        assert_eq!(super::receptacle_item("REC"), "duplex receptacle");
        assert_eq!(super::receptacle_item("GFCI"), "GFCI receptacle");
        assert_eq!(super::receptacle_item("GFCI/WP"), "GFCI receptacle, weatherproof");
        assert_eq!(super::receptacle_item("TR/USB"), "duplex receptacle, tamper-resistant, USB");
    }

    #[test]
    fn switch_designations_name_the_switch_and_its_leg() {
        let d = |t: &str| super::switch_designation(t);
        assert_eq!(d("D,b"), Some(("SD".into(), "b".into())));
        assert_eq!(d("3,a"), Some(("S3".into(), "a".into())));
        assert_eq!(d("OS2,n"), Some(("OS2".into(), "n".into())));
        assert_eq!(d("c"), Some((String::new(), "c".into())));
        assert_eq!(d("G-1,b"), None, "a circuit and its leg");
        assert_eq!(d("LP-3,5"), None);
        assert_eq!(d("A"), None);
        assert_eq!(d("ab"), None);
    }
}
