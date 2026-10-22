//! What one object costs (Q250): every object of a large drawing is an [`Entity`] in its own
//! allocation, so its size is the drawing's size per object. Rare data lives out of line: large,
//! rarely numerous kinds boxed in [`EntityKind`], extended data, annotation scales and material
//! in [`Rare`] fields, and the names an object refers to (layer, linetype, plot style) kept in
//! place, not allocated.

use cadaddy_doc::{Common, Entity, EntityKind, Insert, MText, Name, Rare, RawTag, Text};

#[test]
fn an_object_stays_small() {
    // Before: Entity 424 bytes (Common 200, EntityKind 216).
    assert!(std::mem::size_of::<EntityKind>() <= 144, "EntityKind is {} bytes", std::mem::size_of::<EntityKind>());
    assert!(std::mem::size_of::<Common>() <= 152, "Common is {} bytes", std::mem::size_of::<Common>());
    assert!(std::mem::size_of::<Entity>() <= 304, "Entity is {} bytes", std::mem::size_of::<Entity>());
    // Text is the largest kind kept in place; the others must not grow past it.
    assert!(std::mem::size_of::<Text>() <= 144);
    assert!(std::mem::size_of::<MText>() <= 144 && std::mem::size_of::<Insert>() <= 144);
}

#[test]
fn names_are_kept_in_place() {
    let c = Common::default();
    for n in [&c.layer, &c.linetype, &c.plot_style] {
        assert!(!n.is_heap_allocated(), "{n}");
    }
    // A long layer name (up to 23 bytes) still isn't an allocation of its own.
    let long: Name = "A-WALL-FULL-HEIGHT-EXTR".into();
    assert_eq!(long.len(), 23);
    assert!(!long.is_heap_allocated());
    // Longer ones are shared by every copy.
    let longer: Name = "A-WALL-FULL-HEIGHT-EXTERIOR".into();
    assert!(longer.is_heap_allocated());
    assert_eq!(longer.clone().as_str(), "A-WALL-FULL-HEIGHT-EXTERIOR");
}

#[test]
fn rare_fields_read_as_their_values_and_keep_the_file_form() {
    let mut c = Common::default();
    assert!(c.xdata.is_empty() && c.anno_scales.is_empty() && c.material.is_none());
    let json = serde_json::to_value(&c).unwrap();
    assert!(json.get("xdata").is_none() && json.get("material").is_none() && json.get("annoScales").is_none(), "{json}");
    c.xdata.push(RawTag { code: 1001, value: "SAMPLE_APP".into() });
    c.material = Some("Brick".to_string()).into();
    let json = serde_json::to_value(&c).unwrap();
    assert_eq!(json["xdata"][0]["value"], "SAMPLE_APP");
    assert_eq!(json["material"], "Brick");
    let back: Common = serde_json::from_value(json).unwrap();
    assert_eq!(back, c);
    assert_eq!(back.material.as_deref(), Some("Brick"));
    // Set and emptied again equals never set.
    c.xdata.clear();
    c.material = Rare::default();
    assert_eq!(c, Common::default());
}
