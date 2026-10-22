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

use cadaddy_doc::projrules::{
    ChangePreview, ProjectRules, RuleDomain, RuleEntry, RuleValue, SourceTier,
};
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
            .params("{path: string (.toml or .json)} → {exported}")
            .noundo(),
    ]
}

// ── project.rules.list ─────────────────────────────────────────────────────

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let domain_filter = p
        .get("domain")
        .and_then(Value::as_str)
        .and_then(RuleDomain::parse);

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
    let key = p
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(CMD_GET, "`key` (string) is required"))?;

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
    let key = p
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(CMD_SET, "`key` (string) is required"))?;
    let value_str = p
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(CMD_SET, "`value` (string) is required"))?;

    let tier = p
        .get("tier")
        .and_then(Value::as_str)
        .and_then(SourceTier::parse)
        .unwrap_or(SourceTier::ProjectRule);
    let source = p.get("source").and_then(Value::as_str).unwrap_or_default();
    let set_by = p.get("set_by").and_then(Value::as_str).unwrap_or_default();
    let reason = p.get("reason").and_then(Value::as_str).unwrap_or_default();

    let rule_value = RuleValue::new(value_str)
        .with_source(source)
        .with_tier(tier)
        .with_set_by(set_by);

    let d = s.doc_mut()?;
    let preview = d.project_rules.set(domain, key, rule_value, reason);

    Ok(json!({
        "preview": preview_to_json(&preview),
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
    let key = p
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(CMD_REMOVE, "`key` (string) is required"))?;

    let tier = p
        .get("tier")
        .and_then(Value::as_str)
        .and_then(SourceTier::parse);

    let d = s.doc_mut()?;
    let removed = if let Some(t) = tier {
        d.project_rules.remove_tier(domain, key, t).is_some()
    } else {
        d.project_rules.remove(domain, key).is_some()
    };

    Ok(json!({ "removed": removed }))
}

// ── project.rules.preview ──────────────────────────────────────────────────

fn run_preview(s: &mut Session, p: &Value) -> Result<Value> {
    let domain = parse_domain(CMD_PREVIEW, p)?;
    let key = p
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(CMD_PREVIEW, "`key` (string) is required"))?;
    let value_str = p
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(CMD_PREVIEW, "`value` (string) is required"))?;

    let tier = p
        .get("tier")
        .and_then(Value::as_str)
        .and_then(SourceTier::parse)
        .unwrap_or(SourceTier::ProjectRule);

    let rule_value = RuleValue::new(value_str).with_tier(tier);

    let d = s.doc()?;
    let preview = d.project_rules.preview(domain, key, &rule_value);

    Ok(json!({ "preview": preview_to_json(&preview) }))
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
    let path = p
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(CMD_IMPORT, "`path` (string) is required"))?;
    let merge = p.get("merge").and_then(Value::as_bool).unwrap_or(true);

    let text = std::fs::read_to_string(path)
        .map_err(|e| bad(CMD_IMPORT, format!("cannot read `{path}`: {e}")))?;

    // Cap file size for hostile input.
    if text.len() > 4 * 1024 * 1024 {
        return Err(bad(CMD_IMPORT, "file too large (max 4 MB)"));
    }

    let imported: ProjectRules = if path.ends_with(".toml") {
        parse_toml_rules(&text).map_err(|e| bad(CMD_IMPORT, format!("TOML parse error: {e}")))?
    } else {
        ProjectRules::from_json(&text)
            .map_err(|e| bad(CMD_IMPORT, format!("JSON parse error: {e}")))?
    };

    let count = imported.rules.len();
    let d = s.doc_mut()?;

    let mut changed = 0u64;
    if merge {
        for entry in imported.rules.values() {
            let preview = d.project_rules.set(
                entry.domain,
                &entry.key,
                entry.active.clone(),
                "imported from file",
            );
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
    let path = p
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(CMD_EXPORT, "`path` (string) is required"))?;

    let d = s.doc()?;
    let text = if path.ends_with(".toml") {
        rules_to_toml(&d.project_rules)
    } else {
        serde_json::to_string_pretty(&d.project_rules)
            .map_err(|e| bad(CMD_EXPORT, format!("JSON serialisation error: {e}")))?
    };

    std::fs::write(path, &text)
        .map_err(|e| bad(CMD_EXPORT, format!("cannot write `{path}`: {e}")))?;

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
            let value = t
                .get("value")
                .and_then(toml::Value::as_str)
                .ok_or("each rule must have a `value` string")?;
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
    let s = p
        .get("domain")
        .and_then(Value::as_str)
        .ok_or_else(|| bad(cmd, "`domain` (string) is required"))?;
    RuleDomain::parse(s).ok_or_else(|| {
        bad(
            cmd,
            format!(
                "unknown domain `{s}`; valid: {}",
                RuleDomain::ALL
                    .iter()
                    .map(|d| d.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    })
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

    #[test]
    fn set_and_list_rules() {
        let mut s = session();

        // Set a mounting height rule.
        let r = s
            .execute(
                "project.rules.set",
                &json!({
                    "domain": "mounting_height",
                    "key": "receptacle/general",
                    "value": "18\"",
                    "tier": "project_rule",
                    "source": "NEC 210.52",
                    "reason": "standard height"
                }),
            )
            .unwrap_or_default();
        assert_eq!(r["preview"]["changed"], true);
        assert_eq!(r["preview"]["newValue"], "18\"");

        // List rules.
        let r = s
            .execute("project.rules.list", &json!({}))
            .unwrap_or_default();
        let empty = vec![];
        let rules = r["rules"].as_array().unwrap_or(&empty);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["domain"], "mounting_height");
        assert_eq!(rules[0]["key"], "receptacle/general");
        assert_eq!(rules[0]["value"], "18\"");
    }

    #[test]
    fn get_rule_resolved() {
        let mut s = session();

        s.execute(
            "project.rules.set",
            &json!({
                "domain": "wiring_method",
                "key": "general",
                "value": "MC cable",
                "tier": "project_rule"
            }),
        )
        .ok();

        // Override with spec.
        s.execute(
            "project.rules.set",
            &json!({
                "domain": "wiring_method",
                "key": "general",
                "value": "EMT",
                "tier": "spec",
                "source": "Spec 26 05 33"
            }),
        )
        .ok();

        let r = s
            .execute(
                "project.rules.get",
                &json!({"domain": "wiring_method", "key": "general"}),
            )
            .unwrap_or_default();
        assert_eq!(r["value"], "EMT");
        assert_eq!(r["tier"], "spec");
        assert_eq!(r["source"], "Spec 26 05 33");
        let empty = vec![];
        let overrides = r["overrides"].as_array().unwrap_or(&empty);
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0]["value"], "MC cable");
    }

    #[test]
    fn preview_without_modifying() {
        let mut s = session();

        s.execute(
            "project.rules.set",
            &json!({
                "domain": "clearance",
                "key": "panel_front",
                "value": "36\"",
                "tier": "spec",
                "source": "NEC 110.26"
            }),
        )
        .ok();

        let r = s
            .execute(
                "project.rules.preview",
                &json!({
                    "domain": "clearance",
                    "key": "panel_front",
                    "value": "42\"",
                    "tier": "rfi"
                }),
            )
            .unwrap_or_default();
        assert_eq!(r["preview"]["changed"], true);
        assert_eq!(r["preview"]["newValue"], "42\"");
        assert_eq!(r["preview"]["oldValue"], "36\"");

        // Actual value unchanged.
        let r = s
            .execute(
                "project.rules.get",
                &json!({"domain": "clearance", "key": "panel_front"}),
            )
            .unwrap_or_default();
        assert_eq!(r["value"], "36\"");
    }

    #[test]
    fn remove_rule() {
        let mut s = session();

        s.execute(
            "project.rules.set",
            &json!({
                "domain": "tolerance",
                "key": "placement",
                "value": "1/4\"",
                "tier": "project_rule"
            }),
        )
        .ok();

        let r = s
            .execute(
                "project.rules.remove",
                &json!({"domain": "tolerance", "key": "placement"}),
            )
            .unwrap_or_default();
        assert_eq!(r["removed"], true);

        let r = s
            .execute(
                "project.rules.get",
                &json!({"domain": "tolerance", "key": "placement"}),
            )
            .unwrap_or_default();
        assert!(r.is_null());
    }

    #[test]
    fn remove_tier_promotes_next() {
        let mut s = session();

        s.execute(
            "project.rules.set",
            &json!({
                "domain": "mounting_height",
                "key": "switch/general",
                "value": "48\"",
                "tier": "project_rule"
            }),
        )
        .ok();

        s.execute(
            "project.rules.set",
            &json!({
                "domain": "mounting_height",
                "key": "switch/general",
                "value": "46\"",
                "tier": "rfi",
                "source": "RFI #12"
            }),
        )
        .ok();

        // RFI wins.
        let r = s
            .execute(
                "project.rules.get",
                &json!({"domain": "mounting_height", "key": "switch/general"}),
            )
            .unwrap_or_default();
        assert_eq!(r["value"], "46\"");

        // Remove RFI tier.
        let r = s
            .execute(
                "project.rules.remove",
                &json!({"domain": "mounting_height", "key": "switch/general", "tier": "rfi"}),
            )
            .unwrap_or_default();
        assert_eq!(r["removed"], true);

        // Project rule takes over.
        let r = s
            .execute(
                "project.rules.get",
                &json!({"domain": "mounting_height", "key": "switch/general"}),
            )
            .unwrap_or_default();
        assert_eq!(r["value"], "48\"");
    }

    #[test]
    fn label_set_and_get() {
        let mut s = session();

        let r = s
            .execute("project.rules.label", &json!({"label": "Sample Tower"}))
            .unwrap_or_default();
        assert_eq!(r["label"], "Sample Tower");

        let r = s
            .execute("project.rules.label", &json!({}))
            .unwrap_or_default();
        assert_eq!(r["label"], "Sample Tower");
    }

    #[test]
    fn undo_set() {
        let mut s = session();

        s.execute(
            "project.rules.set",
            &json!({
                "domain": "naming",
                "key": "layer",
                "value": "E-prefix",
                "tier": "project_rule"
            }),
        )
        .ok();

        // Undo should restore.
        s.execute("undo", &json!({})).ok();

        let r = s
            .execute(
                "project.rules.get",
                &json!({"domain": "naming", "key": "layer"}),
            )
            .unwrap_or_default();
        assert!(r.is_null());
    }

    #[test]
    fn filter_by_domain() {
        let mut s = session();

        s.execute(
            "project.rules.set",
            &json!({"domain": "mounting_height", "key": "a", "value": "1"}),
        )
        .ok();
        s.execute(
            "project.rules.set",
            &json!({"domain": "wiring_method", "key": "b", "value": "2"}),
        )
        .ok();

        let r = s
            .execute(
                "project.rules.list",
                &json!({"domain": "mounting_height"}),
            )
            .unwrap_or_default();
        assert_eq!(r["rules"].as_array().unwrap_or(&vec![]).len(), 1);

        let r = s
            .execute("project.rules.list", &json!({}))
            .unwrap_or_default();
        assert_eq!(r["rules"].as_array().unwrap_or(&vec![]).len(), 2);
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
        assert!(s
            .execute(
                "project.rules.set",
                &json!({"domain": "nonsense", "key": "a", "value": "b"})
            )
            .is_err());

        // Missing key.
        assert!(s
            .execute(
                "project.rules.set",
                &json!({"domain": "clearance", "value": "36\""})
            )
            .is_err());

        // Missing value.
        assert!(s
            .execute(
                "project.rules.set",
                &json!({"domain": "clearance", "key": "a"})
            )
            .is_err());

        // Import non-existent file.
        assert!(s
            .execute(
                "project.rules.import",
                &json!({"path": "/nonexistent/file.json"})
            )
            .is_err());
    }

    #[test]
    fn import_and_export_json() {
        let mut s = session();

        s.execute(
            "project.rules.set",
            &json!({"domain": "clearance", "key": "panel_front", "value": "36\"", "tier": "spec", "source": "NEC 110.26"}),
        )
        .ok();
        s.execute(
            "project.rules.label",
            &json!({"label": "Export Test"}),
        )
        .ok();

        let dir = std::env::temp_dir().join("projrules_test");
        let _ = std::fs::create_dir_all(&dir);
        let json_path = dir.join("test_rules.json");

        let r = s
            .execute(
                "project.rules.export",
                &json!({"path": json_path.to_str().unwrap_or_default()}),
            )
            .unwrap_or_default();
        assert_eq!(r["exported"], 1);

        // Import into a new session.
        let mut s2 = session();
        let r = s2
            .execute(
                "project.rules.import",
                &json!({"path": json_path.to_str().unwrap_or_default()}),
            )
            .unwrap_or_default();
        assert_eq!(r["imported"], 1);

        let r = s2
            .execute(
                "project.rules.get",
                &json!({"domain": "clearance", "key": "panel_front"}),
            )
            .unwrap_or_default();
        assert_eq!(r["value"], "36\"");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_and_export_toml() {
        let mut s = session();

        s.execute(
            "project.rules.set",
            &json!({"domain": "mounting_height", "key": "switch/general", "value": "48\"", "tier": "spec", "source": "NEC"}),
        )
        .ok();
        s.execute(
            "project.rules.set",
            &json!({"domain": "wiring_method", "key": "general", "value": "MC cable"}),
        )
        .ok();
        s.execute(
            "project.rules.label",
            &json!({"label": "TOML Test"}),
        )
        .ok();

        let dir = std::env::temp_dir().join("projrules_toml_test");
        let _ = std::fs::create_dir_all(&dir);
        let toml_path = dir.join("test_rules.toml");

        let r = s
            .execute(
                "project.rules.export",
                &json!({"path": toml_path.to_str().unwrap_or_default()}),
            )
            .unwrap_or_default();
        assert_eq!(r["exported"], 2);

        // Import into a new session.
        let mut s2 = session();
        let r = s2
            .execute(
                "project.rules.import",
                &json!({"path": toml_path.to_str().unwrap_or_default()}),
            )
            .unwrap_or_default();
        assert_eq!(r["imported"], 2);

        let r = s2
            .execute(
                "project.rules.get",
                &json!({"domain": "mounting_height", "key": "switch/general"}),
            )
            .unwrap_or_default();
        assert_eq!(r["value"], "48\"");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
