//! What each element is, from the documents, with the source of every field and what is still
//! unresolved (Q299): written by the print readers (`cadaddy-mepfix`'s `spectag`), reviewed with
//! `tags.list` / `tags.set` / `tags.accept`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A field's value and where it came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub value: String,
    /// `plan`, `schedule`, `note`, `rule`, `default`, `user`.
    pub source: String,
}

/// One element and what the documents say it is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElementTag {
    /// Stable per element: `device:<level>:<x>,<y>`, `homerun:<level>:<label>`, `conduit:<level>:<n>`.
    pub id: String,
    /// `luminaire`, `receptacle`, `switch`, `data_outlet`…, `homerun`, `conduit`.
    pub kind: String,
    pub level: String,
    /// Where it is drawn, building feet.
    pub at_ft: [f64; 2],
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Field>,
    /// What's unresolved: `untagged`, `tag_not_in_schedule`, `missing_protection`,
    /// `double_protection`, `no_conductor_note`, `two_rules_apply`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<String>,
    /// A person (or an agent) has checked it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reviewed: bool,
}

impl ElementTag {
    pub fn set(&mut self, name: &str, value: impl Into<String>, source: &str) {
        self.fields.insert(name.to_string(), Field { value: value.into(), source: source.into() });
    }
}

/// A function that reads electrical sheets and returns element tags. Registered on the session
/// by the code that has the reader pipeline (tests or the app), called by `tags.read`.
pub type TagReaderFn = std::sync::Arc<dyn Fn(&cadaddy_doc::projrules::ProjectRules) -> std::result::Result<Vec<ElementTag>, String> + Send + Sync>;

/// The issues a field's value settles: setting it resolves them.
pub fn issues_settled_by(field: &str) -> &'static [&'static str] {
    match field {
        "type" | "luminaire" => &["untagged", "tag_not_in_schedule"],
        "protection" | "device_type" => &["missing_protection", "double_protection"],
        "conductors" => &["no_conductor_note"],
        "wiring_method" => &["two_rules_apply", "method_choice_open"],
        _ => &[],
    }
}

/// Merge new tags into old, keeping fields with `source: "user"` and `reviewed` flags on
/// elements matched by a stable key (kind + level + position within 0.5 ft, or homerun label).
pub fn merge_keeping_overrides(old: &[ElementTag], new: &[ElementTag]) -> Vec<ElementTag> {
    let mut out = Vec::with_capacity(new.len());
    for n in new {
        let prev = old.iter().find(|o| {
            o.kind == n.kind
                && o.level == n.level
                && if o.kind == "homerun" { o.id == n.id } else { (o.at_ft[0] - n.at_ft[0]).hypot(o.at_ft[1] - n.at_ft[1]) <= 0.5 }
        });
        let mut merged = n.clone();
        if let Some(prev) = prev {
            // Keep user-set fields and reviewed flag.
            for (k, f) in &prev.fields {
                if f.source == "user" {
                    merged.fields.insert(k.clone(), f.clone());
                    // Remove issues the user's field settles.
                    for settled in issues_settled_by(k) {
                        merged.issues.retain(|i| i != settled);
                    }
                }
            }
            if prev.reviewed {
                merged.reviewed = true;
            }
        }
        out.push(merged);
    }
    out
}
