//! Reviewing what each element is (Q299): the building spec's element tags — every field with
//! its source, and what is unresolved — listed, changed in bulk and accepted. The Tags palette
//! and agents (through MCP's `execute`) use the same commands.
//!
//! - `tags.list {kind?, issue?, level?, unresolved?, limit?, cursor?}`: a page of tags and the
//!   counts of each open issue.
//! - `tags.set {ids? | kind?/issue?/level?, field, value}`: one field on every tag picked (by id,
//!   or by filter), as the user's; the issues that field settles are resolved.
//! - `tags.accept {ids? | kind?/issue?/level?}`: mark them reviewed, as they are.

use cadaddy_walk3d::tags::{ElementTag, issues_settled_by};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::bspec::current;
use super::*;
use crate::{Result, Session};

/// Tags per `tags.list` answer by default, and at most.
const PAGE: usize = 100;
const MAX_PAGE: usize = 1_000;
/// Ids one call may name.
const MAX_IDS: usize = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tags.list", "List Element Tags", run_list)
            .params("{kind?: luminaire|receptacle|homerun|conduit|…, issue?: untagged|tag_not_in_schedule|missing_protection|double_protection|no_conductor_note|two_rules_apply, level?, unresolved?: bool, limit?: 1–1000 (100), cursor?} → {tags: [{id, kind, level, at_ft, fields: {name: {value, source}}, issues, reviewed?}], total, byIssue, next?}")
            .noundo(),
        CommandSpec::new("tags.set", "Set Element Tag Field", run_set).params(
            "{ids?: [id] (else every tag matching kind/issue/level), kind?, issue?, level?, field: name, value: text} → {changed, resolved: issues cleared}",
        ),
        CommandSpec::new("tags.accept", "Accept Element Tags", run_accept)
            .params("{ids?: [id] (else every tag matching kind/issue/level), kind?, issue?, level?} → {accepted}"),
    ]
}

/// Whether a tag passes the filter.
fn matches(t: &ElementTag, p: &Value) -> bool {
    let eq = |key: &str, v: &str| str_param(p, key).is_none_or(|w| w.eq_ignore_ascii_case(v));
    eq("kind", &t.kind)
        && eq("level", &t.level)
        && str_param(p, "issue").is_none_or(|i| t.issues.iter().any(|x| x.eq_ignore_ascii_case(i)))
        && (!bool_or(p, "unresolved", false) || !t.issues.is_empty())
}

/// The tags a set/accept call picks: by `ids`, else by filter (refusing to pick everything
/// with no filter at all).
fn picked(cmd: &str, tags: &[ElementTag], p: &Value) -> Result<Vec<usize>> {
    if let Some(ids) = p.get("ids").filter(|v| !v.is_null()) {
        let ids: Vec<&str> =
            ids.as_array().ok_or_else(|| bad(cmd, "`ids` must be a list of tag ids"))?.iter().take(MAX_IDS).filter_map(Value::as_str).collect();
        return Ok(tags.iter().enumerate().filter(|(_, t)| ids.contains(&t.id.as_str())).map(|(i, _)| i).collect());
    }
    if ["kind", "issue", "level"].iter().all(|k| str_param(p, k).is_none()) {
        return Err(bad(cmd, "name the tags: `ids`, or a `kind`, `issue` or `level` filter"));
    }
    Ok(tags.iter().enumerate().filter(|(_, t)| matches(t, p)).map(|(i, _)| i).collect())
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "tags.list")?;
    let mut by_issue: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &sp.tags {
        for i in &t.issues {
            *by_issue.entry(i.as_str()).or_insert(0) += 1;
        }
    }
    let limit = p.get("limit").and_then(Value::as_u64).map_or(PAGE, |n| usize::try_from(n).unwrap_or(MAX_PAGE).clamp(1, MAX_PAGE));
    let from = p.get("cursor").and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let all: Vec<&ElementTag> = sp.tags.iter().filter(|t| matches(t, p)).collect();
    let page: Vec<Value> = all.iter().skip(from).take(limit).map(|t| serde_json::to_value(t).unwrap_or(Value::Null)).collect();
    let mut out = json!({ "tags": page, "total": all.len(), "byIssue": by_issue });
    if from.saturating_add(limit) < all.len() {
        out["next"] = json!(from + limit);
    }
    Ok(out)
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "tags.set")?;
    let field = str_param(p, "field")
        .map(str::trim)
        .filter(|f| !f.is_empty() && f.len() <= 64)
        .ok_or_else(|| bad("tags.set", "`field` (a name) is required"))?;
    let value = str_param(p, "value").ok_or_else(|| bad("tags.set", "`value` (text) is required"))?;
    if value.len() > 500 {
        return Err(bad("tags.set", "`value` is longer than 500 characters"));
    }
    let picked = picked("tags.set", &sp.tags, p)?;
    let settles = issues_settled_by(field);
    let mut resolved = 0;
    for i in &picked {
        if let Some(t) = sp.tags.get_mut(*i) {
            t.set(field, value, "user");
            let before = t.issues.len();
            t.issues.retain(|x| !settles.contains(&x.as_str()));
            resolved += before - t.issues.len();
        }
    }
    if !picked.is_empty() {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({ "changed": picked.len(), "resolved": resolved }))
}

fn run_accept(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut sp, _) = current(s, "tags.accept")?;
    let picked = picked("tags.accept", &sp.tags, p)?;
    for i in &picked {
        if let Some(t) = sp.tags.get_mut(*i) {
            t.reviewed = true;
        }
    }
    if !picked.is_empty() {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({ "accepted": picked.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tags(s: &mut Session) {
        let tags = json!([
            {"id": "device:1:10.0,5.0", "kind": "luminaire", "level": "1", "at_ft": [10, 5], "issues": ["tag_not_in_schedule"], "fields": {"type": {"value": "Z", "source": "plan"}}},
            {"id": "device:1:20.0,5.0", "kind": "luminaire", "level": "1", "at_ft": [20, 5], "issues": ["untagged"]},
            {"id": "device:1:30.0,5.0", "kind": "receptacle", "level": "1", "at_ft": [30, 5], "issues": ["double_protection"], "fields": {"protection": {"value": "GFCI breaker and GFCI device", "source": "schedule"}}},
            {"id": "homerun:1:L1/1", "kind": "homerun", "level": "1", "at_ft": [1, 1], "fields": {"conductors": {"value": "2#12 + 1#12G", "source": "note"}}}
        ]);
        s.execute("bspec.set", &json!({ "spec": { "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}], "tags": tags } })).unwrap();
    }

    #[test]
    fn tags_are_listed_changed_in_bulk_and_accepted() {
        let mut s = Session::new();
        with_tags(&mut s);
        let r = s.execute("tags.list", &json!({ "unresolved": true })).unwrap();
        assert_eq!(r["total"], json!(3));
        assert_eq!(r["byIssue"]["untagged"], json!(1));
        let r = s.execute("tags.list", &json!({ "kind": "luminaire", "limit": 1 })).unwrap();
        assert_eq!((r["tags"].as_array().unwrap().len(), r["next"].clone()), (1, json!(1)));
        // Bulk: every luminaire gets type A (the user's), and their issues are settled.
        let r = s.execute("tags.set", &json!({ "kind": "luminaire", "field": "type", "value": "A" })).unwrap();
        assert_eq!((r["changed"].clone(), r["resolved"].clone()), (json!(2), json!(2)));
        let r = s.execute("tags.list", &json!({ "kind": "luminaire" })).unwrap();
        assert!(r["tags"].as_array().unwrap().iter().all(|t| t["fields"]["type"]["source"] == json!("user") && t["issues"].is_null()), "{r}");
        // By id: the double-protected receptacle becomes a standard device on its GFCI breaker.
        s.execute("tags.set", &json!({ "ids": ["device:1:30.0,5.0"], "field": "device_type", "value": "standard duplex receptacle" })).unwrap();
        let r = s.execute("tags.list", &json!({ "unresolved": true })).unwrap();
        assert_eq!(r["total"], json!(0), "{r}");
        let r = s.execute("tags.accept", &json!({ "kind": "homerun" })).unwrap();
        assert_eq!(r["accepted"], json!(1));
        // Undo takes the acceptance back.
        s.execute("undo", &json!({})).unwrap();
        let r = s.execute("tags.list", &json!({ "kind": "homerun" })).unwrap();
        assert!(r["tags"][0]["reviewed"].is_null(), "{r}");
    }

    #[test]
    fn bad_calls_are_refused() {
        let mut s = Session::new();
        with_tags(&mut s);
        assert!(s.execute("tags.set", &json!({ "field": "type", "value": "A" })).is_err(), "no ids and no filter");
        assert!(s.execute("tags.set", &json!({ "kind": "luminaire", "value": "A" })).is_err());
        assert!(s.execute("tags.set", &json!({ "kind": "luminaire", "field": "type" })).is_err());
        assert!(s.execute("tags.set", &json!({ "ids": "x", "field": "type", "value": "A" })).is_err());
        assert!(s.execute("tags.accept", &json!({})).is_err());
        let r = s.execute("tags.list", &json!({ "limit": -1, "cursor": u64::MAX })).unwrap();
        assert_eq!(r["tags"], json!([]));
    }
}
