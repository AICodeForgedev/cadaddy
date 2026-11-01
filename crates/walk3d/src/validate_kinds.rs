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
    let (k, by) = MODEL_KINDS.iter().find(|(k, _)| k.eq_ignore_ascii_case(kind.trim()))?;
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
}
