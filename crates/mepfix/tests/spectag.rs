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
    for k in ["device_type", "protection", "circuit", "conductors", "wiring_method", "location", "type"] {
        let (p, r) = pr(k);
        assert!(totals.get(k).is_some_and(|x| x.0 > 0), "{k}: nothing in the truth");
        assert!(p >= 0.9 && r >= 0.9, "{k}: precision {p:.3} recall {r:.3}");
    }
    // Issues: the planted problems (a handful per set) — none raised falsely, most found.
    for k in ["issue:double_protection", "issue:missing_protection", "issue:tag_not_in_schedule", "issue:untagged", "issue:two_rules_apply"] {
        let (p, r) = pr(k);
        assert!(totals.get(k).is_some_and(|x| x.0 > 0), "{k}: nothing in the truth");
        assert!(p >= 0.85 && r >= 0.7, "{k}: precision {p:.3} recall {r:.3}");
    }
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
