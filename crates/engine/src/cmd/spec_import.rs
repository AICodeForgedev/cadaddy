//! Spec book, schedule and submittal import.
//!
//! - `bim.spec_import` — read a spec book PDF, schedule, or submittal into proposed BIM types.
//! - `bim.spec_accept` — merge accepted proposals into the drawing's type catalog.
//!
//! The agent workflow: `bim.spec_import(file)` → proposed catalog with citations → agent reviews
//! → `bim.spec_accept`. Nothing is applied silently.

use cadaddy_doc::bimspec::{
    self, BimType, Confidence, LocationRule, MAX_RFI_ITEMS, RfiItem, SpecField, SpecImportResult, SpecProposal, SubmittalRecord,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::*;
use crate::{Result, Session};

const CMD_IMPORT: &str = "bim.spec_import";
const CMD_ACCEPT: &str = "bim.spec_accept";

/// Maximum proposals per import (hostile files).
const MAX_PROPOSALS: usize = 5_000;
/// Maximum location rules per import.
const MAX_RULES: usize = 5_000;
/// Maximum submittals per import.
const MAX_SUBMITTALS: usize = 5_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_IMPORT, "Import Specifications", run_import)
            .params(
                "{file: string (path to PDF or text file), format?: \"spec\"|\"schedule\"|\"submittal\"|\"auto\" (default auto), text?: string (inline text instead of file)} → {proposals: [{mark, category, fields, origin, fileSource, locationRules?}], rfis: [{id, mark, field, specValue, scheduleValue, description, sources, severity}], locationRules: [{rule, location, material, source}], submittals: [{specSection, typeMarks, manufacturer, catalogNumber, dimensions, mounting, finish, ratings, reviewStatus, notes, fileSource}], notes: [string]}"
            )
            .noundo(),
        CommandSpec::new(CMD_ACCEPT, "Accept Specifications", run_accept)
            .params(
                "{proposals: [{mark, category?, fields?: {key: {value, source?, confidence?} | string}, locationRules?: [...]}], overwrite?: bool (default false, merge fields)} → {accepted, types: [{mark, category, fields}], skipped: [string]}"
            ),
    ]
}

// ── bim.spec_import ────────────────────────────────────────────────────────

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let format = p.get("format").and_then(Value::as_str).unwrap_or("auto");

    let text = if format == "sheet" {
        // Sheet mode reads from the drawing, not from text/file input.
        String::new()
    } else if let Some(t) = p.get("text").and_then(Value::as_str) {
        t.to_string()
    } else if let Some(path) = p.get("file").and_then(Value::as_str) {
        // Read file — PDF or text.
        if path.is_empty() {
            return Err(bad(CMD_IMPORT, "`file` path must not be empty"));
        }
        let bytes = crate::readfile::read(path).map_err(|e| bad(CMD_IMPORT, format!("{path}: {e}")))?;

        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".pdf") {
            // Extract text from PDF via the host's pdf reader.
            extract_pdf_text(&bytes, path)?
        } else {
            // Assume UTF-8 text.
            String::from_utf8(bytes).map_err(|e| bad(CMD_IMPORT, format!("{path}: {e}")))?
        }
    } else {
        return Err(bad(CMD_IMPORT, "either `file` (path), `text` (string), or `format: \"sheet\"` is required"));
    };

    if format != "sheet" && text.trim().is_empty() {
        return Err(bad(CMD_IMPORT, "the file or text is empty (no extractable text)"));
    }

    let sheet_id = p.get("sheet").and_then(Value::as_str).unwrap_or_default();

    // Check the drawing for existing types (to detect disagreements).
    let existing_types = s.doc().ok().map(|d| &d.bim_types).cloned().unwrap_or_default();

    let result = if format == "sheet" {
        // Read schedules from positioned TEXT entities in the current drawing.
        let drawing = s.doc().map_err(|_| bad(CMD_IMPORT, "no drawing open; `format: \"sheet\"` reads from the current drawing"))?;
        parse_schedule_from_drawing(drawing, sheet_id, &existing_types)
    } else {
        match format {
            "spec" => parse_spec_text(&text, &existing_types),
            "schedule" => parse_schedule_text(&text, &existing_types),
            "submittal" => parse_submittal_text(&text, &existing_types),
            _ => auto_detect_and_parse(&text, &existing_types),
        }
    };

    Ok(result_to_json(&result))
}

// ── bim.spec_accept ────────────────────────────────────────────────────────

fn run_accept(s: &mut Session, p: &Value) -> Result<Value> {
    let proposals = p.get("proposals").and_then(Value::as_array).ok_or_else(|| bad(CMD_ACCEPT, "`proposals` (array) is required"))?;
    let overwrite = p.get("overwrite").and_then(Value::as_bool).unwrap_or(false);

    let d = s.doc_mut()?;
    let mut accepted = 0u64;
    let mut types_out = Vec::new();
    let mut skipped = Vec::new();

    for prop in proposals.iter().take(MAX_PROPOSALS) {
        let mark = match prop.get("mark").and_then(Value::as_str) {
            Some(m) if !m.trim().is_empty() => m,
            _ => {
                skipped.push("(no mark)".to_string());
                continue;
            }
        };

        let existing = d.bim_types.get(mark).cloned();
        let mut ty = if overwrite { BimType::new(mark) } else { existing.unwrap_or_else(|| BimType::new(mark)) };

        if let Some(cat) = prop.get("category").and_then(Value::as_str)
            && !cat.is_empty()
        {
            ty.category = cat.to_string();
        }

        if let Some(fields) = prop.get("fields").and_then(Value::as_object) {
            for (key, val) in fields.iter().take(bimspec::MAX_FIELDS) {
                if let Ok(field) = parse_spec_field(val)
                    && (overwrite || !ty.fields.contains_key(key.as_str()))
                {
                    ty.set_field(key.clone(), field);
                }
            }
        }

        let out = type_to_json(&ty);
        d.bim_types.set(ty);
        types_out.push(out);
        accepted = accepted.saturating_add(1);
    }

    Ok(json!({
        "accepted": accepted,
        "types": types_out,
        "skipped": skipped,
    }))
}

// ── PDF text extraction ────────────────────────────────────────────────────

fn extract_pdf_text(bytes: &[u8], path: &str) -> Result<String> {
    let hooks = super::file::io().ok_or_else(|| bad(CMD_IMPORT, "PDF reading is not available in this build"))?;
    let read_pdf = hooks.read_pdf.ok_or_else(|| bad(CMD_IMPORT, "PDF import is not available in this build"))?;

    // Read up to 200 pages.
    let mut all_text = String::new();
    let max_pages = 200u32;

    // Try reading page 1 first to discover page count.
    let opts = json!({"page": 1, "text": true, "images": false});
    let (first_drawing, report) = (read_pdf)(bytes, &opts).map_err(|e| bad(CMD_IMPORT, format!("{path}: {e}")))?;

    let pages = report.get("pages").and_then(Value::as_u64).unwrap_or(1) as u32;
    all_text.push_str(&format!("--- page 1 of {pages} ---\n"));
    collect_texts(&first_drawing, &mut all_text);

    for pg in 2..=pages.min(max_pages) {
        let opts = json!({"page": pg, "text": true, "images": false});
        if let Ok((drawing, _)) = (read_pdf)(bytes, &opts) {
            all_text.push_str(&format!("\n--- page {pg} of {pages} ---\n"));
            collect_texts(&drawing, &mut all_text);
        }
    }

    if all_text.trim().is_empty() {
        return Err(bad(CMD_IMPORT, format!("{path}: no text could be extracted from the PDF")));
    }

    Ok(all_text)
}

/// Collect all text from a drawing's entities, sorted roughly top-to-bottom.
fn collect_texts(drawing: &cadaddy_doc::Drawing, out: &mut String) {
    let mut texts: Vec<(f64, String)> = Vec::new();
    for ent in drawing.model.iter() {
        match &ent.kind {
            cadaddy_doc::EntityKind::Text(t) => {
                if !t.value.trim().is_empty() {
                    texts.push((t.insert.y, t.value.clone()));
                }
            }
            cadaddy_doc::EntityKind::MText(t) => {
                let plain = strip_mtext_formatting(&t.contents);
                if !plain.trim().is_empty() {
                    texts.push((t.insert.y, plain));
                }
            }
            _ => {}
        }
    }
    // Sort top-to-bottom (highest Y first).
    texts.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, text) in texts {
        out.push_str(&text);
        out.push('\n');
    }
}

// ── sheet schedule reading (MUST 5) ───────────────────────────────────────

/// Read schedule tables from positioned TEXT/MTEXT entities in a drawing.
///
/// Groups text by Y position into rows, detects a header row matching known
/// schedule column names, and parses data rows into proposals.  The `sheet_id`
/// is recorded in each proposal's `file_source`.
fn parse_schedule_from_drawing(drawing: &cadaddy_doc::Drawing, sheet_id: &str, existing: &bimspec::BimTypes) -> SpecImportResult {
    let mut result = SpecImportResult::default();
    let mut rfi_counter = 0u32;

    // Collect text entities with their positions.
    struct PosTxt {
        x: f64,
        y: f64,
        h: f64,
        value: String,
    }
    let mut items: Vec<PosTxt> = Vec::new();
    for ent in drawing.model.iter().take(50_000) {
        match &ent.kind {
            cadaddy_doc::EntityKind::Text(t) => {
                let pt = t.justify_point();
                let v: String = t.value.trim().chars().take(500).collect();
                if pt.is_finite() && t.height.is_finite() && t.height > 0.0 && !v.is_empty() {
                    items.push(PosTxt { x: pt.x, y: pt.y, h: t.height, value: v });
                }
            }
            cadaddy_doc::EntityKind::MText(t) => {
                let plain = strip_mtext_formatting(&t.contents);
                let v: String = plain.trim().chars().take(500).collect();
                if t.insert.x.is_finite() && t.insert.y.is_finite() && t.height.is_finite() && t.height > 0.0 && !v.is_empty() {
                    items.push(PosTxt { x: t.insert.x, y: t.insert.y, h: t.height, value: v });
                }
            }
            _ => {}
        }
    }
    if items.is_empty() {
        return result;
    }

    // Determine row tolerance from the median text height.
    let mut heights: Vec<f64> = items.iter().map(|t| t.h).collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_h = heights.get(heights.len() / 2).copied().unwrap_or(1.0);
    let row_eps = median_h * 0.5;

    // Sort by Y descending (top first), then X ascending.
    items.sort_by(|a, b| b.y.partial_cmp(&a.y).unwrap_or(std::cmp::Ordering::Equal).then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal)));

    // Group into rows by Y tolerance.
    let mut rows: Vec<Vec<&PosTxt>> = Vec::new();
    for item in &items {
        let placed = rows.iter_mut().find(|r| r.first().is_some_and(|f| (f.y - item.y).abs() < row_eps));
        if let Some(row) = placed {
            row.push(item);
        } else {
            rows.push(vec![item]);
        }
    }
    // Sort items within each row by X.
    for row in &mut rows {
        row.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
    }

    // Find the header row: the first row where items match known schedule columns.
    let mut header_idx = None;
    let mut col_map: Vec<(f64, String)> = Vec::new(); // (x_pos, field_key)
    for (ri, row) in rows.iter().enumerate() {
        let mut matches: Vec<(f64, String)> = Vec::new();
        let mut has_mark = false;
        for item in row {
            let lower = item.value.to_ascii_lowercase();
            let lower = lower.trim();
            for &(header, field_key) in SCHEDULE_COLUMNS {
                if lower == header || lower.contains(header) {
                    if field_key == "mark" {
                        has_mark = true;
                    }
                    matches.push((item.x, field_key.to_string()));
                    break;
                }
            }
        }
        if has_mark && matches.len() >= 2 {
            header_idx = Some(ri);
            col_map = matches;
            break;
        }
    }

    let Some(hi) = header_idx else {
        return result;
    };

    // Determine schedule type from the title (row above the header, if any).
    let schedule_type = if hi > 0 {
        let title_text: String = rows.get(hi - 1).map(|r| r.iter().map(|t| t.value.as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        guess_schedule_type(&title_text)
    } else {
        "unknown".into()
    };

    let source = if sheet_id.is_empty() { "sheet".to_string() } else { format!("sheet {sheet_id}") };

    // Column X tolerance: use half the minimum horizontal gap between header columns.
    let col_eps = if col_map.len() >= 2 {
        let mut gaps: Vec<f64> = col_map.windows(2).map(|w| (w[1].0 - w[0].0).abs()).filter(|g| *g > 0.001).collect();
        gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        gaps.first().copied().unwrap_or(median_h * 5.0) * 0.4
    } else {
        median_h * 5.0
    };

    // Parse data rows (below the header).
    for row in rows.iter().skip(hi + 1) {
        let mut mark = String::new();
        let mut fields: BTreeMap<String, SpecField> = BTreeMap::new();

        for item in row {
            // Find the closest column header by X position.
            let closest = col_map.iter().min_by(|a, b| (a.0 - item.x).abs().partial_cmp(&(b.0 - item.x).abs()).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((cx, field_key)) = closest {
                if (cx - item.x).abs() > col_eps {
                    continue;
                }
                if field_key == "mark" {
                    mark = item.value.clone();
                } else {
                    let val = item.value.trim();
                    if !val.is_empty() && val != "-" && val != "\u{2014}" {
                        fields.insert(field_key.clone(), SpecField::new(val).with_source(&source));
                    }
                }
            }
        }

        if mark.is_empty() || fields.is_empty() {
            continue;
        }

        let mut ty = BimType::new(&mark);
        ty.category = schedule_type.clone();
        ty.fields = fields;

        // Check for disagreements with existing types.
        if let Some(existing_ty) = existing.get(&mark) {
            check_disagreements(&mark, existing_ty, &ty, "schedule", &source, &mut result.rfis, &mut rfi_counter);
        }

        if result.proposals.len() < MAX_PROPOSALS {
            result.proposals.push(SpecProposal { bim_type: ty, origin: "schedule".into(), file_source: source.clone(), ..Default::default() });
        }
    }

    result
}

/// Strip MTEXT formatting codes (\P, \f..., {\..}, etc.) to get plain text.
fn strip_mtext_formatting(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('P') | Some('p') => {
                    chars.next();
                    out.push('\n');
                }
                Some('~') => {
                    chars.next();
                    out.push(' ');
                }
                Some('{') | Some('}') => {
                    out.push(chars.next().unwrap_or(c));
                }
                Some('\\') => {
                    chars.next();
                    out.push('\\');
                }
                Some(_) => {
                    // Skip formatting code until ; or end.
                    chars.next();
                    while chars.peek().is_some_and(|&ch| ch != ';') {
                        chars.next();
                    }
                    if chars.peek() == Some(&';') {
                        chars.next();
                    }
                }
                None => {}
            }
        } else if c == '{' || c == '}' {
            // MTEXT grouping — skip.
        } else {
            out.push(c);
        }
    }
    out
}

// ── spec text parsing ──────────────────────────────────────────────────────

/// CSI section number pattern (e.g. "26 51 13", "26 05 33").
fn is_csi_section(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    // Match "SECTION 26 51 13" or "26 51 13" at the start of a line.
    if trimmed.len() >= 8 {
        let start = if trimmed.to_ascii_uppercase().starts_with("SECTION ") { &trimmed[8..] } else { trimmed };
        let parts: Vec<&str> = start.split_whitespace().collect();
        if parts.len() >= 3
            && parts[0].len() == 2
            && parts[0].chars().all(|c| c.is_ascii_digit())
            && parts[1].len() == 2
            && parts[1].chars().all(|c| c.is_ascii_digit())
            && parts[2].len() == 2
            && parts[2].chars().all(|c| c.is_ascii_digit())
        {
            // Return the section number portion.
            let end = start.find(|c: char| !c.is_ascii_digit() && c != ' ').unwrap_or(8.min(start.len()));
            return Some(start[..end.max(8)].trim());
        }
    }
    None
}

fn parse_spec_text(text: &str, existing: &bimspec::BimTypes) -> SpecImportResult {
    let mut result = SpecImportResult::default();
    let mut current_section = String::new();
    let mut current_page = String::new();
    let mut rfi_counter = 0u32;

    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Track page markers.
        if let Some(rest) = line.strip_prefix("--- page ") {
            if let Some(end) = rest.find(" of ") {
                current_page = format!("p.{}", &rest[..end]);
            }
            i += 1;
            continue;
        }

        // Detect CSI section headers.
        if let Some(section) = is_csi_section(line) {
            current_section = section.to_string();
            i += 1;
            continue;
        }

        // Look for type mark definitions: patterns like "Type H:", "TYPE A -", "Mark J2:"
        if let Some((mark, desc)) = extract_type_definition(line) {
            let source = format_source(&current_section, &current_page);
            let mut proposal =
                SpecProposal { bim_type: BimType::new(&mark), origin: "spec".into(), file_source: source.clone(), ..Default::default() };

            if !desc.is_empty() {
                proposal.bim_type.set_field(bimspec::keys::DESCRIPTION, SpecField::new(&desc).with_source(&source));
            }

            if !current_section.is_empty() {
                proposal.bim_type.set_field(bimspec::keys::SPEC_SECTION, SpecField::new(&current_section).with_source(&source));
            }

            // Guess category from section number.
            if !current_section.is_empty() {
                proposal.bim_type.category = category_from_section(&current_section);
            }

            // Scan following lines for manufacturer, model, ratings.
            let mut j = i + 1;
            while j < lines.len() && j < i + 20 {
                let next = lines[j].trim();
                if next.is_empty() || is_csi_section(next).is_some() || extract_type_definition(next).is_some() {
                    break;
                }
                parse_spec_detail(next, &mut proposal.bim_type, &source);
                j += 1;
            }

            // Check for disagreements with existing types.
            if let Some(existing_ty) = existing.get(&mark) {
                check_disagreements(&mark, existing_ty, &proposal.bim_type, "spec", &source, &mut result.rfis, &mut rfi_counter);
            }

            if result.proposals.len() < MAX_PROPOSALS {
                result.proposals.push(proposal);
            }
            i = j;
            continue;
        }

        // Look for location/wiring-method rules.
        if let Some(rule) = extract_location_rule(line, &current_section, &current_page)
            && result.location_rules.len() < MAX_RULES
        {
            result.location_rules.push(rule);
        }

        i += 1;
    }

    result
}

/// Extract a type mark definition from a line (e.g. "Type H: recessed 2x4 LED troffer").
fn extract_type_definition(line: &str) -> Option<(String, String)> {
    let upper = line.to_ascii_uppercase();

    // Pattern: "Type X:" or "TYPE X -" or "Mark X:" or "MARK X -"
    for prefix in &["TYPE ", "MARK "] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            // The mark is the next word (letters, digits, hyphens).
            let mark_end = rest.find(|c: char| c == ':' || c == '-' || c == '–' || c.is_whitespace()).unwrap_or(rest.len());
            if mark_end > 0 && mark_end <= 10 {
                let mark = line[prefix.len()..prefix.len() + mark_end].trim().to_string();
                // Description is everything after the delimiter.
                let desc_start = prefix.len() + mark_end;
                let desc = if desc_start < line.len() {
                    line[desc_start..].trim_start_matches(|c: char| c == ':' || c == '-' || c == '–' || c.is_whitespace()).trim().to_string()
                } else {
                    String::new()
                };
                if !mark.is_empty() && mark.chars().all(|c| c.is_alphanumeric() || c == '-') {
                    return Some((mark, desc));
                }
            }
        }
    }

    // Pattern: single letter/number mark followed by description after colon, like "H: recessed..."
    // Only if it looks like a fixture schedule entry and not a known detail field.
    if line.len() >= 3
        && let Some(colon_pos) = line.find(':')
    {
        let candidate = line[..colon_pos].trim();
        if !candidate.is_empty()
            && candidate.len() <= 6
            && candidate.chars().all(|c| c.is_alphanumeric() || c == '-')
            && candidate.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && !is_detail_prefix(candidate)
        {
            let desc = line[colon_pos + 1..].trim().to_string();
            if !desc.is_empty() {
                return Some((candidate.to_string(), desc));
            }
        }
    }

    None
}

/// Words that look like a mark but are really detail-field prefixes (case-insensitive).
fn is_detail_prefix(candidate: &str) -> bool {
    const PREFIXES: &[&str] =
        &["mfr", "mfg", "cct", "volts", "amps", "model", "mount", "finish", "color", "colour", "size", "weight", "nema", "rating"];
    let lower = candidate.to_ascii_lowercase();
    PREFIXES.iter().any(|p| lower == *p)
        || lower.starts_with("manufact")
        || lower.starts_with("catalog")
        || lower.starts_with("voltage")
        || lower.starts_with("amperage")
        || lower.starts_with("mounting")
        || lower.starts_with("material")
        || lower.starts_with("fire")
        || lower.starts_with("dimen")
}

/// Parse a detail line below a type definition, extracting manufacturer, model, etc.
fn parse_spec_detail(line: &str, ty: &mut BimType, source: &str) {
    let lower = line.to_ascii_lowercase();

    // "Manufacturer: Lithonia" or "Mfr: Lithonia"
    for prefix in &["manufacturer:", "mfr:", "mfg:"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let _ = rest; // use position from original line
            let idx = lower.find(prefix).unwrap_or(0) + prefix.len();
            let val = line[idx..].trim();
            if !val.is_empty() {
                ty.set_field(bimspec::keys::MANUFACTURER, SpecField::new(val).with_source(source));
            }
            return;
        }
    }

    // "Model: 2GTL4 4400LM" or "Catalog No: ..." or "Cat No: ..."
    for prefix in &["model:", "catalog no:", "catalog number:", "cat no:", "cat. no:", "catalog #:"] {
        if lower.starts_with(prefix) {
            let idx = prefix.len();
            let val = line[idx..].trim();
            if !val.is_empty() {
                ty.set_field(bimspec::keys::CATALOG_NUMBER, SpecField::new(val).with_source(source));
            }
            return;
        }
    }

    // "or equal" list: "or equal: Brand X Model Y, Brand Z Model W"
    if lower.contains("or equal") {
        let parts: Vec<&str> = line.splitn(2, ':').collect();
        if parts.len() == 2 {
            let val = parts[1].trim();
            if !val.is_empty() {
                ty.set_field(bimspec::keys::OR_EQUAL, SpecField::new(val).with_source(source));
            }
        }
        return;
    }

    // Voltage: "120/277V", "Voltage: 120V"
    if lower.starts_with("voltage:") || lower.starts_with("volts:") {
        let idx = lower.find(':').unwrap_or(0) + 1;
        let val = line[idx..].trim().trim_end_matches(|c: char| c.is_alphabetic());
        if !val.is_empty() {
            ty.set_field(bimspec::keys::VOLTS, SpecField::new(val.trim()).with_source(source));
        }
        return;
    }

    // Amperage: "20A", "Amps: 20"
    if lower.starts_with("amps:") || lower.starts_with("amperage:") {
        let idx = lower.find(':').unwrap_or(0) + 1;
        let val = line[idx..].trim().trim_end_matches(|c: char| c.is_alphabetic());
        if !val.is_empty() {
            ty.set_field(bimspec::keys::AMPS, SpecField::new(val.trim()).with_source(source));
        }
        return;
    }

    // Mounting: "Mounting: recessed" or "Mount: surface"
    if lower.starts_with("mounting:") || lower.starts_with("mount:") {
        let idx = lower.find(':').unwrap_or(0) + 1;
        let val = line[idx..].trim();
        if !val.is_empty() {
            ty.set_field(bimspec::keys::MOUNTING, SpecField::new(val).with_source(source));
        }
        return;
    }

    // Finish: "Finish: white"
    if lower.starts_with("finish:") || lower.starts_with("color:") || lower.starts_with("colour:") {
        let idx = lower.find(':').unwrap_or(0) + 1;
        let val = line[idx..].trim();
        if !val.is_empty() {
            ty.set_field(bimspec::keys::FINISH, SpecField::new(val).with_source(source));
        }
        return;
    }

    // CCT (correlated colour temperature): "3500K", "CCT: 4000K", "Color Temperature: 3500K"
    if lower.starts_with("cct:") || lower.starts_with("color temperature:") || lower.starts_with("colour temperature:") {
        let idx = lower.find(':').unwrap_or(0) + 1;
        let val = line[idx..].trim();
        if !val.is_empty() {
            ty.set_field(bimspec::keys::CCT, SpecField::new(val).with_source(source));
        }
        return;
    }

    // Material
    if lower.starts_with("material:") {
        let val = line[9..].trim();
        if !val.is_empty() {
            ty.set_field(bimspec::keys::MATERIAL, SpecField::new(val).with_source(source));
        }
        return;
    }

    // Fire rating
    if lower.starts_with("fire rating:") || lower.starts_with("fire-rating:") {
        let idx = lower.find(':').unwrap_or(0) + 1;
        let val = line[idx..].trim();
        if !val.is_empty() {
            ty.set_field(bimspec::keys::FIRE_RATING, SpecField::new(val).with_source(source));
        }
    }
}

/// Guess the BIM category from a CSI spec section number.
fn category_from_section(section: &str) -> String {
    let parts: Vec<&str> = section.split_whitespace().collect();
    if parts.is_empty() {
        return String::new();
    }
    match parts[0] {
        "26" => {
            // Division 26: Electrical
            if parts.len() >= 2 {
                match parts[1] {
                    "51" => "luminaire".into(),
                    "27" | "28" | "29" => "receptacle".into(),
                    "05" => "conduit".into(),
                    "24" => "switchboard".into(),
                    _ => "electrical".into(),
                }
            } else {
                "electrical".into()
            }
        }
        "23" => "hvac".into(),
        "22" => "plumbing".into(),
        "21" => "fire_protection".into(),
        "27" => "communications".into(),
        "28" => "safety".into(),
        "08" => "door".into(),
        "09" => "finish".into(),
        _ => String::new(),
    }
}

fn format_source(section: &str, page: &str) -> String {
    let mut parts = Vec::new();
    if !section.is_empty() {
        parts.push(format!("spec {section}"));
    }
    if !page.is_empty() {
        parts.push(page.to_string());
    }
    parts.join(" ")
}

// ── schedule text parsing ──────────────────────────────────────────────────

fn parse_schedule_text(text: &str, existing: &bimspec::BimTypes) -> SpecImportResult {
    let mut result = SpecImportResult::default();
    let mut rfi_counter = 0u32;

    // Detect schedule tables in text. Look for header rows with known column names.
    let lines: Vec<&str> = text.lines().collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();

        // Try to find a schedule header row.
        if let Some(columns) = detect_schedule_header(line) {
            let schedule_type = guess_schedule_type(line);
            let source_line = format!("schedule row near line {}", i + 1);

            // Read data rows after the header.
            let mut j = i + 1;
            while j < lines.len() {
                let row_line = lines[j].trim();
                if row_line.is_empty() || detect_schedule_header(row_line).is_some() {
                    break;
                }

                if let Some(proposal) = parse_schedule_row(row_line, &columns, &schedule_type, &source_line) {
                    // Check for disagreements with existing types.
                    if let Some(existing_ty) = existing.get(&proposal.bim_type.mark) {
                        check_disagreements(
                            &proposal.bim_type.mark,
                            existing_ty,
                            &proposal.bim_type,
                            "schedule",
                            &source_line,
                            &mut result.rfis,
                            &mut rfi_counter,
                        );
                    }
                    if result.proposals.len() < MAX_PROPOSALS {
                        result.proposals.push(proposal);
                    }
                }
                j += 1;
            }
            i = j;
            continue;
        }

        i += 1;
    }

    result
}

/// Known schedule column headers.
const SCHEDULE_COLUMNS: &[(&str, &str)] = &[
    ("mark", "mark"),
    ("type", "mark"),
    ("type mark", "mark"),
    ("description", "description"),
    ("desc", "description"),
    ("manufacturer", "manufacturer"),
    ("mfr", "manufacturer"),
    ("catalog", "catalogNumber"),
    ("catalog no", "catalogNumber"),
    ("cat no", "catalogNumber"),
    ("model", "catalogNumber"),
    ("volts", "volts"),
    ("voltage", "volts"),
    ("amps", "amps"),
    ("watts", "wattage"),
    ("wattage", "wattage"),
    ("mounting", "mounting"),
    ("mount", "mounting"),
    ("finish", "finish"),
    ("color", "finish"),
    ("colour", "finish"),
    ("cct", "cct"),
    ("color temp", "cct"),
    ("lumens", "lumens"),
    ("remarks", "description"),
    ("notes", "description"),
    ("nema", "nemaConfig"),
    ("rating", "fireRating"),
    ("fire rating", "fireRating"),
    ("material", "material"),
    ("size", "size"),
    ("height", "mountingHeight"),
    ("mounting height", "mountingHeight"),
    ("spec section", "specSection"),
    ("section", "specSection"),
];

fn detect_schedule_header(line: &str) -> Option<Vec<(usize, String)>> {
    let lower = line.to_ascii_lowercase();
    let mut found = Vec::new();

    // Split by tab or multiple spaces.
    let parts: Vec<(usize, &str)> = split_columns(line);

    for (col_idx, part) in &parts {
        let pl = part.to_ascii_lowercase();
        let pl = pl.trim();
        for &(header, field_key) in SCHEDULE_COLUMNS {
            if pl == header || pl.contains(header) {
                found.push((*col_idx, field_key.to_string()));
                break;
            }
        }
    }

    // Need at least a mark column and one other.
    let has_mark = found.iter().any(|(_, k)| k == "mark");
    if has_mark && found.len() >= 2 {
        Some(found)
    } else {
        // Also try if the line has "MARK" and "DESCRIPTION" (common luminaire schedule).
        if lower.contains("mark") && (lower.contains("description") || lower.contains("manufacturer")) {
            // Build column map from positions.
            let mut cols = Vec::new();
            for &(header, field_key) in SCHEDULE_COLUMNS {
                if let Some(pos) = lower.find(header) {
                    cols.push((pos, field_key.to_string()));
                }
            }
            cols.sort_by_key(|(pos, _)| *pos);
            cols.dedup_by(|a, b| a.1 == b.1);
            if cols.iter().any(|(_, k)| k == "mark") && cols.len() >= 2 {
                return Some(cols);
            }
        }
        None
    }
}

/// Split a line into column-index/value pairs (tab-separated or multi-space-separated).
fn split_columns(line: &str) -> Vec<(usize, &str)> {
    if line.contains('\t') {
        line.split('\t').enumerate().map(|(i, s)| (i, s.trim())).collect()
    } else {
        // Split on runs of 2+ spaces.
        let mut parts = Vec::new();
        let mut col = 0usize;
        for segment in line.split("  ") {
            let trimmed = segment.trim();
            if !trimmed.is_empty() {
                parts.push((col, trimmed));
                col += 1;
            }
        }
        parts
    }
}

fn guess_schedule_type(header_line: &str) -> String {
    let lower = header_line.to_ascii_lowercase();
    if lower.contains("luminaire") || lower.contains("fixture") || lower.contains("light") {
        "luminaire".into()
    } else if lower.contains("device") || lower.contains("receptacle") || lower.contains("switch") {
        "device".into()
    } else if lower.contains("panel") {
        "panel".into()
    } else if lower.contains("equipment") {
        "equipment".into()
    } else if lower.contains("plumbing") {
        "plumbing".into()
    } else if lower.contains("finish") {
        "finish".into()
    } else {
        "unknown".into()
    }
}

fn parse_schedule_row(line: &str, columns: &[(usize, String)], schedule_type: &str, source: &str) -> Option<SpecProposal> {
    let parts = split_columns(line);
    if parts.is_empty() {
        return None;
    }

    let mut mark = String::new();
    let mut fields: BTreeMap<String, SpecField> = BTreeMap::new();

    for (col_idx, field_key) in columns {
        if let Some((_, value)) = parts.iter().find(|(i, _)| i == col_idx) {
            let val = value.trim();
            if val.is_empty() || val == "-" || val == "—" {
                continue;
            }
            if field_key == "mark" {
                mark = val.to_string();
            } else {
                fields.insert(field_key.clone(), SpecField::new(val).with_source(source));
            }
        }
    }

    if mark.is_empty() {
        return None;
    }

    let mut ty = BimType::new(&mark);
    ty.category = schedule_type.to_string();
    ty.fields = fields;

    Some(SpecProposal { bim_type: ty, origin: "schedule".into(), file_source: source.to_string(), ..Default::default() })
}

// ── submittal text parsing ─────────────────────────────────────────────────

fn parse_submittal_text(text: &str, existing: &bimspec::BimTypes) -> SpecImportResult {
    let mut result = SpecImportResult::default();
    let mut rfi_counter = 0u32;

    let lines: Vec<&str> = text.lines().collect();
    let mut current_page = String::new();
    let mut current_submittal: Option<SubmittalRecord> = None;
    let mut current_types: Vec<SpecProposal> = Vec::new();
    // Per-type review stamps, parallel to current_types.
    let mut type_stamps: Vec<String> = Vec::new();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();

        // Track page markers.
        if let Some(rest) = line.strip_prefix("--- page ") {
            if let Some(end) = rest.find(" of ") {
                current_page = format!("p.{}", &rest[..end]);
            }
            i += 1;
            continue;
        }

        // Detect review stamps — apply to the LAST type in the current block.
        let upper = line.to_ascii_uppercase();
        if (upper.contains("APPROVED AS NOTED") || upper.contains("APPROVED-AS-NOTED")) && current_submittal.is_some() {
            if let Some(stamp) = type_stamps.last_mut() {
                *stamp = "approved_as_noted".into();
            }
            i += 1;
            continue;
        }
        if (upper == "APPROVED" || upper.starts_with("APPROVED ") || upper.contains("STAMP: APPROVED")) && current_submittal.is_some() {
            if let Some(stamp) = type_stamps.last_mut()
                && stamp.is_empty()
            {
                *stamp = "approved".into();
            }
            i += 1;
            continue;
        }
        if (upper.contains("REVISE AND RESUBMIT") || upper == "REVISE") && current_submittal.is_some() {
            if let Some(stamp) = type_stamps.last_mut() {
                *stamp = "revise".into();
            }
            i += 1;
            continue;
        }
        if (upper == "REJECTED" || upper.contains("STAMP: REJECTED")) && current_submittal.is_some() {
            if let Some(stamp) = type_stamps.last_mut() {
                *stamp = "rejected".into();
            }
            i += 1;
            continue;
        }

        // Detect submittal section headers.
        if upper.starts_with("SECTION ") || is_csi_section(line).is_some() {
            // Save any pending submittal.
            if let Some(sub) = current_submittal.take() {
                finalize_submittal(sub, &current_types, &type_stamps, existing, &mut result, &mut rfi_counter);
                current_types.clear();
                type_stamps.clear();
            }

            let section = is_csi_section(line).unwrap_or("").to_string();
            current_submittal = Some(SubmittalRecord { spec_section: section, file_source: current_page.clone(), ..Default::default() });
            i += 1;
            continue;
        }

        // If we're in a submittal, look for product details.
        if let Some(ref mut sub) = current_submittal {
            parse_submittal_detail(line, sub, &mut current_types, &mut type_stamps, &current_page);
        } else {
            // No section yet — try to start one from product data lines.
            if let Some((mark, desc)) = extract_type_definition(line) {
                current_submittal = Some(SubmittalRecord { file_source: current_page.clone(), ..Default::default() });
                let mut ty = BimType::new(&mark);
                ty.set_field(bimspec::keys::DESCRIPTION, SpecField::new(&desc).with_source(&current_page));
                current_types.push(SpecProposal {
                    bim_type: ty,
                    origin: "submittal".into(),
                    file_source: current_page.clone(),
                    ..Default::default()
                });
                type_stamps.push(String::new());
                if let Some(ref mut sub) = current_submittal {
                    sub.type_marks.push(mark);
                }
            }
        }

        i += 1;
    }

    // Finalize last submittal.
    if let Some(sub) = current_submittal.take() {
        finalize_submittal(sub, &current_types, &type_stamps, existing, &mut result, &mut rfi_counter);
    }

    result
}

fn parse_submittal_detail(line: &str, sub: &mut SubmittalRecord, types: &mut Vec<SpecProposal>, stamps: &mut Vec<String>, page: &str) {
    let lower = line.to_ascii_lowercase();

    // Type mark references — starts a new product block.
    if let Some((mark, desc)) = extract_type_definition(line) {
        if !sub.type_marks.contains(&mark) {
            sub.type_marks.push(mark.clone());
        }
        let mut ty = BimType::new(&mark);
        if !desc.is_empty() {
            ty.set_field(bimspec::keys::DESCRIPTION, SpecField::new(&desc).with_source(page));
        }
        types.push(SpecProposal { bim_type: ty, origin: "submittal".into(), file_source: page.to_string(), ..Default::default() });
        stamps.push(String::new());
        return;
    }

    // All detail fields apply to the LAST type only (the current product block).

    // Manufacturer.
    for prefix in &["manufacturer:", "mfr:", "mfg:", "manufactured by:"] {
        if lower.starts_with(prefix) {
            let val = line[prefix.len()..].trim();
            if !val.is_empty() {
                sub.manufacturer = val.to_string();
                if let Some(ty) = types.last_mut() {
                    ty.bim_type.set_field(bimspec::keys::MANUFACTURER, SpecField::new(val).with_source(page));
                }
            }
            return;
        }
    }

    // Catalog/model number.
    for prefix in &["model:", "catalog no:", "catalog number:", "cat no:", "cat. no:"] {
        if lower.starts_with(prefix) {
            let val = line[prefix.len()..].trim();
            if !val.is_empty() {
                sub.catalog_number = val.to_string();
                if let Some(ty) = types.last_mut() {
                    ty.bim_type.set_field(bimspec::keys::CATALOG_NUMBER, SpecField::new(val).with_source(page));
                }
            }
            return;
        }
    }

    // Dimensions.
    if lower.starts_with("dimensions:") || lower.starts_with("size:") {
        let idx = lower.find(':').unwrap_or(0) + 1;
        let val = line[idx..].trim();
        if !val.is_empty() {
            sub.dimensions = val.to_string();
            if let Some(ty) = types.last_mut() {
                ty.bim_type.set_field(bimspec::keys::DIMENSIONS, SpecField::new(val).with_source(page));
            }
        }
        return;
    }

    // Mounting.
    if lower.starts_with("mounting:") || lower.starts_with("mount:") {
        let idx = lower.find(':').unwrap_or(0) + 1;
        let val = line[idx..].trim();
        if !val.is_empty() {
            sub.mounting = val.to_string();
            if let Some(ty) = types.last_mut() {
                ty.bim_type.set_field(bimspec::keys::MOUNTING, SpecField::new(val).with_source(page));
            }
        }
        return;
    }

    // Finish.
    if lower.starts_with("finish:") || lower.starts_with("color:") || lower.starts_with("colour:") {
        let idx = lower.find(':').unwrap_or(0) + 1;
        let val = line[idx..].trim();
        if !val.is_empty() {
            sub.finish = val.to_string();
            if let Some(ty) = types.last_mut() {
                ty.bim_type.set_field(bimspec::keys::FINISH, SpecField::new(val).with_source(page));
            }
        }
        return;
    }

    // Ratings.
    if lower.starts_with("voltage:") || lower.starts_with("volts:") {
        let idx = lower.find(':').unwrap_or(0) + 1;
        let val = line[idx..].trim();
        if !val.is_empty() {
            if !sub.ratings.is_empty() {
                sub.ratings.push_str("; ");
            }
            sub.ratings.push_str(val);
            if let Some(ty) = types.last_mut() {
                ty.bim_type.set_field(bimspec::keys::VOLTS, SpecField::new(val).with_source(page));
            }
        }
    }
}

fn finalize_submittal(
    mut sub: SubmittalRecord,
    types: &[SpecProposal],
    per_type_stamps: &[String],
    existing: &bimspec::BimTypes,
    result: &mut SpecImportResult,
    rfi_counter: &mut u32,
) {
    // Set the SubmittalRecord's overall review_status from the per-type stamps
    // (most restrictive wins: rejected > revise > approved_as_noted > approved > unreviewed).
    for stamp in per_type_stamps {
        if !stamp.is_empty() && (sub.review_status.is_empty() || stamp == "rejected" || (stamp == "revise" && sub.review_status != "rejected")) {
            sub.review_status.clone_from(stamp);
        }
    }

    for (idx, proposal) in types.iter().enumerate() {
        let stamp = per_type_stamps.get(idx).map(|s| s.as_str()).unwrap_or("");
        let is_approved = stamp == "approved" || stamp == "approved_as_noted";
        let is_revise_or_rejected = stamp == "revise" || stamp == "rejected";

        let mut p = proposal.clone();

        // Mark submittal source on all fields.
        if !sub.file_source.is_empty() {
            p.file_source = sub.file_source.clone();
        }

        // For approved submittals, set source to indicate submittal authority.
        if is_approved {
            for field in p.bim_type.fields.values_mut() {
                if field.source.is_empty() {
                    field.source = format!("submittal ({stamp})");
                }
            }
            p.bim_type.set_field(bimspec::keys::REVIEW_STATUS, SpecField::new(stamp).with_source(&sub.file_source));
            p.bim_type.set_field(bimspec::keys::SUBMITTAL_SOURCE, SpecField::new(&sub.file_source));
        } else if !stamp.is_empty() {
            // Record the stamp even for non-approved types.
            p.bim_type.set_field(bimspec::keys::REVIEW_STATUS, SpecField::new(stamp).with_source(&sub.file_source));
        } else {
            // Unstamped: mark as unreviewed, not approved.
            p.bim_type.set_field(bimspec::keys::REVIEW_STATUS, SpecField::new("unreviewed").with_source(&sub.file_source));
        }

        // Check for mismatches between submittal and existing type.
        if let Some(existing_ty) = existing.get(&p.bim_type.mark) {
            check_disagreements(&p.bim_type.mark, existing_ty, &p.bim_type, "submittal", &sub.file_source, &mut result.rfis, rfi_counter);
        }

        if result.proposals.len() < MAX_PROPOSALS {
            // Only propose approved or unreviewed submittals for merging; revise/rejected are recorded only.
            if !is_revise_or_rejected {
                result.proposals.push(p);
            }
        }
    }

    if result.submittals.len() < MAX_SUBMITTALS {
        result.submittals.push(sub);
    }
}

// ── auto-detect ────────────────────────────────────────────────────────────

fn auto_detect_and_parse(text: &str, existing: &bimspec::BimTypes) -> SpecImportResult {
    let lower = text.to_ascii_lowercase();

    // Count cues for each format.
    let spec_cues = lower.matches("section ").count() + lower.matches("or equal").count() + lower.matches("per specification").count();
    let schedule_cues = lower.matches("mark").count() + lower.matches("schedule").count();
    let submittal_cues = lower.matches("approved").count()
        + lower.matches("submittal").count()
        + lower.matches("product data").count()
        + lower.matches("cut sheet").count()
        + lower.matches("shop drawing").count();

    // Try all three and merge results.
    if spec_cues >= schedule_cues && spec_cues >= submittal_cues {
        let mut merged = parse_spec_text(text, existing);
        // Also try schedule parsing on the same text — specs often contain schedules.
        let sched = parse_schedule_text(text, existing);
        merge_results(&mut merged, sched);
        merged
    } else if submittal_cues > spec_cues && submittal_cues >= schedule_cues {
        parse_submittal_text(text, existing)
    } else {
        let mut merged = parse_schedule_text(text, existing);
        // Also try spec parsing.
        let spec = parse_spec_text(text, existing);
        merge_results(&mut merged, spec);
        merged
    }
}

fn merge_results(target: &mut SpecImportResult, source: SpecImportResult) {
    // Only add proposals with marks not already in target.
    for prop in source.proposals {
        if !target.proposals.iter().any(|p| p.bim_type.mark.eq_ignore_ascii_case(&prop.bim_type.mark)) && target.proposals.len() < MAX_PROPOSALS {
            target.proposals.push(prop);
        }
    }
    for rfi in source.rfis {
        if target.rfis.len() < MAX_RFI_ITEMS {
            target.rfis.push(rfi);
        }
    }
    for rule in source.location_rules {
        if target.location_rules.len() < MAX_RULES {
            target.location_rules.push(rule);
        }
    }
    for sub in source.submittals {
        if target.submittals.len() < MAX_SUBMITTALS {
            target.submittals.push(sub);
        }
    }
    target.notes.extend(source.notes);
}

// ── location rules ─────────────────────────────────────────────────────────

/// Patterns for wiring method / location rules in spec text and general notes.
fn extract_location_rule(line: &str, section: &str, page: &str) -> Option<LocationRule> {
    let lower = line.to_ascii_lowercase();

    // Patterns: "MC cable permitted concealed in walls", "EMT exposed", "PVC below slab",
    // "rigid conduit in wet locations", etc.
    let raceway_keywords = [
        "mc cable",
        "mc ",
        "emt ",
        "emt,",
        "rigid conduit",
        "rigid metallic",
        "imc ",
        "pvc ",
        "pvc conduit",
        "rnmc",
        "lfmc",
        "lfnc",
        "fmc ",
        "wiremold",
        "surface raceway",
    ];

    let location_keywords = [
        "concealed",
        "exposed",
        "below slab",
        "above ceiling",
        "in walls",
        "wet location",
        "damp location",
        "underground",
        "exterior",
        "above accessible ceiling",
        "in concrete",
        "direct burial",
    ];

    let mut found_material = String::new();
    for kw in &raceway_keywords {
        if lower.contains(kw) {
            found_material = kw.trim().to_string();
            break;
        }
    }

    if found_material.is_empty() {
        return None;
    }

    let mut found_location = String::new();
    for kw in &location_keywords {
        if lower.contains(kw) {
            found_location = kw.to_string();
            break;
        }
    }

    let source = format_source(section, page);
    if !source.is_empty() || !found_location.is_empty() {
        Some(LocationRule { rule: line.trim().to_string(), location: found_location, material: found_material, source })
    } else {
        None
    }
}

// ── disagreement detection ─────────────────────────────────────────────────

fn check_disagreements(mark: &str, existing: &BimType, proposed: &BimType, origin: &str, source: &str, rfis: &mut Vec<RfiItem>, counter: &mut u32) {
    // Compare common fields.
    for (key, new_field) in &proposed.fields {
        if let Some(old_field) = existing.get_field(key)
            && !old_field.value.eq_ignore_ascii_case(&new_field.value)
            && !old_field.value.is_empty()
            && !new_field.value.is_empty()
        {
            if rfis.len() >= MAX_RFI_ITEMS {
                break;
            }
            *counter = counter.saturating_add(1);
            let severity = disagreement_severity(key);

            let mut rfi = RfiItem {
                id: format!("RFI-{counter:03}"),
                mark: mark.to_string(),
                field: key.clone(),
                description: format!("{key} for type {mark}: existing \"{}\", {origin} says \"{}\"", old_field.value, new_field.value),
                severity,
                ..Default::default()
            };

            match origin {
                "spec" => {
                    rfi.spec_value = new_field.value.clone();
                    rfi.schedule_value = old_field.value.clone();
                }
                "schedule" => {
                    rfi.schedule_value = new_field.value.clone();
                    rfi.spec_value = old_field.value.clone();
                }
                "submittal" => {
                    rfi.submittal_value = new_field.value.clone();
                    rfi.spec_value = old_field.value.clone();
                }
                _ => {}
            }

            let mut sources = Vec::new();
            if !old_field.source.is_empty() {
                sources.push(old_field.source.clone());
            }
            if !source.is_empty() {
                sources.push(source.to_string());
            }
            rfi.sources = sources;
            rfis.push(rfi);
        }
    }
}

fn disagreement_severity(field: &str) -> String {
    match field {
        bimspec::keys::VOLTS | bimspec::keys::AMPS | bimspec::keys::FIRE_RATING | bimspec::keys::NEMA_CONFIG | bimspec::keys::WET_DAMP => {
            "high".into()
        }
        bimspec::keys::MANUFACTURER | bimspec::keys::CATALOG_NUMBER | bimspec::keys::MATERIAL | bimspec::keys::SPEC_SECTION => "medium".into(),
        _ => "low".into(),
    }
}

// ── JSON conversion helpers ────────────────────────────────────────────────

fn parse_spec_field(val: &Value) -> std::result::Result<SpecField, ()> {
    match val {
        Value::String(s) => Ok(SpecField::new(s.clone())),
        Value::Object(obj) => {
            let value = obj.get("value").and_then(Value::as_str).ok_or(())?;
            let source = obj.get("source").and_then(Value::as_str).unwrap_or_default();
            let confidence = obj.get("confidence").and_then(Value::as_str).map(Confidence::parse).unwrap_or_default();
            Ok(SpecField::new(value).with_source(source).with_confidence(confidence))
        }
        _ => Err(()),
    }
}

fn type_to_json(ty: &BimType) -> Value {
    json!({
        "mark": ty.mark,
        "category": ty.category,
        "fields": fields_to_json(&ty.fields),
    })
}

fn fields_to_json(fields: &BTreeMap<String, SpecField>) -> Value {
    let mut obj = serde_json::Map::new();
    for (key, field) in fields {
        let mut f = serde_json::Map::new();
        f.insert("value".into(), json!(field.value));
        if !field.source.is_empty() {
            f.insert("source".into(), json!(field.source));
        }
        if field.confidence != Confidence::Stated {
            f.insert("confidence".into(), json!(field.confidence.as_str()));
        }
        obj.insert(key.clone(), Value::Object(f));
    }
    Value::Object(obj)
}

fn result_to_json(result: &SpecImportResult) -> Value {
    let proposals: Vec<Value> = result
        .proposals
        .iter()
        .map(|p| {
            let mut v = type_to_json(&p.bim_type);
            if let Some(obj) = v.as_object_mut() {
                if !p.origin.is_empty() {
                    obj.insert("origin".into(), json!(p.origin));
                }
                if !p.file_source.is_empty() {
                    obj.insert("fileSource".into(), json!(p.file_source));
                }
                if !p.location_rules.is_empty() {
                    obj.insert("locationRules".into(), json!(p.location_rules));
                }
            }
            v
        })
        .collect();

    json!({
        "proposals": proposals,
        "rfis": result.rfis,
        "locationRules": result.location_rules,
        "submittals": result.submittals,
        "notes": result.notes,
    })
}

#[cfg(test)]
mod tests {
    use crate::Session;
    use serde_json::{Value, json};

    const EMPTY: &[Value] = &[];

    fn arr(v: &Value) -> &[Value] {
        v.as_array().map(|a| a.as_slice()).unwrap_or(EMPTY)
    }

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).ok();
        s
    }

    // ── spec text parsing tests ────────────────────────────────────────────

    #[test]
    fn parse_spec_book_with_types() {
        let mut s = session();
        let spec_text = "\
SECTION 26 51 13 - INTERIOR LUMINAIRES

Type H: Recessed 2x4 LED troffer
Manufacturer: Pinnacle Lighting
Catalog No: RT-2400-LED-4000LM
Voltage: 120/277V
CCT: 4000K
Mounting: Recessed
Finish: White

Type J: Surface-mounted LED strip
Manufacturer: Pinnacle Lighting
Model: SL-4800-5000K
Voltage: 120V
CCT: 5000K
Mounting: Surface
Finish: Silver

SECTION 26 27 26 - WIRING DEVICES

Type A: Duplex receptacle
Manufacturer: Beacon Electric
Catalog No: BR-5362
Voltage: 125V
Amps: 20
NEMA Configuration: 5-20R
";
        let r = s
            .execute(
                "bim.spec_import",
                &json!({
                    "text": spec_text, "format": "spec"
                }),
            )
            .unwrap_or_default();

        let proposals = arr(&r["proposals"]);
        assert!(proposals.len() >= 3, "should find at least 3 types, got {}", proposals.len());

        // Check type H.
        let h = proposals.iter().find(|p| p["mark"] == "H");
        assert!(h.is_some(), "should find type H");
        let h = h.unwrap();
        assert_eq!(h["category"], "luminaire");
        assert_eq!(h["fields"]["manufacturer"]["value"], "Pinnacle Lighting");
        assert_eq!(h["fields"]["catalogNumber"]["value"], "RT-2400-LED-4000LM");
        assert_eq!(h["fields"]["cct"]["value"], "4000K");
        assert_eq!(h["fields"]["mounting"]["value"], "Recessed");

        // Check type A.
        let a = proposals.iter().find(|p| p["mark"] == "A");
        assert!(a.is_some(), "should find type A");
        let a = a.unwrap();
        assert_eq!(a["fields"]["amps"]["value"], "20");
    }

    #[test]
    fn parse_spec_extracts_location_rules() {
        let mut s = session();
        let spec_text = "\
SECTION 26 05 33 - RACEWAY AND BOXES

MC cable permitted concealed in walls above accessible ceilings
EMT exposed in mechanical rooms
PVC conduit below slab and underground
Rigid metallic conduit in wet locations
";
        let r = s
            .execute(
                "bim.spec_import",
                &json!({
                    "text": spec_text, "format": "spec"
                }),
            )
            .unwrap_or_default();

        let rules = arr(&r["locationRules"]);
        assert!(rules.len() >= 3, "should extract at least 3 rules, got {}", rules.len());

        let mc = rules.iter().find(|r| r["material"].as_str().unwrap_or("").contains("mc"));
        assert!(mc.is_some(), "should find MC cable rule");
    }

    // ── schedule text parsing tests ────────────────────────────────────────

    #[test]
    fn parse_luminaire_schedule() {
        let mut s = session();
        let schedule_text = "\
LUMINAIRE SCHEDULE
Mark\tDescription\tManufacturer\tCatalog No\tVolts\tMounting
H\tRecessed 2x4 LED troffer\tPinnacle Lighting\tRT-2400-LED\t120/277\tRecessed
J\tSurface LED strip\tPinnacle Lighting\tSL-4800\t120\tSurface
K\tWall sconce LED\tBeacon Electric\tWS-1200\t120\tWall
";
        let r = s
            .execute(
                "bim.spec_import",
                &json!({
                    "text": schedule_text, "format": "schedule"
                }),
            )
            .unwrap_or_default();

        let proposals = arr(&r["proposals"]);
        assert_eq!(proposals.len(), 3, "should find 3 types");

        let h = proposals.iter().find(|p| p["mark"] == "H").unwrap();
        assert_eq!(h["fields"]["description"]["value"], "Recessed 2x4 LED troffer");
        assert_eq!(h["fields"]["manufacturer"]["value"], "Pinnacle Lighting");
        assert_eq!(h["fields"]["catalogNumber"]["value"], "RT-2400-LED");
        assert_eq!(h["fields"]["volts"]["value"], "120/277");
    }

    #[test]
    fn parse_device_schedule() {
        let mut s = session();
        let schedule_text = "\
DEVICE SCHEDULE
Mark\tDescription\tManufacturer\tCatalog No\tAmps\tVolts\tNEMA
A\tDuplex receptacle\tBeacon Electric\tBR-5362\t20\t125\t5-20R
B\tGFCI receptacle\tBeacon Electric\tBR-5362-GFI\t20\t125\t5-20R
C\tSingle-pole switch\tBeacon Electric\tBS-1201\t20\t120\t-
";
        let r = s
            .execute(
                "bim.spec_import",
                &json!({
                    "text": schedule_text, "format": "schedule"
                }),
            )
            .unwrap_or_default();

        let proposals = arr(&r["proposals"]);
        assert_eq!(proposals.len(), 3);

        let b = proposals.iter().find(|p| p["mark"] == "B").unwrap();
        assert_eq!(b["fields"]["description"]["value"], "GFCI receptacle");
    }

    // ── submittal text parsing tests ───────────────────────────────────────

    #[test]
    fn parse_approved_submittal() {
        let mut s = session();
        let submittal_text = "\
SECTION 26 51 13 - INTERIOR LUMINAIRES

Type H: Recessed 2x4 LED troffer
Manufacturer: Pinnacle Lighting
Catalog No: RT-2400-LED-4000LM-DIM
Voltage: 120/277V
Finish: Matte white
Mounting: Recessed

APPROVED

Type J: Surface LED strip
Manufacturer: Pinnacle Lighting
Model: SL-4800-5000K-DIM
Voltage: 120V
Finish: Silver
Mounting: Surface

APPROVED AS NOTED
";
        let r = s
            .execute(
                "bim.spec_import",
                &json!({
                    "text": submittal_text, "format": "submittal"
                }),
            )
            .unwrap_or_default();

        let proposals = arr(&r["proposals"]);
        assert!(!proposals.is_empty(), "should have proposals");

        // Approved submittals should be proposed for merging.
        let h = proposals.iter().find(|p| p["mark"] == "H");
        assert!(h.is_some(), "should propose type H");

        let submittals = arr(&r["submittals"]);
        assert!(!submittals.is_empty(), "should have submittal records");
    }

    #[test]
    fn rejected_submittal_not_proposed() {
        let mut s = session();
        let submittal_text = "\
SECTION 26 51 13 - INTERIOR LUMINAIRES

Type H: Wrong brand troffer
Manufacturer: Cheapo Lights
Model: CHEAP-2400

REJECTED
";
        let r = s
            .execute(
                "bim.spec_import",
                &json!({
                    "text": submittal_text, "format": "submittal"
                }),
            )
            .unwrap_or_default();

        let proposals = arr(&r["proposals"]);
        assert!(proposals.is_empty(), "rejected submittal should not produce proposals");

        let submittals = arr(&r["submittals"]);
        assert!(!submittals.is_empty(), "should still record the submittal");
        assert_eq!(submittals[0]["reviewStatus"], "rejected");
    }

    // ── disagreement detection tests ───────────────────────────────────────

    #[test]
    fn spec_vs_existing_type_detects_disagreement() {
        let mut s = session();

        // Create an existing type with CCT = 4000K.
        s.execute(
            "bim.set_type",
            &json!({
                "mark": "H",
                "category": "luminaire",
                "fields": {
                    "cct": {"value": "4000K", "source": "schedule E-1"},
                    "manufacturer": "Pinnacle Lighting"
                }
            }),
        )
        .ok();

        // Import a spec that says 3500K.
        let spec_text = "\
SECTION 26 51 13
Type H: Recessed 2x4 LED troffer
Manufacturer: Pinnacle Lighting
CCT: 3500K
";
        let r = s
            .execute(
                "bim.spec_import",
                &json!({
                    "text": spec_text, "format": "spec"
                }),
            )
            .unwrap_or_default();

        let rfis = arr(&r["rfis"]);
        assert!(!rfis.is_empty(), "should detect CCT disagreement");

        let cct_rfi = rfis.iter().find(|r| r["field"] == "cct");
        assert!(cct_rfi.is_some(), "should find cct RFI");
        let cct_rfi = cct_rfi.unwrap();
        assert_eq!(cct_rfi["mark"], "H");
    }

    // ── bim.spec_accept tests ──────────────────────────────────────────────

    #[test]
    fn accept_proposals_creates_types() {
        let mut s = session();

        let r = s
            .execute(
                "bim.spec_accept",
                &json!({
                    "proposals": [
                        {
                            "mark": "H",
                            "category": "luminaire",
                            "fields": {
                                "description": "Recessed 2x4 LED troffer",
                                "manufacturer": {"value": "Pinnacle Lighting", "source": "spec 26 51 13"}
                            }
                        },
                        {
                            "mark": "A",
                            "category": "receptacle",
                            "fields": {
                                "description": "Duplex receptacle",
                                "amps": "20"
                            }
                        }
                    ]
                }),
            )
            .unwrap_or_default();

        assert_eq!(r["accepted"], 2);
        assert_eq!(r["types"].as_array().map(|a| a.len()), Some(2));

        // Verify they're in the drawing.
        let types = s.execute("bim.types", &json!({})).unwrap_or_default();
        assert_eq!(types["types"].as_array().map(|a| a.len()), Some(2));
    }

    #[test]
    fn accept_merge_preserves_existing_fields() {
        let mut s = session();

        // Create existing type.
        s.execute(
            "bim.set_type",
            &json!({
                "mark": "H",
                "fields": {"description": "original", "volts": "120/277"}
            }),
        )
        .ok();

        // Accept proposal that adds new fields but doesn't overwrite.
        s.execute(
            "bim.spec_accept",
            &json!({
                "proposals": [{
                    "mark": "H",
                    "category": "luminaire",
                    "fields": {
                        "description": "should not overwrite",
                        "manufacturer": "Pinnacle Lighting"
                    }
                }]
            }),
        )
        .ok();

        let types = s.execute("bim.types", &json!({"mark": "H"})).unwrap_or_default();
        let h = &types["types"][0];
        assert_eq!(h["fields"]["description"]["value"], "original", "should not overwrite");
        assert_eq!(h["fields"]["manufacturer"]["value"], "Pinnacle Lighting", "should add new");
        assert_eq!(h["fields"]["volts"]["value"], "120/277", "should keep existing");
    }

    #[test]
    fn accept_overwrite_mode() {
        let mut s = session();

        s.execute(
            "bim.set_type",
            &json!({
                "mark": "H",
                "fields": {"description": "original", "volts": "120/277"}
            }),
        )
        .ok();

        s.execute(
            "bim.spec_accept",
            &json!({
                "proposals": [{
                    "mark": "H",
                    "category": "luminaire",
                    "fields": {"description": "updated"}
                }],
                "overwrite": true
            }),
        )
        .ok();

        let types = s.execute("bim.types", &json!({"mark": "H"})).unwrap_or_default();
        let h = &types["types"][0];
        assert_eq!(h["fields"]["description"]["value"], "updated", "overwrite should replace");
        assert_eq!(h["category"], "luminaire");
        // In overwrite mode, fields not in the proposal are dropped.
        assert!(h["fields"].get("volts").is_none(), "volts should be gone in overwrite mode");
    }

    // ── auto-detect tests ──────────────────────────────────────────────────

    #[test]
    fn auto_detect_spec_format() {
        let mut s = session();
        let text = "\
SECTION 26 51 13 - INTERIOR LUMINAIRES
Type H: Recessed 2x4 LED troffer
Manufacturer: Pinnacle Lighting
";
        let r = s.execute("bim.spec_import", &json!({"text": text})).unwrap_or_default();
        let proposals = arr(&r["proposals"]);
        assert!(!proposals.is_empty(), "auto-detect should parse spec text");
    }

    // ── edge cases ─────────────────────────────────────────────────────────

    #[test]
    fn empty_text_errors() {
        let mut s = session();
        assert!(s.execute("bim.spec_import", &json!({"text": ""})).is_err());
        assert!(s.execute("bim.spec_import", &json!({"text": "   \n\n  "})).is_err());
    }

    #[test]
    fn missing_params_errors() {
        let mut s = session();
        assert!(s.execute("bim.spec_import", &json!({})).is_err());
        assert!(s.execute("bim.spec_accept", &json!({})).is_err());
    }

    #[test]
    fn accept_skips_empty_marks() {
        let mut s = session();
        let r = s
            .execute(
                "bim.spec_accept",
                &json!({
                    "proposals": [
                        {"mark": "", "fields": {"description": "no mark"}},
                        {"fields": {"description": "also no mark"}},
                        {"mark": "X", "fields": {"description": "valid"}}
                    ]
                }),
            )
            .unwrap_or_default();

        assert_eq!(r["accepted"], 1);
        assert_eq!(r["skipped"].as_array().map(|a| a.len()), Some(2));
    }

    #[test]
    fn hostile_params_never_panic() {
        let mut s = session();
        // Extremely long text.
        let long = "a".repeat(100_000);
        let r = s.execute("bim.spec_import", &json!({"text": long}));
        assert!(r.is_ok(), "long text should not panic");

        // Huge proposals list.
        let many: Vec<Value> = (0..10_000).map(|i| json!({"mark": format!("T{i}"), "fields": {"x": "y"}})).collect();
        let r = s.execute("bim.spec_accept", &json!({"proposals": many}));
        assert!(r.is_ok(), "many proposals should not panic");
    }

    // ── MTEXT formatting strip test ────────────────────────────────────────

    #[test]
    fn strip_mtext_formatting_works() {
        assert_eq!(super::strip_mtext_formatting("Hello\\PWorld"), "Hello\nWorld");
        assert_eq!(super::strip_mtext_formatting("{\\fArial;Some text}"), "Some text");
        assert_eq!(super::strip_mtext_formatting("No formatting"), "No formatting");
    }

    // ── full round-trip: import → accept → verify ──────────────────────────

    #[test]
    fn full_round_trip_spec_to_drawing() {
        let mut s = session();

        // Import from spec text.
        let spec_text = "\
SECTION 26 51 13

Type H: Recessed 2x4 LED troffer
Manufacturer: Pinnacle Lighting
Catalog No: RT-2400-LED
Voltage: 120/277V
Mounting: Recessed
Finish: White

Type J: Surface LED downlight
Manufacturer: Beacon Electric
Model: DL-900-30K
Mounting: Surface
";

        let import = s
            .execute(
                "bim.spec_import",
                &json!({
                    "text": spec_text, "format": "spec"
                }),
            )
            .unwrap_or_default();

        let proposals = arr(&import["proposals"]);
        assert!(proposals.len() >= 2);

        // Accept all proposals.
        let accept = s
            .execute(
                "bim.spec_accept",
                &json!({
                    "proposals": proposals
                }),
            )
            .unwrap_or_default();

        assert!(accept["accepted"].as_u64().unwrap_or(0) >= 2);

        // Verify types are in the drawing.
        let types = s.execute("bim.types", &json!({})).unwrap_or_default();
        let type_list = arr(&types["types"]);
        assert!(type_list.len() >= 2);

        let h = type_list.iter().find(|t| t["mark"] == "H");
        assert!(h.is_some());
        assert_eq!(h.unwrap()["fields"]["manufacturer"]["value"], "Pinnacle Lighting");

        // Assign to an entity and read it back.
        let circle = s.execute("circle", &json!({"center": [0, 0], "radius": 5})).unwrap_or_default();
        let handle = circle["handle"].as_str().unwrap_or_default().to_string();

        s.execute("bim.set_spec", &json!({"handles": [&handle], "mark": "H"})).ok();

        let spec = s.execute("bim.get_spec", &json!({"handles": [&handle]})).unwrap_or_default();
        assert_eq!(spec["specs"][0]["mark"], "H");
        assert_eq!(spec["specs"][0]["fields"]["manufacturer"]["value"], "Pinnacle Lighting");
    }

    // ── MUST 1: per-block submittal application ───────────────────────────

    #[test]
    fn submittal_per_block_no_cross_contamination() {
        let mut s = session();
        // Two types in one section.  H gets catalog A-1 and APPROVED,
        // J gets catalog B-2 and REVISE AND RESUBMIT.
        let submittal = "\
SECTION 26 51 13 - INTERIOR LUMINAIRES

Type H: Recessed 2x4 LED troffer
Manufacturer: Apex Lighting
Catalog No: A-1
Voltage: 120/277V
Mounting: Recessed
Finish: White

APPROVED

Type J: Surface LED strip
Manufacturer: Beacon Fixtures
Catalog No: B-2
Voltage: 120V
Mounting: Surface
Finish: Silver

REVISE AND RESUBMIT
";
        let r = s.execute("bim.spec_import", &json!({"text": submittal, "format": "submittal"})).unwrap_or_default();

        let proposals = arr(&r["proposals"]);

        // H should be proposed (approved) with its OWN catalog number.
        let h = proposals.iter().find(|p| p["mark"] == "H");
        assert!(h.is_some(), "H should be proposed (approved)");
        let h = h.unwrap();
        assert_eq!(h["fields"]["catalogNumber"]["value"], "A-1", "H must keep its own catalog number, not J's");
        assert_eq!(h["fields"]["manufacturer"]["value"], "Apex Lighting");
        assert_eq!(h["fields"]["reviewStatus"]["value"], "approved");

        // J should NOT be proposed (revise), but its data should still be
        // recorded on the submittal record.
        let j_proposed = proposals.iter().find(|p| p["mark"] == "J");
        assert!(j_proposed.is_none(), "J (revise) must not be proposed for merging");

        let submittals = arr(&r["submittals"]);
        assert!(!submittals.is_empty());
        let sub = &submittals[0];
        // The submittal record covers both marks.
        let marks = sub["typeMarks"].as_array().unwrap();
        assert!(marks.iter().any(|m| m == "H"), "submittal should list H");
        assert!(marks.iter().any(|m| m == "J"), "submittal should list J");
    }

    #[test]
    fn unstamped_submittal_proposed_as_unreviewed() {
        let mut s = session();
        let submittal = "\
SECTION 26 51 13

Type K: Pendant LED fixture
Manufacturer: Nova Electric
Catalog No: PD-300
";
        let r = s.execute("bim.spec_import", &json!({"text": submittal, "format": "submittal"})).unwrap_or_default();

        let proposals = arr(&r["proposals"]);
        let k = proposals.iter().find(|p| p["mark"] == "K");
        assert!(k.is_some(), "unstamped should be proposed");
        assert_eq!(k.unwrap()["fields"]["reviewStatus"]["value"], "unreviewed", "unstamped must be 'unreviewed', not approved");
    }

    // ── MUST 5: sheet schedule from positioned TEXT entities ───────────────

    #[test]
    fn sheet_schedule_from_text_entities() {
        let mut s = session();

        // Create TEXT entities arranged in a grid to simulate a luminaire
        // schedule drawn on a sheet.  Y decreases for each row (header at
        // top).  Use made-up names only.
        //
        // Header row (y = 100):
        //   x=0: "Mark"  x=40: "Description"  x=120: "Manufacturer"  x=200: "Catalog No"  x=280: "Volts"  x=330: "Mounting"
        // Data row 1 (y = 90):
        //   x=0: "H"     x=40: "Recessed 2x4 LED troffer"  x=120: "Apex Lighting"  x=200: "RT-2400"  x=280: "120/277"  x=330: "Recessed"
        // Data row 2 (y = 80):
        //   x=0: "J"     x=40: "Surface LED strip"  x=120: "Beacon Fixtures"  x=200: "SL-4800"  x=280: "120"  x=330: "Surface"

        // Title row (y = 110).
        s.execute("text", &json!({"at": [60, 110], "height": 4, "text": "LUMINAIRE SCHEDULE"})).ok();

        // Header row.
        s.execute("text", &json!({"at": [0, 100], "height": 3, "text": "Mark"})).ok();
        s.execute("text", &json!({"at": [40, 100], "height": 3, "text": "Description"})).ok();
        s.execute("text", &json!({"at": [120, 100], "height": 3, "text": "Manufacturer"})).ok();
        s.execute("text", &json!({"at": [200, 100], "height": 3, "text": "Catalog No"})).ok();
        s.execute("text", &json!({"at": [280, 100], "height": 3, "text": "Volts"})).ok();
        s.execute("text", &json!({"at": [330, 100], "height": 3, "text": "Mounting"})).ok();

        // Data row 1: type H.
        s.execute("text", &json!({"at": [0, 90], "height": 3, "text": "H"})).ok();
        s.execute("text", &json!({"at": [40, 90], "height": 3, "text": "Recessed 2x4 LED troffer"})).ok();
        s.execute("text", &json!({"at": [120, 90], "height": 3, "text": "Apex Lighting"})).ok();
        s.execute("text", &json!({"at": [200, 90], "height": 3, "text": "RT-2400"})).ok();
        s.execute("text", &json!({"at": [280, 90], "height": 3, "text": "120/277"})).ok();
        s.execute("text", &json!({"at": [330, 90], "height": 3, "text": "Recessed"})).ok();

        // Data row 2: type J.
        s.execute("text", &json!({"at": [0, 80], "height": 3, "text": "J"})).ok();
        s.execute("text", &json!({"at": [40, 80], "height": 3, "text": "Surface LED strip"})).ok();
        s.execute("text", &json!({"at": [120, 80], "height": 3, "text": "Beacon Fixtures"})).ok();
        s.execute("text", &json!({"at": [200, 80], "height": 3, "text": "SL-4800"})).ok();
        s.execute("text", &json!({"at": [280, 80], "height": 3, "text": "120"})).ok();
        s.execute("text", &json!({"at": [330, 80], "height": 3, "text": "Surface"})).ok();

        // Import with format "sheet".
        let r = s.execute("bim.spec_import", &json!({"format": "sheet", "sheet": "E-1"})).unwrap_or_default();

        let proposals = arr(&r["proposals"]);
        assert_eq!(proposals.len(), 2, "should find 2 schedule rows, got {}", proposals.len());

        // Verify type H fields.
        let h = proposals.iter().find(|p| p["mark"] == "H");
        assert!(h.is_some(), "should find type H");
        let h = h.unwrap();
        assert_eq!(h["fields"]["description"]["value"], "Recessed 2x4 LED troffer");
        assert_eq!(h["fields"]["manufacturer"]["value"], "Apex Lighting");
        assert_eq!(h["fields"]["catalogNumber"]["value"], "RT-2400");
        assert_eq!(h["fields"]["volts"]["value"], "120/277");
        assert_eq!(h["fields"]["mounting"]["value"], "Recessed");
        // Sheet id in source.
        assert!(h["fileSource"].as_str().unwrap_or("").contains("E-1"), "source should contain the sheet id");
        assert_eq!(h["category"], "luminaire");

        // Verify type J fields.
        let j = proposals.iter().find(|p| p["mark"] == "J");
        assert!(j.is_some(), "should find type J");
        let j = j.unwrap();
        assert_eq!(j["fields"]["manufacturer"]["value"], "Beacon Fixtures");
        assert_eq!(j["fields"]["catalogNumber"]["value"], "SL-4800");
    }

    // ── MUST 7: truth-scored precision/recall tests ───────────────────────

    /// Compute field-level precision and recall of proposals against a truth map.
    /// Returns (precision, recall, found_conflicts, extra_conflicts).
    fn score_proposals(
        proposals: &[Value],
        rfis: &[Value],
        truth: &serde_json::Map<String, Value>,
        expected_conflicts: &[(/*mark*/ &str, /*field*/ &str)],
    ) -> (f64, f64, Vec<String>, Vec<String>) {
        let mut tp = 0u64; // true positive fields
        let mut fp = 0u64; // false positive (extracted but wrong)
        let mut fn_ = 0u64; // false negative (in truth but not extracted)

        for (mark, truth_fields) in truth {
            let truth_obj = truth_fields.as_object().unwrap();
            let proposal = proposals.iter().find(|p| p["mark"].as_str() == Some(mark.as_str()));
            match proposal {
                Some(p) => {
                    for (key, expected_val) in truth_obj {
                        let expected = expected_val.as_str().unwrap_or_default();
                        let got = p["fields"][key]["value"].as_str().unwrap_or_default();
                        if got.eq_ignore_ascii_case(expected) {
                            tp += 1;
                        } else if got.is_empty() {
                            fn_ += 1;
                        } else {
                            fp += 1;
                        }
                    }
                    // Count extra fields not in truth as false positives.
                    // Exclude metadata fields the parser adds automatically.
                    if let Some(fields) = p["fields"].as_object() {
                        for key in fields.keys() {
                            if !truth_obj.contains_key(key) && !matches!(key.as_str(), "specSection" | "reviewStatus" | "submittalSource") {
                                fp += 1;
                            }
                        }
                    }
                }
                None => {
                    fn_ += truth_obj.len() as u64;
                }
            }
        }

        let precision = if tp + fp > 0 { tp as f64 / (tp + fp) as f64 } else { 0.0 };
        let recall = if tp + fn_ > 0 { tp as f64 / (tp + fn_) as f64 } else { 0.0 };

        // Check conflicts.
        let mut found = Vec::new();
        let mut extra = Vec::new();
        for rfi in rfis {
            let mark = rfi["mark"].as_str().unwrap_or_default();
            let field = rfi["field"].as_str().unwrap_or_default();
            let key = format!("{mark}:{field}");
            if expected_conflicts.iter().any(|(m, f)| *m == mark && *f == field) {
                found.push(key);
            } else {
                extra.push(key);
            }
        }

        (precision, recall, found, extra)
    }

    #[test]
    fn truth_scored_spec_and_schedule() {
        let mut s = session();

        // ── Truth: what the spec book SHOULD produce ─────────────────────
        // Note: the spec parser strips trailing alphabetic chars from voltage
        // values ("120/277V" → "120/277"), so truth matches that behaviour.
        let truth: serde_json::Map<String, Value> = serde_json::from_value(json!({
            "H": {
                "description": "Recessed 2x4 LED troffer",
                "manufacturer": "Apex Lighting",
                "catalogNumber": "RT-2400-LED",
                "volts": "120/277",
                "cct": "3500K",
                "mounting": "Recessed",
                "finish": "White"
            },
            "J": {
                "description": "Surface LED strip",
                "manufacturer": "Beacon Fixtures",
                "catalogNumber": "SL-4800-DIM",
                "volts": "120",
                "cct": "5000K",
                "mounting": "Surface",
                "finish": "Silver"
            },
            "A": {
                "description": "Duplex receptacle",
                "manufacturer": "Meridian Electric",
                "catalogNumber": "ME-5362",
                "volts": "125",
                "amps": "20"
            },
            "P1": {
                "description": "Lighting panel",
                "manufacturer": "Grid Controls",
                "volts": "120/208"
            }
        }))
        .unwrap();

        // ── Spec book text ───────────────────────────────────────────────
        let spec_text = "\
SECTION 26 51 13 - INTERIOR LUMINAIRES

Type H: Recessed 2x4 LED troffer
Manufacturer: Apex Lighting
Catalog No: RT-2400-LED
Voltage: 120/277V
CCT: 3500K
Mounting: Recessed
Finish: White

Type J: Surface LED strip
Manufacturer: Beacon Fixtures
Catalog No: SL-4800-DIM
Voltage: 120V
CCT: 5000K
Mounting: Surface
Finish: Silver

SECTION 26 27 26 - WIRING DEVICES

Type A: Duplex receptacle
Manufacturer: Meridian Electric
Catalog No: ME-5362
Voltage: 125V
Amps: 20

SECTION 26 24 16 - PANELBOARDS

Type P1: Lighting panel
Manufacturer: Grid Controls
Voltage: 120/208V
";

        let r = s.execute("bim.spec_import", &json!({"text": spec_text, "format": "spec"})).unwrap_or_default();
        let proposals = arr(&r["proposals"]);
        let rfis = arr(&r["rfis"]);

        let (precision, recall, _, extra) = score_proposals(proposals, rfis, &truth, &[]);
        assert!(precision >= 0.95, "spec precision {precision:.3} < 0.95; proposals: {proposals:?}");
        assert!(recall >= 0.95, "spec recall {recall:.3} < 0.95; proposals: {proposals:?}");
        assert!(extra.is_empty(), "no unplanted conflicts expected, got: {extra:?}");
    }

    #[test]
    fn truth_scored_with_planted_conflicts() {
        let mut s = session();

        // Pre-load existing types so spec import can detect disagreements.
        // Planted conflicts:
        //   H: CCT mismatch (existing 4000K, spec says 3500K)
        //   J: volts mismatch (existing 277V, spec says 120V)
        s.execute(
            "bim.set_type",
            &json!({
                "mark": "H",
                "category": "luminaire",
                "fields": {
                    "cct": {"value": "4000K", "source": "schedule E-1"},
                    "manufacturer": "Apex Lighting"
                }
            }),
        )
        .ok();
        s.execute(
            "bim.set_type",
            &json!({
                "mark": "J",
                "category": "luminaire",
                "fields": {
                    "volts": {"value": "277V", "source": "schedule E-1"},
                    "manufacturer": "Beacon Fixtures"
                }
            }),
        )
        .ok();

        let spec_text = "\
SECTION 26 51 13 - INTERIOR LUMINAIRES

Type H: Recessed 2x4 LED troffer
Manufacturer: Apex Lighting
Catalog No: RT-2400-LED
Voltage: 120/277V
CCT: 3500K
Mounting: Recessed
Finish: White

Type J: Surface LED strip
Manufacturer: Beacon Fixtures
Catalog No: SL-4800-DIM
Voltage: 120V
CCT: 5000K
Mounting: Surface
Finish: Silver
";

        let r = s.execute("bim.spec_import", &json!({"text": spec_text, "format": "spec"})).unwrap_or_default();

        let proposals = arr(&r["proposals"]);
        let rfis = arr(&r["rfis"]);

        // Truth for the spec fields (voltage alpha suffix stripped by parser).
        let truth: serde_json::Map<String, Value> = serde_json::from_value(json!({
            "H": {
                "description": "Recessed 2x4 LED troffer",
                "manufacturer": "Apex Lighting",
                "catalogNumber": "RT-2400-LED",
                "volts": "120/277",
                "cct": "3500K",
                "mounting": "Recessed",
                "finish": "White"
            },
            "J": {
                "description": "Surface LED strip",
                "manufacturer": "Beacon Fixtures",
                "catalogNumber": "SL-4800-DIM",
                "volts": "120",
                "cct": "5000K",
                "mounting": "Surface",
                "finish": "Silver"
            }
        }))
        .unwrap();

        let expected_conflicts: &[(&str, &str)] = &[("H", "cct"), ("J", "volts")];

        let (precision, recall, found, extra) = score_proposals(proposals, rfis, &truth, expected_conflicts);

        assert!(precision >= 0.95, "precision {precision:.3} < 0.95");
        assert!(recall >= 0.95, "recall {recall:.3} < 0.95");

        // Every planted conflict must be found.
        for &(mark, field) in expected_conflicts {
            let key = format!("{mark}:{field}");
            assert!(found.contains(&key), "planted conflict {key} not found in RFIs; rfis: {rfis:?}");
        }
        // No unplanted conflicts.
        assert!(extra.is_empty(), "unplanted conflicts found: {extra:?}; rfis: {rfis:?}");
    }

    #[test]
    fn truth_scored_submittal_with_two_types() {
        let mut s = session();

        // Two submittals with per-block catalog numbers.  Planted conflict:
        // submittal H has voltage 120V but existing says 120/277V.
        s.execute(
            "bim.set_type",
            &json!({
                "mark": "H",
                "category": "luminaire",
                "fields": {
                    "volts": {"value": "120/277V", "source": "spec 26 51 13"},
                    "manufacturer": "Apex Lighting"
                }
            }),
        )
        .ok();

        let submittal = "\
SECTION 26 51 13

Type H: Recessed 2x4 LED troffer
Manufacturer: Apex Lighting
Catalog No: A-1
Voltage: 120V
Mounting: Recessed
Finish: White

APPROVED

Type J: Surface LED strip
Manufacturer: Beacon Fixtures
Catalog No: B-2
Voltage: 120V
Mounting: Surface
Finish: Silver

APPROVED
";

        let r = s.execute("bim.spec_import", &json!({"text": submittal, "format": "submittal"})).unwrap_or_default();
        let proposals = arr(&r["proposals"]);
        let rfis = arr(&r["rfis"]);

        // Truth for approved submittal fields.  The submittal parser keeps
        // raw voltage text (no alpha suffix stripping unlike the spec parser).
        let truth: serde_json::Map<String, Value> = serde_json::from_value(json!({
            "H": {
                "description": "Recessed 2x4 LED troffer",
                "manufacturer": "Apex Lighting",
                "catalogNumber": "A-1",
                "volts": "120V",
                "mounting": "Recessed",
                "finish": "White"
            },
            "J": {
                "description": "Surface LED strip",
                "manufacturer": "Beacon Fixtures",
                "catalogNumber": "B-2",
                "volts": "120V",
                "mounting": "Surface",
                "finish": "Silver"
            }
        }))
        .unwrap();

        let expected_conflicts: &[(&str, &str)] = &[("H", "volts")];

        let (precision, recall, found, extra) = score_proposals(proposals, rfis, &truth, expected_conflicts);

        assert!(precision >= 0.95, "submittal precision {precision:.3} < 0.95; proposals: {proposals:?}");
        assert!(recall >= 0.95, "submittal recall {recall:.3} < 0.95; proposals: {proposals:?}");

        // H's catalog must be A-1, J's must be B-2 (no cross-contamination).
        let h = proposals.iter().find(|p| p["mark"] == "H").expect("H proposed");
        let j = proposals.iter().find(|p| p["mark"] == "J").expect("J proposed");
        assert_eq!(h["fields"]["catalogNumber"]["value"], "A-1");
        assert_eq!(j["fields"]["catalogNumber"]["value"], "B-2");

        // Planted voltage conflict on H.
        for &(mark, field) in expected_conflicts {
            let key = format!("{mark}:{field}");
            assert!(found.contains(&key), "planted conflict {key} not found; rfis: {rfis:?}");
        }
        assert!(extra.is_empty(), "unplanted conflicts: {extra:?}");
    }
}
