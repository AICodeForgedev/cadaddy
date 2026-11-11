//! What a pipe system carries and whose trade it is (Q6492), by the system's name and the layer
//! its runs are drawn on. The rules are data (`pipe_services.json`): refrigerant, condensate and
//! hydronic piping are the mechanical trade's, domestic water, waste, vent, storm and gas the
//! plumber's, sprinkler and standpipe fire protection's. The model tree, the clash trades and the
//! takeoff all ask here, so one pipe is in one trade everywhere.

use std::sync::OnceLock;

use serde::Deserialize;

/// Most services, words and layer rules read from the data.
const MAX_RULES: usize = 64;
const MAX_WORDS: usize = 64;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Service {
    key: String,
    trade: String,
    contains: Vec<String>,
    words: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ByLayer {
    service: String,
    contains: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MechanicalLayer {
    disciplines: Vec<String>,
    by_layer: Vec<ByLayer>,
    otherwise: Service,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    services: Vec<Service>,
    unnamed_trade: String,
    mechanical_layer: MechanicalLayer,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut r: Rules = serde_json::from_str(include_str!("pipe_services.json")).unwrap_or_default();
        r.services.truncate(MAX_RULES);
        for s in &mut r.services {
            s.contains.truncate(MAX_WORDS);
            s.words.truncate(MAX_WORDS);
        }
        r.mechanical_layer.by_layer.truncate(MAX_RULES);
        r
    })
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).collect()
}

/// The service a system's name says (`refrigerant`, `hydronic`, `cold`, …), if any.
fn by_name(name: &str) -> Option<&'static Service> {
    let low = name.to_ascii_lowercase();
    let w = words(name);
    rules().services.iter().find(|s| s.contains.iter().any(|c| low.contains(c.as_str())) || w.iter().any(|x| s.words.contains(x)))
}

fn service(key: &str) -> Option<&'static Service> {
    let r = rules();
    r.services.iter().chain(std::iter::once(&r.mechanical_layer.otherwise)).find(|s| s.key == key)
}

/// Whether `layer` is a mechanical discipline's (its first part, `M-…`).
fn mechanical_layer(layer: &str) -> bool {
    let up = layer.trim().to_ascii_uppercase();
    let disc = up.split(['-', '_', ' ']).next().unwrap_or_default();
    !disc.is_empty() && rules().mechanical_layer.disciplines.iter().any(|d| d.eq_ignore_ascii_case(disc))
}

/// What a pipe system carries, by its name, else (a name that says nothing) by the mechanical
/// layer its runs are drawn on: a service key of the data (`fire`, `refrigerant`, `hydronic`,
/// `condensate`, `mechanical`, `sanitary`, `vent`, `storm`, `gas`, `hot`, `cold`), or "" for a
/// pipe nothing names.
pub fn pipe_service(name: &str, layer: &str) -> &'static str {
    if let Some(s) = by_name(name) {
        return s.key.as_str();
    }
    if !mechanical_layer(layer) {
        return "";
    }
    let m = &rules().mechanical_layer;
    let up = layer.to_ascii_uppercase();
    m.by_layer
        .iter()
        .find(|b| b.contains.iter().any(|c| up.contains(c.as_str())))
        .and_then(|b| service(&b.service))
        .map_or(m.otherwise.key.as_str(), |s| s.key.as_str())
}

/// The trade a pipe service is installed by: `mechanical`, `plumbing` or `fire` ("" and an
/// unknown key: the data's trade for a pipe nothing names).
pub fn service_trade(key: &str) -> &'static str {
    match service(key).map(|s| s.trade.as_str()) {
        Some("mechanical") => "mechanical",
        Some("fire") => "fire",
        Some(_) => "plumbing",
        None => match rules().unnamed_trade.as_str() {
            "mechanical" => "mechanical",
            "fire" => "fire",
            _ => "plumbing",
        },
    }
}

/// The trade a pipe system is installed by, by its name and layer.
pub fn pipe_trade(name: &str, layer: &str) -> &'static str {
    service_trade(pipe_service(name, layer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_load() {
        assert!(rules().services.len() >= 10, "{:?}", rules());
        assert_eq!(rules().mechanical_layer.otherwise.key, "mechanical");
    }

    #[test]
    fn mechanical_piping_by_name() {
        for (name, want) in [
            ("RS-HP-1", "refrigerant"),
            ("RL-2", "refrigerant"),
            ("HG", "refrigerant"),
            ("REFRIGERANT SUCTION", "refrigerant"),
            ("Refrigerant Liquid", "refrigerant"),
            ("R-410A", "refrigerant"),
            ("CD", "condensate"),
            ("Condensate Drain", "condensate"),
            ("CHW", "hydronic"),
            ("CHWR-1", "hydronic"),
            ("HHW", "hydronic"),
            ("Heating Hot Water Supply", "hydronic"),
            ("Chilled Glycol", "hydronic"),
        ] {
            assert_eq!(pipe_service(name, ""), want, "{name}");
            assert_eq!(pipe_trade(name, "P-PIPE"), "mechanical", "{name}");
        }
    }

    #[test]
    fn plumbing_and_fire_stay_theirs() {
        for (name, want, trade) in [
            ("CW", "cold", "plumbing"),
            ("Domestic Cold Water", "cold", "plumbing"),
            ("HW", "hot", "plumbing"),
            ("HWR", "hot", "plumbing"),
            ("SAN", "sanitary", "plumbing"),
            ("Storm Drain", "storm", "plumbing"),
            ("NG", "gas", "plumbing"),
            ("VTR", "vent", "plumbing"),
            ("Fire Sprinkler Main", "fire", "fire"),
            ("PIPE-8", "", "plumbing"),
        ] {
            assert_eq!(pipe_service(name, "P-PIPE"), want, "{name}");
            assert_eq!(pipe_trade(name, "P-PIPE"), trade, "{name}");
        }
    }

    #[test]
    fn a_mechanical_layer_makes_the_pipe_mechanical() {
        // A name that says nothing: the layer's words, else mechanical piping.
        assert_eq!(pipe_service("PIPE-7", "M-REFR"), "refrigerant");
        assert_eq!(pipe_service("PIPE-7", "M-HYDR-SUPP"), "hydronic");
        assert_eq!(pipe_service("PIPE-7", "M_CNDS"), "condensate");
        assert_eq!(pipe_service("PIPE-7", "M-PIPE"), "mechanical");
        assert_eq!(pipe_trade("PIPE-7", "M-PIPE"), "mechanical");
        // A name that says what it carries keeps it, whatever the layer.
        assert_eq!(pipe_service("RS-1", "M-HYDR"), "refrigerant");
        assert_eq!(pipe_service("CW", "M-CW"), "cold");
        assert_eq!(pipe_service("GAS", "M-PIPE"), "gas");
        assert_eq!(pipe_trade("SAN", "M-PIPE"), "plumbing");
        assert_eq!(pipe_trade("FP", "M-PIPE"), "fire");
        // A layer of another discipline with the same words doesn't count.
        assert_eq!(pipe_service("PIPE-7", "E-COND"), "");
        assert_eq!(pipe_service("PIPE-7", "MEP"), "");
    }
}
