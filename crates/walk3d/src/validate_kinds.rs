//! The element kinds a CADaddy model holds, for the agent's validation rules (Q6114): every
//! `applies_to` kind of a rule must be one of these, and every BIM category one that
//! [`crate::fourd::element_of`] writes. Each kind names what makes it, and the tests below
//! check each one against that producer — the build's part tags, the MEP generator's kinds
//! ([`crate::MepKind`], scheduled equipment, terminals), the MEP items ([`crate::ItemKind`]) and
//! the plan's device blocks ([`crate::validate::mounting_of`] knows where they hang).

use std::collections::BTreeSet;

use cadaddy_doc::{Common, Entity, EntityKind, Handle, Point, RawTag};
use cadaddy_geom::Vec3;

/// What makes an element of a kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Producer {
    /// The model build (`CADADDY_WALLGEN` part: wall, door, ceiling…).
    Build,
    /// A generated MEP run (its [`crate::MepKind`]).
    Run,
    /// A generated electrical MEP element (its `kind:` tag).
    Electrical,
    /// Scheduled MEP equipment (`part:equipment:<kind>`).
    Equipment,
    /// A generated MEP terminal (`part:terminal:<kind>`).
    Terminal,
    /// An MEP item placed from the plan (its [`crate::ItemKind`]).
    Item,
    /// A device or fixture block on the plan (its type attribute or block name).
    Device,
}

impl Producer {
    pub fn name(self) -> &'static str {
        match self {
            Producer::Build => "build",
            Producer::Run => "run",
            Producer::Electrical => "electrical",
            Producer::Equipment => "equipment",
            Producer::Terminal => "terminal",
            Producer::Item => "item",
            Producer::Device => "device",
        }
    }
}

/// The element kinds of a model, with what makes each one.
pub const MODEL_KINDS: &[(&str, Producer)] = &[
    // The build.
    ("wall", Producer::Build),
    ("door", Producer::Build),
    ("window", Producer::Build),
    ("floor", Producer::Build),
    ("ceiling", Producer::Build),
    ("room", Producer::Build),
    ("column", Producer::Build),
    ("stair", Producer::Build),
    ("roof", Producer::Build),
    // MEP runs.
    ("duct", Producer::Run),
    ("pipe", Producer::Run),
    ("conduit", Producer::Run),
    ("tray", Producer::Run),
    ("cable", Producer::Run),
    // Generated electrical elements.
    ("light_fixture", Producer::Electrical),
    ("outlet", Producer::Electrical),
    ("receptacle", Producer::Electrical),
    ("data_outlet", Producer::Electrical),
    ("floor_box", Producer::Electrical),
    ("switch", Producer::Electrical),
    ("disconnect", Producer::Electrical),
    ("junction_box", Producer::Electrical),
    ("panel", Producer::Electrical),
    ("transformer", Producer::Electrical),
    // Scheduled equipment.
    ("ahu", Producer::Equipment),
    ("rtu", Producer::Equipment),
    ("vav", Producer::Equipment),
    ("exhaust_fan", Producer::Equipment),
    ("fan", Producer::Equipment),
    ("pump", Producer::Equipment),
    ("water_heater", Producer::Equipment),
    ("condensing_unit", Producer::Equipment),
    // Terminals.
    ("sprinkler", Producer::Terminal),
    ("diffuser", Producer::Terminal),
    ("grille", Producer::Terminal),
    // Items placed from the plan.
    ("fixture", Producer::Item),
    // Device blocks.
    ("luminaire", Producer::Device),
    ("panelboard", Producer::Device),
    ("smoke_detector", Producer::Device),
    ("heat_detector", Producer::Device),
    ("horn_strobe", Producer::Device),
    ("strobe_wall", Producer::Device),
    ("speaker", Producer::Device),
    ("pull_station", Producer::Device),
    ("exit_sign", Producer::Device),
    ("thermostat", Producer::Device),
];

/// Is `kind` a kind the model holds?
pub fn is_model_kind(kind: &str) -> bool {
    MODEL_KINDS.iter().any(|(k, _)| k.eq_ignore_ascii_case(kind.trim()))
}

/// Kind families (Q6131): a rule naming a family applies to every kind in it, so the rules
/// match the kinds the PDF→BIM job builds (`plumbing_fixture`, `return_grille`,
/// `fire_alarm_notification`…) without each rule listing them. A member may be a family itself
/// (`terminal` ⊇ `grille` ⊇ `return_grille`). This is the one table every rule's `applies_to`
/// matching uses ([`kind_matches`]).
pub const KIND_FAMILIES: &[(&str, &[&str])] = &[
    ("terminal", &["diffuser", "grille", "register", "air_terminal"]),
    ("diffuser", &["supply_diffuser", "linear_diffuser", "slot_diffuser", "ceiling_diffuser"]),
    ("grille", &["return_grille", "exhaust_grille", "supply_grille", "transfer_grille"]),
    ("register", &["supply_register", "return_register", "exhaust_register"]),
    (
        "fixture",
        &[
            "plumbing_fixture",
            "lavatory",
            "water_closet",
            "toilet",
            "urinal",
            "sink",
            "bathtub",
            "tub",
            "shower",
            "mop_sink",
            "service_sink",
            "drinking_fountain",
            "bidet",
        ],
    ),
    ("fire_alarm_device", &["smoke_detector", "heat_detector", "duct_detector", "fire_alarm_notification", "pull_station", "fire_alarm_speaker"]),
    ("fire_alarm_notification", &["horn_strobe", "horn", "strobe", "strobe_wall", "strobe_ceiling", "speaker_strobe"]),
    (
        "receptacle",
        &[
            "gfci_receptacle",
            "ceiling_receptacle",
            "quad_receptacle",
            "duplex_receptacle",
            "usb_receptacle",
            "weatherproof_receptacle",
            "ptac_receptacle",
        ],
    ),
    ("data_outlet", &["tv_outlet", "av_outlet", "telephone_outlet", "catv_outlet"]),
    ("switch", &["3way_switch", "three_way_switch", "4way_switch", "dimmer_switch", "dimmer"]),
    ("occupancy_sensor", &["vacancy_sensor", "motion_sensor", "wall_occupancy_sensor", "ceiling_occupancy_sensor"]),
    ("fan", &["exhaust_fan", "inline_fan", "supply_fan"]),
    ("exhaust_fan", &["ceiling_exhaust_fan", "bath_exhaust_fan", "inline_exhaust_fan"]),
    ("ptac", &["pthp", "through_wall_unit", "ptac_unit", "ptac_sleeve"]),
    ("luminaire", &["light_fixture", "wall_luminaire", "downlight", "sconce"]),
    ("panel", &["panelboard", "load_center"]),
    // A fan coil unit is mechanical equipment (Q6140), not a fan: its words hold `fan`, so it
    // is named here to be read as itself.
    ("fan_coil_unit", &["fan_coil", "fcu", "cassette_fan_coil", "vertical_fan_coil", "horizontal_fan_coil"]),
];

/// The BIM category (IFC class) of a kind the job builds that no producer above tags, when its
/// own words would read as another kind's (`fan_coil_unit` holds `fan`, but it is no IfcFan).
pub const KIND_CATEGORIES: &[(&str, &str)] = &[("fan_coil_unit", "IfcUnitaryEquipment")];

/// Kinds the PDF→BIM job builds that no producer above makes and no family holds (a rule may
/// name them).
pub const JOB_KINDS: &[&str] = &["equipment_connection"];

/// How deep families nest, at most (the table is checked to be shallower).
const MAX_FAMILY_DEPTH: usize = 6;
/// Longest kind read.
const MAX_KIND: usize = 200;

/// Lower case, words joined by `_`.
fn norm(s: &str) -> String {
    let t: String = s.trim().chars().take(MAX_KIND).map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    t.split('_').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("_")
}

fn in_family_at(kind: &str, family: &str, depth: usize) -> bool {
    if kind == family {
        return true;
    }
    if depth >= MAX_FAMILY_DEPTH {
        return false;
    }
    KIND_FAMILIES
        .iter()
        .filter(|(f, _)| *f == family)
        .any(|(_, members)| members.iter().any(|m| *m == kind || (*m != family && in_family_at(kind, m, depth + 1))))
}

/// Is `kind` the kind `family` or in it (through nested families)? Both are compared as words
/// (`Return Grille` = `return_grille`).
pub fn in_family(kind: &str, family: &str) -> bool {
    in_family_at(&norm(kind), &norm(family), 0)
}

/// Is `kind` one the family table names (a family or a member), or one of [`JOB_KINDS`]?
pub fn is_family_kind(kind: &str) -> bool {
    let k = norm(kind);
    JOB_KINDS.contains(&k.as_str()) || KIND_FAMILIES.iter().any(|(f, members)| *f == k || members.contains(&k.as_str()))
}

/// A kind a rule's `applies_to` may name: one the model holds or one of the family table.
pub fn is_rule_kind(kind: &str) -> bool {
    is_model_kind(kind) || is_family_kind(kind)
}

/// Every kind that is known: the model's and the family table's.
fn known_kinds() -> impl Iterator<Item = &'static str> {
    MODEL_KINDS
        .iter()
        .map(|(k, _)| *k)
        .chain(JOB_KINDS.iter().copied())
        .chain(KIND_FAMILIES.iter().flat_map(|(f, m)| std::iter::once(*f).chain(m.iter().copied())))
}

/// The known kind an element's kind is: itself when known, else the longest known kind its
/// words hold in a row (`ceiling_mounted_smoke_detector` → `smoke_detector`); none when its
/// words hold no known kind.
pub fn known_kind(kind: &str) -> Option<String> {
    let k = norm(kind);
    if k.is_empty() {
        return None;
    }
    if is_rule_kind(&k) {
        return Some(k);
    }
    let w = format!("_{k}_");
    known_kinds().filter(|x| w.contains(&format!("_{x}_"))).max_by_key(|x| (x.len(), std::cmp::Reverse(*x))).map(str::to_string)
}

/// Does an element of `kind` get a rule whose `applies_to` names `rule_kind` (`*`: every
/// element)? Only by the family table: a known kind gets the rules of its own kind and of every
/// family it is in (`plumbing_fixture` gets `fixture`'s; `light_fixture` does not, it is no
/// plumbing fixture); a kind the table does not know is read as the known kind its words hold
/// ([`known_kind`]: `gfci duplex receptacle` gets `receptacle`'s).
pub fn kind_matches(rule_kind: &str, kind: &str) -> bool {
    let r = norm(rule_kind);
    if rule_kind.trim() == "*" {
        return true;
    }
    if r.is_empty() {
        return false;
    }
    known_kind(kind).is_some_and(|k| in_family_at(&k, &r, 0))
}

/// A made-up entity tagged the way the model build and the MEP generator tag theirs.
fn tagged(part: &str, extras: &[String]) -> Entity {
    let mut x = vec![RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }];
    for v in [part, "L1", "0"].into_iter().map(str::to_string).chain(extras.iter().cloned()) {
        x.push(RawTag { code: 1000, value: v });
    }
    let common = Common { xdata: x.into(), ..Common::default() };
    Entity { handle: Handle(1), common, kind: EntityKind::Point(Point { p: Vec3::new(0.0, 0.0, 0.0), angle: 0.0 }) }
}

/// The entity the producer of `kind` would make, if it is one IFC export classifies.
fn made(kind: &str, by: Producer) -> Option<Entity> {
    let ex = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Some(match by {
        Producer::Build => tagged(kind, &[]),
        Producer::Run => tagged("mep", &ex(&[&format!("kind:{kind}"), "part:segment"])),
        Producer::Electrical => tagged("mep", &ex(&[&format!("kind:{kind}"), "part:device"])),
        Producer::Equipment => tagged("mep", &ex(&["kind:equipment", &format!("part:equipment:{kind}"), "tag:X-1"])),
        Producer::Terminal => tagged("mep", &ex(&["kind:duct", &format!("part:terminal:{kind}")])),
        Producer::Item | Producer::Device => return None,
    })
}

/// The IFC class an element of `kind` is written as, when its producer tags it.
pub fn category_of(kind: &str) -> Option<String> {
    let Some((k, by)) = MODEL_KINDS.iter().find(|(k, _)| k.eq_ignore_ascii_case(kind.trim())) else {
        // A kind of the family table with a category of its own, or a member of one.
        let n = norm(kind);
        return KIND_CATEGORIES.iter().find(|(f, _)| in_family_at(&n, f, 0)).map(|(_, c)| c.to_string());
    };
    made(k, *by).map(|e| crate::fourd::element_of(&e).class)
}

/// Every BIM category (IFC class) the model's elements are written as.
pub fn model_categories() -> BTreeSet<String> {
    let mut out: BTreeSet<String> = MODEL_KINDS.iter().filter_map(|(k, _)| category_of(k)).collect();
    // Fittings of each run kind.
    for k in ["duct", "pipe", "conduit"] {
        out.insert(crate::fourd::element_of(&tagged("mep", &[format!("kind:{k}"), "part:fitting:elbow".into()])).class);
    }
    out.remove("IfcBuildingElementProxy");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{Mounting, mounting_of};

    #[test]
    fn kinds_are_unique_and_lower_case() {
        let mut seen = BTreeSet::new();
        for (k, _) in MODEL_KINDS {
            assert!(seen.insert(*k), "{k} twice");
            assert_eq!(*k, k.to_ascii_lowercase());
        }
        assert!(is_model_kind(" Exhaust_Fan "));
        assert!(!is_model_kind("unicorn"));
    }

    #[test]
    fn every_kind_is_one_its_producer_makes() {
        for (k, by) in MODEL_KINDS {
            match by {
                Producer::Run => {
                    let mk: Result<crate::MepKind, _> = serde_json::from_value(serde_json::json!(k));
                    assert!(mk.is_ok(), "{k} is not an MEP run kind");
                }
                Producer::Item => {
                    let ik: Result<crate::ItemKind, _> = serde_json::from_value(serde_json::json!(k));
                    assert!(ik.is_ok(), "{k} is not an MEP item kind");
                }
                Producer::Device => assert_ne!(mounting_of(k), Mounting::Other, "{k}: Walk does not know where it hangs"),
                _ => {}
            }
            // Runs are checked by their MEP kind above (IFC export writes a loose cable as a proxy).
            if !matches!(by, Producer::Item | Producer::Device | Producer::Run) {
                let class = category_of(k).unwrap_or_default();
                assert!(class.starts_with("Ifc") && class != "IfcBuildingElementProxy", "{k} ({}) is written as {class}", by.name());
            }
        }
    }

    #[test]
    fn categories_are_the_classes_ifc_export_writes() {
        let c = model_categories();
        for want in
            ["IfcWall", "IfcFan", "IfcDuctSegment", "IfcDuctFitting", "IfcOutlet", "IfcLightFixture", "IfcFireSuppressionTerminal", "IfcAirTerminal"]
        {
            assert!(c.contains(want), "{want} missing from {c:?}");
        }
        assert_eq!(category_of("exhaust_fan").as_deref(), Some("IfcFan"));
        assert_eq!(category_of("panel").as_deref(), Some("IfcElectricDistributionBoard"));
        assert_eq!(category_of("smoke_detector"), None);
    }

    #[test]
    fn a_fan_coil_unit_is_mechanical_equipment_not_a_fan() {
        assert_eq!(known_kind("fan_coil_unit").as_deref(), Some("fan_coil_unit"));
        assert_eq!(known_kind("Fan Coil Unit").as_deref(), Some("fan_coil_unit"));
        assert!(!kind_matches("fan", "fan_coil_unit") && !kind_matches("exhaust_fan", "fan_coil_unit"));
        assert!(kind_matches("fan_coil_unit", "fcu") && kind_matches("fan_coil_unit", "vertical_fan_coil"));
        assert_eq!(category_of("fan_coil_unit").as_deref(), Some("IfcUnitaryEquipment"));
        assert_eq!(category_of("fcu").as_deref(), Some("IfcUnitaryEquipment"));
        assert!(model_categories().contains("IfcUnitaryEquipment"));
        // A fan is still a fan.
        assert!(kind_matches("fan", "inline_fan"));
        assert_eq!(category_of("fan").as_deref(), Some("IfcFan"));
    }

    #[test]
    fn families_hold_the_kinds_the_job_builds() {
        // The spec's pairs: fixture ⊇ plumbing_fixture; terminal ⊇ diffuser, grille, return_grille, exhaust_grille.
        assert!(kind_matches("fixture", "plumbing_fixture"));
        for k in ["diffuser", "grille", "return_grille", "exhaust_grille", "Supply Diffuser"] {
            assert!(kind_matches("terminal", k), "{k}");
        }
        assert!(kind_matches("grille", "return_grille") && !kind_matches("diffuser", "return_grille"));
        assert!(kind_matches("fire_alarm_device", "horn_strobe"), "through fire_alarm_notification");
        assert!(kind_matches("fire_alarm_device", "smoke_detector") && kind_matches("fire_alarm_device", "fire_alarm_device"));
        assert!(kind_matches("fan", "exhaust_fan") && kind_matches("fan", "ceiling_exhaust_fan"));
        assert!(kind_matches("receptacle", "gfci_receptacle") && kind_matches("receptacle", "GFCI Receptacle"));
        assert!(kind_matches("ptac", "through_wall_unit") && kind_matches("occupancy_sensor", "vacancy_sensor"));
        // A known kind is no member of a family it only shares a word with.
        assert!(!kind_matches("fixture", "light_fixture"), "a light fixture is no plumbing fixture");
        assert!(!kind_matches("receptacle", "data_outlet") && !kind_matches("outlet", "data_outlet"));
        assert!(!kind_matches("duct", "pipe") && !kind_matches("switch", "disconnect") && !kind_matches("fire_alarm_device", "conduit"));
        // An unknown kind is read as the known kind its words hold.
        assert_eq!(known_kind("ceiling mounted smoke detector").as_deref(), Some("smoke_detector"));
        assert!(kind_matches("receptacle", "duplex_receptacle_20a") && kind_matches("fire_alarm_device", "addressable_horn_strobe"));
        assert!(!kind_matches("receptacle", "unicorn") && known_kind("").is_none());
        assert!(kind_matches("*", "unicorn") && !kind_matches("", "receptacle"));
    }

    #[test]
    fn the_family_table_is_well_formed() {
        let mut seen = BTreeSet::new();
        for (f, members) in KIND_FAMILIES {
            assert!(seen.insert(*f), "family {f} twice");
            assert_eq!(norm(f), *f);
            assert!(!members.is_empty(), "{f}");
            for m in *members {
                assert_eq!(norm(m), *m, "{m}");
                assert_ne!(m, f, "{f} holds itself");
                // No cycles: a member never holds its family.
                assert!(!in_family(f, m), "{f} ⊂ {m} ⊂ {f}");
            }
        }
        // Every family resolves within the depth cap.
        for (f, _) in KIND_FAMILIES {
            for k in known_kinds() {
                let _ = in_family(k, f);
            }
        }
        assert!(is_rule_kind("terminal") && is_rule_kind("receptacle") && !is_rule_kind("unicorn"));
    }
}
