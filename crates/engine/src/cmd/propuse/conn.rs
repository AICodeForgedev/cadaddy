//! What serves an element, judged by its trade (Q6505): the connection checks of
//! `bim.props.check` go by the system an element is on, not one rule for every terminal.
//!
//! - An air terminal is served by the air unit its links name (`served_by`, `fed_from`, `unit`),
//!   else the unit its built duct network (`connects`) reaches.
//! - A sprinkler head is served by a fire protection path: a pipe of a fire protection system
//!   (rules data `connections.json` `fire_protection.systems`) its links or its pipe network
//!   reach. It is never looked for an air unit.
//! - Only equipment whose type takes power is looked for a circuit: rules data
//!   `connections.json` `power` (its type words and marks, or any electrical rating it carries);
//!   manifolds, hoods, louvers and dampers with no actuator take none.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{El, Model, is_panel};

const RULES: &str = include_str!("connections.json");

/// Most links read from one element's `connects`.
const MAX_LINKS: usize = 64;
/// Most elements joined into the networks (hostile input).
const MAX_ELEMENTS: usize = 2_000_000;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Power {
    pub needs_words: Vec<String>,
    pub needs_marks: Vec<String>,
    pub rating_keys: Vec<String>,
    pub none_words: Vec<String>,
    pub source: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FireProtection {
    pub systems: Vec<String>,
    pub source: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub power: Power,
    pub fire_protection: FireProtection,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// Text as space-separated lower-case words (`exhaust_fan` is `exhaust fan`).
fn normal(t: &str) -> String {
    let mut out = String::with_capacity(t.len() + 2);
    out.push(' ');
    let mut space = true;
    for c in t.chars().take(4_000) {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out
}

/// The longest of `phrases` that starts a word of `text` (as [`normal`] gives it), by length.
fn longest(text: &str, phrases: &[String]) -> usize {
    phrases.iter().map(|p| normal(p)).filter(|p| !p.trim().is_empty() && text.contains(p.as_str())).map(|p| p.trim().len()).max().unwrap_or(0)
}

/// Whether equipment takes a power circuit, and why: `Some(true)` it does, `Some(false)` its type
/// takes none, `None` its type does not say.
pub(crate) fn needs_power(el: &El) -> (Option<bool>, &'static str) {
    let r = &rules().power;
    if r.rating_keys.iter().any(|k| el.get(k).is_some()) {
        return (Some(true), "it carries an electrical rating");
    }
    let text = normal(&el.words());
    let needs = longest(&text, &r.needs_words);
    let none = longest(&text, &r.none_words);
    if none > 0 && none >= needs {
        return (Some(false), "its type takes no power");
    }
    if needs > 0 {
        return (Some(true), "its type takes power");
    }
    let mark =
        |v: Option<String>| -> String { v.unwrap_or_default().trim().to_ascii_uppercase().chars().take_while(char::is_ascii_alphabetic).collect() };
    let marks = [mark(el.text("type_mark")), mark(el.tag())];
    if marks.iter().any(|m| !m.is_empty() && r.needs_marks.iter().any(|x| x.eq_ignore_ascii_case(m))) {
        return (Some(true), "its type mark is equipment that takes power");
    }
    (None, "its type does not say")
}

/// The system an element is on: its `system` (or `service`) property, else the build's.
pub(crate) fn system_of(el: &El) -> Option<String> {
    el.first(&["system", "service"]).or_else(|| super::super::bimlook::made_tag(el.e, "system").map(str::to_string)).filter(|s| !s.is_empty())
}

/// Whether a system name is fire protection (rules data).
pub(crate) fn is_fire_protection(system: &str) -> bool {
    let s = system.trim();
    !s.is_empty() && rules().fire_protection.systems.iter().any(|x| x.eq_ignore_ascii_case(s))
}

/// Whether an element is an electrical element (its network is the circuits', not the ducts' or
/// pipes').
fn electrical(el: &El) -> bool {
    matches!(el.kind.as_str(), "device" | "light_fixture" | "conduit" | "wire" | "cable" | "panel")
}

/// The ducts' and pipes' networks: elements joined by their links, equipment at their ends.
pub(crate) struct Nets {
    /// Each element's network (its root), for elements in one.
    root: Vec<usize>,
    /// The equipment each network ends at.
    units: BTreeMap<usize, Vec<usize>>,
    /// Each network's fire protection pipes.
    fire_pipes: BTreeMap<usize, usize>,
}

/// An element's network root. A parent is always an element before it (a union points the later
/// root at the earlier), so the walk ends.
fn find(root: &mut [usize], mut i: usize) -> usize {
    while let Some(&p) = root.get(i) {
        if p >= i {
            break;
        }
        // Halve the path.
        let gp = root.get(p).copied().unwrap_or(p);
        if let Some(slot) = root.get_mut(i) {
            *slot = gp;
        }
        i = gp;
    }
    i
}

/// The elements an element names through `connects`, `served_by`, `fed_from` and `unit`.
fn links(m: &Model, el: &El) -> Vec<usize> {
    let mut out: Vec<usize> = el
        .text("connects")
        .map(|t| t.split([',', ' ', ';']).filter(|x| !x.trim().is_empty()).take(MAX_LINKS).filter_map(|x| m.resolve(x)).collect())
        .unwrap_or_default();
    out.extend(["served_by", "fed_from", "unit"].iter().filter_map(|k| el.text(k)).filter_map(|v| m.resolve(&v)));
    out
}

impl Nets {
    pub(crate) fn new(m: &Model) -> Nets {
        let n = m.els.len().min(MAX_ELEMENTS);
        let mut root: Vec<usize> = (0..n).collect();
        let mut ends: Vec<(usize, usize)> = Vec::new();
        let carrier = |i: usize| m.get(i).is_some_and(|e| e.kind != "equipment" && !electrical(e));
        for i in 0..n {
            let Some(el) = m.get(i) else { continue };
            if electrical(el) {
                continue;
            }
            for j in links(m, el).into_iter().filter(|j| *j < n && *j != i) {
                let Some(other) = m.get(j) else { continue };
                match (el.kind == "equipment", other.kind == "equipment") {
                    // Equipment ends a network: two networks through one unit stay two.
                    (false, true) if !is_panel(other) => ends.push((i, j)),
                    (true, false) if !is_panel(el) && carrier(j) => ends.push((j, i)),
                    (false, false) if carrier(j) => {
                        let (a, b) = (find(&mut root, i), find(&mut root, j));
                        if a != b
                            && let Some(slot) = root.get_mut(a.max(b))
                        {
                            *slot = a.min(b);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut units: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, unit) in ends {
            let r = find(&mut root, i);
            let v = units.entry(r).or_default();
            if !v.contains(&unit) {
                v.push(unit);
            }
        }
        let mut fire_pipes: BTreeMap<usize, usize> = BTreeMap::new();
        for i in 0..n {
            let Some(el) = m.get(i) else { continue };
            if el.kind == "pipe" && system_of(el).is_some_and(|s| is_fire_protection(&s)) {
                let r = find(&mut root, i);
                *fire_pipes.entry(r).or_default() += 1;
            }
        }
        Nets { root, units, fire_pipes }
    }

    fn root_of(&self, mut i: usize) -> usize {
        while let Some(&p) = self.root.get(i) {
            if p >= i {
                break;
            }
            i = p;
        }
        i
    }

    /// The air unit element `i`'s duct network ends at (the first), if any.
    pub(crate) fn unit_of(&self, i: usize) -> Option<usize> {
        self.units.get(&self.root_of(i)).and_then(|v| v.first().copied())
    }

    /// Whether element `i` (a sprinkler head) is on a network with a fire protection pipe, other
    /// than itself.
    pub(crate) fn on_fire_pipe(&self, m: &Model, i: usize) -> bool {
        let own = m.get(i).is_some_and(|e| e.kind == "pipe" && system_of(e).is_some_and(|s| is_fire_protection(&s)));
        self.fire_pipes.get(&self.root_of(i)).is_some_and(|n| *n > usize::from(own))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_load() {
        let r = rules();
        assert!(!r.power.needs_words.is_empty() && !r.power.none_words.is_empty() && !r.fire_protection.systems.is_empty());
        assert!(!r.power.source.is_empty() && !r.fire_protection.source.is_empty());
    }

    #[test]
    fn phrases_match_at_word_starts_and_the_longest_decides() {
        let t = normal("Tankless_Water-Heater");
        let r = &rules().power;
        assert!(longest(&t, &r.needs_words) > longest(&t, &r.none_words), "a tankless water heater takes power ({t})");
        let t = normal("RELIEF HOOD");
        assert!(longest(&t, &r.none_words) > longest(&t, &r.needs_words));
        assert_eq!(longest(&normal("infant"), &["fan".to_string()]), 0, "not inside a word");
        assert!(longest(&normal("fans"), &["fan".to_string()]) > 0);
        assert_eq!(longest(&normal(""), &[String::new()]), 0);
    }

    #[test]
    fn fire_protection_systems_come_from_the_rules() {
        assert!(is_fire_protection("FP") && is_fire_protection(" sprinkler "));
        assert!(!is_fire_protection("CW") && !is_fire_protection("") && !is_fire_protection("SA"));
    }
}
