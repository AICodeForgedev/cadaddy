//! Which MEP systems are one service (Q6598): a service drawn as two layers or systems (a
//! sprinkler main and its branch lines, a water main and its branches, a riser and its floor
//! branches) is one piping or duct service, so a run of one that ends on a run of the other gets
//! its tee, cross or reducer and the two are connected. The rules are data (`mep_ties.json`):
//! pipes by what they carry ([`crate::pipeservice`]) and the circuit of it (supply and return,
//! suction and liquid never connect), ducts by their duct type, anything else by its name less
//! the words for a run's part in its service (`main`, `branch`, `riser`…).

use std::sync::OnceLock;

use serde::Deserialize;

use crate::{DuctType, MepKind, MepSystem};

/// Most words, services and circuits read from the data.
const MAX_WORDS: usize = 128;
const MAX_RULES: usize = 64;
/// The longest name compared (hostile input).
const MAX_NAME: usize = 200;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Circuit {
    key: String,
    contains: Vec<String>,
    words: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    roles: Vec<String>,
    loose_services: Vec<String>,
    circuits: Vec<Circuit>,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut r: Rules = serde_json::from_str(include_str!("mep_ties.json")).unwrap_or_default();
        r.roles.truncate(MAX_WORDS);
        r.loose_services.truncate(MAX_RULES);
        r.circuits.truncate(MAX_RULES);
        for c in &mut r.circuits {
            c.contains.truncate(MAX_WORDS);
            c.words.truncate(MAX_WORDS);
        }
        r
    })
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).collect()
}

fn kind_str(kind: MepKind) -> &'static str {
    match kind {
        MepKind::Duct => "duct",
        MepKind::Pipe => "pipe",
        MepKind::Conduit => "conduit",
        MepKind::Tray => "tray",
        MepKind::Cable => "cable",
    }
}

/// The circuit of its service a pipe system's name says (`return`, `suction`…), "" for the
/// supply side (a name that says none).
fn circuit(name: &str) -> &'static str {
    let low = name.to_ascii_lowercase();
    let w = words(name);
    rules()
        .circuits
        .iter()
        .find(|c| c.contains.iter().any(|x| low.contains(x.as_str())) || w.iter().any(|x| c.words.contains(x)))
        .map_or("", |c| c.key.as_str())
}

/// A system's name less the words for a run's part in its service (`FP-MAIN` → `fp`); the
/// whole name when it is nothing else.
pub fn base_name(name: &str) -> String {
    let name: String = name.chars().take(MAX_NAME).collect();
    let all = words(&name);
    let roles = &rules().roles;
    let kept: Vec<&str> = all.iter().filter(|w| !roles.contains(w)).map(String::as_str).collect();
    if kept.is_empty() { all.join("-") } else { kept.join("-") }
}

/// The service a system is part of: systems with the same key are one service, and their runs
/// join where they meet. The kind, then what it carries and its circuit (pipes), its duct type
/// (ducts), else its name less its part's words.
pub fn service_key(sy: &MepSystem) -> String {
    let kind = kind_str(sy.kind);
    let name: String = sy.name.chars().take(MAX_NAME).collect();
    match sy.kind {
        MepKind::Pipe => {
            let service = crate::pipeservice::pipe_service(&name, &sy.layer);
            if !service.is_empty() && !rules().loose_services.iter().any(|s| s == service) {
                return format!("{kind}/carries:{service}/{}", circuit(&name));
            }
        }
        MepKind::Duct => {
            let ty = match sy.duct_type {
                Some(DuctType::Supply) => "supply",
                Some(DuctType::Return) => "return",
                Some(DuctType::Exhaust) => "exhaust",
                Some(DuctType::OutsideAir) => "outside_air",
                Some(DuctType::Other) | None => "",
            };
            if !ty.is_empty() {
                return format!("{kind}/type:{ty}");
            }
        }
        MepKind::Conduit | MepKind::Tray | MepKind::Cable => {}
    }
    format!("{kind}/name:{}", base_name(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sys(name: &str, layer: &str, kind: MepKind, duct_type: Option<DuctType>) -> MepSystem {
        MepSystem { name: name.into(), layer: layer.into(), kind, size_in: [1.0, 0.0], bottom_ft: 9.0, show: true, duct_type, color_index: 0 }
    }

    fn pipe(name: &str) -> String {
        service_key(&sys(name, "P-PIPE", MepKind::Pipe, None))
    }

    #[test]
    fn the_rules_load() {
        assert!(rules().roles.len() >= 10 && rules().circuits.len() >= 4, "{:?}", rules());
    }

    /// A main, its branches and its riser are one service, whatever each is named.
    #[test]
    fn mains_branches_and_risers_of_one_service_tie() {
        for group in [
            &["FP-MAIN", "FP", "Sprinkler Branch", "FIRE SPRINKLER MAIN", "Standpipe Riser"][..],
            &["CW", "CW-MAIN", "Domestic Cold Water Branch", "DCW RISER"],
            &["HW", "HWS", "HW-MAIN"],
            &["HWR", "Hot Water Recirc", "HWR-BR"],
            &["SAN", "Waste Main", "SAN RISER"],
            &["CHWS", "CHW", "CHW-MAIN"],
            &["CHWR", "Chilled Water Return Branch"],
        ] {
            let keys: Vec<String> = group.iter().map(|n| pipe(n)).collect();
            assert!(keys.iter().all(|k| *k == keys[0]), "{group:?}: {keys:?}");
        }
    }

    /// Different services, and the circuits of one service that never connect, stay apart.
    #[test]
    fn other_services_and_circuits_stay_apart() {
        for (a, b) in [
            ("SAN", "V"),
            ("CW", "HW"),
            ("HW", "HWR"),
            ("CHWS", "CHWR"),
            ("RS-1", "RL-1"),
            ("FP", "SAN"),
            ("Fire Sprinkler Wet", "Fire Sprinkler Dry"),
            ("Storm Drain", "Overflow Drain"),
            ("G", "CW"),
        ] {
            assert_ne!(pipe(a), pipe(b), "{a} / {b}");
        }
        // A pipe nothing names that is mechanical by its layer only: by its name, not tied to
        // every other unnamed mechanical pipe.
        let a = service_key(&sys("P1", "M-PIPE", MepKind::Pipe, None));
        let b = service_key(&sys("P2", "M-PIPE", MepKind::Pipe, None));
        assert_ne!(a, b);
        let c = service_key(&sys("P1-MAIN", "M-PIPE", MepKind::Pipe, None));
        assert_eq!(a, c);
    }

    /// Ducts by their duct type; without one, by their name less its part's words. Other kinds
    /// by name, never with a pipe.
    #[test]
    fn ducts_by_type_and_others_by_name() {
        let d = |n: &str, t| service_key(&sys(n, n, MepKind::Duct, t));
        assert_eq!(d("SA-MAIN", Some(DuctType::Supply)), d("SA-BRANCH", Some(DuctType::Supply)));
        assert_ne!(d("SA", Some(DuctType::Supply)), d("RA", Some(DuctType::Return)));
        assert_eq!(d("M-DUCT-SUPP", None), d("M-DUCT-SUPP-MAIN", None));
        assert_ne!(d("M-DUCT-SUPP", None), d("M-DUCT-RETN", None));
        assert_ne!(d("FP", None), pipe("FP"));
        let c = |n: &str| service_key(&sys(n, n, MepKind::Conduit, None));
        assert_eq!(c("E-POWER"), c("E-POWER-BRANCH"));
        assert_ne!(c("E-POWER"), c("E-FA"));
        assert_eq!(base_name("MAIN"), "main", "a name that is only a part word is itself");
    }
}
