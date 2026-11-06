//! A job's own words in the readers' word lists (Q6210). The lists are rule data,
//! `wordrules.json` beside this file and `coderules.json`; their loader is
//! `cadaddy_mepfix::wordrules`, shared with the print readers below the engine.
//!
//! A job adds words with a project rule, domain `custom`, key `words:<list>` (`words:rooms.sleeping`),
//! value the words (`DORM; BUNK ROOM`, or a JSON list; `WORD=VALUE` rows for a `pairs` list,
//! `VALUE=WORD` for `groups` and `rules`). `project.rules.set` refuses a list the data doesn't
//! have or a value that can't be read; a guided job's learned rule (`words:…`) is set the same way.
//! Before every command the engine puts the active drawing's additions in force ([`sync`]).

use std::ops::Bound;

use cadaddy_doc::projrules::{ProjectRules, RuleDomain};
use cadaddy_mepfix::wordrules as wr;
use serde_json::Value;

use crate::Session;

/// Most addition rules read from one drawing.
const MAX_RULES: usize = 500;

/// Whether a project rule's key adds words to a list.
pub(crate) fn is_words_key(key: &str) -> bool {
    key.starts_with(wr::RULE_PREFIX)
}

/// Why a `words:<list>` rule can't be set (an unknown list, a value that doesn't read), if so.
pub(crate) fn check_words_rule(domain: RuleDomain, key: &str, value: &str) -> std::result::Result<(), String> {
    if domain != RuleDomain::Custom {
        return Err(format!("`{key}` adds words to a list: set it in the `custom` domain"));
    }
    let list = key.get(wr::RULE_PREFIX.len()..).unwrap_or_default().trim();
    let data = wr::builtin();
    if data.shape(list).is_none() {
        let near: Vec<&str> = data
            .keys()
            .into_iter()
            .map(|(k, _)| k)
            .filter(|k| list.split('.').next().is_some_and(|s| !s.is_empty() && k.starts_with(s)))
            .take(12)
            .collect();
        let hint = if near.is_empty() { String::new() } else { format!(" (lists starting so: {})", near.join(", ")) };
        return Err(format!("no word list `{list}` in {}{hint}", wr::FILE));
    }
    let t = data.with_additions(&[(list.to_string(), value.to_string())]);
    match t.problems().first() {
        Some(p) => Err(p.clone()),
        None => Ok(()),
    }
}

/// A drawing's additions, `(list, value)` in key order.
pub(crate) fn additions(rules: &ProjectRules) -> Vec<(String, String)> {
    let lo = format!("{}/{}", RuleDomain::Custom.as_str(), wr::RULE_PREFIX);
    // The next key after every `custom/words:…` (`;` follows `:`).
    let hi = format!("{}/{}", RuleDomain::Custom.as_str(), wr::RULE_PREFIX.replace(':', ";"));
    rules
        .rules
        .range::<str, _>((Bound::Included(lo.as_str()), Bound::Excluded(hi.as_str())))
        .take(MAX_RULES)
        .filter_map(|(_, e)| Some((e.key.get(wr::RULE_PREFIX.len()..)?.trim().to_string(), e.active.value.clone())))
        .collect()
}

/// Put the active drawing's additions in force (none: the shipped lists).
pub(crate) fn sync(s: &Session) {
    match s.doc() {
        Ok(d) => wr::install(&additions(&d.project_rules)),
        Err(_) => wr::install(&[]),
    }
}

/// The lists in force and what went wrong with them, for a report.
pub(crate) fn report_json() -> Value {
    serde_json::to_value(wr::report()).unwrap_or(Value::Null)
}
