//! Project rules commands.
//!
//! - `project.rules.list` — list all rules (or filter by domain).
//! - `project.rules.get` — get a specific rule's resolved value (which source won, what it overrode).
//! - `project.rules.set` — set a rule value at a tier, with source and reason. Shows what changes.
//! - `project.rules.remove` — remove a rule or a tier from a rule.
//! - `project.rules.preview` — preview what setting a value would change, without modifying anything.
//! - `project.rules.label` — set or get the rules label (project name).
//! - `project.rules.import` — import rules from a TOML or JSON file.
//! - `project.rules.export` — export rules to a TOML or JSON file.

use cadaddy_doc::projrules::{ChangePreview, ProjectRules, RuleDomain, RuleEntry, RuleValue, SourceTier};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD_LIST: &str = "project.rules.list";
const CMD_GET: &str = "project.rules.get";
const CMD_SET: &str = "project.rules.set";
const CMD_REMOVE: &str = "project.rules.remove";
const CMD_PREVIEW: &str = "project.rules.preview";
const CMD_LABEL: &str = "project.rules.label";
const CMD_IMPORT: &str = "project.rules.import";
const CMD_EXPORT: &str = "project.rules.export";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_LIST, "List Project Rules", run_list)
            .menu(&["Tools", "Project Rules", "List"])
            .params("{domain?: string (mounting_height | wiring_method | stud_layout | joist_layout | clearance | tolerance | naming | custom)} → {label, rules: [{domain, key, value, tier, source, overridden: [{value, tier}]}]}")
            .noundo(),
        CommandSpec::new(CMD_GET, "Get Project Rule", run_get)
            .params("{domain: string, key: string} → {value, tier, source, overrides: [{value, tier}]} or null")
            .noundo(),
        CommandSpec::new(CMD_SET, "Set Project Rule", run_set)
            .params("{domain: string, key: string, value: string, tier?: string (default project_rule), source?: string, set_by?: string, reason?: string} → {preview: {oldValue, oldTier, newValue, newTier, changed, overridden}}"),
        CommandSpec::new(CMD_REMOVE, "Remove Project Rule", run_remove)
            .params("{domain: string, key: string, tier?: string (remove only this tier; omit to remove the whole rule)} → {removed: bool}"),
        CommandSpec::new(CMD_PREVIEW, "Preview Project Rule Change", run_preview)
            .params("{domain: string, key: string, value: string, tier?: string} → {preview: {oldValue, oldTier, newValue, newTier, changed, overridden}}")
            .noundo(),
        CommandSpec::new(CMD_LABEL, "Project Rules Label", run_label)
            .params("{label?: string (set the label; omit to just read it)} → {label}")
            .noundo(),
        CommandSpec::new(CMD_IMPORT, "Import Project Rules", run_import)
            .params("{path: string (TOML or JSON file), merge?: bool (default true: merge with existing; false: replace)} → {imported, changed}"),
        CommandSpec::new(CMD_EXPORT, "Export Project Rules", run_export)
            .params("{force?: bool (replace an existing file; otherwise it is refused), path: string (.toml or .json)} → {exported}")
            .noundo(),
    ]
}

// ── project.rules.list ─────────────────────────────────────────────────────

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let domain_filter = p.get("domain").and_then(Value::as_str).and_then(RuleDomain::parse);

    let entries = d.project_rules.list(domain_filter);
    let rules: Vec<Value> = entries.iter().map(|e| entry_to_json(e)).collect();

    Ok(json!({
        "label": d.project_rules.label,
        "rules": rules,
    }))
}

// ── project.rules.get ──────────────────────────────────────────────────────

fn run_get(s: &mut Session, p: &Value) -> Result<Value> {
    let domain = parse_domain(CMD_GET, p)?;
    let key = p.get("key").and_then(Value::as_str).ok_or_else(|| bad(CMD_GET, "`key` (string) is required"))?;

    let d = s.doc()?;
    match d.project_rules.resolve(domain, key) {
        Some(resolved) => Ok(json!({
            "value": resolved.value,
            "tier": resolved.tier,
            "source": resolved.source,
            "overrides": resolved.overrides.iter().map(|(v, t)| json!({"value": v, "tier": t})).collect::<Vec<_>>(),
        })),
        None => Ok(json!(null)),
    }
}

// ── project.rules.set ──────────────────────────────────────────────────────

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let domain = parse_domain(CMD_SET, p)?;
    let key = p.get("key").and_then(Value::as_str).ok_or_else(|| bad(CMD_SET, "`key` (string) is required"))?;
    let value_str = p.get("value").and_then(Value::as_str).ok_or_else(|| bad(CMD_SET, "`value` (string) is required"))?;

    let tier = parse_tier(CMD_SET, p)?;
    let source = p.get("source").and_then(Value::as_str).unwrap_or_default();
    let set_by = p.get("set_by").and_then(Value::as_str).unwrap_or_default();
    let reason = p.get("reason").and_then(Value::as_str).unwrap_or_default();

    // MUST 5: `set` refuses an empty `reason`.
    if reason.is_empty() {
        return Err(bad(CMD_SET, "`reason` (string) is required — explain why this value is being set"));
    }

    // MUST 5: Tiers above project_rule refuse an empty `source`.
    if tier < SourceTier::ProjectRule && source.is_empty() {
        return Err(bad(
            CMD_SET,
            format!("tier `{}` requires a non-empty `source` (document citation) — only project_rule and built_in may omit it", tier.as_str()),
        ));
    }

    // A validation rule override (validate.<RULE>.…) must name a rule, a field a project may
    // change and a value that fits it.
    if matches!(domain, RuleDomain::Tolerance | RuleDomain::Custom) && super::validate_rules::is_override_key(key) {
        super::validate_rules::check_override(domain, key, value_str).map_err(|e| bad(CMD_SET, e))?;
    }
    // What a mark is (validate.kind.<mark>) must name a mark and a kind.
    if super::validate_kind::is_kind_key(key) {
        super::validate_kind::check_kind_rule(domain, key, value_str).map_err(|e| bad(CMD_SET, e))?;
    }
    // A room type's expected set (prescreen.expect.<room type>) must read.
    if super::validate_prescreen::is_expect_key(key) {
        super::validate_prescreen::check_expect_rule(domain, key, value_str).map_err(|e| bad(CMD_SET, e))?;
    }

    let rule_value = RuleValue::new(value_str).with_source(source).with_tier(tier).with_set_by(set_by);

    // Preview element impact BEFORE modifying.
    let old_value = s.doc().ok().and_then(|d| d.project_rules.resolve(domain, key)).map(|r| r.value).unwrap_or_default();
    let (affected_count, affected_ids) = affected_elements(s, domain, key, &old_value, value_str);

    let d = s.doc_mut()?;
    let preview = d.project_rules.set(domain, key, rule_value, reason);

    Ok(json!({
        "preview": preview_to_json(&preview),
        "affectedElements": affected_count,
        "affectedIds": affected_ids,
        "message": if preview.changed {
            format!("Rule {}/{} changed: {} ({}) → {} ({})",
                domain.as_str(), key,
                preview.old_value, preview.old_tier.label(),
                preview.new_value, preview.new_tier.label())
        } else {
            format!("Rule {}/{} unchanged (active value stays {} from {})",
                domain.as_str(), key,
                preview.new_value, preview.new_tier.label())
        },
    }))
}

// ── project.rules.remove ───────────────────────────────────────────────────

fn run_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let domain = parse_domain(CMD_REMOVE, p)?;
    let key = p.get("key").and_then(Value::as_str).ok_or_else(|| bad(CMD_REMOVE, "`key` (string) is required"))?;

    let tier = p.get("tier").and_then(Value::as_str).and_then(SourceTier::parse);

    let d = s.doc_mut()?;
    let removed =
        if let Some(t) = tier { d.project_rules.remove_tier(domain, key, t).is_some() } else { d.project_rules.remove(domain, key).is_some() };

    Ok(json!({ "removed": removed }))
}

// ── project.rules.preview ──────────────────────────────────────────────────

fn run_preview(s: &mut Session, p: &Value) -> Result<Value> {
    let domain = parse_domain(CMD_PREVIEW, p)?;
    let key = p.get("key").and_then(Value::as_str).ok_or_else(|| bad(CMD_PREVIEW, "`key` (string) is required"))?;
    let value_str = p.get("value").and_then(Value::as_str).ok_or_else(|| bad(CMD_PREVIEW, "`value` (string) is required"))?;

    let tier = parse_tier(CMD_PREVIEW, p)?;

    let rule_value = RuleValue::new(value_str).with_tier(tier);

    let d = s.doc()?;
    let preview = d.project_rules.preview(domain, key, &rule_value);

    // Element impact.
    let (affected_count, affected_ids) = affected_elements(s, domain, key, &preview.old_value, &preview.new_value);

    Ok(json!({
        "preview": preview_to_json(&preview),
        "affectedElements": affected_count,
        "affectedIds": affected_ids,
    }))
}

// ── project.rules.label ────────────────────────────────────────────────────

fn run_label(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(label) = p.get("label").and_then(Value::as_str) {
        let d = s.doc_mut()?;
        d.project_rules.label = label.to_string();
        Ok(json!({ "label": label }))
    } else {
        let d = s.doc()?;
        Ok(json!({ "label": d.project_rules.label }))
    }
}

// ── project.rules.import ───────────────────────────────────────────────────

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let path = p.get("path").and_then(Value::as_str).ok_or_else(|| bad(CMD_IMPORT, "`path` (string) is required"))?;
    let merge = p.get("merge").and_then(Value::as_bool).unwrap_or(true);

    let text = std::fs::read_to_string(path).map_err(|e| bad(CMD_IMPORT, format!("cannot read `{path}`: {e}")))?;

    // Cap file size for hostile input.
    if text.len() > 4 * 1024 * 1024 {
        return Err(bad(CMD_IMPORT, "file too large (max 4 MB)"));
    }

    let imported: ProjectRules = if path.ends_with(".toml") {
        parse_toml_rules(&text).map_err(|e| bad(CMD_IMPORT, format!("TOML parse error: {e}")))?
    } else {
        ProjectRules::from_json(&text).map_err(|e| bad(CMD_IMPORT, format!("JSON parse error: {e}")))?
    };

    let count = imported.rules.len();
    let d = s.doc_mut()?;

    let mut changed = 0u64;
    if merge {
        for entry in imported.rules.values() {
            let preview = d.project_rules.set(entry.domain, &entry.key, entry.active.clone(), "imported from file");
            if preview.changed {
                changed = changed.saturating_add(1);
            }
        }
        if !imported.label.is_empty() {
            d.project_rules.label = imported.label.clone();
        }
    } else {
        d.project_rules = imported;
        changed = count as u64;
    }

    Ok(json!({ "imported": count, "changed": changed }))
}

// ── project.rules.export ───────────────────────────────────────────────────

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = p.get("path").and_then(Value::as_str).ok_or_else(|| bad(CMD_EXPORT, "`path` (string) is required"))?;

    super::exchange::guard_output(s, CMD_EXPORT, p, path)?;
    let d = s.doc()?;
    let text = if path.ends_with(".toml") {
        rules_to_toml(&d.project_rules)
    } else {
        serde_json::to_string_pretty(&d.project_rules).map_err(|e| bad(CMD_EXPORT, format!("JSON serialisation error: {e}")))?
    };

    crate::writefile::write(path, &text).map_err(|e| bad(CMD_EXPORT, format!("cannot write `{path}`: {e}")))?;

    Ok(json!({ "exported": d.project_rules.rules.len() }))
}

// ── TOML helpers ───────────────────────────────────────────────────────────

/// Parse a human-readable TOML rules file into ProjectRules.
///
/// Format:
/// ```toml
/// label = "Sample Project"
///
/// [mounting_height]
/// "receptacle/general" = { value = "18\"", source = "NEC 210.52", tier = "spec" }
/// "switch/general" = { value = "48\"", tier = "project_rule" }
///
/// [wiring_method]
/// general = { value = "MC cable", tier = "project_rule" }
/// ```
fn parse_toml_rules(text: &str) -> std::result::Result<ProjectRules, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut rules = ProjectRules::new();

    if let Some(label) = table.get("label").and_then(toml::Value::as_str) {
        rules.label = label.to_string();
    }

    for domain in RuleDomain::ALL {
        if let Some(section) = table.get(domain.as_str()).and_then(toml::Value::as_table) {
            for (key, val) in section {
                let rule_val = parse_toml_rule_value(val)?;
                rules.set(domain, key, rule_val, "imported from TOML");
            }
        }
    }

    Ok(rules)
}

fn parse_toml_rule_value(val: &toml::Value) -> std::result::Result<RuleValue, String> {
    match val {
        toml::Value::String(s) => Ok(RuleValue::new(s.clone())),
        toml::Value::Table(t) => {
            let value = t.get("value").and_then(toml::Value::as_str).ok_or("each rule must have a `value` string")?;
            let mut rv = RuleValue::new(value);
            if let Some(source) = t.get("source").and_then(toml::Value::as_str) {
                rv = rv.with_source(source);
            }
            if let Some(tier_str) = t.get("tier").and_then(toml::Value::as_str)
                && let Some(tier) = SourceTier::parse(tier_str)
            {
                rv = rv.with_tier(tier);
            }
            if let Some(by) = t.get("set_by").and_then(toml::Value::as_str) {
                rv = rv.with_set_by(by);
            }
            Ok(rv)
        }
        _ => Err("rule value must be a string or {value, source?, tier?, set_by?}".to_string()),
    }
}

/// Serialise ProjectRules to a human-readable TOML string.
fn rules_to_toml(rules: &ProjectRules) -> String {
    let mut out = String::new();
    if !rules.label.is_empty() {
        out.push_str(&format!("label = {:?}\n\n", rules.label));
    }

    for domain in RuleDomain::ALL {
        let entries: Vec<&RuleEntry> = rules.list(Some(domain));
        if entries.is_empty() {
            continue;
        }
        out.push_str(&format!("[{}]\n", domain.as_str()));
        for entry in entries {
            let v = &entry.active;
            // Use inline table form for readability.
            out.push_str(&format!("{:?} = {{ value = {:?}", entry.key, v.value));
            if !v.source.is_empty() {
                out.push_str(&format!(", source = {:?}", v.source));
            }
            if v.tier != SourceTier::ProjectRule {
                out.push_str(&format!(", tier = {:?}", v.tier.as_str()));
            }
            if !v.set_by.is_empty() {
                out.push_str(&format!(", set_by = {:?}", v.set_by));
            }
            out.push_str(" }\n");
        }
        out.push('\n');
    }
    out
}

// ── element impact ─────────────────────────────────────────────────────

/// Scan the drawing for entities affected by a rule change. For mounting_height rules,
/// this finds entities with a BIM type mark whose category matches the device kind in the key.
/// Returns (count, first N entity handles).
fn affected_elements(s: &Session, domain: RuleDomain, key: &str, old_value: &str, new_value: &str) -> (u64, Vec<String>) {
    if old_value == new_value {
        return (0, Vec::new());
    }
    // Only mounting_height rules affect elements currently.
    if domain != RuleDomain::MountingHeight {
        return (0, Vec::new());
    }
    // Parse device kind from key: "switch/general" -> "switch".
    let device_kind = key.split('/').next().unwrap_or(key);

    let d = match s.doc() {
        Ok(d) => d,
        Err(_) => return (0, Vec::new()),
    };

    // Find BIM type marks whose category matches the device kind.
    let matching_marks: Vec<&str> =
        d.bim_types.types.iter().filter(|t| t.category.eq_ignore_ascii_case(device_kind)).map(|t| t.mark.as_str()).collect();

    if matching_marks.is_empty() {
        return (0, Vec::new());
    }

    // Scan model entities for matching type marks.
    let mut count: u64 = 0;
    let mut handles = Vec::new();
    let max_handles = 100; // Page size.

    for e in d.model.iter() {
        if let Some(mark) = cadaddy_doc::bimspec::entity_type_mark(&e.common.xdata)
            && matching_marks.iter().any(|m| m.eq_ignore_ascii_case(mark))
        {
            count += 1;
            if handles.len() < max_handles {
                handles.push(format!("{:X}", e.handle.0));
            }
        }
    }

    (count, handles)
}

// ── JSON helpers ───────────────────────────────────────────────────────────

fn entry_to_json(e: &RuleEntry) -> Value {
    json!({
        "domain": e.domain.as_str(),
        "key": e.key,
        "value": e.active.value,
        "tier": e.active.tier.as_str(),
        "source": e.active.source,
        "set_by": e.active.set_by,
        "overridden": e.overridden.iter().map(|v| json!({
            "value": v.value,
            "tier": v.tier.as_str(),
            "source": v.source,
        })).collect::<Vec<_>>(),
        "changelog": e.changelog.iter().take(10).map(|c| json!({
            "from": c.from,
            "to": c.to,
            "reason": c.reason,
            "tier": c.tier.as_str(),
            "set_by": c.set_by,
            "time": c.time,
        })).collect::<Vec<_>>(),
    })
}

fn preview_to_json(p: &ChangePreview) -> Value {
    json!({
        "oldValue": p.old_value,
        "oldTier": p.old_tier.as_str(),
        "newValue": p.new_value,
        "newTier": p.new_tier.as_str(),
        "changed": p.changed,
        "overridden": p.overridden.iter().map(|v| json!({
            "value": v.value,
            "tier": v.tier.as_str(),
        })).collect::<Vec<_>>(),
    })
}

fn parse_domain(cmd: &str, p: &Value) -> Result<RuleDomain> {
    let s = p.get("domain").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`domain` (string) is required"))?;
    RuleDomain::parse(s).ok_or_else(|| {
        bad(cmd, format!("unknown domain `{s}`; valid: {}", RuleDomain::ALL.iter().map(|d| d.as_str()).collect::<Vec<_>>().join(", ")))
    })
}

fn parse_tier(cmd: &str, p: &Value) -> Result<SourceTier> {
    match p.get("tier").and_then(Value::as_str) {
        Some(s) => SourceTier::parse(s).ok_or_else(|| {
            bad(cmd, format!("unknown tier `{s}`; valid: {}", SourceTier::ALL.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", ")))
        }),
        None => Ok(SourceTier::ProjectRule),
    }
}

// ── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::Session;
    use serde_json::json;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).ok();
        s
    }

    /// Helper: set a project_rule tier (no source required).
    fn set_pr(s: &mut Session, domain: &str, key: &str, value: &str, reason: &str) -> serde_json::Value {
        s.execute("project.rules.set", &json!({"domain": domain, "key": key, "value": value, "tier": "project_rule", "reason": reason})).unwrap()
    }

    /// Helper: set at a higher tier (source required).
    fn set_tier(s: &mut Session, domain: &str, key: &str, value: &str, tier: &str, source: &str, reason: &str) -> serde_json::Value {
        s.execute("project.rules.set", &json!({"domain": domain, "key": key, "value": value, "tier": tier, "source": source, "reason": reason}))
            .unwrap()
    }

    #[test]
    fn set_and_list_rules() {
        let mut s = session();
        let r = set_pr(&mut s, "mounting_height", "receptacle/general", "18\"", "standard height");
        assert_eq!(r["preview"]["changed"], true);
        assert_eq!(r["preview"]["newValue"], "18\"");

        let r = s.execute("project.rules.list", &json!({})).unwrap();
        let rules = r["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["domain"], "mounting_height");
        assert_eq!(rules[0]["key"], "receptacle/general");
        assert_eq!(rules[0]["value"], "18\"");
    }

    #[test]
    fn get_rule_resolved() {
        let mut s = session();
        set_pr(&mut s, "wiring_method", "general", "MC cable", "project default");
        set_tier(&mut s, "wiring_method", "general", "EMT", "spec", "Spec 26 05 33", "spec requirement");

        let r = s.execute("project.rules.get", &json!({"domain": "wiring_method", "key": "general"})).unwrap();
        assert_eq!(r["value"], "EMT");
        assert_eq!(r["tier"], "spec");
        assert_eq!(r["source"], "Spec 26 05 33");
        let overrides = r["overrides"].as_array().unwrap();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0]["value"], "MC cable");
    }

    #[test]
    fn preview_without_modifying() {
        let mut s = session();
        set_tier(&mut s, "clearance", "panel_front", "36\"", "spec", "NEC 110.26", "code requirement");

        let r = s.execute("project.rules.preview", &json!({"domain": "clearance", "key": "panel_front", "value": "42\"", "tier": "rfi"})).unwrap();
        assert_eq!(r["preview"]["changed"], true);
        assert_eq!(r["preview"]["newValue"], "42\"");
        assert_eq!(r["preview"]["oldValue"], "36\"");

        // Actual value unchanged.
        let r = s.execute("project.rules.get", &json!({"domain": "clearance", "key": "panel_front"})).unwrap();
        assert_eq!(r["value"], "36\"");
    }

    #[test]
    fn remove_rule() {
        let mut s = session();
        set_pr(&mut s, "tolerance", "placement", "1/4\"", "initial");

        let r = s.execute("project.rules.remove", &json!({"domain": "tolerance", "key": "placement"})).unwrap();
        assert_eq!(r["removed"], true);
        let r = s.execute("project.rules.get", &json!({"domain": "tolerance", "key": "placement"})).unwrap();
        assert!(r.is_null());
    }

    #[test]
    fn remove_tier_promotes_next() {
        let mut s = session();
        set_pr(&mut s, "mounting_height", "switch/general", "48\"", "project default");
        set_tier(&mut s, "mounting_height", "switch/general", "46\"", "rfi", "RFI #12", "RFI response");

        let r = s.execute("project.rules.get", &json!({"domain": "mounting_height", "key": "switch/general"})).unwrap();
        assert_eq!(r["value"], "46\"");

        let r = s.execute("project.rules.remove", &json!({"domain": "mounting_height", "key": "switch/general", "tier": "rfi"})).unwrap();
        assert_eq!(r["removed"], true);

        let r = s.execute("project.rules.get", &json!({"domain": "mounting_height", "key": "switch/general"})).unwrap();
        assert_eq!(r["value"], "48\"");
    }

    #[test]
    fn label_set_and_get() {
        let mut s = session();
        let r = s.execute("project.rules.label", &json!({"label": "Sample Tower"})).unwrap();
        assert_eq!(r["label"], "Sample Tower");
        let r = s.execute("project.rules.label", &json!({})).unwrap();
        assert_eq!(r["label"], "Sample Tower");
    }

    #[test]
    fn undo_set() {
        let mut s = session();
        // Assert the set took effect.
        let r = set_pr(&mut s, "naming", "layer", "E-prefix", "naming convention");
        assert_eq!(r["preview"]["changed"], true);
        let r = s.execute("project.rules.get", &json!({"domain": "naming", "key": "layer"})).unwrap();
        assert_eq!(r["value"], "E-prefix");

        // Undo should restore.
        s.execute("undo", &json!({})).ok();
        let r = s.execute("project.rules.get", &json!({"domain": "naming", "key": "layer"})).unwrap();
        assert!(r.is_null());
    }

    #[test]
    fn undo_remove() {
        let mut s = session();
        set_pr(&mut s, "tolerance", "placement", "1/4\"", "initial");
        let r = s.execute("project.rules.get", &json!({"domain": "tolerance", "key": "placement"})).unwrap();
        assert_eq!(r["value"], "1/4\"");

        s.execute("project.rules.remove", &json!({"domain": "tolerance", "key": "placement"})).unwrap();
        let r = s.execute("project.rules.get", &json!({"domain": "tolerance", "key": "placement"})).unwrap();
        assert!(r.is_null());

        s.execute("undo", &json!({})).ok();
        let r = s.execute("project.rules.get", &json!({"domain": "tolerance", "key": "placement"})).unwrap();
        assert_eq!(r["value"], "1/4\"");
    }

    #[test]
    fn undo_import() {
        let mut s = session();
        set_pr(&mut s, "clearance", "panel_front", "36\"", "initial");

        let dir = std::env::temp_dir().join("projrules_undo_import");
        let _ = std::fs::create_dir_all(&dir);
        let json_path = dir.join("import.json");
        s.execute("project.rules.export", &json!({"path": json_path.to_str().unwrap_or_default()})).unwrap();

        // Import into same session (replace mode changes rules).
        let mut s2 = session();
        set_pr(&mut s2, "tolerance", "placement", "1/8\"", "tighter");
        s2.execute("project.rules.import", &json!({"path": json_path.to_str().unwrap_or_default(), "merge": false})).unwrap();

        // The tolerance rule should be gone (replaced).
        let r = s2.execute("project.rules.get", &json!({"domain": "tolerance", "key": "placement"})).unwrap();
        assert!(r.is_null());
        // The clearance rule should be there from the import.
        let r = s2.execute("project.rules.get", &json!({"domain": "clearance", "key": "panel_front"})).unwrap();
        assert_eq!(r["value"], "36\"");

        // Undo restores the pre-import state.
        s2.execute("undo", &json!({})).ok();
        let r = s2.execute("project.rules.get", &json!({"domain": "tolerance", "key": "placement"})).unwrap();
        assert_eq!(r["value"], "1/8\"");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filter_by_domain() {
        let mut s = session();
        set_pr(&mut s, "mounting_height", "a", "1", "test");
        set_pr(&mut s, "wiring_method", "b", "2", "test");

        let r = s.execute("project.rules.list", &json!({"domain": "mounting_height"})).unwrap();
        assert_eq!(r["rules"].as_array().unwrap().len(), 1);
        let r = s.execute("project.rules.list", &json!({})).unwrap();
        assert_eq!(r["rules"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn hostile_params_never_panic() {
        let mut s = session();
        // Missing required params.
        assert!(s.execute("project.rules.set", &json!({})).is_err());
        assert!(s.execute("project.rules.get", &json!({})).is_err());
        assert!(s.execute("project.rules.remove", &json!({})).is_err());
        assert!(s.execute("project.rules.preview", &json!({})).is_err());
        // Bad domain.
        assert!(s.execute("project.rules.set", &json!({"domain": "nonsense", "key": "a", "value": "b", "reason": "test"})).is_err());
        // Missing key.
        assert!(s.execute("project.rules.set", &json!({"domain": "clearance", "value": "36\"", "reason": "test"})).is_err());
        // Missing value.
        assert!(s.execute("project.rules.set", &json!({"domain": "clearance", "key": "a", "reason": "test"})).is_err());
        // Import non-existent file.
        assert!(s.execute("project.rules.import", &json!({"path": "/nonexistent/file.json"})).is_err());
    }

    #[test]
    fn set_refuses_empty_reason() {
        let mut s = session();
        let err = s.execute("project.rules.set", &json!({"domain": "clearance", "key": "a", "value": "36\""}));
        assert!(err.is_err(), "set without reason should fail");
    }

    #[test]
    fn higher_tier_refuses_empty_source() {
        let mut s = session();
        let err = s.execute(
            "project.rules.set",
            &json!({
                "domain": "clearance", "key": "a", "value": "36\"", "tier": "rfi", "reason": "test"
            }),
        );
        assert!(err.is_err(), "rfi tier without source should fail");
    }

    #[test]
    fn invalid_tier_is_an_error() {
        let mut s = session();
        let err = s.execute(
            "project.rules.set",
            &json!({
                "domain": "clearance", "key": "a", "value": "36\"", "tier": "made_up", "reason": "test"
            }),
        );
        assert!(err.is_err(), "invalid tier should fail");
        // Error should list valid tiers.
        let msg = format!("{:?}", err.unwrap_err());
        assert!(msg.contains("field_verified"), "error should list valid tiers");
    }

    #[test]
    fn changelog_records_lower_tier_set() {
        let mut s = session();
        set_tier(&mut s, "wiring_method", "general", "EMT", "spec", "Spec 26 05 33", "spec says EMT");
        set_pr(&mut s, "wiring_method", "general", "MC cable", "project default");

        let r = s.execute("project.rules.list", &json!({"domain": "wiring_method"})).unwrap();
        let rules = r["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        // Changelog should have 2 entries (both sets logged, even though the lower-tier didn't change active).
        let changelog = rules[0]["changelog"].as_array().unwrap();
        assert!(changelog.len() >= 2, "both sets should be logged, got {}", changelog.len());
    }

    #[test]
    fn preview_returns_affected_element_count() {
        use cadaddy_doc::bimspec::{BimType, SpecField};

        let mut s = session();

        // Add BIM types for switches.
        {
            let d = s.doc_mut().unwrap();
            let mut switch_type = BimType::new("S");
            switch_type.category = "switch".to_string();
            switch_type.set_field("mountingHeight", SpecField::new("48\""));
            d.bim_types.set(switch_type);

            // Add 5 entities with the switch type mark.
            for i in 0..5u64 {
                let mut e = cadaddy_doc::Entity::new(
                    cadaddy_doc::Handle(100 + i),
                    cadaddy_doc::EntityKind::Point(cadaddy_doc::Point { p: cadaddy_geom::Vec3::ZERO, angle: 0.0 }),
                );
                e.common.layer = "DEVICE".into();
                e.common.xdata = cadaddy_doc::Rare::from(cadaddy_doc::bimspec::type_mark_xdata("S"));
                d.model.push(e);
            }
        }

        // Set initial rule.
        set_pr(&mut s, "mounting_height", "switch/general", "48", "initial default");

        // Preview changing height from 48 to 46.
        let r = s
            .execute(
                "project.rules.preview",
                &json!({
                    "domain": "mounting_height",
                    "key": "switch/general",
                    "value": "46",
                    "tier": "rfi"
                }),
            )
            .unwrap();

        assert_eq!(r["preview"]["changed"], true);
        assert_eq!(r["preview"]["newValue"], "46");
        assert_eq!(r["affectedElements"], 5, "all 5 switch entities should be affected");
        let ids = r["affectedIds"].as_array().unwrap();
        assert_eq!(ids.len(), 5, "should list all 5 affected entity handles");

        // Preview with no actual change should show 0 affected.
        let r = s
            .execute(
                "project.rules.preview",
                &json!({
                    "domain": "mounting_height",
                    "key": "switch/general",
                    "value": "48",
                    "tier": "project_rule"
                }),
            )
            .unwrap();
        assert_eq!(r["affectedElements"], 0, "no change means no affected elements");
    }

    #[test]
    fn import_and_export_json() {
        let mut s = session();
        set_tier(&mut s, "clearance", "panel_front", "36\"", "spec", "NEC 110.26", "code requirement");
        s.execute("project.rules.label", &json!({"label": "Export Test"})).ok();

        let dir = std::env::temp_dir().join("projrules_test");
        let _ = std::fs::create_dir_all(&dir);
        let json_path = dir.join("test_rules.json");

        let r = s.execute("project.rules.export", &json!({"path": json_path.to_str().unwrap_or_default()})).unwrap();
        assert_eq!(r["exported"], 1);

        let mut s2 = session();
        let r = s2.execute("project.rules.import", &json!({"path": json_path.to_str().unwrap_or_default()})).unwrap();
        assert_eq!(r["imported"], 1);
        let r = s2.execute("project.rules.get", &json!({"domain": "clearance", "key": "panel_front"})).unwrap();
        assert_eq!(r["value"], "36\"");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_rules_drive_device_mounting_height() {
        use cadaddy_doc::projrules::{ProjectRules, RuleDomain, RuleValue, SourceTier};
        use cadaddy_mepfix::elec_reader::{FoundDevice, apply_project_rules};

        let mut rules = ProjectRules::new();
        rules.set(
            RuleDomain::MountingHeight,
            "switch/general",
            RuleValue::new("46").with_tier(SourceTier::Rfi).with_source("RFI #12").with_set_by("PM"),
            "per RFI #12",
        );

        // Create 3 fake switches at the default 46-inch height (from rule fallback).
        let mut devices: Vec<FoundDevice> = (0..3)
            .map(|i| FoundDevice {
                kind: "switch".into(),
                room: "general".into(),
                mounting_in: 46.0,
                height_source: "rule".into(),
                at_ft: [10.0 * i as f64, 5.0, 46.0 / 12.0],
                ..Default::default()
            })
            .collect();

        let changed = apply_project_rules(&mut devices, &rules);

        // All 3 switches should now be at 46" from the RFI rule.
        for (i, dev) in devices.iter().enumerate() {
            assert_eq!(dev.mounting_in, 46.0, "device {i}: height should be 46 inches");
            assert_eq!(dev.height_source, "project_rule", "device {i}: source should be project_rule");
            assert_eq!(dev.height_tier, "rfi", "device {i}: tier should be rfi");
            assert_eq!(dev.height_citation, "RFI #12", "device {i}: citation should be RFI #12");
        }
        // Heights didn't change (46 -> 46), so changed count = 0.
        assert_eq!(changed, 0, "height didn't change numerically");

        // Now set a different height and verify it changes.
        let mut rules2 = ProjectRules::new();
        rules2.set(RuleDomain::MountingHeight, "switch/general", RuleValue::new("48").with_tier(SourceTier::ProjectRule), "project default 48");
        rules2.set(
            RuleDomain::MountingHeight,
            "switch/general",
            RuleValue::new("46").with_tier(SourceTier::Rfi).with_source("RFI #12"),
            "per RFI #12",
        );

        // Devices start at default 48" (from hard-coded rules).
        let mut devices2: Vec<FoundDevice> = (0..3)
            .map(|i| FoundDevice {
                kind: "switch".into(),
                room: "general".into(),
                mounting_in: 48.0, // default_mounting_in gives 46 but let's say 48 for test.
                height_source: "rule".into(),
                at_ft: [10.0 * i as f64, 5.0, 48.0 / 12.0],
                ..Default::default()
            })
            .collect();

        let changed2 = apply_project_rules(&mut devices2, &rules2);
        assert_eq!(changed2, 3, "all 3 devices should have changed height from 48 to 46");

        for dev in &devices2 {
            assert_eq!(dev.mounting_in, 46.0, "height should be 46 inches from RFI");
            assert_eq!(dev.height_tier, "rfi");
            assert_eq!(dev.height_citation, "RFI #12");
            // Should report overriding the 48" project_rule.
            assert_eq!(dev.height_overrode.len(), 1, "should override one value");
            assert_eq!(dev.height_overrode[0].0, "48");
            assert_eq!(dev.height_overrode[0].1, "project_rule");
        }
    }

    #[test]
    fn import_and_export_toml() {
        let mut s = session();
        set_tier(&mut s, "mounting_height", "switch/general", "48\"", "spec", "NEC", "NEC default");
        set_pr(&mut s, "wiring_method", "general", "MC cable", "project default");
        s.execute("project.rules.label", &json!({"label": "TOML Test"})).ok();

        let dir = std::env::temp_dir().join("projrules_toml_test");
        let _ = std::fs::create_dir_all(&dir);
        let toml_path = dir.join("test_rules.toml");

        let r = s.execute("project.rules.export", &json!({"path": toml_path.to_str().unwrap_or_default()})).unwrap();
        assert_eq!(r["exported"], 2);

        let mut s2 = session();
        let r = s2.execute("project.rules.import", &json!({"path": toml_path.to_str().unwrap_or_default()})).unwrap();
        assert_eq!(r["imported"], 2);
        let r = s2.execute("project.rules.get", &json!({"domain": "mounting_height", "key": "switch/general"})).unwrap();
        assert_eq!(r["value"], "48\"");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
