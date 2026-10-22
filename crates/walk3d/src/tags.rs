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

/// The issues a field's value settles: setting it resolves them.
pub fn issues_settled_by(field: &str) -> &'static [&'static str] {
    match field {
        "type" | "luminaire" => &["untagged", "tag_not_in_schedule"],
        "protection" | "device_type" => &["missing_protection", "double_protection"],
        "conductors" => &["no_conductor_note"],
        "wiring_method" => &["two_rules_apply"],
        _ => &[],
    }
}
