//! Building-code rule book commands (Q6013): `code.select`, `code.rules`, `code.check`.
//!
//! The starter rules ship with the binary ([`cadaddy_walk3d::codebook::starter_book`]). The
//! project's code selection (editions + jurisdiction) lives in the building spec's `codebook`
//! JSON field, so it travels with the drawing. Amendments layer on top of the starter rules.

use cadaddy_walk3d::codebook::{
    Amendment, Code, CodeBook, CodeFinding, CodeSelection, Edition, FindingSeverity, MechCheck, PlumbingCheck, RoomCheck, Topic, WallCheck,
    starter_book,
};
use serde_json::{Value, json};

use super::{CommandSpec, bad};
use crate::{Result, Session};

const SELECT: &str = "code.select";
const RULES: &str = "code.rules";
const CHECK: &str = "code.check";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(SELECT, "Select Building Codes", run_select)
            .params("{code: \"IRC\"|\"IBC\"|\"NEC\"|\"IPC\"|\"IMC\"|\"IFC\"|\"IECC\", edition?: year (default latest starter), jurisdiction?: \"County Name\"} → {editions: [{code, year}], jurisdiction?, rules}: select the code editions for this project; call with no args to see the current selection")
            .noundo(),
        CommandSpec::new(RULES, "Code Rules", run_rules)
            .params("{topic?: \"framing\"|\"electrical\"|\"plumbing\"|\"mechanical\"|\"building\", code?: \"IRC\"|\"IBC\"|\"NEC\"|\"IPC\"|\"IMC\"|\"IFC\"|\"IECC\", section?: \"R602.3\", offset?, limit?} → {rules: [{id, code, section, topic, summary, applies, value, source}], total}")
            .noundo(),
        CommandSpec::new(CHECK, "Check Building Codes", run_check)
            .params("{scope?: \"framing\"|\"electrical\"|\"plumbing\"|\"mechanical\"|\"building\"|\"all\" (default \"all\"), level?, walls?: [{id, wall_type, stud_size?, stud_spacing_in, height_ft, has_fire_blocking?, header_span_ft?, header_size?}], rooms?: [{id, room_type, location?, receptacle_spacing_in?, has_gfci?, has_afci?, panel_clearance_in?}]} → {pass, findings: [{rule_id, cite, element?, level?, expected, found, topic, source, severity}], by_topic: {topic: count}, by_severity: {severity: count}}")
            .noundo(),
    ]
}

/// Get the project's codebook (starter + selection from the spec).
fn project_book(s: &Session, cmd: &str) -> Result<CodeBook> {
    let mut book = starter_book();
    let (sp, _) = super::bspec::current(s, cmd)?;
    if !sp.codebook.is_null() {
        if let Ok(sel) = serde_json::from_value::<CodeSelection>(sp.codebook.clone()) {
            book.selection = sel;
        }
        // Load any project-level amendments from the spec.
        if let Some(amends) = sp.codebook.get("amendments")
            && let Ok(a) = serde_json::from_value::<Vec<Amendment>>(amends.clone())
        {
            book.amendments.extend(a);
        }
    }
    Ok(book)
}

/// Store the selection back into the building spec.
fn store_selection(s: &mut Session, cmd: &str, sel: &CodeSelection) -> Result<()> {
    let val = serde_json::to_value(sel).map_err(|e| bad(cmd, e.to_string()))?;
    s.execute("bspec.set", &json!({"spec": {"codebook": val}}))?;
    Ok(())
}

fn run_select(s: &mut Session, p: &Value) -> Result<Value> {
    let book = project_book(s, SELECT)?;

    // If no params (or just {}), return current selection.
    let code_str = p.get("code").and_then(Value::as_str);
    if code_str.is_none() && p.get("edition").is_none() && p.get("jurisdiction").is_none() {
        let editions: Vec<Value> = book.selection.editions.iter().map(|e| json!({"code": e.code.label(), "year": e.year})).collect();
        return Ok(json!({
            "editions": editions,
            "jurisdiction": book.selection.jurisdiction,
            "rules": book.rules.len(),
        }));
    }

    let mut sel = book.selection.clone();

    if let Some(cs) = code_str {
        let code = Code::parse(cs).ok_or_else(|| bad(SELECT, format!("unknown code `{cs}` — use IBC, IRC, NEC, IPC, IMC, IFC or IECC")))?;
        let year = p.get("edition").and_then(Value::as_u64).map(|y| y as u16);
        let edition_year = year.unwrap_or_else(|| {
            // Find the latest starter edition for this code.
            book.rules.iter().filter(|r| r.code == code).map(|r| r.edition).max().unwrap_or(2021)
        });
        let ed = Edition { code, year: edition_year };
        // Replace any existing edition of the same code.
        sel.editions.retain(|e| e.code != code);
        sel.editions.push(ed);
    }

    if let Some(j) = p.get("jurisdiction").and_then(Value::as_str) {
        if j.is_empty() {
            sel.jurisdiction = None;
        } else {
            sel.jurisdiction = Some(j.to_string());
        }
    }

    store_selection(s, SELECT, &sel)?;

    let updated_book = project_book(s, SELECT)?;
    let editions: Vec<Value> = sel.editions.iter().map(|e| json!({"code": e.code.label(), "year": e.year})).collect();

    Ok(json!({
        "editions": editions,
        "jurisdiction": sel.jurisdiction,
        "rules": updated_book.rules.len(),
    }))
}

fn run_rules(s: &mut Session, p: &Value) -> Result<Value> {
    let book = project_book(s, RULES)?;

    let topic = p.get("topic").and_then(Value::as_str).and_then(Topic::parse);
    let code = p.get("code").and_then(Value::as_str).and_then(Code::parse);
    let section = p.get("section").and_then(Value::as_str);
    let offset = p.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(100).min(1000) as usize;

    let ctx = cadaddy_walk3d::codebook::RuleContext::default();
    let mut matched: Vec<&cadaddy_walk3d::codebook::CodeRule> = book.query(topic, &ctx);

    // Additional filters.
    if let Some(c) = code {
        matched.retain(|r| r.code == c);
    }
    if let Some(sec) = section {
        let sec_lower = sec.to_ascii_lowercase();
        matched.retain(|r| r.section.to_ascii_lowercase().contains(&sec_lower));
    }

    let total = matched.len();
    let page: Vec<Value> = matched
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|r| {
            let val = book.resolve(r);
            json!({
                "id": r.id,
                "code": r.code.label(),
                "edition": r.edition,
                "section": r.section,
                "topic": r.topic,
                "summary": r.summary,
                "applies": r.applies,
                "value": val,
                "source": r.source.label(),
            })
        })
        .collect();

    Ok(json!({
        "rules": page,
        "total": total,
        "offset": offset,
    }))
}

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    let book = project_book(s, CHECK)?;

    let scope = p.get("scope").and_then(Value::as_str).unwrap_or("all");
    let level_filter = p.get("level").and_then(Value::as_str);

    let mut all_findings: Vec<CodeFinding> = Vec::new();

    // Framing checks.
    if scope == "all" || scope == "framing" {
        let walls = parse_walls(p, level_filter);
        if !walls.is_empty() {
            all_findings.extend(book.check_framing(&walls));
        }
    }

    // Electrical checks.
    if scope == "all" || scope == "electrical" {
        let rooms = parse_rooms(p, level_filter);
        if !rooms.is_empty() {
            all_findings.extend(book.check_electrical(&rooms));
        }
    }

    // Plumbing checks.
    if scope == "all" || scope == "plumbing" {
        let fixtures = parse_plumbing(p, level_filter);
        if !fixtures.is_empty() {
            all_findings.extend(book.check_plumbing(&fixtures));
        }
    }

    // Mechanical checks.
    if scope == "all" || scope == "mechanical" {
        let ducts = parse_mechanical(p, level_filter);
        if !ducts.is_empty() {
            all_findings.extend(book.check_mechanical(&ducts));
        }
    }

    // Building checks.
    if scope == "all" || scope == "building" {
        let openings = parse_building(p, level_filter);
        if !openings.is_empty() {
            all_findings.extend(book.check_building(&openings));
        }
    }

    let pass = all_findings.is_empty();

    // Counts by topic and severity.
    let mut by_topic: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut by_severity: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for f in &all_findings {
        *by_topic.entry(f.topic.label().to_string()).or_default() += 1;
        let sev = match f.severity {
            FindingSeverity::High => "high",
            FindingSeverity::Medium => "medium",
            FindingSeverity::Low => "low",
        };
        *by_severity.entry(sev.to_string()).or_default() += 1;
    }

    let findings_json: Vec<Value> = all_findings
        .iter()
        .take(1000)
        .map(|f| {
            json!({
                "rule_id": f.rule_id,
                "cite": f.cite,
                "element": f.element,
                "level": f.level,
                "expected": f.expected,
                "found": f.found,
                "topic": f.topic,
                "source": f.source.label(),
                "severity": f.severity,
            })
        })
        .collect();

    Ok(json!({
        "pass": pass,
        "findings": findings_json,
        "by_topic": by_topic,
        "by_severity": by_severity,
        "total": all_findings.len(),
    }))
}

// ---- parsers for check input data -----------------------------------------------

fn parse_walls(p: &Value, level_filter: Option<&str>) -> Vec<WallCheck> {
    let Some(arr) = p.get("walls").and_then(Value::as_array) else { return vec![] };
    arr.iter()
        .filter_map(|w| {
            let level = w.get("level").and_then(Value::as_str).map(String::from);
            if level_filter.is_some_and(|lf| level.as_deref() != Some(lf)) {
                return None;
            }
            Some(WallCheck {
                id: w.get("id").and_then(Value::as_str).unwrap_or("").into(),
                level,
                wall_type: w.get("wall_type").and_then(Value::as_str).unwrap_or("bearing").into(),
                stud_size: w.get("stud_size").and_then(Value::as_str).map(String::from),
                stud_spacing_in: w.get("stud_spacing_in").and_then(Value::as_f64).unwrap_or(16.0),
                height_ft: w.get("height_ft").and_then(Value::as_f64).unwrap_or(8.0),
                has_fire_blocking: w.get("has_fire_blocking").and_then(Value::as_bool).unwrap_or(false),
                header_span_ft: w.get("header_span_ft").and_then(Value::as_f64),
                header_size: w.get("header_size").and_then(Value::as_str).map(String::from),
            })
        })
        .collect()
}

fn parse_rooms(p: &Value, level_filter: Option<&str>) -> Vec<RoomCheck> {
    let Some(arr) = p.get("rooms").and_then(Value::as_array) else { return vec![] };
    arr.iter()
        .filter_map(|r| {
            let level = r.get("level").and_then(Value::as_str).map(String::from);
            if level_filter.is_some_and(|lf| level.as_deref() != Some(lf)) {
                return None;
            }
            Some(RoomCheck {
                id: r.get("id").and_then(Value::as_str).unwrap_or("").into(),
                level,
                room_type: r.get("room_type").and_then(Value::as_str).unwrap_or("dwelling").into(),
                location: r.get("location").and_then(Value::as_str).map(String::from),
                receptacle_spacing_in: r.get("receptacle_spacing_in").and_then(Value::as_f64),
                has_gfci: r.get("has_gfci").and_then(Value::as_bool).unwrap_or(false),
                has_afci: r.get("has_afci").and_then(Value::as_bool).unwrap_or(false),
                panel_clearance_in: r.get("panel_clearance_in").and_then(Value::as_f64),
            })
        })
        .collect()
}

fn parse_plumbing(p: &Value, level_filter: Option<&str>) -> Vec<PlumbingCheck> {
    let Some(arr) = p.get("plumbing").and_then(Value::as_array) else { return vec![] };
    arr.iter()
        .filter_map(|f| {
            let level = f.get("level").and_then(Value::as_str).map(String::from);
            if level_filter.is_some_and(|lf| level.as_deref() != Some(lf)) {
                return None;
            }
            Some(PlumbingCheck {
                id: f.get("id").and_then(Value::as_str).unwrap_or("").into(),
                level,
                pipe_material: f.get("pipe_material").and_then(Value::as_str).map(String::from),
                pipe_size_in: f.get("pipe_size_in").and_then(Value::as_f64),
                fixture_units: f.get("fixture_units").and_then(Value::as_f64),
            })
        })
        .collect()
}

fn parse_mechanical(p: &Value, level_filter: Option<&str>) -> Vec<MechCheck> {
    let Some(arr) = p.get("mechanical").and_then(Value::as_array) else { return vec![] };
    arr.iter()
        .filter_map(|d| {
            let level = d.get("level").and_then(Value::as_str).map(String::from);
            if level_filter.is_some_and(|lf| level.as_deref() != Some(lf)) {
                return None;
            }
            Some(MechCheck {
                id: d.get("id").and_then(Value::as_str).unwrap_or("").into(),
                level,
                location: d.get("location").and_then(Value::as_str).map(String::from),
                has_damper: d.get("has_damper").and_then(Value::as_bool).unwrap_or(false),
                clearance_in: d.get("clearance_in").and_then(Value::as_f64),
            })
        })
        .collect()
}

fn parse_building(p: &Value, level_filter: Option<&str>) -> Vec<cadaddy_walk3d::codebook::EgressCheck> {
    let Some(arr) = p.get("building").and_then(Value::as_array) else { return vec![] };
    arr.iter()
        .filter_map(|o| {
            let level = o.get("level").and_then(Value::as_str).map(String::from);
            if level_filter.is_some_and(|lf| level.as_deref() != Some(lf)) {
                return None;
            }
            Some(cadaddy_walk3d::codebook::EgressCheck {
                id: o.get("id").and_then(Value::as_str).unwrap_or("").into(),
                level,
                occupancy: o.get("occupancy").and_then(Value::as_str).map(String::from),
                width_in: o.get("width_in").and_then(Value::as_f64),
                clearance_in: o.get("clearance_in").and_then(Value::as_f64),
            })
        })
        .collect()
}
