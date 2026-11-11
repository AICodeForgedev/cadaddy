//! The trade of a built element (Q6528): the one trade that installs it, which its takeoff and
//! BOM line, its model tree trade and its bid all go by.
//!
//! - A run, fitting, terminal or support takes the trade of its system's kind: duct mechanical;
//!   pipe plumbing, a fire protection system's pipe (sprinkler, standpipe, FP) fire protection;
//!   conduit, cable, tray and grounding electrical ([`of_system`]).
//! - A piece of equipment ([`of_equipment`]): its own TRADE property when a document gave one;
//!   else the first rule whose words are whole words of its kind (a pull box, wireway, pole, EV
//!   charger, BDA box or breaker electrical; a fire pump fire protection; a water heater
//!   plumbing); else its systems' trade when they all agree; else mechanical.
//!
//! The words are rules data (`elemtrade.json`), no job's values.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

const RULES: &str = include_str!("elemtrade.json");

pub(crate) const ELECTRICAL: &str = "electrical";
pub(crate) const MECHANICAL: &str = "mechanical";
pub(crate) const PLUMBING: &str = "plumbing";
pub(crate) const FIRE_PROTECTION: &str = "fire protection";

/// The trades an element can be of, as `&'static str` (the BOM's lines keep them so).
const TRADES: [&str; 4] = [ELECTRICAL, FIRE_PROTECTION, PLUMBING, MECHANICAL];

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rule {
    trade: String,
    words: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    aliases: BTreeMap<String, String>,
    equipment: Vec<Rule>,
    default_equipment: String,
    systems: BTreeMap<String, String>,
    fire_pipe: String,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// Lower-case whole words of `s` (`pull_box` → `pull box`).
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).collect()
}

/// Whether the words of `phrase` stand together among `hay`.
fn has_phrase(hay: &[String], phrase: &str) -> bool {
    let p = words(phrase);
    !p.is_empty() && hay.windows(p.len()).any(|w| w == p.as_slice())
}

/// One of the four trades by its name or an alias (`electric`, `fire alarm`, `hvac`).
pub(crate) fn named(trade: &str) -> Option<&'static str> {
    let t = words(trade).join(" ");
    if t.is_empty() {
        return None;
    }
    let t = rules().aliases.get(&t).map_or(t.as_str(), String::as_str);
    TRADES.iter().copied().find(|x| *x == t)
}

/// A system's name without the provenance a takeoff row adds after it (`E-1 (inferred)`).
pub(crate) fn base_system(name: &str) -> &str {
    let n = name.trim_end();
    for made in ["(derived)", "(inferred)", "(drawn)"] {
        if let Some(head) = n.strip_suffix(made) {
            return head.trim_end();
        }
    }
    n
}

/// The trade of a system of kind `kind` (`duct`, `pipe`, `conduit`, `cable`, `tray`,
/// `grounding`) named `name`; `None` for a kind no rule names.
pub(crate) fn of_system(kind: &str, name: &str) -> Option<&'static str> {
    let k = kind.trim().to_ascii_lowercase();
    let r = rules();
    if k == "pipe" && super::bimlook::pipe_trade(base_system(name)) == "fire" {
        return named(&r.fire_pipe);
    }
    r.systems.get(&k).and_then(|t| named(t))
}

/// The trade of a piece of equipment of kind `kind`, with its own TRADE property `own` (empty:
/// none) and the trades of the systems it is on (`None`: a system of no known kind).
pub(crate) fn of_equipment(kind: &str, own: &str, systems: &[Option<&'static str>]) -> &'static str {
    if let Some(t) = named(own) {
        return t;
    }
    let r = rules();
    let w = words(kind);
    if let Some(t) = r.equipment.iter().find(|x| x.words.iter().any(|p| has_phrase(&w, p))).and_then(|x| named(&x.trade)) {
        return t;
    }
    let mut known = systems.iter().flatten();
    if let Some(first) = known.next()
        && known.all(|t| t == first)
        && systems.iter().all(Option::is_some)
    {
        return first;
    }
    named(&r.default_equipment).unwrap_or(MECHANICAL)
}

/// The model tree's trade row (bimlook) of equipment of `trade`.
pub(crate) fn equipment_tree_key(trade: &str) -> &'static str {
    match named(trade) {
        Some(ELECTRICAL) => "gear",
        Some(FIRE_PROTECTION) => "fire",
        Some(PLUMBING) => "plumbing_equipment",
        _ => "equipment",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_read() {
        let r = rules();
        assert!(!r.equipment.is_empty() && !r.systems.is_empty(), "elemtrade.json reads");
        assert!(r.equipment.iter().all(|x| named(&x.trade).is_some()), "every rule's trade is one of the four");
        assert!(r.systems.values().all(|t| named(t).is_some()));
    }

    #[test]
    fn equipment_by_kind_systems_and_own_trade() {
        for k in ["pull_box", "wireway", "pole", "ev_charger", "bda_riser_box", "bda_antenna_box", "panel", "breaker", "switchboard"] {
            assert_eq!(of_equipment(k, "", &[]), ELECTRICAL, "{k}");
        }
        assert_eq!(of_equipment("fire_pump", "", &[]), FIRE_PROTECTION);
        assert_eq!(of_equipment("water_heater", "", &[]), PLUMBING);
        assert_eq!(of_equipment("water_meter", "", &[]), PLUMBING, "a water meter is not the electrical meter");
        assert_eq!(of_equipment("ahu", "", &[Some(MECHANICAL)]), MECHANICAL);
        assert_eq!(of_equipment("unit", "", &[Some(ELECTRICAL), Some(ELECTRICAL)]), ELECTRICAL, "on raceway systems only");
        assert_eq!(of_equipment("unit", "", &[Some(ELECTRICAL), Some(MECHANICAL)]), MECHANICAL, "systems that disagree decide nothing");
        assert_eq!(of_equipment("unit", "", &[Some(ELECTRICAL), None]), MECHANICAL, "an unknown system decides nothing");
        assert_eq!(of_equipment("ahu", "Electrical", &[]), ELECTRICAL, "its own trade wins");
        assert_eq!(of_equipment("", "", &[]), MECHANICAL);
    }

    #[test]
    fn systems_and_provenance_suffixes() {
        assert_eq!(base_system("E-1 (inferred)"), "E-1");
        assert_eq!(base_system("CW (derived) "), "CW");
        assert_eq!(base_system("SA"), "SA");
        assert_eq!(of_system("pipe", "FP-MAIN (inferred)"), Some(FIRE_PROTECTION));
        assert_eq!(of_system("pipe", "SPRINKLER"), Some(FIRE_PROTECTION));
        assert_eq!(of_system("pipe", "DCW"), Some(PLUMBING));
        assert_eq!(of_system("duct", "SA"), Some(MECHANICAL));
        assert_eq!(of_system("tray", "T-1"), Some(ELECTRICAL));
        assert_eq!(of_system("conduit", "E-FDR"), Some(ELECTRICAL));
        assert_eq!(of_system("", "X"), None);
        assert_eq!(equipment_tree_key(ELECTRICAL), "gear");
        assert_eq!(equipment_tree_key(MECHANICAL), "equipment");
    }

    #[test]
    fn hostile_names_never_panic() {
        let long = "x_".repeat(50_000);
        assert_eq!(of_equipment(&long, &long, &[None; 3]), MECHANICAL);
        assert_eq!(named("  "), None);
        assert_eq!(base_system(""), "");
    }
}
