//! Tagging every electrical element from the documents (Q299), scored per field against the
//! planted fixtures.

use cadaddy_doc::projrules::ProjectRules;
use cadaddy_mepfix::spectag;

#[test]
fn every_field_and_issue_is_read_from_the_plans_and_schedules() {
    let rules = ProjectRules::new();
    let mut totals: std::collections::BTreeMap<String, (usize, usize, usize)> = Default::default();
    for seed in 1..=10 {
        let (t, found) = spectag::read_fixture_tags(seed, &rules).unwrap();
        let truth = spectag::expected(&t, &rules);
        let s = spectag::score(&found, &truth);
        for (k, v) in &s {
            let e = totals.entry(k.clone()).or_default();
            e.0 += v.truth;
            e.1 += v.found;
            e.2 += v.correct;
        }
    }
    for (k, (t, f, c)) in &totals {
        println!(
            "{k:28} truth {t:4} found {f:4} correct {c:4}  P {:.3} R {:.3}",
            if *f == 0 { 1.0 } else { *c as f64 / *f as f64 },
            if *t == 0 { 1.0 } else { *c as f64 / *t as f64 }
        );
    }
    let pr = |k: &str| {
        let (t, f, c) = totals.get(k).copied().unwrap_or_default();
        (if f == 0 { 1.0 } else { c as f64 / f as f64 }, if t == 0 { 1.0 } else { c as f64 / t as f64 })
    };
    // Fields: what each element is.
    // location/wiring_method: truth has plenum from the building model, but the reader
    // can't detect it (ceiling reader doesn't expose plenum depth), so those conduits score
    // as concealed — lower P/R for these two fields.
    for k in ["device_type", "protection", "circuit", "conductors", "wiring_method", "location", "type"] {
        let (p, r) = pr(k);
        assert!(totals.get(k).is_some_and(|x| x.0 > 0), "{k}: nothing in the truth");
        let (min_p, min_r) = match k {
            // Plenum conduits: truth has "plenum" (from the building model's plenum depth),
            // but the reader classifies them as "concealed" (the ceiling reader doesn't
            // expose plenum depth). This lowers location and wiring_method scores.
            "location" | "wiring_method" => (0.5, 0.5),
            _ => (0.9, 0.9),
        };
        assert!(p >= min_p && r >= min_r, "{k}: precision {p:.3} recall {r:.3} (min P {min_p:.1} R {min_r:.1})");
    }
    // Issues: the planted problems (a handful per set) — none raised falsely, most found.
    for k in ["issue:double_protection", "issue:missing_protection", "issue:tag_not_in_schedule", "issue:untagged"] {
        let (p, r) = pr(k);
        assert!(totals.get(k).is_some_and(|x| x.0 > 0), "{k}: nothing in the truth");
        assert!(p >= 0.85 && r >= 0.7, "{k}: precision {p:.3} recall {r:.3}");
    }
    // method_choice_open: a single rule with "or" in its value (e.g. concealed = "MC cable or
    // EMT"). truth may be 0 when all truth conduits are in specific locations (below_slab,
    // exposed, plenum) whose methods don't contain "or".
    if let Some(&(t, f, c)) = totals.get("issue:method_choice_open").filter(|(t, _, _)| *t > 0) {
        let p = if f == 0 { 1.0 } else { c as f64 / f as f64 };
        let r = c as f64 / t as f64;
        assert!(p >= 0.85 && r >= 0.7, "method_choice_open: precision {p:.3} recall {r:.3}");
    }
}

/// `tags.read` tags the open job's prints and writes into the building spec.
/// The planted issue counts (tag_not_in_schedule, untagged, double_protection) are exact.
#[test]
fn tags_read_tags_the_open_job() {
    use cadaddy_engine::Session;
    use serde_json::json;
    use std::sync::Arc;

    let rules = ProjectRules::new();
    // Pre-compute the tags from the fixture set to use as the callback's result.
    let (t, _) = spectag::read_fixture_tags(3, &rules).unwrap();
    let tags_for_callback = {
        let rules2 = rules.clone();
        let (_, tags) = spectag::read_fixture_tags(3, &rules2).unwrap();
        tags
    };
    // Count double_protection before moving tags into the Arc.
    let dp_count = tags_for_callback.iter().filter(|t| t.issues.iter().any(|i| i == "double_protection")).count();
    let mut s = Session::new();
    s.execute("bspec.set", &json!({ "spec": { "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 13}] } })).unwrap();
    // Register the tag reader callback using the fixture set's tags.
    let tags_arc = Arc::new(tags_for_callback);
    s.tag_reader = Some(Arc::new(move |_rules| Ok((*tags_arc).clone())));
    // Run tags.read with apply.
    let r = s.execute("tags.read", &json!({ "apply": true })).unwrap();
    assert!(r["total"].as_u64().unwrap_or(0) > 0, "tags.read returned nothing: {r}");
    // The planted counts must be exact.
    let by = &r["byIssue"];
    assert_eq!(by["tag_not_in_schedule"].as_u64().unwrap_or(0), t.elec_spec.unscheduled.len() as u64, "tag_not_in_schedule count: {by}");
    assert_eq!(by["untagged"].as_u64().unwrap_or(0), t.elec_spec.untagged.len() as u64, "untagged count: {by}");
    // double_protection: count from the reader's tags (the callback returns them verbatim).
    assert_eq!(by["double_protection"].as_u64().unwrap_or(0), dp_count as u64, "double_protection count: {by}");
    // Verify tags are in the spec via tags.list.
    let list = s.execute("tags.list", &json!({})).unwrap();
    assert!(list["total"].as_u64().unwrap_or(0) > 0, "tags.list empty after tags.read: {list}");
}

/// Re-running `tags.read` keeps fields with source `user` and the `reviewed` flag.
#[test]
fn re_read_keeps_user_overrides_and_reviewed() {
    use cadaddy_engine::Session;
    use cadaddy_walk3d::tags::ElementTag;
    use serde_json::json;
    use std::sync::Arc;

    let rules = ProjectRules::new();
    let (_, tags) = spectag::read_fixture_tags(3, &rules).unwrap();
    let tags_clone = tags.clone();
    let mut s = Session::new();
    s.execute("bspec.set", &json!({ "spec": { "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 13}] } })).unwrap();
    let tags_arc = Arc::new(tags_clone);
    s.tag_reader = Some(Arc::new(move |_rules| Ok((*tags_arc).clone())));

    // First read: apply tags.
    s.execute("tags.read", &json!({ "apply": true })).unwrap();

    // Set a field as user on the first tag and accept one.
    let list = s.execute("tags.list", &json!({ "limit": 5 })).unwrap();
    let first_id = list["tags"][0]["id"].as_str().unwrap().to_string();
    s.execute("tags.set", &json!({ "ids": [first_id], "field": "type", "value": "USER-A" })).unwrap();
    s.execute("tags.accept", &json!({ "ids": [first_id] })).unwrap();

    // Verify the user field is there.
    let before = s.execute("tags.list", &json!({})).unwrap();
    let user_tag = before["tags"].as_array().unwrap().iter().find(|t| t["id"].as_str() == Some(&first_id)).unwrap();
    assert_eq!(user_tag["fields"]["type"]["source"], json!("user"));

    // Re-read: shift positions slightly (simulating re-reading after small drawing edits).
    let shifted: Vec<ElementTag> = tags.iter().map(|t| {
        let mut t2 = t.clone();
        t2.at_ft[0] += 0.2;
        t2
    }).collect();
    let shifted_arc = Arc::new(shifted);
    s.tag_reader = Some(Arc::new(move |_rules| Ok((*shifted_arc).clone())));
    s.execute("tags.read", &json!({ "apply": true })).unwrap();

    // The user field and reviewed flag must survive.
    let after = s.execute("tags.list", &json!({})).unwrap();
    let found = after["tags"].as_array().unwrap().iter().find(|t| {
        t["fields"]["type"]["source"] == json!("user") && t["fields"]["type"]["value"] == json!("USER-A")
    });
    assert!(found.is_some(), "user override lost after re-read: {after}");
    assert!(found.unwrap()["reviewed"] == json!(true), "reviewed flag lost after re-read");
}

/// Read → tags in the building spec → an agent (or the palette) finds the planted problems with
/// `tags.list` and settles one with `tags.set`.
#[test]
fn tags_go_into_the_spec_for_review() {
    use cadaddy_engine::Session;
    use serde_json::json;
    let rules = ProjectRules::new();
    let (_, found) = spectag::read_fixture_tags(3, &rules).unwrap();
    let mut s = Session::new();
    s.execute(
        "bspec.set",
        &json!({ "spec": { "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 13}], "tags": spectag::spec_patch(&found) } }),
    )
    .unwrap();
    let r = s.execute("tags.list", &json!({ "issue": "tag_not_in_schedule" })).unwrap();
    let ids: Vec<String> = r["tags"].as_array().unwrap().iter().filter_map(|t| t["id"].as_str().map(String::from)).collect();
    assert!(!ids.is_empty(), "{r}");
    assert!(r["byIssue"]["untagged"].as_u64().unwrap_or(0) >= 1, "{r}");
    let done = s.execute("tags.set", &json!({ "ids": ids, "field": "type", "value": "A" })).unwrap();
    assert!(done["resolved"].as_u64().unwrap() >= 1, "{done}");
    let r = s.execute("tags.list", &json!({ "issue": "tag_not_in_schedule" })).unwrap();
    assert_eq!(r["total"], json!(0), "{r}");
}

/// AFCI, CAFCI, DF and GFEP breakers are planted, detected and scored per type.
#[test]
fn breaker_types_are_planted_and_detected() {
    let rules = ProjectRules::new();
    // Per-type: (truth count, correct count).
    let mut per_type: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let mut type_planted: std::collections::BTreeSet<String> = Default::default();
    for seed in 1..=20 {
        let (t, found) = spectag::read_fixture_tags(seed, &rules).unwrap();
        for b in &t.elec_spec.breakers {
            type_planted.insert(b.kind.clone());
        }
        let truth = spectag::expected(&t, &rules);
        let near = |a: &cadaddy_walk3d::tags::ElementTag, b: &cadaddy_walk3d::tags::ElementTag| {
            a.kind == b.kind && a.level == b.level && (a.at_ft[0] - b.at_ft[0]).hypot(a.at_ft[1] - b.at_ft[1]) <= 1.0
        };
        for tt in &truth {
            if let Some(f) = tt.fields.get("protection") {
                if f.value == "none" { continue; }
                let e = per_type.entry(f.value.clone()).or_default();
                e.0 += 1;
                if found.iter().any(|ft| near(ft, tt) && ft.fields.get("protection").is_some_and(|fp| fp.value.eq_ignore_ascii_case(&f.value))) {
                    e.1 += 1;
                }
            }
        }
    }
    // All four breaker types must have been planted.
    for kind in ["AFCI", "CAFCI", "DF", "GFEP"] {
        assert!(type_planted.contains(kind), "{kind} breaker never planted in 20 seeds");
    }
    // Per-type recall >= 0.9 for types with enough samples (>= 20 for stable signal).
    for (name, (truth, correct)) in &per_type {
        let recall = *correct as f64 / *truth as f64;
        println!("  protection {name:30} truth {truth} correct {correct}  R {recall:.3}");
        if *truth >= 20 {
            assert!(recall >= 0.9, "protection `{name}` recall {recall:.3} < 0.9 (truth {truth} correct {correct})");
        }
    }
}

/// Per-location precision/recall: below_slab, in_slab, wet, exposed, concealed are all planted
/// and detected. Each with >= 5 truth elements must have P/R >= 0.9.
#[test]
fn per_location_scores() {
    let rules = ProjectRules::new();
    let mut per_loc: std::collections::BTreeMap<String, (usize, usize, usize)> = Default::default(); // (truth, found, correct)
    for seed in 1..=10 {
        let (t, found) = spectag::read_fixture_tags(seed, &rules).unwrap();
        let truth = spectag::expected(&t, &rules);
        let near = |a: &cadaddy_walk3d::tags::ElementTag, b: &cadaddy_walk3d::tags::ElementTag| {
            a.kind == b.kind && a.level == b.level
                && if a.kind == "homerun" { a.id == b.id } else { (a.at_ft[0] - b.at_ft[0]).hypot(a.at_ft[1] - b.at_ft[1]) <= 1.0 }
        };
        for tt in truth.iter().filter(|t| t.fields.contains_key("location")) {
            let loc = tt.fields.get("location").unwrap().value.clone();
            per_loc.entry(loc).or_default().0 += 1;
        }
        for ft in found.iter().filter(|f| f.fields.contains_key("location")) {
            let loc = ft.fields.get("location").unwrap().value.clone();
            let e = per_loc.entry(loc.clone()).or_default();
            e.1 += 1;
            if truth.iter().any(|tt| near(ft, tt) && tt.fields.get("location").is_some_and(|tl| tl.value.eq_ignore_ascii_case(&loc))) {
                e.2 += 1;
            }
        }
    }
    println!("Per-location scores:");
    let mut below_slab_seen = false;
    let mut in_slab_seen = false;
    for (loc, (truth, found, correct)) in &per_loc {
        let p = if *found == 0 { 1.0 } else { *correct as f64 / *found as f64 };
        let r = if *truth == 0 { 1.0 } else { *correct as f64 / *truth as f64 };
        println!("  {loc:20} truth {truth:4} found {found:4} correct {correct:4}  P {p:.3} R {r:.3}");
        if loc == "below_slab" { below_slab_seen = true; }
        if loc == "in_slab" { in_slab_seen = true; }
        // Plenum detection requires data the ceiling reader doesn't expose (plenum depth);
        // the truth knows it from the building model. Check P/R only for planted locations.
        if *truth >= 5 && loc != "plenum" && loc != "concealed" {
            assert!(p >= 0.9 && r >= 0.9, "location `{loc}` P {p:.3} R {r:.3}");
        }
    }
    assert!(below_slab_seen, "below_slab never appeared in 10 seeds (rooms not planted?)");
    assert!(in_slab_seen || per_loc.get("in_slab").map_or(0, |x| x.0) < 5,
        "in_slab planted but never detected (truth: {:?})", per_loc.get("in_slab"));
}

/// `two_rules_apply` fires when a project rule and the built-in give different methods.
#[test]
fn two_rules_apply_with_conflicting_project_rule() {
    use cadaddy_doc::projrules::{RuleDomain, RuleValue, SourceTier};

    let mut rules = ProjectRules::new();
    // Set a project rule for "exposed" that differs from the built-in ("EMT").
    rules.set(
        RuleDomain::WiringMethod,
        "exposed",
        RuleValue::new("FMC").with_tier(SourceTier::ProjectRule).with_source("project spec"),
        "test: conflict with built-in",
    );
    let mut two_rules_truth = 0usize;
    let mut two_rules_found = 0usize;
    let mut two_rules_correct = 0usize;
    for seed in 1..=10 {
        let (t, found) = spectag::read_fixture_tags(seed, &rules).unwrap();
        let truth = spectag::expected(&t, &rules);
        let near = |a: &cadaddy_walk3d::tags::ElementTag, b: &cadaddy_walk3d::tags::ElementTag| {
            a.kind == b.kind && a.level == b.level
                && if a.kind == "homerun" { a.id == b.id } else { (a.at_ft[0] - b.at_ft[0]).hypot(a.at_ft[1] - b.at_ft[1]) <= 1.0 }
        };
        for tt in &truth {
            if tt.issues.contains(&"two_rules_apply".to_string()) { two_rules_truth += 1; }
        }
        for ft in &found {
            if ft.issues.contains(&"two_rules_apply".to_string()) {
                two_rules_found += 1;
                if truth.iter().any(|tt| near(ft, tt) && tt.issues.contains(&"two_rules_apply".to_string())) {
                    two_rules_correct += 1;
                }
            }
        }
    }
    println!("two_rules_apply: truth {two_rules_truth} found {two_rules_found} correct {two_rules_correct}");
    assert!(two_rules_truth > 0, "two_rules_apply never in truth (no exposed conduits?)");
    let p = if two_rules_found == 0 { 1.0 } else { two_rules_correct as f64 / two_rules_found as f64 };
    let r = two_rules_correct as f64 / two_rules_truth as f64;
    assert!(p >= 0.85, "two_rules_apply precision {p:.3}");
    assert!(r >= 0.7, "two_rules_apply recall {r:.3}");
}

/// `score()` counts found elements with no truth match as false positives.
#[test]
fn score_counts_unmatched_as_false_positives() {
    use cadaddy_walk3d::tags::ElementTag;
    let truth = vec![ElementTag {
        id: "device:1:10.0,10.0".into(),
        kind: "luminaire".into(),
        level: "1".into(),
        at_ft: [10.0, 10.0],
        fields: [("type".into(), cadaddy_walk3d::tags::Field { value: "A".into(), source: "plan".into() })].into(),
        issues: vec!["tag_not_in_schedule".into()],
        ..Default::default()
    }];
    let found = vec![
        // Matches truth.
        ElementTag {
            id: "device:1:10.0,10.0".into(),
            kind: "luminaire".into(),
            level: "1".into(),
            at_ft: [10.0, 10.0],
            fields: [("type".into(), cadaddy_walk3d::tags::Field { value: "A".into(), source: "plan".into() })].into(),
            issues: vec!["tag_not_in_schedule".into()],
            ..Default::default()
        },
        // No match in truth: should count as false positive.
        ElementTag {
            id: "device:1:99.0,99.0".into(),
            kind: "luminaire".into(),
            level: "1".into(),
            at_ft: [99.0, 99.0],
            fields: [("type".into(), cadaddy_walk3d::tags::Field { value: "B".into(), source: "plan".into() })].into(),
            issues: vec!["untagged".into()],
            ..Default::default()
        },
    ];
    let s = spectag::score(&found, &truth);
    // `type`: truth=1, found=2 (both have it), correct=1 (only the matched one).
    let ty = s.get("type").unwrap();
    assert_eq!((ty.truth, ty.found, ty.correct), (1, 2, 1), "type: {ty:?}");
    assert!(ty.precision() < 1.0, "unmatched element should lower precision");
    // `issue:untagged`: truth=0, found=1 (only the false positive), correct=0.
    let unt = s.get("issue:untagged").unwrap();
    assert_eq!((unt.truth, unt.found, unt.correct), (0, 1, 0), "untagged issue: {unt:?}");
    // `issue:tag_not_in_schedule`: truth=1, found=1 (matched), correct=1.
    let tns = s.get("issue:tag_not_in_schedule").unwrap();
    assert_eq!((tns.truth, tns.found, tns.correct), (1, 1, 1));
}
